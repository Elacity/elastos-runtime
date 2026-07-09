//! Bounded viewer-session admission (Sprint 47 — Track C1): the ONE sweep/cap discipline both
//! viewer session stores (media + object) apply, so neither can grow without bound.
//!
//! WHY: a viewer session pins real resources — sealed material, clear init segments (MBs for
//! media), and above all an `Arc` to a gateway-spawned KEY-AUTHORITY SUBPROCESS. The stores were
//! plain `HashMap`s: SEGMENT/bytes/page reads failed closed after `expires_at`, but the
//! `/init`, `/cover`, and manifest routes had NO expiry check (the lookup-sweep below is what
//! closes them — council S47 guardian F2), and the ENTRY (with its subprocess) lived until an
//! explicit client close — sustained opens grew memory and process count without limit, and an
//! abandoned tab pinned a child process forever.
//!
//! THE DISCIPLINE (the map surgery runs under the store's own lock; the DROPS do not):
//! - **Sweep**: every admission AND every lookup first REMOVES all EXPIRED entries, which are
//!   RETURNED to the caller and dropped after the lock releases (deferred-drop contract — a drop
//!   can reap an authority subprocess with a ~1s grace, and that must never stall the store).
//!   The last Arc drop reaps the subprocess via its bounded `Drop` (S42 `reap_grouped`); in-flight
//!   reads hold their own clone and finish undisturbed.
//! - **Cap**: after the sweep, if the store still holds `MAX_VIEWER_SESSIONS`, the entries with
//!   the SOONEST `expires_at` are evicted until the new session fits. TTL order is the eviction
//!   policy (not LRU): sessions are fixed-window views, so soonest-to-expire is least remaining
//!   value, needs no per-read bookkeeping, and cannot be gamed by an attacker touching their own
//!   sessions. The just-admitted session is never self-evicted (it fits by construction).
//!
//! FAIL-CLOSED DIRECTION: an evicted session's next read is a plain "no such session" — the
//! viewer re-opens, which re-runs the FULL authorization gate. Eviction can cost a re-open,
//! never grant access.

use std::collections::HashMap;

/// Ceiling on concurrently held viewer sessions PER STORE (media and object each). A session is
/// one open asset in one viewer; 256 concurrent opens per kind is far beyond a single-operator
/// deployment's real use. HONEST WORST-CASE MATH (council S47 red-team F6 / guardian F4): each
/// session's descriptor line is `MAX_CAPSULE_LINE`-capped (4 MiB ⇒ ≤ ~3 MiB decoded init bytes,
/// held ~3× across authority/session/track copies), and each authority child spawns its own
/// decrypt child — so a fully adversarial store tops out around ~2.3 GB + ~512 processes per
/// store. FAR better than unbounded (the byte bound rests on `MAX_CAPSULE_LINE`, not on init
/// segments being small), not "free". The cap is PROCESS-GLOBAL and principal-blind: eviction
/// pressure requires an AUTHENTICATED principal running (expensive, subprocess-spawning) opens,
/// and a greedy one can evict other principals' live sessions — a detection-visible nuisance
/// (the warn fires; the victim re-opens through the full auth gate), never an access grant.
/// Per-principal fairness is future work. Deliberately a compile-time constant, not env-tunable:
/// an operator raising it under pressure would be masking a leak the sweep is designed to surface.
pub(crate) const MAX_VIEWER_SESSIONS: usize = 256;

/// Sweep expired entries, then evict soonest-to-expire until `map` has room under `cap` for one
/// more entry. Called by both stores' `put` (admission) with `cap = MAX_VIEWER_SESSIONS`; `get`
/// (lookup) uses [`sweep_expired`] alone.
///
/// RETURNS the removed values instead of dropping them (DEFERRED-DROP CONTRACT): dropping a
/// session can run a subprocess reap with a ~1s grace (`MediaAuthorityProc::Drop` →
/// `reap_grouped`), and the sweep runs UNDER the store Mutex — dropping in place would stall
/// EVERY viewer's session lookup for N×grace on a sweep of N hung authorities. The call site
/// MUST let the returned Vec fall out of scope AFTER releasing the store lock. `live_evicted`
/// counts LIVE (unexpired) sessions evicted for cap room — 0 in the common case; non-zero is
/// worth a warn at the call site.
pub(crate) struct SweepOutcome<V> {
    /// Removed sessions — drop AFTER releasing the store lock (each drop may reap a subprocess).
    pub removed: Vec<V>,
    /// How many of `removed` were LIVE cap evictions (not expired sweeps).
    pub live_evicted: usize,
}

pub(crate) fn sweep_and_make_room<V>(
    map: &mut HashMap<String, V>,
    now: u64,
    cap: usize,
    expires_at: impl Fn(&V) -> u64,
) -> SweepOutcome<V> {
    let mut removed = sweep_expired(map, now, &expires_at);
    let mut live_evicted = 0;
    // cap.max(1): even a pathological cap of 0 admits the incoming session (the store must never
    // refuse an AUTHORIZED open outright — bounding is about memory, not authorization).
    while map.len() >= cap.max(1) {
        let Some(soonest) = map
            .iter()
            .min_by_key(|(_, v)| expires_at(v))
            .map(|(k, _)| k.clone())
        else {
            break;
        };
        if let Some(v) = map.remove(&soonest) {
            removed.push(v);
        }
        live_evicted += 1;
    }
    SweepOutcome {
        removed,
        live_evicted,
    }
}

/// Remove every entry whose `expires_at` is in the past and RETURN them (deferred-drop contract —
/// see [`sweep_and_make_room`]). Reads already fail closed on expiry; removal releases what the
/// entry PINS (sealed material, init bytes, the authority subprocess) — at the call site, after
/// the lock.
pub(crate) fn sweep_expired<V>(
    map: &mut HashMap<String, V>,
    now: u64,
    expires_at: &impl Fn(&V) -> u64,
) -> Vec<V> {
    let dead: Vec<String> = map
        .iter()
        .filter(|(_, v)| expires_at(v) < now)
        .map(|(k, _)| k.clone())
        .collect();
    dead.into_iter().filter_map(|k| map.remove(&k)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map_of(entries: &[(&str, u64)]) -> HashMap<String, u64> {
        entries.iter().map(|(k, e)| (k.to_string(), *e)).collect()
    }

    /// The store stays bounded under sustained admissions: expired entries go first, then the
    /// soonest-to-expire live ones; the incoming session always fits; distant sessions survive.
    #[test]
    fn sustained_admissions_stay_bounded_and_evict_soonest_to_expire() {
        let mut map = HashMap::new();
        for i in 0..500u64 {
            // Every admission runs the discipline, exactly as the stores do.
            sweep_and_make_room(&mut map, 1_000, 4, |v| *v);
            map.insert(format!("s{i}"), 2_000 + i); // all live, later = longer-lived
        }
        assert!(
            map.len() <= 4,
            "cap held under sustained opens: {}",
            map.len()
        );
        assert!(
            map.contains_key("s499"),
            "the most recent admission always survives its own cap"
        );
        // The survivors are the LONGEST-lived (soonest-to-expire were evicted).
        assert!(map.values().all(|e| *e >= 2_496), "survivors: {map:?}");
    }

    /// Expired entries are removed by the sweep even when the cap is not under pressure — and
    /// RETURNED for deferred drop (the resources they pin are released off the store lock).
    #[test]
    fn expired_sessions_are_swept_without_cap_pressure() {
        let mut map = map_of(&[("dead1", 5), ("dead2", 40), ("live", 5_000)]);
        let out = sweep_and_make_room(&mut map, 100, 256, |v| *v);
        assert_eq!(map.len(), 1);
        assert!(map.contains_key("live"));
        assert_eq!(
            out.removed.len(),
            2,
            "expired entries handed back for deferred drop"
        );
        assert_eq!(out.live_evicted, 0);
    }

    /// Live evictions are counted (the call site warns), expired sweeps are not — and BOTH are
    /// returned for deferred drop, never dropped under the caller's lock.
    #[test]
    fn only_live_evictions_are_counted_and_all_removals_are_deferred() {
        let mut map = map_of(&[("dead", 5), ("a", 200), ("b", 300)]);
        let out = sweep_and_make_room(&mut map, 100, 2, |v| *v);
        assert_eq!(out.live_evicted, 1);
        assert_eq!(
            out.removed.len(),
            2,
            "one expired + one live eviction, both deferred"
        );
        assert!(
            !map.contains_key("a"),
            "soonest-to-expire live entry evicted"
        );
        assert!(map.contains_key("b"));
    }

    /// A pathological cap of 0/1 still admits the incoming session (bounding is about memory,
    /// never a refusal of an authorized open) — and never loops forever on an empty map.
    #[test]
    fn a_zero_cap_never_refuses_or_spins() {
        let mut map: HashMap<String, u64> = HashMap::new();
        assert_eq!(
            sweep_and_make_room(&mut map, 100, 0, |v| *v).live_evicted,
            0
        );
        map.insert("s".into(), 500);
        assert_eq!(
            sweep_and_make_room(&mut map, 100, 0, |v| *v).live_evicted,
            1
        );
        assert!(map.is_empty(), "room made even at cap 0");
    }
}
