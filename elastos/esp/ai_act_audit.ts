/**
 * ESP v0 — the flywheel's first turn (W7): the signed receipt, re-projected as
 * the enterprise containment-audit artifact.
 *
 * The SAME `AffordanceGrantReceiptV1` that delights the consumer IS the
 * compliance evidence a regulator/insurer forces. This module maps a redemption
 * (the consent context + the signed receipt) onto the controls an auditor checks:
 *
 *   - EU AI Act **Art 12** (record-keeping): an automatic, tamper-evident record
 *     of the agent action — the ed25519-signed receipt.
 *   - EU AI Act **Art 14** (human oversight): a high-risk or user-approval act
 *     required explicit human consent before it executed.
 *
 * Pure projection — it asserts the evidence is PRESENT (signed record + matching
 * oversight); the cryptographic verification is the runtime's job.
 */

import type { AffordanceConsentPending, AffordanceGrantReceiptV1 } from "./esp_v0.js";

export const AI_ACT_AUDIT_SCHEMA_V1 = "elastos.audit.ai-act.v1";

/**
 * Mirror of the runtime's `primitives::audit::ChainAttestation` (serde): a LIVE full hash+signature
 * walk of the custody log backing this evidence. The exported artifact embeds this so a consumer
 * can see the chain verified — `records` long, under `signer` — rather than trusting the export.
 */
export interface ChainAttestation {
  /** The full hash + signature chain walked clean end to end. */
  verified: boolean;
  /** Records verified (chain length) on a clean walk; 0 on failure. */
  records: number;
  /** The signing key (hex) the chain verifies under, when signed. */
  signer: string | null;
  /** The first break naming why verification failed; null when clean. */
  error: string | null;
}

/** Risk classes that require a human in the loop regardless of approval mode. */
const HIGH_RISK: ReadonlySet<string> = new Set(["payment", "rights", "actuator", "privileged"]);

/** The consent context an audit record needs (a subset of the 202 consent fact). */
export type ConsentContext = Pick<
  AffordanceConsentPending,
  "approval" | "principal_id" | "request_id" | "risk"
>;

export interface AiActAuditRecordV1 {
  schema: typeof AI_ACT_AUDIT_SCHEMA_V1;
  /** What the agent did, in the user's name (from the signed receipt). */
  act: {
    capsule: string;
    method_id: string;
    resource: string;
    action: string;
    input_hash: string;
  };
  /** Human oversight (EU AI Act Art 14). */
  human_oversight: {
    required: boolean;
    mechanism: "user-consent" | "runtime-policy";
    principal: string;
    request_id: string;
  };
  /** Record-keeping (EU AI Act Art 12): the tamper-evident signed proof. */
  record_keeping: {
    token_id: string;
    signed: boolean;
    signer: string;
    signature: string;
    recorded_at: unknown;
    /**
     * Live integrity of the custody chain backing this record, when the export had access to a
     * file-backed audit plane. `null` when unavailable (memory-only plane / not threaded) — absence
     * is NOT a pass; it just falls back to the signed-record check (see `containmentEvidence`).
     */
    chain_attestation: ChainAttestation | null;
  };
  /** Plain-language mapping of each fact to the control it satisfies. */
  controls: {
    article_12_logging: string;
    article_14_oversight: string;
    soc2_for_agents: string;
  };
}

/**
 * Project a redemption (consent context + signed receipt) into a containment-audit
 * record. `required` human oversight is true for a user-approval act OR any
 * high-risk class; `signed` is true only when both a signer and a signature are
 * present on the receipt.
 */
export function toAiActAuditRecord(
  consent: ConsentContext,
  receipt: AffordanceGrantReceiptV1,
  chain: ChainAttestation | null = null,
): AiActAuditRecordV1 {
  const requiresHuman = consent.approval === "user" || HIGH_RISK.has(consent.risk);
  const signed = receipt.signer.length > 0 && receipt.signature.length > 0;
  return {
    schema: AI_ACT_AUDIT_SCHEMA_V1,
    act: {
      capsule: receipt.capsule,
      method_id: receipt.method_id,
      resource: receipt.resource,
      action: receipt.action,
      input_hash: receipt.input_hash,
    },
    human_oversight: {
      required: requiresHuman,
      mechanism: consent.approval === "user" ? "user-consent" : "runtime-policy",
      principal: consent.principal_id,
      request_id: consent.request_id,
    },
    record_keeping: {
      token_id: receipt.token_id,
      signed,
      signer: receipt.signer,
      signature: receipt.signature,
      recorded_at: receipt.redeemed_at,
      chain_attestation: chain,
    },
    controls: {
      article_12_logging:
        "EU AI Act Art 12 — automatic, tamper-evident record of the agent action (the ed25519-signed receipt).",
      article_14_oversight:
        "EU AI Act Art 14 — human-in-the-loop: a high-risk or user-approval act required explicit human consent before execution.",
      soc2_for_agents:
        "Containment evidence: the act was gated, scoped (single-use, bound), and signed — provably stayed in its lane.",
    },
  };
}

export interface ContainmentEvidence {
  /** A tamper-evident signed record exists AND its custody chain verified (Art 12). */
  article_12_met: boolean;
  /** Where human oversight was required, a human actually consented (Art 14). */
  article_14_met: boolean;
  /**
   * The custody chain backing the record verified live. `true` when no attestation was available
   * (memory-only / not threaded) — absence is not a failure, but a PRESENT-and-broken chain is.
   */
  chain_intact: boolean;
  /** Both controls satisfied — the act is provably contained. */
  contained: boolean;
}

/**
 * Whether the audit record demonstrates containment. Fail-closed at the audit
 * layer: an unsigned record fails Art 12 (no receipt → no provable act), a
 * PRESENT-but-broken custody chain also fails Art 12 (the tamper-evident record
 * is compromised — a tampered chain cannot back the evidence), and a
 * high-risk/user-approval act executed without human consent fails Art 14.
 */
export function containmentEvidence(record: AiActAuditRecordV1): ContainmentEvidence {
  const chain = record.record_keeping.chain_attestation;
  // Absent attestation (null) ⇒ fall back to the signed-record check (memory-only / not threaded);
  // a PRESENT attestation that did not verify breaks the tamper-evidence and fails Art 12.
  const chainIntact = chain === null || chain.verified;
  const article12Met = record.record_keeping.signed && chainIntact;
  const article14Met =
    !record.human_oversight.required || record.human_oversight.mechanism === "user-consent";
  return {
    article_12_met: article12Met,
    article_14_met: article14Met,
    chain_intact: chainIntact,
    contained: article12Met && article14Met,
  };
}
