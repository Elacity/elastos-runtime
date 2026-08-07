# Session Lifecycle Foundation — design

Date: 2026-07-22
Status: approved, implemented in the same commit

## Problem

Opening a protected asset is a uniform pipeline: authority spawn → session-store
admission → read routes. Closing was not wired at all. Both viewer session stores
(`viewer_object`, `viewer_media`) had a `remove_*_session` function with **zero
callers**, so closing a viewer window left the session — and the authority
subprocess chain it pins (`ddrm-media-authority --quorum` → `decrypt-provider` +
`key-provider`) — alive for the full TTL (default 3600s). Expiry itself was lazy:
released only on the next store access, so an idle machine kept expired
subprocesses indefinitely.

## Decisions (user-approved)

1. **Scope**: viewer sessions now; foundation open to future kinds (browser
   pages, exit streams) without changes to routes or sweeper.
2. **Close signals**: viewer capsule beacon on `pagehide` AND Home shell hook on
   window close. TTL remains the backstop.
3. **Idle expiry**: a registry-driven periodic sweeper (60s) in addition to the
   existing lazy sweep.
4. Approach A (trait + registry) over a generic-store rewrite (B) or an event
   bus (C): smallest change that is genuinely a foundation; the
   council-ratcheted `session_bounds` internals stay untouched.

## Architecture

### Foundation — `api/session_lifecycle.rs`

```rust
pub(crate) trait SessionLifecycle: Send + Sync {
    fn kind(&self) -> &'static str;            // route segment: "object" | "media"
    fn close(&self, session_id: &str) -> bool; // idempotent explicit release
    fn sweep(&self, now: u64) -> usize;        // periodic-tick hook
}
```

- Fixed, explicit registry (`registry()`) instead of a dynamic `register()`:
  the set of session kinds is a compile-time property of the gateway; an
  auditable list beats runtime mutation. Adding a kind = one trait impl + one
  registry line (OCP); routes and sweeper depend only on the trait (DIP).
- `close(kind, id) -> Option<bool>`: `None` = unknown kind, caller fails closed.
- `spawn_sweeper()`: idempotent; 60s `tokio::interval` (missed-tick Delay);
  calls `sweep_all(now)`; spawned in `start_gateway_server`.
- Implementors keep the deferred-drop contract: entries removed under the store
  lock are dropped after it releases (a drop may reap a subprocess, ~1s grace).

### Store adapters

- `remove_object_session` / `remove_media_session` now return `bool`
  (was `()`; they had no production callers).
- New `sweep_object_sessions(now)` / `sweep_media_sessions(now)`: the same
  `session_bounds::sweep_expired` the lazy path runs, callable on the clock.
- `OBJECT_SESSION_LIFECYCLE` / `MEDIA_SESSION_LIFECYCLE` statics implement the
  trait by delegation only — no store internals changed.

### HTTP surface

`POST /api/viewers/:viewer/object/:session/close` and
`POST /api/viewers/:viewer/media/:session/close` (literal kind segments,
matching the existing route family; one thin handler per kind so each keeps its
store's error shape).

Authorization is byte-for-byte the read gate (`authorize_*_session`): valid
`x-elastos-home-token` header scoped to the viewer, `session.viewer` match,
`session.principal_id` match.

Response semantics — idempotent and non-leaking:

| Outcome                                   | Status |
| ----------------------------------------- | ------ |
| closed                                     | 204    |
| already gone (swept/raced) or NOT OWNED    | 204    |
| missing/invalid token                      | 401    |
| malformed viewer                           | 400    |
| unknown kind                               | 404 (unrouted) |

"Gone" and "not yours" are deliberately indistinguishable: a token holder must
not be able to probe other principals' session ids. This refines the earlier
draft (which had 404 for wrong-principal) — distinguishability was itself the
leak. A close can only cost a re-open (full auth gate re-runs); it can never
grant access.

### Client wiring

- `capsules/ddrm-viewer/viewer.js`, `capsules/elacity-player/player.js`: on
  `pagehide`, `fetch(<close>, { method: "POST", keepalive: true, headers:
  launchHeaders() })`. `keepalive` survives iframe/tab teardown; `sendBeacon`
  was rejected because it cannot carry the token header.
- `capsules/home/browser/shell-windows.js`: `removeWindowEntries` calls
  `releaseViewerSession(entry)` per closing window. The iframe's
  `dataset.route` (`/apps/<viewer>/?session=..#home_token=..` — session in the
  query, launch token in the fragment) already carries both credentials, so the
  shell needs no session bookkeeping. Viewer→kind map:
  `ddrm-viewer → object`, `elacity-player → media`.
- Both signals may fire for one session; the endpoint is idempotent.

## Failure posture

Lost beacon → shell hook; both lost → TTL + 60s sweeper; sweeper dead → lazy
sweep on next access (kept, not replaced). Every failure mode degrades to a
slower release, never to an access grant.

## Testing

- `session_lifecycle`: dispatch by kind, unknown-kind `None` (fail closed),
  sweep visits every store, production registry lists exactly
  `["object", "media"]` (catches an unregistered future store).
- `viewer_media`: explicit close removes + second close is a no-op; periodic
  sweep hook releases expired only, leaves live untouched. (`ObjectSession`
  requires a live authority subprocess, so store semantics are pinned on the
  media store — the discipline and helper are shared.)
- All existing S42/S47 session tests unchanged.

## Future work (explicitly out of scope)

- Registering browser-engine pages / exit streams as lifecycle kinds.
- Generic `BoundedSessionStore<V>` unification (Approach B) beneath this trait.
- Per-principal session-cap fairness (noted in `session_bounds`).
