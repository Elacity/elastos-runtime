# dDRM encrypt side — invariant #1, the gap, and the target contract

**Status:** encrypt boundary pinned by characterization tests; **the in-boundary
CEK/KID generation gap is CLOSED (Day 19)** — CEK+KID are now minted with a CSPRNG
inside the wasm boundary and consumed by a vendored CENC cipher; only the sealing
rail (PQ-envelope CEK escrow) + ciphertext availability remain, behind a
fail-closed `seal`. Branch: `feat/decrypt-provider-cenc`.

This is the *producer* end of the dDRM chain and the home of **Irzhy's security
invariant #1**:

> During encryption, the CEK and KID should be generated within a wasm boundary;
> only the ciphertext and its relatives should be set as output of the process and
> used by the other components of the workflow.

The decrypt side (invariant #2) is already pinned end-to-end (envelope spec +
cenc + consumer contract — see `DDRM_STATUS.md`). This doc closes the loop on the
encrypt side.

## What PC2 does today (the stable reference)

PC2's encrypt path (`pc2-node`), traced against the repo:

| Step | Where | File |
|---|---|---|
| Generate CEK (`randomBytes(16)`) + KID (`randomUUID`) | **Node/TS host** | `src/services/media/dashPackager.ts::generateCEK` |
| Escrow/seal the CEK (Chipotle) | host → provider | `dashPackager.ts::encryptMediaCEK` |
| CENC-encrypt fMP4 segments (AES-128-CTR) | **wasm** (`cenc-encrypt`) | `crates/cenc-encrypt/src/lib.rs` |
| Zeroize raw CEK | host (`cek.fill(0)`) + wasm (`cek_bytes … = 0`) | both |
| Emit ciphertext + IVs + PSSH/tenc (KID) | wasm output | `cenc-encrypt` `EncryptResult` |

**The cipher and zeroization are disciplined**, and `cenc-encrypt`'s output only
ever carries ciphertext + IVs + KID-derived boxes — it **never emits the CEK**.
That satisfies invariant #1's *output* half.

**The gap:** CEK/KID **generation happens in the Node host**, and the raw CEK is
held in host memory long enough to be passed *into* the wasm encryptor
(`cek_b64`). That is precisely the part of invariant #1 that is not yet in-boundary
— Irzhy's concern, verbatim.

## What the runtime has today

- `elastos-common` already defines the durable **output types**: `SealedObjectV1`
  (`payload_cid`, `rights_policy_cid`, `key_envelope`, `viewer`) and
  `KeyEnvelopeV1` (`scheme`, `kid`, **`wrapped_cek`**, `policy_hash`, `algorithms`).
  Note: the CEK only ever exists as `wrapped_cek` in these types — the shape itself
  forbids a raw CEK in output.
- There is **no `encrypt-provider` capsule** producing them. Structural gap.

## Decision: a dedicated `encrypt-provider` capsule

Encrypt is a distinct, high-authority concern (it mints the only true secret), so
it gets its own provider — not folded into decrypt or the trusted core.

- **Tier:** `microvm` provider (max isolation), same as the rest of the chain;
  ships as wasm-proven today, microVM as the hardening upgrade (same Rust source).
- **Invariant #1, enforced by construction:**
  - the **caller never supplies a CEK** — `SealRequest` has no key field and
    `deny_unknown_fields` wire-rejects any smuggled `cek`/`cek_b64`;
  - the CEK+KID are **minted inside the boundary** (the engine);
  - the plaintext asset is referenced by handle and consumed in-boundary;
  - outputs carry only `payload_cid` (ciphertext), `kid`, IV(s), and a
    **`wrapped_cek`** — never the raw CEK or plaintext;
  - the raw CEK is **zeroized** before the boundary returns.

### elastos-common reconcile — DONE (Day 39)

Days 16–38 kept `encrypt-provider` **self-contained** (no `elastos-common` dep)
while Anders redid 0.4.0, so churn in the shared `protected_content` types could
not break the producer. That risk is gone: the contract has been byte-identical for
many days and `ddrm-drift-check.sh` pins the full consumed surface — the duplication
was a rebase liability, not a safeguard. Reconciled:

- The sealed **OUTPUT** now uses the shared
  `elastos_common::protected_content::SealedObjectV1` / `KeyEnvelopeV1` /
  `KeyEnvelopeAlgorithmsV1` / `ViewerRequirementV1` (the `sealed_output_*` test
  builds the *typed* shared struct, so a raw-CEK field cannot exist by
  construction), and the producer's algorithm set is checked by the shared
  `validate_protected_content_key_envelope_algorithms` — the same validator
  `key-provider` runs downstream.
- The local `SEALED_OBJECT_SCHEMA` const was removed in favour of the shared one.
- **Local-by-design residue:** the encrypt **INPUT** `SealRequest` stays local —
  there is no shared seal-request type in `protected_content` yet. If one is added
  (e.g. `EncryptSealRequestV1`), pin it in the drift guard and adopt it here.

13 tests green, `wasm32-wasip1` build clean, full `ddrm-verify.sh` PASS.

## Invariant #1 → enforcement (executable)

All in `capsules/encrypt-provider/src/main.rs`, all passing except the marked gap:

| Invariant #1 clause | Test | State |
|---|---|---|
| Caller cannot supply a CEK (key minted in-boundary) | `seal_request_cannot_carry_a_cek_on_the_wire` | ✅ pass |
| Output carries only sealed/non-secret material (no raw CEK) | `sealed_output_never_carries_raw_cek` | ✅ pass |
| Raw CEK zeroized after use | `cek_is_zeroized_after_use` | ✅ pass |
| Boundary blocks raw_cek + plaintext authority | `status_blocks_raw_cek_and_plaintext_authority` | ✅ pass |
| Nothing seals by accident (fail-closed) | `seal_fails_closed_until_engine_configured` | ✅ pass |
| Weak scheme rejected | `seal_rejects_unsupported_scheme` | ✅ pass |
| **CEK+KID generated in-boundary (no host involvement)** | `cek_and_kid_generated_inside_boundary` | ✅ pass (Day 19) |
| Engine emits no key material (ciphertext + KID + IVs only) | `seal_engine_emits_no_key_material` | ✅ pass (Day 19) |

## What closed the gap (Day 19)

Vendored PC2 `cenc-encrypt`'s AES-128-CTR cipher core (`crates/cenc-encrypt` @
`a0a910158`) into `capsules/encrypt-provider/src/cenc.rs` — the symmetric
counterpart of the AES-CTR core `decrypt-provider` vendored from `cenc-decrypt`,
plus the in-boundary keygen PC2 lacks:

- `mint_cek_and_kid()` mints a 16-byte CEK + 16-byte KID with a CSPRNG
  (`getrandom` → WASI `random_get` on `wasm32-wasip1`). Generation is
  unconditional, takes **no caller input**, and never leaves the sandbox — this is
  the precise move that closes the gap (PC2 minted these in the Node host via
  `dashPackager.ts::generateCEK`).
- `seal_segment_in_boundary()` mints the key, CENC-encrypts the asset's samples
  with it, scrubs the CEK on drop (`Zeroizing<[u8; 16]>`), and returns
  `SealedSegment` — **which has no CEK field**, so the output half of invariant #1
  is enforced by construction.

**Scope held deliberately tight (one boundary at a time):** the cipher core only.
PC2's full fMP4 box surgery (mp4box) and Elacity PSSH injection were **not**
vendored — PSSH embeds chain/Lit authority, a PC2 trust-model concern we must
*translate*, not copy (ACL law). Those + the actual CEK sealing are a later
boundary, so `seal` dispatch stays fail-closed, exactly as decrypt-provider keeps
`open_session` fail-closed behind its already-proven cenc engine.

## What remains (not the keygen gap)

Wire the full `seal`: seal the minted CEK to the rights/key authority via the
PQ-hybrid envelope (proven wasm-viable, `DDRM_STATUS.md` §PQ), package the full
fMP4 (mp4box) + translated protection metadata, upload ciphertext, return a
`SealedObjectV1`. This depends on the same CEK-transport rail the decrypt side
awaits (Anders).

Open question for Anders: should the runtime keep PC2's split (CEK escrow to a
key/license provider) or mint+seal entirely within `encrypt-provider`? Either way,
generation already moved in-boundary — that is the invariant, and it is now met.
