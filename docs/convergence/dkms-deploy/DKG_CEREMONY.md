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
reconfiguration, and `rotate_share`/`revoke_caller` for lifecycle). All are **operator-authorized**:
the install carries an operator-signed authorization bound to the ceremony's `(kid, dkg_id,
node_set, t, m)`. A non-operator install fails closed.

## Procedure (operator-driven)

Prerequisite: the three nodes are up (RUNBOOK §1–§8) **and** can reach each other over the
WireGuard mesh (intake A7) — DKG is node-to-node, not just runtime-to-node.

1. **Authorize.** The operator console mints the ceremony authorization (operator-signed, bound to
   `kid`, a fresh `dkg_id`, the three-node set, `t=2`, `m=3`).
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
