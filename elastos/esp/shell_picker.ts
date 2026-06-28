/**
 * ESP v0 — the shell-picker (W6), as pure projection logic.
 *
 * "The one shell" became "a shell among shells" (W3). The picker is the
 * read-only projection that lets a user choose WHICH shell is active: it
 * enumerates the catalog capsules that are *eligible* to be the active shell and
 * renders an honest Trust Card for each (reusing the two-channel trust-material).
 *
 * Fail-closed, mirroring the runtime's `shell_token_eligible`: a capsule is
 * selectable ONLY if it holds the `shell` role AND is launchable. The picker only
 * RENDERS and REQUESTS — the runtime's `set_active_shell` is the sole thing that
 * actually switches authority (ESP read-only principle).
 */

import type { CapsuleCatalogResponse, CapsuleSummary } from "./esp_v0.js";
import { trustMaterial } from "./two_channel.js";
import type { TrustMaterial } from "./two_channel.js";

/**
 * Whether a capsule may be selected as the active shell. The ESP mirror of the
 * runtime's `shell_token_eligible`: a non-`shell` capsule can NEVER be the active
 * shell, even if it is launchable.
 */
export function isShellSelectable(capsule: Pick<CapsuleSummary, "role" | "launchable">): boolean {
  return capsule.role === "shell" && capsule.launchable === true;
}

/** The catalog capsules eligible to be the active shell. */
export function selectableShells(catalog: CapsuleCatalogResponse): CapsuleSummary[] {
  return catalog.capsules.filter(isShellSelectable);
}

/** An honest at-a-glance card for one shell — its name + its trust verdict. */
export interface ShellTrustCard {
  name: string;
  title: string;
  /** The shell's own trust-material (Channel 1) — verified / content-addressed /
   *  unsigned. A user picks a shell knowing how trustworthy it is. */
  trust: TrustMaterial;
}

export function shellTrustCard(capsule: CapsuleSummary): ShellTrustCard {
  return { name: capsule.name, title: capsule.title, trust: trustMaterial(capsule) };
}

export interface ShellPicker {
  /** Every selectable shell, with its Trust Card. */
  shells: ShellTrustCard[];
  /** The currently active shell — always one of `shells` (or "" if none). */
  active: string;
}

/**
 * Build a picker from a catalog. `active` defaults to the first selectable shell;
 * a caller-provided `active` is honoured only if it is itself selectable
 * (fail-closed — a stale or non-shell pointer never sticks).
 */
export function shellPicker(catalog: CapsuleCatalogResponse, active?: string): ShellPicker {
  const cards = selectableShells(catalog).map(shellTrustCard);
  const names = new Set(cards.map((c) => c.name));
  const chosen = active !== undefined && names.has(active) ? active : (cards[0]?.name ?? "");
  return { shells: cards, active: chosen };
}

/**
 * Select a new active shell. Fail-closed: returns `null` if `name` is not a
 * selectable shell in this picker, so a non-shell can never become active.
 */
export function withActiveShell(picker: ShellPicker, name: string): ShellPicker | null {
  if (!picker.shells.some((c) => c.name === name)) {
    return null;
  }
  return { ...picker, active: name };
}
