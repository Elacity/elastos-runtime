# Elastos I/O Kit — Capability-Gated request→response|stream Interop for External Apps

Date: 2026-07-14 (updated 2026-07-15: reshaped — canonical stream contract is primary, this
kit is demoted to a use case; all-party trade-offs in §11)
Status: **RESHAPED — superseded as a standalone design.** Decision (2026-07-15, PO + product
owner concurring): the primary artifact is a **canonical Runtime stream/session contract**
(one contract with the existing capability, principal, audit, cancellation, and lifecycle
model — its own spec, to be designed with the parallel work). External interop — this kit and
its subsidiaries (WS front door, reference client, the render use case) — is **a use case of
that contract**: a thin edge adapter, never a second authority system. §0.5 below seeds the
canonical contract's requirements; §§2–9 are retained as **input material** for it, not
implementation guidance; §11 records the settled/open trade-offs.
Sub-project 2 of the dKMS improvement program. Context:
`docs/superpower/specs/dkms-improvements.md`, and the t-of-n-custody spec.

## 0. Where this sits

Program ordering: (0) bugs; (1) t-of-n pool custody [draft — custody ADR pending]; (2) the
**canonical Runtime stream/session contract** [spec to be written]; **(3) external interop as
its first use case** ← what remains of this spec. The process manager sub-project is
**postponed until ESP/System lands** (`2026-07-14-process-manager-design.md`).

The original "independence is a hard requirement" stance (own branch, ships against today's
runtime) survives only for the **unary** half — request/response ops over the existing
capability path (§11.3). For streaming it was illusory: the substrate buffers, and Component
Bus v1 intentionally has no streaming, so the canonical contract must exist before any
external stream does. The kit still never touches dKMS custody internals — that decoupling
holds under any shape.

## 0.5 Requirements seed for the canonical Runtime stream/session contract

Collected from all feedback rounds so the future spec starts complete. The contract must:

1. **Reuse, not duplicate:** the existing Runtime **capability, principal, audit,
   cancellation, and lifecycle** model — one authority path; edge adapters and internal
   consumers get the *same* contract (PO: "one canonical Runtime stream/session contract…
   external interop can then be a thin edge adapter over that contract").
2. **True producer streams:** backpressured, cancelable, sequence-ordered — replacing today's
   buffered `transfer:"stream"` mode; a stalled consumer fails closed, never unbounded growth.
3. **Session binding:** streams bound to an authenticated session/principal; opening a stream
   is a distinct gated act, not implied by a request channel.
4. **Positioning:** Runtime-owned; **not** part of Component Bus v1 (which intentionally
   excludes streaming and resident lifecycle) and **not** an external RPC protocol — edge
   adapters (localhost WS for browsers, Unix socket for native) are projections of it.
   NOTE (epistemic): Component Bus is known to us only via PO/CTO feedback quotes — it exists
   in the parallel-work branch, not in this tree. Planning must first obtain its v1 spec or
   branch pointer; that input decides how thin the edge adapter can be.
5. **Grant scope:** narrow, resource-bound grants (kid / kid-set — the Tier A Merkle primitive
   from the t-of-n spec §6); no broad "owned-assets" authority.
6. **Op taxonomy per the caching doctrine:** uncredentialed cacheable reads (public signed
   metadata, encrypted published bytes — no per-read prompts) vs capability-gated everything
   else (mutation, repair, private metadata, rights, keys, decrypt).
7. **Honest egress classes:** an op that emits decoded frames emits **decrypted content** —
   its policy floor (forensic watermark, rate limit, narrow grant, explicit consent) is part
   of the contract, not adapter goodwill.

---

**Everything below this line is retained as INPUT MATERIAL for the canonical-contract spec
and its edge-adapter use case — the shapes (kit core, own capability service, generic
target/op front door) are superseded by the reshape decision above.** The threat table (§5),
consent flow (§3), egress discipline, and wire-envelope details remain the best-developed
thinking we have and should be mined, not rewritten from scratch.

## 1. Goal

A **general I/O interop kit**: external (non-runtime) apps issue a request and receive either a
**unary response** or a **piecewise stream** from relevant runtime components, through one
capability-gated front door. "Recover CEK + decode/render an owned asset to frames over a socket"
is the **first use case**, not the whole kit — it is a thin consumer that adds only a capability
scope and rides the generic machinery.

Hard invariants (security wins over UX):
- External apps **never** touch the CEK, ciphertext, or a plaintext file, and never supply a
  decoder. Decrypt + decode happen **in-boundary**; only decoded / render-IR bytes egress.
- **Capability ≠ rights** — a capability gates *whether an app may ask*; the runtime's existing
  content-rights + dKMS authorization still runs underneath, unchanged. The kit adds no bypass.
- **No ambient authority** — everything an app holds is explicitly owner-consented and named in a
  scoped, signed, expiring grant.
- Fail closed everywhere.

## 2. Architecture

Three server-side units + one wire protocol + one reference client. Every collision-prone edge is
a **port** (Ports & Adapters), so nothing couples at compile time to the parallel work or to dKMS.

1. **I/O Kit core** (transport-agnostic) — accepts a validated `Invocation`, enforces the presented
   capability against the external profile, and dispatches via a **`ProviderDispatch` port**
   (default adapter → the existing `ProviderRegistry` / `elastos.provider.invocation/v1` plane).
   Returns a **Response (unary)** or opens a **Stream (piecewise)** — the request→response|stream
   duality, mapped onto the plane's existing `json|bytes` vs `stream` transfer modes. Knows nothing
   about WebSockets, dKMS, or decoding.
2. **WS front-door adapter** (Transport adapter #1, behind a `Transport` port) — a localhost
   WebSocket server. Terminates connections, checks **Origin**, frames control + stream messages.
   The only unit that touches WebSocket. A Unix-socket adapter is a later drop-in behind the port.
3. **Capability / consent service** — issues owner-consented, scoped, expiring capability grants and
   verifies them per-invocation. The trust root is the human owner.

Wire + client:
4. **The I/O protocol** — a versioned envelope (`hello`, `invoke`, `response`,
   `stream_open`/binary frames/`stream_end`, `cancel`, `error`). The protocol — not the reference
   client — is the contract.
5. **Reference client** (TS/JS) — a thin library: `invoke(request) → Response | AsyncStream`. One
   reference; others implement the protocol.

Data flow:
```text
app ─WS frame─▶ [WS adapter: origin check, framing]
             ─▶ [Kit core: verify capability → external-profile check → dispatch via ProviderDispatch port]
                 ├─▶ ProviderRegistry today  ─┐
                 │                             ├─ unary → Response (one frame)
                 └─ or canonical dispatch later┘   or → Stream (many binary frames, gated)
```

Reused: the `elastos.provider.invocation/v1` envelope + its `transfer` modes + the capability-string
shape. New: the external front door (WS adapter), the external-caller capability profile + consent
flow, the transport-agnostic core, and the reference client.

## 3. Capability & consent model (the security heart)

### Two independent authorization layers

A capability answers **"may this app ask?"** — never **"does the owner hold rights?"** The runtime's
rights check (on-chain `hasAccessByContentId`, dKMS authorization) **still runs underneath,
unchanged**. So an app with a valid `decrypt:render` capability still only opens assets the **owner**
owns. A stolen capability grants nothing the owner couldn't already do, and nothing for assets the
owner doesn't hold. The capability is stacked on top of — never a substitute for — the content
rights gate.

### The external capability profile (a hard subset; `key` is never reachable)

External callers get a strictly smaller allowlist than internal peers, expressed as **data**
(allowlist + per-op egress rule), so `key` can never be added by accident — the kit refuses any op
not in the external profile *before* dispatch.

| target:op | egress | why safe |
|---|---|---|
| `decrypt:render` | decoded frames / render-IR **only** | decode-in-boundary; no CEK, plaintext file, or ciphertext leaves |
| `rights:check` | boolean / status | no content |
| `availability:check` | boolean / metadata | no content |
| **`key:*`** | **FORBIDDEN** | would egress key material — never externally invocable, by construction |

(Future safe ops are added as profile entries, not code. `content:fetch`, `drm:license` are
deliberately excluded from the MVP — YAGNI + they widen egress.)

### The scoped grant

Binds: **app identity + origin**, the allowed `(target,op)` set (⊆ external profile), a **resource
constraint** (e.g. "owned assets" or a specific KID set), an **expiry**, and a **revocation handle**.
Signed by the runtime (the owner's authority). Bound to app+origin so a lifted token is inert
elsewhere; expiring so exposure is bounded; revocable so the owner can cut an app off immediately.

### Consent flow

```text
app:     request_grant({ scopes:["decrypt:render"], resource:"owned-assets" })
runtime: show the OWNER a consent prompt — "App X (origin Y) wants: render assets you own. Approve?"
owner:   approve → runtime mints a signed, scoped, expiring capability bound to (app, origin, scopes, resource, exp)
app:     presents the capability on hello; kit verifies (sig, exp, revocation, scope covers (target,op,resource))
```

The owner is always the human root.

### App-identity binding (closes the native-client gap)

- **Browser apps** → the WS `Origin` (each app has a distinct origin per the parallel per-origin move).
- **Native apps** → an **app keypair**; the grant is bound to the app's public key and `hello` carries
  a possession signature. A lifted token without the app key is inert.

### Egress discipline

Each allowed op declares what may leave; the kit enforces the op is in-profile, the *provider*
enforces the egress shape (`decrypt-provider` already emits only decoded output, never the CEK).
Decoded content egress still carries the runtime's **forensic watermark** and is **rate-limited** —
the threat for a render op is content-capture, not key-theft.

## 4. Wire protocol

Versioned, message-oriented, over the WS channel. **Control messages are JSON text frames; stream
data are binary frames** (decoded video / render-IR is not base64-bloated). Every message carries
`protocol_version`.

**Capability (once per session, after out-of-band consent):**
```json
→ { "t":"hello", "protocol_version":1, "capability":"<signed grant token>", "app_proof":"<sig, native only>" }
← { "t":"welcome", "session":"…" }        // or { "t":"error", code, message } → close
```

**Unary invoke → response:**
```json
→ { "t":"invoke", "id":"req-1", "target":"rights", "op":"check", "transfer":"json", "payload":{ "kid":"0x…" } }
← { "t":"response", "id":"req-1", "ok":true, "data":{ "held":true } }
```

**Streaming invoke → stream:**
```json
→ { "t":"invoke", "id":"req-2", "target":"decrypt", "op":"render", "transfer":"stream", "payload":{ "asset":"…" } }
← { "t":"stream_open", "id":"req-2", "meta":{ "encoding":"render-ir/v1", "frames":300 } }
← <binary frame: [id-len | id | seq(u32)] ‖ bytes>       // many, ordered
← { "t":"stream_end", "id":"req-2", "status":"complete" } // or "error" / "canceled"
→ { "t":"cancel", "id":"req-2" }                          // client may cancel anytime
```

**Errors (typed, coarse — never leak internal state):**
```json
← { "t":"error", "id":"req-2",
    "code":"capability_denied|not_in_profile|rights_denied|not_found|rate_limited|protocol", "message":"…" }
```

### Mapping down

The kit core translates an `invoke` into an internal provider invocation and dispatches via the
`ProviderDispatch` port, reusing the `elastos.provider.invocation/v1` shape (schema, source = app
identity, target, op, `capability: provider:{app}->{target}:{op}`, transfer). The `transfer` field is
the same `json|bytes|stream` the plane already speaks. Local WS transport replaces the carrier
`connect_ticket` route; the port hides the difference.

### Streaming discipline (aligned with the PTY/stream contract)

- Opening a stream is a **distinct capability/launch-token-gated act**, not implied by the request
  channel (matches "PTY streaming is a separate launch-token-gated Runtime contract").
- **Ordered, sequence-numbered binary frames**; client may `cancel` anytime → provider stream torn
  down, boundary state zeroized.
- **Backpressure**: bounded in-flight buffering; a stalled consumer past the limit ends the stream
  `error` (fail closed), never unbounded growth.

## 5. Security model & error handling

| Threat | Gate |
|---|---|
| Random web page / DNS-rebinding connects | Origin allowlist at the WS adapter + capability required before any invoke |
| Native local process connects (no Origin) | capability must prove app-identity possession (app keypair); no valid grant → no access |
| Capability token lifted | bound to app identity + origin, short TTL, revocable; rights still enforced underneath |
| App tries to reach `key` / escalate | op not in external profile → refused before dispatch (`not_in_profile`) |
| CEK / ciphertext / plaintext-file egress | only `*:render`-class ops reachable; provider decodes in-boundary; per-op egress rule |
| Content capture of decoded frames | inherent to a render use case → forensic watermark (attribution) + rate-limit + owner consent + owner must hold rights |
| Replay | per-invocation `id`; capability `exp`; streams single-use |
| Confused deputy | capability names exact `(target, op, resource)`; matched before dispatch |

### The invariant that makes external exposure safe

**The kit adds no bypass.** Every existing protection on the open path runs in full underneath —
rights check, dKMS recover authorization, decode-in-boundary, watermark. The kit only decides **who
may ask**; it can never make the runtime do something the owner couldn't already authorize.

Floor: (1) no CEK, ciphertext, or plaintext-file ever egresses — only decoded / render-IR; the app
never supplies a decoder. (2) Capability ≠ rights — both enforced. (3) No ambient authority. (4)
Decoupled from dKMS internals. (5) Fail closed everywhere.

### Error handling

- Bad/absent/expired capability at `hello` → typed `error`, **close** (no partial session).
- Op not in profile / capability doesn't cover `(target,op,resource)` → `not_in_profile` /
  `capability_denied`, no dispatch.
- Underlying rights denial → `rights_denied` (distinct from capability denial, so the app can prompt
  the owner to acquire access).
- Per-grant rate limits / quotas; exceeded → `rate_limited`.
- Stream consumer stalls past the buffer bound → stream ends `error`, boundary torn down + zeroized.
- All error messages are coarse — name the gate that fired, never internal state.

## 6. Decoupling & parallel-work alignment

- **Own branch off `feature/cenc-core-decouple`**, parallel to spec #2. The kit references **no**
  dKMS/pool/DHT types; a dependency check in CI enforces the absence of that edge.
- **`ProviderDispatch` port** is the single seam to the runtime (default adapter → `ProviderRegistry`).
  Canonical provider dispatch, when it lands, is one new adapter; the core is untouched.
- **`Transport` port** — WS adapter #1; Unix-socket adapter later.
- **No ambient authority** (capability tokens only); **streaming is a separate gated contract**
  (PTY-contract alignment); **per-origin** authorization (app-origin migration alignment); **no
  compile-time coupling** to Component Bus / their dispatch / PTY.

## 7. Component boundaries (SOLID; slices the plan)

`io-kit-core` (dispatch + capability enforcement, pure) · `ws-front-door` (Transport adapter) ·
`capability-service` (consent + mint + verify) · `external-profile` (allowlist + egress rules, as
data) · `provider-dispatch` adapter (→ ProviderRegistry) · `reference-client` (TS/JS). Each testable
in isolation.

## 8. Testing

- **Core (fakes):** `FakeDispatch` drives unary + stream; assert capability enforcement,
  `not_in_profile` refusal (esp. **`key` always refused**), egress-rule enforcement,
  cancel/backpressure teardown.
- **Capability service:** consent → mint → verify; expiry, revocation, app-identity/origin binding,
  lifted-token-is-inert.
- **Security pins:** `key:*` can never be invoked externally; a decode stream emits only render-IR;
  rights denial surfaces as `rights_denied` (never bypassed).
- **WS adapter integration:** origin allowlist, binary-frame framing, hello→welcome, a full unary
  and a full stream round-trip against a fake dispatch.
- **Reference client:** `invoke → Response | AsyncStream` against the real WS adapter.
- **Decoupling pin:** no dependency edge from the kit to dKMS/pool/DHT code (build/dependency check).

## 9. First use case, end to end (proves the kit)

```text
external app: hello(capability: decrypt:render on owned-assets)
   → invoke({ target:"decrypt", op:"render", asset }, transfer:"stream")
runtime: rights check (owner holds it?) → dKMS recover (however it works today) → decode IN-BOUNDARY
   → stream_open → binary render-IR frames → stream_end
app: renders frames. Never saw the CEK, the ciphertext, or supplied a decoder.
```

The CEK-recover+decode case is a **thin consumer** — it adds only the `decrypt:render` profile entry
and rides the generic kit. Any future safe use case is another profile entry, no new machinery.

## 10. Out of scope (explicit)

- Multi-language SDKs — one TS/JS reference client; the protocol is the contract.
- Unix-socket / other transports — a later adapter behind the `Transport` port.
- `content:fetch`, `drm:license`, and any op that widens egress beyond decoded/status — future
  profile entries, deliberately excluded from the MVP.
- Remote/networked exposure — the front door is local (WS on localhost); no WAN surface.
- Any change to dKMS custody — the kit is strictly a consumer of the provider abstraction.

## 11. Open trade-offs to settle at planning (all parties' positions, recorded 2026-07-15)

Not decided here. The discussion map for the planning session.

### 11.1 Front-door shape — generic `(target,op,payload)` proxy vs named domain verbs

**Review feedback (PO).** A generic WebSocket target/op/payload front door plus a separate
capability service would **expose the internal provider plane** and **create a second authority
model**.
**CTO ruling.** Component Bus is the internal capsule ABI, NOT the external RPC protocol. A
common request/response/stream protocol is agreed **provided** it is an edge adapter into the
existing Runtime session/capability/routing/audit path.
**Our analysis.** The tell is this spec's own `key:* FORBIDDEN` row — needing it means the
default posture is exposure, opted out per-op. An external contract should name domain
operations (the way `content/*` does for capsules) rather than mirror internal dispatch tuples;
every internal provider must not be "one profile entry away" from external reach.
**Settled:** edge adapter into the existing Runtime path (all parties), and — per the
2026-07-15 reshape — the verb taxonomy and adapter thinness are now questions *for the
canonical stream/session contract spec* (§0.5), not for this kit.
Note the localhost WS transport itself is not contested — browsers cannot speak Unix sockets;
the objection is to WS fronting a generic plane proxy.

### 11.2 Capability model — own consent/grant service vs reuse of existing primitives

**Review feedback.** The separate capability service is half of the "second authority model"
objection (§11.1).
**Our analysis.** The kit's grant object would be the *third* authority artifact in the tree
(Runtime capabilities, dKMS `AccessGrantV1`, kit grants). The kid-set Merkle scope from the
scoped-delegation v2 design (t-of-n spec §6) is a ready-made narrow-grant primitive — one
consent names an enumerable set.
**Open:** whether the kit's grant IS the same primitive as the dKMS scoped delegation (one
scoping story platform-wide) or a Runtime-capability extension; who mints/verifies; where
revocation lives. **Settled:** no new standalone authority service.

### 11.3 Streaming — substrate reality and sequencing — **SETTLED 2026-07-15**

**Review feedback.** Current provider stream support is **buffered**, not a true producer
stream; Component Bus v1 **intentionally** leaves streaming (and resident lifecycle) out.
Define one canonical Runtime stream/session contract first (capability, principal, audit,
cancellation, lifecycle); external interop is then a thin edge adapter over it.
**Our analysis.** §4's backpressure/cancel/sequenced-frames discipline assumed a substrate
capability that does not exist; inventing the streaming contract at the edge would let the
external bridge de facto define the runtime's streaming model from the outside in. Concession:
the kit's "independence" hard requirement was already illusory for the streaming half.
**SETTLED (product owner concurring with PO):** the canonical Runtime stream/session contract
comes first, as its own spec (§0.5 seeds its requirements); the interop kit and its
subsidiaries are use cases of it — thin edge adapters, nothing more. **Residual open (minor,
product call):** whether a render-less unary v0 edge (request/response ops over the existing
capability path) is worth shipping while the contract is designed, or nothing external ships
until the contract lands.

### 11.4 Grant scope — broad `owned-assets` vs narrow resource-bound

**Review feedback.** Use narrow resource-bound grants rather than a broad owned-assets grant.
**Our analysis.** §3's `resource:"owned-assets"` is exactly the Tier B "wallet-wide" scope the
delegation work deferred *with compensating controls* — this spec waved it through with none.
**Settled:** narrow, resource-bound (kid or kid-set) grants; the broad grant is out. **Open:**
whether "owned-assets-at-consent-time" (an enumerated snapshot set) is an acceptable UX middle
ground — it is bounded and Tier-A-shaped, unlike an open-ended "whatever I ever own".

### 11.5 Content-boundary honesty (settled — wording + policy floor)

**Review feedback.** Describe the boundary honestly: **decoded frames are decrypted content**,
even though the CEK and reusable decrypted files remain inside the protected boundary.
**Consequence for this spec's rewrite:** §1's invariants oversell ("never touch… a plaintext
file"); the render op deliberately egresses the content in consumable form, gated by consent.
Once stated honestly, the policy floor becomes non-negotiable rather than best-effort:
forensic watermark on every egressed frame, rate limits, narrow grants, per-set consent.

### 11.6 Op taxonomy from the caching doctrine (new input, shapes the verb design)

**CTO ruling.** Public signed metadata and encrypted published bytes remain cacheable
**without per-read prompts**; mutation, repair, private metadata, rights, keys, and decrypt
stay capability-gated.
**Consequence:** the external contract gets two op classes — uncredentialed cacheable reads
(public metadata, encrypted published bytes) and capability-gated everything else. The reshaped
verb set must be classified accordingly from day one; a per-read consent prompt on public
reads is as wrong as an ungated decrypt.
