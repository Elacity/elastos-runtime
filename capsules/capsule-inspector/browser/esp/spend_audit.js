/**
 * ESP v0 — read-only view-models for the two runtime custody facts the Home /
 * auditor surface paints: the per-capsule SPEND budget and the live AUDIT-chain
 * attestation. (W5b — the headless projection behind `<SpendBudgetMeter>` and
 * `<AuditChainBadge>`; the live Svelte paint is the browser lane.)
 *
 * Both are PURE projections of signed runtime state (Bret Victor's law): props in
 * are the runtime's serde shapes, pixels out render only what the runtime already
 * decided. Neither view holds a key/token, and neither re-derives authority. The
 * fail-honest defaults mirror the runtime exactly:
 *   - a `null` budget means UNMETERED — never "0 spent, all clear";
 *   - a `null` chain means NO DURABLE CHAIN TO ATTEST (memory-only plane) — never
 *     "verified", and never a failure either; absence is rendered as absence;
 *   - a present-but-unverified chain renders as a tamper warning, never optimistic.
 */
/** Fraction of the budget at/above which the meter renders a warning (pure display threshold). */
export const SPEND_WARNING_FRACTION = 0.8;
/**
 * Project the inspector's `spend_budget` snapshot into a render-ready view-model.
 * `null`/`undefined` ⇒ UNMETERED (the capsule runs without a budget; the meter is
 * hidden, not shown as a satisfied 0/0). A metered snapshot reports its live
 * limit/spent/remaining; `exhausted` is `remaining <= 0` (so a hard-stop budget of
 * limit 0 reads as exhausted, matching `ELASTOS_DEFAULT_SPEND_BUDGET=0`).
 */
export function spendBudgetView(snapshot) {
    if (snapshot === null || snapshot === undefined) {
        return {
            metered: false,
            limit: 0,
            spent: 0,
            remaining: 0,
            fractionUsed: 0,
            exhausted: false,
            state: "unmetered",
        };
    }
    const limit = Math.max(0, snapshot.limit);
    const spent = Math.max(0, snapshot.spent);
    const remaining = Math.max(0, snapshot.remaining);
    const fractionUsed = limit > 0 ? Math.min(1, spent / limit) : remaining <= 0 ? 1 : 0;
    const exhausted = remaining <= 0;
    const state = exhausted
        ? "exhausted"
        : fractionUsed >= SPEND_WARNING_FRACTION
            ? "warning"
            : "ok";
    return { metered: true, limit, spent, remaining, fractionUsed, exhausted, state };
}
/**
 * Project a live `ChainAttestation` into a render-ready badge view-model.
 * `null`/`undefined` ⇒ ABSENT (memory-only / not threaded) — rendered as "no
 * durable chain", neither a pass nor a failure. A present attestation that did NOT
 * verify renders as `broken` (a tamper warning surfacing the first break), never
 * optimistically as verified.
 */
export function auditChainView(chain) {
    if (chain === null || chain === undefined) {
        return { present: false, verified: false, records: 0, signer: null, error: null, state: "absent" };
    }
    return {
        present: true,
        verified: chain.verified,
        records: chain.records,
        signer: chain.signer,
        error: chain.error,
        state: chain.verified ? "verified" : "broken",
    };
}
/**
 * Project the inspector's intent-proof summary into a render-ready view-model.
 * `null`/`undefined` ⇒ ABSENT (no intent-proof custody for this capsule — neither a pass
 * nor a failure; absence is rendered as absence). A present summary with any non-zero
 * denied/diverged/undelivered ⇒ FLAGGED (an alarm); all-zero ⇒ CLEAN. Counts floor at 0.
 */
export function intentProofView(summary) {
    if (summary === null || summary === undefined) {
        return { present: false, denied: 0, diverged: 0, undelivered: 0, flagged: 0, state: "absent" };
    }
    const denied = Math.max(0, summary.denied);
    const diverged = Math.max(0, summary.diverged);
    const undelivered = Math.max(0, summary.undelivered);
    const flagged = denied + diverged + undelivered;
    return {
        present: true,
        denied,
        diverged,
        undelivered,
        flagged,
        state: flagged > 0 ? "flagged" : "clean",
    };
}
/**
 * Compose the inspector's `spend_budget` + `audit.chain` + intent-proof facts into the
 * Home capsule-detail custody view-model. Pure composition: each field is exactly its own
 * fail-honest projection — unmetered / exhausted spend, absent / broken chain, and
 * absent / flagged intent-proof are carried through verbatim, never masked by an
 * optimistic roll-up. `intentProof` is optional (absent when not supplied).
 */
export function homeCustodyView(spendBudget, auditChain, intentProof) {
    return {
        spend: spendBudgetView(spendBudget),
        audit: auditChainView(auditChain),
        intent: intentProofView(intentProof),
    };
}
// ─────────────────────────── The custody-panel DISPLAY contract ───────────────
// One tested source of truth for how the three custody channels render — so the Svelte
// `<CapsuleCustodyPanel>` and the vanilla-JS inspector paint IDENTICALLY (no per-shell
// label drift). Each row maps an already-decided honest state to a label + optional detail;
// it adds NO logic (the states come from `homeCustodyView`). There is no "all good" row —
// a channel can only ever show its own honest sub-state.
/** Honest display labels, one per fail-honest state. Absence is never dressed up as a pass. */
export const CUSTODY_SPEND_LABEL = {
    unmetered: "Unmetered",
    ok: "Within budget",
    warning: "Near budget limit",
    exhausted: "Budget exhausted",
};
export const CUSTODY_AUDIT_LABEL = {
    absent: "No durable chain",
    verified: "Chain verified",
    broken: "Chain tampered",
};
export const CUSTODY_INTENT_LABEL = {
    absent: "No agent-intent custody",
    clean: "Intents within grant",
    flagged: "Intents flagged",
};
/**
 * The three custody rows for a [`HomeCustodyView`], in fixed order (spend, audit, intent).
 * The single display contract both the Svelte panel and the vanilla inspector consume — so a
 * verified chain sitting beside an exhausted budget or a flagged intent is never masked, in
 * ANY shell. Detail rows appear only when meaningful (metered spend / present chain / flagged intent).
 */
export function custodyDisplayRows(view) {
    const { spend, audit, intent } = view;
    return [
        {
            channel: "spend",
            label: "Spend",
            state: spend.state,
            value: CUSTODY_SPEND_LABEL[spend.state],
            detail: spend.metered ? `${spend.spent} / ${spend.limit}` : null,
        },
        {
            channel: "audit",
            label: "Audit chain",
            state: audit.state,
            value: CUSTODY_AUDIT_LABEL[audit.state],
            detail: audit.present ? `${audit.records} records` : null,
        },
        {
            channel: "intent",
            label: "Agent intents",
            state: intent.state,
            value: CUSTODY_INTENT_LABEL[intent.state],
            detail: intent.flagged > 0
                ? `${intent.denied} denied · ${intent.diverged} diverged · ${intent.undelivered} undelivered`
                : null,
        },
    ];
}
