# The media pipeline — CENC, envelopes, packaging, and the decrypt rail

How plaintext becomes ciphertext and back: what the encrypt boundary mints, how the CEK
travels, and what the decrypt boundary does with it. This is the deep dive behind
[SECURITY_MODEL.md](SECURITY_MODEL.md) §4–§7.

Grounded in `capsules/encrypt-provider/`, `capsules/decrypt-provider/`,
`capsules/ddrm-envelope/`, `capsules/cenc-core/`, `capsules/ddrm-media/`, and
`elastos/crates/elastos-common/src/protected_content.rs`.

---

## 1. The two binding invariants

1. **Encrypt.** The CEK and KID are generated **inside** the wasm boundary; only the
   ciphertext and its non-secret relatives (KID, IVs, metadata) leave.
2. **Decrypt.** The CEK is **never** passed in plaintext to any other component. CEK
   recovery **and** content decryption happen **colocated in one boundary**, with
   zeroization at the end.

Everything below exists to make those true by construction, not by discipline.

---

## 2. Encrypt — mint, CENC, escrow

`encrypt-provider` owns the producer end. Invariant #1 is enforced structurally:

- **The caller cannot supply a CEK.** `SealRequest` has no key field and
  `deny_unknown_fields` wire-rejects a smuggled `cek` / `cek_b64`.
- **The CEK and KID are minted in-boundary.** `mint_cek_and_kid()` draws a 16-byte CEK and
  a 16-byte KID from a CSPRNG (`getrandom` → WASI `random_get` on `wasm32-wasip1`).
  Generation is unconditional, takes no caller input, and never leaves the sandbox.
- **The engine emits no key material.** `seal_segment_in_boundary()` mints the key,
  CENC-encrypts the asset's samples with it (AES-128-CTR), scrubs the CEK on drop
  (`Zeroizing<[u8; 16]>`), and returns a `SealedSegment` that **has no CEK field**.
- **The output type forbids a raw CEK.** The sealed output is the shared
  `elastos_common::protected_content::SealedObjectV1` (`payload_cid`, `rights_policy_cid`,
  `key_envelope`, `viewer`) whose `KeyEnvelopeV1` carries `scheme`, `kid`, **`wrapped_cek`**,
  `policy_hash`, `algorithms` — the CEK only ever exists wrapped. The producer's algorithm
  set is checked by the shared `validate_protected_content_key_envelope_algorithms`, the
  same validator `key-provider` runs downstream.

Pinned by (all in `capsules/encrypt-provider/src/main.rs`):
`seal_request_cannot_carry_a_cek_on_the_wire`, `sealed_output_never_carries_raw_cek`,
`cek_is_zeroized_after_use`, `status_blocks_raw_cek_and_plaintext_authority`,
`seal_fails_closed_until_engine_configured`, `seal_rejects_unsupported_scheme`,
`cek_and_kid_generated_inside_boundary`, `seal_engine_emits_no_key_material`.

**Local-by-design residue:** the encrypt *input* `SealRequest` stays capsule-local — there
is no shared seal-request type in `protected_content` yet. If one is added, pin it in
`scripts/ddrm-drift-check.sh` and adopt it here.

### Content addressing and packaging

The ciphertext is published to a content-addressed store and fetched back by its CIDv1.
The addressing is **byte-identical to `@helia/unixfs` `addBytes`**: CIDv1, raw leaves,
1 MiB fixed-size chunks, balanced dag-pb layout, single-chunk collapse to the raw leaf
(`bafkrei…`), multi-chunk under a dag-pb root (`bafybei…`), and a balanced tree above the
1024-child fan-out with cumulative `Tsize` and per-level `blocksizes` exactly as
`ipfs-unixfs-importer` emits them.

Fetch verifies the bytes hash back to the requested CID and **fails closed** at every
block and every tree level — a missing block, a tampered leaf, a tampered intermediate, a
tampered root, or any length/structure mismatch. A corrupt or malicious store can never
substitute, reorder, or truncate content under a root the runtime trusts.

The dag-pb encoding and root CIDs are pinned byte-for-byte against the real Helia importer
by `scripts/dev/unixfs-oracle` (a Node ground-truth oracle using `@helia/unixfs`
directly); the goldens live in `scripts/dev/ddrm-runtime-open/src/main.rs`, test
`content_plane_tests::unixfs_root_cid_matches_helia_oracle`. Regenerate the oracle and
update that test in lockstep if the importer's defaults change.

---

## 3. The crypto profile

Post-quantum hybrid by default; anything weaker is rejected at validation
(`elastos-common::protected_content`).

| Property | Value |
|---|---|
| Content cipher | `aes-256-gcm` or `chacha20-poly1305` for envelopes; CENC media uses AES-128-CTR (`aes-128-gcm` is rejected as an envelope cipher) |
| KEM | hybrid **`x25519 + ml-kem-768`** required (classical-only rejected) |
| Signatures | `ed25519` + `ml-dsa-65` (a PQ signature is required) |
| Key sharing | `shamir-t-of-n` threshold |
| Envelope scheme | `elastos-pq-hybrid-threshold-v0` |

### PQ-hybrid envelope

`capsules/decrypt-provider/src/pq_envelope.rs` implements the hybrid seal/unwrap:
`x25519 + ml-kem-768` hybrid KEM → SHA-256 KDF → AES-256-GCM unwrap, recovering the CEK in
`Zeroizing`. The signature sits behind a `CekSealVerifier` abstraction, so the
straight ML-DSA-65 verifier (`pq_envelope::mldsa::MlDsa65Verifier`, FIPS 204) and the
hybrid ECDSA-P256 + ML-DSA-65 verifier (`pq_envelope::hybrid::HybridVerifier`, where
**both** halves must verify or it fails closed) are a policy pick, not a build task. Both
build to `wasm32-wasip1`.

Fail-closed on: a wrong KEM secret, a wrong session secret, a tampered blob, a bad
signature, malformed framing. Pinned by `pq_hybrid_round_trip_recovers_cek`,
`wrong_session_secret_fails_closed`, `tampered_signature_fails_closed`,
`sealed_envelope_has_no_raw_cek`.

`ml-dsa` is pre-1.0 — pin exact versions and keep the signature behind the envelope
abstraction.

---

## 4. The decrypt rail — Option A, material pushed in

The decrypt boundary is a **pure transform**: it *receives* VM-sealed material rather than
reaching out for it, so the highest-authority component holds no outbound network or
capability authority. The chain upstream is provider-to-provider
(`drm → rights → key/dKMS`), but the decrypt *step* is handed what it needs.

This is the safest split and the smallest blast radius: the CEK exists unwrapped only
inside the boundary, for the duration of one decrypt.

### In-sandbox session key

`init` calls `pq_envelope::mint_session()` — `x25519` + ML-KEM-768 via `OsRng` → WASI
`random_get`, `wasm32-wasip1`-clean. The secret stays in the boundary; the canonical
public bytes and the suite are **published** in the init response as
`decrypt_session_public_key_b64`. The key authority seals to that published key. The
session secret is never a request field, and a fresh key is minted per init. Minting is the
only entropy the boundary needs — the unwrap path itself is RNG-free.

### Transcript binding

The sealed material binds the **full transcript**, so a validly-sealed CEK cannot be
replayed against a different session, object, or receipt. `DecryptTranscriptV1` encodes
into a domain-separated, length-prefixed AAD (`to_aad()`):

principal · session · object CID / content hash · action · viewer interface · output kind ·
expiry · SHA-256 `release_receipt_hash` · the in-sandbox `decrypt_session_pub` · the suite
id · the provider id · the node-set id · a replay `nonce`.

The envelope binds it **two ways**: the CEK is AES-256-GCM-wrapped with the transcript as
**AAD**, and the ML-DSA-65 signature covers `payload ‖ transcript`. (`aad == b""` reproduces
the legacy unbound envelope byte-for-byte, so committed goldens are unchanged.) The
boundary rebuilds the transcript from the **authenticated request plus its own provisioned
session public key** — it never trusts the transcript from the carrier.

Proven fail-closed against: a replay under a different `session_id`, a swapped replay
nonce, and a tampered carrier.

### Multi-segment binding

Real media is many `moof+mdat` fragments sharing one presentation CEK.
`to_aad_with_segments(Some(digests))` appends the concatenation of each segment's content
digest (the same digest under each segment's raw CIDv1) **after** the node-set id — so a
single-segment open (`None`) is byte-identical to before, while a multi-segment open is
welded to the exact ordered, content-addressed set. A reorder, drop, add, or substitute
changes the digests, the AAD no longer matches the seal, and the unwrap fails closed
**before any byte is decrypted**.

`decrypt_session_segments` loops the in-boundary single-segment decrypt over N segments
under the one CEK (the per-sample IV counter continues across segments, so no IV is
reused), sums `sample_count`, reports `segment_count`, and **fails closed on the first bad
segment, naming its index** — never a partially decrypted asset. The CEK is unwrapped once
and held in `Zeroizing` across the loop.

### Threshold and quorum rails

The split rails are at full parity. They reconstruct the CEK **once in-boundary** — XOR for
2-of-2, Lagrange at x=0 over GF(256) for 2-of-3 — then loop the same way
(`decrypt_from_carrier_threshold_segments` / `decrypt_from_carrier_quorum_segments`). Every
dKMS node seals its share to the same segment-bound transcript; the node needs no change
because it seals to a runtime-supplied `aad_b64`. `key-provider` merges the shares into the
material so the boundary rebuilds that exact AAD. **The whole CEK is never assembled in
`key-provider` and never crosses a wire.**

### Expiry and audit

The bound-open path takes an **injected capability clock** (`now_unix` — never an ambient
read) and enforces expiry **before any crypto**: past `request.expires_at` or the release
receipt's expiry fails closed with `expired`, and the CEK never materializes for a stale
grant.

Every decision emits a scoped audit record — schema `elastos.ddrm/decrypt-audit@1` —
carrying request id, principal, session, object, action, suite, provider, decision, reason,
the **`transcript_hash`** (SHA-256 of the bound transcript), and the timestamp. It carries
**no CEK and no plaintext** on either path; on `opened` it also carries the scoped session,
on `denied` the reason only.

### Consolidated envelope — `SealedDecryptMaterialV1`

The carrier is a single backend-neutral, **suite-tagged** type. The `suite` tag makes the
backend a FIELD, not a fork:

```rust
/// Backend-neutral, suite-tagged sealed decrypt material (pushed in to the boundary).
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
```

The canonical open routes by suite into the audited, expiry-enforcing, transcript-bound
path. The compat suite is recognised but rejected on the product path; an unknown suite
fails closed.

**Upstream item still open:** folding `sealed_decrypt_material` into the shared
`elastos_common::protected_content::DecryptSessionRequestV1` as an additive
`Option<SealedDecryptMaterialV1>` field (unset == today's fail-closed `not_configured`
path; set == the live transcript-bound open). Until then it rides a capsule-local request
variant so `protected_content` stays byte-identical and `scripts/ddrm-drift-check.sh` stays
green. The decrypt-session public key the CEK is sealed to is published by the boundary at
init and is never a request field either way.

---

## 5. What each viewer receives

Both viewers are *consumers* of scoped, already-decrypted output. Neither ever sees the CEK.

| Viewer | Content | Receives | Addressed by | Never receives |
|---|---|---|---|---|
| **`elacity-player`** (media) | video, audio | decrypted fMP4 segments (init + media) for MSE `appendBuffer`, streamed per segment | opaque session handle | CEK, IV, raw key bytes |
| **`ddrm-viewer`** (non-media) | PDF, EPUB, CBZ, images, code | rendered pixels (pixel-lock) or sanitized markup plus watermark (html-lock) | opaque session id | CEK, IV, raw key bytes |

Presentation-layer lockdown in the non-media viewer is *rendering policy*, not key custody.

Runtime enforcement (`capsules/decrypt-provider/src/main.rs`): the scoped response carries
**metadata only** — an allow-list of `schema`, `session_id`, `object_cid`,
`viewer_interface`, `output_kind`, `is_protected`, `sample_count`, `expires_at` — and a
forbidden-key check rejects any `cek` / `iv` / `key` / `plaintext` / `decrypted` / `secret`
field ever appearing in the player-facing response, for **both** viewer kinds. A media
segment decrypt asserts that neither the CEK nor the decrypted bytes reach the scoped
output.

See [VIEWER_SESSIONS.md](VIEWER_SESSIONS.md) for the session lifecycle and how the viewer
is reached.

---

## 6. Verification

```bash
scripts/ddrm-verify.sh          # drift + PC2 conformance + test ladder + WASI smoke
scripts/ddrm-drift-check.sh     # fails loudly if a consumed protected_content symbol moved
scripts/ddrm-ladder-check.sh    # per-feature test-count rungs + clean wasm32-wasip1 builds
scripts/ddrm-chain-smoke.sh     # all four chain providers under wasmtime, fail-closed
scripts/ddrm-producer-smoke.sh  # mint → escrow → recover → re-seal → decrypt, in one run
```

`DDRM_VERIFY_FAST=1` skips the two heavy gates.

---

## 7. Where the design came from

The design decisions above were reached over the convergence effort and are recorded, with
the alternatives considered and the day-by-day evidence, in:

- [history/DDRM_DECRYPT_RAIL.md](history/DDRM_DECRYPT_RAIL.md) — the push-in-vs-pull
  decision, the per-rung `rail-*` feature ladder, and the exact contract delta.
- [history/DDRM_ENCRYPT_INVARIANT.md](history/DDRM_ENCRYPT_INVARIANT.md) — the in-boundary
  keygen gap and how it was closed.
- [history/PC2_PLAYER_ALIGNMENT.md](history/PC2_PLAYER_ALIGNMENT.md) — the two-viewer split
  validated against the reference implementation.

Those are snapshots. This page is the current contract.
