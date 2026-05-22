//! Vz compute provider implementation (Phase 1: stubs).
//!
//! Implements [`ComputeProvider`] with the same six methods
//! [`elastos_crosvm::CrosvmProvider`] implements. Every entry point
//! that would touch Apple `Virtualization.framework` returns a
//! deliberate [`crate::PHASE_1_STUB_MESSAGE`] error so callers
//! fail-closed with a single, searchable error string. No
//! `objc2_virtualization` symbols are referenced anywhere in this
//! file — Vz wiring is Phase 2.

use async_trait::async_trait;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::RwLock;

use elastos_common::{
    CapsuleId, CapsuleManifest, CapsuleStatus, CapsuleType, ElastosError, Result,
};
use elastos_compute::{CapsuleHandle, CapsuleInfo, ComputeProvider};

use crate::config::{VmConfig, VzConfig};
use crate::network::NetworkConfig;
use crate::vm::RunningVm;
use crate::PHASE_1_STUB_MESSAGE;

/// Apple Virtualization.framework compute provider.
///
/// Phase 1: holds configuration + a `RunningVm` map for future
/// state but never actually boots a VM. Phase 2 wires the real
/// `VZVirtualMachine` lifecycle.
pub struct VzProvider {
    /// Configuration.
    config: VzConfig,

    /// Running VMs indexed by capsule ID. Phase 1: the map exists so
    /// the public API can be exercised; entries are inserted by
    /// `load()` only if Phase 2 is reached. Today `load()` fails
    /// closed before any insertion, so the field is intentionally
    /// unread until Phase 2.
    #[allow(dead_code)]
    vms: Arc<RwLock<HashMap<CapsuleId, RunningVm>>>,
}

impl VzProvider {
    /// Create a new Vz provider.
    pub fn new(config: VzConfig) -> Result<Self> {
        // Phase 1 deliberately does NOT call `config.validate()` —
        // that would require the user to have a vmlinux on disk
        // before the provider can be constructed, which is fine on
        // Linux/crosvm but creates a usability cliff on Mac during
        // Phase 1 development. Phase 2 will validate at `load()` time
        // (before the first real Vz call) where the failure message
        // can point at the install instructions for the Mac kernel
        // artifact.
        Ok(Self {
            config,
            vms: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    /// Create a provider with default config.
    pub fn with_defaults() -> Result<Self> {
        Self::new(VzConfig::default())
    }

    /// Initialize the provider (create directories, etc.).
    ///
    /// Phase 1: ensures the state and rootfs-cache dirs exist so
    /// Phase 2 can rely on them without re-checking. No Vz calls.
    pub async fn init(&self) -> Result<()> {
        tokio::fs::create_dir_all(&self.config.state_dir)
            .await
            .map_err(|e| ElastosError::Compute(format!("Failed to create vz state dir: {}", e)))?;
        tokio::fs::create_dir_all(&self.config.rootfs_cache_dir)
            .await
            .map_err(|e| {
                ElastosError::Compute(format!("Failed to create rootfs cache dir: {}", e))
            })?;
        Ok(())
    }

    /// Read-only view of the configuration.
    pub fn config(&self) -> &VzConfig {
        &self.config
    }
}

#[async_trait]
impl ComputeProvider for VzProvider {
    async fn load(&self, path: &Path, manifest: CapsuleManifest) -> Result<CapsuleHandle> {
        if manifest.capsule_type != CapsuleType::MicroVM {
            return Err(ElastosError::Compute(format!(
                "VzProvider only supports MicroVM capsules, got: {:?}",
                manifest.capsule_type
            )));
        }

        // Phase 1 builds the VmConfig (data-only) so the from-manifest
        // translation is exercised by tests, then fail-closed before
        // touching Vz. Phase 2 replaces the `Err(...)` below with the
        // real `VZVirtualMachine` construction + `vms.insert(...)`.
        let _vm_config = VmConfig::from_manifest(&manifest, path, &self.config.kernel_path);

        Err(ElastosError::Compute(format!(
            "{} (load: capsule='{}')",
            PHASE_1_STUB_MESSAGE, manifest.name
        )))
    }

    async fn start(&self, handle: &CapsuleHandle) -> Result<()> {
        Err(ElastosError::Compute(format!(
            "{} (start: handle='{}')",
            PHASE_1_STUB_MESSAGE, handle.id
        )))
    }

    async fn stop(&self, handle: &CapsuleHandle) -> Result<()> {
        Err(ElastosError::Compute(format!(
            "{} (stop: handle='{}')",
            PHASE_1_STUB_MESSAGE, handle.id
        )))
    }

    async fn status(&self, handle: &CapsuleHandle) -> Result<CapsuleStatus> {
        Err(ElastosError::Compute(format!(
            "{} (status: handle='{}')",
            PHASE_1_STUB_MESSAGE, handle.id
        )))
    }

    async fn info(&self, handle: &CapsuleHandle) -> Result<CapsuleInfo> {
        Err(ElastosError::Compute(format!(
            "{} (info: handle='{}')",
            PHASE_1_STUB_MESSAGE, handle.id
        )))
    }

    fn supports(&self, capsule_type: &CapsuleType) -> bool {
        matches!(capsule_type, CapsuleType::MicroVM)
    }
}

impl VzProvider {
    /// Configure session credentials for a VM before starting it.
    /// Mirrors [`elastos_crosvm::CrosvmProvider::set_session_for_vm`]
    /// so the supervisor's call site can be platform-conditional
    /// without diverging in shape. Fails closed in Phase 1 because
    /// no VM exists yet.
    pub async fn set_session_for_vm(
        &self,
        capsule_id: &CapsuleId,
        _token: &str,
        _api_addr: &str,
    ) -> Result<()> {
        Err(ElastosError::Compute(format!(
            "{} (set_session_for_vm: capsule='{}')",
            PHASE_1_STUB_MESSAGE, capsule_id.0
        )))
    }

    /// Attach explicit guest-network TAP equivalent. Phase 3 wires
    /// this to a `VZNATNetworkDeviceAttachment`. Phase 1 fails closed.
    pub async fn set_network_for_vm(
        &self,
        capsule_id: &CapsuleId,
        _network: NetworkConfig,
    ) -> Result<()> {
        Err(ElastosError::Compute(format!(
            "{} (set_network_for_vm: capsule='{}')",
            PHASE_1_STUB_MESSAGE, capsule_id.0
        )))
    }

    /// Return None in Phase 1 — no VMs registered.
    pub async fn get_network_for_vm(&self, _capsule_id: &CapsuleId) -> Option<NetworkConfig> {
        None
    }

    /// Append boot arguments to a VM before start. Fails closed in
    /// Phase 1.
    pub async fn append_boot_args_for_vm(&self, capsule_id: &CapsuleId, _args: &str) -> Result<()> {
        Err(ElastosError::Compute(format!(
            "{} (append_boot_args_for_vm: capsule='{}')",
            PHASE_1_STUB_MESSAGE, capsule_id.0
        )))
    }

    /// Get the VM ID for a capsule. Returns None in Phase 1 because
    /// no VMs are registered.
    pub async fn get_vm_id(&self, _capsule_id: &CapsuleId) -> Option<String> {
        None
    }

    /// Get the HTTP port for a VM. Phase 1: fails closed because no
    /// VM exists.
    pub async fn http_port(&self, handle: &CapsuleHandle) -> Result<Option<u16>> {
        Err(ElastosError::Compute(format!(
            "{} (http_port: handle='{}')",
            PHASE_1_STUB_MESSAGE, handle.id
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use elastos_common::{CapsuleRole, CapsuleType, MicroVmConfig, ResourceLimits, SCHEMA_V1};

    fn capsule_manifest(name: &str, ty: CapsuleType) -> CapsuleManifest {
        CapsuleManifest {
            schema: SCHEMA_V1.into(),
            version: "0.1.0".into(),
            name: name.into(),
            description: None,
            author: None,
            role: CapsuleRole::App,
            capsule_type: ty,
            entrypoint: "rootfs.ext4".into(),
            requires: Vec::new(),
            provides: None,
            capabilities: Vec::new(),
            resources: ResourceLimits {
                memory_mb: 128,
                cpu_shares: 100,
                gpu: false,
            },
            permissions: Default::default(),
            microvm: Some(MicroVmConfig {
                kernel: None,
                boot_args: "console=ttyS0".into(),
                http_port: None,
                vcpu_count: Some(1),
                rootfs_cid: None,
                kernel_cid: None,
                rootfs_size: None,
                persistent_storage_mb: None,
            }),
            providers: None,
            viewer: None,
            signature: None,
        }
    }

    #[tokio::test]
    async fn vz_provider_new_succeeds_without_a_kernel_on_disk() {
        // Phase 1 deliberately defers validation to `load()` so a Mac
        // dev can `cargo test` without a vmlinux installed.
        let provider = VzProvider::new(VzConfig::default());
        assert!(provider.is_ok());
    }

    #[test]
    fn vz_provider_supports_microvm_only() {
        let provider = VzProvider::new(VzConfig::default()).unwrap();
        assert!(provider.supports(&CapsuleType::MicroVM));
        assert!(!provider.supports(&CapsuleType::Wasm));
        assert!(!provider.supports(&CapsuleType::Oci));
    }

    #[tokio::test]
    async fn vz_provider_load_microvm_returns_phase_1_stub_error() {
        let provider = VzProvider::new(VzConfig::default()).unwrap();
        let manifest = capsule_manifest("smoke", CapsuleType::MicroVM);

        let err = provider
            .load(std::path::Path::new("/tmp/does-not-exist"), manifest)
            .await
            .unwrap_err();

        let msg = err.to_string();
        assert!(
            msg.contains(PHASE_1_STUB_MESSAGE),
            "expected stub message in error, got: {msg}"
        );
        assert!(msg.contains("smoke"));
    }

    #[tokio::test]
    async fn vz_provider_load_rejects_wasm_capsule_with_typed_error() {
        let provider = VzProvider::new(VzConfig::default()).unwrap();
        let manifest = capsule_manifest("not-a-vm", CapsuleType::Wasm);

        let err = provider
            .load(std::path::Path::new("/tmp"), manifest)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("only supports MicroVM"));
    }

    #[tokio::test]
    async fn vz_provider_init_creates_state_and_cache_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let config = VzConfig::new()
            .with_state_dir(tmp.path().join("vz"))
            .with_rootfs_cache_dir(tmp.path().join("rootfs-cache"));
        let provider = VzProvider::new(config).unwrap();

        provider.init().await.unwrap();

        assert!(tmp.path().join("vz").is_dir());
        assert!(tmp.path().join("rootfs-cache").is_dir());
    }

    #[tokio::test]
    async fn vz_provider_lifecycle_methods_all_fail_closed_with_stub_message() {
        let provider = VzProvider::new(VzConfig::default()).unwrap();
        let handle = CapsuleHandle {
            id: CapsuleId::new("phase1-test".to_string()),
            manifest: capsule_manifest("phase1-test", CapsuleType::MicroVM),
            args: vec![],
        };

        for (label, err) in [
            ("start", provider.start(&handle).await.unwrap_err()),
            ("stop", provider.stop(&handle).await.unwrap_err()),
            ("status", provider.status(&handle).await.unwrap_err()),
            ("info", provider.info(&handle).await.unwrap_err()),
            ("http_port", provider.http_port(&handle).await.unwrap_err()),
        ] {
            assert!(
                err.to_string().contains(PHASE_1_STUB_MESSAGE),
                "{label}: expected stub message, got: {err}"
            );
        }
    }
}
