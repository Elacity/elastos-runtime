import type {
  InspectDispatchResult,
  InspectGatePreview,
  InspectObjectProjection,
  EspRequestBinding,
  JsonValue,
} from "./esp_v0.ts";
import { requestBindingView } from "./consent.ts";

export type AuditState = "absent" | "clean" | "denied" | "attested";

export interface AuditCountsView {
  present: boolean;
  total: number;
  denied: number;
  attested: number;
  recent: JsonValue[];
  state: AuditState;
}

const finiteNumber = (value: unknown): number =>
  typeof value === "number" && Number.isFinite(value) ? Math.max(0, value) : 0;

export function auditCountsView(
  audit: InspectObjectProjection["audit"] | null | undefined,
): AuditCountsView {
  if (!audit) {
    return { present: false, total: 0, denied: 0, attested: 0, recent: [], state: "absent" };
  }
  const total = finiteNumber(audit.counts?.total);
  const denied = finiteNumber(audit.counts?.denied);
  const attested = finiteNumber(audit.counts?.attested);
  const recent = Array.isArray(audit.recent) ? audit.recent : [];
  const state: AuditState =
    denied > 0 ? "denied" : attested > 0 ? "attested" : total > 0 ? "clean" : "absent";
  return { present: true, total, denied, attested, recent, state };
}

export interface GatePreviewAuditView {
  state: "preview" | "degraded";
  operation: string;
  capability_count: number;
  audit_events: string[];
  preview_only: boolean;
  can_dispatch: boolean;
}

export function gatePreviewAuditView(preview: InspectGatePreview): GatePreviewAuditView {
  const preview_only =
    preview.dispatch === false && preview.execution?.mode === "preview_only";
  const can_dispatch = preview.execution?.can_dispatch === true;
  return {
    state: preview_only && !can_dispatch ? "preview" : "degraded",
    operation: preview.operation,
    capability_count: preview.capabilities.length,
    audit_events: Array.isArray(preview.audit_events) ? preview.audit_events : [],
    preview_only,
    can_dispatch,
  };
}

export interface DispatchResultAuditView {
  state: "approved" | "degraded";
  operation: string;
  target: string;
  capability_count: number;
  audit_events: string[];
  approved_execution: boolean;
  provider_status: string | null;
  request_id: string;
  request_bound: boolean;
}

export function dispatchResultAuditView(
  result: InspectDispatchResult,
  expected?: EspRequestBinding,
): DispatchResultAuditView {
  const response =
    typeof result.provider_response === "object" &&
    result.provider_response !== null &&
    !Array.isArray(result.provider_response)
      ? result.provider_response
      : {};
  const status = response.status;
  const approved_execution =
    result.execution?.mode === "approved_dispatch" &&
    result.execution?.approval_surface === "inbox";
  const provider_status = typeof status === "string" ? status : null;
  const binding = result.request_binding;
  const bindingView = requestBindingView(binding);
  const capabilityResources = Array.isArray(result.capabilities)
    ? result.capabilities
        .map((capability) => capability?.resource)
        .filter((resource): resource is string => typeof resource === "string")
        .sort()
    : [];
  const bindingResources = Array.isArray(binding?.resources)
    ? binding.resources.filter((resource): resource is string => typeof resource === "string").sort()
    : [];
  const expectedMatches = expected === undefined || requestBindingsEqual(binding, expected);
  const transfer =
    typeof response._runtime_transfer === "object" &&
    response._runtime_transfer !== null &&
    !Array.isArray(response._runtime_transfer)
      ? response._runtime_transfer
      : {};
  const runtimeReceiptMatches =
    transfer.schema === "elastos.provider.transfer/v1" &&
    transfer.source === "inspect" &&
    transfer.target === result.target &&
    transfer.op === result.operation &&
    transfer.status === "completed";
  const request_bound =
    bindingView.state === "bound" &&
    binding.capsule === result.id &&
    binding.interface === null &&
    binding.method === result.operation &&
    JSON.stringify(bindingResources) === JSON.stringify([...new Set(capabilityResources)]) &&
    expectedMatches &&
    runtimeReceiptMatches;
  return {
    state: approved_execution && provider_status === "ok" && request_bound ? "approved" : "degraded",
    operation: result.operation,
    target: result.target,
    capability_count: result.capabilities.length,
    audit_events: result.audit_events,
    approved_execution,
    provider_status,
    request_id: typeof binding?.request_id === "string" ? binding.request_id : "",
    request_bound,
  };
}

function requestBindingsEqual(
  left: EspRequestBinding | null | undefined,
  right: EspRequestBinding | null | undefined,
): boolean {
  return (
    left?.schema === right?.schema &&
    left?.request_id === right?.request_id &&
    left?.principal === right?.principal &&
    left?.capsule === right?.capsule &&
    left?.interface === right?.interface &&
    left?.method === right?.method &&
    JSON.stringify(left?.resources) === JSON.stringify(right?.resources) &&
    left?.sha256 === right?.sha256 &&
    left?.bytes === right?.bytes &&
    left?.truncated === right?.truncated &&
    canonicalJson(left?.preview) === canonicalJson(right?.preview)
  );
}

function canonicalJson(value: JsonValue | undefined): string {
  if (Array.isArray(value)) {
    return `[${value.map(canonicalJson).join(",")}]`;
  }
  if (typeof value === "object" && value !== null) {
    return `{${Object.keys(value)
      .sort()
      .map((key) => `${JSON.stringify(key)}:${canonicalJson(value[key])}`)
      .join(",")}}`;
  }
  return JSON.stringify(value) ?? "undefined";
}
