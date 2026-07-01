# Intent-Proof Loop — the prover/verifier loop for agent *actions* (design)

**Status:** design (in-cloud). Most of the verifier substrate already ships (W2 consent +
the signed audit chain); this doc specifies the *new* surface — a first-class **intent
record** and an **intent↔outcome reconciliation** — and the fail-closed contracts so the
implementation is test-drivable before a line is written. No hardware lane required.

## The one sentence

Make an autonomous agent **declare what it intends to do before it does it**, have the
runtime **prove that intent is within a standing authorization** (fail-closed) before the
act fires, and then **record the gap between what was declared and what actually happened**
as a signed, tamper-evident custody fact — so an agent can run *unsupervised* because an
independent verifier checks and records every act.

## Why now (the insight, grounded)

Recent prover/verifier results in mathematics work because **verification is asymmetric to
generation** (checking a proof is easier than finding one) and because trust ultimately
rests on a **machine-checkable substrate** (the proofs are formalized in Lean, which the
model cannot talk its way past), not on a second model's opinion.

Port that to agents:

| Math harness | This runtime |
|---|---|
| Prover = the model generating a proof | Prover = the agent generating an **action** |
| LLM verifier (foolable) | the agent's own narration / a watcher LLM (foolable) |
| **Lean** = machine-checkable ground truth | **this runtime** = machine-checkable ground truth for *what the agent was allowed to do and actually did* |

The runtime is already the "Lean" for agent custody: capability tokens are the proof
*obligations*, the signed audit chain is the proof *checker*. The intent-proof loop closes
the remaining gap — it makes the agent emit its proof obligation (an **intent**) *before*
acting, and turns the **declared-vs-done** delta into a checkable quantity. Verification is
not a tax on autonomy here; it is the thing that *lets you take your foot off the brake* —
you can run an agent unsupervised precisely because something unfoolable checks and records
each act.

## Where we are (what's already true — the verifier substrate ships)

The W2 affordance-consent flow is already a prover/verifier loop for a **single,
human-approved** act. The pieces:

- **The intent's content already has a home.** A capability request carries an
  affordance **binding** — `(capsule, principal_id, method_id, input_hash, resource,
  action)` — set via `PendingCapabilityRequest::with_affordance_binding`
  (`elastos-runtime/src/capability/pending.rs`). `input_hash` is the deterministic
  `canonical_input_hash(&Value)` (`elastos-common/src/canonical_hash.rs`) of the exact
  arguments, so an approval binds to a specific method **and** specific args.
- **The verifier already runs, fail-closed.** `validate-and-consume`
  (`CapabilityManager::validate`) re-hashes the *actual* invocation input and compares it
  to the binding; a different method or different args fails closed. The
  `ValidatedAffordanceGrant` witness (W2 step 8) makes the check compiler-enforced — consent
  cannot be dispatched without it.
- **The proof is already signed and durable.** A redemption emits an
  `AffordanceGrantReceiptV1` (`elastos-runtime/src/capability/receipt.rs`), ed25519-signed by
  the runtime issuer key over `(capsule, method_id, input_hash, resource, action, token_id,
  redeemed_at)` under a domain tag — *"if there is no receipt, there is no act"*: the
  redemption fails closed unless a durable signed record exists.
- **The ground-truth substrate is the audit chain.** `primitives::audit::AuditLog` is the
  per-record ed25519-signed hash chain; `emit()` is fail-closed; `chain_attestation()`
  re-verifies the whole chain on read. Receipts and denials ride this chain.
- **It already projects honestly.** The two-channel projection (trust + custody) and the
  Home/inspector surfaces render these facts read-only (ESP); nothing in the UI re-derives
  authority.

So the asymmetry, the machine-checkable substrate, and the "no receipt → no act" discipline
are **already here**. What is missing is (a) the intent as its own pre-act record, (b) the
reconciliation of declared-vs-done, and (c) the standing envelope that lets the loop run
*without* a human approval per act.

## The gap (what this design adds)

1. **An explicit, signed, pre-act intent record.** Today the binding lives inside a transient
   pending request that exists *because a human is being asked*. For unsupervised autonomy the
   agent must emit an intent as a first-class custody record on the chain **before** acting —
   the agent's proof obligation, recorded whether or not anyone is watching.
2. **Intent↔outcome reconciliation as a custody fact.** Today we have the receipt (what was
   redeemed) but no recorded comparison of *declared intent* vs *actual outcome*. The delta —
   matched / diverged / undelivered — must itself be a signed record, because the gap between
   "what the agent said" and "what it did" is the whole point.
3. **A standing-grant envelope.** Today consent is per-act and human-gated. Unsupervised
   autonomy needs a pre-authorized **capability envelope** (issued once, via the existing
   consent path) within which the agent may declare-and-act repeatedly, each act verified
   `intent ⊆ envelope` and recorded — no per-act human approval.

## The architecture

```
  agent                          runtime (the verifier / "Lean")            chain
  ─────                          ─────────────────────────────────          ─────
  1. declare intent  ─────────▶  IntentDeclarationV1 (signed)        ──emit──▶ [intent]
     (method, args→hash,
      resource, action,
      standing_grant_id)

  2.                             VERIFY  intent ⊆ standing grant
                                 (reuse binding + canonical_input_hash;
                                  fail-closed: method/resource/action/args
                                  outside the envelope ⇒ DENY)
                                        │
                                   deny │────────────────────────────emit──▶ [denied]
                                        │ pass
  3. act  ◀──────────────────────  dispatch (within envelope)
                                        │
  4.                             redeem ⇒ AffordanceGrantReceiptV1   ──emit──▶ [receipt]
                                 (no receipt ⇒ no act, as today)
                                        │
  5.                             RECONCILE  receipt + act audit events
                                 vs the declared intent
                                   Matched | Diverged | Undelivered  ──emit──▶ [reconciliation]
```

Every step that matters lands on the same durable chain, so an auditor reads, in order: the
agent *declared* X, the runtime *verified* X was authorized (or denied it), the act produced
receipt Y, and the reconciliation says Y *matched* (or *diverged from*) X — all
signature-verifiable under the runtime's key.

## The contracts (specify before building)

New records (mirror the existing `*V1` + schema-tag + ed25519-signed conventions in
`receipt.rs`):

- **`IntentDeclarationV1`** — `{ schema, capsule, method_id, input_hash, resource, action,
  standing_grant_id, declared_at, signer, signature }`. The agent's proof obligation. Signed
  (by the agent's key if it holds one, else runtime-countersigned at the boundary it crosses);
  `input_hash` is `canonical_input_hash` of the declared args, identical to the W2 binding so
  the verifier reuses one hashing path.
- **`IntentReconciliationV1`** — `{ schema, intent_id, receipt_id?, status, divergence_detail?,
  reconciled_at, signer, signature }` where `status ∈ { Matched, Diverged, Undelivered }`.
  Runtime-signed. `Matched` ⇒ the receipt's `(method_id, input_hash, resource, action)` equal
  the intent's; `Diverged` ⇒ a redeemed act whose fields differ from the declared intent (within
  the envelope, so it fired, but it is flagged); `Undelivered` ⇒ intent declared, no receipt
  (act never completed) — never silently dropped.
- **Standing grant** — an existing capability envelope (issued once through the W2 consent path)
  the intent is checked against; reuses revoke/expiry custody (G8b `CapabilityManager::revoke`,
  fail-closed durable) so a revoked envelope denies fail-closed.

Fail-closed branch matrix (Kent Beck — each row is a test):

| Condition | Outcome |
|---|---|
| No intent declared / unsigned intent | No act — there is no obligation to discharge |
| `intent ⊄ standing grant` (method/resource/action/args outside envelope) | **DENY**, emit `[denied]`; act never dispatched |
| Standing grant expired or revoked | **DENY** (reuse revoke/expiry custody) |
| Spend budget exhausted | **DENY** (reuse `SpendMeter` fail-closed `try_debit`) |
| Act dispatched but no receipt | No act counts (existing "no receipt → no act") |
| Receipt fields ≠ intent fields | Act fired (within envelope) but `[reconciliation: Diverged]` is emitted and surfaced |
| Intent declared, no receipt within TTL | `[reconciliation: Undelivered]` — absence is recorded, never a silent pass |
| Audit chain unavailable / `emit` fails | Fail closed — no act (custody is mandatory, per W2 `validate`'s blocking audit) |

## Code-today vs new surface (honest deferrals)

- **Reuse, no new crypto:** `canonical_input_hash`, the ed25519 receipt signing pattern, the
  `AuditLog` chain + `chain_attestation`, the `ValidatedAffordanceGrant` witness, the spend
  meter, revoke custody. The verifier substrate is *not* new.
- **New surface (this design):** the two records above; the `intent ⊆ envelope` check (a
  superset of the existing exact-binding compare — it must match `method/resource/action` and
  confirm `input_hash` is within the envelope's allowed shape, *not* a single frozen hash); the
  reconciliation step; and the standing-grant envelope issuance (one consent, repeated acts).
- **Projection (follow-on):** intent + reconciliation become custody sub-states the inspector /
  Home project (a `Diverged`/`Undelivered` reconciliation renders red, like a broken chain —
  never greened). This is a pure-surface ESP addition, in the cloud lane.

## Scope / non-goals (the boundary, stated out loud)

- This loop verifies **containment + custody**, **not correctness of judgment**. It can prove
  *"the agent declared it would email finance, was authorized to, and the signed record shows it
  emailed exactly that"* — it **cannot** prove the email's *content* was wise. It checks the
  envelope and records the delta; it does not score the decision. (This is exactly what EU AI Act
  Art 12/14 demands, and it is the honest limit.)
- The asymmetry is cleanest where the act crosses a **mediated boundary** the runtime already
  sees — capability use, egress, spend. Physical/analog side effects beyond mediated boundaries
  are **out of scope** here (and would need their own sensing surface; named, not hand-waved).
- "Diverged" is a flag, not an automatic rollback. Rollback/compensation is a separate policy
  layer; this design's job is to make divergence *undeniable and visible*, not to undo it.

## Test plan (test-drivable, in-cloud)

1. Record round-trips: `IntentDeclarationV1` / `IntentReconciliationV1` sign + verify under the
   runtime key; chain `emit` + `chain_attestation` confirms each record verifies (mirror the
   `egress_audit`/receipt tests).
2. Verifier matrix: every row of the fail-closed table above as a unit test — in particular
   `intent ⊄ envelope ⇒ DENY` and `revoked envelope ⇒ DENY`.
3. Reconciliation: `Matched` on equal fields; `Diverged` on a mismatched redeemed field;
   `Undelivered` on a TTL-elapsed intent with no receipt — assert each emits the right signed
   record and never the wrong one.
4. Projection conformance (follow-on): a `Diverged`/`Undelivered` reconciliation renders as an
   honest alarm sub-state and is never masked by a clean trust/spend channel (mirror the W5b
   independent-channels SSR tests).

## Implementation status (as built on `flint`)

The verifiable core is built, gated, and on-chain — `capability/intent.rs` + the `AuditEvent`
intent variants + the ESP `intentProofView` — AND the live dispatch mode that *calls* the gate now
exists (`dispatch_standing_act` against `StandingGrantStore`), proven end-to-end from a real token.

- ✅ **ch1** — `IntentDeclarationV1` / `IntentReconciliationV1` (ed25519-signed) + the fail-closed
  verifier matrix (`check_intent_within_envelope`, `reconcile`).
- ✅ **ch2** — `AuditEvent::{IntentDeclared,IntentDenied,IntentReconciled}` + builders; emit-and-
  `chain_attestation`-verify test (the verdict is tamper-evident custody).
- ✅ **ch3** — `StandingGrantEnvelope::from_token` derives the envelope from a real
  `CapabilityToken` (capsule/resource/action/expiry signed-in; methods + revocation supplied by the
  caller — named, not faked).
- ✅ **ch4** — `run_intent_gate` orchestrator: custody-first → verify → the act runs ONLY past a
  passing gate → reconcile. The load-bearing test proves a denied intent NEVER runs the act.
- ✅ **ch5** — ESP `intentProofView` + `<CapsuleCustodyPanel>` paint: the verdict is a third
  INDEPENDENT custody channel (absent / clean / flagged), never masked by a green chain/meter.
- ✅ **ch5b (runtime)** — `count_intent_proof` + `AuditLog::intent_proof_summary`, PRESENCE-aware
  (a non-gated capsule is ABSENT, not falsely "clean").
- ✅ **ch5b (inspector) = Tier 2b** — `intent_proof_summary` exposed through the `AuditSource` trait
  (fail-honest `None` default), an `intent_proof` field projected on the capsule detail (keyed
  `vm-{name}`), threaded through the ESP data path. The intent channel is LIVE (absent / clean /
  flagged), no longer latently ABSENT.
- ✅ **ch4b = Tier 2c** — the standing-grant dispatch mode: `StandingGrantStore` (fail-closed
  issue/revoke registry) + `dispatch_standing_act`, which routes a self-declared agent act through
  `run_intent_gate` against a stored standing grant. Proven end-to-end from a real `CapabilityToken`:
  derive the envelope via `from_token` → issue → dispatch (act runs, reconciles matched) → **revoke
  by token id → the next dispatch is denied and its act never runs** (the autonomy kill switch).
  **This is "an agent runs unsupervised under this loop" — and revoking its grant halts its run.**
  Revocation semantics (honest): the gate re-reads the grant at the START of each dispatch, so a
  revoke denies every act that has not yet passed the gate. It does NOT interrupt a single act
  already past the gate and executing (the usual check-then-act window) — it stops the agent's run
  at the next act, not the one in mid-execution.

NOTE: the gate is deliberately NOT wired into the existing per-act carrier path — that path already
enforces via validate-and-consume (single-use consent), so re-checking an envelope derived from the
same token would be redundant. The gate belongs to ch4b's standing-grant mode.

## One-line summary for `state.md`

> Intent-proof loop = the prover/verifier loop for agent *actions*: declare intent → verify
> `intent ⊆ standing grant` (fail-closed) → act → record declared-vs-done as a signed custody
> fact. Most of the verifier substrate already ships (W2 binding + receipt + audit chain); the
> new surface is the intent record, the reconciliation record, and the standing envelope.
