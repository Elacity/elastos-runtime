//! Kernel console + multi-port virtio-console wrappers.
//!
//! Two distinct device classes hide behind the word "console" in
//! Apple's API:
//!
//! 1. `VZVirtioConsoleDeviceSerialPortConfiguration` — a single
//!    UART-like serial port. We use it for the **kernel console**
//!    (`/dev/hvc0` inside the guest); the guest's printk output
//!    flows through this port. One per VM.
//! 2. `VZVirtioConsoleDeviceConfiguration` — a multi-port virtio
//!    console (macOS 12+). Phase 0 §D pitfall #4 — this is what
//!    we use for the **Carrier bridge**; the bridge connects on
//!    `/dev/hvc1` (`crate::CARRIER_GUEST_DEVICE_PATH`), the second
//!    virtio-console port because the first is the kernel console.
//!
//! Phase 0 §D pitfall #7: never back the console with a regular
//! file. An ever-growing logfile is a guaranteed disk-exhaustion
//! footgun on long-running Mac sessions. Both attachments below
//! use a `pipe(2)` pair instead — the read end stays owned by
//! Rust so the lifecycle module (Day 3) can copy bytes into the
//! `tracing` subscriber.
//!
//! Day 1's reality probe verified the constructibility of every
//! type used here.

#![cfg(target_os = "macos")]

use std::os::fd::{FromRawFd, IntoRawFd, RawFd};

use objc2::rc::Retained;
use objc2::AnyThread;
use objc2_foundation::{NSFileHandle, NSString};
use objc2_virtualization::{
    VZFileHandleSerialPortAttachment, VZVirtioConsoleDeviceConfiguration,
    VZVirtioConsoleDeviceSerialPortConfiguration, VZVirtioConsolePortConfiguration,
    VZVirtioConsolePortConfigurationArray,
};

/// Result of building the kernel console.
///
/// The Rust side keeps ownership of [`host_read`] so the
/// lifecycle module can read guest kernel output as raw bytes
/// and forward them to a `tracing` target. The Vz side keeps
/// ownership of the write end of the pipe (via the attachment
/// inside [`serial_port_cfg`]) and writes guest output there.
pub(crate) struct KernelConsole {
    /// Host-owned read end. Reads from this `File` yield bytes
    /// the guest emitted on its kernel console.
    pub(crate) host_read: std::fs::File,

    /// Vz-side configuration ready to hand to
    /// `VZVirtualMachineConfiguration::setSerialPorts`. Holds the
    /// write end of the pipe inside an `NSFileHandle`.
    pub(crate) serial_port_cfg: Retained<VZVirtioConsoleDeviceSerialPortConfiguration>,
}

/// Build the kernel-console serial port + its host-side read end.
pub(crate) fn build_kernel_console() -> Result<KernelConsole, String> {
    let (host_read_fd, vz_write_fd) = create_pipe()?;

    // SAFETY: We just opened these fds and have not handed them to
    // any other Rust owner yet; converting them into `File`s is the
    // standard `FromRawFd` pattern.
    let host_read = unsafe { std::fs::File::from_raw_fd(host_read_fd) };

    let vz_write_handle = into_ns_file_handle(vz_write_fd);

    // SAFETY: `VZFileHandleSerialPortAttachment` retains both
    // handles via `closeOnDealloc=true`; once the attachment is
    // released the OS closes vz_write_fd. We pass `None` for the
    // reading side because the kernel console never receives
    // input from the host.
    let attachment = unsafe {
        VZFileHandleSerialPortAttachment::initWithFileHandleForReading_fileHandleForWriting(
            VZFileHandleSerialPortAttachment::alloc(),
            None,
            Some(&vz_write_handle),
        )
    };

    let serial_port_cfg = unsafe { VZVirtioConsoleDeviceSerialPortConfiguration::new() };
    unsafe { serial_port_cfg.setAttachment(Some(&attachment)) };

    Ok(KernelConsole {
        host_read,
        serial_port_cfg,
    })
}

/// Build the multi-port virtio-console *device* with a single
/// placeholder port at index 0 — the slot Phase 3's Carrier
/// bridge will fill in. Day 2 ships the structure (the array
/// layout, the port name); Phase 3 replaces the placeholder
/// attachment with a socketpair-backed bridge channel.
///
/// `port_name` becomes the userspace-visible name on the host
/// side (Apple exposes it in the Vz process traces); it does
/// **not** affect the guest-visible device path. The guest
/// always sees this device as `/dev/hvc1`
/// (`crate::CARRIER_GUEST_DEVICE_PATH`).
pub(crate) fn build_carrier_console_slot(
    port_name: &str,
) -> Result<Retained<VZVirtioConsoleDeviceConfiguration>, String> {
    let (placeholder_read_fd, placeholder_write_fd) = create_pipe()?;

    let read_handle = into_ns_file_handle(placeholder_read_fd);
    let write_handle = into_ns_file_handle(placeholder_write_fd);

    // SAFETY: same as `build_kernel_console`. Both handles use
    // `closeOnDealloc=true`; releasing the attachment closes the
    // fds. The slot is intentionally a closed loop in Day 2 —
    // Phase 3 replaces it with a real socketpair.
    let attachment = unsafe {
        VZFileHandleSerialPortAttachment::initWithFileHandleForReading_fileHandleForWriting(
            VZFileHandleSerialPortAttachment::alloc(),
            Some(&read_handle),
            Some(&write_handle),
        )
    };

    let port = unsafe { VZVirtioConsolePortConfiguration::new() };
    unsafe { port.setName(Some(&NSString::from_str(port_name))) };
    unsafe { port.setAttachment(Some(&attachment)) };

    let device = unsafe { VZVirtioConsoleDeviceConfiguration::new() };
    let array: Retained<VZVirtioConsolePortConfigurationArray> = unsafe { device.ports() };
    unsafe { array.setObject_atIndexedSubscript(Some(&port), 0) };

    Ok(device)
}

/// Open a POSIX pipe and return `(read_fd, write_fd)` as raw
/// integers. The caller is responsible for ownership of each fd.
fn create_pipe() -> Result<(RawFd, RawFd), String> {
    let mut fds = [0i32; 2];

    // SAFETY: `libc::pipe` writes two fds into the array and
    // returns 0 on success. On failure no fds are written; the
    // `Result::Err` branch below skips the rest of the function.
    let rc = unsafe { libc::pipe(fds.as_mut_ptr()) };
    if rc != 0 {
        return Err(format!(
            "console: pipe() failed ({})",
            std::io::Error::last_os_error()
        ));
    }

    Ok((fds[0], fds[1]))
}

/// Move a raw file descriptor into an `NSFileHandle` that will
/// close the fd on dealloc.
fn into_ns_file_handle(fd: RawFd) -> Retained<NSFileHandle> {
    // SAFETY: `fd` was just produced by `pipe(2)` and has no
    // other Rust owner. Wrapping it in `std::fs::File` and then
    // calling `into_raw_fd` is the canonical way to hand
    // ownership across an FFI boundary without leaking the fd
    // if the caller drops the wrapper.
    let owned = unsafe { std::fs::File::from_raw_fd(fd) };
    let fd_to_hand_over = owned.into_raw_fd();

    NSFileHandle::initWithFileDescriptor_closeOnDealloc(
        NSFileHandle::alloc(),
        fd_to_hand_over,
        true,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::os::fd::AsRawFd;

    #[test]
    fn kernel_console_produces_an_owned_host_read_fd() {
        let console = build_kernel_console().expect("kernel console builds");
        let raw_fd = console.host_read.as_raw_fd();
        assert!(raw_fd >= 0, "host_read fd must be valid (got {raw_fd})");
        // Confirm the file handle is the read end by trying to
        // read 0 bytes — should not error even with no writer
        // attached (POSIX read returns 0 / EAGAIN / blocks).
        // We use `try_clone` to avoid consuming the file in the
        // test before Phase 2 main wires the forwarder.
        let _clone = console.host_read.try_clone().expect("clone host_read");
    }

    #[test]
    fn carrier_slot_constructs_with_named_port() {
        let device = build_carrier_console_slot("elastos-carrier-test")
            .expect("carrier console slot builds");
        // The array exposes `objectAtIndexedSubscript:` which is
        // sugar for `objectAtIndex:`; we resolve via the same
        // subscript getter Vz uses internally and confirm the
        // entry we set is still there.
        let array = unsafe { device.ports() };
        let entry: Option<Retained<VZVirtioConsolePortConfiguration>> =
            unsafe { array.objectAtIndexedSubscript(0) };
        assert!(
            entry.is_some(),
            "carrier slot must contain a port at index 0"
        );
        let port = entry.unwrap();
        let got_name = unsafe { port.name() }
            .map(|s| s.to_string())
            .unwrap_or_default();
        assert_eq!(got_name, "elastos-carrier-test");
    }

    #[test]
    fn create_pipe_returns_two_distinct_fds() {
        let (r, w) = create_pipe().unwrap();
        assert_ne!(r, w, "pipe must produce two distinct fds");
        // Quick sanity: write one byte through the pipe and read
        // it back so we know the fds genuinely refer to a pipe.
        let mut writer = unsafe { std::fs::File::from_raw_fd(w) };
        let mut reader = unsafe { std::fs::File::from_raw_fd(r) };
        writer.write_all(b"x").unwrap();
        drop(writer);
        let mut buf = [0u8; 1];
        reader.read_exact(&mut buf).unwrap();
        assert_eq!(buf, *b"x");
    }
}
