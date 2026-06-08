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

## What is NOT blocked (and is done/ready)

- The cenc engine is vendored, characterization-tested, and proven to contain +
  zeroize the CEK (Day 1).
- `decrypt-provider` validates the full session+receipt contract and fails closed.
- Once the rail is chosen, wiring is a small, well-scoped step: validated
  request (+ material) → `cenc::process` → scoped output.
