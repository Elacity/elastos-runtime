//! `VZVirtualMachineDelegate` implementation for Vz lifecycle
//! observation.
//!
//! Delegate-driven exit observation: when the guest cleanly stops,
//! or when Apple tears the VM down because of an error, the
//! corresponding delegate method fires and we signal the supervisor
//! with a typed exit reason.
//!
//! The delegate is a custom NSObject subclass declared via
//! `objc2::define_class!`. Apple holds a *weak* reference to
//! delegates, so [`VzMachineHandle`][super::lifecycle::VzMachineHandle]
//! keeps the `Retained<ElastosVzDelegate>` alive for the VM's
//! entire lifetime — dropping the handle dealloc's the delegate
//! and any subsequent dispatch from Vz becomes a no-op (the
//! `VZVirtualMachine` releases its weak ref as the VM tears
//! itself down on drop).
//!
//! Threading: every delegate method fires on the VM's
//! associated `VzDispatchQueue` — Apple's contract for
//! `setDelegate:`. The `oneshot::Sender` inside our ivars is a
//! Tokio primitive; sending from inside a GCD queue closure is
//! a synchronous channel push that does not block.

#![cfg(target_os = "macos")]

use std::sync::{Arc, Mutex};

use elastos_logger::{log_info, log_warn};
use objc2::define_class;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{msg_send, AnyThread, DefinedClass};
use objc2_foundation::{NSError, NSObject, NSObjectProtocol};
use objc2_virtualization::{VZVirtualMachine, VZVirtualMachineDelegate};

use super::error::ns_error_to_string;

const LOG_COMPONENT: &str = "vm.vz";

/// Terminal-state classification surfaced by the delegate and
/// by `VzMachineHandle::stop`.
///
/// : the public exit-code + telemetry-label
/// mapping moved to [`crate::error::VzExitReason`]; this enum
/// remains as the FFI-internal representation. [`VzMachineHandle::wait_for_exit_classified`]
/// is the conversion point.
#[derive(Clone, Debug)]
pub(crate) enum DelegateExit {
    /// `guestDidStopVirtualMachine:` — the guest shut itself
    /// down cleanly (`poweroff -h`, `init 0`, etc.). Exit 0.
    GuestCleanStop,
    /// `virtualMachine:didStopWithError:` — Vz tore the VM
    /// down because of an internal error. The string is
    /// Apple's `NSError.localizedDescription`. Exit 1.
    ///
    /// The inner message is logged via `Debug` in
    /// [`ElastosVzDelegate::signal_exit`] before the variant
    /// crosses the channel; downstream consumers map straight
    /// to the typed [`crate::error::VzExitReason`] so the
    /// string is not read again, but we keep it for
    /// diagnostics.
    #[allow(dead_code)]
    StoppedWithError(String),
    /// `VzMachineHandle::stop` succeeded — the supervisor
    /// asked for the VM to stop and Apple confirmed. Exit 0.
    HostInitiatedStop,
    /// `VzMachineHandle::stop` returned with a typed timeout
    /// because Apple's `stopWithCompletionHandler:` block did
    /// not fire within `VzConfig::stop_timeout`. The supervisor
    /// has already orphaned the Vz handle (best-effort
    /// cleanup); waiters on `wait_for_exit_classified` resolve
    /// with `VzExitReason::ForcedAfterTimeout` whose
    /// `exit_code()` is 137 (matches the SIGKILL semantics
    /// Linux uses when its 5 s SIGTERM grace elapses).
    ForcedAfterTimeout,
}

/// Shared exit-signal handle — held by both
/// [`ElastosVzDelegate`]'s ivars and the
/// [`VzMachineHandle`][super::lifecycle::VzMachineHandle]'s
/// `stop` path. The first caller to take the sender wins; all
/// subsequent terminal observations are dropped silently
/// (they're already logged at the dispatch site).
pub(crate) type SharedExitSender = Arc<Mutex<Option<tokio::sync::oneshot::Sender<DelegateExit>>>>;

/// Ivars holding the delegate's runtime state.
pub(crate) struct ElastosVzDelegateIvars {
    /// First-to-fire-wins sender. Shared with the handle so
    /// host-initiated stops can also resolve the channel.
    exit_tx: SharedExitSender,
    /// VM identifier for log diagnostics.
    vm_id: String,
}

define_class!(
    /// Custom `NSObject` subclass that observes Vz VM
    /// terminal states and forwards them to a Tokio oneshot.
    ///
    /// See module-level doc for lifecycle and threading
    /// contract.
    #[unsafe(super(NSObject))]
    #[name = "ElastosVzDelegate"]
    #[ivars = ElastosVzDelegateIvars]
    pub(crate) struct ElastosVzDelegate;

    unsafe impl NSObjectProtocol for ElastosVzDelegate {}

    unsafe impl VZVirtualMachineDelegate for ElastosVzDelegate {
        #[unsafe(method(guestDidStopVirtualMachine:))]
        fn guest_did_stop_virtual_machine(&self, _vm: &VZVirtualMachine) {
            self.signal_exit(DelegateExit::GuestCleanStop);
        }

        #[unsafe(method(virtualMachine:didStopWithError:))]
        fn virtual_machine_did_stop_with_error(&self, _vm: &VZVirtualMachine, error: &NSError) {
            self.signal_exit(DelegateExit::StoppedWithError(ns_error_to_string(error)));
        }

        #[unsafe(method(virtualMachine:networkDevice:attachmentWasDisconnectedWithError:))]
        fn virtual_machine_network_device_attachment_was_disconnected_with_error(
            &self,
            _vm: &VZVirtualMachine,
            _network_device: &objc2_virtualization::VZNetworkDevice,
            error: &NSError,
        ) {
            log_warn!(
                component: LOG_COMPONENT,
                "vz-delegate vm_id={}: network attachment disconnected: {}",
                self.ivars().vm_id,
                ns_error_to_string(error)
            );
            // Non-terminal — do not consume the exit channel.
        }
    }
);

impl ElastosVzDelegate {
    /// Allocate and initialise the delegate with a shared exit
    /// sender. The sender is taken on the first terminal
    /// observation.
    pub(crate) fn new(vm_id: String, exit_tx: SharedExitSender) -> Retained<Self> {
        let this = Self::alloc().set_ivars(ElastosVzDelegateIvars { exit_tx, vm_id });
        unsafe { msg_send![super(this), init] }
    }

    /// Wrap the delegate in a `ProtocolObject` reference so
    /// it can be handed to `VZVirtualMachine::setDelegate:`.
    pub(crate) fn as_protocol(&self) -> &ProtocolObject<dyn VZVirtualMachineDelegate> {
        ProtocolObject::from_ref(self)
    }

    /// First-wins exit signal. Subsequent calls (and host
    /// `stop()`-induced calls) find the sender already taken
    /// and become no-ops at the channel level.
    fn signal_exit(&self, exit: DelegateExit) {
        let ivars = self.ivars();
        log_info!(
            component: LOG_COMPONENT,
            "vz-delegate vm_id={}: delegate observed terminal state: {:?}",
            ivars.vm_id,
            exit
        );
        if let Some(tx) = ivars.exit_tx.lock().expect("delegate exit_tx mutex").take() {
            let _ = tx.send(exit);
        }
    }
}

/// `Retained<ElastosVzDelegate>` wrapped with the same Send +
/// Sync contract `SendableVm` uses. Apple's threading model
/// keeps all delegate touches on the VM's associated dispatch
/// queue; we never deref the inner `Retained` off-queue.
pub(crate) struct SendableDelegate(pub(crate) Retained<ElastosVzDelegate>);

// SAFETY: the inner Retained is only ever touched through
// `VZVirtualMachine::setDelegate:` (queue-dispatched) and
// through `signal_exit` (queue-dispatched by Apple). The
// `Retained` itself is just a refcounted pointer move; pointer
// equality is stable across threads.
unsafe impl Send for SendableDelegate {}
unsafe impl Sync for SendableDelegate {}

#[cfg(test)]
mod tests {
    use super::*;

    // the integer exit-code mapping moved to
    // `VzExitReason::exit_code()` (see `crate::error::tests`).
    // This module's only contract is that the delegate observes
    // every terminal state correctly; the
    // `delegate_signal_exit_*` test below covers that.

    #[tokio::test]
    async fn delegate_signal_exit_sends_first_terminal_observation_only() {
        // Verify the first-wins semantics without involving
        // Apple's queue: drive `signal_exit` directly.
        let (tx, rx) = tokio::sync::oneshot::channel::<DelegateExit>();
        let shared: SharedExitSender = Arc::new(Mutex::new(Some(tx)));
        let delegate = ElastosVzDelegate::new("test-vm".into(), shared.clone());

        delegate.signal_exit(DelegateExit::GuestCleanStop);
        // Second signal must be a no-op at the channel level
        // (the receiver only resolves once).
        delegate.signal_exit(DelegateExit::StoppedWithError("ignored".into()));

        let got = rx.await.expect("first signal must resolve");
        assert!(matches!(got, DelegateExit::GuestCleanStop));
        assert!(
            shared.lock().unwrap().is_none(),
            "sender must be consumed after first signal"
        );
    }
}
