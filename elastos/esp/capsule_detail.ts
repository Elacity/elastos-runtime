/**
 * ESP v0 — the Home capsule-detail view-model (W5b).
 *
 * Composes the two independent read-only channels the detail surface shows for one
 * capsule: its TRUST material (is it real — `trustMaterial`) and its live CUSTODY
 * state (spend budget + audit chain — `homeCustodyView`). Pure composition: it adds
 * NO logic of its own (no blended "overall safe" verdict), so trust and custody stay
 * the two independent channels the moat depends on — a verified capsule can still
 * show an exhausted budget or a broken chain, and an unsigned one is never dressed up
 * by a clean custody panel.
 */

import type { ChainAttestation } from "./ai_act_audit.js";
import type { CapsuleSummary } from "./esp_v0.js";
import { homeCustodyView, type BudgetSnapshotV1, type HomeCustodyView } from "./spend_audit.js";
import { trustMaterial, type TrustMaterial } from "./two_channel.js";

/** Render-ready view-model for the Home capsule-detail surface. */
export interface CapsuleDetailView {
  name: string;
  title: string;
  /** Channel 1 — is it real (fail-honest: unknown trust_state ⇒ unsigned). */
  trust: TrustMaterial;
  /** Channel 2 — live custody (spend + audit), each fail-honest. */
  custody: HomeCustodyView;
}

/**
 * Compose a capsule's trust verdict + live custody facts into the detail view-model.
 * Pure composition — `trust` is `trustMaterial(capsule)` and `custody` is
 * `homeCustodyView(spend_budget, audit.chain)`, each carried through verbatim with no
 * cross-channel roll-up.
 */
export function capsuleDetailView(
  capsule: Pick<CapsuleSummary, "name" | "title" | "trust_state">,
  spendBudget: BudgetSnapshotV1 | null | undefined,
  auditChain: ChainAttestation | null | undefined,
): CapsuleDetailView {
  return {
    name: capsule.name,
    title: capsule.title,
    trust: trustMaterial(capsule),
    custody: homeCustodyView(spendBudget, auditChain),
  };
}
