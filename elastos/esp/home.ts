/**
 * ESP v0 — the Home FLEET view-model (W5b).
 *
 * The Home surface is the landing view: a LIST of capsules, each painted as the same
 * two independent channels the detail surface uses (trust + custody). This composes
 * `capsuleDetailView` over the whole fleet and adds exactly ONE derived figure — an
 * `attention` count — chosen so it can only ever draw the eye TOWARD a problem, never
 * reassure over one.
 *
 * The moat at fleet scale (Bret Victor's law, fleet edition): there is deliberately NO
 * blended "all systems green" verdict. A fleet of one verified-but-exhausted capsule
 * next to one unsigned-but-clean capsule renders BOTH honest states side by side; no
 * roll-up can paint the Home green while a capsule is unsigned, exhausted, or its chain
 * is broken. The only summary is `needsAttention`, a monotonic-toward-caution count of
 * the unambiguously-wrong sub-states — so the summary's "0" means literally "no capsule
 * is in a wrong state", derived from the honest per-capsule projections, never an
 * independent optimistic flag.
 */

import type { ChainAttestation } from "./ai_act_audit.js";
import { capsuleDetailView, type CapsuleDetailView } from "./capsule_detail.js";
import type { CapsuleSummary } from "./esp_v0.js";
import type { BudgetSnapshotV1 } from "./spend_audit.js";

/** One capsule's live runtime facts, as the inspector projects them per capsule. */
export interface CapsuleFleetEntry {
  capsule: Pick<CapsuleSummary, "name" | "title" | "trust_state">;
  spendBudget: BudgetSnapshotV1 | null | undefined;
  auditChain: ChainAttestation | null | undefined;
}

/** Render-ready view-model for the Home fleet surface. */
export interface HomeView {
  /** One detail view-model per capsule, in input order (no reordering by "health"). */
  capsules: CapsuleDetailView[];
  /** Total capsules in the fleet. */
  total: number;
  /** Count of capsules in an unambiguously-wrong state (see `capsuleNeedsAttention`). */
  needsAttention: number;
}

/**
 * Capsule roles that belong on the USER-FACING Home — the shell-launchable set, mirroring
 * the runtime's `CapsuleRole::is_shell_launchable` (`shell | app | viewer`). Infrastructure
 * roles (`provider`, `content`) are operator / data surfaces, not Home: they are never
 * signed by a third-party author, so showing them on Home as "unsigned → needs attention"
 * cries wolf over what is by-design internal machinery. Excluding them keeps Home a
 * truthful view of what a user actually launches.
 */
const HOME_ROLES: ReadonlySet<string> = new Set(["shell", "app", "viewer"]);

/** Whether a capsule role belongs on the user-facing Home (see [`HOME_ROLES`]). */
export function isHomeCapsule(role: string): boolean {
  return HOME_ROLES.has(role);
}

/**
 * Scope a catalog down to the user-facing Home set (drops infrastructure providers and
 * content). Pure filter, input order preserved — the Home renders what a user launches,
 * not the runtime's internal provider capsules. Apply BEFORE `fleetEntries` so the
 * attention count reflects only user-facing capsules.
 */
export function homeCapsules<T extends { role: string }>(capsules: ReadonlyArray<T>): T[] {
  return capsules.filter((c) => isHomeCapsule(c.role));
}

/**
 * Whether a capsule is in an unambiguously-wrong custody/trust state that an auditor
 * should be drawn to. This is a pure DISPLAY policy (like `SPEND_WARNING_FRACTION`):
 * it flags only the three states that mean "something is wrong or unprovable" —
 *   - `unsigned` trust (not signature-verified),
 *   - `exhausted` spend (hit the hard budget stop),
 *   - `broken` audit chain (present but failed verification / tampered).
 * It deliberately does NOT flag the honest INTERMEDIATE states — `content_addressed`
 * trust, `warning` spend, or `absent` audit (absence is neither a pass nor a failure).
 * Each capsule still paints its own honest sub-state regardless; this only governs the
 * attention COUNT, never what a row renders.
 */
export function capsuleNeedsAttention(view: CapsuleDetailView): boolean {
  return (
    view.trust === "unsigned" ||
    view.custody.spend.state === "exhausted" ||
    view.custody.audit.state === "broken"
  );
}

/**
 * Project a fleet of per-capsule runtime facts into the Home view-model. Pure
 * composition over `capsuleDetailView` (each capsule carried through verbatim, in
 * order) plus the `needsAttention` count derived from the honest per-capsule states.
 * No cross-capsule roll-up: the only fleet-level figure is a count that can grow but
 * never manufacture reassurance.
 */
export function homeView(fleet: ReadonlyArray<CapsuleFleetEntry>): HomeView {
  const capsules = fleet.map((e) => capsuleDetailView(e.capsule, e.spendBudget, e.auditChain));
  const needsAttention = capsules.reduce((n, v) => n + (capsuleNeedsAttention(v) ? 1 : 0), 0);
  return { capsules, total: capsules.length, needsAttention };
}
