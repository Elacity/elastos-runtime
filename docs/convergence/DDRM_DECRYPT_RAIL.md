# dDRM Decrypt Rail — design decision (open)

**Status:** Decision required (architecture + security). Routes to Anders.
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
| `decrypt-provider` | yes (+ tested decrypt-step core seam) | yes | 17 | builds | `scripts/wasm-smoke.sh` |
| `key-provider` | yes (+ rights-receipt binding: allowed + principal/session/object/right must match) | yes | 9 | builds | `scripts/wasm-smoke.sh` |
| `rights-provider` | (declared; not yet brought to this bar) | — | — | — | — |

`key-provider` now verifies the upstream rights decision before it would ever
release a key (the `rights -> key` link): a denied, malformed, or mis-bound
`RightsDecisionReceiptV1` is rejected with `invalid_request`; a valid request
still fails closed (`not_configured`) until the PQ-hybrid dKMS backend exists.
The CEK only ever appears here as `key_envelope.wrapped_cek` — never raw.

## What is NOT blocked (and is done/ready)

- The cenc engine is vendored, characterization-tested, and proven to contain +
  zeroize the CEK (Day 1).
- `decrypt-provider` validates the full session+receipt contract and fails closed.
- Once the rail is chosen, wiring is a small, well-scoped step: validated
  request (+ material) → `cenc::process` → scoped output.
