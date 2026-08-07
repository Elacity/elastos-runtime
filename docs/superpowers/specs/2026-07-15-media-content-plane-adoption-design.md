# dDRM Media on the Content/Availability Plane — P1 Fix as Contract Adoption

Date: 2026-07-15
Status: proposed design, pre-implementation
Fixes the P1 availability bug (`dkms-brainstorming.md` §11.1) by adopting the existing
`elastos://content/*` plane on the media mint/open spine, per the 2026-07-15 review feedback.

## 0. Where this sits

This spec absorbs the program's "P1 IPFS pin/provide" bug item. It does NOT introduce a new
subsystem: the architecture the review prescribes **already exists in-tree** —

- All six capsule verbs routed: `elastos://content/{publish,fetch,status,ensure,repair,unpublish}`
  ([provider_resource.rs:322](../../../elastos/crates/elastos-server/src/provider_resource.rs)).
- Implementation: `ContentProvider` ([content.rs](../../../elastos/crates/elastos-server/src/content.rs),
  landed 2026-06-08) — signed availability receipts (`elastos.content.availability.receipt/v1`),
  repair tasks/workers/fleets, storage quotas, admission + abuse controls, Carrier peer attestation.
- Doctrine already stated for the library: "fetch keylessly via the `content/*` plane (P4 — never
  raw ipfs), pin via `content/ensure`" ([library.rs:1999](../../../elastos/crates/elastos-server/src/library.rs)).

**The problem is a bypass, not a missing feature:** the dDRM media spine ignores this plane.
`creator.rs` publishes DASH directories over the raw `ipfs-provider` rail (no durable pin, no
provide/announce, no receipt), and the media open path fetches via `ipfs-provider` `ls`/`cat`
with the dead-end `_files.json` fallback. P1 is a symptom of the bypass.

**Relationship to parallel/ESP work:** unlike the postponed process manager (which would have
created a second lifecycle truth), this work *adopts* the single existing truth. It consumes the
plane's public versioned verbs and receipt schemas; it does not modify `ContentProvider`
internals. No dependency on ESP/System landing. The one coordination point is timing/consent
with the plane's owners — a conversation, not a technical blocker.

## 1. The layering contract (normative, from the review)

```text
capsule
  -> Component Bus + Runtime capability
  -> elastos://content/* provider
       -> elastos://ipfs/* backend for local CID/add/pin/cat
       -> availability provider for replication policy
            -> Carrier for peer discovery and remote provider transport
  -> signed availability receipt
```

| Layer | Owns | Never does |
|---|---|---|
| Runtime | principal/session binding, capability validation, dispatch, quotas, audit | content semantics |
| Content provider | publish / fetch / status / ensure / repair / unpublish semantics; signed receipts | expose Kubo/gateway/Carrier authority upward |
| Availability provider | replication policy + proof; implementations may use Carrier, Elacity, supernodes, or another storage network | leak peer/transport details upward |
| Carrier | authenticated peer discovery, coordination, transport between providers | content policy |
| IPFS provider | low-level **system-only** local block/CID backend | serve capsules directly |
| Capsule | requests an operation on an object | choose peers, ports, Kubo APIs, gateways, or Carrier tickets |

Library, players, and marketplace all consume the same `content/*` contract without receiving
raw Carrier, IPFS, gateway, or provider authority. The media spine becomes one more consumer.

## 2. The P1 defects, restated against this layering

From `dkms-brainstorming.md` §11.1 (reproduced, root-caused 2026-07-13):

- **(a) Misleading diagnostics:** `ipfs-provider::ls` falls back to `<cid>/_files.json`, which no
  media/DASH directory ever contains → every availability failure surfaces as a bogus 404.
- **(b) The substantive bug:** mint does not durably pin + provide (announce) the published DAG
  on the local node — re-opening your own asset depends on re-fetching it from strangers.
- **(c) Fail-hard posture:** "No HTTP fallback is allowed" (correct sovereignty stance) converts
  every local-retrieval gap into a hard failure.

Against the layering: (b) exists because mint bypasses `content/publish` (which pins and
returns a receipt); (c) is survivable only when an availability provider — not a public
gateway — is the safety net; (a) is a defect inside the system-only backend and is fixed there.

## 3. Design — three independent slices

### Slice 1 — backend diagnostics fix (immediate; no dependencies)

In `capsules/ipfs-provider`: drop (or correct) the `_files.json` fallback for media/DASH
directories; make `ls_failed` name the actual cause — "CID not retrievable locally: no local
pin, no provider found within N s" — never a 404 on a file that never exists. Pure bug fix on
the system-only backend; ships regardless of everything else.

### Slice 2 — mint publishes through the plane (closes P1(b))

`creator.rs` mint paths (DASH directory, metadata, thumbnails/previews) publish via
**`content/publish`** instead of raw `ipfs-provider` add:

- The plane pins the DAG durably on the local node and announces (provides) it.
- The returned **signed availability receipt** is persisted with the asset record — publication
  becomes a verifiable artifact, not an assumption.
- **Fail-closed mint:** no receipt → the mint fails with a typed error; an asset is never
  minted "published" without proof (mirrors the shard-push durability rule in the t-of-n spec).
- Replication beyond the local node is the availability provider's policy (Carrier-backed
  remote providers, supernodes) — the mint spine does not choose or know.

### Slice 3 — open fetches through the plane

The media open path (DASH fetch) moves from `ipfs-provider` `ls`/`cat` to **`content/fetch`**:

- First-open-elsewhere (cold CID) gets a bounded, explicit retry/backoff policy inside the
  plane — distinct from the "we minted it, it must be local" path, which after Slice 2 is
  always warm.
- The viewer side may **`content/ensure`** (pin-forward) when policy allows — the existing
  cluster pin-forward intent, expressed through the contract.
- The no-public-HTTP-gateway posture is preserved verbatim; the availability provider is what
  makes it tenable.
- `content/status` + `content/repair` give the operator story for degraded assets (the repair
  fleet machinery already exists in the plane).

## 4. What this deliberately does NOT do

- No new provider plane, socket, or protocol — consume `content/*` verbs + receipt schemas only.
- No modification of `ContentProvider` internals; if a needed semantic is missing, it is
  requested from the plane's owners, not bolted on from outside.
- No raw Kubo/gateway/Carrier authority handed to capsules — players/marketplace/library stay
  (or become) consumers of the same contract.
- No public-HTTP-gateway fallback, silent or otherwise.
- No coupling to ESP/System timing — adoption consumes today's in-tree contract.

## 5. Error handling (fail-closed, honestly attributed)

- `content/publish` failure at mint → typed error, mint refused (no phantom-published assets).
- `content/fetch` miss → typed cause from the plane (`not_pinned_locally`,
  `no_provider_within_bound`, `quota_exceeded`, …) — never a fabricated `_files.json` 404.
- Receipt verification failure at read-back → surfaced as an integrity error, not availability.
- Capability/principal rejection at the Runtime → refused before any provider work (audit-logged).

## 6. Testing

- **Regression pin for the misattribution:** a failing media-dir `ls` names the real cause;
  `_files.json` never appears in a media-path error again.
- **Mint durability:** mint → receipt persisted → restart local Kubo → open succeeds with zero
  remote fetches (the minter always serves its own asset).
- **Cold fetch:** second node without the pin opens via the plane within the bounded policy;
  exhausting the bound fails closed with the typed cause.
- **Fail-closed mint:** plane down / publish refused → mint fails; no asset record without a
  receipt.
- **Contract conformance:** the media spine issues only `content/*` verbs — a grep-level pin
  that `creator.rs` / the media open path no longer reference the raw ipfs rail.

## 7. Ordering & effort

Slice 1 is a small immediate patch. Slice 2 and 3 are independent of each other and of the
t-of-n custody bump (they touch the data path, not the key path; §11.0 vs §11.1 of the
brainstorming doc are independent failure modes — both must land for opens to be reliable).
No geo-node redeploy is involved anywhere in this spec.

## 8. Out of scope

- Replication/repair policy design (how many replicas, placement, settlement) — owned by the
  availability provider; this spec only consumes its receipts.
- Migrating already-published assets' pins — a backfill `content/ensure` sweep is a follow-up
  operational task, not part of the code change.
- The dKMS key path (t-of-n spec) and external interop (I/O-kit spec).

## 9. 2026-07-15 PO/CTO rulings applied here + remaining open points

**Settled — proceed now (explicit directive).** "ESP/System should not put the bug fixes on
hold. The CEK commitment, content pin/provide, diagnostics, and caller-seed exposure should be
fixed independently now." All three slices of this spec are therefore unblocked without further
coordination; the "consent with the plane's owners" note in §0 is reduced to a courtesy
heads-up, not a gate.

**Caching doctrine (CTO) — shapes slice 3.** Public signed metadata and encrypted published
bytes remain **cacheable without per-read prompts**; mutation, repair, private metadata,
rights, keys, and decrypt stay capability-gated. Applied to the verbs used here:

- `content/fetch` of published *encrypted* media bytes + public metadata → the cacheable
  class: no per-read consent prompt on the open path (the capability gate is the runtime
  principal check, not a user interaction; rights/keys are gated later, on the key path).
- `content/publish`, `content/ensure` (pin-forward is a mutation of pin state),
  `content/repair`, `content/unpublish` → capability-gated, audited.

**Open points to settle at planning (small):**
- Whether `content/status` on a public CID is cacheable-class (leaning yes — it reveals
  availability of already-public bytes) or gated (if it exposes placement/replica detail).
- Receipt persistence location for slice 2 (asset record vs alongside published metadata) —
  whichever the plane's owners prefer; both satisfy the fail-closed-mint rule.
- The backfill `content/ensure` sweep for already-published assets (out of scope above):
  when it runs and under whose quota.
- For the availability provider's replication policy (out of scope here, noted 2026-07-16):
  erasure coding / information dispersal (Rabin IDA) over the encrypted payload is a
  space-efficient alternative to full n× replication (~1.5× storage for comparable fault
  tolerance) — a policy option inside the provider, invisible to the `content/*` contract.

## 10. Later improvement — object-rail `.ddrm` storage inflation (ELACITY-2294)

Outline only (analyzed 2026-07-22); the spec will be defined when ELACITY-2294 is picked up.

**Issue.** Object-rail assets (images, docs, audio-as-file, 3D) are stored in the user
workspace base64-encoded **twice**, inside two nested JSON envelopes, inflating them ~1.78×
on disk (observed: a 13 MB image → 25 MB `.ddrm`):

1. **Capsule layer:** mint embeds the CENC ciphertext inline as `ciphertext_b64` in the
   pretty-printed `elastos.ddrm.capsule/v1` JSON
   ([creator.rs:3054](../../../elastos/crates/elastos-server/src/api/creator.rs)). +33%.
2. **At-rest layer:** the library write then AES-256-GCM-encrypts the *entire capsule JSON*
   and base64url-encodes it again into the `elastos.principal-root.object/v1` envelope
   (`write_principal_root_object`,
   [auth.rs:600](../../../elastos/crates/elastos-server/src/auth.rs)). +33% on top.

The media rail does not have this problem — its `.ddrm` is a ~40 KB manifest and the
encrypted DASH segments live behind `asset_cid` — which is also why this item belongs with
this spec: the fix direction is the same contract adoption.

**Secondary costs, same root:** the pipeline is whole-file-in-memory with no streaming
(upload `file_b64` → decoded → re-base64'd into the encrypt-capsule stdin JSON → decoded →
re-encoded into the `.ddrm`; the read path decodes both layers back), holding ~3–4 concurrent
copies and paying four base64 round-trips per lifecycle. The inline ciphertext is already
known to strain the key-provider recover frame
([key-provider/main.rs:1661](../../../capsules/key-provider/src/main.rs)).

**Direction (to be specced in the ticket):** store the object rail the way the media rail
already works — ciphertext out of the JSON (binary sidecar, or simply behind the already
published `asset_cid` via `content/fetch`), keeping the `.ddrm` a small manifest; and make
the at-rest layer binary-in/binary-out (nonce + header prefix instead of a JSON/base64
envelope) or skip re-wrapping already-DRM-sealed payloads. Expected effect: ~25 MB → ~13 MB
on disk and removal of the double decode from the open path.
