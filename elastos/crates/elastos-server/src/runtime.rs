//! Core runtime implementation

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, RwLock as StdRwLock};
use tokio::sync::RwLock;

use elastos_common::{CapsuleManifest, CapsuleType, ElastosError, Result};
use elastos_compute::providers::{BridgeHostcall, ComponentProvider};
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
    /// Reference to the component provider for ABI-specific launch.
    component_provider: Option<Arc<ComponentProvider>>,
    /// Per-launch manifest capability ceilings for capsule bridge requests.
    bridge_manifest_capabilities: Arc<StdRwLock<HashMap<String, Vec<String>>>>,
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
            component_provider: None,
            bridge_manifest_capabilities: Arc::new(StdRwLock::new(HashMap::new())),
        }
    }

    pub fn with_providers_and_component(
        storage: Arc<dyn StorageProvider>,
        compute_providers: Vec<Arc<dyn ComputeProvider>>,
        component_provider: Option<Arc<ComponentProvider>>,
    ) -> Self {
        Self {
            _storage: storage,
            compute_providers,
            signature_verifier: RwLock::new(SignatureVerifier::new()),
            signature_dev_mode: RwLock::new(false),
            provider_registry: RwLock::new(None),
            running_capsules: RwLock::new(HashMap::new()),
            component_provider,
            bridge_manifest_capabilities: Arc::new(StdRwLock::new(HashMap::new())),
        }
    }

    /// Set the provider registry and capability manager for provider dispatch
    /// and real token minting.
    ///
    /// Also configures the component hostcall so component capsules can
    /// communicate with providers through Runtime-controlled authority.
    pub async fn set_provider_registry(
        &self,
        registry: Arc<ProviderRegistry>,
        capability_manager: Arc<elastos_runtime::capability::CapabilityManager>,
        pending_store: Arc<elastos_runtime::capability::pending::PendingRequestStore>,
        data_dir: std::path::PathBuf,
    ) {
        if let Some(ref component) = self.component_provider {
            let host_reg = registry.clone();
            let host_cap_mgr = capability_manager.clone();
            let host_pending = pending_store.clone();
            let host_data_dir = data_dir.clone();
            let host_manifest_capabilities = self.bridge_manifest_capabilities.clone();
            let host_handle = tokio::runtime::Handle::current();
            component.set_bus_hostcall(Arc::new(move |line, capsule_id, principal_id| {
                let manifest_capabilities = host_manifest_capabilities
                    .read()
                    .ok()
                    .and_then(|guard| guard.get(capsule_id).cloned())
                    .unwrap_or_default();
                let ctx = crate::carrier_bridge::BridgeContext {
                    provider_registry: host_reg.clone(),
                    capability_manager: host_cap_mgr.clone(),
                    pending_store: host_pending.clone(),
                    capsule_id: capsule_id.to_string(),
                    principal_id: principal_id.map(ToOwned::to_owned),
                    manifest_capabilities,
                    data_dir: Some(host_data_dir.clone()),
                };
                let response = host_handle
                    .block_on(crate::carrier_bridge::handle_component_carrier_request(
                        line, ctx,
                    ))
                    .map_err(|err| err.to_string())?;
                serde_json::to_string(&response).map_err(|err| err.to_string())
            }));
            tracing::info!("Component Carrier bridge configured");
        }

        let mut guard = self.provider_registry.write().await;
        *guard = Some(registry);
    }

    /// Override the default local capsule carrier host-call handler.
    ///
    /// Used by attached-runtime execution to route guest calls
    /// through the already-running local runtime API.
    pub fn set_bridge_hostcall(&self, hostcall: BridgeHostcall) {
        if let Some(ref component) = self.component_provider {
            component.set_bus_hostcall(hostcall);
        }
    }

    /// Find a compute provider that supports the given capsule type
    fn get_provider(&self, capsule_type: &CapsuleType) -> Option<Arc<dyn ComputeProvider>> {
        self.compute_providers
            .iter()
            .find(|p| p.supports(capsule_type))
            .cloned()
    }

    fn get_provider_for_manifest(
        &self,
        manifest: &CapsuleManifest,
    ) -> Option<Arc<dyn ComputeProvider>> {
        if manifest.is_component_capsule() {
            return self
                .component_provider
                .as_ref()
                .map(|provider| provider.clone() as Arc<dyn ComputeProvider>);
        }
        if manifest.capsule_type == CapsuleType::Wasm {
            return None;
        }
        self.get_provider(&manifest.capsule_type)
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
        crate::component::validate_component_capsule(path, &manifest).await?;

        // Find a compute provider that supports this capsule type
        let provider = self.get_provider_for_manifest(&manifest).ok_or_else(|| {
            if manifest.is_component_capsule() {
                ElastosError::Compute("No component compute provider is configured".to_string())
            } else {
                ElastosError::Compute(format!(
                    "No compute provider supports capsule type: {:?}",
                    manifest.capsule_type
                ))
            }
        })?;

        // Load capsule
        let mut handle = provider.load(path, manifest).await?;
        if let Ok(mut bounds) = self.bridge_manifest_capabilities.write() {
            bounds.insert(
                handle.id.0.clone(),
                handle.manifest.resource_authority_bounds(),
            );
        }
        if handle.manifest.is_component_capsule() {
            if let Some(ref component) = self.component_provider {
                component
                    .set_bridge_principal(&handle.id, principal_id.clone())
                    .await;
            }
        }

        // Set args on handle before starting
        handle.args = args;

        // Start capsule
        let start = provider.start(&handle).await;
        if handle.manifest.is_component_capsule() {
            if let Some(ref component) = self.component_provider {
                component.clear_bridge_principal(&handle.id).await;
            }
        }
        if let Ok(mut bounds) = self.bridge_manifest_capabilities.write() {
            bounds.remove(&handle.id.0);
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
            .get_provider_for_manifest(&handle.manifest)
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
            if let Some(provider) = self.get_provider_for_manifest(&handle.manifest) {
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
mod verified_signer_tests {
    use super::*;
    use elastos_common::{CapsuleType, Permissions, ResourceLimits};
    use elastos_runtime::signature::{generate_keypair, key_fingerprint, sign_capsule};

    fn test_manifest() -> CapsuleManifest {
        CapsuleManifest {
            schema: elastos_common::SCHEMA_V1.into(),
            version: "0.1.0".into(),
            name: "vm-player".into(),
            description: Some("Test".into()),
            author: Some("Test Author".into()),
            role: elastos_common::CapsuleRole::App,
            capsule_type: CapsuleType::Wasm,
            runtime_abi: None,
            bus_contract: None,
            wit_world_sha256: None,
            execution: None,
            projections: Vec::new(),
            entrypoint: "main.wasm".into(),
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

    /// Write the entrypoint bytes into a temp dir and sign the manifest over their hash,
    /// exactly as the launch path will read+verify them. Returns (dir, content_bytes).
    fn signed_capsule_dir(
        signing_key: &elastos_runtime::signature::SigningKey,
        manifest: &mut CapsuleManifest,
    ) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let content = b"entrypoint bytes";
        std::fs::write(dir.path().join(&manifest.entrypoint), content).unwrap();
        let content_hash = hash_content(content);
        sign_capsule(signing_key, manifest, &content_hash).unwrap();
        dir
    }

    #[tokio::test]
    async fn a_trusted_key_match_yields_the_signer_fingerprint() {
        let (sk, vk) = generate_keypair();
        let mut manifest = test_manifest();
        let dir = signed_capsule_dir(&sk, &mut manifest);
        let mut verifier = SignatureVerifier::new();
        verifier.add_trusted_key(vk);

        let signer = resolve_verified_signer_with(&verifier, &manifest, dir.path()).await;
        assert_eq!(
            signer,
            Some(key_fingerprint(&vk)),
            "a real trusted-key match yields that key's fingerprint"
        );
    }

    #[tokio::test]
    async fn a_disabled_verifier_yields_none_even_for_a_signed_capsule() {
        let (sk, _vk) = generate_keypair();
        let mut manifest = test_manifest();
        let dir = signed_capsule_dir(&sk, &mut manifest);
        // No trusted keys ⇒ verification was OFF ⇒ no verified signer (honest), even
        // though the manifest is signed.
        let verifier = SignatureVerifier::new();
        assert_eq!(
            resolve_verified_signer_with(&verifier, &manifest, dir.path()).await,
            None
        );
    }

    #[tokio::test]
    async fn an_untrusted_signer_yields_none() {
        let (sk, _vk) = generate_keypair();
        let (_other_sk, other_vk) = generate_keypair();
        let mut manifest = test_manifest();
        let dir = signed_capsule_dir(&sk, &mut manifest);
        // The capsule is signed, but by a key the verifier does not trust.
        let mut verifier = SignatureVerifier::new();
        verifier.add_trusted_key(other_vk);
        assert_eq!(
            resolve_verified_signer_with(&verifier, &manifest, dir.path()).await,
            None,
            "an untrusted signature is not a verified signer (verification, not presence)"
        );
    }

    #[tokio::test]
    async fn an_unsigned_manifest_yields_none() {
        let (_sk, vk) = generate_keypair();
        let manifest = test_manifest(); // never signed
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(&manifest.entrypoint), b"x").unwrap();
        let mut verifier = SignatureVerifier::new();
        verifier.add_trusted_key(vk);
        assert_eq!(
            resolve_verified_signer_with(&verifier, &manifest, dir.path()).await,
            None
        );
    }

    #[tokio::test]
    async fn a_missing_entrypoint_yields_none_not_a_panic() {
        let (sk, vk) = generate_keypair();
        let mut manifest = test_manifest();
        // Sign over some content, but DON'T write the entrypoint file — the resolver must
        // fail honest (None), never panic or fabricate a signer.
        let content_hash = hash_content(b"entrypoint bytes");
        sign_capsule(&sk, &mut manifest, &content_hash).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let mut verifier = SignatureVerifier::new();
        verifier.add_trusted_key(vk);
        assert_eq!(
            resolve_verified_signer_with(&verifier, &manifest, dir.path()).await,
            None
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use elastos_common::{
        elastos_bus_v1_wit_sha256, CapsuleId, CapsuleManifest, CapsuleRole, CapsuleStatus,
        CapsuleType, Permissions, ResourceLimits, ELASTOS_BUS_V1_CONTRACT, SCHEMA_V1,
    };
    use elastos_compute::{CapsuleInfo, ComputeProvider};
    use elastos_runtime::capability::pending::GrantDuration;
    use elastos_runtime::capability::{CapabilityManager, CapabilityStore, TokenConstraints};
    use elastos_runtime::primitives::audit::{AuditEvent, AuditLog};
    use elastos_runtime::primitives::metrics::MetricsManager;
    use elastos_runtime::provider::{
        Provider, ProviderError, ProviderRegistry, ResourceRequest, ResourceResponse,
    };
    use elastos_storage::providers::LocalFSProvider;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::Mutex;
    use wasm_encoder::{
        Component, ComponentImportSection, ComponentTypeRef as EncodedComponentTypeRef,
        ComponentTypeSection, InstanceType,
    };

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

    struct CountingComputeProvider {
        loads: Arc<AtomicUsize>,
    }

    #[derive(Default)]
    struct BusConformanceProvider {
        requests: Mutex<Vec<serde_json::Value>>,
    }

    #[async_trait]
    impl Provider for BusConformanceProvider {
        async fn handle(
            &self,
            _request: ResourceRequest,
        ) -> std::result::Result<ResourceResponse, ProviderError> {
            Err(ProviderError::Provider(
                "the Bus conformance provider accepts JSON dispatch only".to_string(),
            ))
        }

        fn schemes(&self) -> Vec<&'static str> {
            vec!["test"]
        }

        fn name(&self) -> &'static str {
            "bus-v1-conformance"
        }

        async fn send_raw(
            &self,
            request: &serde_json::Value,
        ) -> std::result::Result<serde_json::Value, ProviderError> {
            self.requests.lock().await.push(request.clone());
            Ok(serde_json::json!({"status": "ok"}))
        }
    }

    #[async_trait]
    impl ComputeProvider for CountingComputeProvider {
        async fn load(&self, _path: &Path, manifest: CapsuleManifest) -> Result<CapsuleHandle> {
            self.loads.fetch_add(1, Ordering::SeqCst);
            Ok(CapsuleHandle {
                id: CapsuleId::new("counting-capsule"),
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
            runtime_abi: None,
            bus_contract: None,
            wit_world_sha256: None,
            execution: None,
            projections: Vec::new(),
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

    fn component_with_extra_import() -> Vec<u8> {
        let mut types = ComponentTypeSection::new();
        types.instance(&InstanceType::new());

        let mut imports = ComponentImportSection::new();
        imports.import(
            "wasi:filesystem/types@0.2.0",
            EncodedComponentTypeRef::Instance(0),
        );

        let mut component = Component::new();
        component.section(&types);
        component.section(&imports);
        component.finish()
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

    #[tokio::test]
    async fn component_import_validation_fails_before_provider_load() {
        let temp_dir = tempfile::tempdir().unwrap();
        let storage = Arc::new(
            LocalFSProvider::new(temp_dir.path().to_path_buf())
                .await
                .unwrap(),
        );
        let loads = Arc::new(AtomicUsize::new(0));
        let compute = Arc::new(CountingComputeProvider {
            loads: loads.clone(),
        });
        let runtime = Runtime::new(storage, compute);
        runtime
            .configure_signature_verification(&[], true)
            .await
            .unwrap();

        let capsule_dir = temp_dir.path().join("capsule");
        std::fs::create_dir_all(&capsule_dir).unwrap();
        std::fs::write(
            capsule_dir.join("app.component.wasm"),
            component_with_extra_import(),
        )
        .unwrap();
        std::fs::write(
            capsule_dir.join("capsule.json"),
            format!(
                r#"{{
                    "schema": "elastos.capsule/v1",
                    "version": "0.1.0",
                    "name": "bad-component",
                    "role": "app",
                    "type": "wasm",
                    "runtime_abi": "elastos.component/v1",
                    "bus_contract": "{}",
                    "wit_world_sha256": "{}",
                    "execution": "component",
                    "projections": ["cli"],
                    "entrypoint": "app.component.wasm"
                }}"#,
                ELASTOS_BUS_V1_CONTRACT,
                elastos_bus_v1_wit_sha256()
            ),
        )
        .unwrap();

        let err = runtime
            .run_local_with_principal(&capsule_dir, Vec::new(), None)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("component import"));
        assert_eq!(loads.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn component_conformance_exercises_bus_authorization_dispatch_and_audit() {
        let temp_dir = tempfile::tempdir().unwrap();
        let storage = Arc::new(
            LocalFSProvider::new(temp_dir.path().join("storage"))
                .await
                .unwrap(),
        );
        let component_provider = Arc::new(ComponentProvider::new());
        let runtime = Arc::new(Runtime::with_providers_and_component(
            storage,
            Vec::new(),
            Some(component_provider.clone()),
        ));
        runtime
            .configure_signature_verification(&[], true)
            .await
            .unwrap();

        let audit_log = Arc::new(AuditLog::new());
        let capability_manager = Arc::new(CapabilityManager::new(
            Arc::new(CapabilityStore::new()),
            audit_log.clone(),
            Arc::new(MetricsManager::new()),
        ));
        let pending_store = Arc::new(
            elastos_runtime::capability::pending::PendingRequestStore::new(audit_log.clone()),
        );
        let registry = Arc::new(ProviderRegistry::new());
        let provider = Arc::new(BusConformanceProvider::default());
        registry.register(provider.clone()).await;
        runtime
            .set_provider_registry(
                registry,
                capability_manager.clone(),
                pending_store.clone(),
                temp_dir.path().join("runtime-data"),
            )
            .await;

        let fixture_dir =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/components/bus-v1");
        let launch_runtime = runtime.clone();
        let launch_fixture = fixture_dir.clone();
        let launch = tokio::spawn(async move {
            launch_runtime
                .run_local_with_principal(
                    &launch_fixture,
                    Vec::new(),
                    Some("person:local:bus-conformance".to_string()),
                )
                .await
        });

        let pending = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                if let Some(request) = pending_store.list_pending().await.into_iter().next() {
                    break request;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("component must create a pending capability request");

        assert_eq!(pending.resource.as_str(), "elastos://test/bus/probe");
        assert_eq!(pending.action, elastos_runtime::capability::Action::Read);
        assert_eq!(pending.reason, "verify the Component-to-Bus authority path");
        assert!(pending.session_id.as_str().starts_with("component-"));

        let token = capability_manager.grant(
            pending.session_id.as_str(),
            pending.resource.clone(),
            pending.action,
            TokenConstraints::default(),
            None,
        );
        pending_store
            .grant_request(pending.id.as_str(), token, GrantDuration::Session)
            .await
            .unwrap();

        let handle = tokio::time::timeout(std::time::Duration::from_secs(5), launch)
            .await
            .expect("component must finish after the grant")
            .expect("component launch task must not panic")
            .expect("component must complete the Bus conformance flow");
        assert_eq!(handle.manifest.name, "bus-v1-conformance");

        let requests = provider.requests.lock().await;
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0]["op"], "read");
        assert_eq!(requests[0]["probe"], "bus-v1-conformance");
        drop(requests);

        let events = audit_log.recent_events(32);
        assert!(events.iter().any(|event| matches!(
            event,
            AuditEvent::CapabilityRequested { resource, .. }
                if resource == "elastos://test/bus/probe"
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            AuditEvent::CapabilityUse { capsule_id, success: true, .. }
                if capsule_id == pending.session_id.as_str()
        )));
        let requested_audit = events.iter().find_map(|event| match event {
            AuditEvent::Custom {
                event_type,
                details,
            } if event_type == "component.invoke.requested" => {
                details.get("audit_id").and_then(|value| value.as_str())
            }
            _ => None,
        });
        let completed_audit = events.iter().find_map(|event| match event {
            AuditEvent::Custom {
                event_type,
                details,
            } if event_type == "component.invoke.completed" => {
                details.get("audit_id").and_then(|value| value.as_str())
            }
            _ => None,
        });
        assert!(requested_audit.is_some());
        assert_eq!(requested_audit, completed_audit);

        let denied_runtime = runtime.clone();
        let denied_fixture = fixture_dir.clone();
        let denied_launch = tokio::spawn(async move {
            denied_runtime
                .run_local_with_principal(
                    &denied_fixture,
                    Vec::new(),
                    Some("person:local:bus-conformance".to_string()),
                )
                .await
        });
        let denied_pending = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                if let Some(request) = pending_store.list_pending().await.into_iter().next() {
                    break request;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("second component activation must request a capability");
        let wrong_action_token = capability_manager.grant(
            denied_pending.session_id.as_str(),
            denied_pending.resource.clone(),
            elastos_runtime::capability::Action::Write,
            TokenConstraints::default(),
            None,
        );
        pending_store
            .grant_request(
                denied_pending.id.as_str(),
                wrong_action_token,
                GrantDuration::Session,
            )
            .await
            .unwrap();

        let denied = tokio::time::timeout(std::time::Duration::from_secs(5), denied_launch)
            .await
            .expect("denied component activation must finish")
            .expect("denied component task must not panic")
            .expect_err("wrong-action grant must not authorize provider dispatch");
        assert!(denied.to_string().contains("Component capsule denied"));
        assert_eq!(provider.requests.lock().await.len(), 1);
        assert!(audit_log.recent_events(64).iter().any(|event| matches!(
            event,
            AuditEvent::Custom { event_type, details }
                if event_type == "component.invoke.completed"
                    && details["capsule_id"] == denied_pending.session_id.as_str()
                    && details["outcome"] == "denied"
        )));
        assert!(matches!(
            component_provider.status(&handle).await,
            Err(ElastosError::CapsuleNotFound(_))
        ));
    }
}
