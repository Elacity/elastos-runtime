import type {
  CapsuleAffordanceDescriptor,
  CapsuleMethodBindingSummary,
  CapsuleSummary,
  InspectObjectProjection,
} from "./esp_v0.ts";
import {
  trustMaterial,
  verificationState,
  type TrustMaterial,
  type VerificationState,
} from "./trust.ts";

export const AUTHORITY_INVARIANT_FLAGS = {
  verification_grants_authority: false,
  verification_proves_executable: false,
  declared_permissions_grant_authority: false,
  declared_risk_is_advisory: true,
  route_grants_authority: false,
  frame_grants_authority: false,
  iframe_placement_grants_authority: false,
  http_success_grants_authority: false,
} as const;

export interface AuthorityInvariantView {
  trust_evidence: {
    material: TrustMaterial;
    verification: VerificationState;
  };
  declared_permissions: {
    state: "declared" | "none-declared" | "unknown";
    resources: string[];
  };
  executable_binding: {
    state: "executable" | "non-executable" | "inconsistent" | "unknown";
    executable: boolean;
    handler: string | null;
  };
  policy_gate: {
    state: "approval-required" | "runtime-policy" | "none-declared" | "unknown";
    declared_risk: string | null;
    declared_risk_is_advisory: true;
  };
  authorization: {
    state: "unknown";
    authorized: null;
    decided_by: "runtime-route-policy";
  };
  invariants: typeof AUTHORITY_INVARIANT_FLAGS;
}

export function authorityInvariantView(
  capsule: Pick<CapsuleSummary, "trust_state" | "capabilities"> | null | undefined,
  object?: Pick<InspectObjectProjection, "trust_evidence"> | null,
  method?: Pick<CapsuleAffordanceDescriptor, "risk" | "approval"> | null,
  binding?: Pick<
    CapsuleMethodBindingSummary,
    "state" | "handler_available" | "executable" | "handler_kind" | "handler"
  > | null,
): AuthorityInvariantView {
  const permissions = capsule?.capabilities;
  const resources = Array.isArray(permissions)
    ? permissions.filter((resource): resource is string => typeof resource === "string")
    : [];
  const permissionState = Array.isArray(permissions)
    ? resources.length > 0
      ? "declared"
      : "none-declared"
    : "unknown";

  const concreteRuntimeBinding =
    binding?.state === "executable" &&
    binding.handler_available === true &&
    binding.executable === true &&
    binding.handler_kind === "runtime" &&
    typeof binding.handler === "string" &&
    binding.handler.length > 0;
  const bindingState = !binding
    ? "unknown"
    : concreteRuntimeBinding
      ? "executable"
      : binding.executable === true
        ? "inconsistent"
        : "non-executable";

  const policyState =
    binding?.state === "approval-required" || method?.approval === "user"
      ? "approval-required"
      : method?.approval === "runtime_policy"
        ? "runtime-policy"
        : method?.approval === "none"
          ? "none-declared"
          : "unknown";

  return {
    trust_evidence: {
      material: trustMaterial(capsule),
      verification: verificationState(object),
    },
    declared_permissions: {
      state: permissionState,
      resources,
    },
    executable_binding: {
      state: bindingState,
      executable: concreteRuntimeBinding,
      handler: concreteRuntimeBinding ? binding?.handler ?? null : null,
    },
    policy_gate: {
      state: policyState,
      declared_risk: typeof method?.risk === "string" ? method.risk : null,
      declared_risk_is_advisory: true,
    },
    authorization: {
      state: "unknown",
      authorized: null,
      decided_by: "runtime-route-policy",
    },
    invariants: AUTHORITY_INVARIANT_FLAGS,
  };
}
