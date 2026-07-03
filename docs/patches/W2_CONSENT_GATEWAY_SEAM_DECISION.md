# Preserved for decision: gateway consent-request seam (w2-consent-source)

**Status: DECISION NEEDED — deliberately not applied.**

## What this is

The `w2-consent-source` branch was retired during the 2026-07-03 branch
consolidation (all value folded into `flint-0.5`). A content audit found 2 of
its 3 commits fully superseded by `flint-0.5` (`canonical_input_hash` lives in
`elastos-common/src/canonical_hash.rs`; the pending-request consent binding
lives in `capability/pending.rs` and evolved further into
`validate_and_consume` + `AffordanceGrantReceiptV1`).

One commit is genuinely absent and is preserved verbatim as
`w2-gateway-consent-request-3694975.patch` (author: SashaMIT, 2026-06-27):

> feat(gateway): consent-request path replaces the flat 403 for gated
> affordances (W2 steps 4b.2, 4b.3) — `InvocationGate{Direct,Consent}`,
> `affordance_consent_descriptor`, `request_affordance_consent`,
> `AffordanceConsentPending` 202 (~498 lines in
> `elastos-server/src/api/gateway_capsule_catalog.rs`).

## Why it was NOT merged mechanically

It collides with a **deliberate** `flint-0.5` posture, not an accidental gap:

- `flint-0.5`'s `enforce_affordance_invocation_policy` still dead-rejects
  gated affordances with the flat 403 (`FORBIDDEN "approval_required"`), and a
  test **affirmatively pins that behavior**
  (`gateway_capsule_catalog.rs`, `assert_eq!(err.1, "approval_required")`).
- `flint-0.5` carries a *more advanced* alternative consent architecture the
  branch never had: runtime intent-envelope redemption
  (`capability/intent.rs` `IntentDeclarationV1`, `validate_and_consume`,
  `AffordanceGrantReceiptV1`).

## The decision to make

Choose one, then act:

1. **Runtime intent-envelope path wins (likely):** the gateway seam stays
   fail-closed 403 by design; delete this patch and the pinned test stands.
2. **Gateway 202-consent seam wanted after all:** apply the banked patch
   (`git apply --3way docs/patches/w2-gateway-consent-request-3694975.patch`
   — it applied cleanly as of 2026-07-03), update the pinned flat-403 test,
   and reconcile with the intent-envelope path so there is ONE consent story.

Until decided, the patch file is the single source of this work; the
`w2-consent-source` branch is safe to delete.
