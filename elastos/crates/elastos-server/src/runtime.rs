//! Core runtime implementation

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::RwLock;

use elastos_common::{CapsuleManifest, CapsuleType, ElastosError, Result};
use elastos_compute::providers::{BridgeHostcall, BridgeSpawner, WasmProvider};
use elastos_compute::{CapsuleHandle, ComputeProvider};
use elastos_storage::StorageProvider;

use elastos_runtime::provider::ProviderRegistry;
use elastos_runtime::signature::{hash_content, key_fingerprint, SignatureVerifier};

/// Information about a running capsule (for API responses and lifecycle management)
#[derive(Debug, Clone)]
pub struct RunningCapsuleInfo {
    /// Unique capsule instance ID
    pub id: String,
    /// Capsule name from manifest
    pub name: String,
    /// Current status (running, stopped, etc.)
    pub status: String,
    /// Capsule type (WASM, MicroVM)
    pub capsule_type: CapsuleType,
    /// Manifest the capsule was launched with. Retained so introspection
    /// surfaces (the inspect provider) can project capabilities, affordances,
    /// and provenance without re-reading capsule.json. Boxed to keep the
    /// struct lean.
    pub manifest: Box<CapsuleManifest>,
    /// Handle for stopping the capsule (optional, not all capsules have handles)
    pub handle: Option<CapsuleHandle>,
    /// The HONEST verified-signer fingerprint, set at launch ONLY when the configured
    /// verifier's real ed25519 check matched a trusted key (G2b). `None` when signature
    /// verification was disabled (no trusted keys), the capsule was unsigned, or no
    /// trusted key matched — so the inspector reports "verified" trust strictly behind a
    /// genuine signature check, never from mere signature presence.
    pub verified_signer: Option<String>,
}

/// The main ElastOS runtime
pub struct Runtime {
    // Keep the storage provider alive for runtime/lifecycle ownership even
    // before direct storage API calls are exposed from this layer.
    _storage: Arc<dyn StorageProvider>,
    /// Compute providers (WASM, crosvm, etc.)
    compute_providers: Vec<Arc<dyn ComputeProvider>>,
    /// Signature verifier for capsule integrity
    signature_verifier: RwLock<SignatureVerifier>,
    /// Dev/source mode allows unsigned local capsules. Release paths should set
    /// this false and configure trusted keys.
    signature_dev_mode: RwLock<bool>,
    /// Provider registry (optional, set when server mode is active)
    provider_registry: RwLock<Option<Arc<ProviderRegistry>>>,
    /// Registry of running capsules (for API queries)
    running_capsules: RwLock<HashMap<String, RunningCapsuleInfo>>,
    /// Reference to the concrete WASM provider for bridge configuration
    wasm_provider: Option<Arc<WasmProvider>>,
}

impl Runtime {
    /// Create a new runtime with a single compute provider (backward compatible)
    pub fn new(storage: Arc<dyn StorageProvider>, compute: Arc<dyn ComputeProvider>) -> Self {
        Self {
            _storage: storage,
            compute_providers: vec![compute],
            signature_verifier: RwLock::new(SignatureVerifier::new()),
            signature_dev_mode: RwLock::new(false),
            provider_registry: RwLock::new(None),
            running_capsules: RwLock::new(HashMap::new()),
            wasm_provider: None,
        }
    }

    /// Create a new runtime with multiple compute providers and an optional
    /// reference to the concrete WasmProvider for bridge configuration.
    pub fn with_providers(
        storage: Arc<dyn StorageProvider>,
        compute_providers: Vec<Arc<dyn ComputeProvider>>,
        wasm_provider: Option<Arc<WasmProvider>>,
    ) -> Self {
        Self {
            _storage: storage,
            compute_providers,
            signature_verifier: RwLock::new(SignatureVerifier::new()),
            signature_dev_mode: RwLock::new(false),
            provider_registry: RwLock::new(None),
            running_capsules: RwLock::new(HashMap::new()),
            wasm_provider,
        }
    }

    /// Set the provider registry and capability manager for provider dispatch
    /// and real token minting.
    ///
    /// Also configures the WASM bridge spawner so WASM capsules can
    /// communicate with providers via piped stdio.
    pub async fn set_provider_registry(
        &self,
        registry: Arc<ProviderRegistry>,
        capability_manager: Arc<elastos_runtime::capability::CapabilityManager>,
        pending_store: Arc<elastos_runtime::capability::pending::PendingRequestStore>,
        data_dir: std::path::PathBuf,
        spend_policy: Option<crate::carrier_bridge::SpendPolicy>,
    ) {
        // Accepted to keep the call-site API stable while WASM-path spend metering is re-wired
        // onto the 0.5 base (tracked follow-up). The BridgeContext sites below still pass `None`;
        // when the wiring lands, thread this value through instead of dropping it here.
        let _ = &spend_policy;
        // Configure WASM bridge if we have a concrete WasmProvider reference
        if let Some(ref wasm) = self.wasm_provider {
            let reg = registry.clone();
            let cap_mgr = capability_manager.clone();
            let pending = pending_store.clone();
            let bridge_data_dir = data_dir.clone();
            wasm.set_bridge_spawner(Arc::new(move |pipes| {
                let ctx = crate::carrier_bridge::BridgeContext {
                    provider_registry: reg.clone(),
                    capability_manager: cap_mgr.clone(),
                    pending_store: pending.clone(),
                    capsule_id: pipes.capsule_id.clone(),
                    principal_id: pipes.principal_id.clone(),
                    data_dir: Some(bridge_data_dir.clone()),
                    // WASM-path spend metering is deferred on the 0.5 base (tracked follow-up); the
                    // SpendMeter primitive + inspector projection are intact, this wiring is not yet re-applied.
                    spend_policy: None,
                };
                crate::carrier_bridge::spawn_wasm_carrier_bridge(pipes, ctx);
            }));

            let host_reg = registry.clone();
            let host_cap_mgr = capability_manager.clone();
            let host_pending = pending_store.clone();
            let host_data_dir = data_dir.clone();
            let host_handle = tokio::runtime::Handle::current();
            wasm.set_bridge_hostcall(Arc::new(move |line, capsule_id, principal_id| {
                let ctx = crate::carrier_bridge::BridgeContext {
                    provider_registry: host_reg.clone(),
                    capability_manager: host_cap_mgr.clone(),
                    pending_store: host_pending.clone(),
                    capsule_id: capsule_id.to_string(),
                    principal_id: principal_id.map(ToOwned::to_owned),
                    data_dir: Some(host_data_dir.clone()),
                    spend_policy: None,
                };
                let response = host_handle
                    .block_on(crate::carrier_bridge::handle_request(line, &Some(ctx)))
                    .map_err(|err| err.to_string())?;
                serde_json::to_string(&response).map_err(|err| err.to_string())
            }));
            tracing::info!("WASM Carrier bridge configured (hostcall + FIFO fallback)");
        }

        let mut guard = self.provider_registry.write().await;
        *guard = Some(registry);
    }

    /// Override the default WASM bridge spawner.
    ///
    /// Used by attached-runtime WASM execution so the local WASM process can
    /// keep terminal ownership while forwarding bridge traffic to the running
    /// runtime daemon.
    pub fn set_wasm_bridge_spawner(&self, spawner: BridgeSpawner) {
        if let Some(ref wasm) = self.wasm_provider {
            wasm.set_bridge_spawner(spawner);
        }
    }

    /// Override the default WASM carrier host-call handler.
    ///
    /// Used by attached-runtime WASM execution to route guest calls
    /// through the already-running local runtime API.
    pub fn set_wasm_bridge_hostcall(&self, hostcall: BridgeHostcall) {
        if let Some(ref wasm) = self.wasm_provider {
            wasm.set_bridge_hostcall(hostcall);
        }
    }

    /// Find a compute provider that supports the given capsule type
    fn get_provider(&self, capsule_type: &CapsuleType) -> Option<&Arc<dyn ComputeProvider>> {
        self.compute_providers
            .iter()
            .find(|p| p.supports(capsule_type))
    }

    /// Get mutable access to the signature verifier for configuration
    pub async fn signature_verifier(&self) -> tokio::sync::RwLockWriteGuard<'_, SignatureVerifier> {
        self.signature_verifier.write().await
    }

    /// Configure capsule manifest signature verification from runtime config.
    ///
    /// `dev_mode=true` is the source-home/dev escape hatch for unsigned local
    /// capsules. With `dev_mode=false`, at least one trusted key is required
    /// and unsigned capsules fail closed.
    pub async fn configure_signature_verification(
        &self,
        trusted_keys: &[String],
        dev_mode: bool,
    ) -> Result<()> {
        let mut verifier = SignatureVerifier::new();
        for key in trusted_keys {
            verifier.add_trusted_key_hex(key)?;
        }

        if !dev_mode && !verifier.is_enabled() {
            return Err(ElastosError::InvalidManifest(
                "Capsule signature enforcement requires trusted_keys; set dev_mode = true for source-home/dev unsigned capsules".into(),
            ));
        }

        let trusted_key_count = verifier.trusted_key_count();
        *self.signature_verifier.write().await = verifier;
        *self.signature_dev_mode.write().await = dev_mode;

        if dev_mode {
            tracing::warn!(
                "Capsule signature verification running in dev_mode=true; unsigned local capsules are allowed"
            );
        } else {
            tracing::info!(
                "Capsule signature verification enforced with {} trusted key(s)",
                trusted_key_count
            );
        }

        Ok(())
    }

    /// Load and run a capsule from a local directory
    pub async fn run_local(&self, path: &Path, args: Vec<String>) -> Result<CapsuleHandle> {
        self.run_local_with_principal(path, args, None).await
    }

    /// Load and run a capsule from a local directory with an optional runtime
    /// principal for current-user storage alias resolution.
    pub async fn run_local_with_principal(
        &self,
        path: &Path,
        args: Vec<String>,
        principal_id: Option<String>,
    ) -> Result<CapsuleHandle> {
        // Read manifest
        let manifest_path = path.join("capsule.json");
        let manifest_data = tokio::fs::read_to_string(&manifest_path)
            .await
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    ElastosError::InvalidManifest(format!(
                        "capsule.json not found in {}",
                        path.display()
                    ))
                } else {
                    ElastosError::Io(e)
                }
            })?;

        let manifest: CapsuleManifest = serde_json::from_str(&manifest_data).map_err(|e| {
            ElastosError::InvalidManifest(format!("Failed to parse capsule.json: {}", e))
        })?;
        manifest.validate().map_err(ElastosError::InvalidManifest)?;

        // Reject entrypoints that escape the capsule directory
        if manifest.entrypoint.contains("..")
            || std::path::Path::new(&manifest.entrypoint).is_absolute()
        {
            return Err(ElastosError::InvalidManifest(
                "Entrypoint must be a relative path within the capsule directory".into(),
            ));
        }

        tracing::info!(
            "Loading capsule '{}' ({:?})",
            manifest.name,
            manifest.capsule_type
        );

        // Check provider dependencies
        self.check_provider_dependencies(&manifest).await?;

        // Verify signature if verifier is configured
        self.verify_capsule_signature(&manifest, path).await?;

        // Find a compute provider that supports this capsule type
        let provider = self.get_provider(&manifest.capsule_type).ok_or_else(|| {
            ElastosError::Compute(format!(
                "No compute provider supports capsule type: {:?}",
                manifest.capsule_type
            ))
        })?;

        // Load capsule
        let mut handle = provider.load(path, manifest).await?;
        if handle.manifest.capsule_type == CapsuleType::Wasm {
            if let Some(ref wasm) = self.wasm_provider {
                wasm.set_bridge_principal(&handle.id, principal_id).await;
            }
        }

        // Set args on handle before starting
        handle.args = args;

        // Start capsule
        let start = provider.start(&handle).await;
        if handle.manifest.capsule_type == CapsuleType::Wasm {
            if let Some(ref wasm) = self.wasm_provider {
                wasm.clear_bridge_principal(&handle.id).await;
            }
        }
        start?;

        Ok(handle)
    }

    /// Check that all providers required by a capsule are registered
    async fn check_provider_dependencies(&self, manifest: &CapsuleManifest) -> Result<()> {
        if let Some(ref providers) = manifest.providers {
            let registry = self.provider_registry.read().await;
            if let Some(ref registry) = *registry {
                for scheme in providers.keys() {
                    if !registry.has_provider(scheme).await {
                        return Err(ElastosError::Compute(format!(
                            "Capsule '{}' requires provider for scheme '{}' which is not registered",
                            manifest.name, scheme
                        )));
                    }
                }
            }
            // If no registry is set, skip check (CLI mode without server)
        }
        Ok(())
    }

    /// Verify capsule signature if verification is enabled
    async fn verify_capsule_signature(
        &self,
        manifest: &CapsuleManifest,
        path: &Path,
    ) -> Result<()> {
        let verifier = self.signature_verifier.read().await;

        let dev_mode = *self.signature_dev_mode.read().await;

        if manifest.signature.is_none() {
            if dev_mode {
                tracing::warn!(
                    "Unsigned capsule '{}' allowed because dev_mode=true",
                    manifest.name
                );
                return Ok(());
            }

            return Err(ElastosError::InvalidManifest(
                "Capsule is unsigned and dev_mode=false".into(),
            ));
        }

        if !verifier.is_enabled() {
            return Err(ElastosError::InvalidManifest(
                "Capsule signature verification has no trusted keys configured".into(),
            ));
        }

        // Compute content hash (hash the entrypoint file)
        let entrypoint_path = path.join(&manifest.entrypoint);
        let content = tokio::fs::read(&entrypoint_path).await.map_err(|e| {
            ElastosError::InvalidManifest(format!(
                "Failed to read entrypoint {}: {}",
                manifest.entrypoint, e
            ))
        })?;
        let content_hash = hash_content(&content);

        // Verify signature
        if !verifier.verify_capsule(manifest, &content_hash)? {
            return Err(ElastosError::InvalidManifest(
                "Capsule signature verification failed".into(),
            ));
        }

        tracing::info!("Capsule signature verified successfully");
        Ok(())
    }

    /// Resolve the HONEST verified-signer fingerprint for a capsule, for trust
    /// projection by the inspector (G2b). Thin wrapper that reads the configured
    /// verifier and delegates to [`resolve_verified_signer_with`].
    ///
    /// This is ADDITIVE display truth, NOT an enforcement gate: the fail-closed launch
    /// gate is [`Runtime::verify_capsule_signature`]. A capsule that launched with the
    /// gate ENABLED has, by construction, already passed a real check (so this returns
    /// `Some`); with the gate DISABLED (dev mode) nothing was verified, so this returns
    /// `None` and the inspector keeps reporting honest presence-based trust.
    pub async fn resolve_verified_signer(
        &self,
        manifest: &CapsuleManifest,
        path: &Path,
    ) -> Option<String> {
        let verifier = self.signature_verifier.read().await;
        resolve_verified_signer_with(&verifier, manifest, path).await
    }

    /// Stop a running capsule
    pub async fn stop(&self, handle: &CapsuleHandle) -> Result<()> {
        // Find the provider that supports this capsule type
        let provider = self
            .get_provider(&handle.manifest.capsule_type)
            .ok_or_else(|| {
                ElastosError::Compute(format!(
                    "No compute provider supports capsule type: {:?}",
                    handle.manifest.capsule_type
                ))
            })?;

        provider.stop(handle).await
    }

    /// Check if a capsule type is supported by any provider
    pub fn supports_capsule_type(&self, capsule_type: &CapsuleType) -> bool {
        self.get_provider(capsule_type).is_some()
    }

    /// Register a running capsule with the runtime
    ///
    /// This is used by external code (like main.rs for MicroVM capsules) to
    /// register capsules that weren't started through Runtime's run_* methods.
    pub async fn register_capsule(&self, info: RunningCapsuleInfo) {
        let mut capsules = self.running_capsules.write().await;
        tracing::info!("Registered capsule '{}' with ID: {}", info.name, info.id);
        capsules.insert(info.id.clone(), info);
    }

    /// Unregister a capsule from the runtime
    pub async fn unregister_capsule(&self, id: &str) {
        let mut capsules = self.running_capsules.write().await;
        if capsules.remove(id).is_some() {
            tracing::info!("Unregistered capsule: {}", id);
        }
    }

    /// List all registered capsules
    pub async fn list_capsules(&self) -> Vec<RunningCapsuleInfo> {
        let capsules = self.running_capsules.read().await;
        capsules.values().cloned().collect()
    }

    /// Get a specific capsule by ID
    pub async fn get_capsule(&self, id: &str) -> Option<RunningCapsuleInfo> {
        let capsules = self.running_capsules.read().await;
        capsules.get(id).cloned()
    }

    /// Update a capsule's status
    pub async fn update_capsule_status(&self, id: &str, status: &str) {
        let mut capsules = self.running_capsules.write().await;
        if let Some(info) = capsules.get_mut(id) {
            info.status = status.to_string();
            tracing::debug!("Updated capsule {} status to: {}", id, status);
        }
    }

    /// Stop a capsule by its ID
    ///
    /// This looks up the capsule in the registry and attempts to stop it.
    /// Returns Ok(true) if stopped, Ok(false) if not found, Err on failure.
    pub async fn stop_capsule_by_id(&self, id: &str) -> Result<bool> {
        // Get capsule info and handle
        let capsule_info = {
            let capsules = self.running_capsules.read().await;
            capsules.get(id).cloned()
        };

        let Some(info) = capsule_info else {
            return Ok(false); // Not found
        };

        // If we have a handle, try to stop via compute provider
        if let Some(handle) = &info.handle {
            // Find the provider that supports this capsule type
            if let Some(provider) = self.get_provider(&info.capsule_type) {
                tracing::info!("Stopping capsule '{}' ({})", info.name, id);
                provider.stop(handle).await?;
            }
        }

        // Update status to stopped
        {
            let mut capsules = self.running_capsules.write().await;
            if let Some(info) = capsules.get_mut(id) {
                info.status = "stopped".to_string();
            }
        }

        // Unregister the capsule
        self.unregister_capsule(id).await;

        Ok(true)
    }
}

/// Resolve the verified-signer fingerprint for a capsule against a specific verifier
/// (G2b) — the testable core behind [`Runtime::resolve_verified_signer`].
///
/// Returns `Some(fingerprint)` ONLY when ALL hold: the verifier has trusted keys
/// (`is_enabled()`), the manifest carries a signature, the entrypoint is readable, and a
/// real ed25519 check matches one of the trusted keys. Any other case — disabled
/// verifier, unsigned manifest, unreadable entrypoint, structurally bad signature, or no
/// trusted-key match — is `None`. It NEVER fabricates a signer: absence of a genuine
/// match is `None`, so a capsule can read "verified" only behind a real signature check.
pub(crate) async fn resolve_verified_signer_with(
    verifier: &SignatureVerifier,
    manifest: &CapsuleManifest,
    path: &Path,
) -> Option<String> {
    if !verifier.is_enabled() {
        return None;
    }
    // Unsigned ⇒ no verified signer (the launch gate rejects this when enabled anyway).
    manifest.signature.as_ref()?;
    let entrypoint_path = path.join(&manifest.entrypoint);
    let content = tokio::fs::read(&entrypoint_path).await.ok()?;
    let content_hash = hash_content(&content);
    // A real ed25519 check; `Err` (bad signature bytes) and `Ok(None)` (no trusted key
    // matched) both collapse to `None` — never an upgraded trust without a true match.
    let key = verifier
        .verify_capsule_signer(manifest, &content_hash)
        .ok()??;
    Some(key_fingerprint(&key))
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use elastos_common::{
        CapsuleId, CapsuleManifest, CapsuleRole, CapsuleStatus, CapsuleType, Permissions,
        ResourceLimits, SCHEMA_V1,
    };
    use elastos_compute::{CapsuleInfo, ComputeProvider};
    use elastos_storage::providers::LocalFSProvider;

    struct NoopComputeProvider;

    #[async_trait]
    impl ComputeProvider for NoopComputeProvider {
        async fn load(&self, _path: &Path, manifest: CapsuleManifest) -> Result<CapsuleHandle> {
            Ok(CapsuleHandle {
                id: CapsuleId::new("test-capsule"),
                manifest,
                args: Vec::new(),
            })
        }

        async fn start(&self, _handle: &CapsuleHandle) -> Result<()> {
            Ok(())
        }

        async fn stop(&self, _handle: &CapsuleHandle) -> Result<()> {
            Ok(())
        }

        async fn status(&self, _handle: &CapsuleHandle) -> Result<CapsuleStatus> {
            Ok(CapsuleStatus::Running)
        }

        async fn info(&self, handle: &CapsuleHandle) -> Result<CapsuleInfo> {
            Ok(CapsuleInfo {
                id: handle.id.clone(),
                name: handle.manifest.name.clone(),
                status: CapsuleStatus::Running,
                memory_used_mb: 0,
            })
        }

        fn supports(&self, _capsule_type: &CapsuleType) -> bool {
            true
        }
    }

    async fn test_runtime() -> (Runtime, tempfile::TempDir) {
        let temp_dir = tempfile::tempdir().unwrap();
        let storage = Arc::new(
            LocalFSProvider::new(temp_dir.path().to_path_buf())
                .await
                .unwrap(),
        );
        let compute = Arc::new(NoopComputeProvider);
        (Runtime::new(storage, compute), temp_dir)
    }

    fn unsigned_manifest() -> CapsuleManifest {
        CapsuleManifest {
            schema: SCHEMA_V1.to_string(),
            version: "0.1.0".to_string(),
            name: "unsigned-test".to_string(),
            description: None,
            author: None,
            role: CapsuleRole::App,
            capsule_type: CapsuleType::Wasm,
            entrypoint: "app.wasm".to_string(),
            requires: Vec::new(),
            provides: None,
            authority: None,
            capabilities: Vec::new(),
            interfaces: Vec::new(),
            resources: ResourceLimits::default(),
            permissions: Permissions::default(),
            microvm: None,
            providers: None,
            viewer: None,
            signature: None,
        }
    }

    #[tokio::test]
    async fn strict_signature_mode_requires_trusted_keys() {
        let (runtime, _temp_dir) = test_runtime().await;

        let err = runtime
            .configure_signature_verification(&[], false)
            .await
            .unwrap_err();

        assert!(err.to_string().contains("requires trusted_keys"));
    }

    #[tokio::test]
    async fn dev_signature_mode_allows_unsigned_capsules() {
        let (runtime, temp_dir) = test_runtime().await;
        runtime
            .configure_signature_verification(&[], true)
            .await
            .unwrap();

        runtime
            .verify_capsule_signature(&unsigned_manifest(), temp_dir.path())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn strict_signature_mode_rejects_unsigned_capsules() {
        let (runtime, temp_dir) = test_runtime().await;

        let err = runtime
            .verify_capsule_signature(&unsigned_manifest(), temp_dir.path())
            .await
            .unwrap_err();

        assert!(err.to_string().contains("dev_mode=false"));
    }
}
