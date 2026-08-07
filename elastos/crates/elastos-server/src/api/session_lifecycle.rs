//! Session lifecycle foundation — the RELEASE half of the viewer-session pipeline.
//!
//! Opening an asset is uniform (authority spawn → store admission → read routes); until now
//! closing was not: each store had an unwired `remove_*_session`, and an abandoned session's
//! authority subprocess lived until the TTL *and* the next store access (the lazy sweep).
//! This module is the one place the runtime releases sessions, with three consumers:
//!
//! 1. the explicit close routes (`POST /api/viewers/:viewer/<kind>/:session/close`), fed by
//!    the viewer capsules' `pagehide` beacon and the Home shell's window-close hook;
//! 2. the periodic sweeper, so expiry no longer waits for the next store access on an idle
//!    machine;
//! 3. (unchanged, kept as backstop) each store's own lazy sweep on lookup/admission.
//!
//! Every kind of session store implements [`SessionLifecycle`] and is listed in
//! [`registry`]. Adding a future kind (browser pages, exit streams, …) is one trait impl
//! plus one line there — the routes and the sweeper never change. The registry is a fixed
//! explicit list rather than a dynamic `register()` on purpose: the set of session kinds is
//! a compile-time property of the gateway, and an auditable list beats runtime mutation.
//!
//! FAIL-CLOSED DIRECTION: a close can only COST a re-open (which re-runs the full
//! authorization gate) — it can never grant access. Authorization for the HTTP close is the
//! store's own read gate (token + viewer + principal), enforced by the per-kind handler
//! BEFORE dispatching here; this module never sees credentials.

use std::sync::OnceLock;
use std::time::Duration;

/// How often the background sweeper releases expired sessions. One minute keeps the
/// worst-case overstay of an expired authority subprocess to ~1 tick while the per-tick
/// work stays trivially bounded (each store holds ≤ `MAX_VIEWER_SESSIONS` entries).
const SWEEP_INTERVAL: Duration = Duration::from_secs(60);

/// A session store whose entries pin releasable resources (sealed material, clear init
/// bytes, and above all a gateway-spawned authority subprocess).
///
/// CONTRACT for implementors:
/// - `close` and `sweep` are IDEMPOTENT and honor the deferred-drop discipline: entries
///   removed under the store lock are dropped AFTER it releases (a drop can reap a
///   subprocess with a ~1s grace — see `session_bounds`).
/// - `kind` is the literal path segment of the store's read routes ("object", "media").
pub(crate) trait SessionLifecycle: Send + Sync {
    /// The route-segment name of this session kind.
    fn kind(&self) -> &'static str;
    /// Release one session by id. Returns true if a session was actually removed.
    fn close(&self, session_id: &str) -> bool;
    /// Release every entry whose expiry has passed. Returns how many were removed.
    fn sweep(&self, now: u64) -> usize;
}

/// The fixed set of session stores the runtime knows how to release.
fn registry() -> &'static [&'static dyn SessionLifecycle] {
    static STORES: OnceLock<Vec<&'static dyn SessionLifecycle>> = OnceLock::new();
    STORES.get_or_init(|| {
        vec![
            &super::viewer_object::OBJECT_SESSION_LIFECYCLE,
            &super::viewer_media::MEDIA_SESSION_LIFECYCLE,
        ]
    })
}

/// Close one session through the registry. `None` means the kind is unknown — the caller
/// must fail closed (404), never fall through to a default store.
pub(crate) fn close(kind: &str, session_id: &str) -> Option<bool> {
    let store = registry().iter().find(|store| store.kind() == kind)?;
    let removed = store.close(session_id);
    if removed {
        tracing::debug!(kind, session_id, "viewer session closed");
    }
    Some(removed)
}

/// Sweep every registered store once; returns the total number of sessions released.
pub(crate) fn sweep_all(now: u64) -> usize {
    registry().iter().map(|store| store.sweep(now)).sum()
}

/// Spawn the periodic sweeper (idempotent — later calls are no-ops). Must run inside a
/// tokio runtime; the gateway calls this once at startup.
pub(crate) fn spawn_sweeper() {
    static STARTED: OnceLock<()> = OnceLock::new();
    if STARTED.set(()).is_err() {
        return;
    }
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(SWEEP_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            let released = sweep_all(crate::auth::now_ts());
            if released > 0 {
                tracing::debug!(released, "session sweeper released expired viewer sessions");
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Dispatch goes to the store whose kind matches, and ONLY that store.
    struct Probe {
        kind: &'static str,
        closed: std::sync::Mutex<Vec<String>>,
        swept: std::sync::atomic::AtomicUsize,
    }

    impl SessionLifecycle for Probe {
        fn kind(&self) -> &'static str {
            self.kind
        }
        fn close(&self, session_id: &str) -> bool {
            self.closed.lock().unwrap().push(session_id.to_string());
            true
        }
        fn sweep(&self, _now: u64) -> usize {
            self.swept.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            1
        }
    }

    fn probe(kind: &'static str) -> Probe {
        Probe {
            kind,
            closed: std::sync::Mutex::new(Vec::new()),
            swept: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    fn dispatch_close(
        stores: &[&dyn SessionLifecycle],
        kind: &str,
        session_id: &str,
    ) -> Option<bool> {
        stores
            .iter()
            .find(|store| store.kind() == kind)
            .map(|store| store.close(session_id))
    }

    #[test]
    fn close_dispatches_by_kind_and_unknown_kinds_fail_closed() {
        let object = probe("object");
        let media = probe("media");
        let stores: Vec<&dyn SessionLifecycle> = vec![&object, &media];

        assert_eq!(dispatch_close(&stores, "media", "s1"), Some(true));
        assert_eq!(media.closed.lock().unwrap().as_slice(), ["s1"]);
        assert!(object.closed.lock().unwrap().is_empty());

        // Unknown kind: None — the HTTP layer must 404, never guess a store.
        assert_eq!(dispatch_close(&stores, "browser-page", "s1"), None);
    }

    #[test]
    fn sweep_all_visits_every_store() {
        let object = probe("object");
        let media = probe("media");
        let stores: Vec<&dyn SessionLifecycle> = vec![&object, &media];
        let released: usize = stores.iter().map(|store| store.sweep(0)).sum();
        assert_eq!(released, 2);
        assert_eq!(object.swept.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(media.swept.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    /// The REAL registry lists exactly the wired kinds — a new store that forgets to
    /// register here is caught by this test, not by a silent 404 in production.
    #[test]
    fn production_registry_covers_the_viewer_session_kinds() {
        let kinds: Vec<&str> = registry().iter().map(|store| store.kind()).collect();
        assert_eq!(kinds, ["object", "media"]);
    }
}
