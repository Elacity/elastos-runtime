//! W1b/C3-glue — turn kernel NFLOG egress drops into signed `EgressDenied` custody.
//!
//! The per-TAP firewall ([`elastos_crosvm::EgressFirewall`]) DROPS egress in-kernel and
//! rate-limited-`log`s each drop through NFLOG group [`EGRESS_NFLOG_GROUP`]. This server-side
//! glue reads that group (via the crosvm [`NflogReader`]), maps the TAP device back to the
//! canonical `vm-{name}` via the [`TapRegistry`], and emits a signed `EgressDenied` onto the
//! SAME durable audit chain the spend/grant custody rides — so a blocked egress attempt is
//! a first-class, tamper-evident custody record.
//!
//! Invariants (carried from C1/C2 and the design forks):
//! - **Enforcement is independent of this reader.** The in-kernel DROP always happens; if the
//!   NFLOG socket can't bind (no privilege / no `nfnetlink_log`) or the reader dies, we lose
//!   audit records, never containment. Emission is best-effort.
//! - **The TAP→identity map is overwrite-correct.** A recycled TAP is re-labelled by the next
//!   VM at firewall-install (before any of its drops are read), so a stale entry never
//!   misattributes a drop.
//! - **A drop on an unknown TAP is still recorded** (honest `vm-unknown:<tap>`), never silently
//!   discarded — absence is never a pass.
//!
//! SCOPE (this glue): per-drop `EgressDenied` emission. The rate-limit reconciliation marker
//! (`suppressed = total_dropped - per_drop_emitted`, read from the nft `counter` BEFORE
//! teardown) is the immediate follow-on — its pure delta lives here ([`reconcile_suppressed`]),
//! but the periodic/at-teardown counter read is wired alongside the C4 hardware run, where the
//! live `nft` counter validates it. Per-drop `suppressed` is `0`, which is exact for every
//! non-flood drop (the kernel rate-limit only elides logs above the per-second cap).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};

use elastos_runtime::primitives::audit::{AuditEvent, AuditLog};
use elastos_runtime::primitives::time::SecureTimestamp;

/// Maps a TAP device (`cvXXXXXXXX`) to the canonical capsule identity (`vm-{name}`), so a kernel
/// drop logged on a TAP becomes a custody event keyed on the SAME identity as the spend/grant
/// chain (W1b/F1: the firewall keys on the TAP, the audit keys on `vm-{name}`).
///
/// Cheaply cloneable (shared inner `Arc`): the supervisor records, the reader resolves. Lock-poison
/// tolerant — a poisoned lock degrades to "unknown TAP" (still records the drop) rather than a panic.
#[derive(Clone, Default)]
pub struct TapRegistry {
    inner: Arc<RwLock<HashMap<String, String>>>,
}

impl TapRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record (or OVERWRITE) the `vm-{name}` for a TAP. Overwrite is the correctness mechanism for
    /// TAP reuse: a new VM on a recycled TAP re-labels here at firewall-install, before any of its
    /// drops are read, so a stale entry can never misattribute a drop.
    pub fn record(&self, tap: &str, capsule_id: &str) {
        if let Ok(mut m) = self.inner.write() {
            m.insert(tap.to_string(), capsule_id.to_string());
        }
    }

    /// Resolve a TAP to its capsule identity, or `None` if unknown / lock poisoned.
    pub fn resolve(&self, tap: &str) -> Option<String> {
        self.inner.read().ok()?.get(tap).cloned()
    }

    /// Forget a TAP (best-effort cleanliness on teardown). A lingering entry is harmless because
    /// [`record`](Self::record) overwrites on reuse; this only bounds memory over a long session.
    pub fn forget(&self, tap: &str) {
        if let Ok(mut m) = self.inner.write() {
            m.remove(tap);
        }
    }

    /// Snapshot the currently-known TAPs (for the reconcile poller to sweep). Lock-poison tolerant
    /// (a poisoned lock yields an empty sweep this tick, never a panic).
    pub fn taps(&self) -> Vec<String> {
        self.inner
            .read()
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default()
    }
}

/// Per-TAP reconciliation bookkeeping shared between the NFLOG reader (which increments `emitted`
/// once per per-drop record it lands) and the reconcile sweep / final-read (which increments
/// `suppressed_emitted` as it accounts for kernel-suppressed drops). Two counters because the
/// suppressed total is `kernel_counter - emitted`, and we must only emit the *new* slice of it.
#[derive(Default, Clone, Copy)]
struct TapCounters {
    /// Per-drop `EgressDenied` records the reader has landed for this TAP.
    emitted: u64,
    /// Suppressed-marker drops already accounted onto the chain for this TAP.
    suppressed_emitted: u64,
}

/// Shared per-TAP counters. Cheaply cloneable; lock-poison tolerant (a poisoned lock degrades to a
/// no-op for that operation — we lose a reconciliation tick, never enforcement and never a panic).
#[derive(Clone, Default)]
pub struct EgressCounters {
    inner: Arc<Mutex<HashMap<String, TapCounters>>>,
}

impl EgressCounters {
    pub fn new() -> Self {
        Self::default()
    }

    /// Note that the reader landed one more per-drop `EgressDenied` for this TAP.
    pub fn note_emitted(&self, tap: &str) {
        if let Ok(mut m) = self.inner.lock() {
            m.entry(tap.to_string()).or_default().emitted += 1;
        }
    }

    /// Given the live kernel drop counter for `tap`, return the not-yet-accounted suppressed delta
    /// and advance the accounted baseline by it. `suppressed_total = counter - emitted` (saturating,
    /// per [`reconcile_suppressed`]); the returned delta is that minus what we already emitted, so
    /// repeated sweeps never double-count and a counter read that lags the emit count yields `0`.
    pub fn take_suppressed_delta(&self, tap: &str, total_dropped: u64) -> u64 {
        let Ok(mut m) = self.inner.lock() else {
            return 0;
        };
        let c = m.entry(tap.to_string()).or_default();
        let suppressed_total = reconcile_suppressed(total_dropped, c.emitted);
        let delta = suppressed_total.saturating_sub(c.suppressed_emitted);
        c.suppressed_emitted = c.suppressed_emitted.saturating_add(delta);
        delta
    }

    /// Forget a TAP's counters (best-effort cleanliness on teardown; only bounds memory).
    pub fn forget(&self, tap: &str) {
        if let Ok(mut m) = self.inner.lock() {
            m.remove(tap);
        }
    }
}

/// The capsule identity to record on an `EgressDenied` for a given TAP. A known TAP yields its
/// `vm-{name}`; an unknown TAP yields an honest `vm-unknown:<tap>` marker — the drop is ALWAYS
/// recorded, never dropped because we couldn't attribute it.
pub fn capsule_id_for_tap(registry: &TapRegistry, tap: &str) -> String {
    registry
        .resolve(tap)
        .unwrap_or_else(|| format!("vm-unknown:{tap}"))
}

/// Build a signable `EgressDenied` event from a parsed kernel drop. Pure (modulo the timestamp).
pub fn build_egress_denied(
    capsule_id: String,
    tap: &str,
    dest: &str,
    proto: &str,
    suppressed: u64,
) -> AuditEvent {
    AuditEvent::EgressDenied {
        timestamp: SecureTimestamp::now(),
        capsule_id,
        tap: tap.to_string(),
        dest: dest.to_string(),
        proto: proto.to_string(),
        suppressed,
    }
}

/// Drops the kernel rate-limit suppressed beyond what the reader recorded per-drop: the nft
/// `counter` ground truth minus the per-drop records emitted. Saturating (a counter read that
/// races behind the emit count can never produce a negative — it yields `0`).
pub fn reconcile_suppressed(total_dropped: u64, per_drop_emitted: u64) -> u64 {
    total_dropped.saturating_sub(per_drop_emitted)
}

/// Spawn the single process-wide NFLOG egress-audit reader: bind [`EGRESS_NFLOG_GROUP`] and, for
/// every parsed drop, emit a signed `EgressDenied` (per-drop `suppressed = 0`) onto the durable
/// chain, keyed on the canonical `vm-{name}` from `registry`.
///
/// Best-effort and **enforcement-independent**: if the socket cannot bind (no privilege / no
/// `nfnetlink_log`) or `recv` fails, the reader logs loudly and exits — the in-kernel DROP is
/// unaffected. Runs on a dedicated blocking thread (the `recv` is a blocking syscall).
#[cfg(target_os = "linux")]
pub fn spawn_egress_audit_reader(
    audit_log: Arc<AuditLog>,
    registry: TapRegistry,
    counters: EgressCounters,
) {
    use elastos_crosvm::{NflogReader, EGRESS_NFLOG_GROUP};
    let _ = std::thread::Builder::new()
        .name("egress-audit-reader".to_string())
        .spawn(move || {
            let mut reader = match NflogReader::bind(EGRESS_NFLOG_GROUP) {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!(
                        "egress audit reader disabled (NFLOG bind failed: {e}); \
                         enforcement is unaffected — drops are still in-kernel"
                    );
                    return;
                }
            };
            tracing::info!("egress audit reader bound NFLOG group {EGRESS_NFLOG_GROUP}");
            loop {
                match reader.recv() {
                    Ok(drops) => {
                        for d in drops {
                            let capsule_id = capsule_id_for_tap(&registry, &d.tap);
                            audit_log.emit_best_effort(build_egress_denied(
                                capsule_id, &d.tap, &d.dest, &d.proto, 0,
                            ));
                            // Count this per-drop record so the reconcile sweep can tell how many
                            // kernel drops the rate-limit suppressed (counter − emitted = delta).
                            counters.note_emitted(&d.tap);
                        }
                    }
                    Err(e) => {
                        tracing::warn!(
                            "egress audit reader recv failed ({e}); exiting reader \
                             — enforcement unaffected"
                        );
                        return;
                    }
                }
            }
        });
}

/// Non-Linux: no NFLOG, so the reader is a no-op (the firewall itself is Linux-only too).
#[cfg(not(target_os = "linux"))]
pub fn spawn_egress_audit_reader(
    _audit_log: Arc<AuditLog>,
    _registry: TapRegistry,
    _counters: EgressCounters,
) {
}

/// How often the reconcile sweep reads the live nft drop counters. Frequent enough that a synthetic
/// flood's suppressed marker lands in near-real-time (so C4 evidence is captured during VM life,
/// independent of the final-read-before-teardown), cheap enough to be invisible at idle.
#[cfg(target_os = "linux")]
const RECONCILE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);

/// Spawn the periodic reconcile sweep: every [`RECONCILE_INTERVAL`], read each known TAP's live nft
/// drop counter and emit a signed suppressed-marker `EgressDenied` for any kernel-suppressed drops
/// not yet accounted (the rate-limit elides logs above the per-second cap; the counter is ground
/// truth). Best-effort and enforcement-independent, exactly like the reader.
#[cfg(target_os = "linux")]
pub fn spawn_egress_reconcile_poller(
    audit_log: Arc<AuditLog>,
    registry: TapRegistry,
    counters: EgressCounters,
) {
    let _ = std::thread::Builder::new()
        .name("egress-reconcile".to_string())
        .spawn(move || loop {
            std::thread::sleep(RECONCILE_INTERVAL);
            for tap in registry.taps() {
                reconcile_tap(&audit_log, &registry, &counters, &tap);
            }
        });
}

/// Read `tap`'s live kernel drop counter and, if the rate-limit suppressed any drops beyond what
/// the reader has logged per-drop, emit ONE signed suppressed-marker `EgressDenied` for the new
/// slice (`dest = "(rate-limited)"`, `proto = "-"`). Best-effort: a missing counter (chains already
/// gone) or a zero delta is a no-op. Used by both the periodic sweep and the supervisor's
/// final-read-before-teardown (which MUST run before the chains are deleted, or the delta is lost).
#[cfg(target_os = "linux")]
pub fn reconcile_tap(
    audit_log: &AuditLog,
    registry: &TapRegistry,
    counters: &EgressCounters,
    tap: &str,
) {
    let Some(total) = elastos_crosvm::read_drop_count_for_tap(tap) else {
        return;
    };
    let delta = counters.take_suppressed_delta(tap, total);
    if delta > 0 {
        let capsule_id = capsule_id_for_tap(registry, tap);
        audit_log.emit_best_effort(build_egress_denied(
            capsule_id,
            tap,
            "(rate-limited)",
            "-",
            delta,
        ));
    }
}

/// Non-Linux: no nft counters, so the final reconcile is a no-op (keeps the supervisor call site
/// platform-uniform).
#[cfg(not(target_os = "linux"))]
pub fn reconcile_tap(
    _audit_log: &AuditLog,
    _registry: &TapRegistry,
    _counters: &EgressCounters,
    _tap: &str,
) {
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_records_resolves_and_overwrites_on_tap_reuse() {
        let reg = TapRegistry::new();
        assert_eq!(reg.resolve("cv1a2b3c4d"), None, "unknown TAP is None");
        reg.record("cv1a2b3c4d", "vm-act-emitter");
        assert_eq!(reg.resolve("cv1a2b3c4d").as_deref(), Some("vm-act-emitter"));
        // TAP reuse: a new VM on the same TAP re-labels (overwrite), so a later drop is
        // attributed to the CURRENT occupant, never the dead one.
        reg.record("cv1a2b3c4d", "vm-chat");
        assert_eq!(reg.resolve("cv1a2b3c4d").as_deref(), Some("vm-chat"));
        reg.forget("cv1a2b3c4d");
        assert_eq!(reg.resolve("cv1a2b3c4d"), None, "forgotten TAP is None");
    }

    #[test]
    fn unknown_tap_is_recorded_not_dropped() {
        let reg = TapRegistry::new();
        // A drop on a TAP we can't attribute still produces a custody identity — never discarded.
        assert_eq!(
            capsule_id_for_tap(&reg, "cvdeadbeef"),
            "vm-unknown:cvdeadbeef"
        );
        reg.record("cvdeadbeef", "vm-known");
        assert_eq!(capsule_id_for_tap(&reg, "cvdeadbeef"), "vm-known");
    }

    #[test]
    fn build_egress_denied_carries_canonical_identity_and_drop_fields() {
        // The heart of C3-glue: a kernel drop on a known TAP becomes an EgressDenied keyed on the
        // canonical vm-{name} (custody correlation with spend/grant), preserving dest/proto.
        let reg = TapRegistry::new();
        reg.record("cv1a2b3c4d", "vm-chat");
        let capsule_id = capsule_id_for_tap(&reg, "cv1a2b3c4d");
        let ev = build_egress_denied(capsule_id, "cv1a2b3c4d", "1.2.3.4:443", "tcp", 0);
        match ev {
            AuditEvent::EgressDenied {
                capsule_id,
                tap,
                dest,
                proto,
                suppressed,
                ..
            } => {
                assert_eq!(capsule_id, "vm-chat", "keyed on vm-{{name}}, not the TAP");
                assert_eq!(tap, "cv1a2b3c4d");
                assert_eq!(dest, "1.2.3.4:443");
                assert_eq!(proto, "tcp");
                assert_eq!(suppressed, 0);
            }
            other => panic!("expected EgressDenied, got {other:?}"),
        }
    }

    #[test]
    fn egress_denied_emits_onto_the_durable_chain_and_verifies() {
        // End-to-end on the pure path: the event the reader builds chains + verifies on a
        // file-backed log, exactly as it will on the shared durable chain in production.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit.log");
        let log = AuditLog::with_file(&path).unwrap();
        let reg = TapRegistry::new();
        reg.record("cv1a2b3c4d", "vm-chat");
        let ev = build_egress_denied(
            capsule_id_for_tap(&reg, "cv1a2b3c4d"),
            "cv1a2b3c4d",
            "8.8.8.8:53",
            "udp",
            0,
        );
        log.emit(ev).expect("emit");
        // Self-verify via the live verify-on-read path (chain_attestation walks the chain under
        // the log's own key) — the EgressDenied record chains + verifies like any custody event.
        let att = log.chain_attestation().expect("file-backed ⇒ attestable");
        assert!(att.verified, "the EgressDenied record verifies: {att:?}");
        assert_eq!(att.records, 1);
    }

    #[test]
    fn take_suppressed_delta_emits_only_the_new_slice_and_never_double_counts() {
        let counters = EgressCounters::new();
        let tap = "cv1a2b3c4d";
        // No drops logged yet, counter at 0 → nothing suppressed.
        assert_eq!(counters.take_suppressed_delta(tap, 0), 0);
        // Reader logged 3 per-drop records; counter shows 10 → 7 suppressed, all new.
        counters.note_emitted(tap);
        counters.note_emitted(tap);
        counters.note_emitted(tap);
        assert_eq!(counters.take_suppressed_delta(tap, 10), 7);
        // Same counter on the next sweep → nothing new (no double-count).
        assert_eq!(counters.take_suppressed_delta(tap, 10), 0);
        // Flood continues: counter climbs to 25 with no new per-drop logs → 15 new suppressed.
        assert_eq!(counters.take_suppressed_delta(tap, 25), 15);
        // A counter read that lags the emit count never yields a negative slice.
        counters.note_emitted(tap);
        assert_eq!(counters.take_suppressed_delta(tap, 25), 0);
    }

    #[test]
    fn reconcile_suppressed_is_saturating_ground_truth() {
        // Under flood the counter exceeds the per-drop records → the delta is the suppressed count.
        assert_eq!(reconcile_suppressed(50, 10), 40);
        // No flood: counter == emitted → nothing suppressed.
        assert_eq!(reconcile_suppressed(10, 10), 0);
        // A counter read that lags the emit count never goes negative.
        assert_eq!(reconcile_suppressed(8, 10), 0);
    }
}
