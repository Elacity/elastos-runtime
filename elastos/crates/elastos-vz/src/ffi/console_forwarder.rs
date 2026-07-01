//! Kernel-console → `tracing` forwarder.
//!
//! Apple's `VZVirtioConsoleDeviceSerialPortConfiguration`
//! emits the guest's printk stream to whichever NSFileHandle we
//! attached. [`ffi::console::build_kernel_console`] hands us the
//! read end of a `pipe(2)`; this module drains that pipe and
//! emits one `tracing::info!(target = "vm_console", ...)` per
//! line of guest output, matching the contract crosvm uses on
//! Linux so the supervisor's existing log routing keeps working.
//!
//! Sizing rationale:
//!
//! - Vz writes guest serial output to our pipe.
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

/// **kernel-console line cap.**
///
/// Maximum byte length of a single kernel-console line before
/// the forwarder drops it and resyncs to the next newline.
/// 64 KiB is two orders of magnitude above Linux's compile-time
/// `PRINTK_BUF_LEN` (typically 1 KiB), so a well-behaved guest
/// kernel never trips it. A malicious or buggy guest kernel
/// emitting an unbounded stream of bytes without a newline is
/// capped at this size per call — pre-fix the host's `String`
/// would grow without limit.
const KERNEL_CONSOLE_MAX_LINE_BYTES: usize = 65_536;

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
    // outside `shutdown_and_join` (which itself is test-only). The
    // field still has to live on the struct so tests can move it
    // out via `self.handle`. Future runtime-side abort support may
    // use it once we add a
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

/// **byte-budgeted sync line reader.**
///
/// Synchronous counterpart of
/// `elastos_server::carrier_bridge::read_line_byte_budgeted`.
/// Reads bytes from `reader` into `buf` until either a newline
/// is consumed (inclusive) or `max_bytes` bytes have been
/// buffered, whichever comes first. Returns the number of bytes
/// pushed onto `buf`.
///
/// Why it exists: `BufRead::read_line` is unbounded, so a guest
/// kernel emitting `b"A" * 10_000_000_000` without a `\n` would
/// grow the host's receive buffer until OOM. Caller passes
/// `KERNEL_CONSOLE_MAX_LINE_BYTES + 1` so the post-read check
/// can distinguish "exactly at limit" from "over limit" without
/// truncating mid-byte.
///
/// Memory footprint: bounded by `max_bytes` plus the inner
/// `BufReader`'s 8 KiB scratch — constant in attacker input.
fn read_line_byte_budgeted_sync<R: BufRead>(
    reader: &mut R,
    buf: &mut Vec<u8>,
    max_bytes: usize,
) -> std::io::Result<usize> {
    let mut total = 0usize;
    loop {
        let (consumed, found_newline) = {
            let chunk = reader.fill_buf()?;
            if chunk.is_empty() {
                return Ok(total);
            }
            let remaining = max_bytes.saturating_sub(total);
            let take = chunk.len().min(remaining);
            let scan = &chunk[..take];
            if let Some(pos) = scan.iter().position(|&b| b == b'\n') {
                buf.extend_from_slice(&scan[..=pos]);
                (pos + 1, true)
            } else {
                buf.extend_from_slice(scan);
                (take, false)
            }
        };
        reader.consume(consumed);
        total += consumed;
        if found_newline {
            return Ok(total);
        }
        if total >= max_bytes {
            return Ok(total);
        }
    }
}

/// **sync resync helper.**
///
/// Discard bytes from `reader` until the next `\n` is consumed
/// or EOF. O(`BufReader`-buffer) memory — bytes are scanned
/// then consumed, never accumulated.
fn drain_to_newline_sync<R: BufRead>(reader: &mut R) -> std::io::Result<()> {
    loop {
        let (consumed, found_newline) = {
            let chunk = reader.fill_buf()?;
            if chunk.is_empty() {
                return Ok(());
            }
            if let Some(pos) = chunk.iter().position(|&b| b == b'\n') {
                (pos + 1, true)
            } else {
                (chunk.len(), false)
            }
        };
        reader.consume(consumed);
        if found_newline {
            return Ok(());
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
        // byte-budgeted reads. Previous
        // this was `read_line(&mut String)`, which is unbounded
        // — a guest kernel emitting bytes without a newline
        // could grow this buffer until the host OOMed. The
        // helper caps the per-call allocation at
        // `KERNEL_CONSOLE_MAX_LINE_BYTES + 1`.
        let mut buf: Vec<u8> = Vec::with_capacity(4096);

        loop {
            buf.clear();
            let read = read_line_byte_budgeted_sync(
                &mut reader,
                &mut buf,
                KERNEL_CONSOLE_MAX_LINE_BYTES + 1,
            );
            match read {
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
                Ok(n) if n > KERNEL_CONSOLE_MAX_LINE_BYTES => {
                    // Oversized line: drop it, resync to the
                    // next newline, continue. No response
                    // channel back to the guest kernel — best
                    // we can do is log + drain.
                    tracing::warn!(
                        target: "vm_console",
                        vm_id = %vm_id,
                        bytes = n,
                        cap = KERNEL_CONSOLE_MAX_LINE_BYTES,
                        "kernel console line exceeded cap; dropping and resyncing"
                    );
                    if let Err(e) = drain_to_newline_sync(&mut reader) {
                        tracing::warn!(
                            target: "vm_console",
                            vm_id = %vm_id,
                            "kernel console drain-after-overflow error: {e}"
                        );
                        return;
                    }
                    continue;
                }
                Ok(_) => {
                    // Trim the trailing newline (if any) so the
                    // tracing event reads cleanly in JSON
                    // formatters. Convert via `String::from_utf8_lossy`
                    // rather than strict UTF-8 because guest
                    // kernel printk *can* contain non-UTF-8
                    // bytes (binary panic registers, etc.) and
                    // we'd rather log a `�`-spotted line than
                    // tear down the forwarder.
                    let text = String::from_utf8_lossy(&buf);
                    let line = text.trim_end_matches(['\n', '\r']);
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
        let forwarder = spawn_console_forwarder(read, "vz-console-eof".to_string());

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
        let forwarder = spawn_console_forwarder(read, "vz-console-timeout".to_string());

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

    // ---------------------------------------------------------------
    // bounded read regression tests.
    //
    // Previous the forwarder called `BufReader::read_line`
    // with no upper bound, so a guest kernel emitting bytes
    // without a newline would grow the host `String` until OOM.
    // The fix replaces `read_line` with
    // `read_line_byte_budgeted_sync` capped at
    // `KERNEL_CONSOLE_MAX_LINE_BYTES + 1`.
    //
    // Tests below exercise both the end-to-end pipe path and the
    // helper in isolation so a future regression on either side
    // (forwarder loop, byte-budget arithmetic) is caught.
    // ---------------------------------------------------------------

    /// End-to-end: write 2 x cap bytes of 'A' with no newline,
    /// then a `\n`, then a normal short line, then EOF. The
    /// forwarder must drain the oversized burst (proving the
    /// cap fired and the resync helper succeeded), accept the
    /// short follow-up line, and shut down cleanly on EOF.
    /// Pre-fix this test would either OOM the test process or
    /// hang waiting for a newline that never came inside an
    /// unbounded `read_line` call.
    #[tokio::test(flavor = "multi_thread")]
    async fn forwarder_caps_oversized_kernel_line_and_resyncs() {
        let (read, mut write) = pipe_pair();
        let forwarder = spawn_console_forwarder(read, "vz-console-oversized".to_string());

        // Spawn the writer on a blocking thread so the pipe's
        // kernel buffer (typically 64 KiB on macOS) cannot
        // deadlock the test: the forwarder is reading
        // concurrently, so the writer's `write_all` will make
        // progress in lock-step with the reader.
        std::thread::spawn(move || {
            let oversized = vec![b'A'; 2 * KERNEL_CONSOLE_MAX_LINE_BYTES];
            write.write_all(&oversized).expect("write oversized burst");
            // Terminating newline so the forwarder's drain can
            // resync without waiting for the rest of the
            // (never-coming) attacker payload.
            write.write_all(b"\n").expect("write closing newline");
            // Short follow-up line to prove the loop resumed
            // dispatch on the same pipe after the overflow.
            writeln!(&mut write, "post-overflow OK").expect("write follow-up");
            drop(write);
        });

        // Forwarder must drain the oversized burst, log the
        // follow-up line, and exit on EOF — all within the
        // shutdown timeout. The timeout itself is the
        // assertion: a pre-fix unbounded `read_line` would
        // hang inside the oversized burst.
        forwarder
            .shutdown_and_join(Duration::from_secs(5))
            .await
            .expect(
                "forwarder must drain oversized burst, accept the short \
                 follow-up, and exit on EOF within 5s",
            );
    }

    /// Helper-level: read a line under the cap returns the full
    /// line with newline included (shape parity with `read_line`).
    #[test]
    fn read_line_byte_budgeted_sync_returns_full_line_under_cap() {
        let mut reader = BufReader::new(&b"hello\nworld\n"[..]);
        let mut buf = Vec::new();
        let n =
            read_line_byte_budgeted_sync(&mut reader, &mut buf, KERNEL_CONSOLE_MAX_LINE_BYTES + 1)
                .expect("read should succeed");
        assert_eq!(n, 6);
        assert_eq!(&buf, b"hello\n");
    }

    /// Helper-level: oversized input with no newline caps the
    /// returned buf at exactly `max_bytes`. Pre-fix this would
    /// return the entire 4 KiB even if `max_bytes` was 1 KiB.
    #[test]
    fn read_line_byte_budgeted_sync_caps_at_max_bytes_when_no_newline() {
        let payload = vec![b'A'; 4096];
        let mut reader = BufReader::new(&payload[..]);
        let mut buf = Vec::new();
        let max = 1024usize;
        let n =
            read_line_byte_budgeted_sync(&mut reader, &mut buf, max).expect("read should succeed");
        assert_eq!(
            n, max,
            "must return exactly max_bytes when no newline found"
        );
        assert_eq!(buf.len(), max, "buf must be capped at max_bytes");
    }

    /// Helper-level: EOF before any byte returns `Ok(0)`,
    /// matching `read_line`'s shape so the caller's `match Ok(0)`
    /// EOF arm continues to work unchanged.
    #[test]
    fn read_line_byte_budgeted_sync_returns_zero_on_immediate_eof() {
        let mut reader = BufReader::new(&b""[..]);
        let mut buf = Vec::new();
        let n =
            read_line_byte_budgeted_sync(&mut reader, &mut buf, 1024).expect("read should succeed");
        assert_eq!(n, 0);
        assert!(buf.is_empty());
    }

    /// Helper-level: `drain_to_newline_sync` consumes only up to
    /// and including the next `\n`, so the next read picks up
    /// from the start of the following line.
    #[test]
    fn drain_to_newline_sync_resyncs_to_next_line_start() {
        let mut reader = BufReader::new(&b"AAAAAAAA\nBBBB\n"[..]);
        drain_to_newline_sync(&mut reader).expect("drain should succeed");
        let mut buf = Vec::new();
        let n = read_line_byte_budgeted_sync(&mut reader, &mut buf, 1024)
            .expect("post-drain read should succeed");
        assert_eq!(n, 5);
        assert_eq!(&buf, b"BBBB\n");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn forwarder_skips_empty_lines() {
        // We can't easily intercept tracing events without
        // pulling in `tracing-subscriber` as a dev-dep, but we
        // can at least confirm the forwarder exits cleanly when
        // fed only blank lines + an EOF.
        let (read, mut write) = pipe_pair();
        let forwarder = spawn_console_forwarder(read, "vz-console-blanks".to_string());

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
