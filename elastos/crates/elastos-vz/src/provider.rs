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
// PHASE_1_STUB_MESSAGE is referenced from the non-macOS branch
// of `load_with_vm_config` and from several `#[cfg(test)]`
// regression tests that assert it is NEVER surfaced from a
// post-Day-1 code path. On macOS-prod the non-test reference is
// `cfg`-gated out, so the import is technically unused there —
// the `allow` keeps clippy happy without losing the import in
// the test build.
#[cfg_attr(all(target_os = "macos", not(test)), allow(unused_imports))]
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

        // Trait entry point: the caller (e.g. a smoke test) wants
        // VzProvider to do all the VmConfig defaulting itself.
        // The supervisor takes the parallel `load_with_vm_config`
        // path instead, because it already bakes session tokens,
        // command payloads, carrier-path, etc. into the boot args
        // before calling us (Phase 3 Day 1 port plan).
        self.load_with_vm_config(vm_config, manifest).await
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
    /// Load a microVM from a pre-built [`VmConfig`].
    ///
    /// **Phase 3 Day 1 seam.** This is the API shape the
    /// supervisor needs — Apple's `VZVirtualMachineConfiguration`
    /// is frozen the moment `VZVirtualMachine::initWithConfiguration:queue:`
    /// is invoked, so every boot arg (session token, command
    /// payload, capsule args, carrier path, …) MUST be baked
    /// into the `VmConfig` **before** `load`. The supervisor
    /// does exactly this composition on Linux today
    /// ([`elastos-server/src/supervisor.rs`](../../../elastos/crates/elastos-server/src/supervisor.rs)
    /// L1019–1133) and Phase 3 Day 1 mirrors the same flow on
    /// macOS.
    ///
    /// The trait method [`Self::load`] is a thin wrapper that
    /// builds a default `VmConfig` from the manifest and calls
    /// this method — useful for smoke tests and callers that
    /// don't need bespoke boot-arg composition.
    pub async fn load_with_vm_config(
        &self,
        vm_config: VmConfig,
        manifest: CapsuleManifest,
    ) -> Result<CapsuleHandle> {
        if manifest.capsule_type != CapsuleType::MicroVM {
            return Err(ElastosError::Compute(format!(
                "VzProvider only supports MicroVM capsules, got: {:?}",
                manifest.capsule_type
            )));
        }

        if !vm_config.kernel_path.exists() {
            return Err(ElastosError::Compute(format!(
                "Kernel not found: {}",
                vm_config.kernel_path.display()
            )));
        }

        if !vm_config.rootfs_path.exists() {
            return Err(ElastosError::CapsuleNotFound(format!(
                "Rootfs not found: {}",
                vm_config.rootfs_path.display()
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
                "{} (load_with_vm_config: capsule='{}')",
                PHASE_1_STUB_MESSAGE, manifest.name
            )))
        }
    }

    /// **Deprecated by Phase 3 Day 2.** Apple's
    /// `VZVirtualMachineConfiguration` is frozen post-init
    /// (Phase 0 §D pitfall #9); no session credentials can be
    /// applied after the VM has been loaded. The correct shape
    /// is to bake session args into [`VmConfig::boot_args`] (use
    /// [`VmConfig::with_session`]) **before** calling
    /// [`Self::load_with_vm_config`].
    ///
    /// This method is intentionally kept to surface a clear,
    /// typed migration message to any caller still on the
    /// pre-Day-1 API. It always fails closed.
    pub async fn set_session_for_vm(
        &self,
        capsule_id: &CapsuleId,
        _token: &str,
        _api_addr: &str,
    ) -> Result<()> {
        Err(ElastosError::Compute(format!(
            "vz: set_session_for_vm is unsupported — \
             VZVirtualMachineConfiguration is frozen after load. \
             Use VmConfig::with_session(token, api_addr) (or append \
             `elastos.token=<t>` to VmConfig.boot_args directly) and call \
             VzProvider::load_with_vm_config(vm_config, manifest) instead. \
             (capsule='{}'; see docs/vz-backend/PHASE_3_DAY_1_PORT_PLAN.md)",
            capsule_id.0
        )))
    }

    /// **Deprecated by Phase 3 Day 2.** Late-binding network
    /// attachment is not possible on Vz because Apple's
    /// `VZVirtualMachineConfiguration` is frozen after init.
    /// The supervisor must compose the network configuration on
    /// the `VmConfig` **before** calling [`Self::load_with_vm_config`].
    ///
    /// Note: Mac currently uses Vz NAT-only networking by
    /// default (no Apple entitlement required). Bridged-mode
    /// support is deferred to Phase 3 Day 4+ — see the port plan.
    ///
    /// This method always fails closed with a typed migration
    /// message.
    pub async fn set_network_for_vm(
        &self,
        capsule_id: &CapsuleId,
        _network: NetworkConfig,
    ) -> Result<()> {
        Err(ElastosError::Compute(format!(
            "vz: set_network_for_vm is unsupported — \
             VZVirtualMachineConfiguration is frozen after load. \
             Default Vz networking is NAT (no entitlement required) and is \
             attached automatically by the builder. Bridged mode is Phase 3 \
             Day 4+ work and requires the `com.apple.vm.networking` Apple \
             entitlement. \
             (capsule='{}'; see docs/vz-backend/PHASE_3_DAY_1_PORT_PLAN.md)",
            capsule_id.0
        )))
    }

    /// Return None — same shape as
    /// `CrosvmProvider::get_network_for_vm` for an unattached VM.
    pub async fn get_network_for_vm(&self, _capsule_id: &CapsuleId) -> Option<NetworkConfig> {
        None
    }

    /// **Deprecated by Phase 3 Day 1.** Apple's
    /// `VZVirtualMachineConfiguration` is frozen post-init
    /// (Phase 0 §D pitfall #9); no boot arg can be appended
    /// after the VM has been loaded. The correct shape is to
    /// bake every boot arg into [`VmConfig::boot_args`]
    /// **before** calling [`Self::load_with_vm_config`].
    ///
    /// This method is intentionally kept to surface a clear,
    /// typed migration message to any caller still on the
    /// pre-Day-1 API. It always fails closed.
    pub async fn append_boot_args_for_vm(&self, capsule_id: &CapsuleId, _args: &str) -> Result<()> {
        Err(ElastosError::Compute(format!(
            "vz: append_boot_args_for_vm is unsupported — \
             VZVirtualMachineConfiguration is frozen after load. \
             Bake boot args into VmConfig.boot_args and call \
             VzProvider::load_with_vm_config(vm_config, manifest) instead. \
             (capsule='{}'; see docs/vz-backend/PHASE_3_DAY_1_PORT_PLAN.md)",
            capsule_id.0
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
    async fn vz_provider_set_session_for_vm_returns_typed_migration_error_after_phase3_day2() {
        // Phase 3 Day 2 contract: the old mid-life-cycle
        // set_session_for_vm shape is unsupported because
        // VZVirtualMachineConfiguration is frozen after load.
        // The method exists only to surface a typed migration
        // message pointing callers at VmConfig::with_session +
        // load_with_vm_config.
        let provider = VzProvider::new(VzConfig::default()).unwrap();
        let capsule_id = CapsuleId::new("phase3-day2-session".to_string());

        let err = provider
            .set_session_for_vm(&capsule_id, "abc12345", "http://127.0.0.1:3000")
            .await
            .unwrap_err();
        let msg = err.to_string();

        // Must NOT carry the old PHASE_1_STUB_MESSAGE.
        assert!(
            !msg.contains(PHASE_1_STUB_MESSAGE),
            "set_session_for_vm should no longer use the Phase 1 stub message; got: {msg}"
        );

        // Must point at the correct new API and the port plan.
        assert!(
            msg.contains("VmConfig::with_session"),
            "expected the error to name VmConfig::with_session, got: {msg}"
        );
        assert!(
            msg.contains("load_with_vm_config"),
            "expected the error to name load_with_vm_config, got: {msg}"
        );
        assert!(
            msg.contains("VZVirtualMachineConfiguration is frozen"),
            "expected the error to name Apple's constraint, got: {msg}"
        );
        assert!(
            msg.contains(&capsule_id.0),
            "expected the error to carry the capsule id, got: {msg}"
        );
    }

    #[tokio::test]
    async fn vz_provider_set_network_for_vm_returns_typed_migration_error_after_phase3_day2() {
        // Phase 3 Day 2 contract: late-binding network
        // attachment is unsupported; default Vz networking is
        // NAT (attached by the builder). Bridged mode is Phase 3
        // Day 4+ and needs Apple's `com.apple.vm.networking`
        // entitlement.
        let provider = VzProvider::new(VzConfig::default()).unwrap();
        let capsule_id = CapsuleId::new("phase3-day2-network".to_string());

        let err = provider
            .set_network_for_vm(&capsule_id, NetworkConfig::new("phase3-day2-network"))
            .await
            .unwrap_err();
        let msg = err.to_string();

        assert!(
            !msg.contains(PHASE_1_STUB_MESSAGE),
            "set_network_for_vm should no longer use the Phase 1 stub message; got: {msg}"
        );
        assert!(
            msg.contains("NAT"),
            "expected the error to mention the default NAT path, got: {msg}"
        );
        assert!(
            msg.contains("com.apple.vm.networking"),
            "expected the error to name the entitlement required for bridged mode, got: {msg}"
        );
        assert!(
            msg.contains(&capsule_id.0),
            "expected the error to carry the capsule id, got: {msg}"
        );
    }

    #[tokio::test]
    async fn vz_provider_append_boot_args_returns_typed_migration_error_after_phase3_day1() {
        // Phase 3 Day 1 contract: the old mid-life-cycle
        // append_boot_args_for_vm shape is unsupported because
        // VZVirtualMachineConfiguration is frozen after load.
        // The method exists only to surface a typed migration
        // message pointing callers at load_with_vm_config.
        let provider = VzProvider::new(VzConfig::default()).unwrap();
        let capsule_id = CapsuleId::new("phase3-day1-bootargs".to_string());

        let err = provider
            .append_boot_args_for_vm(&capsule_id, "elastos.token=ignored")
            .await
            .unwrap_err();
        let msg = err.to_string();

        // Must NOT carry the old PHASE_1_STUB_MESSAGE — Day 1
        // explicitly retires that text here so operators don't
        // confuse this with the (still-stubbed) session/network
        // surfaces.
        assert!(
            !msg.contains(PHASE_1_STUB_MESSAGE),
            "append_boot_args_for_vm should no longer use the Phase 1 stub message; got: {msg}"
        );

        // Must point at the correct new API and the port plan.
        assert!(
            msg.contains("load_with_vm_config"),
            "expected the error to name load_with_vm_config, got: {msg}"
        );
        assert!(
            msg.contains("VZVirtualMachineConfiguration is frozen"),
            "expected the error to name Apple's constraint, got: {msg}"
        );
        assert!(
            msg.contains(&capsule_id.0),
            "expected the error to carry the capsule id, got: {msg}"
        );
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

    use std::path::PathBuf;

    /// Synthesise a `VmConfig` shaped like one
    /// `Supervisor::start_capsule_vm` would build for Vz —
    /// minimal fields, no real Vz allocation required.
    fn synthetic_vm_config(name: &str, kernel: PathBuf, rootfs: PathBuf) -> VmConfig {
        VmConfig {
            vm_id: format!("microvm-{name}"),
            kernel_path: kernel,
            boot_args: String::from("console=hvc0"),
            rootfs_path: rootfs,
            rootfs_readonly: false,
            mem_size_mib: 128,
            vcpu_count: 1,
            http_port: None,
            data_disk_path: None,
            vsock_cid: 3,
            network: None,
            interactive_stdio: false,
            carrier_socket_path: None,
            initramfs_path: None,
        }
    }

    #[tokio::test]
    async fn vz_provider_load_with_vm_config_rejects_non_microvm_capsule_type() {
        // Phase 3 Day 1 contract: load_with_vm_config enforces
        // the same MicroVM-only constraint as load(), so wrong
        // capsule types fail fast before any Vz allocation
        // happens.
        let provider = VzProvider::new(VzConfig::default()).unwrap();
        let manifest = capsule_manifest("wasm-capsule", CapsuleType::Wasm);
        let vm_config = synthetic_vm_config(
            "phase3-day1-wrongtype",
            PathBuf::from("/nonexistent/kernel"),
            PathBuf::from("/nonexistent/rootfs"),
        );

        let err = provider
            .load_with_vm_config(vm_config, manifest)
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("only supports MicroVM"),
            "expected MicroVM-only error, got: {err}"
        );
    }

    #[tokio::test]
    async fn vz_provider_load_with_vm_config_returns_kernel_not_found_when_kernel_missing() {
        // Phase 3 Day 1 contract: load_with_vm_config validates
        // its inputs in the same order Day 3's load() did —
        // capsule type, then kernel, then rootfs. The supervisor
        // can rely on this order when surfacing typed errors
        // upstream.
        let provider = VzProvider::new(VzConfig::default()).unwrap();
        let manifest = capsule_manifest("phase3-day1-no-kernel", CapsuleType::MicroVM);
        let vm_config = synthetic_vm_config(
            "phase3-day1-no-kernel",
            PathBuf::from("/nonexistent/kernel-for-phase3-day1"),
            PathBuf::from("/nonexistent/rootfs-for-phase3-day1"),
        );

        let err = provider
            .load_with_vm_config(vm_config, manifest)
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("Kernel not found"),
            "expected 'Kernel not found' (first validation gate), got: {err}"
        );
    }

    #[tokio::test]
    async fn vz_provider_load_with_vm_config_preserves_baked_boot_args() {
        // Phase 3 Day 1 contract: any boot args the supervisor
        // bakes into VmConfig.boot_args (session token, command
        // payload, capsule args, carrier path, …) must reach
        // load_with_vm_config unchanged. We assert the VmConfig
        // round-trips because the production validation path
        // exits early on kernel-missing, before any FFI build.
        // The boot-args plumbing into VZLinuxBootLoader is
        // already covered by
        // ffi::builder::tests::from_vm_config_*.
        let mut vm_config = synthetic_vm_config(
            "phase3-day1-bootargs",
            PathBuf::from("/nonexistent/kernel-phase3-day1-bootargs"),
            PathBuf::from("/nonexistent/rootfs-phase3-day1-bootargs"),
        );
        let baked = "console=hvc0 elastos.token=abc elastos.carrier_path=/dev/hvc1";
        vm_config.boot_args = baked.to_string();

        assert_eq!(
            vm_config.boot_args, baked,
            "VmConfig.boot_args is a plain mutable field; the seam relies on the supervisor's pre-load mutation"
        );
        assert!(
            vm_config.boot_args.contains("elastos.token="),
            "session token survived the seam"
        );
        assert!(
            vm_config
                .boot_args
                .contains("elastos.carrier_path=/dev/hvc1"),
            "carrier path survived the seam (Mac uses /dev/hvc1, not /dev/hvc0)"
        );
    }
}
