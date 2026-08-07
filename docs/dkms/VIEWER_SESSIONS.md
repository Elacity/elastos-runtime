# Viewer sessions — open, deliver, close, sweep

How an owned protected asset reaches a viewer without the viewer ever holding a key: the
open gate, viewer selection, token delivery, the scoped read routes, and the release
lifecycle.

Grounded in `elastos/crates/elastos-server/src/api/{viewer_open,viewer_media,viewer_object,
media_authority,object_authority,session_lifecycle,session_bounds,access_grant,mod}.rs` and
the `elacity-player` / `ddrm-viewer` capsules.

---

## 1. The open gate

`POST /api/viewers/open { uri }` (`viewer_open::open_owned_in_viewer`) is the single
canonical entry. It:

1. **Resolves the object inside the principal's own root only.** A buyer can never name a
   path into another principal's space.
2. **Resolves the on-chain subject** — the signed-in principal's linked EVM account. On a
   dev build `ELASTOS_DDRM_SUBJECT` can pin one; see the caveat in [README.md](README.md) §4.
3. **Asks the chain**, through `rights-provider` → `chain-provider`
   `has_access_by_content_id(holder, bytes16 contentId)`, and gets back a signed
   `RightsDecisionReceiptV1`.
4. **Launches the authority** bound to the `content_id` and the rights-receipt hash, which
   is welded into the decrypt transcript AAD.
5. Returns `{ schema: "elastos.viewer.open/v1", viewer, session, title, play_url,
   rights_binding }`.

`POST /api/viewers/prepare-grant` is the companion for chain-mode dKMS: it prepares a
delegation the wallet signs (`personal_sign`), producing the `AccessGrantV1` the quorum
nodes verify themselves.

### The refusal is not an existence oracle

An asset the caller has not acquired and an asset that does not exist return
**byte-identical** `404 owned object not found` responses, and the body leaks neither the
owner's identity nor the on-disk layout. A token holder cannot walk the node's Library by
status code. Pinned by
`api::gateway_tests::dkms_rail::dkms_rail_open_fails_closed_before_acquisition`.

The retry path is Home-driven: a rights-denied open surfaces 403, Home offers the buy, and
the open is retried. **The buy leg is a money verb** with its own spend confirmation and
passkey step-up — see [README.md](README.md) §3 and [COMMERCE.md](COMMERCE.md).

---

## 2. Viewer selection

The sealed object's `viewer.required_interface` is the single source of truth — trust and
access travel with the signed content, so the runtime never guesses.

| `required_interface` | Viewer capsule | Session kind |
|---|---|---|
| `elastos.viewer/media@1` | `elacity-player` | `media` |
| `elastos.viewer/document@1` | `ddrm-viewer` | `object` |
| anything else | *(none)* — fails closed | — |

`viewer_media::viewer_for_required_interface` returns `None` for an unknown interface
rather than defaulting to a viewer. The marketplace and other apps never choose a viewer;
they open the `play_url` the runtime returned and render nothing themselves.

---

## 3. Launch-token delivery — the fragment rule

A launch token is a bearer credential. It rides the URL **fragment**, never the query:

```
/apps/elacity-player/?session={id}#home_token={token}
/apps/ddrm-viewer/?session={id}#home_token={token}
```

A fragment is never transmitted to a server, so the token stays out of `Referer` on every
subsequent request, out of access logs, and out of proxies — none of which is true of a
`?…&home_token=…` query pair.

- **The only builder** is `viewer_route_with_launch_token(route, token)` in
  `api/mod.rs`, which percent-encodes via `url::form_urlencoded::Serializer` and returns
  `format!("{route}#{fragment}")`. The session id stays in the query; the token is
  fragment-only.
- **Base routes:** `viewer_media::media_play_route`, `viewer_object::object_view_route`.
- **Enforced by two tests** in `api::mod::launch_token_delivery_tests`:
  `viewer_launch_routes_carry_the_token_only_in_the_fragment` (asserts nothing precedes the
  `#`, and pins the encoding), and `no_gateway_source_builds_a_url_borne_launch_token`,
  which walks every `.rs` file under `src/api` and fails the build if any of them assembles
  `?home_token=` or `&home_token=`.
- **Client side** reads it from the fragment: `capsules/ddrm-viewer/viewer.js` and
  `capsules/elacity-player/player.js` (`fragmentValue`), plus the Home and storefront
  shells.

> Two doc comments still describe the old query-string delivery
> (`viewer_media::media_play_route`, `viewer_object::object_view_route`). The code, the
> builder, and both tests are fragment-only — treat the comments as stale.

---

## 4. Scoped read routes

The runtime holds, per open session, **only** CEK-free sealed material, the clear init
segment (CENC init is unencrypted), and public metadata. It never holds the CEK and never
holds decrypted media at rest; each segment is decrypted on demand by `decrypt-provider`
(the `stream_segment` op) and proxied straight through.

**Media** (`elastos.viewer.media/v1`):

| Route | Returns |
|---|---|
| `GET /api/viewers/:viewer/media/:session` | `{ mime, segment_count, has_init, is_protected, expires_at }` — metadata only |
| `GET /api/viewers/:viewer/media/:session/init` | clear init segment bytes |
| `GET /api/viewers/:viewer/media/:session/cover` | cover art |
| `GET /api/viewers/:viewer/media/:session/segment/:index` | one decrypted media segment |
| `GET /api/viewers/:viewer/media/:session/track/:track/init` · `/segment/:index` | per-track variants |

**Non-media** (`elastos.viewer.object/v1`):

| Route | Returns |
|---|---|
| `GET /api/viewers/:viewer/object/:session` | `{ mime, byte_length, is_protected, expires_at }` — metadata only |
| `GET /api/viewers/:viewer/object/:session/bytes` | decrypted object bytes |
| `GET /api/viewers/:viewer/object/:session/page` | a rendered page |

Every read is gated by the launch token scoped to that viewer, and the token principal must
own the session. Range and expiry are enforced **before** any relay. Out-of-range, expired,
or unauthorized reads are 4xx — fail closed.

**Defense in depth:** the provider response is scanned for forbidden key fields
(`cek`, `iv`, `key`, `plaintext`, `decrypted`, `secret`, …) on these routes. Their presence
would mean key material escaped a boundary, so the runtime refuses rather than surfacing
it — on top of the boundary's own guarantee.

Only one segment's plaintext is ever in flight.

---

## 5. Release — close and sweep

`api/session_lifecycle.rs` is the one place the runtime releases sessions. Every session
store implements `SessionLifecycle` (`kind` / `close` / `sweep`) and is listed in a fixed,
explicit `registry` — a compile-time property of the gateway, auditable, not runtime
mutation. Adding a future kind is one trait impl plus one line; the routes and the sweeper
never change.

Three consumers:

1. **Explicit close routes** — `POST /api/viewers/:viewer/media/:session/close` and
   `POST /api/viewers/:viewer/object/:session/close`. Fed by the viewer capsules'
   `pagehide` beacon and the Home shell's window-close hook. Idempotent (204).
2. **The periodic sweeper** — every 60 s, so expiry no longer waits for the next store
   access on an idle machine. Per-tick work is bounded (each store holds at most
   `MAX_VIEWER_SESSIONS` entries).
3. **Each store's own lazy sweep** on lookup and admission, kept as a backstop.

Contract for implementors: `close` and `sweep` are idempotent and honor the deferred-drop
discipline — entries removed under the store lock are dropped *after* it releases, because
a drop can reap an authority subprocess with a grace period (`session_bounds`).

**Fail-closed direction:** a close can only *cost* a re-open, which re-runs the full
authorization gate. It can never grant access. Authorization for the HTTP close is the
store's own read gate (token + viewer + principal), enforced by the per-kind handler before
dispatching; `session_lifecycle` never sees credentials.

---

## 6. Provider watchdogs

The sidecars on the open/view path are bounded by the shared `capsule_watchdog`
(`ELASTOS_CHAIN_READ_DEADLINE_SECS`, default 30 s): a hung media authority, object
authority, or grant sidecar is group-killed at the deadline and the open or view is
**denied** — fail-closed, the mirror of the rights-decide rule. These sit on the open/view
paths, never on the pay spine, so a timeout here is never a money decision. Pinned by
`object_authority::a_hung_object_authority_is_killed_and_access_is_denied`,
`media_authority::a_hung_media_authority_is_killed_and_access_is_denied`, and
`access_grant::a_hung_grant_sidecar_is_killed_and_the_open_fails_closed`.

---

## 7. What the viewer never gets

Neither viewer ever receives the CEK, an IV, or raw key bytes. `elacity-player`
proactively **fails closed** if any forbidden key field appears in what it receives. The
non-media viewer's presentation lockdown (pixel-lock for image/PDF/CBZ/code; html-lock plus
a forensic watermark for reflowable EPUB) is rendering policy, not key custody.

See [MEDIA_PIPELINE.md](MEDIA_PIPELINE.md) §5 for the exact scoped-response allow-list and
the tests that pin it.

---

## 8. Local demo

```bash
cargo run --manifest-path scripts/dev/ddrm-viewer-demo/Cargo.toml -- [--video PATH] [--port 8099]
```

Transcodes a video to fMP4, CENC-packs it under a fresh CEK, launches a real
`decrypt-provider` (features `rail-stream,rail-mint`), seals the CEK to the boundary's
in-VM session key, and serves the real `elacity-player` capsule plus the scoped media
routes — so the video plays in the browser with the CEK and IV never crossing. Needs
`ffmpeg`.
