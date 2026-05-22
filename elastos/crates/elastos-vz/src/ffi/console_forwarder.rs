//! Kernel-console → `tracing` forwarder.
//!
//! Phase 0 §B "kernel console" row + §D pitfall #7 (no
//! file-backed console). Apple's `VZVirtioConsoleDeviceSerialPortConfiguration`
//! emits the guest's printk stream to whichever NSFileHandle we
//! attached. [`ffi::console::build_kernel_console`] hands us the
//! read end of a `pipe(2)`; this module drains that pipe and
//! emits one `tracing::info!(target = "vm_console", ...)` per
//! line of guest output, matching the contract crosvm uses on
//! Linux so the supervisor's existing log routing keeps working.
//!
//! Sizing rationale:
//!
//! - Day 2's probe verified the pipe is the right transport
//!   (Vz writes guest serial output to our pipe).
//! - The forwarder is intentionally minimal: a single blocking
//!   reader spawned via `tokio::task::spawn_blocking`. Per-line
//!   parsing keeps the supervisor's existing `vm_console`
//!   filtering rules portable.
//! - Shutdown happens **naturally**: when the VM stops, Apple
//!   releases the `NSFileHandle` holding the pipe's write end,
//!   our read end sees EOF, the loop exits. The
//!   [`ConsoleForwarder::shutdown_and_join`] timeout exists only
//!   to bound the wait if Vz never closes the pipe (stuck VM,
//!   etc.).

use std::io::{BufRead, BufReader};
use std::time::Duration;

use tokio::task::JoinHandle;

/// Handle returned by [`spawn_console_forwarder`]. Hold it for
/// the lifetime of the VM. Tokio detaches the `JoinHandle` on
/// drop, so the forwarder keeps running until the kernel
/// console reaches EOF (which happens when the
/// `VZVirtualMachine` releases the `NSFileHandle` — i.e. when
/// `VzMachineHandle` itself drops).
///
/// Tests can call [`Self::shutdown_and_join`] to bound the wait
/// for the task to finish in a deterministic way; production
/// code never needs to — see the long-form note in
/// [`crate::ffi::lifecycle::VzMachineHandle::stop`].
pub(crate) struct ConsoleForwarder {
    // `dead_code` because the *value* `handle` is never read
    // outside `shutdown_and_join` (which itself is test-only as
    // of Day 3). The field still has to live on the struct so
    // tests can move it out via `self.handle`. Phase 5+ may
    // resurrect it for runtime-side abort once we add a
    // `VZVirtualMachineDelegate`.
    #[allow(dead_code)]
    handle: JoinHandle<()>,
}

impl ConsoleForwarder {
    /// Wait for the forwarder to finish naturally (EOF on the
    /// pipe), with a hard upper bound. If the timeout elapses
    /// the task is aborted; any unread bytes are lost.
    ///
    /// Production callers do **not** use this method — Apple's
    /// VZVirtualMachine holds the write fd open across `stop`,
    /// so a forced join would always time out. The method
    /// exists for tests that explicitly close their own writer.
    #[allow(dead_code)]
    pub(crate) async fn shutdown_and_join(self, timeout: Duration) -> Result<(), String> {
        match tokio::time::timeout(timeout, self.handle).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(join_err)) => {
                // JoinError covers panics + cancellation. Either
                // is unusual enough to surface.
                Err(format!(
                    "console forwarder task did not finish cleanly: {join_err}"
                ))
            }
            Err(_) => Err(format!(
                "console forwarder did not finish within {} ms (guest may be hung)",
                timeout.as_millis()
            )),
        }
    }
}

/// Spawn a forwarder that drains `host_read` to `tracing`.
///
/// `vm_id` is attached to every emitted event under the
/// `vm_console.vm_id` field so multiple VMs can share a single
/// tracing subscriber without log-line confusion.
pub(crate) fn spawn_console_forwarder(host_read: std::fs::File, vm_id: String) -> ConsoleForwarder {
    let handle = tokio::task::spawn_blocking(move || {
        let mut reader = BufReader::new(host_read);
        let mut buf = String::new();

        loop {
            buf.clear();
            match reader.read_line(&mut buf) {
                Ok(0) => {
                    // EOF — Vz closed the write end. Normal
                    // shutdown path.
                    tracing::debug!(
                        target: "vm_console",
                        vm_id = %vm_id,
                        "kernel console EOF (vz closed write end)"
                    );
                    return;
                }
                Ok(_) => {
                    // Trim the trailing newline (if any) so the
                    // tracing event reads cleanly in JSON
                    // formatters.
                    let line = buf.trim_end_matches(['\n', '\r']);
                    if line.is_empty() {
                        continue;
                    }
                    tracing::info!(
                        target: "vm_console",
                        vm_id = %vm_id,
                        "{line}"
                    );
                }
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::Interrupted | std::io::ErrorKind::WouldBlock
                    ) =>
                {
                    // Spurious wakeups — keep reading. The pipe
                    // is blocking by default so WouldBlock is
                    // unlikely, but handle it for resilience if
                    // future code sets O_NONBLOCK.
                    continue;
                }
                Err(e) => {
                    tracing::warn!(
                        target: "vm_console",
                        vm_id = %vm_id,
                        "kernel console read error: {e}"
                    );
                    return;
                }
            }
        }
    });

    ConsoleForwarder { handle }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::os::fd::FromRawFd;

    /// Build a `pipe(2)` pair and return `(read_file, write_file)`.
    fn pipe_pair() -> (std::fs::File, std::fs::File) {
        let mut fds = [0i32; 2];
        // SAFETY: standard POSIX pipe call; on success two valid
        // fds are written.
        let rc = unsafe { libc::pipe(fds.as_mut_ptr()) };
        assert_eq!(rc, 0, "pipe(2) failed");
        // SAFETY: fds were just produced by pipe(2) and have no
        // other Rust owner.
        let r = unsafe { std::fs::File::from_raw_fd(fds[0]) };
        let w = unsafe { std::fs::File::from_raw_fd(fds[1]) };
        (r, w)
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn forwarder_exits_on_eof_and_joins_clean() {
        let (read, mut write) = pipe_pair();
        let forwarder = spawn_console_forwarder(read, "phase2-day3-eof".to_string());

        // Send a few lines, then close the write end.
        writeln!(&mut write, "boot line A").unwrap();
        writeln!(&mut write, "boot line B").unwrap();
        drop(write); // close write end → reader sees EOF

        forwarder
            .shutdown_and_join(Duration::from_secs(2))
            .await
            .expect("forwarder should complete on EOF within 2s");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn forwarder_shutdown_times_out_if_writer_stays_open() {
        let (read, write) = pipe_pair();
        let forwarder = spawn_console_forwarder(read, "phase2-day3-timeout".to_string());

        // Keep `write` alive so the read never sees EOF. The
        // shutdown_and_join must time out and surface a clear
        // error so the supervisor knows the guest is hung.
        let err = forwarder
            .shutdown_and_join(Duration::from_millis(200))
            .await
            .expect_err("forwarder must time out while writer is held");
        assert!(
            err.contains("did not finish within"),
            "expected timeout error, got: {err}"
        );

        // Drop the writer so the spawned task can exit, leaving
        // no leaked threads behind the test.
        drop(write);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn forwarder_skips_empty_lines() {
        // We can't easily intercept tracing events without
        // pulling in `tracing-subscriber` as a dev-dep, but we
        // can at least confirm the forwarder exits cleanly when
        // fed only blank lines + an EOF.
        let (read, mut write) = pipe_pair();
        let forwarder = spawn_console_forwarder(read, "phase2-day3-blanks".to_string());

        writeln!(&mut write).unwrap();
        writeln!(&mut write).unwrap();
        writeln!(&mut write, "actual content").unwrap();
        drop(write);

        forwarder
            .shutdown_and_join(Duration::from_secs(2))
            .await
            .expect("forwarder completes after blank lines + EOF");
    }
}
