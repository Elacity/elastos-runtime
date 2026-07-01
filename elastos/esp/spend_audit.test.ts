/**
 * ESP v0 — spend-budget + audit-chain projection conformance tests (W5b).
 *
 * Proves the two custody view-models are pure, fail-honest projections of the
 * runtime's signed facts: an unmetered capsule renders as unmetered (never a
 * satisfied 0/0), a hard-stop / drained budget renders as exhausted, an absent
 * chain renders as absent (neither pass nor fail), and a present-but-broken chain
 * renders as a tamper warning (never optimistically verified).
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import type { ChainAttestation } from "./ai_act_audit.js";
import {
  SPEND_WARNING_FRACTION,
  auditChainView,
  custodyDisplayRows,
  homeCustodyView,
  intentProofView,
  spendBudgetView,
  type BudgetSnapshotV1,
  type IntentProofSummaryV1,
} from "./spend_audit.js";

describe("spendBudgetView (adoption wedge #4)", () => {
  it("renders null as UNMETERED, never a satisfied 0/0", () => {
    const v = spendBudgetView(null);
    assert.equal(v.metered, false);
    assert.equal(v.state, "unmetered");
    assert.equal(v.exhausted, false);
    assert.equal(v.fractionUsed, 0);
    // undefined is treated identically (must-ignore-absent).
    assert.deepEqual(spendBudgetView(undefined), v);
  });

  it("projects a healthy budget as ok with the live fraction", () => {
    const snap: BudgetSnapshotV1 = { limit: 100, spent: 30, remaining: 70 };
    const v = spendBudgetView(snap);
    assert.equal(v.metered, true);
    assert.equal(v.state, "ok");
    assert.equal(v.remaining, 70);
    assert.equal(v.fractionUsed, 0.3);
    assert.equal(v.exhausted, false);
  });

  it("flags a near-limit budget as a warning at the threshold", () => {
    const at = spendBudgetView({ limit: 100, spent: 80, remaining: 20 });
    assert.equal(at.fractionUsed, SPEND_WARNING_FRACTION);
    assert.equal(at.state, "warning", "at the threshold the meter warns");
    const below = spendBudgetView({ limit: 100, spent: 79, remaining: 21 });
    assert.equal(below.state, "ok", "just below the threshold stays ok");
  });

  it("renders a drained budget as exhausted (fraction pinned to 1)", () => {
    const v = spendBudgetView({ limit: 100, spent: 100, remaining: 0 });
    assert.equal(v.state, "exhausted");
    assert.equal(v.exhausted, true);
    assert.equal(v.fractionUsed, 1);
  });

  it("renders a hard-stop budget (limit 0) as exhausted", () => {
    // ELASTOS_DEFAULT_SPEND_BUDGET=0 ⇒ every act is refused; the meter must read exhausted.
    const v = spendBudgetView({ limit: 0, spent: 0, remaining: 0 });
    assert.equal(v.metered, true);
    assert.equal(v.state, "exhausted");
    assert.equal(v.fractionUsed, 1);
  });

  it("never reports negative values even if a snapshot is malformed", () => {
    const v = spendBudgetView({ limit: 100, spent: 150, remaining: -50 });
    assert.equal(v.remaining, 0, "remaining floors at 0");
    assert.equal(v.fractionUsed, 1, "spent over limit clamps to 1");
    assert.equal(v.exhausted, true);
  });
});

describe("auditChainView (the flight recorder)", () => {
  const verified: ChainAttestation = { verified: true, records: 42, signer: "deadbeefkey", error: null };
  const broken: ChainAttestation = {
    verified: false,
    records: 0,
    signer: "deadbeefkey",
    error: "audit tamper at seq 7: record_hash mismatch (content edited)",
  };

  it("renders null as ABSENT — no durable chain, neither pass nor fail", () => {
    const v = auditChainView(null);
    assert.equal(v.present, false);
    assert.equal(v.state, "absent");
    assert.equal(v.verified, false, "absence is NOT verified");
    assert.equal(v.error, null, "absence is NOT a failure");
    assert.deepEqual(auditChainView(undefined), v);
  });

  it("renders a clean walk as verified, surfacing records + signer", () => {
    const v = auditChainView(verified);
    assert.equal(v.present, true);
    assert.equal(v.state, "verified");
    assert.equal(v.verified, true);
    assert.equal(v.records, 42);
    assert.equal(v.signer, "deadbeefkey");
  });

  it("renders a present-but-unverified chain as a BROKEN tamper warning", () => {
    const v = auditChainView(broken);
    assert.equal(v.present, true);
    assert.equal(v.state, "broken");
    assert.equal(v.verified, false);
    assert.ok((v.error ?? "").includes("tamper"), "the first break is surfaced, not hidden");
  });
});

describe("intentProofView (the prover/verifier custody channel)", () => {
  it("renders null as ABSENT — no intent-proof custody, neither pass nor fail", () => {
    const v = intentProofView(null);
    assert.equal(v.present, false);
    assert.equal(v.state, "absent");
    assert.equal(v.flagged, 0);
    assert.deepEqual(intentProofView(undefined), v);
  });

  it("renders an all-zero summary as CLEAN (only when present)", () => {
    const v = intentProofView({ denied: 0, diverged: 0, undelivered: 0 });
    assert.equal(v.present, true);
    assert.equal(v.state, "clean");
    assert.equal(v.flagged, 0);
    // Absent must NOT equal clean — absence is not a pass.
    assert.ok(v.state !== intentProofView(null).state, "absent must not equal clean");
  });

  it("flags ANY non-zero denied/diverged/undelivered (never masked)", () => {
    for (const summary of [
      { denied: 1, diverged: 0, undelivered: 0 },
      { denied: 0, diverged: 2, undelivered: 0 },
      { denied: 0, diverged: 0, undelivered: 3 },
    ] as IntentProofSummaryV1[]) {
      const v = intentProofView(summary);
      assert.equal(v.state, "flagged", `must flag ${JSON.stringify(summary)}`);
      assert.ok(v.flagged > 0);
    }
    const all = intentProofView({ denied: 1, diverged: 2, undelivered: 3 });
    assert.equal(all.flagged, 6, "flagged is the sum of the three honest counts");
  });

  it("floors negative counts at 0 (never a negative flag)", () => {
    const v = intentProofView({ denied: -5, diverged: 0, undelivered: 0 });
    assert.equal(v.denied, 0);
    assert.equal(v.state, "clean");
  });
});

describe("homeCustodyView (the Home capsule-detail custody panel)", () => {
  const verified: ChainAttestation = { verified: true, records: 42, signer: "deadbeefkey", error: null };
  const broken: ChainAttestation = {
    verified: false,
    records: 0,
    signer: "deadbeefkey",
    error: "audit tamper at seq 7: record_hash mismatch",
  };

  it("is a pure composition — each field is exactly its own projection", () => {
    const snap: BudgetSnapshotV1 = { limit: 100, spent: 30, remaining: 70 };
    const v = homeCustodyView(snap, verified);
    assert.deepEqual(v.spend, spendBudgetView(snap));
    assert.deepEqual(v.audit, auditChainView(verified));
    assert.deepEqual(v.intent, intentProofView(undefined));
    // No roll-up verdict field was invented (no new logic over the three projections).
    assert.deepEqual(Object.keys(v).sort(), ["audit", "intent", "spend"]);
  });

  it("the ONLY all-green panel is metered-ok + verified", () => {
    const v = homeCustodyView({ limit: 100, spent: 30, remaining: 70 }, verified);
    assert.equal(v.spend.state, "ok");
    assert.equal(v.audit.state, "verified");
  });

  it("unmetered + absent never reads as a satisfied/green panel", () => {
    const v = homeCustodyView(null, null);
    assert.equal(v.spend.state, "unmetered");
    assert.equal(v.spend.metered, false);
    assert.equal(v.audit.state, "absent");
    assert.equal(v.audit.present, false);
    // Absence on both channels must NOT masquerade as verified/ok.
    assert.ok(v.audit.state !== "verified", "absent chain must not read as verified");
    assert.ok(v.spend.state !== "ok", "unmetered spend must not read as ok");
  });

  it("a warning budget is carried through verbatim (not softened)", () => {
    const v = homeCustodyView({ limit: 100, spent: 90, remaining: 10 }, verified);
    assert.equal(v.spend.state, "warning");
    assert.equal(v.audit.state, "verified");
  });

  it("exhausted spend + broken chain surfaces BOTH alarms, never green", () => {
    const v = homeCustodyView({ limit: 100, spent: 100, remaining: 0 }, broken);
    assert.equal(v.spend.state, "exhausted");
    assert.equal(v.audit.state, "broken");
    assert.equal(v.audit.verified, false, "a tampered chain can never render verified on the panel");
  });

  it("the intent channel is independent — a flagged verdict sits beside a clean chain", () => {
    // verified chain + within-budget, BUT flagged intents ⇒ the intent alarm is NOT masked
    // by the two green channels (the moat: three independent channels).
    const v = homeCustodyView({ limit: 100, spent: 1, remaining: 99 }, verified, {
      denied: 2,
      diverged: 1,
      undelivered: 0,
    });
    assert.equal(v.spend.state, "ok");
    assert.equal(v.audit.state, "verified");
    assert.equal(v.intent.state, "flagged", "a flagged intent verdict is never masked by green spend/audit");
    assert.equal(v.intent.flagged, 3);
  });
});

describe("custodyDisplayRows (the one display contract for every shell)", () => {
  const verified: ChainAttestation = { verified: true, records: 42, signer: "k", error: null };
  const broken: ChainAttestation = { verified: false, records: 0, signer: "k", error: "tamper" };

  it("returns the three channels in fixed order with honest labels", () => {
    const rows = custodyDisplayRows(homeCustodyView(null, null));
    assert.deepEqual(rows.map((r) => r.channel), ["spend", "audit", "intent"]);
    // Absence renders as absence — never a green/pass affordance.
    assert.deepEqual(
      rows.map((r) => r.value),
      ["Unmetered", "No durable chain", "No agent-intent custody"],
    );
    assert.deepEqual(rows.map((r) => r.detail), [null, null, null], "no detail rows when nothing to show");
  });

  it("keeps the channels INDEPENDENT — a verified chain never masks an exhausted budget or flagged intent", () => {
    const rows = custodyDisplayRows(
      homeCustodyView({ limit: 5, spent: 5, remaining: 0 }, verified, { denied: 2, diverged: 1, undelivered: 0 }),
    );
    const by = Object.fromEntries(rows.map((r) => [r.channel, r]));
    assert.equal(by.spend.state, "exhausted");
    assert.equal(by.spend.value, "Budget exhausted");
    assert.equal(by.spend.detail, "5 / 5", "metered detail is surfaced");
    assert.equal(by.audit.state, "verified"); // green chain...
    assert.equal(by.audit.detail, "42 records");
    // ...does NOT soften the flagged intent beside it.
    assert.equal(by.intent.state, "flagged");
    assert.equal(by.intent.value, "Intents flagged");
    assert.equal(by.intent.detail, "2 denied · 1 diverged · 0 undelivered");
  });

  it("a broken chain reads tampered (never verified) and a clean intent reads clean only when present", () => {
    const rows = custodyDisplayRows(
      homeCustodyView({ limit: 100, spent: 1, remaining: 99 }, broken, { denied: 0, diverged: 0, undelivered: 0 }),
    );
    const by = Object.fromEntries(rows.map((r) => [r.channel, r]));
    assert.equal(by.audit.value, "Chain tampered");
    assert.equal(by.spend.value, "Within budget");
    assert.equal(by.intent.value, "Intents within grant", "present + no issues = clean");
    assert.equal(by.intent.detail, null, "clean has no flagged-count detail");
  });
});
