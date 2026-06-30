/**
 * ESP v0 — Home fleet data-path conformance (W5b).
 *
 * Pins the adapter to the REAL inspector projection shape (inspect_provider.rs
 * `project`: `data.name`, `data.spend_budget` = {limit,spent,remaining}|null,
 * `data.audit.chain` = serialized ChainAttestation|null) and proves:
 *   - TRUST is taken from the catalog (not the inspector's divergent `trust_level`);
 *   - custody is joined by name, fail-honestly (missing ⇒ unmetered + absent);
 *   - a malformed budget ⇒ unmetered, an absent chain ⇒ absent, a present-but-
 *     unparseable chain ⇒ broken (never dressed up as absent or verified);
 *   - unknown extra fields are ignored; order follows the catalog.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  chainFromAudit,
  custodyMap,
  fleetEntries,
  inspectCustody,
  spendFromInspect,
} from "./fleet_source.js";
import { homeCapsules, homeView } from "./home.js";

// A faithful slice of inspect_provider.rs `project()` output for one capsule. Carries
// the inspector's OWN trust_level ("signed" — a vocabulary we must IGNORE) and extra
// fields (must-ignore-unknown), so the test pins behaviour against the real shape.
function inspectData(over: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    id: "vm-player",
    name: "vm-player",
    identity: { did: null, cid: null, trust_level: "signed", signature_present: true },
    spend_budget: { limit: 100, spent: 30, remaining: 70 },
    audit: {
      counts: { total: 9, denied: 0, attested: 9 },
      chain: { verified: true, records: 9, signer: "k", error: null },
      recent: [],
    },
    processes: [{ kind: "micro_vm", status: "running" }],
    ...over,
  };
}

describe("spendFromInspect (fail-honest budget extraction)", () => {
  it("extracts a well-formed {limit,spent,remaining}", () => {
    assert.deepEqual(spendFromInspect({ limit: 5, spent: 5, remaining: 0 }), { limit: 5, spent: 5, remaining: 0 });
  });
  it("null / absent ⇒ null (unmetered)", () => {
    assert.equal(spendFromInspect(null), null);
    assert.equal(spendFromInspect(undefined), null);
  });
  it("malformed (a missing/non-numeric field) ⇒ null (unmetered), never NaN", () => {
    assert.equal(spendFromInspect({ limit: 100, spent: 30 }), null, "missing remaining");
    assert.equal(spendFromInspect({ limit: "100", spent: 30, remaining: 70 }), null, "non-numeric limit");
  });
});

describe("chainFromAudit (fail-honest attestation extraction)", () => {
  it("carries a well-formed attestation through (ignoring unknown extra fields)", () => {
    const att = chainFromAudit({ chain: { verified: true, records: 9, signer: "k", error: null, extra: "x" } });
    assert.deepEqual(att, { verified: true, records: 9, signer: "k", error: null });
  });
  it("an absent chain ⇒ null (absent, neither pass nor fail)", () => {
    assert.equal(chainFromAudit({ chain: null }), null);
    assert.equal(chainFromAudit({ counts: {} }), null, "no chain key");
    assert.equal(chainFromAudit(null), null, "no audit section");
  });
  it("a broken attestation is carried as broken, never absent", () => {
    const att = chainFromAudit({ chain: { verified: false, records: 0, signer: "k", error: "tamper" } });
    assert.deepEqual(att, { verified: false, records: 0, signer: "k", error: "tamper" });
  });
  it("a PRESENT but unparseable chain ⇒ broken (alarm), never absent or verified", () => {
    const att = chainFromAudit({ chain: { records: 9 } }); // no boolean `verified`
    assert.equal(att?.verified, false);
    assert.equal(att?.error, "unparseable attestation");
  });
});

describe("inspectCustody / custodyMap", () => {
  it("extracts name + custody from a real projection, ignoring the inspector's own trust_level", () => {
    const c = inspectCustody(inspectData());
    assert.equal(c?.name, "vm-player");
    assert.deepEqual(c?.spend, { limit: 100, spent: 30, remaining: 70 });
    assert.equal(c?.audit?.verified, true);
    // The inspector's identity.trust_level ("signed") is NOT carried — trust is the catalog's job.
    assert.equal("trust" in (c as object), false);
  });
  it("a projection with no usable name ⇒ null (cannot be joined)", () => {
    assert.equal(inspectCustody({ spend_budget: null }), null);
    assert.equal(inspectCustody(null), null);
  });
  it("custodyMap keys by name, drops nameless, last-write-wins", () => {
    const m = custodyMap([
      inspectData({ name: "vm-a", spend_budget: { limit: 1, spent: 0, remaining: 1 } }),
      { spend_budget: null }, // dropped (no name)
      inspectData({ name: "vm-a", spend_budget: { limit: 9, spent: 0, remaining: 9 } }), // overwrites
    ]);
    assert.equal(m.size, 1);
    assert.deepEqual(m.get("vm-a")?.spend, { limit: 9, spent: 0, remaining: 9 });
  });
});

describe("fleetEntries (catalog ⨝ inspector custody)", () => {
  const catalog = [
    { name: "vm-a", title: "A", trust_state: "cid-with-manifest-signature" },
    { name: "vm-b", title: "B", trust_state: "local-dev" },
    { name: "vm-c", title: "C", trust_state: "cid-with-manifest-signature" }, // no custody → fail-honest
  ];

  it("joins by name, keeps catalog order, trust comes from the catalog", () => {
    const custody = custodyMap([
      inspectData({ name: "vm-a", spend_budget: { limit: 5, spent: 5, remaining: 0 }, audit: { chain: { verified: false, records: 0, signer: "k", error: "tamper" } } }),
      inspectData({ name: "vm-b", spend_budget: { limit: 100, spent: 10, remaining: 90 }, audit: { chain: { verified: true, records: 3, signer: "k", error: null } } }),
    ]);
    const entries = fleetEntries(catalog, custody);
    assert.deepEqual(entries.map((e) => e.capsule.name), ["vm-a", "vm-b", "vm-c"], "catalog order preserved");
    assert.equal(entries[0].capsule.trust_state, "cid-with-manifest-signature", "trust from catalog");
    assert.deepEqual(entries[0].spendBudget, { limit: 5, spent: 5, remaining: 0 });
    assert.equal(entries[0].auditChain?.verified, false);
    // vm-c has no inspector custody yet ⇒ fail-honest null/null (unmetered + absent).
    assert.equal(entries[2].spendBudget, null);
    assert.equal(entries[2].auditChain, null);
  });

  it("scoping to Home capsules first drops infra noise from the live attention count", () => {
    // The live finding: a full catalog of mostly-unsigned providers read as "44 of 45 need
    // attention". Scoping to the user-facing set (homeCapsules) BEFORE the join means the
    // headline reflects only user capsules — exactly one app here, honestly un-flagged.
    const fullCatalog = [
      { name: "vm-act", title: "Act", trust_state: "cid-with-manifest-signature", role: "app" },
      ...Array.from({ length: 20 }, (_, i) => ({
        name: `prov-${i}`,
        title: `Provider ${i}`,
        trust_state: "local-dev", // unsigned infra — each would trip attention if shown
        role: "provider",
      })),
    ];
    const scoped = homeCapsules(fullCatalog);
    const custody = custodyMap([
      inspectData({ name: "vm-act", spend_budget: { limit: 100, spent: 1, remaining: 99 }, audit: { chain: { verified: true, records: 3, signer: "k", error: null } } }),
    ]);
    const view = homeView(fleetEntries(scoped, custody));
    assert.equal(view.total, 1, "only the user-facing app remains on Home");
    assert.equal(view.needsAttention, 0, "no infra providers left to cry wolf over");
  });

  it("end-to-end: fleetEntries → homeView attention reflects the joined honest states", () => {
    const custody = custodyMap([
      // vm-a: verified trust (catalog) BUT exhausted + tampered ⇒ attention
      inspectData({ name: "vm-a", spend_budget: { limit: 5, spent: 5, remaining: 0 }, audit: { chain: { verified: false, records: 0, signer: "k", error: "tamper" } } }),
      // vm-b: unsigned trust (catalog) ⇒ attention regardless of clean custody
      inspectData({ name: "vm-b", spend_budget: { limit: 100, spent: 10, remaining: 90 }, audit: { chain: { verified: true, records: 3, signer: "k", error: null } } }),
    ]);
    const view = homeView(fleetEntries(catalog, custody));
    assert.equal(view.total, 3);
    // vm-a (exhausted+broken), vm-b (unsigned), vm-c (no chain ⇒ absent, trust verified, unmetered)
    // → vm-a and vm-b are wrong; vm-c is honest-intermediate (absent is not "wrong"). attention = 2.
    assert.equal(view.needsAttention, 2);
  });
});
