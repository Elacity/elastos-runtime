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

import type { ChainAttestation } from "./ai_act_audit.js";

// ─────────────────────────── Spend budget (adoption wedge #4) ────────────────
// Mirror of `primitives::spend::BudgetSnapshot` (elastos-runtime/src/primitives/
// spend.rs), projected by the capsule inspector as the `spend_budget` field keyed
// on `vm-{name}`. The inspector emits `null` for an unmetered capsule — the view
// MUST treat null as unmetered, never as an empty/zero budget.

/** The live per-capsule act budget. `SpendUnits` are non-negative integer counts. */
export interface BudgetSnapshotV1 {
  limit: number;
  spent: number;
  remaining: number;
}

export type SpendState = "unmetered" | "ok" | "warning" | "exhausted";

/** Render-ready view-model for the spend meter. */
export interface SpendBudgetView {
  /** A budget is being enforced for this capsule (the inspector projected a snapshot). */
  metered: boolean;
  limit: number;
  spent: number;
  remaining: number;
  /** `spent / limit`, clamped to [0,1]; 0 when unmetered, 1 on a hard-stop (limit 0). */
  fractionUsed: number;
  exhausted: boolean;
  state: SpendState;
}

/** Fraction of the budget at/above which the meter renders a warning (pure display threshold). */
export const SPEND_WARNING_FRACTION = 0.8;

/**
 * Project the inspector's `spend_budget` snapshot into a render-ready view-model.
 * `null`/`undefined` ⇒ UNMETERED (the capsule runs without a budget; the meter is
 * hidden, not shown as a satisfied 0/0). A metered snapshot reports its live
 * limit/spent/remaining; `exhausted` is `remaining <= 0` (so a hard-stop budget of
 * limit 0 reads as exhausted, matching `ELASTOS_DEFAULT_SPEND_BUDGET=0`).
 */
export function spendBudgetView(snapshot: BudgetSnapshotV1 | null | undefined): SpendBudgetView {
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
  const state: SpendState = exhausted
    ? "exhausted"
    : fractionUsed >= SPEND_WARNING_FRACTION
      ? "warning"
      : "ok";
  return { metered: true, limit, spent, remaining, fractionUsed, exhausted, state };
}

// ─────────────────────────── Audit chain (the flight recorder) ───────────────
// `ChainAttestation` mirrors `primitives::audit::ChainAttestation` (the live full
// hash+signature walk). The inspector's `audit_attestation` op / `audit.chain`
// field emits `null` for a memory-only plane (no durable chain to attest).

export type AuditChainState = "absent" | "verified" | "broken";

/** Render-ready view-model for the live custody-chain attestation. */
export interface AuditChainView {
  /** A durable chain attestation was available to project (memory-only plane ⇒ false). */
  present: boolean;
  verified: boolean;
  records: number;
  signer: string | null;
  /** The first break naming why verification failed; null when clean or absent. */
  error: string | null;
  state: AuditChainState;
}

/**
 * Project a live `ChainAttestation` into a render-ready badge view-model.
 * `null`/`undefined` ⇒ ABSENT (memory-only / not threaded) — rendered as "no
 * durable chain", neither a pass nor a failure. A present attestation that did NOT
 * verify renders as `broken` (a tamper warning surfacing the first break), never
 * optimistically as verified.
 */
export function auditChainView(chain: ChainAttestation | null | undefined): AuditChainView {
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
