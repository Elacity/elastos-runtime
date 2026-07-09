//! The ONE subprocess-conversation watchdog (Sprint 40/41): bounds ANY capsule-provider
//! conversation so no runtime thread ever parks forever on a hung or hostile subprocess.
//!
//! Sprint 40 bounded the chain-provider conversation; Sprint 41 factored that discipline here so
//! the chain, rights, and wallet providers share ONE implementation of the subtle bits — the
//! process-group spawn, the disarm-before-reap ordering (a recycled pid can never take a stray
//! group kill), the length-capped read, and the shared deadline knob. No third copy of the
//! ordering to drift.
//!
//! UNIX-ONLY kill (like the flock protections): elsewhere the watchdog is a stated no-op and the
//! old unbounded behavior remains.

use std::io::{BufRead, Read};
use std::process::{Child, Command, ExitStatus};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use serde_json::Value;

/// Longest single response line a capsule may emit. A JSON-RPC response is kilobytes; this is a
/// generous ceiling that turns a hostile/compromised provider streaming a newline-free firehose
/// (memory-exhaustion DoS within the deadline window — council S40 red-team F2) into a bounded
/// error instead of an OOM abort of the whole runtime.
pub(crate) const MAX_CAPSULE_LINE: u64 = 4 * 1024 * 1024;

/// How long ONE provider conversation (spawn → init → op → response) may take before the child is
/// killed. `ELASTOS_CHAIN_READ_DEADLINE_SECS` (kept as the shared name — one deadline for every
/// provider), default 30. A malformed value (or `< 1`) uses the DEFAULT with a loud warning: the
/// deadline is an availability protection, not a money decision, so a typo must not silently
/// remove it (fail SAFE, not fail open-ended). Set it ABOVE your P99 RPC roundtrip — too low
/// forces every live op to time out, the safe money direction but a real availability cliff.
pub(crate) fn capsule_read_deadline() -> Duration {
    const DEFAULT_SECS: u64 = 30;
    let secs = match std::env::var("ELASTOS_CHAIN_READ_DEADLINE_SECS") {
        Ok(raw) => match raw.trim().parse::<u64>() {
            Ok(n) if n >= 1 => n,
            _ => {
                tracing::warn!(
                    "ELASTOS_CHAIN_READ_DEADLINE_SECS={raw:?} is not a positive integer — \
                     using the default {DEFAULT_SECS}s deadline"
                );
                DEFAULT_SECS
            }
        },
        Err(_) => DEFAULT_SECS,
    };
    Duration::from_secs(secs)
}

/// Runtime-only SECRETS a spawned capsule must never inherit (Sprint 46, council S43 guardian F4 —
/// P16). Both are consumed exclusively IN-PROCESS by the runtime and passed onward as explicit op
/// ARGUMENTS where needed, so no capsule has a legitimate use for the env copy:
/// - `ELASTOS_DDRM_BUY_SIGNED_TX` — a fully broadcastable signed transaction. Read only by
///   `buy_authority` (the external-signature buy leg); leaked to a hostile provider BINARY it
///   could be broadcast out-of-band while the read leg fails "pre-broadcast".
/// - `ELASTOS_PAYMENT_TOKEN` — the HTTP payment rail's bearer token. Read only by `server.rs`
///   when wiring `HttpPaymentProvider`; a capsule holding it could charge the rail directly.
///
/// Stripping happens HERE, at the single spawn seam every capsule goes through (P5: one place, no
/// per-call-site copy to forget). This is a targeted denylist of runtime-only secrets, NOT a full
/// env allowlist — capsules legitimately read their own `ELASTOS_*` config (RPC URLs, chain ids,
/// bin paths), so an allowlist would need a per-capsule contract; that remains the stronger
/// tracked hardening (KNOWN_GAPS).
const RUNTIME_ONLY_SECRETS: &[&str] = &["ELASTOS_DDRM_BUY_SIGNED_TX", "ELASTOS_PAYMENT_TOKEN"];

/// Spawn a command in its OWN process group (unix) so a deadline kill takes down the provider AND
/// anything it spawned — a killed parent whose helper child still holds the stdout pipe would
/// leave the read blocked past the deadline (exactly the hang the deadline exists to end).
/// Also strips [`RUNTIME_ONLY_SECRETS`] from the child's environment (P16) — see the const's docs.
pub(crate) fn spawn_grouped(cmd: &mut Command) -> std::io::Result<Child> {
    for secret in RUNTIME_ONLY_SECRETS {
        cmd.env_remove(secret);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        cmd.process_group(0);
    }
    cmd.spawn()
}

/// A watchdog that group-SIGKILLs `pid` after `deadline` unless disarmed first.
///
/// ORDERING CONTRACT (council S40 red-team F1 / guardian F3): call [`disarm`](Self::disarm)
/// (which disarms AND joins) BEFORE reaping the child. The watchdog can then only ever kill a
/// child that has NOT yet been reaped, so a reaped-then-recycled pid can never receive a stray
/// group kill. For a persistent session (wallet), the child is reaped only at session drop, so
/// arm/disarm per read is safe by the same argument.
pub(crate) struct DeadlineWatchdog {
    fired: Arc<AtomicBool>,
    disarm_tx: mpsc::Sender<()>,
    handle: Option<JoinHandle<()>>,
}

impl DeadlineWatchdog {
    /// Arm a watchdog for `pid` (the child's own process group). Off-unix this is a no-op that
    /// never fires (the read stays unbounded, as stated).
    pub(crate) fn arm(pid: u32, deadline: Duration) -> Self {
        let fired = Arc::new(AtomicBool::new(false));
        let (disarm_tx, disarm_rx) = mpsc::channel::<()>();
        let handle = {
            let fired = fired.clone();
            std::thread::spawn(move || {
                if disarm_rx.recv_timeout(deadline).is_err() {
                    // `fired` is set ONLY where a kill actually happens (unix), so a deadline
                    // error is never minted on a platform where nothing was killed (S40 G-F2).
                    #[cfg(unix)]
                    {
                        // A pid of 0 would make `kill(-0)` signal the CALLER's OWN process group —
                        // runtime suicide dressed as an availability protection (council S42
                        // guardian F2). No legitimate child has pid 0; refuse to kill on it (and do
                        // not set `fired`, so no bogus deadline error is minted either).
                        if pid != 0 {
                            fired.store(true, Ordering::SeqCst);
                            unsafe {
                                libc::kill(-(pid as i32), libc::SIGKILL);
                            }
                        } else {
                            tracing::error!(
                                "DeadlineWatchdog armed on pid 0 — refusing kill(-0) (it would \
                                 signal the runtime's own process group)"
                            );
                        }
                    }
                    #[cfg(not(unix))]
                    {
                        let _ = (&fired, pid);
                    }
                }
            })
        };
        Self {
            fired,
            disarm_tx,
            handle: Some(handle),
        }
    }

    /// Disarm AND join the watchdog thread, returning whether it fired (killed the group). MUST
    /// be called before reaping the child (see the ordering contract).
    pub(crate) fn disarm(mut self) -> bool {
        let _ = self.disarm_tx.send(());
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
        self.fired.load(Ordering::SeqCst)
    }
}

impl Drop for DeadlineWatchdog {
    fn drop(&mut self) {
        // Defensive (council S41 guardian F2): a watchdog dropped WITHOUT an explicit `disarm`
        // (e.g. a future `?` early-return slipped between arm and disarm) would otherwise leave
        // the timer thread running until the FULL deadline and THEN group-kill — a latent stray
        // kill in a shared module. Disarming on drop makes an un-disarmed watchdog a no-op, not a
        // loaded gun. `disarm` takes `self` by value so it also drops through here, but it has
        // already sent and taken the handle: this send no-ops on the closed channel and the join
        // is skipped.
        let _ = self.disarm_tx.send(());
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

/// Reap a grouped capsule child WITHOUT reintroducing an unbounded park. The disarm-before-reap
/// ordering ends the *read* deadline, but a provider that answered every op and then refuses to
/// exit on EOF/`shutdown` would wedge the runtime thread on a plain `child.wait()` forever — the
/// very hang the deadline exists to end, slipped past the read into the reap (council S41 guardian
/// F1/F2). Call this AFTER [`DeadlineWatchdog::disarm`]: it gives the child a brief grace to exit
/// cleanly (the normal path), then group-SIGKILLs and reaps. `try_wait` never blocks and does not
/// free the pid until the child has actually exited, so the group-kill always targets our OWN
/// un-reaped child — the pid-reuse invariant that governs the whole module still holds.
///
/// Off-unix there is no group kill (as stated module-wide): the grace lapses and the final `wait`
/// is the old unbounded behavior.
///
/// Returns the child's [`ExitStatus`] when it could be collected (so a one-shot caller can fold a
/// non-success exit into its error), or `None` if `try_wait` itself errored.
pub(crate) fn reap_grouped(child: &mut Child) -> Option<ExitStatus> {
    const GRACE_TICKS: u32 = 20;
    const TICK: Duration = Duration::from_millis(50); // ~1s total grace for a clean exit
    for _ in 0..GRACE_TICKS {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status), // exited on its own — reaped, nothing to kill
            Ok(None) => std::thread::sleep(TICK),
            Err(_) => return None, // cannot reap here; the guard/session Drop reap is the backstop
        }
    }
    #[cfg(unix)]
    {
        // Still alive after the grace: end the conversation for good. The child is un-reaped, so
        // its group id is still ours — the kill cannot land on a recycled pid.
        unsafe {
            libc::kill(-(child.id() as i32), libc::SIGKILL);
        }
    }
    child.wait().ok()
}

/// A spawned capsule child that is BOUNDED-reaped on drop unless explicitly disarmed. Between spawn
/// and the moment the child is handed to its long-lived owner, MANY fallible steps run (take
/// stdin/stdout, read + parse a descriptor, extract fields) — every early return among them must
/// reap the child, or a hostile sidecar that forces one (a well-formed-but-incomplete descriptor,
/// an EPIPE on the first write, a self-exit) leaks a live process or a zombie (council S42 red-team
/// F1/F2). `std::process::Child::drop` neither kills nor waits, so this guard closes that gap: on
/// success `disarm()` transfers the child to the owner; on ANY early return the `Drop` reaps it via
/// [`reap_grouped`] (group-kill after a grace). This is the ONE reap-guard the access sidecars
/// share (P5) so the discipline cannot drift across copies.
pub(crate) struct ReapGuard(Option<Child>);

impl ReapGuard {
    pub(crate) fn new(child: Child) -> Self {
        Self(Some(child))
    }
    /// The armed child (present until [`disarm`](Self::disarm)); for taking stdin/stdout.
    pub(crate) fn child_mut(&mut self) -> &mut Child {
        self.0.as_mut().expect("child present until disarm")
    }
    /// The armed child's pid (its own process group), for arming a read watchdog on it. Panics
    /// rather than ever returning 0 — a `kill(-0)` would signal the GATEWAY's own group (council
    /// S42 red-team F4). The child is present until disarm by construction.
    pub(crate) fn pid(&self) -> u32 {
        self.0
            .as_ref()
            .map(Child::id)
            .expect("child present until disarm")
    }
    /// Success path: transfer the child out to its long-lived owner, disarming the guard.
    pub(crate) fn disarm(mut self) -> Child {
        self.0.take().expect("child present until disarm")
    }
}

impl Drop for ReapGuard {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            reap_grouped(&mut child);
        }
    }
}

/// How long a CONTENT-OPEN conversation (spawn → transcode/seal handshake → first descriptor line)
/// may take before the sidecar is killed. SEPARATE from [`capsule_read_deadline`] because a media
/// open runs ffmpeg + a seal handshake BEFORE it answers, so the RPC-tuned 30s would group-kill a
/// legitimate slow transcode and DENY the honest open (council S42 red-team F3 — an availability
/// cliff). `ELASTOS_CONTENT_OPEN_DEADLINE_SECS`, default 120; malformed/`< 1` ⇒ default with a
/// loud warning (the deadline is an availability protection, not a money decision — a typo must not
/// silently remove it). The per-op VIEW reads (segment/page/object) and the grant crypto keep the
/// shorter [`capsule_read_deadline`] — they are quick relays, not a transcode.
pub(crate) fn content_open_deadline() -> Duration {
    const DEFAULT_SECS: u64 = 120;
    let secs = match std::env::var("ELASTOS_CONTENT_OPEN_DEADLINE_SECS") {
        Ok(raw) => match raw.trim().parse::<u64>() {
            Ok(n) if n >= 1 => n,
            _ => {
                tracing::warn!(
                    "ELASTOS_CONTENT_OPEN_DEADLINE_SECS={raw:?} is not a positive integer — \
                     using the default {DEFAULT_SECS}s content-open deadline"
                );
                DEFAULT_SECS
            }
        },
        Err(_) => DEFAULT_SECS,
    };
    Duration::from_secs(secs)
}

/// Read one newline-delimited JSON response line from a capsule's stdout, LENGTH-bounded to
/// [`MAX_CAPSULE_LINE`] so a firehose provider is a bounded error, not an OOM.
pub(crate) fn read_capsule_line(reader: impl BufRead) -> Result<Value, String> {
    let mut line = String::new();
    // `take` caps the bytes read this call; a line at/over the cap is refused rather than grown
    // unbounded. `read_line` still stops at the first newline within the cap. Taking the reader
    // by value (callers pass `&mut reader`, and `&mut R: BufRead`) sidesteps the double-reference
    // method-resolution snag `.take()` hits on a `&mut impl BufRead`.
    let n = reader
        .take(MAX_CAPSULE_LINE + 1)
        .read_line(&mut line)
        .map_err(|e| format!("read capsule stdout: {e}"))?;
    if n == 0 {
        return Err("capsule exited before answering".to_string());
    }
    if line.len() as u64 > MAX_CAPSULE_LINE {
        return Err("capsule response line exceeds the size cap (refused)".to_string());
    }
    serde_json::from_str(line.trim()).map_err(|e| format!("parse capsule response: {e}"))
}

/// The marker an ACCESS-path (content open/view) deadline carries (Sprint 42). ACCESS NOTE: a
/// content-authority timeout is a DENY (fail-closed) — the exact mirror of the rights-decide rule
/// (`RIGHTS_DEADLINE_MARKER`), never a money decision. The sidecars this bounds (the media/object
/// authorities and the grant sidecar) sit on the OPEN and VIEW paths, not the pay spine, so a
/// timeout simply refuses the open/read; there is no charge to classify.
pub(crate) const ACCESS_DEADLINE_MARKER: &str = "content-authority read deadline exceeded";

/// Read one capsule response line bounded by an explicit `deadline`: arm a watchdog on `child_pid`'s
/// process group, read (length-capped), disarm. On a fire the child's group is SIGKILLed and this
/// returns an [`ACCESS_DEADLINE_MARKER`] error so the caller DENIES (fail-closed) — no access thread
/// parks forever on a hung content-authority sidecar. `what` names the sidecar for diagnostics.
///
/// This arms/disarms per read but does NOT reap — a persistent session reaps its child once at Drop
/// (via [`reap_grouped`]), so the disarm-before-reap ordering holds by construction (every per-read
/// disarm precedes the single Drop reap). After a fire the child is dead; the session's next read
/// EOFs and denies again (fail-closed), and Drop reaps the corpse.
///
/// NOTE: only the READ is watchdog-bounded. The paired request WRITE (the caller's `writeln!` of a
/// one-line JSON op) runs before this and is NOT bounded — it is safe because every op payload is a
/// short JSON line far below the OS pipe buffer, so the kernel accepts it without blocking on the
/// child draining. A future op with a payload approaching the pipe buffer would need its own bound.
fn read_line_deadlined_with(
    child_pid: u32,
    reader: impl BufRead,
    what: &str,
    deadline: Duration,
) -> Result<Value, String> {
    let watchdog = DeadlineWatchdog::arm(child_pid, deadline);
    let read = read_capsule_line(reader);
    let fired = watchdog.disarm();
    if fired {
        // Fail closed whether or not a late response also landed: a response the watchdog had to
        // kill for is by definition past the deadline, so denying it is the correct direction.
        return Err(format!(
            "{ACCESS_DEADLINE_MARKER}: no response within {}s — {what} killed; access DENIED \
             (fail-closed)",
            deadline.as_secs()
        ));
    }
    read
}

/// Bound a per-op VIEW read (segment/page/object relay) with the shorter [`capsule_read_deadline`]
/// — these are quick relays, not a transcode.
pub(crate) fn read_line_deadlined(
    child_pid: u32,
    reader: impl BufRead,
    what: &str,
) -> Result<Value, String> {
    read_line_deadlined_with(child_pid, reader, what, capsule_read_deadline())
}

/// Bound a content-OPEN descriptor read with the longer [`content_open_deadline`] — the sidecar
/// runs ffmpeg + a seal handshake before it answers, so the RPC-tuned deadline would kill an honest
/// slow open (council S42 red-team F3).
pub(crate) fn read_open_deadlined(
    child_pid: u32,
    reader: impl BufRead,
    what: &str,
) -> Result<Value, String> {
    read_line_deadlined_with(child_pid, reader, what, content_open_deadline())
}

/// Run a ONE-SHOT capsule read-to-EOF bounded by the shared [`capsule_read_deadline`]: arm a
/// watchdog on `child`'s group, read ALL of `stdout` to EOF (length-capped to [`MAX_CAPSULE_LINE`],
/// so a firehose sidecar is a bounded error not an OOM), disarm, then reap. Unlike the persistent
/// per-line readers this OWNS the reap (the child is single-use), so the caller must NOT also reap
/// — it hands the child in and this returns the parsed stdout (or a fail-closed error). On a fire
/// the group is SIGKILLed and this returns an [`ACCESS_DEADLINE_MARKER`] error; a non-success exit
/// status is folded into the error too (`what` names the sidecar). This is the ONE implementation
/// of the one-shot ordering — a caller (the grant sidecar) must not hand-roll it (council S42
/// guardian F3).
pub(crate) fn read_to_eof_deadlined(
    child: &mut Child,
    mut stdout: impl Read,
    what: &str,
) -> Result<String, String> {
    let deadline = capsule_read_deadline();
    let watchdog = DeadlineWatchdog::arm(child.id(), deadline);
    let mut buf = String::new();
    let read = (&mut stdout)
        .take(MAX_CAPSULE_LINE + 1)
        .read_to_string(&mut buf);
    let fired = watchdog.disarm();
    let status = reap_grouped(child); // disarm-before-reap: watchdog already joined above
    if fired {
        return Err(format!(
            "{ACCESS_DEADLINE_MARKER}: no response within {}s — {what} killed; access DENIED \
             (fail-closed)",
            deadline.as_secs()
        ));
    }
    read.map_err(|e| format!("read {what} stdout: {e}"))?;
    if buf.len() as u64 > MAX_CAPSULE_LINE {
        return Err(format!("{what} output exceeds the size cap (refused)"));
    }
    if let Some(status) = status {
        if !status.success() {
            return Err(format!("{what} exited with {status} (open fails closed)"));
        }
    }
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_malformed_deadline_env_keeps_the_default_protection() {
        let _g = crate::api::ddrm_env_lock();
        let prior = std::env::var("ELASTOS_CHAIN_READ_DEADLINE_SECS").ok();
        std::env::set_var("ELASTOS_CHAIN_READ_DEADLINE_SECS", "soon");
        assert_eq!(capsule_read_deadline(), Duration::from_secs(30));
        std::env::set_var("ELASTOS_CHAIN_READ_DEADLINE_SECS", "0");
        assert_eq!(capsule_read_deadline(), Duration::from_secs(30));
        std::env::set_var("ELASTOS_CHAIN_READ_DEADLINE_SECS", "5");
        assert_eq!(capsule_read_deadline(), Duration::from_secs(5));
        match prior {
            Some(v) => std::env::set_var("ELASTOS_CHAIN_READ_DEADLINE_SECS", v),
            None => std::env::remove_var("ELASTOS_CHAIN_READ_DEADLINE_SECS"),
        }
    }

    /// Sprint 46 (P16, council S43 guardian F4): the runtime-only secrets NEVER reach a spawned
    /// capsule's environment — a canary broadcastable-signed-tx set in the runtime env is ABSENT in
    /// the child, while an ordinary `ELASTOS_*` config var still passes through. The strip lives at
    /// `spawn_grouped` (the single spawn seam), so every provider/sidecar gets it for free.
    #[test]
    #[cfg(unix)]
    fn runtime_only_secrets_are_stripped_from_spawned_capsules() {
        let _g = crate::api::ddrm_env_lock();
        std::env::set_var("ELASTOS_DDRM_BUY_SIGNED_TX", "0xCANARY_SIGNED_TX");
        std::env::set_var("ELASTOS_PAYMENT_TOKEN", "canary-bearer");
        std::env::set_var("ELASTOS_S46_ORDINARY_CONFIG", "passes-through");

        let mut cmd = Command::new("/bin/sh");
        cmd.arg("-c").arg(
            "printf '%s|%s|%s' \
             \"${ELASTOS_DDRM_BUY_SIGNED_TX:-ABSENT}\" \
             \"${ELASTOS_PAYMENT_TOKEN:-ABSENT}\" \
             \"${ELASTOS_S46_ORDINARY_CONFIG:-ABSENT}\"",
        );
        cmd.stdout(std::process::Stdio::piped());
        let child = spawn_grouped(&mut cmd).expect("spawn canary shell");
        let out = child.wait_with_output().expect("collect canary output");

        std::env::remove_var("ELASTOS_DDRM_BUY_SIGNED_TX");
        std::env::remove_var("ELASTOS_PAYMENT_TOKEN");
        std::env::remove_var("ELASTOS_S46_ORDINARY_CONFIG");

        let seen = String::from_utf8_lossy(&out.stdout).to_string();
        assert_eq!(
            seen, "ABSENT|ABSENT|passes-through",
            "secrets stripped, ordinary config inherited: {seen}"
        );
    }
}
