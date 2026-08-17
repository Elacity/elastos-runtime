//! Bounded, expiring, concurrency-safe **two-phase** replay state machine (DKMS-3).
//!
//! This is the node-held anti-replay + delegation-revocation store, replacing the old
//! one-shot `ReplayGuard::check_and_record` (which inserted a nonce BEFORE signatures were
//! checked and held a process-global lock across the on-chain RPC). Its contract, from the
//! remediation plan's "Replay and concurrency" invariants:
//!
//!   1. A structural-, owner-signature-, or request-signature-invalid grant never reserves a
//!      nonce — the reservation ([`ReplayStore::begin`]) is the LAST step, run only after all
//!      verification passes (see [`crate::access::verify_and_reserve`]).
//!   2. NO lock is held across RPC/crypto/IO: `begin`/`commit`/`abort` each take the internal
//!      state lock for a bounded, allocation-free critical section and release it before
//!      returning. The on-chain access read happens between `begin` and `commit`, with no lock
//!      held (proven by [`ReplayStore::state_lock_is_free`] + the node's concurrency tests).
//!   3. A successful `(delegation_nonce, request_nonce)` commits at most once, including under
//!      concurrent identical requests: the first `begin` reserves an `InFlight` slot; a
//!      concurrent duplicate observes it and is rejected as [`AccessVerifyError::Replayed`].
//!   4. An `InFlight` reservation is bounded and released on ANY authorization failure — the
//!      [`Reservation`] is an RAII guard whose `Drop` aborts (removes the slot) unless
//!      [`Reservation::commit`] promoted it to a seen entry through a short atomic transition.
//!   5. Seen entries expire no later than the delegation's bounded expiry; delegation-nonce
//!      revocations expire at the delegation expiry the operator recorded.
//!   6. Capacity exhaustion FAILS CLOSED with a stable error + metric; an unexpired
//!      seen/revoked/in-flight record is NEVER evicted to admit new work.
//!   7. Every collection has a documented global bound; `seen` also has a per-delegation bound.
//!
//! Keys are fixed-size Keccak-256 digests of length-framed nonce bytes — never the raw,
//! attacker-controlled nonce strings — so nothing here can be turned into an unbounded-key or
//! log-injection vector, and no raw nonce is ever retained or logged.

use std::collections::{BinaryHeap, HashMap};
use std::sync::{Arc, Mutex};

use sha3::{Digest, Keccak256};

use crate::access::AccessVerifyError;

/// Max concurrent `InFlight` reservations. The node serves each connection on its own thread and
/// runs a SYNCHRONOUS recover loop per connection, so at most one reservation is in flight per
/// active connection; connections are themselves capped (dkms-authority `MAX_ACTIVE_CONNECTIONS`
/// = 512). This is 2x that headroom — past it, `begin` fails closed rather than growing the map.
pub const MAX_IN_FLIGHT: usize = 1024;
/// Max retained successful `(delegation_nonce, request_nonce)` seen entries. Bounds live memory by
/// the delegation window (<= 24h) x request rate rather than by process uptime, since entries expire
/// at the delegation's own `expires_at`. ~2^18 entries x ~64B ≈ 16MB worst case; past it, `begin`
/// fails closed (never evict an unexpired record).
pub const MAX_SEEN: usize = 262_144;
/// Per-delegation cap on live seen entries: one delegation (one 24h window) issuing distinct
/// request nonces cannot claim more than this slice of [`MAX_SEEN`], so a single owner can't crowd
/// every other caller out of the shared map.
pub const MAX_SEEN_PER_DELEGATION: usize = 4_096;
/// Max retained delegation-nonce revocations. Each expires at the delegation's `expires_at`, so the
/// live set is bounded by the number of delegations revoked within one window.
pub const MAX_REVOCATIONS: usize = 16_384;
/// Max DECODED length (bytes) of either nonce before it is hashed into a key. A cheap DoS guard
/// applied before any hashing/insertion; real nonces are 8–32 bytes, this is generous headroom.
pub const MAX_NONCE_BYTES: usize = 64;
/// An `InFlight` reservation that never commits/aborts (only reachable via a leaked guard, e.g. a
/// hard process fault between `begin` and completion) is reclaimed once its deadline passes. RAII
/// makes this pure defense-in-depth: the normal path always aborts or commits within milliseconds.
///
/// SAFETY BOUND: this MUST stay strictly greater than the worst-case `begin`→`commit` latency, i.e.
/// `MAX_COVERED_ADDRESSES × rpc_pool_len × per_call_timeout_seconds` (the sequential on-chain poll
/// between the two phases). If the reservation could be pruned mid-authorization, a concurrent
/// duplicate `begin` would see neither `in_flight` nor `seen` and reserve the same nonce again. The
/// node enforces this at startup (`dkms-authority` `NodeChain::from_env`), refusing to construct a
/// pool large/slow enough to violate it. `pub` so that startup check can reference this constant.
pub const IN_FLIGHT_TTL_SECONDS: u64 = 120;

/// Domain separators so the three key spaces (seen pair, delegation, revocation) cannot collide.
const SEEN_DOMAIN: &[u8] = b"elastos.ddrm.replay.seen.v1";
const DEL_DOMAIN: &[u8] = b"elastos.ddrm.replay.del.v1";

type Key = [u8; 32];

/// A min-heap expiry index over `(expiry, key)`, used to prune a companion map without ever
/// scanning it. Uses lazy deletion: a key re-inserted with a fresher expiry leaves a stale heap
/// entry behind, which the pruner drops by matching the map's CURRENT expiry before removing. Each
/// entry is pushed once and popped once over its lifetime ⇒ pruning is O(1) amortized per insert and
/// only ever touches already-expired heap heads, never a live map entry.
#[derive(Default)]
struct ExpiryIndex {
    // `std::cmp::Reverse` turns the max-heap into a min-heap keyed on expiry.
    heap: BinaryHeap<std::cmp::Reverse<(u64, Key)>>,
}

impl ExpiryIndex {
    fn push(&mut self, expiry: u64, key: Key) {
        self.heap.push(std::cmp::Reverse((expiry, key)));
    }

    /// Pop every head whose expiry is `< now`, invoking `remove(key, expiry)` for each. `remove`
    /// applies the CURRENT-expiry guard so a re-inserted fresher entry is preserved.
    fn drain_expired(&mut self, now: u64, mut remove: impl FnMut(&Key, u64)) {
        while let Some(std::cmp::Reverse((expiry, _))) = self.heap.peek() {
            if *expiry >= now {
                break;
            }
            let std::cmp::Reverse((expiry, key)) = self.heap.pop().expect("peeked");
            remove(&key, expiry);
        }
    }
}

#[derive(Default)]
struct Inner {
    /// `seen_key -> in-flight deadline`. A reservation between `begin` and commit/abort.
    in_flight: HashMap<Key, u64>,
    // NOTE: this expiry heap's size is bounded by begin-rate × IN_FLIGHT_TTL_SECONDS (lazy deletion
    // leaves a stale entry per reservation until its deadline is drained), NOT by MAX_IN_FLIGHT.
    in_flight_exp: ExpiryIndex,
    /// `seen_key -> (delegation expiry, del_key)`. A committed, single-use request nonce. The
    /// del_key is retained so pruning can decrement the per-delegation counter exactly.
    // NOTE: `seen` can transiently exceed MAX_SEEN by up to MAX_IN_FLIGHT, because `begin` admits a
    // reservation while `seen.len() < MAX_SEEN` but `commit` inserts UNCONDITIONALLY — so every
    // already-reserved in-flight slot may still commit above the cap. Correct and bounded (soft cap).
    seen: HashMap<Key, (u64, Key)>,
    seen_exp: ExpiryIndex,
    /// `del_key -> live seen count`, for the per-delegation bound. Incremented on commit, decremented
    /// (and removed at zero) as the delegation's seen entries expire — so it is bounded by the number
    /// of live delegations, never leaks across expired ones.
    per_delegation: HashMap<Key, u32>,
    /// `del_key -> revocation expiry`. A revoked delegation nonce.
    revoked: HashMap<Key, u64>,
    revoked_exp: ExpiryIndex,
    /// Monotonic count of capacity-exhaustion rejections (fail-closed metric, no secrets).
    capacity_rejections: u64,
}

/// The node-held, cheaply-cloneable handle to the shared replay state. `Clone` shares one backing
/// store (an `Arc`), exactly like the caller-revocation set — one per node process, cloned into
/// every connection thread so a commit/revocation is visible to all of them immediately. `Default`
/// yields an independent empty store (fixtures/tests).
#[derive(Clone, Default)]
pub struct ReplayStore(Arc<Mutex<Inner>>);

impl std::fmt::Debug for ReplayStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never renders keys/nonces — only bounded sizes.
        let g = self.lock();
        f.debug_struct("ReplayStore")
            .field("in_flight", &g.in_flight.len())
            .field("seen", &g.seen.len())
            .field("revoked", &g.revoked.len())
            .field("capacity_rejections", &g.capacity_rejections)
            .finish()
    }
}

impl ReplayStore {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        // A poisoned replay lock must not wedge the node: recover the state and fail closed on the
        // individual request path, never panic the whole serve loop.
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Revoke a delegation (by its nonce bytes) until `expires_at`. Fails closed with
    /// [`AccessVerifyError::CapacityExhausted`] if the revocation table is full of UNEXPIRED entries
    /// — a revocation is never silently dropped (that would fail open) nor evicts a live revocation.
    pub fn revoke(
        &self,
        delegation_nonce: &[u8],
        expires_at: u64,
        now: u64,
    ) -> Result<(), AccessVerifyError> {
        if delegation_nonce.len() > MAX_NONCE_BYTES {
            return Err(AccessVerifyError::DelMalformed);
        }
        let key = del_key(delegation_nonce);
        let mut g = self.lock();
        g.prune(now);
        // Refreshing an existing revocation (same delegation) is always allowed.
        if !g.revoked.contains_key(&key) && g.revoked.len() >= MAX_REVOCATIONS {
            g.capacity_rejections += 1;
            return Err(AccessVerifyError::CapacityExhausted);
        }
        g.revoked.insert(key, expires_at);
        g.revoked_exp.push(expires_at, key);
        Ok(())
    }

    /// Read-only revocation check for the caller that wants to reject a revoked delegation BEFORE
    /// doing signature work. `begin` re-checks this under the same lock, so skipping it is safe.
    pub fn validate_not_revoked(
        &self,
        delegation_nonce: &[u8],
        now: u64,
    ) -> Result<(), AccessVerifyError> {
        if delegation_nonce.len() > MAX_NONCE_BYTES {
            return Err(AccessVerifyError::DelMalformed);
        }
        let key = del_key(delegation_nonce);
        let g = self.lock();
        match g.revoked.get(&key) {
            Some(&exp) if exp >= now => Err(AccessVerifyError::Revoked),
            _ => Ok(()),
        }
    }

    /// PHASE 4 — short locked transition to `InFlight`. Run ONLY after every structural + signature
    /// check has passed. Rejects, fail-closed and without mutating state, a revoked delegation
    /// ([`AccessVerifyError::Revoked`]), an already-seen OR already-in-flight pair
    /// ([`AccessVerifyError::Replayed`] — this is what makes concurrent identical requests resolve
    /// to at most one commit), an over-long nonce, and capacity exhaustion
    /// ([`AccessVerifyError::CapacityExhausted`]). On success it reserves the slot and returns an
    /// RAII [`Reservation`]; the lock is released before returning, so the caller does its on-chain
    /// read with NO lock held.
    pub fn begin(
        &self,
        delegation_nonce: &[u8],
        request_nonce: &[u8],
        expires_at: u64,
        now: u64,
    ) -> Result<Reservation, AccessVerifyError> {
        if delegation_nonce.len() > MAX_NONCE_BYTES {
            return Err(AccessVerifyError::DelMalformed);
        }
        if request_nonce.len() > MAX_NONCE_BYTES {
            return Err(AccessVerifyError::ReqMalformed);
        }
        let dkey = del_key(delegation_nonce);
        let skey = seen_key(delegation_nonce, request_nonce);

        let mut g = self.lock();
        g.prune(now);

        if g.revoked.get(&dkey).is_some_and(|&exp| exp >= now) {
            return Err(AccessVerifyError::Revoked);
        }
        if g.seen.contains_key(&skey) || g.in_flight.contains_key(&skey) {
            // Already committed, or a concurrent identical request already holds the slot.
            return Err(AccessVerifyError::Replayed);
        }
        // Capacity: check BOTH the in-flight table (this insert) and the seen table (the eventual
        // commit) and the per-delegation slice, so we never admit work we couldn't commit, and never
        // evict a live record. Every rejection is fail-closed + metered.
        if g.in_flight.len() >= MAX_IN_FLIGHT
            || g.seen.len() >= MAX_SEEN
            || g.per_delegation.get(&dkey).copied().unwrap_or(0) as usize >= MAX_SEEN_PER_DELEGATION
        {
            g.capacity_rejections += 1;
            return Err(AccessVerifyError::CapacityExhausted);
        }

        let deadline = now.saturating_add(IN_FLIGHT_TTL_SECONDS);
        g.in_flight.insert(skey, deadline);
        g.in_flight_exp.push(deadline, skey);
        Ok(Reservation {
            store: self.clone(),
            del_key: dkey,
            seen_key: skey,
            expires_at,
            done: false,
        })
    }

    /// How many committed `(delegation_nonce, request_nonce)` pairs are currently retained.
    /// READ-ONLY observability (tests + metrics): it changes no decision and holds the lock only for
    /// the read. The only sanctioned way to see whether the collection is bounded.
    pub fn tracked_len(&self) -> usize {
        self.lock().seen.len()
    }

    /// Count of capacity-exhaustion rejections since start (fail-closed metric).
    pub fn capacity_rejections(&self) -> u64 {
        self.lock().capacity_rejections
    }

    /// How many replay reservations are currently in flight (a `begin` not yet `commit`ted or
    /// dropped). READ-ONLY observability (ops counters + tests): it changes no decision and holds the
    /// lock only for the read. Bounded, secret-free — mirrors [`tracked_len`](Self::tracked_len).
    pub fn in_flight_len(&self) -> usize {
        self.lock().in_flight.len()
    }

    /// TEST/DIAGNOSTIC: `true` iff the internal state lock is free at this instant. Used to PROVE no
    /// lock is held across the on-chain RPC: while a reservation's access read is in flight, this
    /// must return `true` (the old design held the lock for the whole RPC and would return `false`).
    pub fn state_lock_is_free(&self) -> bool {
        self.0.try_lock().is_ok()
    }
}

impl Inner {
    /// Drop every expired seen / in-flight / revoked entry via the time-ordered heaps. Never scans a
    /// live map; only pops already-expired heap heads (amortized O(1) per prior insert).
    fn prune(&mut self, now: u64) {
        let seen = &mut self.seen;
        let per_delegation = &mut self.per_delegation;
        self.seen_exp.drain_expired(now, |key, expiry| {
            if let std::collections::hash_map::Entry::Occupied(e) = seen.entry(*key) {
                // Only remove if this heap entry matches the CURRENT stored expiry (lazy deletion).
                if e.get().0 == expiry {
                    let (_, dkey) = e.remove();
                    // Decrement the delegation's live-seen counter, removing it at zero so the
                    // per-delegation map is bounded by live delegations, not by all-time ones.
                    if let std::collections::hash_map::Entry::Occupied(mut d) =
                        per_delegation.entry(dkey)
                    {
                        let c = d.get_mut();
                        *c = c.saturating_sub(1);
                        if *c == 0 {
                            d.remove();
                        }
                    }
                }
            }
        });

        let in_flight = &mut self.in_flight;
        self.in_flight_exp.drain_expired(now, |key, deadline| {
            if let std::collections::hash_map::Entry::Occupied(e) = in_flight.entry(*key) {
                if *e.get() == deadline {
                    e.remove();
                }
            }
        });

        let revoked = &mut self.revoked;
        self.revoked_exp.drain_expired(now, |key, expiry| {
            if let std::collections::hash_map::Entry::Occupied(e) = revoked.entry(*key) {
                if *e.get() == expiry {
                    e.remove();
                }
            }
        });
    }
}

/// PHASE 4→6 RAII reservation. Between `begin` and completion the caller runs the on-chain access
/// read with NO lock held. On success the caller calls [`Reservation::commit`], atomically promoting
/// the `InFlight` slot to a committed seen entry. On ANY failure — a denied/unavailable access read,
/// an early `?` return, or a panic — `Drop` runs `abort`, removing the `InFlight` slot so the nonce
/// is NOT burned and can be retried. This is what makes "denial burns nothing" hold by construction.
#[must_use = "a Reservation must be committed on success or it will abort (release the nonce) on drop"]
pub struct Reservation {
    store: ReplayStore,
    del_key: Key,
    seen_key: Key,
    expires_at: u64,
    done: bool,
}

impl std::fmt::Debug for Reservation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never renders the (hashed) keys; a Reservation is opaque in diagnostics.
        f.debug_struct("Reservation")
            .field("committed", &self.done)
            .finish()
    }
}

impl Reservation {
    /// Promote the in-flight reservation to a committed seen entry (single, short locked
    /// transition). Idempotent-safe: consumes `self`, and the subsequent `Drop` is a no-op.
    ///
    /// The seen entry is recorded UNCONDITIONALLY — a committed nonce IS authorized, so it must be
    /// marked seen even when its own `InFlight` slot was already pruned by `IN_FLIGHT_TTL_SECONDS`
    /// (which the node's startup bound prevents in normal operation, but which must still fail
    /// safe). The old guard (`if in_flight.remove(...).is_some()`) would, on a pruned slot, record
    /// nothing — leaving the nonce replayable. We still remove the slot if present, and we only
    /// count a brand-new key toward the per-delegation slice so a duplicate that committed first
    /// (possible only in that pruned-slot window) is not double-counted.
    pub fn commit(mut self) {
        let mut g = self.store.lock();
        g.in_flight.remove(&self.seen_key);
        if !g.seen.contains_key(&self.seen_key) {
            *g.per_delegation.entry(self.del_key).or_insert(0) += 1;
        }
        g.seen
            .insert(self.seen_key, (self.expires_at, self.del_key));
        g.seen_exp.push(self.expires_at, self.seen_key);
        self.done = true;
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        if self.done {
            return;
        }
        // ABORT: release the in-flight slot. The nonce was never committed, so a legitimate retry
        // (a fresh request nonce, or the same one after a transient denial) is free to proceed.
        let mut g = self.store.lock();
        g.in_flight.remove(&self.seen_key);
    }
}

/// Fixed-size key for a `(delegation_nonce, request_nonce)` pair. Length-framed so no pair of
/// distinct nonces can alias, and domain-separated from the delegation key space.
fn seen_key(delegation_nonce: &[u8], request_nonce: &[u8]) -> Key {
    let mut h = Keccak256::new();
    h.update(SEEN_DOMAIN);
    h.update((delegation_nonce.len() as u64).to_le_bytes());
    h.update(delegation_nonce);
    h.update((request_nonce.len() as u64).to_le_bytes());
    h.update(request_nonce);
    h.finalize().into()
}

/// Fixed-size key for a delegation nonce (revocation + per-delegation accounting).
fn del_key(delegation_nonce: &[u8]) -> Key {
    let mut h = Keccak256::new();
    h.update(DEL_DOMAIN);
    h.update((delegation_nonce.len() as u64).to_le_bytes());
    h.update(delegation_nonce);
    h.finalize().into()
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_000_000;

    #[test]
    fn begin_commit_makes_a_pair_single_use() {
        let store = ReplayStore::new();
        let r = store
            .begin(b"del-A", b"req-1", NOW + 60, NOW)
            .expect("first begin ok");
        r.commit();
        assert_eq!(store.tracked_len(), 1);
        // The same pair is now a replay.
        assert_eq!(
            store.begin(b"del-A", b"req-1", NOW + 60, NOW).unwrap_err(),
            AccessVerifyError::Replayed
        );
    }

    #[test]
    fn abort_on_drop_does_not_burn_the_nonce() {
        let store = ReplayStore::new();
        {
            let _r = store
                .begin(b"del-A", b"req-1", NOW + 60, NOW)
                .expect("begin ok");
            // dropped without commit → abort
        }
        assert_eq!(
            store.tracked_len(),
            0,
            "an aborted reservation leaves no seen entry"
        );
        // The pair is free to be retried and this time committed.
        store
            .begin(b"del-A", b"req-1", NOW + 60, NOW)
            .expect("retry after abort ok")
            .commit();
        assert_eq!(store.tracked_len(), 1);
    }

    #[test]
    fn commit_records_seen_even_after_the_in_flight_slot_was_pruned() {
        let store = ReplayStore::new();
        let expires_at = NOW + 10_000;
        let r = store
            .begin(b"del-A", b"req-1", expires_at, NOW)
            .expect("first begin ok");
        // Simulate begin→commit exceeding IN_FLIGHT_TTL_SECONDS: an unrelated operation at a clock
        // PAST the reservation's deadline runs prune(), dropping the InFlight slot before we commit.
        store
            .revoke(
                b"unrelated-del",
                NOW + IN_FLIGHT_TTL_SECONDS + 200,
                NOW + IN_FLIGHT_TTL_SECONDS + 1,
            )
            .expect("revoke triggers prune");
        // A committed nonce IS authorized: commit must record it even though its slot was pruned.
        r.commit();
        assert_eq!(
            store.tracked_len(),
            1,
            "commit must record the seen entry even when its InFlight slot was already pruned",
        );
        // Proof it is now durably seen: replaying the SAME pair is rejected.
        assert_eq!(
            store.begin(b"del-A", b"req-1", expires_at, NOW).unwrap_err(),
            AccessVerifyError::Replayed,
        );
    }

    #[test]
    fn a_concurrent_duplicate_is_rejected_while_in_flight() {
        let store = ReplayStore::new();
        let _first = store
            .begin(b"del-A", b"req-1", NOW + 60, NOW)
            .expect("first in flight");
        // A second identical begin, while the first is still in flight, is refused.
        assert_eq!(
            store.begin(b"del-A", b"req-1", NOW + 60, NOW).unwrap_err(),
            AccessVerifyError::Replayed,
        );
    }

    #[test]
    fn distinct_pairs_do_not_collide() {
        let store = ReplayStore::new();
        let a = store.begin(b"del-A", b"req-1", NOW + 60, NOW).expect("a");
        let b = store
            .begin(b"del-A", b"req-2", NOW + 60, NOW)
            .expect("b (distinct req)");
        let c = store
            .begin(b"del-B", b"req-1", NOW + 60, NOW)
            .expect("c (distinct del)");
        a.commit();
        b.commit();
        c.commit();
        assert_eq!(store.tracked_len(), 3);
    }

    #[test]
    fn revoked_delegation_is_refused_and_expires_by_policy() {
        let store = ReplayStore::new();
        store.revoke(b"del-A", NOW + 100, NOW).expect("revoke ok");
        assert_eq!(
            store.validate_not_revoked(b"del-A", NOW),
            Err(AccessVerifyError::Revoked)
        );
        assert_eq!(
            store.begin(b"del-A", b"req-1", NOW + 60, NOW).unwrap_err(),
            AccessVerifyError::Revoked
        );
        // After the revocation expiry, it is pruned and no longer blocks.
        assert!(store.validate_not_revoked(b"del-A", NOW + 101).is_ok());
    }

    #[test]
    fn seen_entries_expire_no_later_than_the_delegation() {
        let store = ReplayStore::new();
        let expires_at = NOW + 60;
        for i in 0..2_000u32 {
            store
                .begin(b"del-A", format!("req-{i}").as_bytes(), expires_at, NOW)
                .expect("first use accepted")
                .commit();
        }
        assert_eq!(
            store.tracked_len(),
            2_000,
            "baseline: every commit is remembered"
        );
        // The delegation has expired; a begin at a later clock prunes all of them first.
        store
            .begin(b"del-B", b"req-0", NOW + 200, expires_at + 1)
            .expect("fresh delegation accepted")
            .commit();
        assert!(
            store.tracked_len() <= 1,
            "seen entries must expire no later than the delegation expiry — {} stale entries retained",
            store.tracked_len(),
        );
    }

    #[test]
    fn capacity_exhaustion_fails_closed_without_evicting_live_state() {
        let store = ReplayStore::new();
        // Fill the per-delegation slice to its cap with committed entries.
        for i in 0..MAX_SEEN_PER_DELEGATION as u32 {
            store
                .begin(b"del-A", format!("req-{i}").as_bytes(), NOW + 3600, NOW)
                .expect("under cap")
                .commit();
        }
        let before = store.tracked_len();
        // One more for the SAME delegation must fail closed (per-delegation bound) and evict nothing.
        assert_eq!(
            store
                .begin(b"del-A", b"req-overflow", NOW + 3600, NOW)
                .unwrap_err(),
            AccessVerifyError::CapacityExhausted,
        );
        assert_eq!(
            store.tracked_len(),
            before,
            "no live seen entry was evicted to admit new work"
        );
        assert!(store.capacity_rejections() >= 1, "the rejection is metered");
    }

    #[test]
    fn over_long_nonces_are_refused_before_hashing() {
        let store = ReplayStore::new();
        let big = vec![0u8; MAX_NONCE_BYTES + 1];
        assert_eq!(
            store.begin(&big, b"req", NOW + 60, NOW).unwrap_err(),
            AccessVerifyError::DelMalformed
        );
        assert_eq!(
            store.begin(b"del", &big, NOW + 60, NOW).unwrap_err(),
            AccessVerifyError::ReqMalformed
        );
    }
}
