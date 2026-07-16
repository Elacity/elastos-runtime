import type {
  CapsuleCatalogResponse,
  CapsuleRole,
  CapsuleSummary,
  InspectObjectProjection,
} from "./esp_v0.ts";
import { capsuleDetailView, type CapsuleDetailView } from "./capsule_detail.ts";

const HOME_ROLES: ReadonlySet<CapsuleRole> = new Set(["shell", "app", "viewer"]);

export function isHomeCapsule(role: string): role is "shell" | "app" | "viewer" {
  return HOME_ROLES.has(role as CapsuleRole);
}

export function homeCapsules<T extends { role: string }>(capsules: readonly T[]): T[] {
  return capsules.filter((capsule) => isHomeCapsule(capsule.role));
}

export function isInstalled(capsule: { installed?: boolean }): boolean {
  return capsule.installed === true;
}

export function homeFleetScope<T extends { role: string; installed?: boolean }>(
  capsules: readonly T[],
): T[] {
  return homeCapsules(capsules).filter(isInstalled);
}

export function inspectObjectsByName(
  objects: readonly InspectObjectProjection[],
): Map<string, InspectObjectProjection> {
  return new Map(objects.map((object) => [object.name, object]));
}

export interface HomeFleetView {
  capsules: CapsuleDetailView[];
  total: number;
  needs_attention: number;
}

export function capsuleNeedsAttention(view: CapsuleDetailView): boolean {
  return (
    view.trust.trust === "unsigned" ||
    view.custody.state !== "complete" ||
    view.audit.state === "denied" ||
    view.custody.processes.total > 0 && view.custody.processes.running === 0
  );
}

export function homeFleetView(
  catalog: CapsuleCatalogResponse,
  inspected: ReadonlyMap<string, InspectObjectProjection> = new Map(),
): HomeFleetView {
  const capsules = homeFleetScope(catalog.capsules).map((capsule: CapsuleSummary) =>
    capsuleDetailView(capsule, inspected.get(capsule.name)),
  );
  return {
    capsules,
    total: capsules.length,
    needs_attention: capsules.reduce(
      (count, capsule) => count + (capsuleNeedsAttention(capsule) ? 1 : 0),
      0,
    ),
  };
}
