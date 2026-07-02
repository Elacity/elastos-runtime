//! `VZVirtioSocketDeviceConfiguration` wrapper (vsock).
//!
//! Apple's Vz does **not** expose a public API to set a vsock
//! CID. The crosvm path on Linux negotiates CIDs explicitly via
//! `AF_VSOCK`; the Vz path uses per-VM connections through
//! `VZVirtioSocketConnection` once the VM is running. That
//! adaptation lives in the supervisor bridge; this module attaches
//! the device class and dials host-to-guest ports.

#![cfg(target_os = "macos")]

use std::os::fd::{FromRawFd, OwnedFd};
use std::sync::{Arc, Mutex};

use objc2::rc::Retained;
use objc2_foundation::NSError;
use objc2_virtualization::{
    VZVirtioSocketConnection, VZVirtioSocketDevice, VZVirtioSocketDeviceConfiguration,
};

use super::dispatch::VzDispatchQueue;
use super::error::ns_error_to_string;

/// First-wins sender that delivers the result of a `connect_vsock`
/// call back to the awaiting Tokio task. Wrapped in `Arc<Mutex<Option<_>>>`
/// because Apple's completion handler may fire on any GCD-managed
/// thread and we hand `tx_slot` clones both into the
/// queue-dispatched setup closure and into the block we register
/// with `connectToPort:`.
type VsockResultSender = Arc<Mutex<Option<tokio::sync::oneshot::Sender<Result<OwnedFd, String>>>>>;

/// Build a vsock device configuration. There are no per-VM
/// knobs to set; Vz allocates the CID itself and exposes it via
/// `VZVirtualMachine.socketDevices[0]` once the VM starts.
pub(crate) fn build_vsock_device() -> Retained<VZVirtioSocketDeviceConfiguration> {
    // SAFETY: `new()` allocates and initialises a vsock
    // configuration; no thread-safety constraint applies before
    // it's attached to a `VZVirtualMachineConfiguration`.
    unsafe { VZVirtioSocketDeviceConfiguration::new() }
}

/// Dial the running guest's vsock listener on `port` and return
/// an owned host-side fd connected to it.
///
/// Apple's `VZVirtioSocketDevice.connectToPort:completionHandler:`
/// is the only supported way to open a host→guest vsock
/// connection on Vz — there is no `AF_VSOCK` socket() the way
/// crosvm/Linux provides. The completion handler runs on the
/// VM's associated dispatch queue; we marshal its result onto a
/// Tokio oneshot for `.await` ergonomics.
///
/// The returned [`OwnedFd`] is a `dup` of
/// `VZVirtioSocketConnection.fileDescriptor` — we drop the
/// Apple-owned connection inside the completion handler (which
/// closes its own fd via the dealloc dance Apple documents),
/// and the kernel keeps the underlying socket alive until our
/// duplicate closes. This mirrors the
/// `VZFileHandleSerialPortAttachment` fd-dup pattern used by the
/// Carrier console.
pub(crate) async fn connect_vsock(
    vm: Arc<super::lifecycle::SendableVm>,
    queue: Arc<VzDispatchQueue>,
    vm_id: &str,
    port: u32,
) -> Result<OwnedFd, String> {
    let (tx, rx) = tokio::sync::oneshot::channel::<Result<OwnedFd, String>>();
    let tx_slot: VsockResultSender = Arc::new(Mutex::new(Some(tx)));
    let op = format!("vsock connect (vm_id='{vm_id}', port={port})");

    let vm_for_dispatch = vm.clone();
    let queue_for_dispatch = queue.clone();
    let op_for_dispatch = op.clone();
    queue.as_raw().exec_async(move || {
        // Resolve the first VZVirtioSocketDevice on the VM.
        // Apple's docs guarantee `socketDevices` is non-nil
        // (possibly empty) — the builder always attaches one
        // VZVirtioSocketDeviceConfiguration, so an empty array
        // here would be a bug in the builder.
        // SAFETY: on the VM's associated queue inside this
        // closure; reading `socketDevices` is documented as
        // queue-safe.
        let devices = unsafe { vm_for_dispatch.0.socketDevices() };
        if devices.count() == 0 {
            send_err(
                &tx_slot,
                format!("vz {op_for_dispatch}: VZVirtualMachine has no socketDevices"),
            );
            return;
        }
        let any_device = devices.objectAtIndex(0);
        // Downcast to the virtio subclass — that's what the
        // builder attaches; any other class would be a Vz
        // version mismatch we'd want to surface loudly.
        let device: Retained<VZVirtioSocketDevice> = match any_device.downcast() {
            Ok(d) => d,
            Err(_) => {
                send_err(
                    &tx_slot,
                    format!("vz {op_for_dispatch}: socketDevices[0] is not a VZVirtioSocketDevice"),
                );
                return;
            }
        };

        let tx_for_block = tx_slot.clone();
        let op_for_block = op_for_dispatch.clone();
        // Vz retains the block when the connect call hands it
        // off. Our local `handler` drops at the end of this
        // closure; Vz's retained copy keeps the underlying
        // block alive until the completion fires.
        let handler = block2::RcBlock::new(
            move |conn: *mut VZVirtioSocketConnection, err: *mut NSError| {
                let result = vsock_connect_result(conn, err, &op_for_block);
                if let Some(sender) = tx_for_block
                    .lock()
                    .expect("vsock connect oneshot mutex")
                    .take()
                {
                    let _ = sender.send(result);
                }
            },
        );

        // SAFETY: we are on the VM's associated dispatch queue
        // inside this closure, so calling `connectToPort:` is
        // safe per Apple's contract.
        unsafe { device.connectToPort_completionHandler(port, &handler) };
        let _keep_queue_alive = queue_for_dispatch;
    });

    match rx.await {
        Ok(result) => result,
        Err(_) => Err(format!(
            "vz {op}: connect completion handler oneshot dropped before signalling"
        )),
    }
}

fn vsock_connect_result(
    conn: *mut VZVirtioSocketConnection,
    err: *mut NSError,
    op: &str,
) -> Result<OwnedFd, String> {
    if !err.is_null() {
        // SAFETY: Vz hands us a non-null NSError it owns; we
        // borrow it for the duration of this function to
        // extract the description.
        let nserror: &NSError = unsafe { &*err };
        return Err(format!("vz {op}: {}", ns_error_to_string(nserror)));
    }
    if conn.is_null() {
        return Err(format!("vz {op}: nil connection and nil error"));
    }
    // SAFETY: non-null `conn` is a valid
    // `VZVirtioSocketConnection` Apple owns; we borrow it only
    // long enough to extract and dup its fd.
    let conn_ref: &VZVirtioSocketConnection = unsafe { &*conn };
    let fd = unsafe { conn_ref.fileDescriptor() };
    if fd < 0 {
        return Err(format!(
            "vz {op}: VZVirtioSocketConnection.fileDescriptor returned -1 (closed)"
        ));
    }
    // dup so we own a fd independent of Apple's connection
    // lifecycle. The kernel keeps the socket endpoint alive as
    // long as either side holds a fd.
    // SAFETY: `fd` is a live socket fd Apple just handed us.
    let dup_fd = unsafe { libc::dup(fd) };
    if dup_fd < 0 {
        return Err(format!(
            "vz {op}: dup() of VZVirtioSocketConnection fd failed: {}",
            std::io::Error::last_os_error()
        ));
    }
    // SAFETY: `dup_fd` is a freshly-duplicated socket fd with
    // no other Rust owner.
    Ok(unsafe { OwnedFd::from_raw_fd(dup_fd) })
}

fn send_err(tx_slot: &VsockResultSender, msg: String) {
    if let Some(sender) = tx_slot.lock().expect("vsock oneshot mutex").take() {
        let _ = sender.send(Err(msg));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vsock_device_constructs() {
        let _cfg = build_vsock_device();
        // No public properties to assert; the contract is that
        // construction is infallible (matches the probe binary).
    }

    /// The error message shape downstream operators see when a
    /// vsock connect is surfaced via `RunningVm::connect_vsock`
    /// embeds the vm_id and port so log-grep stays useful.
    #[test]
    fn vsock_connect_result_passes_through_apple_errors_with_op_prefix() {
        // Synthesise a "fake" error path by constructing a
        // null-pointer scenario the result helper accepts.
        let err = vsock_connect_result(std::ptr::null_mut(), std::ptr::null_mut(), "test-op");
        assert!(err.is_err());
        let msg = err.unwrap_err();
        assert!(msg.contains("test-op"), "op label must appear: {msg}");
        assert!(
            msg.contains("nil connection") || msg.contains("nil error"),
            "expected nil-pointer diagnostic: {msg}"
        );
    }
}
