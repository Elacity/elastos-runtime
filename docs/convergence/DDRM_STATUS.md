# dDRM chain — status & review package

**Branch:** `feat/decrypt-provider-cenc` (based on `origin/0.4.0`, **+14 commits**)
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
> is kept contract-first (PC2 as the stable reference) to stay rebase-safe — e.g.
> `encrypt-provider` is intentionally self-contained until the shared types settle.

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

## The one open decision (for Anders / Irzhy)

How the CEK reaches the decrypt boundary. **Hybrid chosen** (decrypt step
*receives* sealed material; upstream rights→key is a provider chain). Irzhy
independently converged on the same gap and proposed **two boxes + secured channel
(ECDH + DSA)** over merging — adopted, upgraded to the runtime PQ-hybrid profile.
Three sharpened sub-questions remain for Anders:

1. Does the **dKMS seal directly** to the decrypt session key (key-provider as a
   pure broker that never holds a raw CEK), or is a key-provider **re-seal** ok?
2. Signature during transition: straight to **ml-dsa-65**, or a **hybrid**
   (ECDSA + ml-dsa) while PC2's classical path is migrated?
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
