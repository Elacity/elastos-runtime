import type { InspectObjectProjection, JsonValue } from "./esp_v0.ts";
import { auditCountsView, type AuditCountsView } from "./audit_views.ts";

export interface CustodyView {
  state: "absent" | "complete" | "incomplete" | "degraded";
  present: boolean;
  required_capabilities: number;
  granted_capabilities: number;
  storage_declared: boolean;
  carrier_declared: boolean;
  carrier_endpoints: number;
  processes: {
    total: number;
    running: number;
  };
  audit: AuditCountsView;
}

const isRecord = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null && !Array.isArray(value);

const truthyJson = (value: JsonValue): boolean => {
  if (value === null || value === false) return false;
  if (Array.isArray(value)) return value.length > 0;
  if (typeof value === "object") return Object.keys(value).length > 0;
  return true;
};

export function custodyView(object: InspectObjectProjection | null | undefined): CustodyView {
  if (!object) {
    return {
      state: "absent",
      present: false,
      required_capabilities: 0,
      granted_capabilities: 0,
      storage_declared: false,
      carrier_declared: false,
      carrier_endpoints: 0,
      processes: { total: 0, running: 0 },
      audit: auditCountsView(null),
    };
  }
  const processes = Array.isArray(object?.processes) ? object.processes : [];
  const running = processes.filter(
    (process) => isRecord(process) && process.status === "running",
  ).length;
  const endpoints = Array.isArray(object?.carrier?.endpoints) ? object.carrier.endpoints : [];
  const audit = auditCountsView(object?.audit);
  const incomplete =
    !Array.isArray(object.required_capabilities) ||
    !Array.isArray(object.granted_capabilities) ||
    object.storage_namespaces === undefined ||
    object.carrier === undefined ||
    !Array.isArray(object.processes) ||
    object.audit == null;
  const degraded = audit.state === "denied" || (processes.length > 0 && running === 0);
  return {
    state: degraded ? "degraded" : incomplete ? "incomplete" : "complete",
    present: true,
    required_capabilities: Array.isArray(object?.required_capabilities)
      ? object.required_capabilities.length
      : 0,
    granted_capabilities: Array.isArray(object?.granted_capabilities)
      ? object.granted_capabilities.length
      : 0,
    storage_declared: object?.storage_namespaces ? truthyJson(object.storage_namespaces) : false,
    carrier_declared: object?.carrier?.enabled ? truthyJson(object.carrier.enabled) : false,
    carrier_endpoints: endpoints.length,
    processes: { total: processes.length, running },
    audit,
  };
}
