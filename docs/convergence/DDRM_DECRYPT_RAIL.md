# dDRM Decrypt Rail — design decision (open)

**Status:** **DECIDED by Anders (Day 45).** Hybrid path, ElastOS-native contract
(not Lit/Chipotle); Option A at the decrypt boundary (sealed material pushed in, no
outbound key-fetch); chain `drm → rights → key/dKMS → decrypt`; decrypt mints a
per-session key in-sandbox; CEK sealed to that key (dKMS-direct preferred,
key-provider re-seal as audited migration); keep key/decrypt providers separate;
PQ-hybrid as the root, P-256/Lit as compatibility only. `DecryptSessionRequestV1`
gains a backend-neutral `sealed_decrypt_material` envelope that **binds the full
transcript**. The recommended rail is already WIRED as a fail-closed reference
(`rail-live`), and the transcript binding is now implemented + proven (`rail-bind`,
see §Transcript binding). Remaining: fold the blessed envelope into the shared
contract (we can't push yet) + the upstream dKMS/key-provider sealing side.
**Context:** Day 3 of the dDRM convergence. The PC2 `cenc-decrypt` engine is
vendored and tested inside `decrypt-provider` (Day 1). This note records *why
end-to-end wiring is blocked on a contract/architecture decision* and lays out
the options, so we don't invent the most security-critical boundary in the
system unilaterally (see `CONVERGENCE_PLAYBOOK.md`: contract-first; ask when the
boundary is unclear; CEK containment).

## The finding

`decrypt-provider`'s public op contract (`DecryptSessionRequestV1` in
`elastos-common::protected_content`) carries **authority and intent only**:

```
schema, request_id, principal_id, session_id, object_cid, action,
viewer_interface, release_receipt: ReleaseReceiptV1, output_kind, reason, expires_at
```

It deliberately carries **no key material and no ciphertext**:
- `ReleaseReceiptV1` *attests* the key was released (status="released"); it has
  no CEK field.
- The CEK only exists as `KeyEnvelopeV1.wrapped_cek` (PQ-hybrid threshold-wrapped)
  inside `SealedObjectV1` / `KeyReleaseRequestV1` — which the **key-provider**
  handles, not the decrypt-provider.
- There is no field for the encrypted bytes/segment to decrypt.

So `open_session`/`render` correctly **fail closed** today ("not_configured")
*after* full validation. To actually decrypt, the decrypt microVM needs two
things the current contract/runtime does not yet deliver: **(a) the CEK** (in a
form only the decrypt VM can use) and **(b) the ciphertext**. How those arrive
is an architecture decision.

## Options

### Option A — Material pushed in the request (contract extension)
The runtime (after orchestrating rights→key) hands the decrypt-provider a sealed
decrypt-material bundle: the CEK **sealed to the decrypt-provider's ephemeral
in-VM public key** (so only this microVM can unwrap it), plus a content handle
or the ciphertext bytes. Decrypt VM unwraps → decrypts → zeroizes.

- **Pros:** decrypt-provider stays a *pure transform* with no outbound
  authority; CEK only ever exists unwrapped inside the VM; smallest blast radius;
  easiest to reason about and test.
- **Cons:** extends the public contract; must define the ephemeral-seal scheme
  (reuse `KeyEnvelopeV1` PQ-hybrid KEM) and content delivery.

### Option B — Provider-chain capability calls (no contract change)
On `open_session`, decrypt-provider calls downstream providers via
`carrier_invoke`: `key-provider` (with the `release_receipt`) → returns the CEK
sealed to the decrypt VM; `object-provider` → returns the ciphertext. Mirrors the
documented chain `drm/open → rights → key → decrypt`.

- **Pros:** public contract unchanged; matches the canonical chain; key release
  stays entirely in key-provider.
- **Cons:** requires the **provider-invocation rail inside the capsule**
  (carrier_invoke client) — overlaps Anders' provider-invocation transfer-rail
  work; decrypt-provider gains scoped outbound authority (must be tightly
  capability-bound); more moving parts to isolate.

## Recommendation

**Option A for the decrypt boundary itself** (decrypt-provider is a pure
transform that receives VM-sealed material), composed with **Option B's chain
upstream** (runtime/drm orchestrates rights→key, and key-provider seals the CEK
to the decrypt VM). i.e. the *chain* is provider-to-provider, but the decrypt
step receives its material rather than reaching out for it. This keeps the
highest-authority VM (decrypt) free of outbound network/capability authority,
which is the safest split and aligns with PC2 v1.3's emphasis on minimizing where
the live CEK can exist.

Either way the **CEK is sealed to the decrypt VM's ephemeral key**, unwrapped
only inside the VM, used by the cenc engine, and zeroized — never returned,
logged, or surfaced (already enforced in the vendored engine).

## Why this needs Anders

- Option B depends on the provider-invocation rail he is actively building.
- The CEK-sealing scheme is core dDRM / PC2 v1.3 security; it must match the
  key-provider side he/we will build.
- It defines whether `DecryptSessionRequestV1` grows a `material`/`sealed_cek`
  field (public contract change) or stays as-is.

## Questions for Anders

1. Should decrypt-provider **receive** VM-sealed material (Option A) or **pull**
   it via capability calls to key/object providers (Option B)?
2. Is there an existing/planned CEK-sealing-to-provider scheme (ephemeral KEM)
   we should target, so decrypt-provider and key-provider agree?
3. Does the provider-invocation rail expose an in-capsule `carrier_invoke`
   client today that a microvm provider may use, or is that still landing?

## Reference rail LANDED — `rail-live` (Day 45)

The recommended split (**Option A at the decrypt boundary**) is now **wired into
the provider dispatch** behind the `rail-live` feature — no longer just a tested
island. This is the working reference Anders can read and bless; flipping it to
the default is a one-line move once the public contract carries the material.

What landed (`capsules/decrypt-provider/src/main.rs`):
- A new op `OpenSessionLive { request, material }` that performs the single
  in-boundary operation: `rail_shim::decrypt_from_carrier(session, carrier,
  verifier)` → map `(bytes, meta)` into the existing **scoped** response.
- The CEK materializes only inside the engine (in `Zeroizing`), is zeroized
  there, and the plaintext is dropped at the boundary — the response carries
  session/output metadata only. Proven by `open_session_live_*` tests (decrypt
  through dispatch with **no CEK/plaintext leak**; tampered carrier and
  unprovisioned boundary both **fail closed**).
- Default build is **byte-identical and fully fail-closed** — `OpenSessionLive`,
  the material struct, and the session state are all `#[cfg(feature="rail-live")]`.
- Builds to `wasm32-wasip1`; pinned in the ladder gate at **57 passed**.

### Why it does NOT touch the shared contract yet (and the exact delta when it can)

To keep `elastos-common::protected_content` **byte-identical to v0.4.0** (drift
gate stays green), the VM-sealed material rides a *capsule-local* request variant,
not the shared `DecryptSessionRequestV1`. The moment Option A is blessed, the
proposed public-contract delta is precisely:

```rust
// elastos-common::protected_content — additive, Option A
pub struct DecryptSessionRequestV1 {
    // ... all existing authority/intent fields UNCHANGED ...
    /// CEK sealed to the decrypt VM's ephemeral in-VM public key
    /// (PqSealedEnvelope wire form). Only this microVM can unwrap it.
    pub sealed_cek: Vec<u8>,        // or base64 String for JSON parity
    /// The ciphertext fMP4 segment (or a content handle) to decrypt.
    pub ciphertext: Vec<u8>,        // or a ContentHandleV1
    /// Optional init segment (tenc IV defaults).
    pub init_segment: Option<Vec<u8>>,
    // profile (pq_hybrid|classical_p256) can be implied by deployment or a small enum.
}
```

When that lands, `OpenSessionLive` folds back into the normal `OpenSession`
(the body is already written) and the local variant is deleted. The VM session
secret stays VM-minted/off-wire either way (it is provisioned in the boundary,
never a request field). Q1 (dKMS-direct vs key-provider re-seal) and Q2
(ML-DSA-65 vs hybrid ECDSA+ML-DSA signature) do **not** change this shape — Q2
plugs into the `CekSealVerifier` slot the rail already uses, and both answers are
pre-proven.

## Transcript binding — LANDED (`rail-bind`, Day 46)

Anders' Day-45 decision added one hard requirement on top of Option A: the sealed
material must be a **backend-neutral `sealed_decrypt_material` envelope that binds
the full transcript** (principal, session, object CID/content hash, action, viewer
interface, output kind, expiry, release-receipt hash, decrypt-session public key,
algorithm suite, provider identity) with **nonce/replay protection, signature
verification, AEAD/AAD binding, short expiry, audit, and zeroization**. That is the
property that stops a validly-sealed CEK from being **replayed** against a different
session/object/receipt.

This is implemented and proven on our PQ-hybrid profile (feature `rail-bind`):

- A capsule-local `DecryptTranscriptV1` encodes exactly that field set into a
  domain-separated, length-prefixed AAD (`to_aad()`), with a SHA-256
  `release_receipt_hash`, the in-sandbox `decrypt_session_pub`, the suite id
  (`elastos-pq-hybrid-threshold-v0`), the provider id, and a replay `nonce`.
- The PQ-hybrid envelope binds it **two ways**: the CEK is AES-256-GCM-wrapped with
  the transcript as **AAD**, and the **ML-DSA-65 signature covers `payload ‖
  transcript`**. (`hybrid_unwrap_bound` / `seal_bound`; `aad == b""` reproduces the
  legacy envelope byte-for-byte, so every committed unbound golden is unchanged.)
- `OpenSessionBound` rebuilds the transcript from the **authenticated request + the
  boundary's own provisioned session public key** — never trusting it from the
  carrier — then opens via `decrypt_from_carrier_bound`. The CEK only materializes
  (in `Zeroizing`) after both the GCM tag and the signature accept; plaintext never
  crosses to the caller.
- Proven (`rail-bind`=60): a matching transcript decrypts with no CEK/plaintext
  leak; a **replay against a different `session_id`**, a **swapped replay nonce**,
  and a **tampered carrier** all **fail closed** (`decrypt_failed`).

What this leaves for the upstream/contract side (needs Anders/dKMS, not blocking
our boundary): (1) fold `sealed_decrypt_material` into the shared
`DecryptSessionRequestV1` (we can't push while access is suspended — exact additive
delta above, now extended with the transcript fields); (2) the **dKMS-direct
sealing** producer (or the audited key-provider re-seal migration) on the key side.
The decrypt boundary is ready for both.

### In-sandbox session-key mint + publish — LANDED (`rail-mint`, Day 47)

Anders' requirement that *"decrypt-provider creates a per-session one-time public
key inside its sandbox"* is implemented. Under `rail-mint`, `init` calls
`pq_envelope::mint_session()` (x25519 + ML-KEM-768 via `OsRng` → WASI `random_get`,
`wasm32-wasip1`-clean), holds the secret in-VM, and publishes the canonical pubkey
bytes (`session_public_bytes`) + suite in the init response as
`decrypt_session_public_key_b64`. The key authority seals to that published key
(`session_public_from_bytes`), transcript-bound, and the boundary opens it with the
minted secret — proven end to end with no injected secret and a fresh key per init.
Minting is the ONLY entropy the boundary needs; the unwrap path remains RNG-free
(separate feature axis), preserving the verify-only wasm property of the lower rungs.

### Short-expiry enforcement + scoped audit — LANDED (`rail-audit`, Day 48)

Anders' "short expiry, audit" requirement is implemented. The `OpenSessionAudited`
op takes an **injected capability clock** (`now_unix` — never an ambient read) and:
- **Enforces expiry before any crypto:** if `now_unix` is past `request.expires_at`
  or the release receipt's expiry, it fails closed with `expired` and performs no
  unwrap (the CEK never materializes for a stale grant).
- **Emits a scoped audit record on every decision** (`opened`|`denied`): schema
  `elastos.ddrm/decrypt-audit@1` carrying request_id, principal, session, object,
  action, suite, provider, decision, reason, **`transcript_hash`** (SHA-256 of the
  bound transcript) and the timestamp — and **no CEK and no plaintext**. On `opened`
  it also carries the scoped session; on `denied`, the reason only.

Proven (`rail-audit`=62): a fresh grant opens and audits `opened`; an expired grant
fails closed and audits `denied`/`expired` with no session; the audit envelope is
CEK/plaintext-free on both paths. (The shared bound-open path was refactored into
`prepare_bound_open`; `rail-bind`/`rail-mint` counts unchanged.)

### Consolidated envelope `SealedDecryptMaterialV1` — LANDED (`rail-material`, Day 49)

The carrier is now a single backend-neutral, **suite-tagged** type — the exact
drop-in shape to fold into the shared contract. The `suite` tag makes the backend a
FIELD, not a fork (dKMS-native PQ-hybrid vs P-256/Lit compat). The canonical op
`OpenSessionV1` routes by suite into the audited/expiry-enforcing bound path;
the compat suite is recognised but rejected on the (product, transcript-bound)
path, and an unknown suite fails closed (`rail-material`=65).

**Verbatim additive contract delta** (lift into `elastos-common::protected_content`
when the contract opens — `DecryptSessionRequestV1` gains exactly this field):

```rust
/// Backend-neutral, suite-tagged sealed decrypt material (Option A push-in).
/// Carries only sealed/public bytes — never a raw CEK.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SealedDecryptMaterialV1 {
    /// "elastos-pq-hybrid-threshold-v0" (product) | "p256-classical-compat" (migration).
    pub suite: String,
    /// CEK sealed to the decrypt VM's published session key (suite-specific wire form), base64.
    pub sealed_cek_b64: String,
    /// Ciphertext fMP4 segment (or a ContentHandleV1), base64.
    pub ciphertext_b64: String,
    /// Optional init segment (tenc IV defaults), base64.
    pub init_segment_b64: Option<String>,
    /// Per-release replay nonce (key-authority chosen), base64.
    pub nonce_b64: String,
    /// Object content hash binding the CEK to THIS content, base64.
    pub content_hash_b64: String,
}

// on DecryptSessionRequestV1, additive:
//   pub sealed_decrypt_material: Option<SealedDecryptMaterialV1>,
// (Option<> keeps it additive: unset == today's fail-closed not_configured path;
//  set == the live transcript-bound open. The decrypt-session public key the CEK
//  is sealed to is published by the boundary at init, never a request field.)
```

When this lands, `OpenSessionV1` becomes the body of the normal `OpenSession` and
all the `Open*Live/Bound/Audited/V1` capsule-local variants are deleted — the
binding, expiry, audit, and in-sandbox key behaviour are already proven here.

**Status of Anders' decrypt-side spec:** all four requirements are now implemented
as fail-closed references — Option A push-in (`rail-live`), full-transcript binding
(`rail-bind`), in-sandbox session key (`rail-mint`), short-expiry + audit
(`rail-audit`). What is left is genuinely upstream and not on the decrypt boundary:
(1) fold the `sealed_decrypt_material` envelope into the shared
`DecryptSessionRequestV1` (needs push access); (2) the dKMS-direct sealing producer
(or the audited key-provider re-seal migration).

## Isolation tier — wasm now, microVM as hardening (recommendation)

All providers declare `type: microvm`, but the microVM substrate (CrosvmProvider)
is Linux+KVM only and is not the live path on dev/macOS today; the capsules that
actually execute cross-platform are `wasm` (`home`, `system`) and `data`.

Recommendation for `decrypt-provider`:
- Ship as **`wasm` (`wasm32-wasip1`)** now. The cenc engine is pure compute
  (AES-128-CTR + fMP4 parsing, stdio only) — an ideal wasm workload with zero
  ambient authority. It runs on macOS today and composes cleanly with the Hybrid
  rail (decrypt step *receives* material → no outbound authority needed).
- Treat **microVM as a later max-isolation upgrade** for a Linux/KVM (or Mac VZ)
  deployment. Because the logic is pure Rust, the same capsule targets both tiers;
  only the manifest `type` and transport change.
- The fail-closed security contract and CEK containment are enforced by the
  capability model + provider boundary, not by the VM, so the tier choice does
  not weaken the security guarantees.
- First concrete task on `wasm` confirmation: verify `elastos-common`
  (`protected_content`) compiles clean to `wasm32-wasip1`.

### wasm viability — confirmed (2026-06-08)

`cargo build --target wasm32-wasip1` for `decrypt-provider` succeeds with **zero
code changes**. The full decrypt path is wasm-clean: the cenc engine (`aes`,
`ctr`, `base64`), `elastos-common::protected_content` (`serde`, `serde_json`,
`sha2`, `hex`, `thiserror`), and the provider itself all compile to
`wasm32-wasip1` (valid WebAssembly MVP module emitted). All 17 host tests remain
green. This is hard evidence that `decrypt-provider` can ship on the live wasm
substrate today; microVM remains the later max-isolation upgrade for the same
Rust source.

Reproduce:
```
cd capsules/decrypt-provider && cargo build --target wasm32-wasip1
```

### WASI-sandbox execution — confirmed (2026-06-08)

Beyond compiling, `decrypt-provider.wasm` *executes* correctly under a real WASI
host (wasmtime 45.0.1), driving its newline-delimited JSON protocol over
stdin/stdout. The fail-closed security contract holds in the sandbox:

| Input | Result |
| --- | --- |
| `status` | advertises blocked raw authority (`raw_cek`, `raw_plaintext`, …) |
| malformed op | `invalid_request` (rejected) |
| valid `open_session` | `not_configured` (fails closed until the rail lands) |
| `open_session` with `output_kind: raw_plaintext` | `invalid_request` (rejected up front) |

Reusable harness: `capsules/decrypt-provider/scripts/wasm-smoke.sh` (exit 0 on all
pass; suitable for CI). This upgrades the evidence from "compiles to wasm" to
"runs correctly and stays fail-closed in the wasm substrate."

## Chain status (`rights -> key -> decrypt`)

| Provider | Contract + validation | Fail-closed | Host tests | wasm32-wasip1 | WASI smoke |
| --- | --- | --- | --- | --- | --- |
| `drm-provider` (orchestrator) | yes (validates sealed object; declares canonical open sequence) | yes | 12 | builds | `scripts/wasm-smoke.sh` |
| `rights-provider` | yes (typed questions; wire-rejects hidden chain/wallet/key fields) | yes | 9 | builds | `scripts/wasm-smoke.sh` |
| `key-provider` | yes (+ rights-receipt binding: allowed + principal/session/object/right must match) | yes | 9 | builds | `scripts/wasm-smoke.sh` |
| `decrypt-provider` | yes (+ tested decrypt-step core seam) | yes | 17 | builds | `scripts/wasm-smoke.sh` |

`drm-provider` is the `drm/open` front door: it validates the `SealedObjectV1`,
declares the canonical sequence (`content -> rights -> key -> decrypt -> render ->
release_receipt -> audit`), and fails closed until the chain + runtime events exist.
Its `chain_seam_tests` characterize the inter-provider handoffs: a
`RightsDecisionReceiptV1` deserializes exactly into the key step's request, and a
`ReleaseReceiptV1` into the decrypt step's request — so the contracts compose
end-to-end and any shared-type drift fails loudly at test time.

The full `rights -> key -> decrypt` chain now compiles to `wasm32-wasip1`, executes
under WASI, and is fail-closed end-to-end:
- `rights-provider` answers typed access questions and fails closed
  (`not_configured`) until the dDRM/chain policy backend exists; it never exposes
  chain/wallet/contract/key authority (wire-level `deny_unknown_fields`).
- `key-provider` verifies the upstream `RightsDecisionReceiptV1` (allowed +
  principal/session/object/right must match) before any release; the CEK only ever
  appears as `key_envelope.wrapped_cek` — never raw.
- `decrypt-provider` validates the session + release receipt, holds the cenc engine
  with CEK containment + zeroization, and fails closed until the rail lands.

The **only** remaining gap to end-to-end decrypt is the CEK/ciphertext transport
rail (Hybrid chosen; questions for Anders above). Everything else is proven on the
live wasm substrate.

## What is NOT blocked (and is done/ready)

- The cenc engine is vendored, characterization-tested, and proven to contain +
  zeroize the CEK (Day 1).
- `decrypt-provider` validates the full session+receipt contract and fails closed.
- Once the rail is chosen, wiring is a small, well-scoped step: validated
  request (+ material) → `cenc::process` → scoped output.

### Rail transport shim — the carrier→engine adapter is built (Day 27)

The adapter that takes the **sealed-CEK carrier off the wire** and routes it to
the proven in-boundary engines now exists behind the `rail-shim` feature
(`capsules/decrypt-provider/src/rail_shim.rs`, default OFF, NOT wired into
`OpenSession`). It encodes recommended **Option A** (decrypt VM *receives*
VM-sealed material) for **both** profiles:

- `SealedDecryptBundle { profile, sealed_cek, ciphertext_segment, init_segment }`
  — carries only sealed/public bytes, **never** a raw CEK. (Renamed from
  `SealedDecryptCarrier` to kill the collision with Principle #4's Carrier *plane*:
  "carrier" here is the *data* the CEK is carried in, not the transport substrate.
  The `decrypt_from_carrier*` function family keeps its name — it matches the
  `rail_carrier_*.json` goldens — and there "carrier" always means *this bundle*.)
- `SessionSecret` (the VM's in-VM session key, a separate argument — never on the
  wire) dispatches: `ClassicalP256` → `decrypt_sealed_segment` (`rail-prep`);
  `PqHybrid` → `PqSealedEnvelope::from_bytes` (new wire-decode) →
  `decrypt_pq_sealed_segment` (`pq-rail-prep`).
- 7 characterization tests pin it: classical happy path (driven by the committed
  `classical_cenc.json` golden, so the shim and PC2-conformance share one fixture)
  + PQ happy path; and fail-closed for wrong session (both profiles), malformed
  carrier (both), profile/secret mismatch, and tampered PQ signature.

**The single line `OpenSession` adds the day the rail is confirmed:**
```rust
let (bytes, meta) =
    rail_shim::decrypt_from_carrier(&vm_session_secret, &carrier, &verifier)?;
// then map (bytes, meta) into the existing scoped media response.
```

How the open questions map onto this (so none of them is now a *design* task —
each is a one-line selection):
- **Q1 (dKMS-direct seal vs key-provider re-seal):** does not touch the adapter —
  either way the decrypt VM receives a sealed carrier; only *who sealed it* differs.
- **Q2 (signature scheme — `ml-dsa-65` vs hybrid `ECDSA+ml-dsa`):** the PQ path
  verifies through a `CekSealVerifier`, so the chosen verifier plugs in without
  touching `rail_shim.rs`. **BOTH answers are now pre-proven — Q2 is a pure policy
  pick, not a build task:**
  - *straight ML-DSA-65* (Day 32–33): the real FIPS 204 verifier
    (`pq_envelope::mldsa::MlDsa65Verifier`, feature `pq-mldsa`) is built,
    `wasm32-wasip1`-verified, and proven end-to-end through `decrypt_from_carrier` on a
    committed real-signed carrier golden (`rail_carrier_pq_mldsa.json`, feature
    `rail-shim-mldsa`).
  - *hybrid ECDSA-P256 + ML-DSA-65* (Day 41): `pq_envelope::hybrid::HybridVerifier`
    (feature `pq-mldsa-hybrid`) where BOTH halves must verify (fail-closed if either
    fails), `wasm32-wasip1`-built, driven through the same `hybrid_unwrap` path;
    proven for happy path, both-halves-required, tampered, and malformed framing.

  Whichever policy Anders picks, `OpenSession` constructs that verifier and the rest
  of the rail is unchanged.
- **Profile choice (classical migration vs PQ target):** selected per-deployment by
  `SealProfile`; classical exists only for PC2 parity during migration.

The remaining genuinely-blocked pieces are *external* to this capsule: who mints
the carrier (runtime/key-provider, Q1), the concrete signature primitive (Q2), and
the carrier delivery field/transport (whether `DecryptSessionRequestV1` grows a
`material` field, or it arrives via `carrier_invoke`).

## Inter-stage CEK transport — ECDH + DSA, PQ-hybrid (Irzhy, 2026-06-08)

Irzhy independently flagged this exact gap and proposed: either wrap key-release +
decrypt into one box, or secure the channel between them with ECDH + DSA. Decision:
**two boxes, secured channel** (do not merge — merging widens the authority blast
radius). The CEK travels **sealed to decrypt-provider's per-session public key and
signed**; it is unwrapped, used, and zeroized **only inside decrypt-provider**, so
recovery + decrypt are colocated (the "one box" benefit) without collapsing the
rights → key → decrypt authority separation.

- decrypt-provider mints a per-session keypair + publishes its session pubkey
  (PC2 precedent: `ddrm-decrypt/session.rs`).
- the key authority seals the CEK to that pubkey (KEM/ECDH) + signs (DSA).
- **strongest variant:** the dKMS seals **directly** to the decrypt session key, so
  key-provider is a pure broker and never holds a raw CEK.
- **crypto reconciliation:** PC2's vendored envelope is classical P-256 ECDH +
  ECDSA; the shipped rail must use the runtime PQ-hybrid profile
  (`x25519 + ml-kem-768`, `ml-dsa-65`, `elastos-pq-hybrid-threshold-v0`) — keep
  PC2's envelope structure + discipline, upgrade the crypto.
- **PQ profile de-risked (Day 20):** the PQ-hybrid seal/unwrap shape is built and
  characterization-tested as `decrypt-provider/src/pq_envelope.rs` (feature
  `pq-envelope`, default OFF) — `x25519+ml-kem-768` hybrid KEM → SHA-256 KDF →
  AES-256-GCM unwrap recovers the CEK in `Zeroizing`; wrong KEM secret / tampered
  blob / bad signature all fail closed; the signature sits behind a
  `CekSealVerifier` abstraction so `ml-dsa-65` (or a hybrid) plugs in cleanly; and
  it builds to `wasm32-wasip1`. So once Q2 (signature scheme) is answered, swapping
  the classical `envelope.rs` path for the PQ one is a known-good drop-in, not a
  research task. Details + versions: `DDRM_STATUS.md` §"PQ-hybrid envelope
  de-risked".

Full write-up + threat model + invariant→test table: `DDRM_SECURITY_MODEL.md`.

### Sharpened questions for Anders
1. Should the **dKMS seal directly** to the decrypt session key (key-provider as
   pure broker), or is a key-provider **re-seal** acceptable?
2. Signature during transition: move straight to **ml-dsa-65**, or keep a
   **hybrid** (ECDSA + ml-dsa) while PC2's classical path is migrated?
