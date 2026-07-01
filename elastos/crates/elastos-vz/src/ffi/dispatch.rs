//! Per-provider serial GCD dispatch queue.
//!
//! Apple's `VZVirtualMachine` requires every call (init, start,
//! stop, delegate callback) to be issued on the **same** GCD queue
//! — that is the second positional argument to
//! `VZVirtualMachine::initWithConfiguration:queue:`. Mixing
//! threads triggers `NSInternalInconsistencyException` at runtime
//! and Apple's docs are explicit about it
//! ([Threading the Virtualization framework][threading]).
//!
//! The backend uses one serial GCD queue per VM so each
//! `VZVirtualMachine` is accessed on its associated queue without
//! serializing unrelated VMs through a provider-wide queue.
//!
//! This is a thin owned wrapper so the main VM path has one type to
//! pass into the `VZVirtualMachine` init site. The
//! wrapper exists only to (a) name the queue for `Instruments.app`
//! traces and (b) keep the queue alive for the lifetime of the
//! provider; everything else is plain `dispatch2::DispatchQueue`
//! reuse.
//!
//! [threading]: https://developer.apple.com/documentation/virtualization/threading_considerations

#![cfg(target_os = "macos")]

use dispatch2::DispatchRetained;

/// Owned serial dispatch queue. One per `VzProvider`.
pub(crate) struct VzDispatchQueue {
    inner: DispatchRetained<dispatch2::DispatchQueue>,
}

impl VzDispatchQueue {
    /// Build a fresh serial queue. The `label` is what
    /// `Instruments.app` and `lldb` will see and should include the
    /// provider name plus a session-unique suffix so concurrently
    /// running VzProviders are distinguishable in traces.
    pub(crate) fn new(label: &str) -> Self {
        // dispatch2 0.3 accepts `&str` for the label; passing `None`
        // for the attribute yields a serial queue, which is what
        // VZVirtualMachine requires.
        Self {
            inner: dispatch2::DispatchQueue::new(label, None),
        }
    }

    /// Borrow the raw `DispatchQueue`. the main VM path will hand this
    /// to `VZVirtualMachine::initWithConfiguration:queue:` in the
    /// lifecycle module.
    ///
    /// `DispatchRetained<T>` implements `Deref<Target = T>`, so the
    /// explicit `&*` strips one level of retained wrapper and
    /// hands callers the bare queue reference Apple's API expects.
    #[allow(dead_code)]
    pub(crate) fn as_raw(&self) -> &dispatch2::DispatchQueue {
        &self.inner
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dispatch_queue_constructs_without_panic() {
        let _q = VzDispatchQueue::new("elastos-vz-test.queue");
        // No public API on dispatch2::DispatchQueue exposes the
        // label back; the contract here is "it constructs cleanly".
        // The probe binary verified the same call path on the host.
    }

    #[test]
    fn two_queues_are_independent() {
        let _a = VzDispatchQueue::new("elastos-vz-test.a");
        let _b = VzDispatchQueue::new("elastos-vz-test.b");
        // Just confirm we can hold multiple queues alive
        // simultaneously without dispatch2 enforcing any global
        // singleton constraint.
    }
}
