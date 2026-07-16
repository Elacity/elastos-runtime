import type {
  InspectDispatchResult,
  InspectGatePreview,
  InspectObjectProjection,
  JsonValue,
} from "./esp_v0.ts";

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
}

export function dispatchResultAuditView(result: InspectDispatchResult): DispatchResultAuditView {
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
  return {
    state: approved_execution && provider_status === "ok" ? "approved" : "degraded",
    operation: result.operation,
    target: result.target,
    capability_count: result.capabilities.length,
    audit_events: result.audit_events,
    approved_execution,
    provider_status,
  };
}
