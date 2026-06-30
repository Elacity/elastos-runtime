//! Core runtime implementation

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::RwLock;

use elastos_common::{CapsuleManifest, CapsuleType, ElastosError, Result};
use elastos_compute::providers::{BridgeSpawner, WasmProvider};
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
        // Configure WASM bridge if we have a concrete WasmProvider reference
        if let Some(ref wasm) = self.wasm_provider {
            let reg = registry.clone();
            let cap_mgr = capability_manager.clone();
            let pending = pending_store.clone();
            wasm.set_bridge_spawner(Arc::new(move |pipes| {
                let ctx = crate::carrier_bridge::BridgeContext {
                    provider_registry: reg.clone(),
                    capability_manager: cap_mgr.clone(),
                    pending_store: pending.clone(),
                    capsule_id: pipes.capsule_id.clone(),
                    principal_id: pipes.principal_id.clone(),
                    data_dir: Some(data_dir.clone()),
                    // WASM capsule acts are metered under the same shared budget as serve/microVM.
                    spend_policy: spend_policy.clone(),
                };
                crate::carrier_bridge::spawn_wasm_carrier_bridge(pipes, ctx);
            }));
            tracing::info!("WASM Carrier bridge configured (real token minting)");
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

        // If no trusted keys configured, skip verification (development mode)
        if !verifier.is_enabled() {
            tracing::warn!("Signature verification skipped (no trusted keys configured)");
            return Ok(());
        }

        // If verification is enabled, capsule must be signed
        if manifest.signature.is_none() {
            return Err(ElastosError::InvalidManifest(
                "Capsule is unsigned but signature verification is enabled".into(),
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
