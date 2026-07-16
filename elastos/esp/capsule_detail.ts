import type { CapsuleSummary, InspectObjectProjection } from "./esp_v0.ts";
import { auditCountsView, type AuditCountsView } from "./audit_views.ts";
import { custodyView, type CustodyView } from "./custody.ts";
import { trustCard, type TrustCard } from "./trust.ts";

export interface CapsuleDetailView {
  name: string;
  title: string;
  role: string;
  type: string;
  launchable: boolean;
  trust: TrustCard;
  custody: CustodyView;
  audit: AuditCountsView;
  affordance_count: number;
}

export function capsuleDetailView(
  capsule: CapsuleSummary,
  object?: InspectObjectProjection | null,
): CapsuleDetailView {
  const affordanceCount = capsule.interfaces?.reduce(
    (count, descriptor) => count + descriptor.methods.length,
    0,
  ) ?? 0;
  return {
    name: capsule.name,
    title: capsule.title,
    role: capsule.role,
    type: capsule.type,
    launchable: capsule.launchable,
    trust: trustCard(capsule, object),
    custody: custodyView(object),
    audit: auditCountsView(object?.audit),
    affordance_count: affordanceCount,
  };
}
