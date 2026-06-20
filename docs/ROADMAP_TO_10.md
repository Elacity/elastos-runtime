# Roadmap to 10/10 — decisions & sequenced plan

Status snapshot (this branch, 13 commits): **composite ~8.0 / 10**, security-weighted ~8.4.
This doc captures the moves that take us higher. It separates what was **built on-branch (Lane A)**
from what needs a **decision** (external audit, a coordinated node redeploy, the permissionless track).

> Forward-looking design direction, not a verified implementation spec. The node-side items (②, ③)
> must be re-verified against the live node wire format at execution time. Lane-A work is already
> landed + gated.

## Where the points are (and aren't)

| Dimension | Score | Lever | Lane |
|---|---|---|---|
| Crypto core | 9.0 | external funded crypto/protocol audit | Decision ① |
| Trust root | 9.0 | AAD-binding into possession-proof | ② (node) |
| Metadata privacy | 5.2 | blinded ids / anon-cred / PIR / padding | ② (rung) + ③ |
| Decentralization | 6.0 | staking/slashing + attestation (+ higher n/t) | ③ |
| Docs-alignment | 7.5 | verdict + KNOWN_GAPS ratchets | ✅ Lane A (done) |
| (perf/test/custody/etc.) | 8.5–8.7 | mostly banked | — |

The cheap, self-servable wins are **done**. 8.0 → ~9 is ① + ②; ~9 → 10 is ③ + the full PETs stack.

## Decision ① — Schedule the external crypto/protocol audit (now)

Not *whether* — *when/budget*. It is the pre-mainnet gate for custodying real value (per PRE_AUDIT).
Our verified-safe registries (`tests/ddrm_verdicts.rs`, `capability_conformance.rs` `KNOWN_GAPS`) and the
PRE_AUDIT verification pass let the firm **scope resolved items out** — cheaper, faster engagement.
**Recommend: green-light this first.** Moves Crypto 9.0 → 9.5+.

## Decision ② — One coordinated node redeploy, three items bundled (execute later)

"The node" = the **dKMS key-share custodians** (`capsules/dkms-authority`, the geo-nodes). Each holds one
share of the content key; ≥2 of 3 release shares to reconstruct. Changing what they do / how the gateway
talks to them changes the **wire format**, so all nodes must update together — a *coordinated redeploy*.
Bundle three changes into ONE redeploy so the coordination cost is paid once. (Operator has node access
+ runbooks; this is a "later" ops task.)

1. **AAD-binding into the possession-proof** — the node binds `aad / segment_digests / node_set_id` into
   its proof so a tampered AAD fails closed *at the node*, not only at the decrypt boundary. *Trust-root
   9.0 → 9.5.* (Already a scoped pre-mainnet item — cheap once redeploying.)
2. **Blinded content-ids + ephemeral per-open keys** — the node verifies authorization over *blinded*
   identifiers + a per-open ephemeral key, so an operator can't build a "who-watched-what" profile from
   the requests it serves. *Privacy 5.2 → ~6.5.* The best-ROI privacy step short of full PIR.
3. **Enforce the node-set pin in deploy config** — make `DKMS_AUTHORITY_NODE_SET_ID_B64` mandatory in
   the deployed config (the release `compile_error!` fence already exists; this is the ops half).

**Rollout safety (must-haves for the plan at execution time):**
- Version the wire format; nodes accept old+new during a drain window so live playback never breaks.
- Canary one node, verify a real open end-to-end, then roll the rest.
- Keep cheater-detection + CEK-commitment (PRE-1) intact throughout.
- A test that a tampered AAD now fails *at the node* (closes the AUDITOR_PACKET §1 invariant).

## Decision ③ — Permissionless-node track (the path to "trustless at scale")

Today: 2-of-3 over an **operator-curated** set — real trust-*minimization*, not trust-*lessness*. To let
*untrusted* nodes join safely you need **two complementary mechanisms** (they cover different axes):

- **Economic security — staking + slashing** (the "collateral on the line"). Each node posts stake;
  provable misbehavior burns it. This is what gives **Sybil resistance** (identities cost money) and
  **accountability**. *The bigger lever for permissionless.* Needs an on-chain node registry + slashing
  conditions + challenge/proof mechanics.
- **Attestation** — prove a node runs correct, untampered code. Two options:
  - **TEE** (SGX / SEV-SNP / Nitro): also keeps the share sealed so a malicious *operator can't extract
    it* — directly raises the t=2 collusion bar. **Caveats:** TEEs have a history of side-channel breaks
    (defense-in-depth, not a vault); re-centralizes trust in a chip vendor; operationally heavy.
  - **Reproducible builds + signed binaries + transparency log**: proves the *code* without trusting
    silicon — more genuinely decentralized, lighter, but no share-sealing.
- **Higher n / higher t** — more nodes + higher threshold so collusion needs more colluders.

**Recommendation:** sequence ③ *after* ① and ②. Within it: **staking first** (Sybil + accountability),
**attestation second** — and design an *attestation-shaped seam* without hard-committing to TEE; decide
TEE vs reproducible-builds by value-at-risk and your decentralization ethos. The strongest end state is
**threshold + attestation + staking together.** *Decentralization 6 → 8+, and softens the t=2 caveat.*

## Tracked follow-ups (Lane A, self-servable — so they don't rot)

- **Per-capsule WASM memory budget — ✅ DONE (B1).** The WASM provider now honors each capsule's
  declared `manifest.resources.memory_mb` clamped to a host ceiling (`ELASTOS_WASM_MEMORY_CEILING_MB`,
  8 GiB default), consistent with the crosvm/VM provider (Principle 7 + 11). Remaining refinement
  (lower priority): a separate per-launch user/policy **grant** step above the manifest declaration
  (today it's manifest-declared + clamped, no interactive approval) — only needed if/when untrusted
  capsules should require explicit memory approval beyond their declaration.
- **WASM CPU runaway protection (Chunk B).** Epoch-interruption so a no-progress spinner can be
  trapped / an operator can terminate a runaway (today a spinning capsule permanently holds a
  blocking thread + CPU and `stop()` cannot kill it). Needs the service-vs-command policy decision.

## Recommended sequence

1. **Now:** green-light ① (external audit) — clock starts; our registries make it cheaper.
2. **Soon (you, later):** execute ② as one coordinated redeploy (I'll write the detailed, verified plan
   on request). Biggest buildable score mover.
3. **Roadmap epic:** ③ permissionless track — staking → attestation (TEE *or* reproducible-builds) → n/t.
4. **Ongoing (me, on-branch):** keep the ratchets honest; small test/coverage strengthening.

**Honest ceiling:** we do **not** reach 10/10 by branch commits — that ceiling is hit. 10/10 is the audit
+ the node-side privacy/trust work + permissionless economic security + the full PETs stack. Those are
investments and decisions, not low-hanging fruit.
