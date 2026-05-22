//! Vz compute provider implementation.
//!
//! Phase 2 Day 3: `load` / `start` / `stop` / `status` / `info` /
//! `http_port` are wired to Apple's `VZVirtualMachine` via
//! [`crate::ffi::lifecycle::VzMachineHandle`]. The
//! genuinely-unimplemented surfaces (`set_session_for_vm`,
//! `append_boot_args_for_vm`) keep returning
//! [`crate::PHASE_1_STUB_MESSAGE`] until a later day implements
//! late-bound boot-arg mutation (`VZVirtualMachineConfiguration`
//! is frozen after `VZVirtualMachine::initWithConfiguration:queue:`,
//! so re-applying boot args needs a teardown + rebuild that we
//! defer past first boot).
//!
//! Linux build path: every method that would touch Apple's
//! framework fails closed with the existing Phase 1 stub
//! message. The `network_stub` module + the cfg gates on
//! `ffi::lifecycle` keep the Linux workspace build green.

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
pub struct VzProvider {
    config: VzConfig,

    /// Running VMs indexed by capsule ID.
    vms: Arc<RwLock<HashMap<CapsuleId, RunningVm>>>,

    /// One serial dispatch queue per provider. Every
    /// `VZVirtualMachine` constructed by this provider is bound
    /// to this queue per Apple's threading requirement (Phase 0
    /// §D pitfall #10).
    #[cfg(target_os = "macos")]
    queue: Arc<crate::ffi::dispatch::VzDispatchQueue>,
}

impl VzProvider {
    /// Create a new Vz provider.
    pub fn new(config: VzConfig) -> Result<Self> {
        Ok(Self {
            config,
            vms: Arc::new(RwLock::new(HashMap::new())),
            #[cfg(target_os = "macos")]
            queue: Arc::new(crate::ffi::dispatch::VzDispatchQueue::new(
                "elastos-vz.provider",
            )),
        })
    }

    /// Create a provider with default config.
    pub fn with_defaults() -> Result<Self> {
        Self::new(VzConfig::default())
    }

    /// Initialise on-disk state directories.
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

        let rootfs_path = path.join(&manifest.entrypoint);
        if !rootfs_path.exists() {
            return Err(ElastosError::CapsuleNotFound(format!(
                "Rootfs not found: {}",
                rootfs_path.display()
            )));
        }

        let mut vm_config = VmConfig::from_manifest(&manifest, path, &self.config.kernel_path);

        // Apply the provider-wide initramfs default. The capsule
        // manifest doesn't carry an initramfs path (Phase 2 design
        // decision — `elastos-common` is Linux-untouched), so
        // `vm-debug boot --initramfs …` propagates the path here
        // via `VzConfig::with_initramfs_path`. A future per-VM
        // override would land before this line and take
        // precedence by only setting from the provider default
        // when the per-VM field is still `None`.
        if vm_config.initramfs_path.is_none() {
            if let Some(default_initramfs) = self.config.initramfs_path.as_ref() {
                vm_config.initramfs_path = Some(default_initramfs.clone());
            }
        }

        if !vm_config.kernel_path.exists() {
            return Err(ElastosError::Compute(format!(
                "Kernel not found: {}",
                vm_config.kernel_path.display()
            )));
        }

        // Mint a fresh capsule id matching the crosvm convention
        // so log lines stay diff-able across substrates.
        let id = CapsuleId::new(format!("microvm-{}", uuid::Uuid::new_v4()));
        let socket_path = self.config.state_dir.join(&id.0);

        #[cfg(target_os = "macos")]
        {
            let built = crate::ffi::builder::BuiltMachine::from_vm_config(&vm_config, &self.config)
                .map_err(ElastosError::Compute)?;

            let handle = crate::ffi::lifecycle::VzMachineHandle::new(
                built,
                self.queue.clone(),
                vm_config.vm_id.clone(),
            )
            .map_err(ElastosError::Compute)?;

            let vm = RunningVm::with_handle(vm_config, manifest.clone(), socket_path, handle);

            self.vms.write().await.insert(id.clone(), vm);

            tracing::info!("Loaded MicroVM capsule '{}' with ID {}", manifest.name, id);

            Ok(CapsuleHandle {
                id,
                manifest,
                args: vec![],
            })
        }

        #[cfg(not(target_os = "macos"))]
        {
            // Non-macOS keeps the historical fail-closed
            // contract: there is no Vz framework here. The
            // workspace builds, but every load resolves to the
            // stub message.
            let _ = (vm_config, socket_path); // silence unused
            Err(ElastosError::Compute(format!(
                "{} (load: capsule='{}')",
                PHASE_1_STUB_MESSAGE, manifest.name
            )))
        }
    }

    async fn start(&self, handle: &CapsuleHandle) -> Result<()> {
        let mut vms = self.vms.write().await;
        let vm = vms
            .get_mut(&handle.id)
            .ok_or_else(|| ElastosError::CapsuleNotFound(handle.id.0.clone()))?;
        vm.start().await
    }

    async fn stop(&self, handle: &CapsuleHandle) -> Result<()> {
        let mut vms = self.vms.write().await;
        if let Some(vm) = vms.get_mut(&handle.id) {
            vm.stop().await?;
        }
        Ok(())
    }

    async fn status(&self, handle: &CapsuleHandle) -> Result<CapsuleStatus> {
        let vms = self.vms.read().await;
        let vm = vms
            .get(&handle.id)
            .ok_or_else(|| ElastosError::CapsuleNotFound(handle.id.0.clone()))?;

        if vm.is_running() {
            Ok(CapsuleStatus::Running)
        } else {
            Ok(vm.status)
        }
    }

    async fn info(&self, handle: &CapsuleHandle) -> Result<CapsuleInfo> {
        let vms = self.vms.read().await;
        let vm = vms
            .get(&handle.id)
            .ok_or_else(|| ElastosError::CapsuleNotFound(handle.id.0.clone()))?;

        Ok(CapsuleInfo {
            id: handle.id.clone(),
            name: vm.manifest.name.clone(),
            status: vm.status,
            memory_used_mb: vm.config.mem_size_mib,
        })
    }

    fn supports(&self, capsule_type: &CapsuleType) -> bool {
        matches!(capsule_type, CapsuleType::MicroVM)
    }
}

impl VzProvider {
    /// Configure session credentials for a VM **before** load.
    ///
    /// Apple's `VZVirtualMachineConfiguration` is frozen the
    /// moment `VZVirtualMachine::initWithConfiguration:queue:`
    /// is invoked (Phase 0 audit §D pitfall #9 covers the
    /// related "no late mutation" rule). Day 3 therefore keeps
    /// the Phase 1 stub for this surface — the supervisor's
    /// macOS path (still gated by Phase 1's bail) is the only
    /// caller anyway. A later day will surface a different API
    /// shape (set session **before** load) once we add the
    /// supervisor-side route.
    pub async fn set_session_for_vm(
        &self,
        capsule_id: &CapsuleId,
        _token: &str,
        _api_addr: &str,
    ) -> Result<()> {
        Err(ElastosError::Compute(format!(
            "{} (set_session_for_vm: capsule='{}', see provider.rs comment)",
            PHASE_1_STUB_MESSAGE, capsule_id.0
        )))
    }

    /// Attach explicit guest-network TAP equivalent. Phase 3
    /// wires this to `VZNATNetworkDeviceAttachment` with an
    /// explicit subnet. Day 3: not implemented.
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

    /// Return None — same shape as
    /// `CrosvmProvider::get_network_for_vm` for an unattached VM.
    pub async fn get_network_for_vm(&self, _capsule_id: &CapsuleId) -> Option<NetworkConfig> {
        None
    }

    /// Append boot arguments to a VM before start. See the
    /// note on `set_session_for_vm` for why this remains
    /// fail-closed until a later day.
    pub async fn append_boot_args_for_vm(&self, capsule_id: &CapsuleId, _args: &str) -> Result<()> {
        Err(ElastosError::Compute(format!(
            "{} (append_boot_args_for_vm: capsule='{}')",
            PHASE_1_STUB_MESSAGE, capsule_id.0
        )))
    }

    /// Get the underlying VM ID for a capsule, if loaded.
    pub async fn get_vm_id(&self, capsule_id: &CapsuleId) -> Option<String> {
        let vms = self.vms.read().await;
        vms.get(capsule_id).map(|vm| vm.config.vm_id.clone())
    }

    /// Get the HTTP port for a loaded VM.
    pub async fn http_port(&self, handle: &CapsuleHandle) -> Result<Option<u16>> {
        let vms = self.vms.read().await;
        let vm = vms
            .get(&handle.id)
            .ok_or_else(|| ElastosError::CapsuleNotFound(handle.id.0.clone()))?;
        Ok(vm.http_port())
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
    async fn vz_provider_load_rejects_non_microvm_with_typed_error() {
        let provider = VzProvider::new(VzConfig::default()).unwrap();
        let manifest = capsule_manifest("not-a-vm", CapsuleType::Wasm);
        let err = provider
            .load(std::path::Path::new("/tmp"), manifest)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("only supports MicroVM"));
    }

    #[tokio::test]
    async fn vz_provider_load_returns_capsule_not_found_when_rootfs_missing() {
        // Phase 1 used to assert PHASE_1_STUB_MESSAGE here; Day 3
        // now actually validates the inputs in the same order
        // crosvm does, so the first error is CapsuleNotFound for
        // a missing rootfs.
        let provider = VzProvider::new(VzConfig::default()).unwrap();
        let manifest = capsule_manifest("smoke", CapsuleType::MicroVM);
        let err = provider
            .load(std::path::Path::new("/tmp/does-not-exist"), manifest)
            .await
            .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("Rootfs not found"),
            "expected 'Rootfs not found', got: {msg}"
        );
    }

    #[tokio::test]
    async fn vz_provider_lifecycle_methods_fail_closed_for_unloaded_handle() {
        let provider = VzProvider::new(VzConfig::default()).unwrap();
        let handle = CapsuleHandle {
            id: CapsuleId::new("never-loaded".to_string()),
            manifest: capsule_manifest("never-loaded", CapsuleType::MicroVM),
            args: vec![],
        };

        // start / status / info / http_port all surface
        // CapsuleNotFound for an unloaded handle — same
        // contract as the crosvm provider.
        let start_err = provider.start(&handle).await.unwrap_err();
        let status_err = provider.status(&handle).await.unwrap_err();
        let info_err = provider.info(&handle).await.unwrap_err();
        let http_err = provider.http_port(&handle).await.unwrap_err();

        for (label, err) in [
            ("start", start_err),
            ("status", status_err),
            ("info", info_err),
            ("http_port", http_err),
        ] {
            assert!(
                matches!(err, ElastosError::CapsuleNotFound(_)),
                "{label}: expected CapsuleNotFound, got: {err}"
            );
        }

        // stop is intentionally idempotent: missing VM ⇒ Ok(())
        provider.stop(&handle).await.unwrap();
    }

    #[tokio::test]
    async fn vz_provider_session_and_boot_args_still_fail_closed_with_stub() {
        let provider = VzProvider::new(VzConfig::default()).unwrap();
        let capsule_id = CapsuleId::new("phase2-day3-session".to_string());

        let session_err = provider
            .set_session_for_vm(&capsule_id, "abc12345", "http://127.0.0.1:3000")
            .await
            .unwrap_err();
        let bootargs_err = provider
            .append_boot_args_for_vm(&capsule_id, "extra.token=value")
            .await
            .unwrap_err();
        let network_err = provider
            .set_network_for_vm(&capsule_id, NetworkConfig::new("phase2-day3-session"))
            .await
            .unwrap_err();

        for (label, err) in [
            ("set_session_for_vm", session_err),
            ("append_boot_args_for_vm", bootargs_err),
            ("set_network_for_vm", network_err),
        ] {
            let msg = err.to_string();
            assert!(
                msg.contains(PHASE_1_STUB_MESSAGE),
                "{label}: expected stub message, got: {msg}"
            );
        }
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
    async fn vz_provider_get_vm_id_returns_none_for_unloaded_capsule() {
        let provider = VzProvider::new(VzConfig::default()).unwrap();
        let id = CapsuleId::new("not-here".to_string());
        assert!(provider.get_vm_id(&id).await.is_none());
    }
}
