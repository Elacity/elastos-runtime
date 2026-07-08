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

use std::io::BufRead;
use std::process::{Child, Command};
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

/// Spawn a command in its OWN process group (unix) so a deadline kill takes down the provider AND
/// anything it spawned — a killed parent whose helper child still holds the stdout pipe would
/// leave the read blocked past the deadline (exactly the hang the deadline exists to end).
pub(crate) fn spawn_grouped(cmd: &mut Command) -> std::io::Result<Child> {
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
                        fired.store(true, Ordering::SeqCst);
                        unsafe {
                            libc::kill(-(pid as i32), libc::SIGKILL);
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
pub(crate) fn reap_grouped(child: &mut Child) {
    const GRACE_TICKS: u32 = 20;
    const TICK: Duration = Duration::from_millis(50); // ~1s total grace for a clean exit
    for _ in 0..GRACE_TICKS {
        match child.try_wait() {
            Ok(Some(_)) => return, // exited on its own — reaped, nothing to kill
            Ok(None) => std::thread::sleep(TICK),
            Err(_) => return, // cannot reap here; the caller's Drop kill()+wait() is the backstop
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
    let _ = child.wait();
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
}
