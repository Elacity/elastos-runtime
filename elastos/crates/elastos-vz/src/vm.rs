//! Running VM record.
//!
//! Mirrors the shape of [`elastos_crosvm::RunningVm`] so the
//! supervisor's existing state-tracking code can hold a
//! `RunningVm` from either substrate behind the
//! [`ComputeProvider`] abstraction.
//!
//! As of Phase 2 Day 3:
//!
//! - On macOS, [`RunningVm`] optionally carries a
//!   [`crate::ffi::lifecycle::VzMachineHandle`] populated by
//!   [`crate::VzProvider::load`]. `start` / `stop` /
//!   `is_running` delegate to it.
//! - On non-macOS platforms, the same struct compiles as a
//!   fail-closed stub (the FFI handle is gated to macOS).
//! - The legacy [`RunningVm::new`] three-argument constructor is
//!   preserved so the Phase 1 tests and any caller that just
//!   wants the data half of the record keep working; on macOS
//!   it produces an "incomplete" record whose `start` fails
//!   closed with [`crate::PHASE_1_STUB_MESSAGE`] — same contract
//!   as before, just narrower in scope.

use std::path::PathBuf;

use elastos_common::{CapsuleManifest, CapsuleStatus, ElastosError, Result};

use crate::config::VmConfig;

/// State of a single capsule's VM as seen by the Vz provider.
pub struct RunningVm {
    /// VM configuration captured at load time.
    pub config: VmConfig,

    /// Capsule manifest the VM was launched from.
    pub manifest: CapsuleManifest,

    /// Unix-socket path used for cleanup. On Vz this is the
    /// per-VM state dir base — the supervisor's existing
    /// socket-cleanup logic works unchanged.
    pub socket_path: PathBuf,

    /// Last observed status. Updated by [`Self::start`] /
    /// [`Self::stop`].
    pub status: CapsuleStatus,

    /// Live Vz handle (macOS only). Populated by
    /// [`crate::VzProvider::load`]; absent for records
    /// constructed via the legacy [`Self::new`] path (which
    /// stays fail-closed for back-compat with Phase 1 tests).
    #[cfg(target_os = "macos")]
    handle: Option<crate::ffi::lifecycle::VzMachineHandle>,
}

impl RunningVm {
    /// Legacy data-only constructor. Phase 1 callers use this;
    /// Phase 2's `VzProvider::load` builds a complete record via
    /// [`Self::with_handle`] (macOS-only).
    pub fn new(config: VmConfig, manifest: CapsuleManifest, socket_path: PathBuf) -> Self {
        Self {
            config,
            manifest,
            socket_path,
            status: CapsuleStatus::Stopped,
            #[cfg(target_os = "macos")]
            handle: None,
        }
    }

    /// Construct a `RunningVm` already bound to a Vz handle.
    /// `VzProvider::load` is the only caller.
    #[cfg(target_os = "macos")]
    pub(crate) fn with_handle(
        config: VmConfig,
        manifest: CapsuleManifest,
        socket_path: PathBuf,
        handle: crate::ffi::lifecycle::VzMachineHandle,
    ) -> Self {
        Self {
            config,
            manifest,
            socket_path,
            status: CapsuleStatus::Stopped,
            handle: Some(handle),
        }
    }

    /// Start the VM.
    ///
    /// - macOS + handle present: dispatch
    ///   `VZVirtualMachine.startWithCompletionHandler` and wait.
    /// - macOS + handle missing: fail-closed with the Phase 1
    ///   stub message (legacy callers).
    /// - non-macOS: fail-closed with the Phase 1 stub message.
    pub async fn start(&mut self) -> Result<()> {
        #[cfg(target_os = "macos")]
        {
            let handle = self.handle.as_ref().ok_or_else(|| {
                ElastosError::Compute(format!(
                    "{} (RunningVm::start: vm_id='{}', handle missing — use VzProvider::load)",
                    crate::PHASE_1_STUB_MESSAGE,
                    self.config.vm_id
                ))
            })?;
            handle.start().await.map_err(ElastosError::Compute)?;
            self.status = CapsuleStatus::Running;
            Ok(())
        }

        #[cfg(not(target_os = "macos"))]
        {
            Err(ElastosError::Compute(format!(
                "{} (RunningVm::start: vm_id='{}')",
                crate::PHASE_1_STUB_MESSAGE,
                self.config.vm_id
            )))
        }
    }

    /// Stop the VM. Idempotent: returns `Ok(())` if no Vz handle
    /// is attached (mirrors crosvm's `RunningVm::stop` semantics
    /// when already stopped). On macOS with an attached handle,
    /// dispatches `VZVirtualMachine.stopWithCompletionHandler`.
    pub async fn stop(&mut self) -> Result<()> {
        #[cfg(target_os = "macos")]
        {
            if let Some(handle) = self.handle.as_ref() {
                handle.stop().await.map_err(ElastosError::Compute)?;
            }
        }
        self.status = CapsuleStatus::Stopped;
        Ok(())
    }

    /// Is the VM currently running.
    ///
    /// macOS: queries Vz's `state` property through the
    /// dispatch queue and compares against [`Running`][running].
    /// Other platforms / no-handle: returns the cached `status`
    /// field.
    ///
    /// [running]: crate::ffi::lifecycle
    pub fn is_running(&self) -> bool {
        #[cfg(target_os = "macos")]
        {
            if let Some(handle) = self.handle.as_ref() {
                return matches!(
                    handle.current_state(),
                    crate::ffi::lifecycle::VmState::Running
                );
            }
        }
        matches!(self.status, CapsuleStatus::Running)
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
            name: "phase2-day3-vm".into(),
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

    fn make_legacy_vm() -> RunningVm {
        let m = manifest();
        let config =
            VmConfig::from_manifest(&m, std::path::Path::new("/c"), std::path::Path::new("/k"));
        RunningVm::new(config, m, PathBuf::from("/tmp/vm.sock"))
    }

    #[test]
    fn running_vm_new_captures_config_and_starts_stopped() {
        let vm = make_legacy_vm();
        assert!(matches!(vm.status, CapsuleStatus::Stopped));
        assert!(!vm.is_running());
    }

    #[tokio::test]
    async fn running_vm_start_without_handle_fails_closed_with_phase_1_stub() {
        // The Phase 1 contract survives Day 3 for callers that
        // construct a `RunningVm` via the legacy `new` path.
        // `VzProvider::load` builds a fully-wired record via
        // `with_handle` (macOS only); everyone else gets the
        // documented fail-closed error.
        let mut vm = make_legacy_vm();
        let err = vm.start().await.unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains(crate::PHASE_1_STUB_MESSAGE),
            "expected stub message, got: {msg}"
        );
    }

    #[tokio::test]
    async fn running_vm_stop_is_idempotent_for_legacy_record() {
        let mut vm = make_legacy_vm();
        vm.stop().await.unwrap();
        vm.stop().await.unwrap();
    }

    #[test]
    fn running_vm_http_port_passes_through_from_config() {
        let vm = make_legacy_vm();
        assert_eq!(vm.http_port(), Some(4100));
    }
}
