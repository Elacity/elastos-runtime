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
  spendBudgetView,
  type BudgetSnapshotV1,
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
