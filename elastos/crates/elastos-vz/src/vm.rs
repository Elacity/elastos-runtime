//! Running VM placeholder (Phase 1).
//!
//! Mirrors the shape of [`elastos_crosvm::RunningVm`] so the
//! supervisor's existing state-tracking code can hold a `RunningVm`
//! from either substrate behind the [`ComputeProvider`] abstraction.
//! Phase 2 fills `start`/`stop` with real
//! `VZVirtualMachine.start`/`.requestStop` lifecycle wiring; Phase 1
//! is data-only.

use std::path::PathBuf;

use elastos_common::{CapsuleManifest, CapsuleStatus, ElastosError, Result};

use crate::config::VmConfig;
use crate::PHASE_1_STUB_MESSAGE;

/// State of a single capsule's VM as seen by the Vz provider.
///
/// In Phase 1 this struct exists only so callers can reference its
/// type. No instance is ever constructed by the provider's
/// fail-closed `load()`. Phase 2 will construct one per booted VM
/// and route delegate state-change callbacks into [`Self::status`].
pub struct RunningVm {
    /// VM configuration captured at load time.
    pub config: VmConfig,

    /// Capsule manifest the VM was launched from.
    pub manifest: CapsuleManifest,

    /// Unix-socket path crosvm uses for control; on Vz this is the
    /// per-VM state dir base. Kept as a `PathBuf` so the supervisor's
    /// existing socket-cleanup logic works unchanged.
    pub socket_path: PathBuf,

    /// Last observed status (set by the Vz delegate in Phase 2).
    pub status: CapsuleStatus,
}

impl RunningVm {
    /// Construct a (not-yet-running) VM record. Mirrors
    /// [`elastos_crosvm::RunningVm::new`] in shape.
    pub fn new(config: VmConfig, manifest: CapsuleManifest, socket_path: PathBuf) -> Self {
        Self {
            config,
            manifest,
            socket_path,
            status: CapsuleStatus::Stopped,
        }
    }

    /// Start the VM. Phase 1: fail closed. Phase 2 wires this to
    /// `VZVirtualMachine.start(...)`.
    pub async fn start(&mut self) -> Result<()> {
        Err(ElastosError::Compute(format!(
            "{} (RunningVm::start: vm_id='{}')",
            PHASE_1_STUB_MESSAGE, self.config.vm_id
        )))
    }

    /// Stop the VM. Phase 1: idempotent no-op because no VM was
    /// started (matches `RunningVm::stop` semantics on crosvm when
    /// already stopped). Phase 2 wires this to
    /// `VZVirtualMachine.requestStop` with a SIGKILL-equivalent
    /// fallback per `docs/vz-backend/PHASE_0_SCOPE.md` §D pitfall #9.
    pub async fn stop(&mut self) -> Result<()> {
        self.status = CapsuleStatus::Stopped;
        Ok(())
    }

    /// Is the VM currently running. Phase 1 always returns `false`;
    /// Phase 2 reads the cached delegate state.
    pub fn is_running(&self) -> bool {
        false
    }

    /// HTTP port forwarded for this VM, if any.
    pub fn http_port(&self) -> Option<u16> {
        self.config.http_port
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use elastos_common::{CapsuleRole, CapsuleType, MicroVmConfig, ResourceLimits, SCHEMA_V1};

    fn manifest() -> CapsuleManifest {
        CapsuleManifest {
            schema: SCHEMA_V1.into(),
            version: "0.1.0".into(),
            name: "phase1-vm".into(),
            description: None,
            author: None,
            role: CapsuleRole::App,
            capsule_type: CapsuleType::MicroVM,
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
                http_port: Some(4100),
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

    #[test]
    fn running_vm_new_captures_config_and_starts_stopped() {
        let m = manifest();
        let config =
            VmConfig::from_manifest(&m, std::path::Path::new("/c"), std::path::Path::new("/k"));
        let vm = RunningVm::new(config, m, PathBuf::from("/tmp/vm.sock"));
        assert!(matches!(vm.status, CapsuleStatus::Stopped));
        assert!(!vm.is_running());
    }

    #[tokio::test]
    async fn running_vm_start_fails_closed_with_phase_1_stub() {
        let m = manifest();
        let config =
            VmConfig::from_manifest(&m, std::path::Path::new("/c"), std::path::Path::new("/k"));
        let mut vm = RunningVm::new(config, m, PathBuf::from("/tmp/vm.sock"));

        let err = vm.start().await.unwrap_err();
        assert!(err.to_string().contains(PHASE_1_STUB_MESSAGE));
    }

    #[tokio::test]
    async fn running_vm_stop_is_idempotent_in_phase_1() {
        let m = manifest();
        let config =
            VmConfig::from_manifest(&m, std::path::Path::new("/c"), std::path::Path::new("/k"));
        let mut vm = RunningVm::new(config, m, PathBuf::from("/tmp/vm.sock"));

        // Calling stop on a never-started VM must not error — same
        // contract as crosvm's RunningVm::stop.
        vm.stop().await.unwrap();
        vm.stop().await.unwrap();
    }

    #[test]
    fn running_vm_http_port_passes_through_from_config() {
        let m = manifest();
        let config =
            VmConfig::from_manifest(&m, std::path::Path::new("/c"), std::path::Path::new("/k"));
        let vm = RunningVm::new(config, m, PathBuf::from("/tmp/vm.sock"));
        assert_eq!(vm.http_port(), Some(4100));
    }
}
