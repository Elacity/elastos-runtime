# dDRM chain — status & review package

**Branch:** `feat/decrypt-provider-cenc` (based on `origin/0.4.0`, **~44 commits**, tip `b3b5f0a9d` + Day-40 integrity audit)
**State:** the full Elacity dDRM provider chain is **fail-closed**, **compiles to
`wasm32-wasip1`**, **executes under WASI**, and has **verified inter-provider
contract handoffs**. Both chain ends are now pinned by tests: the **upstream rail
contract** (ECDH CEK-sealing envelope, `decrypt-provider/src/envelope.rs`) and the
**downstream consumer contract** (both players receive scoped output, never the
CEK). A full team-facing **security + threat model** is in
`DDRM_SECURITY_MODEL.md`. The only thing between here and live decrypt is one
architecture decision (the CEK transport rail) — see `DDRM_DECRYPT_RAIL.md`.

> **Base volatility (Anders, 2026-06-08):** only ~20% of 0.4.0 is on GitHub and its
> latest commits are being redone. This branch is based on `origin/0.4.0`, so its
> base will shift; expect a rebase + re-verify of any `elastos-common`
> `protected_content` types these providers consume once 0.4.0 stabilises. New work
> is kept contract-first (PC2 as the stable reference) to stay rebase-safe. The
> contract has since held byte-identical for many days, so `encrypt-provider`'s
> sealed output was reconciled to the shared `SealedObjectV1` (Day 39); only its
> input `SealRequest` stays local (no shared seal-request type yet).

## The chain

```
app/viewer --drm/open--> drm-provider --sequences--> rights -> key -> decrypt --scoped output--> app
                                          RightsReceipt -^   ReleaseReceipt -^ (wrapped CEK only)
```

## Parity table (proven bar)

| Provider | Role | Fail-closed | Host tests | wasm32-wasip1 | WASI smoke |
| --- | --- | --- | --- | --- | --- |
| `encrypt-provider` | seal/produce (invariant #1) | yes | 13 | builds | — |
| `drm-provider` | orchestrator (`drm/open`) + chain-seam | yes | 12 | builds | 4/4 |
| `rights-provider` | rights decision | yes | 9 | builds | 4/4 |
| `key-provider` | key release (rights-bound) | yes | 9 | builds | 4/4 |
| `decrypt-provider` | decrypt/render (cenc + envelope + consumer contract) | yes | 25 (+2 `rail-prep`) | builds | 4/4 |

The chain now has **both ends present**: `encrypt-provider` is the producer
(invariant #1) and `decrypt-provider` the consumer (invariant #2). **The encrypt
side's in-boundary keygen gap is CLOSED (Day 19):** the CEK+KID are now minted with
a CSPRNG inside the wasm boundary (`getrandom` → WASI `random_get`) and consumed by
a vendored CENC AES-128-CTR cipher (PC2 `cenc-encrypt` @ `a0a910158`), with the CEK
held in `Zeroizing` and an output type (`SealedSegment`) that has no CEK field. The
once-`#[ignore]`d `cek_and_kid_generated_inside_boundary` now passes. Only the full
`seal` (PQ-envelope CEK escrow + fMP4 packaging + ciphertext availability) remains,
behind a fail-closed `seal` — it shares the decrypt side's rail dependency. See
`DDRM_ENCRYPT_INVARIANT.md`.

## Security properties proven

- **Zero ambient authority surfaced.** Every provider's `status` advertises the
  raw authority it blocks (`raw_cek`, `chain_rpc`, `wallet_rpc`, `key_backend_sdk`,
  `kubo_api`, `elacity_sdk`, …) and wire-rejects hidden authority fields
  (`deny_unknown_fields`).
- **Fail-closed by default.** Every operation returns `not_configured` after full
  validation until its real backend exists. Invalid/mis-bound input returns
  `invalid_request`. Nothing opens by accident.
- **CEK containment.** The CEK only ever appears `wrapped` (key step) or
  contained + zeroized inside the cenc engine (decrypt step). The decrypt-step core
  seam is tested to leak neither the CEK nor plaintext to the caller.
- **Authorization binding.** `key-provider` verifies the upstream
  `RightsDecisionReceiptV1` (allowed + principal/session/object/right must match)
  before any release.
- **Contracts compose.** `drm-provider::chain_seam_tests` prove a
  `RightsDecisionReceiptV1` deserializes into the key request and a
  `ReleaseReceiptV1` into the decrypt request — shared-type drift fails loudly.
- **Upstream rail contract pinned (executable spec).** The CEK-sealing envelope
  (vendored from PC2 `ddrm-decrypt`: P-256 ECDH unwrap → AES-256-CBC) is captured
  as `decrypt-provider/src/envelope.rs` with characterization tests: v2/v3
  round-trip, fail-closed parsing, `Zeroizing` on recovered material, and a
  `sealed_envelope_does_not_contain_raw_cek` containment check. This is the
  concrete shape of the rail's "Option A" decrypt boundary.
- **Downstream consumer contract pinned (both players).** Tests in
  `decrypt-provider` prove the scoped, player-facing response carries **metadata
  only** for both viewer capsules — media (fMP4 segments via opaque handle) and
  non-media (render-only plaintext via opaque session id) — and that a real
  decrypted media segment never lets the CEK/IV/plaintext reach the player
  boundary (`media_segment_decrypt_keeps_cek_and_plaintext_off_the_player_boundary`).
- **Rail-landing composition prepped (Parallel Change, feature `rail-prep`).** The
  two previously-separate tested islands — the upstream envelope unwrap
  (`envelope::{parse, ecdh_unwrap, extract_cek}`) and the decrypt-step core
  (`decrypt_session_segment`) — are now joined by `decrypt_sealed_segment`, the
  single in-boundary operation the Hybrid rail will invoke once Anders confirms the
  CEK transport. It mirrors PC2 `ddrm-decrypt::session::unwrap_envelope` → cenc
  decrypt: the CEK materializes only after a correct ECDH unwrap, is held in
  `Zeroizing`, is consumed + zeroized by the cenc engine, and never reaches the
  scoped response. Pinned by characterization tests
  (`sealed_segment_decrypts_end_to_end_and_keeps_cek_off_the_boundary`,
  `sealed_segment_fails_closed_on_wrong_session_key`) and proven to build to
  `wasm32-wasip1`. **The flag is OFF by default — the live dispatch and the 25-test
  default suite are unchanged** — so the live wiring is a one-step swap into
  `open_session`/`render` once the rail + session-key provisioning land.

## How to run it yourself

```bash
# one-time prerequisites
rustup target add wasm32-wasip1
brew install wasmtime

# whole chain, one command:
scripts/ddrm-chain-smoke.sh

# per-provider host tests:
( cd capsules/drm-provider     && cargo test )
( cd capsules/rights-provider  && cargo test )
( cd capsules/key-provider     && cargo test )
( cd capsules/decrypt-provider && cargo test )
```

## PQ-hybrid-in-wasm viability (de-risked, Day 15)

The runtime profile requires PQ-hybrid crypto for the inter-stage CEK seal
(`x25519 + ml-kem-768` KEM, `ml-dsa-65` signature). Before committing the rail to
that profile, we proved the PQ halves actually build inside the wasm boundary:

| Crate | Algorithm | Resolved version | `wasm32-wasip1` |
| --- | --- | --- | --- |
| `ml-kem` (RustCrypto) | ML-KEM-768 (FIPS 203) | 0.2.3 | **builds clean** |
| `ml-dsa` (RustCrypto) | ML-DSA-65 (FIPS 204) | 0.0.4 | **builds clean** |

Proof: a throwaway crate depending on both, built with `cargo build --target
wasm32-wasip1` under the pinned `1.89.0` toolchain — green. Their transitive deps
(`sha3 0.10.9`, `keccak 0.1.6`, `kem`, `signature`, `zeroize`) are all wasm-clean.
The classical halves (`x25519-dalek`, `aes-gcm`) are already wasm-proven in tree.

**Go/no-go:** GO on PQ-in-wasm. One caveat to flag at rail-design time: `ml-dsa`
is still `0.0.x` (early, pre-1.0 API churn likely); `ml-kem` is more settled at
`0.2.x`. Recommend pinning exact versions and keeping the signature scheme behind
the envelope abstraction so a hybrid (ECDSA + ml-dsa) transition stays cheap.

### PQ-hybrid envelope de-risked end-to-end (Day 20)

Beyond "the crates compile", the **seal/unwrap shape now composes and recovers a
CEK** — `decrypt-provider/src/pq_envelope.rs`, the PQ analogue of the classical
`envelope.rs`, behind the `pq-envelope` feature (default OFF, Parallel Change):

- **Hybrid KEM:** `x25519` DH ‖ `ML-KEM-768`; the AES-256-GCM wrap key is derived
  (SHA-256 KDF, labelled + length-prefixed) from **both** shared secrets, so
  confidentiality holds if **either** primitive stays unbroken.
- **AEAD wrap:** authenticated — a wrong KEM secret or tampered blob fails closed
  (`UnsealFailed`), no plaintext on error.
- **Signature behind `CekSealVerifier`** so ml-dsa-65 (or hybrid ECDSA+ml-dsa)
  plugs in without touching the unwrap path (honours the caveat above).
- **CEK returned in `Zeroizing`**; the raw CEK never appears in the sealed bytes.
- **Unwrap needs no RNG and no outbound authority** — a pure in-VM transform, like
  the classical path.

Pinned by 4 characterization tests (`pq_hybrid_round_trip_recovers_cek`,
`wrong_session_secret_fails_closed`, `tampered_signature_fails_closed`,
`sealed_envelope_has_no_raw_cek`) and **proven to build to `wasm32-wasip1`** under
`1.89.0` with the feature on. Resolved versions: `ml-kem 0.2.3`, `x25519-dalek 2`,
`aes-gcm 0.10`, `sha2 0.10`. Run: `cargo test --features pq-envelope` (29 green:
25 default + 4 PQ). **The PQ rail is now a known-good drop-in for the classical
envelope the moment Anders confirms the transport + signature scheme.**

### Full PQ data path proven end-to-end, pre-rail (Day 21)

The three in-boundary engines — Day-18 rail-prep composition, Day-19 in-boundary
keygen, Day-20 PQ envelope — are now bound into **one executable cross-engine
proof**: `pq_envelope::decrypt_pq_sealed_segment` (feature `pq-rail-prep`, default
OFF, enables `pq-envelope`) chains `hybrid_unwrap → decrypt_session_segment`, i.e.
the PQ analogue of the Day-18 classical `decrypt_sealed_segment`. The PQ unwrap
slots exactly where the classical `ecdh_unwrap` does (mirroring PC2
`ddrm-decrypt::session::unwrap_envelope` → cenc), with the CEK in `Zeroizing`
throughout, consumed + zeroized by the cenc engine, and never reaching the scoped
response.

Pinned by a **cross-engine golden**: PQ-seal a CEK and CENC-encrypt a segment with
that *same* CEK, then prove the composed path recovers the plaintext while the CEK
stays off the boundary (`pq_sealed_segment_decrypts_end_to_end_and_keeps_cek_off_the_boundary`),
plus a wrong-session fail-closed case. Builds clean to `wasm32-wasip1`. Run:
`cargo test --features pq-rail-prep` (31 green: 29 + 2 cross-engine). **The entire
PQ dDRM data path — sealed CEK in → rendered bytes out, key contained — is now
proven before the rail lands; the remaining work is the transport shim, not the
crypto or the engines.**

### Engines pinned by portable golden vectors (Day 22)

Both decrypt data paths are now locked by **substrate-independent golden vectors**
(Feathers' characterization/golden-file pattern) committed under
`capsules/decrypt-provider/tests/vectors/` — fixed input bytes → expected output,
captured once and replayed through the engines with **no in-test sealing and no
RNG** (every consumer step — ECDH/x25519 DH, ML-KEM decapsulate, AES open, CENC
decrypt — is deterministic given the captured material):

- **`classical_cenc.json`** — P-256 ECDH envelope (v3) → CENC AES-128-CTR. This
  vector is **byte-compatible with PC2 `ddrm-decrypt`** (same envelope + cenc wire
  shapes), so it doubles as a cross-implementation conformance fixture that can be
  replayed against the reference implementation.
- **`pq_hybrid_cenc.json`** — x25519+ML-KEM-768 hybrid seal → CENC AES-128-CTR
  (`elastos-pq-hybrid-threshold-v0`). Runtime-specific (PC2 has no PQ), so the
  vector pins it across refactor/rebase/port. Replaying it also reconstructs the
  **typed `PqSealedEnvelope` from flat bytes** (ML-KEM dk + ciphertext
  (de)serialization) — exercising the exact wire-decode the live rail will need.

Each vector has a **replay** test (recover CEK → decrypt → assert plaintext) and a
**corrupted-input fail-closed** test. The schema lives in `src/vector_format.rs`.
Feature split keeps the surface clean: `vectors` (default OFF, enables
`pq-rail-prep`) compiles + runs the four replay tests against the committed
fixtures; `gen-vectors` regenerates the fixtures (`cargo test --features
gen-vectors emit_`). The four base suites are **unchanged** (default 25, `rail-prep`
27, `pq-envelope` 29, `pq-rail-prep` 31); `cargo test --features vectors` = **35
green** (31 + 4 golden). Builds clean to `wasm32-wasip1`. **The engines are now
refactor-/rebase-/port-safe and the classical path is conformance-checkable against
PC2 — independent of any in-test seal helper.**

### PC2 cross-impl conformance is now executable (Day 23)

The "byte-compatible with PC2 `ddrm-decrypt`" claim is no longer an assertion — it
**runs**. `scripts/pc2-conformance.sh` decrypts the committed `classical_cenc.json`
using PC2 `ddrm-decrypt`'s **real code** and asserts byte-for-byte parity end to
end:

1. **CEK transport** — PC2 `envelope::parse → ecdh_unwrap → extract_keys_blob`
   recovers the **same 16-byte CEK** from our sealed envelope.
2. **Media** — PC2 `mp4box::parse_segment → cenc::decrypt_samples` decrypts our
   segment to the **same plaintext**.

The harness compiles a small driver (`scripts/pc2-conformance/driver.rs`) against
the PC2 repo on demand via a temp crate, so **no absolute path or PC2 coupling ever
enters the ElastOS build graph**. It resolves PC2 via `PC2_REPO` (default
`/Users/sash/Documents/Cursor/pc2.net/pc2-node`) and **skips clean (exit 0)** when
PC2 is absent, so the default chain is never broken; it **fails (exit 1) only on a
genuine divergence**. Current result against the live PC2 checkout: **PASS** (CEK
and plaintext both match). **Two independent implementations now agree on the exact
bytes of the classical CEK rail — the strongest convergence evidence short of a
shared test crate, and a regression tripwire if either wire format drifts.**

### Conformance promoted to a standing gate + widened (Day 24)

The cross-impl check is now part of the standard pre-rebase/pre-PR gate and covers
more of the contract:

- **`scripts/ddrm-verify.sh`** — one button-press aggregator that runs (1) the
  contract drift check and (2) the PC2 cross-impl conformance. Exits non-zero if
  either gate fails; the conformance step **skips clean** when PC2 is absent, so
  the gate is safe to run anywhere. This is now the recommended first check before
  any rebase onto a moving 0.4.0.
- **Two envelope versions** are cross-checked: `classical_cenc.json` (**v3**,
  random IV) and `classical_cenc_v2.json` (**v2**, IV derived from the ephemeral
  pubkey) — both PC2-supported wire shapes. Each is replayed in-repo
  (`--features vectors` = **36 green**, +1 for the v2 replay).
- **Negative parity:** for every vector the harness also tampers the envelope and
  asserts **PC2 fails closed too** (`tamper: ... rejected ... fail-closed parity
  OK`) — proving both implementations reject the same corruption rather than
  silently leaking plaintext.

Current result against the live PC2 checkout: **PASS** for v3 + v2, positive and
negative. Base suites unchanged (25/27/29/31); chain 68; drift PASS. **The rail
contract is now guarded on both the happy path and the fail-closed path, across
both envelope versions, by code that runs the reference implementation.**

### Encrypt→decrypt round-trip golden — both invariants pinned on one artifact (Day 26)

The two ends were proven separately (invariant #1: `encrypt-provider` mints CEK+KID
in-boundary and CENC-encrypts; invariant #2: `decrypt-provider` unwraps + cenc-
decrypts). They are now proven to **compose** on a single artifact:

- `encrypt-provider` (feature `gen-vectors`) runs its **real in-boundary engine**
  (`mint_cek_and_kid` → `cenc::encrypt_samples` → mux) and writes
  `roundtrip_encrypt_to_decrypt.json` into `decrypt-provider/tests/vectors/`.
- `decrypt-provider` (feature `vectors`) replays it
  (`encrypt_to_decrypt_round_trip_golden`) and asserts it **recovers the producer's
  exact plaintext**, with the CEK leaking onto neither the producer's output type
  (`SealedSegment` has no CEK field — compile-time) nor the consumer's scoped
  response.

`cargo test --features vectors` (decrypt) = **37 green** (+1 round-trip); the base
ladder is unchanged (25/27/29/31) and `encrypt-provider` stays **13** (emit gated
off by default). Both build clean to `wasm32-wasip1`.

**Recorded gap (the rail, unchanged):** the CEK is captured into the fixture as a
stand-in for the still-blocked transport rail — in production it reaches decrypt
**sealed**, never in the clear. So this golden pins the **cipher + keygen
composition** (an asset sealed here decrypts there); the **seal/envelope transport**
is exactly what lands when Anders confirms the rail. The byte-identical cipher cores
(both `apply_keystream` AES-128-CTR with `pad_iv`) make that composition sound.

### Rail transport shim — the rail is now a flag flip, not a design (Day 27)

Everything *downstream* of the rail was proven (unwrap + cenc, both classical and
PQ). The missing piece was the **carrier→engine adapter**: the thin code that takes
the sealed-CEK material off the wire and hands it to the right engine. That adapter
now exists behind the `rail-shim` feature (`decrypt-provider/src/rail_shim.rs`,
default OFF, **NOT** wired into `OpenSession`/dispatch — a Parallel-Change island):

- `SealedDecryptCarrier { profile, sealed_cek, ciphertext_segment, init_segment }` —
  carries only sealed/public bytes (**never** a raw CEK), mirroring rail Option A
  (decrypt VM *receives* VM-sealed material) and PC2 `session::unwrap_envelope`
  (the VM holds the session key; the envelope arrives from outside).
- `decrypt_from_carrier(session, carrier, verifier)` dispatches on profile:
  `ClassicalP256` → `decrypt_sealed_segment` (`rail-prep`); `PqHybrid` →
  new **`PqSealedEnvelope::from_bytes`** wire-decode → `decrypt_pq_sealed_segment`
  (`pq-rail-prep`). The VM session secret is a separate argument — never a carrier
  field. CEK materializes only inside the engine, in `Zeroizing`, off the response.
- **7 characterization tests** (`cargo test --features rail-shim` = **41 green**):
  classical happy path is driven by the committed `classical_cenc.json` golden (so
  the shim and PC2-conformance share one fixture); PQ happy path uses the shared
  `seal_support` sealer; fail-closed is pinned for wrong session (both profiles),
  malformed carrier (both), profile/secret mismatch, and tampered PQ signature.

The base ladder is **unchanged** (25/27/29/31, `vectors` 37); `rail-shim` builds
clean to `wasm32-wasip1`; `ddrm-verify.sh` PASS. The day Anders answers, `OpenSession`
adds exactly one call — `rail_shim::decrypt_from_carrier(&vm_session_secret,
&carrier, &verifier)?` — then maps `(bytes, meta)` into the existing scoped response.
Q1 (dKMS-direct vs re-seal) does not touch the adapter; Q2 (signature scheme) plugs
in through the `CekSealVerifier`; profile is a per-deployment `SealProfile` pick.
Precise wire-up + question→knob mapping: `DDRM_DECRYPT_RAIL.md` §"Rail transport shim".

### Rail carrier pinned as a portable golden + checked against PC2's session API (Day 28)

The shim was proven in-process (Day 27); now its **carrier wire shape** is locked
the same way every engine is — a substrate-independent golden — and cross-checked
against PC2's *session model*, not just its crypto primitives:

- `tests/vectors/rail_carrier_classical.json` (schema `RailCarrierVector`) captures
  the rail Option-A carrier `{profile, sealed_cek, ciphertext_segment, init?,
  expected_plaintext}`. It is **derived from** `classical_cenc.json`, so its
  `sealed_cek` is byte-identical to the PC2-conformant fixture.
- `rail_shim::tests::rail_carrier_golden_replays_through_shim` replays it through
  **`decrypt_from_carrier`** (the exact entrypoint `OpenSession` will call) and
  recovers the plaintext; `…_tampered_fails_closed` pins fail-closed. So the
  carrier format now survives refactor/rebase/port, pinned at the shim boundary.
- **`scripts/pc2-conformance.sh` now checks two layers** against PC2's real code:
  the existing primitive parity (`envelope` + `cenc`) **and** the session/carrier
  path — a session holding the vector key runs PC2's public
  `session::unwrap_envelope` (L1 ECDH + L2 CEK store) → `media::decrypt_segment`
  (tenc IV-size + moof/traf/senc walk) and recovers the exact plaintext, for both
  the v3 and v2 envelopes; a tampered carrier fails closed inside `unwrap_envelope`
  too. This proves our Option-A carrier is wire-compatible with the **entrypoints
  PC2 production calls**, not merely its primitives.

`vectors` stays **37** and the base ladder is unchanged (25/27/29/31); `rail-shim`
= **43** (+2 carrier-golden); builds clean to `wasm32-wasip1`; `ddrm-verify.sh`
PASS (now including the two-layer session conformance). The carrier golden is the
artifact `OpenSession` will accept on the day Anders confirms the rail.

### PQ carrier golden — profile symmetry closed (Day 30)

Day 28 pinned the *classical* carrier as a portable golden + PC2 session conformance;
the **PQ-hybrid** profile now has the matching carrier golden:

- `tests/vectors/rail_carrier_pq.json` (schema `RailCarrierVector`, `profile: PqHybrid`):
  the `sealed_cek` is `PqSealedEnvelope::to_bytes()` (the carrier wire form the shim's
  `from_bytes` decodes); the VM session secret is carried as its **two parts** (x25519
  static secret + ML-KEM-768 decapsulation key) so replay reconstructs it with **no RNG**.
- `rail_shim::tests::rail_carrier_pq_golden_replays_through_shim` replays it through
  `decrypt_from_carrier`'s PQ branch (`from_bytes` → `decrypt_pq_sealed_segment`) and
  recovers the plaintext; `…_tampered_fails_closed` pins fail-closed.
- New `seal_support::session_secret_from_parts` reconstructs `SessionKemSecret`
  deterministically (mirrors the VM restoring its own session key).

**Deliberately no PC2 cross-impl layer for this profile.** The PQ-hybrid profile is
runtime-only (`elastos-pq-hybrid-threshold-v0`); PC2's `ddrm-decrypt` is classical
P-256 and has **no PQ session counterpart**, so there is no reference implementation to
check byte-parity against. (The classical carrier remains two-layer PC2-conformant.)

Base ladder unchanged (25/27/29/31, `vectors` 37); `rail-shim` = **45** (+2 PQ carrier);
builds clean to `wasm32-wasip1`; `ddrm-verify.sh` PASS. Both rail profiles now have a
carrier golden replayed through the exact `OpenSession` entrypoint.

### Media (cenc) golden widened toward real playback shapes (Day 31)

The media-contract goldens were all single-sample / single-subsample / default-IV.
Real fMP4 isn't, so the parts most likely to bite at wire-up are now pinned by
**executable PC2 parity**:

- `tests/vectors/classical_cenc_multisample.json` — a **3-sample** segment, each with
  its own per-sample IV and a fresh AES-128-CTR counter (`trun` per-sample sizes;
  `senc` no-subsample).
- `tests/vectors/classical_cenc_subsample.json` — a **subsample** sample (clear+encrypted
  ranges, `senc` flags `0x000002`); the CTR keystream is continuous across encrypted
  ranges only (clear bytes skipped).
- `tests/vectors/classical_cenc_initseg.json` — a **16-byte IV** segment whose size is
  driven by an `init` segment's `tenc.default_per_sample_iv_size` (moov→…→stsd→encv→sinf
  →schi→tenc), exercising the init-derived IV path.

Each replays through our decrypt engine (`vectors`, +3 → **40**) **and** through PC2's
real `mp4box::parse_segment` + `cenc::decrypt_samples` **and** PC2's session API
(`session::unwrap_envelope` → `media::decrypt_segment`, init threaded for the IV-size
case) in `scripts/pc2-conformance.sh`, asserting byte parity + tamper fail-closed. The
`ClassicalVector` schema gained optional `init_segment_b64` / `iv_size` (backward
compatible); the conformance driver now parses `senc` at the vector's IV size and passes
the init to `decrypt_segment`. Box layouts validated against PC2 `mp4box.rs`/`cenc.rs`
(byte-identical `parse_init_for_tenc`, incl. the encv 78-byte skip).

Base ladder otherwise unchanged (default 25; `rail-shim` 45); `wasm32-wasip1` clean;
`ddrm-verify.sh` PASS.

### Real ML-DSA-65 signature primitive — the last PQ placeholder closed (Day 32)

The PQ envelope's seal-signature was a **`StubSigner`/`StubVerifier`** (a SHA-256
placeholder behind the `CekSealVerifier` slot). The **real FIPS 204 ML-DSA-65**
primitive is now wired in, behind a new `pq-mldsa` feature (separate axis — the
default build + base ladder stay byte-stable):

- `pq_envelope::mldsa::MlDsa65Verifier` (production) implements `CekSealVerifier` over
  RustCrypto `ml-dsa` 0.1 (same family as the already-vetted `ml-kem`). The decrypt
  boundary only ever **verifies** — construction (`VerifyingKey::new_from_slice`) + verify
  need **no RNG**, so it compiles cleanly to **`wasm32-wasip1`** (the real constraint:
  ML-DSA verify inside the WASI sandbox). Fail-closed: a wrong-size key encoding yields
  no verifier; a malformed/non-matching signature verifies `false` (no panic, no
  which-half probe). `ml-dsa` is pulled with `default-features = false` (no pkcs8 /
  getrandom).
- **Proven (feature `pq-mldsa`, +5 tests → 34):** the real primitive plugs into the exact
  `hybrid_unwrap` path (genuine seal signature → CEK recovered; tampered sig → `BadSignature`);
  rejects a **wrong key**; rejects a **tampered body**; fails closed on **malformed**
  encodings.
- **Committed KAT** (`tests/vectors/mldsa65_kat.json`, schema `MlDsaKatVector`): a
  verifying key + signature over a fixed canonical transcript, generated deterministically
  via `SigningKey::from_seed`. Replayed under `pq-mldsa` (verify-accept + tamper-sig/body
  fail-closed). It pins the real primitive across refactor/rebase/port **and upstream-crate
  drift** — if `ml-dsa` ever changed its keygen/signature output, this would stop verifying.

**What this means for "quantum-proof":** the PQ rail is no longer stubbed anywhere — the
shipped signature primitive is real and WASI-verified. The remaining PQ gaps are now purely
*external*: Anders' Q2 transition policy (straight ML-DSA-65 vs hybrid ECDSA+ML-DSA during
PC2's migration) and landing the rail (the `rail-shim` flag-flip, which already accepts any
`CekSealVerifier` — `MlDsa65Verifier` drops straight in).

Base ladder byte-stable (default 25 / rail-prep 27 / pq-envelope 29 / pq-rail-prep 31 /
vectors 40 / rail-shim 45); new `pq-mldsa` = **34**; `wasm32-wasip1` clean (default +
`pq-mldsa`); `ddrm-verify.sh` PASS.

### Real ML-DSA-65 verified through the rail entrypoint — loop closed (Day 33)

Day 32 proved the real primitive in `hybrid_unwrap`; Day 33 drives it through the **exact
`decrypt_from_carrier` entrypoint** `OpenSession` flag-flips on, on a **committed
real-signed carrier golden** (feature `rail-shim-mldsa = rail-shim + pq-mldsa`):

- `tests/vectors/rail_carrier_pq_mldsa.json` — a PQ-hybrid carrier whose `sealed_cek`
  signature is a **genuine FIPS 204 ML-DSA-65 signature** (key authority key deterministic
  via `from_seed`, so the golden is reproducible). It carries the published verifying key
  (`mldsa_vk_b64`, new optional `RailCarrierVector` field — needed because the real verifier
  holds a key where the stub held none).
- Replayed through `decrypt_from_carrier`'s PQ branch verified by the production
  `MlDsa65Verifier(mldsa_vk_b64)` (**not** the stub): plaintext recovered; and fail-closed on
  (a) **tampered signature**, (b) a **different verifying key**, (c) a **tampered envelope body**.
  +4 tests → `rail-shim-mldsa` = **54**.

This is the strongest possible pre-rail proof: *the real PQ signature, verified through the
real rail entrypoint, on a portable committed artifact.* The day Anders answers Q2, the live
`OpenSession` passes a `MlDsa65Verifier` into the one `decrypt_from_carrier` call — nothing
else changes. `DDRM_DECRYPT_RAIL.md` Q2 updated: no longer a build gap, purely a policy choice.

Base ladder + `pq-mldsa` byte-stable (25/27/29/31/40/45; `pq-mldsa` 34); new
`rail-shim-mldsa` = 54; `wasm32-wasip1` clean; `ddrm-verify.sh` PASS.

### Fail-closed under adversarial input — proven (Day 34)

The wire-decoders are the surfaces the rail exposes to **attacker-controlled carrier
bytes** (`envelope::parse`, `PqSealedEnvelope::from_bytes`, `decrypt_from_carrier`
dispatch). A new test-only `harden` feature (= `rail-shim-mldsa`; off by default, base
ladder byte-stable) adds an adversarial negative-space + containment sweep (+11 →
`harden` = **65**):

- **Truncation sweep:** *every* proper prefix of a valid envelope/carrier fails closed
  (classical + PQ).
- **Byte-flip sweep:** single-byte corruption at *every* position **never panics** (a
  panic in a wasm capsule is a DoS) — classical parse, PQ `from_bytes`, and the
  `decrypt_from_carrier` dispatch.
- **Oversized length prefixes:** over-large `u16`/`u32` prefixes (incl. `u32::MAX`,
  exercising the `checked_add` overflow guard) fail closed — no over-read.
- **Corruption-never-recovers:** a tampered-but-decodable PQ carrier still fails closed
  at unwrap (AES-256-GCM auth ‖ ML-DSA-65 signature) — never yields a CEK; error
  surfaces stay coarse (no which-field probe).
- **Containment:** profile/secret mismatch fails closed **both** directions; and neither
  the scoped metadata (happy path) nor the error string (tampered) contains the plaintext
  or the CEK across the carrier path.

This makes "**fail-closed and panic-free under adversarial input**" — a core capability-
security claim — executable, on the exact boundaries the rail will expose. Base ladder
byte-stable; `wasm32-wasip1` clean; `ddrm-verify.sh` PASS.

### Producer round-trip widened to real playback shapes (Day 37)

Day 26 pinned invariant #1 ↔ #2 on a single-sample artifact; the decrypt side already
proved multi-sample / subsample / non-default-IV shapes (Day 31). Day 37 closes that
asymmetry **from the producer end**: `encrypt-provider`'s real in-boundary engine
(`mint CEK+KID → cenc::encrypt_samples`) now emits two more round-trip goldens —
`roundtrip_multisample_encrypt_to_decrypt.json` (4 samples, per-sample IVs) and
`roundtrip_subsample_encrypt_to_decrypt.json` (16-byte clear leader + encrypted body) —
muxed with framing that mirrors PC2 `cenc-encrypt::mp4box::build_senc` /
`build_senc_with_subsamples`. `decrypt-provider` replays each back to the producer's
**exact** plaintext with the CEK off the scoped boundary (`vectors` 40 → **42**). The gate
(`ddrm-ladder-check.sh`) runs all three `*_round_trip_golden` tests **by name** (asserts 3
passed), so an encrypt-side change that breaks decrypt over any shape fails the gate.
`wasm32-wasip1` clean; `ddrm-verify.sh` PASS.

### Producer output proven consumable by PC2's real decrypt (Day 38)

The Day-37 round-trips proved *our* producer ↔ *our* consumer. Day 38 closes the
convergence-critical loop: the multi-sample + subsample segments
`encrypt-provider`'s real in-boundary engine emitted are now driven through **PC2
`ddrm-decrypt`'s real `mp4box::parse_segment` + `cenc::decrypt_samples`** in
`scripts/pc2-conformance.sh`, asserting byte-for-byte plaintext parity plus a
wrong-CEK key-bound check (PC2 must NOT recover the plaintext under a flipped CEK).
The driver dispatches on schema (classical envelope vectors keep their two-layer
envelope+session parity; producer round-trips, which carry no envelope, run the
segment-decrypt parity). PC2 decrypts our producer's output byte-for-byte —
**our producer ↔ PC2's real decrypt is now executable**, not just our internal
round-trip. `ddrm-verify.sh` PASS with PC2 present.

## Integrity audit — every claim maps to a gate (Day 40)

A "trust-but-verify-the-verifier" pass: every "proven"/count claim above was checked
against something the standing gate (`scripts/ddrm-verify.sh`) or a named test
actually enforces. Counts were re-validated by running the suites fresh, not from
memory.

| Claim | Enforced by (re-run Day 40) |
|---|---|
| 5 providers fail-closed, host-tested (13/12/9/9/25 = 68) | gate 3 ladder — per-provider suites, counts asserted |
| decrypt feature ladder (27/29/31/42/45/34/54/65) | gate 3 ladder — each rung run, count asserted (a dropped/feature-gated-out test fails the gate) |
| contract types intact on the current base | gate 1 drift — 13 consts / 10 structs / 1 fn / 10 fields |
| byte-compatible with PC2 (consumer *and* producer) | gate 2 conformance — classical envelope+session + producer round-trip segments through PC2's real code; skips clean w/o PC2 |
| builds to `wasm32-wasip1` (5 providers + PQ/rail features) | gate 3 ladder — 7 wasm builds |
| **executes under WASI, fail-closed end-to-end** | gate 4 WASI smoke — `ddrm-chain-smoke.sh` under wasmtime (added to the standing gate Day 40; skips clean w/o wasmtime) |
| encrypt↔decrypt seam over real shapes (single/multi/subsample) | gate 3 seam — all 3 `*_round_trip_golden` run by name (3 passed) |
| real ML-DSA-65 verified through the rail entrypoint | gate 3 — `rail-shim-mldsa` rung (54) + committed `rail_carrier_pq_mldsa.json` |
| fail-closed + panic-free under adversarial input | gate 3 — `harden` rung (65) |

**Orphan / dead-surface sweep:** all **13** committed golden vectors in
`decrypt-provider/tests/vectors/` are referenced by at least one test or the
conformance script (no orphan fixtures). Every decrypt-provider feature flag is a
ladder rung except `gen-vectors` (a fixture-regeneration tool, intentionally not a
test rung) — no documented-but-unwired flags. The only previously doc-only claim
("executes under WASI") is now gate-backed (gate 4). `ddrm-verify.sh`: **ALL GATES
PASS** (with PC2 + wasmtime present on this machine).

## The one open decision (for Anders / Irzhy)

How the CEK reaches the decrypt boundary. **Hybrid chosen** (decrypt step
*receives* sealed material; upstream rights→key is a provider chain). Irzhy
independently converged on the same gap and proposed **two boxes + secured channel
(ECDH + DSA)** over merging — adopted, upgraded to the runtime PQ-hybrid profile.
Three sharpened sub-questions remain for Anders:

1. Does the **dKMS seal directly** to the decrypt session key (key-provider as a
   pure broker that never holds a raw CEK), or is a key-provider **re-seal** ok?
2. Signature during transition: straight to **ml-dsa-65**, or a **hybrid**
   (ECDSA + ml-dsa) while PC2's classical path is migrated? *(The real ML-DSA-65
   verifier is now built + WASI-verified behind `pq-mldsa` and drops into the
   `CekSealVerifier` slot — this is purely a policy choice now, not a build gap.)*
3. Does the provider-invocation rail expose an in-capsule `carrier_invoke` client
   a microvm provider may use today, or is that still landing?

Full options, threat model, and the invariant→test table:
`DDRM_DECRYPT_RAIL.md` + `DDRM_SECURITY_MODEL.md`.

## Isolation tier

Providers ship as **`wasm` now** (proven cross-platform, runs on macOS today);
**microVM** remains the later max-isolation upgrade from the same Rust source. The
fail-closed contract is tier-independent. Rationale in `DDRM_DECRYPT_RAIL.md`.

## Base reconciliation (Day 17) — 0.4.0 force-push, zero type drift

Anders force-pushed `origin/0.4.0` (`42e4d7ffd` → `67b7560a7`), redoing commits as
warned, with more still to come. We did **not** rebase yet (0.4.0 is still moving),
but verified the impact:

- **`elastos-common/protected_content.rs` is byte-identical** between this branch
  and the redone `origin/0.4.0` (`git diff` = 0 lines). The redone base
  independently landed the exact types our providers were built against
  (`RightsDecisionReceiptV1`, `KeyReleaseRequestV1.rights_receipt`, typed
  `DecryptSessionRequestV1.release_receipt`, `ReleaseReceiptV1.session_id/action`).
  **The convergence held — zero type drift.**
- A drift guard, `scripts/ddrm-drift-check.sh`, asserts every schema constant,
  struct, and chain-binding field the chain depends on still exists on the current
  base. Run it before any rebase/PR; it fails loudly if a future 0.4.0 redo moves a
  type. **Currently: PASS.**
- All five providers' host tests pass against the current tree:
  `encrypt 13`, `drm 12`, `rights 9`, `key 9`, `decrypt 25` → **68 green, 0 ignored**
  (Day 19 closed the encrypt keygen gap: 6+1-ignored → 13). `decrypt` adds **+2**
  under `--features rail-prep` (Day-18 rail-landing composition).
- Rebase recipe + safety backup (`backup/decrypt-provider-cenc-preD17`):
  `PUSH_PLAN.md` § "Base moved".

**Day 36 reconcile-prep re-verification.** Re-measured against the force-pushed base:
- `origin/0.4.0` is **no longer an ancestor** of our branch (diverged; merge-base
  `589092b95`, +3 base commits). The rebase recipe now uses `git rebase --onto
  origin/0.4.0 "$(git merge-base …)"` so only our own commits replay, with a
  `ddrm-verify.sh` checkpoint after each branch and `git range-diff` to confirm
  nothing drops. `PUSH_PLAN.md` § "Rebase recipe" is now button-press + has the
  per-branch conflict surface (incl. the `encrypt-provider` self-containment and
  bincode-2x churn points).
- **Contract still byte-identical** (re-verified): `git diff
  origin/0.4.0..feat/decrypt-provider-cenc -- …/protected_content.rs` = 0 lines.
- **Drift guard widened to the full consumed surface** (was a Day-17 subset): now
  pins **13 consts / 10 structs / 1 free fn / 10 fields**, adding the genuinely-
  consumed-but-unpinned symbols — `validate_protected_content_key_envelope_algorithms`
  (called by drm + key), the `DEFAULT_PROTECTED_CONTENT_*` algorithm sets,
  `ViewerRequirementV1`, and the PQ-negotiation fields on `KeyEnvelopeAlgorithmsV1`
  (`cipher`/`kem`/`signature`/`share_scheme`). A rename of any now fails the guard
  loudly instead of surfacing as a compile error mid-rebase.
- **The encrypt↔decrypt seam is now gate-enforced:** `ddrm-ladder-check.sh` runs
  `encrypt_to_decrypt_round_trip_golden` **by name** and asserts 1 passed, so an
  encrypt-side change that breaks decrypt (or a silent cfg/rename drop of the
  cross-invariant golden) fails the gate.

## Commits (on `feat/decrypt-provider-cenc`, not yet pushed — GitHub suspension)

**17 of our commits** (the original 14 below, plus Day 15 status/PQ, Day 16
encrypt-provider, Day 17 drift guard). Note: `git rev-list --count
origin/0.4.0..HEAD` reports **19** against the force-pushed base because 2 orphaned
old-upstream commits are still in range — the rebase (`PUSH_PLAN.md`) drops them.
Newest last:

1. `docs(convergence)` — north-star playbook, product vision PRD, v0.4.0 plan
2. `feat(decrypt-provider)` — vendor PC2 cenc-decrypt engine as fail-closed backend
3. `docs(ddrm)` — record decrypt-rail decision (CEK/ciphertext transport)
4. `feat(decrypt-provider)` — tested decrypt-step core seam (Branch-by-Abstraction)
5. `docs(ddrm)` — isolation-tier recommendation (wasm now, microVM as hardening)
6. `docs(ddrm)` — confirm decrypt-provider compiles clean to wasm32-wasip1
7. `test(decrypt-provider)` — WASI-sandbox smoke harness proves fail-closed execution
8. `feat(key-provider)` — bind rights receipt + bring to wasm/WASI-proven bar
9. `test(rights-provider)` — WASI smoke completes rights→key→decrypt chain parity
10. `test(drm-provider)` — WASI smoke + cross-provider contract-seam tests
11. `feat(ddrm)` — unified chain smoke runner + review-ready status package
12. `feat(ddrm)` — vendor ECDH CEK-sealing envelope spec + PC2 player alignment
13. `test(ddrm)` — pin decrypt→player consumer contract for both viewer capsules
14. `docs(ddrm)` — security model doc + inter-stage CEK transport decision

Push order & PR mapping when GitHub returns: `PUSH_PLAN.md`.

Supporting docs: `DDRM_DECRYPT_RAIL.md`, `DDRM_SECURITY_MODEL.md`,
`PC2_PLAYER_ALIGNMENT.md`, `CONVERGENCE_PLAYBOOK.md`, `PRODUCT_VISION.md`,
`PUSH_PLAN.md`.
