//! Running-VM lifecycle: validate → init → start → stop.
//!
//! Bridges Apple's GCD-backed `VZVirtualMachine` to Tokio
//! async/await. Day 3 covers what we need for first boot:
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
//! What Day 3 deliberately **does not** include:
//!
//! - `VZVirtualMachineDelegate` (used for observing intermediate
//!   states / network-disconnect / guest-initiated stop). Polling
//!   `state` is enough for start/stop/status; the delegate lands
//!   in Day 5+ when we surface crash details.
//! - Code-signing / entitlements provisioning. Without the
//!   `com.apple.security.virtualization` entitlement Apple's
//!   `validateWithError` fails; we surface that with a single,
//!   operator-friendly error string that points at the Day 4
//!   codesign script.
//!
//! Anchors:
//! - Phase 0 §B "lifecycle / control" row
//! - Phase 0 §D pitfall #9 (delegate threading) — addressed by
//!   funneling every Vz call through a single serial dispatch
//!   queue (one per `VzProvider`).
//! - Phase 0 §D pitfall #10 (GCD queue) — `VzDispatchQueue` is
//!   created once per provider in `ffi::dispatch`.

use std::sync::{Arc, Mutex};

use objc2::rc::Retained;
use objc2::AnyThread;
use objc2_foundation::NSError;
use objc2_virtualization::{VZVirtualMachine, VZVirtualMachineState};

use super::builder::BuiltMachine;
use super::console_forwarder::{spawn_console_forwarder, ConsoleForwarder};
use super::dispatch::VzDispatchQueue;
use super::error::ns_error_to_string;

/// Operator-facing hint appended to the error returned by
/// [`VzMachineHandle::new`] when Apple's `validateWithError`
/// rejects the configuration for missing entitlements. Day 4
/// ships `scripts/dev/sign-elastos-vz/` which fixes this; the
/// hint string is the single source of truth so docs, errors and
/// tests can agree.
pub(crate) const ENTITLEMENT_HINT: &str =
    "missing com.apple.security.virtualization entitlement — sign the binary with \
     scripts/dev/sign-elastos-vz/ (Phase 2 Day 4) or see docs/MAC.md";

/// Mirrors `VZVirtualMachineState` with a Rust-idiomatic enum so
/// the supervisor and the (Day 5+) status reporter don't depend
/// on raw `NSInteger` values.
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
struct SendableVm(Retained<VZVirtualMachine>);

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
    #[allow(dead_code)]
    forwarder: ConsoleForwarder,
    //
    // NOTE on the missing `carrier_console` field: Day 2's
    // `BuiltMachine` carries a `Retained<VZVirtioConsoleDeviceConfiguration>`
    // so Phase 3's Carrier bridge can patch the slot in-place.
    // For Day 3 we don't need that handle (the assembled
    // `VZVirtualMachineConfiguration` already retains it via
    // `setConsoleDevices`), and `Retained<NSObject>` is not
    // unconditionally `Send`. Reintroducing it would require a
    // `SendableConsole` newtype mirroring `SendableVm`. Phase 3
    // adds both together.
}

impl VzMachineHandle {
    /// Validate the assembled machine, instantiate the
    /// `VZVirtualMachine` on the provided queue and spawn the
    /// kernel-console forwarder.
    pub(crate) fn new(
        built: BuiltMachine,
        queue: Arc<VzDispatchQueue>,
        vm_id: String,
    ) -> Result<Self, String> {
        let BuiltMachine {
            vz_config,
            kernel_console_host_read,
            carrier_console: _carrier_console,
            identifier_path: _,
        } = built;

        // SAFETY: validateWithError runs entirely on the calling
        // thread; the documentation does not require a dispatch
        // queue for validation.
        if let Err(err) = unsafe { vz_config.validateWithError() } {
            return Err(validate_error_message(&err, &vm_id));
        }

        // Spawn the console forwarder before the VM starts —
        // any stray bytes the kernel emits during early boot are
        // captured. The forwarder is idle until the guest writes
        // its first byte.
        let forwarder = spawn_console_forwarder(kernel_console_host_read, vm_id.clone());

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

        Ok(Self {
            vm: Arc::new(SendableVm(vm)),
            queue,
            vm_id,
            forwarder,
        })
    }

    /// Start the VM. Resolves when Apple's
    /// `startWithCompletionHandler:` invokes its block.
    pub(crate) async fn start(&self) -> Result<(), String> {
        run_completion_handler_on_queue(
            self.vm.clone(),
            self.queue.clone(),
            &format!("start (vm_id='{}')", self.vm_id),
            |vm, handler| unsafe { vm.startWithCompletionHandler(handler) },
        )
        .await
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
    pub(crate) async fn stop(&self) -> Result<(), String> {
        run_completion_handler_on_queue(
            self.vm.clone(),
            self.queue.clone(),
            &format!("stop (vm_id='{}')", self.vm_id),
            |vm, handler| unsafe { vm.stopWithCompletionHandler(handler) },
        )
        .await
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
async fn run_completion_handler_on_queue<F>(
    vm: Arc<SendableVm>,
    queue: Arc<VzDispatchQueue>,
    op_label: &str,
    issue: F,
) -> Result<(), String>
where
    F: FnOnce(&VZVirtualMachine, &block2::DynBlock<dyn Fn(*mut NSError)>) + Send + 'static,
{
    let (tx, rx) = tokio::sync::oneshot::channel::<Result<(), String>>();
    let tx_slot = Arc::new(Mutex::new(Some(tx)));
    let op = op_label.to_string();

    let vm_for_dispatch = vm.clone();
    queue.as_raw().exec_sync(move || {
        let tx_for_block = tx_slot.clone();
        let op_for_block = op.clone();

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
                // closure to extract the description.
                let nserror: &NSError = unsafe { &*err };
                Err(format!(
                    "vz {op_for_block}: {}",
                    ns_error_to_string(nserror)
                ))
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
        Err(_) => Err(format!(
            "vz {op_label}: completion handler oneshot dropped before signalling"
        )),
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

    /// The most operationally-important Day 3 contract: when
    /// `validateWithError` rejects the configuration for missing
    /// entitlements, the wrapped error string MUST embed the
    /// Day-4 script hint so the operator immediately knows the
    /// next step. This pure-string test guards the wiring
    /// regardless of whether the test runner is itself signed.
    #[test]
    fn format_validate_error_embeds_day_4_hint_when_apple_says_entitlement() {
        let apple_msg = "Invalid virtual machine configuration. The process doesn't have \
                         the \"com.apple.security.virtualization\" entitlement.";
        let wrapped = format_validate_error(apple_msg, "phase2-day3-test");

        assert!(
            wrapped.contains(ENTITLEMENT_HINT),
            "entitlement-flagged validate error must embed operator hint: {wrapped}"
        );
        assert!(
            wrapped.contains("Apple error:"),
            "wrapped error must preserve Apple's original text: {wrapped}"
        );
        assert!(
            wrapped.contains("phase2-day3-test"),
            "wrapped error must include vm_id for log diffability: {wrapped}"
        );
    }

    #[test]
    fn format_validate_error_passes_through_other_failures_unmodified() {
        let apple_msg = "Memory size too small for the platform";
        let wrapped = format_validate_error(apple_msg, "phase2-day3-test");

        assert!(
            !wrapped.contains(ENTITLEMENT_HINT),
            "non-entitlement errors must not be decorated with the entitlement hint: {wrapped}"
        );
        assert!(wrapped.contains(apple_msg));
        assert!(wrapped.contains("phase2-day3-test"));
    }
}
