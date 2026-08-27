//! Running VM record.
//!
//! Mirrors the shape of [`elastos_crosvm::RunningVm`] so the
//! supervisor's existing state-tracking code can hold a
//! `RunningVm` from either substrate behind the
//! [`ComputeProvider`] abstraction.
//!
//! - On macOS, [`RunningVm`] optionally carries a
//!   [`crate::ffi::lifecycle::VzMachineHandle`] populated by
//!   [`crate::VzProvider::load`]. `start` / `stop` /
//!   `is_running` delegate to it.
//! - On non-macOS platforms, the same struct compiles as a
//!   fail-closed stub (the FFI handle is gated to macOS).
//! - The [`RunningVm::new`] three-argument constructor builds a
//!   data-only record for tests and trusted callers that do not
//!   have a live Vz handle. On macOS it produces an "incomplete"
//!   record whose `start` fails closed with
//!   [`crate::VZ_BACKEND_UNAVAILABLE_MESSAGE`].

use std::path::{Path, PathBuf};

use elastos_common::{CapsuleManifest, CapsuleStatus, ElastosError, Result};

use crate::config::VmConfig;
// `VzError` is consumed only inside `#[cfg(target_os = "macos")]`
// blocks in this file (struct fields, accessors, test-only setters, error
// classification in `stop`). On Linux the import would sit unused and CI's
// `-D warnings` flag turns the unused-import lint into a hard error.
// `VzExitReason` is referenced in the public `last_exit_reason()` signature
// on both platforms and stays unconditional.
#[cfg(target_os = "macos")]
use crate::error::VzError;
use crate::error::VzExitReason;

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
    /// constructed via the data-only [`Self::new`] path (which
    /// stays fail-closed for back-compat with data-only tests).
    #[cfg(target_os = "macos")]
    handle: Option<crate::ffi::lifecycle::VzMachineHandle>,

    /// Host-side endpoint of the Carrier console socketpair
    /// (macOS only). Populated by
    /// [`crate::VzProvider::load_with_vm_config`]; the
    /// supervisor takes it via [`Self::take_carrier_host_fd`]
    /// and feeds it to the Carrier bridge so bytes flow
    /// guest↔host on `/dev/hvc1`.
    ///
    /// Absent for records constructed via the data-only
    /// [`Self::new`] path (no Vz wiring) — the supervisor's
    /// dispatch arms handle the `None` case as "no real
    /// Carrier channel, log and continue".
    #[cfg(target_os = "macos")]
    carrier_host_fd: Option<std::os::fd::OwnedFd>,

    /// Typed Vz error from the most recent failed
    /// [`Self::stop`] (macOS only). Populated
    /// before `stop` returns so the supervisor can read
    /// `last_vz_error()` for structured telemetry without
    /// re-parsing the [`ElastosError`] string surface.
    #[cfg(target_os = "macos")]
    last_vz_error: Option<VzError>,

    /// Typed Vz exit reason from the most recent terminal
    /// observation (macOS only). Populated by
    /// [`Self::stop`] on host-initiated stops and by
    /// [`Self::wait_for_exit_code`] on delegate-observed exits;
    /// the supervisor surfaces this via
    /// `SupervisorResponse::last_exit_reason`.
    #[cfg(target_os = "macos")]
    last_exit_reason: Option<VzExitReason>,
}

impl RunningVm {
    /// Legacy data-only constructor. Production Vz launches build a
    /// complete record via [`Self::with_handle`] (macOS-only).
    pub fn new(config: VmConfig, manifest: CapsuleManifest, socket_path: PathBuf) -> Self {
        Self {
            config,
            manifest,
            socket_path,
            status: CapsuleStatus::Stopped,
            #[cfg(target_os = "macos")]
            handle: None,
            #[cfg(target_os = "macos")]
            carrier_host_fd: None,
            #[cfg(target_os = "macos")]
            last_vz_error: None,
            #[cfg(target_os = "macos")]
            last_exit_reason: None,
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
        carrier_host_fd: std::os::fd::OwnedFd,
    ) -> Self {
        Self {
            config,
            manifest,
            socket_path,
            status: CapsuleStatus::Stopped,
            handle: Some(handle),
            carrier_host_fd: Some(carrier_host_fd),
            last_vz_error: None,
            last_exit_reason: None,
        }
    }

    /// Typed Vz error from the most recent failed [`Self::stop`]
    /// call (macOS only). Returns `None` on success, before any
    /// stop attempt, or on platforms without Vz.
    ///
    /// The supervisor reads this immediately after `stop`
    /// returns `Err` so it can populate
    /// `SupervisorResponse::last_exit_reason` with the typed
    /// telemetry label (e.g. `"forced_after_timeout"`,
    /// `"vz_internal"`) without re-parsing the [`ElastosError`]
    /// string surface.
    #[cfg(target_os = "macos")]
    pub fn last_vz_error(&self) -> Option<&VzError> {
        self.last_vz_error.as_ref()
    }

    /// Typed Vz exit reason from the most recent terminal
    /// observation (macOS only). Returns `None` before any stop
    /// or `wait_for_exit_code` call.
    ///
    /// The supervisor reads this after `stop` returns `Ok` (or
    /// after `wait_for_exit_code` resolves) so the
    /// `SupervisorResponse::last_exit_reason` JSON field is
    /// populated with the canonical telemetry label
    /// (`"guest_clean_stop"`, `"host_initiated_stop"`,
    /// `"forced_after_timeout"`, `"stopped_with_error"`).
    pub fn last_exit_reason(&self) -> Option<VzExitReason> {
        #[cfg(target_os = "macos")]
        {
            self.last_exit_reason
        }
        #[cfg(not(target_os = "macos"))]
        {
            None
        }
    }

    /// Test-only injection point for [`Self::last_exit_reason`].
    ///
    /// Synthetic supervisor tests in `elastos-server` need to
    /// drive `capsule_status` / `stop_capsule` through every
    /// `VzExitReason` variant without having to spin up a real
    /// Vz VM and provoke each terminal state (impossible in CI
    /// without an Apple runner). This
    /// hook is `#[doc(hidden)]` so it does not appear in the
    /// public API surface; it is not part of any contract and
    /// production code MUST NOT call it.
    #[doc(hidden)]
    #[cfg(target_os = "macos")]
    pub fn set_last_exit_reason_for_testing(&mut self, reason: VzExitReason) {
        self.last_exit_reason = Some(reason);
    }

    /// Test-only injection point for [`Self::last_vz_error`].
    ///
    /// Supervisor tests need to drive the new `CapsuleVzError`
    /// RPC through every [`VzError`] variant without provoking
    /// real Apple NSErrors (impossible to provoke in CI without
    /// an Apple runner). `#[doc(hidden)]` so production code MUST NOT
    /// call it.
    #[doc(hidden)]
    #[cfg(target_os = "macos")]
    pub fn set_last_vz_error_for_testing(&mut self, err: VzError) {
        self.last_vz_error = Some(err);
    }

    /// Test-only setter for the cached lifecycle [`status`][CapsuleStatus]
    /// field.
    ///
    /// Synthetic Vz capsules constructed via [`Self::new`] have
    /// no Vz handle attached so [`Self::is_running`] defers to
    /// the cached `status`. Supervisor tests need to mark such
    /// records as `Running` to keep
    /// [`crate::Supervisor::reap_dead_capsules`][reap] from
    /// pruning them mid-test. `#[doc(hidden)]` because this is
    /// a test fixture, not a real API.
    ///
    /// [reap]: # "elastos-server/src/supervisor.rs::Supervisor::reap_dead_capsules"
    #[doc(hidden)]
    pub fn set_status_for_testing(&mut self, status: CapsuleStatus) {
        self.status = status;
    }

    /// Take the host-side carrier console fd, leaving `None` in
    /// its place. The supervisor calls this
    /// exactly once per VM, immediately after
    /// `VzProvider::take_running_vm`, to feed the Carrier
    /// bridge dispatch loop.
    ///
    /// Subsequent calls return `None` — the fd has already been
    /// handed off and the bridge owns it.
    #[cfg(target_os = "macos")]
    pub fn take_carrier_host_fd(&mut self) -> Option<std::os::fd::OwnedFd> {
        self.carrier_host_fd.take()
    }

    /// Start the VM.
    ///
    /// - macOS + handle present: dispatch
    ///   `VZVirtualMachine.startWithCompletionHandler` and wait.
    /// - macOS + handle missing: fail closed with the shared
    ///   unavailable-backend message (data-only records).
    /// - non-macOS: fail closed with the shared unavailable-backend message.
    pub async fn start(&mut self) -> Result<()> {
        #[cfg(target_os = "macos")]
        {
            let handle = self.handle.as_ref().ok_or_else(|| {
                ElastosError::Compute(format!(
                    "{} (RunningVm::start: vm_id='{}', handle missing — use VzProvider::load)",
                    crate::VZ_BACKEND_UNAVAILABLE_MESSAGE,
                    self.config.vm_id
                ))
            })?;
            // typed `VzError` from the handle is
            // converted to `ElastosError::Compute` at the trait
            // boundary so the public surface stays
            // backward-compatible. The kind_label prefix (e.g.
            // `vz_internal: …`) survives via `VzError::Display`
            // so log-grep telemetry still recognises the
            // classification.
            handle
                .start()
                .await
                .map_err(|e| ElastosError::Compute(e.to_string()))?;
            // Successful start clears any previously-cached
            // failure metadata — operators looking at
            // `last_vz_error()` after a restart see "no error
            // since this start" not a stale pre-restart error.
            self.last_vz_error = None;
            self.last_exit_reason = None;
            self.status = CapsuleStatus::Running;
            Ok(())
        }

        #[cfg(not(target_os = "macos"))]
        {
            Err(ElastosError::Compute(format!(
                "{} (RunningVm::start: vm_id='{}')",
                crate::VZ_BACKEND_UNAVAILABLE_MESSAGE,
                self.config.vm_id
            )))
        }
    }

    /// Stop the VM. Idempotent: returns `Ok(())` if no Vz handle
    /// is attached (mirrors crosvm's `RunningVm::stop` semantics
    /// when already stopped). On macOS with an attached handle,
    /// dispatches `VZVirtualMachine.stopWithCompletionHandler`.
    ///
    /// : on both success and failure paths the
    /// typed [`VzError`] / [`VzExitReason`] is captured on this
    /// record for the supervisor to read via [`Self::last_vz_error`]
    /// / [`Self::last_exit_reason`] when populating
    /// `SupervisorResponse::last_exit_reason`. The public
    /// signature stays `Result<()>` to keep the
    /// [`ComputeProvider`] trait surface backward-compatible.
    pub async fn stop(&mut self) -> Result<()> {
        #[cfg(target_os = "macos")]
        {
            if let Some(handle) = self.handle.as_ref() {
                match handle.stop().await {
                    Ok(()) => {
                        self.last_vz_error = None;
                        // `VzMachineHandle::stop` only resolves
                        // `Ok` on the host-initiated path; the
                        // guest-clean / stopped-with-error /
                        // forced-after-timeout cases route via
                        // `wait_for_exit_code` and `Err(...)`
                        // respectively.
                        self.last_exit_reason = Some(VzExitReason::HostInitiatedStop);
                    }
                    Err(err) => {
                        let label = err.kind_label();
                        // Map timeout into the typed exit
                        // reason directly so the supervisor's
                        // `last_exit_reason` JSON field reports
                        // `"forced_after_timeout"` even though
                        // `stop` is returning Err.
                        self.last_exit_reason = if matches!(err, VzError::TimedOut { .. }) {
                            Some(VzExitReason::ForcedAfterTimeout)
                        } else {
                            None
                        };
                        let formatted = err.to_string();
                        self.last_vz_error = Some(err);
                        // Status remains `Stopped` even on error,
                        // since
                        // the supervisor performs best-effort
                        // cleanup either way.
                        self.status = CapsuleStatus::Stopped;
                        return Err(ElastosError::Compute(format!(
                            "{label}: {formatted}",
                            formatted = formatted
                        )));
                    }
                }
            }
        }
        self.status = CapsuleStatus::Stopped;
        Ok(())
    }

    /// Pause, save machine state to `path`, and leave the VM
    /// paused for the caller to stop. macOS VZ only; other
    /// substrates fail closed.
    pub async fn hibernate_to(&mut self, path: &Path) -> Result<()> {
        #[cfg(target_os = "macos")]
        {
            let handle = self.handle.as_ref().ok_or_else(|| {
                ElastosError::Compute(format!(
                    "{} (RunningVm::hibernate_to: vm_id='{}', handle missing — use VzProvider::load_with_vm_config)",
                    crate::VZ_BACKEND_UNAVAILABLE_MESSAGE,
                    self.config.vm_id
                ))
            })?;
            handle
                .pause()
                .await
                .map_err(|err| ElastosError::Compute(err.to_string()))?;
            handle
                .save_machine_state(path)
                .await
                .map_err(|err| ElastosError::Compute(err.to_string()))?;
            self.status = CapsuleStatus::Stopped;
            Ok(())
        }

        #[cfg(not(target_os = "macos"))]
        {
            let _ = path;
            Err(ElastosError::Compute(format!(
                "{} (RunningVm::hibernate_to: vm_id='{}')",
                crate::VZ_BACKEND_UNAVAILABLE_MESSAGE,
                self.config.vm_id
            )))
        }
    }

    /// Restore a saved machine state and resume the VM. macOS VZ
    /// only; other substrates fail closed.
    pub async fn restore_from_hibernation(&mut self, path: &Path) -> Result<()> {
        #[cfg(target_os = "macos")]
        {
            let handle = self.handle.as_ref().ok_or_else(|| {
                ElastosError::Compute(format!(
                    "{} (RunningVm::restore_from_hibernation: vm_id='{}', handle missing — use VzProvider::load_with_vm_config)",
                    crate::VZ_BACKEND_UNAVAILABLE_MESSAGE,
                    self.config.vm_id
                ))
            })?;
            handle
                .restore_machine_state(path)
                .await
                .map_err(|err| ElastosError::Compute(err.to_string()))?;
            handle
                .resume()
                .await
                .map_err(|err| ElastosError::Compute(err.to_string()))?;
            self.last_vz_error = None;
            self.last_exit_reason = None;
            self.status = CapsuleStatus::Running;
            Ok(())
        }

        #[cfg(not(target_os = "macos"))]
        {
            let _ = path;
            Err(ElastosError::Compute(format!(
                "{} (RunningVm::restore_from_hibernation: vm_id='{}')",
                crate::VZ_BACKEND_UNAVAILABLE_MESSAGE,
                self.config.vm_id
            )))
        }
    }

    /// Whether this VM configuration passed Apple's save/restore
    /// validation.
    pub fn supports_hibernation(&self) -> bool {
        #[cfg(target_os = "macos")]
        {
            self.handle
                .as_ref()
                .is_some_and(|handle| handle.supports_save_restore())
        }
        #[cfg(not(target_os = "macos"))]
        {
            false
        }
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

    /// Dial the guest's vsock listener on `port` and return an
    /// owned host-side fd.
    ///
    /// macOS + handle present: delegates to
    /// [`VzMachineHandle::connect_vsock`][handle], which calls
    /// Apple's `VZVirtioSocketDevice.connectToPort:`.
    ///
    /// macOS + handle missing (data-only [`Self::new`] record):
    /// fails closed with the shared unavailable-backend message; there is
    /// no Vz VM to dial.
    ///
    /// Non-macOS: fails closed; vsock is the wrong abstraction
    /// for the substrate.
    ///
    /// [handle]: crate::ffi::lifecycle::VzMachineHandle::connect_vsock
    pub async fn connect_vsock(&self, port: u32) -> Result<std::os::fd::OwnedFd> {
        #[cfg(target_os = "macos")]
        {
            let handle = self.handle.as_ref().ok_or_else(|| {
                ElastosError::Compute(format!(
                    "{} (RunningVm::connect_vsock: vm_id='{}', handle missing — use VzProvider::load_with_vm_config)",
                    crate::VZ_BACKEND_UNAVAILABLE_MESSAGE,
                    self.config.vm_id
                ))
            })?;
            handle
                .connect_vsock(port)
                .await
                .map_err(ElastosError::Compute)
        }

        #[cfg(not(target_os = "macos"))]
        {
            let _ = port; // suppress unused warning on Linux.
            Err(ElastosError::Compute(format!(
                "{} (RunningVm::connect_vsock: vm_id='{}')",
                crate::VZ_BACKEND_UNAVAILABLE_MESSAGE,
                self.config.vm_id
            )))
        }
    }

    /// Wait for the VM to reach a terminal state and return a
    /// classified exit code.
    ///
    /// Uses a delegate-driven `tokio::sync::oneshot`. Exit codes:
    ///
    /// - `0` — guest-initiated clean shutdown
    ///   (`guestDidStopVirtualMachine:`), or host-initiated
    ///   stop via [`Self::stop`] that succeeded.
    /// - `1` — Apple tore the VM down because of an internal
    ///   error (`virtualMachine:didStopWithError:`).
    ///
    /// Network-attachment disconnects are logged but do not
    /// terminate the VM and do not resolve this future.
    ///
    /// On non-macOS this method fails closed — the substrate
    /// has no VM to wait on.
    pub async fn wait_for_exit_code(&mut self) -> Result<i32> {
        #[cfg(target_os = "macos")]
        {
            let Some(handle) = self.handle.as_ref() else {
                return Err(ElastosError::Compute(format!(
                    "{} (wait_for_exit_code: vm_id='{}', handle missing — use VzProvider::load_with_vm_config)",
                    crate::VZ_BACKEND_UNAVAILABLE_MESSAGE,
                    self.config.vm_id
                )));
            };
            // use the typed `wait_for_exit_classified`
            // so we can capture the `VzExitReason` for the
            // supervisor's `last_exit_reason` telemetry while
            // preserving the public `i32` exit-code surface.
            let reason = handle
                .wait_for_exit_classified()
                .await
                .map_err(ElastosError::Compute)?;
            self.last_exit_reason = Some(reason);
            self.status = CapsuleStatus::Stopped;
            Ok(reason.exit_code())
        }

        #[cfg(not(target_os = "macos"))]
        {
            Err(ElastosError::Compute(format!(
                "{} (wait_for_exit_code: vm_id='{}')",
                crate::VZ_BACKEND_UNAVAILABLE_MESSAGE,
                self.config.vm_id
            )))
        }
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
            name: "vz-loaded-vm".into(),
            description: None,
            author: None,
            role: CapsuleRole::App,
            capsule_type: CapsuleType::MicroVM,
            runtime_abi: None,
            bus_contract: None,
            wit_world_sha256: None,
            execution: None,
            projections: Vec::new(),
            entrypoint: "rootfs.ext4".into(),
            requires: Vec::new(),
            provides: None,
            capabilities: Vec::new(),
            interfaces: Vec::new(),
            resources: ResourceLimits {
                memory_mb: 128,
                cpu_shares: 100,
                gpu: false,
            },
            permissions: Default::default(),
            // v0.3.0 added the principal-binding `authority` field; None
            // here = "no authority constraint" for in-module unit tests.
            authority: None,
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
            icon: None,
            viewer: None,
            signature: None,
        }
    }

    fn make_data_only_vm() -> RunningVm {
        let m = manifest();
        let config =
            VmConfig::from_manifest(&m, std::path::Path::new("/c"), std::path::Path::new("/k"));
        RunningVm::new(config, m, PathBuf::from("/tmp/vm.sock"))
    }

    #[test]
    fn running_vm_new_captures_config_and_starts_stopped() {
        let vm = make_data_only_vm();
        assert!(matches!(vm.status, CapsuleStatus::Stopped));
        assert!(!vm.is_running());
    }

    #[tokio::test]
    async fn running_vm_start_without_handle_fails_closed() {
        // Callers that construct a `RunningVm` via the data-only `new`
        // path get the documented fail-closed error.
        // `VzProvider::load` builds a fully-wired record via
        // `with_handle` (macOS only).
        let mut vm = make_data_only_vm();
        let err = vm.start().await.unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains(crate::VZ_BACKEND_UNAVAILABLE_MESSAGE),
            "expected unavailable-backend message, got: {msg}"
        );
    }

    #[tokio::test]
    async fn running_vm_stop_is_idempotent_for_data_only_record() {
        let mut vm = make_data_only_vm();
        vm.stop().await.unwrap();
        vm.stop().await.unwrap();
    }

    #[test]
    fn running_vm_http_port_passes_through_from_config() {
        let vm = make_data_only_vm();
        assert_eq!(vm.http_port(), Some(4100));
    }
}
