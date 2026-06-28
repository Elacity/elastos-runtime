/**
 * ESP v0 — the two-channel object (the "never-seen" moment), as projection logic.
 *
 * Every "Allow?" ever built conflates two questions — *is it real?* and *how far
 * does it reach?* KEEP splits them into TWO channels rendered from REAL runtime
 * verdicts:
 *
 *   Channel 1 — TRUST-MATERIAL (is it real): projected from the runtime's
 *               `trust_state` verdict (signed / content-addressed / unsigned).
 *   Channel 2 — BLAST-RADIUS (how far it reaches): projected from the
 *               core-computed `ReachDescriptorV1` (egress / scope / reversibility
 *               / isolation), honest about `observed`.
 *
 * The gasp: a VERIFIED capsule can be visibly MORE dangerous than an UNSIGNED
 * one. This module is pure projection — it holds no key and re-derives no crypto;
 * it renders the verdicts the core already proved (ESP read-only principle).
 */

import type { AffordanceReachView, CapsuleSummary, ReachDescriptorV1 } from "./esp_v0.js";

// ─────────────────────────── Channel 1: trust-material ──────────────────────

/** Is it real? — projected from the runtime's trust verdict, never re-derived. */
export type TrustMaterial = "verified" | "content_addressed" | "unsigned";

/**
 * Project the trust-material channel from a capsule's runtime trust verdict.
 * Fail-honest: an unknown `trust_state` reads as `unsigned` — we never over-trust
 * a verdict we do not recognise.
 */
export function trustMaterial(capsule: Pick<CapsuleSummary, "trust_state">): TrustMaterial {
  switch (capsule.trust_state) {
    case "cid-with-manifest-signature":
    case "local-manifest-signature":
      return "verified";
    case "cid-without-manifest-signature":
      return "content_addressed";
    case "local-dev":
      return "unsigned";
    default:
      return "unsigned";
  }
}

// ─────────────────────────── Channel 2: blast-radius ────────────────────────

/** How hot the act reads. */
export type HazardLevel = "cool" | "warm" | "hot";

export interface BlastRadius {
  level: HazardLevel;
  /** Plain-language reasons the band is what it is (for the halo's tooltip). */
  reasons: string[];
  /** True when a reach dimension could not be pinned (`observed === false`): the
   *  halo must render INCOMPLETE, never a falsely-cool reading. */
  incomplete: boolean;
}

/** Numeric rank so callers (and tests) can ORDER hazard bands. */
export function hazardRank(level: HazardLevel): number {
  return level === "hot" ? 2 : level === "warm" ? 1 : 0;
}

/**
 * Project the blast-radius channel from the core-computed reach. A leashed
 * (allowlisted) egress reads cooler than an open one — the whole point of W1.
 */
export function blastRadius(reach: ReachDescriptorV1): BlastRadius {
  const reasons: string[] = [];
  let score = 0;

  if (reach.egress === "open") {
    // Unrestricted internet egress is high-hazard on its own.
    score += 3;
    reasons.push("open network egress");
  } else if (reach.egress === "allowlisted") {
    score += 1;
    reasons.push("leashed (allowlisted) network egress");
  }

  if (reach.scope === "system") {
    score += 2;
    reasons.push("system-wide scope");
  } else if (reach.scope === "collection") {
    score += 1;
    reasons.push("collection scope");
  }

  if (reach.reversibility === "one_way") {
    score += 2;
    reasons.push("irreversible (one-way)");
  }

  if (reach.isolation === "host_process") {
    score += 1;
    reasons.push("host-process isolation");
  }

  const level: HazardLevel = score >= 3 ? "hot" : score >= 1 ? "warm" : "cool";
  if (level === "cool") {
    reasons.push("contained");
  }
  return { level, reasons, incomplete: !reach.observed };
}

// ─────────────────────────── The two-channel object ─────────────────────────

export interface TwoChannelObject {
  /** Channel 1 — is it real. */
  trust: TrustMaterial;
  /** Channel 2 — how far it reaches. */
  blast: BlastRadius;
  /**
   * The never-seen contradiction: a thing you were TRAINED to trust (verified)
   * that nonetheless reaches dangerously far. When true, the UI lets you refuse
   * the thing the green checkmark told you to accept.
   */
  refuseTrained: boolean;
  /** The capsule claimed low risk but the core-observed reach is far. */
  declaredUnderstatesReach: boolean;
}

/**
 * Compose the two channels for one affordance. `trust` is the capsule's verdict
 * (Channel 1); `view` carries the core-computed reach (Channel 2) and the
 * declared-understates flag.
 */
export function twoChannel(trust: TrustMaterial, view: AffordanceReachView): TwoChannelObject {
  const blast = blastRadius(view.reach);
  return {
    trust,
    blast,
    refuseTrained: trust === "verified" && blast.level === "hot",
    declaredUnderstatesReach: view.declared_understates_reach,
  };
}
