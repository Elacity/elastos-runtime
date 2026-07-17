import type { CapsuleCatalogResponse, CapsuleSummary } from "./esp_v0.ts";
import {
  trustMaterial,
  type TrustMaterial,
  type VerificationState,
} from "./trust.ts";

const HOME_HOST_ID = "home";

export function shellIdentity(name: string): string {
  return name.trim();
}

export function isShellSelectable(
  capsule: Pick<CapsuleSummary, "name" | "role" | "launchable">,
): boolean {
  return (
    capsule.name.trim() !== HOME_HOST_ID &&
    capsule.role === "shell" &&
    capsule.launchable === true
  );
}

export function selectableShells(catalog: CapsuleCatalogResponse): CapsuleSummary[] {
  return catalog.capsules.filter(isShellSelectable);
}

export interface ShellTrustCard {
  name: string;
  title: string;
  trust: TrustMaterial;
  verification: VerificationState;
}

export function shellTrustCard(capsule: CapsuleSummary): ShellTrustCard {
  return {
    name: capsule.name,
    title: capsule.title,
    trust: trustMaterial(capsule),
    verification: "unknown",
  };
}

export interface ShellPicker {
  shells: ShellTrustCard[];
  active: string;
}

export function shellPicker(catalog: CapsuleCatalogResponse, active?: string): ShellPicker {
  const shellsByName = new Map<string, ShellTrustCard>();
  for (const capsule of selectableShells(catalog)) {
    const card = shellTrustCard(capsule);
    const existing = shellsByName.get(card.name);
    if (!existing || capsule.name === card.name) {
      shellsByName.set(card.name, card);
    }
  }
  const shells = [...shellsByName.values()];
  const names = new Set(shells.map((shell) => shell.name));
  const requestedActive = shellIdentity(active || "");
  return {
    shells,
    active: requestedActive && names.has(requestedActive) ? requestedActive : (shells[0]?.name ?? ""),
  };
}

export function withActiveShell(picker: ShellPicker, name: string): ShellPicker | null {
  const active = shellIdentity(name);
  if (!picker.shells.some((shell) => shell.name === active)) return null;
  return { ...picker, active };
}
