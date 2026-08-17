//! Stage 8 operational counters — process-global, secret-free fail-closed metrics.
//!
//! These mirror the `capacity_rejections` counter already carried inside
//! [`ddrm_envelope::replay::ReplayStore`]: a monotonic count of a specific fail-closed event, held
//! as a plain atomic and surfaced through a bounded, secret-free render. They exist so an operator
//! can SEE when the hardened node is failing closed (quorum split, allow-list misconfiguration,
//! replay capacity pressure, a durable-revocation write failure, a substituted lifecycle manifest)
//! WITHOUT the node ever logging key material, raw nonces, addresses, signatures, or endpoint URLs.
//!
//! INVARIANT: nothing here records a secret. Every counter is a `u64` event tally; the render emits
//! only those tallies and the replay store's bounded collection sizes. No CEK, master/caller seed,
//! session key, full nonce, operator signature, sealed share, allow-list entry, or endpoint URL is
//! ever touched. Counters are best-effort observability (`Relaxed` ordering) — they never gate a
//! security decision, so a lost increment under contention cannot weaken the fail-closed boundary.

use std::sync::atomic::{AtomicU64, Ordering};

use ddrm_envelope::replay::ReplayStore;

/// Covered-subject fan-out rejected because it exceeded `MAX_COVERED_ADDRESSES` (DKMS-4 bound).
pub static GRANT_LIST_BOUND_REJECTED: AtomicU64 = AtomicU64::new(0);
/// Multi-RPC reads that failed closed because reachable endpoints DISAGREED (DKMS-2).
pub static QUORUM_DISAGREEMENT: AtomicU64 = AtomicU64::new(0);
/// Multi-RPC reads that failed closed because too few endpoints were reachable (DKMS-2).
pub static QUORUM_UNAVAILABLE: AtomicU64 = AtomicU64::new(0);
/// Durable security-state loads that failed closed at startup (corrupt/unknown-schema/bad-sig, DKMS-6).
pub static REVOCATION_LOAD_FAILURES: AtomicU64 = AtomicU64::new(0);
/// Revocations refused because the DURABLE write failed (never reported as durable when it is not, DKMS-6).
pub static REVOCATION_WRITE_FAILURES: AtomicU64 = AtomicU64::new(0);
/// Startups aborted because the caller allow-list configuration was malformed/ambiguous (DKMS-8).
pub static INVALID_ALLOW_LIST: AtomicU64 = AtomicU64::new(0);
/// Lifecycle ops refused because the operator authorization did not open under the exact v2 manifest (DKMS-5).
pub static LIFECYCLE_MANIFEST_MISMATCH: AtomicU64 = AtomicU64::new(0);

/// Record one occurrence of a fail-closed event. `Relaxed`: a counter never gates a decision.
#[inline]
pub fn incr(counter: &AtomicU64) {
    counter.fetch_add(1, Ordering::Relaxed);
}

/// Render the current counters as ONE bounded, secret-free line for operator logs. Pulls the replay
/// store's own bounded metrics (capacity rejections + live collection sizes) so the whole fail-closed
/// picture is one grep-able line. Never renders a secret — only tallies and collection sizes.
pub fn render_line(replay: &ReplayStore) -> String {
    format!(
        "dkms-authority ops-counters: grant_list_bound_rejected={} quorum_disagreement={} \
         quorum_unavailable={} replay_capacity_rejected={} replay_in_flight={} replay_seen={} \
         revocation_load_failures={} revocation_write_failures={} invalid_allow_list={} \
         lifecycle_manifest_mismatch={}",
        GRANT_LIST_BOUND_REJECTED.load(Ordering::Relaxed),
        QUORUM_DISAGREEMENT.load(Ordering::Relaxed),
        QUORUM_UNAVAILABLE.load(Ordering::Relaxed),
        replay.capacity_rejections(),
        replay.in_flight_len(),
        replay.tracked_len(),
        REVOCATION_LOAD_FAILURES.load(Ordering::Relaxed),
        REVOCATION_WRITE_FAILURES.load(Ordering::Relaxed),
        INVALID_ALLOW_LIST.load(Ordering::Relaxed),
        LIFECYCLE_MANIFEST_MISMATCH.load(Ordering::Relaxed),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn incr_advances_the_counter_and_render_is_secret_free() {
        // A local counter so the test is order-independent of the process-global statics.
        static LOCAL: AtomicU64 = AtomicU64::new(0);
        assert_eq!(LOCAL.load(Ordering::Relaxed), 0);
        incr(&LOCAL);
        incr(&LOCAL);
        assert_eq!(
            LOCAL.load(Ordering::Relaxed),
            2,
            "each incr adds exactly one"
        );

        // The render is a single bounded line naming every counter, with no obvious secret markers.
        let replay = ReplayStore::default();
        let line = render_line(&replay);
        assert!(line.contains("grant_list_bound_rejected="));
        assert!(line.contains("quorum_disagreement="));
        assert!(line.contains("quorum_unavailable="));
        assert!(line.contains("replay_capacity_rejected="));
        assert!(line.contains("replay_in_flight="));
        assert!(line.contains("replay_seen="));
        assert!(line.contains("revocation_load_failures="));
        assert!(line.contains("revocation_write_failures="));
        assert!(line.contains("invalid_allow_list="));
        assert!(line.contains("lifecycle_manifest_mismatch="));
        // One line, and bounded (no keys/nonces can bloat it — it is pure tallies).
        assert!(!line.contains('\n'), "the counter render is a single line");
        assert!(line.len() < 512, "bounded render");
    }
}
