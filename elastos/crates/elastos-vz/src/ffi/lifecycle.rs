//! Running-VM lifecycle: validate → init → start → stop.
//!
//! Bridges Apple's GCD-backed `VZVirtualMachine` to Tokio
//! async/await. Responsibilities:
//!
//! - Validate the configuration produced by [`ffi::builder`].
//! - Construct a `VZVirtualMachine` bound to the per-provider
//!   dispatch queue.
//! - Issue `startWithCompletionHandler:` / `stopWithCompletionHandler:`
//!   from the dispatch queue and forward their result onto a
//!   Tokio `oneshot`.
//! - Read the `state` property on demand.
//! - Hold the kernel-console host-read fd until [`VzMachineHandle::stop`]
//!   joins the [`console_forwarder`][super::console_forwarder]
//!   that drains it.
//!
//! Not handled here:
//!
//! - Code-signing / entitlements provisioning. Without the
//!   `com.apple.security.virtualization` entitlement Apple's
//!   `validateWithError` fails; we surface that with a single,
//!   operator-friendly error string that points at the codesign script.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use objc2::rc::Retained;
use objc2::AnyThread;
use objc2_foundation::{NSError, NSString, NSURL};
use objc2_virtualization::{
    VZVirtualMachine, VZVirtualMachineConfiguration, VZVirtualMachineState,
};

use super::console_forwarder::{spawn_console_forwarder, ConsoleForwarder};
use super::delegate::{DelegateExit, ElastosVzDelegate, SendableDelegate, SharedExitSender};
use super::dispatch::VzDispatchQueue;
use super::error::ns_error_to_string;
use crate::error::{VzError, VzExitReason};

/// Operator-facing hint appended to the error returned by
/// [`VzMachineHandle::new`] when Apple's `validateWithError`
/// rejects the configuration for missing entitlements. The hint
/// string is the single source of truth so docs, errors and tests
/// can agree.
pub(crate) const ENTITLEMENT_HINT: &str =
    "missing com.apple.security.virtualization entitlement — sign the binary with \
     scripts/dev/sign-elastos-vz/ or see docs/MAC.md";

/// Mirrors `VZVirtualMachineState` with a Rust-idiomatic enum so
/// the supervisor and status reporting do not depend on raw
/// `NSInteger` values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum VmState {
    Stopped,
    Running,
    Paused,
    Starting,
    Pausing,
    Resuming,
    Stopping,
    Saving,
    Restoring,
    Error,
    /// A new VZVirtualMachineState value Apple may add in a
    /// future macOS we haven't tested against. We treat it as
    /// `Error` for safety but surface the raw integer so logs
    /// stay diagnosable.
    Unknown(isize),
}

impl From<VZVirtualMachineState> for VmState {
    fn from(value: VZVirtualMachineState) -> Self {
        match value {
            VZVirtualMachineState::Stopped => Self::Stopped,
            VZVirtualMachineState::Running => Self::Running,
            VZVirtualMachineState::Paused => Self::Paused,
            VZVirtualMachineState::Starting => Self::Starting,
            VZVirtualMachineState::Pausing => Self::Pausing,
            VZVirtualMachineState::Resuming => Self::Resuming,
            VZVirtualMachineState::Stopping => Self::Stopping,
            VZVirtualMachineState::Saving => Self::Saving,
            VZVirtualMachineState::Restoring => Self::Restoring,
            VZVirtualMachineState::Error => Self::Error,
            // NSInteger is already `isize` on every Apple platform
            // we support, so no cast is needed.
            other => Self::Unknown(other.0),
        }
    }
}

/// `Retained<VZVirtualMachine>` wrapped with `unsafe impl Send`.
///
/// Apple's threading model: all `VZVirtualMachine` calls must
/// happen on the dispatch queue we associated at init time. We
/// guarantee that invariant by funnelling every access through
/// the `VzDispatchQueue` exec_sync / exec_async APIs. Inside the
/// queue's closure the thread is whatever GCD chose, so we need
/// `Send` to move the `Retained` across the Tokio boundary into
/// the closure that finally executes on the queue.
///
/// **Safety contract** for users of [`SendableVm`]:
/// - Never call a `VZVirtualMachine` method without first
///   re-entering the associated dispatch queue.
/// - Never deref the inner `Retained` from arbitrary threads.
pub(crate) struct SendableVm(pub(crate) Retained<VZVirtualMachine>);

// SAFETY: see the type-level docstring. Every external use of
// `SendableVm.0` goes through `VzDispatchQueue::exec_sync` or
// `exec_async`, which serialises access onto a single queue.
unsafe impl Send for SendableVm {}

// SAFETY: same as `Send` — read access (e.g. cloning the
// Retained, comparing addresses) is safe because the underlying
// NSObject pointer is itself stable and the dispatch queue
// serialises mutations.
unsafe impl Sync for SendableVm {}

/// Handle to a running (or stopped) Vz VM. One per loaded
/// capsule.
pub(crate) struct VzMachineHandle {
    /// Apple's VZVirtualMachine instance.
    vm: Arc<SendableVm>,

    /// The dispatch queue this VM is associated with. The same
    /// queue must be used for every call. We `clone()` the
    /// `DispatchRetained` (cheap refcount bump) for each
    /// `exec_sync` / `exec_async`.
    queue: Arc<VzDispatchQueue>,

    /// Diagnostic identifier embedded in tracing events. Matches
    /// `VmConfig::vm_id`.
    vm_id: String,

    /// Kernel-console forwarder spawned in [`Self::new`]. Held
    /// for the handle's lifetime; Tokio's `JoinHandle` is
    /// detached on drop, so when this struct drops the
    /// `VZVirtualMachine` releases the kernel-console
    /// `NSFileHandle`, the pipe's write end closes, the
    /// forwarder's blocking read returns 0, and the task ends
    /// cleanly. We never `await`/`abort` it directly because
    /// Apple keeps the write fd open across `stop`, so a forced
    /// join would block until drop anyway.
    ///
    /// `None` for the interactive-stdio variant: in that build
    /// path Vz is wired directly to the operator's
    /// stdin/stdout so there is no in-process pipe to forward.
    #[allow(dead_code)]
    forwarder: Option<ConsoleForwarder>,

    /// Held to keep the delegate alive — Apple's
    /// `setDelegate:` uses a weak reference, so the
    /// `VZVirtualMachine` does not retain it. Dropping this
    /// field after the VM is gone is a no-op; dropping it
    /// while the VM is still alive would invalidate Apple's
    /// weak pointer.
    #[allow(dead_code)]
    delegate: SendableDelegate,

    /// Shared sender to the exit oneshot — also held inside
    /// the delegate's ivars. First-to-take-it-wins:
    /// - The delegate takes it on
    ///   `guestDidStopVirtualMachine:` or
    ///   `virtualMachine:didStopWithError:`.
    /// - [`Self::stop`] takes it after a successful host
    ///   `stopWithCompletionHandler:` so `wait_for_exit`
    ///   resolves on intentional shutdowns too.
    exit_state: SharedExitSender,

    /// Receiver consumed by [`Self::wait_for_exit`] exactly
    /// once. Subsequent calls return a typed error rather than
    /// hang forever — the supervisor enforces this with a
    /// single waiter per capsule.
    exit_rx: Mutex<Option<tokio::sync::oneshot::Receiver<DelegateExit>>>,

    /// Upper bound on `stopWithCompletionHandler:` wait. macOS has
    /// no `kill -9` equivalent on a Vz VM. If Apple's
    /// completion block doesn't fire within this budget,
    /// [`Self::stop`] returns a typed error and signals
    /// [`DelegateExit::ForcedAfterTimeout`] so any concurrent
    /// `wait_for_exit` resolves rather than hanging forever.
    stop_timeout: Duration,

    /// `validateSaveRestoreSupportWithError:` result captured at
    /// VM construction time. Apple requires save/restore support
    /// to be checked against the exact VM configuration; storing
    /// the failure string here lets hibernation fail closed before
    /// issuing a save or restore operation.
    save_restore_support_error: Option<String>,
}

impl VzMachineHandle {
    /// Validate the assembled machine, instantiate the
    /// `VZVirtualMachine` on the provided queue and spawn the
    /// kernel-console forwarder.
    ///
    /// Takes a destructured `BuiltMachine` so the caller (the
    /// `VzProvider`) can split off the `carrier_host_fd` and
    /// keep ownership of it for the supervisor's Carrier
    /// bridge. The `carrier_console`
    /// `Retained` is held by the `VZVirtualMachineConfiguration`
    /// already (via `setConsoleDevices`), so it does not need
    /// to be kept here.
    pub(crate) fn new(
        vz_config: Retained<VZVirtualMachineConfiguration>,
        kernel_console_host_read: Option<std::fs::File>,
        vm_id: String,
        stop_timeout: Duration,
    ) -> Result<Self, String> {
        // per-VM dispatch queue. Apple's
        // threading rules apply per `VZVirtualMachine` — each VM
        // gets its own serial queue so concurrent launches do
        // not serialize through a single GCD queue. The label
        // embeds `vm_id` so `Instruments.app` / `lldb` can
        // attribute traces back to the right capsule.
        let queue = Arc::new(VzDispatchQueue::new(&format!("elastos-vz.vm.{vm_id}")));
        // SAFETY: validateWithError runs entirely on the calling
        // thread; the documentation does not require a dispatch
        // queue for validation.
        if let Err(err) = unsafe { vz_config.validateWithError() } {
            return Err(validate_error_message(&err, &vm_id));
        }

        let save_restore_support_error =
            match unsafe { vz_config.validateSaveRestoreSupportWithError() } {
                Ok(()) => None,
                Err(err) => Some(ns_error_to_string(&err)),
            };

        // Spawn the console forwarder before the VM starts —
        // any stray bytes the kernel emits during early boot are
        // captured. The forwarder is idle until the guest writes
        // its first byte.
        //
        // when `interactive_stdio` was set on the
        // VmConfig, the builder wires Vz directly to host
        // stdin/stdout and there's no host_read pipe to forward.
        // The lifecycle skips the spawn in that branch; the
        // operator's terminal becomes the forwarder.
        let forwarder = kernel_console_host_read
            .map(|host_read| spawn_console_forwarder(host_read, vm_id.clone()));

        // prepare the delegate + exit channel
        // BEFORE the VM is constructed, so `setDelegate:` can
        // run immediately after init on the same dispatch
        // queue.
        let (exit_tx, exit_rx) = tokio::sync::oneshot::channel::<DelegateExit>();
        let exit_state: SharedExitSender = Arc::new(Mutex::new(Some(exit_tx)));
        let delegate = ElastosVzDelegate::new(vm_id.clone(), exit_state.clone());

        // SAFETY: `initWithConfiguration_queue` documents that
        // the queue is retained for the lifetime of the VM and
        // that all subsequent VM calls must happen on the same
        // queue. We hand it the `vz_config` (which it copies)
        // and the queue's underlying GCD handle.
        let vm = unsafe {
            VZVirtualMachine::initWithConfiguration_queue(
                VZVirtualMachine::alloc(),
                &vz_config,
                queue.as_raw(),
            )
        };
        let vm = Arc::new(SendableVm(vm));

        // Set the delegate on the dispatch queue — Apple
        // requires every VM property mutation to happen on
        // the associated queue.
        let vm_for_setup = vm.clone();
        let delegate_for_setup = SendableDelegate(delegate.clone());
        queue.as_raw().exec_sync(move || {
            // SAFETY: we are on the VM's associated queue
            // inside this closure, satisfying Apple's
            // setDelegate: threading contract.
            unsafe {
                vm_for_setup
                    .0
                    .setDelegate(Some(delegate_for_setup.0.as_protocol()));
            }
        });

        Ok(Self {
            vm,
            queue,
            vm_id,
            forwarder,
            delegate: SendableDelegate(delegate),
            exit_state,
            exit_rx: Mutex::new(Some(exit_rx)),
            stop_timeout,
            save_restore_support_error,
        })
    }

    /// Start the VM. Resolves when Apple's
    /// `startWithCompletionHandler:` invokes its block.
    ///
    /// : returns the typed [`VzError`] surface
    /// instead of a flat string so the supervisor can pattern
    /// match without re-parsing log lines.
    pub(crate) async fn start(&self) -> Result<(), VzError> {
        run_completion_handler_on_queue(
            self.vm.clone(),
            self.queue.clone(),
            &format!("start (vm_id='{}')", self.vm_id),
            |vm, handler| unsafe { vm.startWithCompletionHandler(handler) },
        )
        .await
    }

    /// Pause a running VM. Used by Browser hibernation before
    /// saving machine state.
    pub(crate) async fn pause(&self) -> Result<(), VzError> {
        if !self.can_pause() {
            return Err(VzError::InvalidState {
                description: format!(
                    "vz pause (vm_id='{}'): VM state {:?} cannot be paused",
                    self.vm_id,
                    self.current_state()
                ),
            });
        }
        run_completion_handler_on_queue(
            self.vm.clone(),
            self.queue.clone(),
            &format!("pause (vm_id='{}')", self.vm_id),
            |vm, handler| unsafe { vm.pauseWithCompletionHandler(handler) },
        )
        .await
    }

    /// Resume a paused VM. Used after restoring a saved machine
    /// state, because Apple restores into the Paused state.
    pub(crate) async fn resume(&self) -> Result<(), VzError> {
        if !self.can_resume() {
            return Err(VzError::InvalidState {
                description: format!(
                    "vz resume (vm_id='{}'): VM state {:?} cannot be resumed",
                    self.vm_id,
                    self.current_state()
                ),
            });
        }
        run_completion_handler_on_queue(
            self.vm.clone(),
            self.queue.clone(),
            &format!("resume (vm_id='{}')", self.vm_id),
            |vm, handler| unsafe { vm.resumeWithCompletionHandler(handler) },
        )
        .await
    }

    /// Save a paused VM's machine state to `path`. The caller is
    /// responsible for pausing first and for atomically publishing
    /// the completed file.
    pub(crate) async fn save_machine_state(&self, path: &Path) -> Result<(), VzError> {
        self.ensure_save_restore_supported("save")?;
        let path_text = path.to_string_lossy().to_string();
        run_completion_handler_on_queue(
            self.vm.clone(),
            self.queue.clone(),
            &format!("saveMachineState (vm_id='{}')", self.vm_id),
            move |vm, handler| unsafe {
                let url = NSURL::fileURLWithPath(&NSString::from_str(&path_text));
                vm.saveMachineStateToURL_completionHandler(&url, handler);
            },
        )
        .await
    }

    /// Restore machine state from `path`. Apple restores into
    /// Paused; callers should invoke [`Self::resume`] after a
    /// successful restore.
    pub(crate) async fn restore_machine_state(&self, path: &Path) -> Result<(), VzError> {
        self.ensure_save_restore_supported("restore")?;
        let path_text = path.to_string_lossy().to_string();
        run_completion_handler_on_queue(
            self.vm.clone(),
            self.queue.clone(),
            &format!("restoreMachineState (vm_id='{}')", self.vm_id),
            move |vm, handler| unsafe {
                let url = NSURL::fileURLWithPath(&NSString::from_str(&path_text));
                vm.restoreMachineStateFromURL_completionHandler(&url, handler);
            },
        )
        .await
    }

    /// Whether Apple's save/restore validation accepted this VM
    /// configuration.
    pub(crate) fn supports_save_restore(&self) -> bool {
        self.save_restore_support_error.is_none()
    }

    /// Request the VM to stop. Resolves when Apple's
    /// `stopWithCompletionHandler:` invokes its block.
    ///
    /// The kernel-console forwarder is intentionally *not*
    /// joined here. Apple's `VZVirtualMachine` retains the
    /// `VZVirtualMachineConfiguration` (and therefore the
    /// `NSFileHandle` holding the pipe's write end) for its
    /// entire lifetime — not just while the VM is running. As a
    /// result the host-side read never sees EOF after `stop`;
    /// it only sees EOF when this `VzMachineHandle` itself
    /// drops, at which point `VZVirtualMachine` releases the
    /// config, the `NSFileHandle` deallocs, the write fd closes
    /// and the forwarder exits naturally. Until drop, the
    /// forwarder's `JoinHandle` sits idle in [`Self::forwarder`]
    /// (Tokio detaches it on drop, so no leak).
    pub(crate) async fn stop(&self) -> Result<(), VzError> {
        // wrap the completion-handler future
        // with a `tokio::time::timeout` so a wedged Apple
        // framework call cannot pin the supervisor's
        // `stop_capsule` indefinitely. The supervisor's outer
        // RPC timeout existed before but had no internal
        // counter-pressure; this is the per-VM enforcement
        // point.
        //
        // the inner future now resolves to a
        // typed `VzError` (instead of a flat `String`) so the
        // outer `drive_stop_with_timeout` can either pass
        // through Apple's classified failure or wrap it into
        // `VzError::TimedOut`.
        let op_label = format!("stop (vm_id='{}')", self.vm_id);
        let inner = run_completion_handler_on_queue(
            self.vm.clone(),
            self.queue.clone(),
            &op_label,
            |vm, handler| unsafe { vm.stopWithCompletionHandler(handler) },
        );
        let outcome = drive_stop_with_timeout(inner, self.stop_timeout, &self.vm_id).await;

        // Always signal the shared exit channel, regardless of
        // whether the completion fired or we timed out. The
        // delegate only fires for guest-initiated stops or
        // errors, so without this any concurrent
        // `wait_for_exit` would hang on a host-initiated stop.
        // on timeout we send
        // `ForcedAfterTimeout` (exit 137) so operators see the
        // forced-stop marker; on success we send the
        // historical `HostInitiatedStop` (exit 0).
        let signal = match &outcome {
            Ok(()) => DelegateExit::HostInitiatedStop,
            Err(VzError::TimedOut { .. }) => DelegateExit::ForcedAfterTimeout,
            Err(_) => DelegateExit::HostInitiatedStop,
        };
        if let Some(tx) = self.exit_state.lock().expect("exit_state mutex").take() {
            let _ = tx.send(signal);
        }

        outcome
    }

    /// Dial the guest's vsock listener on `port` and return an
    /// owned host-side fd connected to it.
    ///
    /// Delegates to [`super::vsock::connect_vsock`] which
    /// dispatches `VZVirtioSocketDevice.connectToPort:` on the
    /// VM's associated queue and marshals the completion
    /// handler into a Tokio oneshot. The returned fd is a
    /// `dup` of Apple's connection fd, so the caller owns it
    /// independently of Apple's `VZVirtioSocketConnection`
    /// lifecycle.
    pub(crate) async fn connect_vsock(&self, port: u32) -> Result<std::os::fd::OwnedFd, String> {
        super::vsock::connect_vsock(self.vm.clone(), self.queue.clone(), &self.vm_id, port).await
    }

    /// Wait for a terminal lifecycle observation and return the
    /// typed [`VzExitReason`] classification. The delegate-driven
    /// `oneshot` lets the supervisor populate
    /// `last_exit_reason` telemetry directly without re-parsing
    /// the integer exit code.
    ///
    /// Signalled by either:
    /// - the [`ElastosVzDelegate`] delegate (guest clean stop,
    ///   crash with NSError), or
    /// - [`Self::stop`] after a successful host-initiated stop /
    ///   forced-after-timeout.
    ///
    /// The receiver is consumed on first call; subsequent calls
    /// return a typed error rather than block indefinitely.
    pub(crate) async fn wait_for_exit_classified(&self) -> Result<VzExitReason, String> {
        let rx = self.exit_rx.lock().expect("exit_rx mutex").take();
        let Some(rx) = rx else {
            return Err(format!(
                "vz wait_for_exit (vm_id='{}'): receiver already consumed",
                self.vm_id
            ));
        };
        match rx.await {
            Ok(exit) => Ok(delegate_exit_to_reason(exit)),
            Err(_) => Err(format!(
                "vz wait_for_exit (vm_id='{}'): delegate sender dropped before signalling",
                self.vm_id
            )),
        }
    }

    /// Read the current VM state. Runs on the dispatch queue
    /// because Apple's docs require all VM property reads to be
    /// dispatched through the associated queue.
    pub(crate) fn current_state(&self) -> VmState {
        let vm = self.vm.clone();
        let cell: Arc<Mutex<Option<VZVirtualMachineState>>> = Arc::new(Mutex::new(None));
        let cell_for_closure = cell.clone();

        self.queue.as_raw().exec_sync(move || {
            // SAFETY: we are on the VM's associated dispatch
            // queue inside this closure (`exec_sync` blocks on
            // the queue), so calling `state()` is safe.
            let s = unsafe { vm.0.state() };
            *cell_for_closure.lock().expect("state cell mutex") = Some(s);
        });

        let raw = cell
            .lock()
            .expect("state cell mutex")
            .take()
            .expect("dispatch_sync closure must have populated the cell");
        VmState::from(raw)
    }

    fn can_pause(&self) -> bool {
        self.vm_bool_on_queue(|vm| unsafe { vm.canPause() })
    }

    fn can_resume(&self) -> bool {
        self.vm_bool_on_queue(|vm| unsafe { vm.canResume() })
    }

    fn vm_bool_on_queue<F>(&self, check: F) -> bool
    where
        F: FnOnce(&VZVirtualMachine) -> bool + Send + 'static,
    {
        let vm = self.vm.clone();
        let cell: Arc<Mutex<Option<bool>>> = Arc::new(Mutex::new(None));
        let cell_for_closure = cell.clone();
        self.queue.as_raw().exec_sync(move || {
            *cell_for_closure.lock().expect("bool cell mutex") = Some(check(&vm.0));
        });
        let result = cell
            .lock()
            .expect("bool cell mutex")
            .take()
            .expect("dispatch_sync closure must have populated the cell");
        result
    }

    fn ensure_save_restore_supported(&self, operation: &str) -> Result<(), VzError> {
        match &self.save_restore_support_error {
            None => Ok(()),
            Some(error) => Err(VzError::NotSupported {
                description: format!(
                    "vz {operation} machine state (vm_id='{}'): VM configuration does not support save/restore: {error}",
                    self.vm_id
                ),
            }),
        }
    }
}

/// Generic completion-handler driver: dispatches `issue` onto
/// the queue, captures the resulting `*mut NSError` and resolves
/// a oneshot.
///
/// `block2::RcBlock` wraps a non-`Send` `NonNull<Block<…>>`, so
/// we construct the block **inside** the dispatch closure (where
/// we already hold the queue's thread). The closure itself only
/// needs `Send` for the `tx_slot` + `vm` + `op` captures, all of
/// which are Send by construction.
///
/// : returns a typed [`VzError`] instead of a
/// flat string so callers can pattern-match on the underlying
/// Apple `VZErrorCode`.
async fn run_completion_handler_on_queue<F>(
    vm: Arc<SendableVm>,
    queue: Arc<VzDispatchQueue>,
    op_label: &str,
    issue: F,
) -> Result<(), VzError>
where
    F: FnOnce(&VZVirtualMachine, &block2::DynBlock<dyn Fn(*mut NSError)>) + Send + 'static,
{
    let (tx, rx) = tokio::sync::oneshot::channel::<Result<(), VzError>>();
    let tx_slot = Arc::new(Mutex::new(Some(tx)));

    let vm_for_dispatch = vm.clone();
    queue.as_raw().exec_sync(move || {
        let tx_for_block = tx_slot.clone();

        // Vz retains the block when the issue closure passes it
        // into startWithCompletionHandler / stopWithCompletionHandler.
        // Our local `handler` drops at the end of this closure;
        // Vz's retained copy keeps the underlying block alive
        // until the completion fires.
        let handler = block2::RcBlock::new(move |err: *mut NSError| {
            let result = if err.is_null() {
                Ok(())
            } else {
                // SAFETY: Vz hands us a non-null NSError it
                // owns; we borrow it for the duration of this
                // closure to extract the typed pieces.
                let nserror: &NSError = unsafe { &*err };
                Err(ns_error_to_vz_error(nserror))
            };
            if let Some(sender) = tx_for_block.lock().expect("oneshot mutex").take() {
                let _ = sender.send(result);
            }
        });

        // SAFETY: we are on the VM's associated dispatch queue
        // inside this closure, satisfying Apple's "all VM calls
        // on the same queue" contract.
        issue(&vm_for_dispatch.0, &handler);
    });

    match rx.await {
        Ok(result) => result,
        Err(_) => Err(VzError::Internal {
            description: format!(
                "vz {op_label}: completion handler oneshot dropped before signalling"
            ),
        }),
    }
}

/// Pull the typed pieces out of an Apple `NSError` and classify
/// it through [`VzError::from_ns_error_parts`].
fn ns_error_to_vz_error(err: &NSError) -> VzError {
    let domain = err.domain().to_string();
    // NSInteger is `isize` on every Apple platform we support.
    let code = err.code();
    let description = ns_error_to_string(err);
    VzError::from_ns_error_parts(&domain, code, &description)
}

/// Drive [`VzMachineHandle::stop`]'s inner future under a
/// timeout. Extracted as a free `async fn` so unit tests can
/// exercise the timeout path without needing a real Apple
/// dispatch queue.
///
/// Returns typed [`VzError`]: Apple's completion failures route as
/// the typed Vz variant straight
/// from [`run_completion_handler_on_queue`]; the timeout path
/// constructs a [`VzError::TimedOut`].
async fn drive_stop_with_timeout<F>(inner: F, timeout: Duration, vm_id: &str) -> Result<(), VzError>
where
    F: std::future::Future<Output = Result<(), VzError>>,
{
    match tokio::time::timeout(timeout, inner).await {
        Ok(result) => result,
        Err(_elapsed) => Err(VzError::TimedOut {
            vm_id: vm_id.to_string(),
            budget: timeout,
        }),
    }
}

/// Map the FFI-internal [`DelegateExit`] (which we keep
/// `pub(crate)` for hygiene) to the public [`VzExitReason`] the
/// supervisor surfaces via `last_exit_reason`.
fn delegate_exit_to_reason(exit: DelegateExit) -> VzExitReason {
    match exit {
        DelegateExit::GuestCleanStop => VzExitReason::GuestCleanStop,
        DelegateExit::HostInitiatedStop => VzExitReason::HostInitiatedStop,
        DelegateExit::StoppedWithError(_) => VzExitReason::StoppedWithError,
        DelegateExit::ForcedAfterTimeout => VzExitReason::ForcedAfterTimeout,
    }
}

fn validate_error_message(err: &NSError, vm_id: &str) -> String {
    format_validate_error(&ns_error_to_string(err), vm_id)
}

/// Pure helper extracted from [`validate_error_message`] so unit
/// tests can assert the operator hint contract without having to
/// construct an `NSError`.
fn format_validate_error(raw_apple_message: &str, vm_id: &str) -> String {
    if raw_apple_message.to_lowercase().contains("entitlement") {
        format!(
            "vz validate (vm_id='{vm_id}'): {ENTITLEMENT_HINT}. Apple error: {raw_apple_message}"
        )
    } else {
        format!("vz validate (vm_id='{vm_id}'): {raw_apple_message}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vm_state_translates_from_vz_state_for_every_documented_variant() {
        // Apple's enum (as of macOS 14): 10 variants. We assert
        // each maps to a non-`Unknown` Rust variant so future
        // additions are loud.
        let cases: &[(VZVirtualMachineState, VmState)] = &[
            (VZVirtualMachineState::Stopped, VmState::Stopped),
            (VZVirtualMachineState::Running, VmState::Running),
            (VZVirtualMachineState::Paused, VmState::Paused),
            (VZVirtualMachineState::Error, VmState::Error),
            (VZVirtualMachineState::Starting, VmState::Starting),
            (VZVirtualMachineState::Pausing, VmState::Pausing),
            (VZVirtualMachineState::Resuming, VmState::Resuming),
            (VZVirtualMachineState::Stopping, VmState::Stopping),
            (VZVirtualMachineState::Saving, VmState::Saving),
            (VZVirtualMachineState::Restoring, VmState::Restoring),
        ];
        for (apple, rust) in cases {
            assert_eq!(VmState::from(*apple), *rust, "{apple:?} mapping");
        }
    }

    #[test]
    fn vm_state_unknown_variants_preserve_raw_integer() {
        let weird = VZVirtualMachineState(42);
        assert_eq!(VmState::from(weird), VmState::Unknown(42));
    }

    #[test]
    fn entitlement_hint_constant_points_at_day_4_script_and_docs() {
        assert!(ENTITLEMENT_HINT.contains("com.apple.security.virtualization"));
        assert!(ENTITLEMENT_HINT.contains("scripts/dev/sign-elastos-vz/"));
        assert!(ENTITLEMENT_HINT.contains("docs/MAC.md"));
    }

    /// When `validateWithError` rejects the configuration for
    /// missing entitlements, the wrapped error string must embed
    /// the signing hint so the operator immediately knows the next
    /// step. This pure-string test guards the wiring
    /// regardless of whether the test runner is itself signed.
    #[test]
    fn format_validate_error_embeds_day_4_hint_when_apple_says_entitlement() {
        let apple_msg = "Invalid virtual machine configuration. The process doesn't have \
                         the \"com.apple.security.virtualization\" entitlement.";
        let wrapped = format_validate_error(apple_msg, "vz-lifecycle-test");

        assert!(
            wrapped.contains(ENTITLEMENT_HINT),
            "entitlement-flagged validate error must embed operator hint: {wrapped}"
        );
        assert!(
            wrapped.contains("Apple error:"),
            "wrapped error must preserve Apple's original text: {wrapped}"
        );
        assert!(
            wrapped.contains("vz-lifecycle-test"),
            "wrapped error must include vm_id for log diffability: {wrapped}"
        );
    }

    #[test]
    fn format_validate_error_passes_through_other_failures_unmodified() {
        let apple_msg = "Memory size too small for the platform";
        let wrapped = format_validate_error(apple_msg, "vz-lifecycle-test");

        assert!(
            !wrapped.contains(ENTITLEMENT_HINT),
            "non-entitlement errors must not be decorated with the entitlement hint: {wrapped}"
        );
        assert!(wrapped.contains(apple_msg));
        assert!(wrapped.contains("vz-lifecycle-test"));
    }

    /// `drive_stop_with_timeout` must surface a
    /// typed [`VzError::TimedOut`] when the inner future never
    /// resolves. This proves the wrapper that
    /// [`VzMachineHandle::stop`] uses to guard against a wedged
    /// `stopWithCompletionHandler:` block actually fires, with
    /// an operator-facing description that names the budget and
    /// the vm_id (for log correlation) and points at the runbook.
    ///
    /// The error is typed [`VzError::TimedOut`]; the description
    /// string remains grep-friendly via [`VzError::description`].
    #[tokio::test(start_paused = true)]
    async fn drive_stop_with_timeout_returns_typed_error_when_inner_future_never_resolves() {
        let never_resolving = std::future::pending::<Result<(), VzError>>();
        let budget = Duration::from_millis(100);

        let started = tokio::time::Instant::now();
        let outcome = drive_stop_with_timeout(never_resolving, budget, "vz-stop-timeout-vm").await;
        let elapsed = started.elapsed();

        match outcome {
            Err(VzError::TimedOut { vm_id, budget: b }) => {
                assert_eq!(vm_id, "vz-stop-timeout-vm");
                assert_eq!(b, budget);
                let desc = VzError::TimedOut {
                    vm_id: vm_id.clone(),
                    budget: b,
                }
                .description();
                assert!(
                    desc.contains("vz-stop-timeout-vm"),
                    "timeout description must include vm_id for log correlation: {desc}"
                );
                assert!(
                    desc.contains("100ms") || desc.contains("100 ms"),
                    "timeout description must name the budget: {desc}"
                );
                assert!(
                    desc.contains("docs/MAC.md"),
                    "timeout description must point at the runbook: {desc}"
                );
            }
            other => panic!("expected VzError::TimedOut, got {other:?}"),
        }

        // With `start_paused = true` the Tokio runtime advances
        // virtual time on its own; elapsed wall-clock should be
        // negligible (well under 2 × budget). The test cares
        // about the contract (typed timeout), not the precise
        // tick count.
        assert!(
            elapsed < Duration::from_secs(2),
            "drive_stop_with_timeout must not block beyond ~2×budget; took {elapsed:?}"
        );
    }

    /// when the inner future completes cleanly
    /// the wrapper must surface `Ok(())` (the path Apple's
    /// nominal `stopWithCompletionHandler:` takes).
    #[tokio::test]
    async fn drive_stop_with_timeout_passes_through_ok_when_inner_future_resolves_first() {
        let inner = async { Ok::<(), VzError>(()) };
        let outcome = drive_stop_with_timeout(inner, Duration::from_secs(30), "vm").await;
        assert!(outcome.is_ok(), "successful completion must pass through");
    }

    /// When the inner future resolves with a typed Apple error,
    /// the wrapper must pass it
    /// through (distinct from `TimedOut` — the supervisor uses
    /// the distinction to choose between `HostInitiatedStop` and
    /// `ForcedAfterTimeout` telemetry).
    #[tokio::test]
    async fn drive_stop_with_timeout_classifies_apple_error_distinctly_from_timeout() {
        let apple_err = VzError::from_ns_error_parts(
            "VZErrorDomain",
            3,
            "Invalid virtual machine state for stop",
        );
        let inner = async move { Err::<(), VzError>(apple_err) };
        let outcome = drive_stop_with_timeout(inner, Duration::from_secs(30), "vm").await;
        match outcome {
            Err(VzError::InvalidState { description }) => {
                assert!(description.contains("Invalid virtual machine state"));
            }
            other => panic!("expected VzError::InvalidState, got {other:?}"),
        }
    }

    /// every `DelegateExit` variant the FFI
    /// surfaces must map to a public `VzExitReason`, including
    /// the `StoppedWithError` arm that carries a payload. This
    /// catches a regression where adding a new delegate variant
    /// without updating the classifier would silently route
    /// through the wrong telemetry label.
    #[test]
    fn delegate_exit_to_reason_classifies_every_variant() {
        assert_eq!(
            delegate_exit_to_reason(DelegateExit::GuestCleanStop),
            VzExitReason::GuestCleanStop
        );
        assert_eq!(
            delegate_exit_to_reason(DelegateExit::HostInitiatedStop),
            VzExitReason::HostInitiatedStop
        );
        assert_eq!(
            delegate_exit_to_reason(DelegateExit::StoppedWithError("kernel panic".into())),
            VzExitReason::StoppedWithError
        );
        assert_eq!(
            delegate_exit_to_reason(DelegateExit::ForcedAfterTimeout),
            VzExitReason::ForcedAfterTimeout
        );
    }
}
