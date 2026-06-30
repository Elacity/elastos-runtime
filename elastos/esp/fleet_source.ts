/**
 * ESP v0 — the Home fleet DATA PATH (W5b).
 *
 * Turns the runtime's two live sources into the `CapsuleFleetEntry[]` that `homeView`
 * projects, fail-honestly and with ONE trust classifier:
 *
 *   - TRUST comes from the CATALOG (`GET /api/capsules/catalog` →
 *     `CapsuleSummary.trust_state`), projected by `trustMaterial`. This is the single
 *     source of the trust verdict — we do NOT re-classify trust from the inspector.
 *   - CUSTODY (live spend budget + audit-chain attestation) comes from the per-capsule
 *     INSPECTOR projection (`inspect_provider.rs::project`), which uniquely owns these
 *     live facts. The inspector's own `identity.trust_level` uses a different
 *     vocabulary ("signed"/"content-addressed") and is deliberately IGNORED here.
 *
 * The join is keyed on the capsule `name` (== the inspector's `data.name`, and the
 * `vm-{name}` the spend meter / audit chain key on). A capsule with no inspector
 * custody yet (not running / not inspected) is fail-honest: `null` spend ⇒ unmetered,
 * `null` chain ⇒ absent — never a fabricated all-clear.
 *
 * Fail-honest extraction (contract-drift defense — the conformance test pins the real
 * inspector shape, so drift is caught at test time, never silently in prod):
 *   - a well-formed `{limit, spent, remaining}` ⇒ metered; anything else ⇒ `null`
 *     (unmetered), matching the runtime's own null-budget contract;
 *   - an ABSENT `audit.chain` ⇒ `null` (absent, neither pass nor fail); a PRESENT but
 *     unparseable attestation ⇒ a BROKEN attestation (present-but-unverifiable is an
 *     alarm, never dressed up as absent — mirrors `auditChainView`'s present+!verified).
 */

import type { ChainAttestation } from "./ai_act_audit.js";
import type { CapsuleSummary } from "./esp_v0.js";
import type { CapsuleFleetEntry } from "./home.js";
import type { BudgetSnapshotV1 } from "./spend_audit.js";

type Json = Record<string, unknown>;

function isRecord(v: unknown): v is Json {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}
function num(v: unknown): number | null {
  return typeof v === "number" && Number.isFinite(v) ? v : null;
}

/** The live custody facts the Home fleet reads from one capsule's inspector projection. */
export interface InspectCustody {
  /** The capsule name (`data.name`) — the join key onto the catalog. */
  name: string;
  /** Live spend budget, or `null` (unmetered / absent / unparseable). */
  spend: BudgetSnapshotV1 | null;
  /** Live audit-chain attestation, or `null` (absent / memory-only). */
  audit: ChainAttestation | null;
}

/**
 * Extract a `BudgetSnapshotV1` from the inspector's `data.spend_budget`. Only a
 * well-formed `{limit, spent, remaining}` of finite numbers is treated as metered;
 * `null`/absent/malformed ⇒ `null` (unmetered) — matching the runtime, which emits
 * `null` for an unmetered capsule. (`spendBudgetView` then clamps the values.)
 */
export function spendFromInspect(spendBudget: unknown): BudgetSnapshotV1 | null {
  if (!isRecord(spendBudget)) return null;
  const limit = num(spendBudget.limit);
  const spent = num(spendBudget.spent);
  const remaining = num(spendBudget.remaining);
  if (limit === null || spent === null || remaining === null) return null;
  return { limit, spent, remaining };
}

/**
 * Extract a `ChainAttestation` from the inspector's `data.audit` SECTION
 * (`{counts, chain, recent}`). An absent/`null` `chain` ⇒ `null` (absent — memory-only
 * plane, neither pass nor fail). A PRESENT `chain` object missing a boolean `verified`
 * is contract-drift / corruption: surfaced as a BROKEN attestation (never absent, never
 * verified). A well-formed attestation is carried through verbatim (unknown extra
 * fields ignored, per the ESP must-ignore-unknown rule).
 */
export function chainFromAudit(auditSection: unknown): ChainAttestation | null {
  if (!isRecord(auditSection)) return null;
  const chain = auditSection.chain;
  if (chain === null || chain === undefined) return null; // memory-only ⇒ absent
  if (!isRecord(chain) || typeof chain.verified !== "boolean") {
    // Present but unparseable ⇒ broken (an attestation we cannot verify is an alarm).
    return { verified: false, records: 0, signer: null, error: "unparseable attestation" };
  }
  return {
    verified: chain.verified,
    records: num(chain.records) ?? 0,
    signer: typeof chain.signer === "string" ? chain.signer : null,
    error: typeof chain.error === "string" ? chain.error : null,
  };
}

/**
 * Build the live custody record from one capsule's inspector projection `data`
 * (the object inside `{ status: "ok", data: {...} }`). Returns `null` only when the
 * projection has no usable `name` to join on.
 */
export function inspectCustody(data: unknown): InspectCustody | null {
  if (!isRecord(data) || typeof data.name !== "string") return null;
  return {
    name: data.name,
    spend: spendFromInspect(data.spend_budget),
    audit: chainFromAudit(data.audit),
  };
}

/**
 * Join the catalog fleet (the trust source) with per-capsule inspector custody into
 * the `CapsuleFleetEntry[]` `homeView` consumes. Order follows the catalog (never
 * reordered by health). A capsule with no custody entry is fail-honest: `null` spend
 * (unmetered) + `null` chain (absent).
 */
export function fleetEntries(
  catalog: ReadonlyArray<Pick<CapsuleSummary, "name" | "title" | "trust_state">>,
  custodyByName: ReadonlyMap<string, InspectCustody>,
): CapsuleFleetEntry[] {
  return catalog.map((c) => {
    const custody = custodyByName.get(c.name);
    return {
      capsule: { name: c.name, title: c.title, trust_state: c.trust_state },
      spendBudget: custody ? custody.spend : null,
      auditChain: custody ? custody.audit : null,
    };
  });
}

/**
 * Convenience: build the custody-by-name map from a list of raw inspector projection
 * `data` objects (e.g. one per capsule the client inspected). Projections without a
 * usable `name` are dropped (they cannot be joined). A later projection for the same
 * name overwrites an earlier one (last-write-wins, like the runtime's own keying).
 */
export function custodyMap(projections: ReadonlyArray<unknown>): Map<string, InspectCustody> {
  const m = new Map<string, InspectCustody>();
  for (const p of projections) {
    const c = inspectCustody(p);
    if (c) m.set(c.name, c);
  }
  return m;
}
