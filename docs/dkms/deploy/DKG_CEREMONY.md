# Born-Distributed DKG Ceremony (advanced / opt-in)

The baseline quorum (RUNBOOK §9) is **producer-escrow**: the publisher splits the CEK and escrows
one share to each node. That already gives you "no single node holds the key, any two recover".

**DKG (Distributed Key Generation)** raises the bar: the CEK is **born distributed** — it exists
**nowhere** during generation, not even at birth. Each node deals a private contribution and
installs its share by summing the dealers' sub-shares; any two shares reconstruct the same CEK, and
no node (nor the runtime, nor the producer) ever sees the whole key.

This is **implemented and verified today** — the runtime orchestrator drives it across three real
`dkms-authority` daemons, and the dry-run asserts it (gates 49–51, **PASS**):

> a fresh 2-of-3 CEK was BORN distributed across THREE real daemons — each dealt a private
> contribution and each installed its share by SUMMING the dealers' sub-shares; any two shares
> reconstruct the SAME CEK … the CEK existed NOWHERE during generation.
>
> the ceremony is VERIFIABLE — a tampered sub-share is refused at install and the contributing
> dealer is NAMED (each sub-share is sealed + signed, AEAD-bound to its dealer→target pair) … and
> generation is OPERATOR-BOUND (a non-operator install is refused live).

## Node ops involved (grounded)

The node advertises these in its `status.supported_operations`:
`dkg_contribute`, `dkg_install` (alongside `reshare_contribute`/`reshare_install` for
reconfiguration, and `rotate_share`/`revoke_caller` for lifecycle). All are **operator-authorized**.

### Authorization is a versioned v2 manifest digest (DKMS-5)

The hardened node does **not** authorize a lifecycle op against a loose `(kid, dkg_id, node_set, t,
m)` tuple. Each op is authorized against a **canonical, versioned v2 manifest** (`LifecycleManifestV2`
in the shared `ddrm_envelope::lifecycle` encoder), and the operator signs a collision-resistant
SHA-256 **digest** of the *complete* semantic request. The manifest commits to:

- the **operation and phase** (a `dkg_contribute` token cannot authorize `dkg_install`, re-share,
  rotate, or recover — domain- and version-separated);
- `kid`, `scheme`, the **ceremony id** (`dkg_id`) / source+successor set ids;
- the **thresholds** `(t, m)` and the **executing node** (target coordinate);
- the **ordered membership** — every member's coordinate, verifying key, and **recipient key**
  (shares cannot be reinterpreted by swapping a recipient key the set id never hashed);
- the **input-material / contribution digests** (for install: the exact inbound sub-shares).

Substituting any of these — a key, coordinate, scheme, threshold, set, target, or contribution
digest — makes the authorization **fail to open before any secret material is touched**. A
non-operator authorization fails closed.

**Per-node authorization.** Authorization is verified **on each node** against **that node's own**
manifest (its own executing-node/target coordinate), not once centrally. Mint one operator
authorization per node.

**Digest review.** Before signing, the operator reviews the manifest digest the console computes
against the ceremony parameters it intended (op, kid, ids, thresholds, ordered members/recipients,
material digests). The node's refusal message names exactly which bound fields must match — treat a
refusal as a manifest-mismatch signal, not a transport glitch (the `lifecycle_manifest_mismatch`
ops counter increments on this path).

**Retry semantics.** Lifecycle outputs are **not** byte-identical across retries (KEM sealing draws
fresh randomness), so an **exact retry is rejected as replay**, not served idempotently: the node
records the accepted manifest digest and refuses a second op with the same digest. A legitimate
re-run uses a **fresh ceremony id** (`dkg_id`), which yields a distinct manifest and digest. Capacity
exhaustion of the ceremony replay set fails **closed**.

**No mixed v1/v2 ceremony (migration prohibition).** v2 is a **cutover boundary**. The node no
longer accepts the old v1 lifecycle authorization, so a ceremony must be **entirely v2**. Finish or
abandon any in-flight v1 ceremony before upgrading; never pair a v1-authorized contribute with a
v2-authorized install (it fails closed). See RUNBOOK §12.4.

## Procedure (operator-driven)

Prerequisite: the three nodes are up (RUNBOOK §1–§8) **and** can reach each other over the
WireGuard mesh (intake A7) — DKG is node-to-node, not just runtime-to-node.

1. **Authorize.** The operator console builds a v2 manifest for **each executing node** (op = the
   phase being authorized, `kid`, a fresh `dkg_id`, the ordered three-node membership with each
   node's verifying + recipient key, `t=2`, `m=3`, the target coordinate, and — for install — the
   inbound contribution digests), **reviews the digest**, and signs it (operator-signed, one per
   node). The same manifest discipline authorizes `reshare_contribute`/`reshare_install`, where the
   manifest additionally commits to the source escrow/material identity and the exact successor
   membership.
2. **Contribute.** Each node runs `dkg_contribute`: it deals a private polynomial and emits one
   sealed+signed sub-share per target node (AEAD-bound to the dealer→target pair).
3. **Install.** Each node runs `dkg_install`: it verifies every inbound sub-share (a tampered one is
   refused and its dealer named) and sums them into its own share. The result is a 2-of-3 share set
   for a CEK no party ever assembled.
4. **Bind + publish.** The DKG node-set id (distinct from any pre-existing pin) is recorded; the
   content binding is published. From here the open path is byte-identical to the escrow quorum —
   any two nodes serve, attestation works, and the DKG-born shares are a drop-in for the
   reshare/reconfigure lifecycle (you can later re-share to a new `(t,n)` preserving the same CEK).

## What's left to make this a one-command operator step

The ceremony logic is done and tested; the only gap is an **operator-facing coordinator command**
(`elastos dkms ceremony`) that runs steps 1–4 against the deployed nodes (the dry-run drives the
identical sequence from inside the runtime orchestrator). This is tracked in
[INTAKE §D](./INTAKE.md) and is **optional for launch** — start with producer-escrow 2-of-3, adopt
DKG per-key when you want born-distributed assurance.
