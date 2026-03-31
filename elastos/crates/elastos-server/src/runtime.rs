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
use elastos_runtime::signature::{hash_content, SignatureVerifier};

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
    /// Handle for stopping the capsule (optional, not all capsules have handles)
    pub handle: Option<CapsuleHandle>,
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
                    capsule_id: format!(
                        "wasm-{}",
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_millis())
                            .unwrap_or(0)
                    ),
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

        // Set args on handle before starting
        handle.args = args;

        // Start capsule
        provider.start(&handle).await?;

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
