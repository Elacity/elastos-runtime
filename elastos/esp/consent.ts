import type {
  InspectActionRequestResponse,
  InspectGatePreview,
  InspectRequestBinding,
  JsonValue,
} from "./esp_v0.ts";

export interface RequestBindingView {
  state: "absent" | "bound" | "truncated" | "incomplete";
  present: boolean;
  bytes: number;
  truncated: boolean;
  hash_short: string;
  preview_available: boolean;
}

export function requestBindingView(
  binding: InspectRequestBinding | null | undefined,
): RequestBindingView {
  if (!binding) {
    return {
      state: "absent",
      present: false,
      bytes: 0,
      truncated: false,
      hash_short: "",
      preview_available: false,
    };
  }
  const hash = typeof binding.sha256 === "string" ? binding.sha256 : "";
  const bytes = typeof binding.bytes === "number" && Number.isFinite(binding.bytes)
    ? Math.max(0, binding.bytes)
    : 0;
  const truncated = binding.truncated === true;
  const incomplete = hash.length === 0 || typeof binding.bytes !== "number";
  return {
    state: incomplete ? "incomplete" : truncated ? "truncated" : "bound",
    present: true,
    bytes,
    truncated,
    hash_short: hash.slice(0, 12),
    preview_available: binding.preview !== null,
  };
}

const isObject = (value: unknown): value is Record<string, JsonValue> =>
  typeof value === "object" && value !== null && !Array.isArray(value);

export interface ConsentValidation {
  ok: boolean;
  reasons: string[];
  request_id: string;
  operation: string;
  binding: RequestBindingView;
}

export function gatePreviewIsPreviewOnly(plan: InspectGatePreview | JsonValue): boolean {
  return (
    isObject(plan) &&
    plan.schema === "elastos.inspect.gate-preview/v1" &&
    plan.dispatch === false &&
    isObject(plan.execution) &&
    plan.execution.mode === "preview_only" &&
    plan.execution.can_dispatch === false &&
    plan.execution.can_mutate === false
  );
}

export function inspectActionRequestValidation(
  request: InspectActionRequestResponse,
): ConsentValidation {
  const reasons: string[] = [];
  if (request.status !== "pending") reasons.push("status_not_pending");
  if (!request.request_id) reasons.push("missing_request_id");
  if (!request.id) reasons.push("missing_target");
  if (!request.operation) reasons.push("missing_operation");
  if (!gatePreviewIsPreviewOnly(request.plan)) reasons.push("plan_not_preview_only");
  const binding = requestBindingView(request.request_binding);
  if (!binding.present) reasons.push("missing_request_binding");
  if (binding.state === "incomplete") reasons.push("incomplete_request_binding");
  if (binding.present && binding.hash_short.length === 0) reasons.push("missing_binding_hash");
  return {
    ok: reasons.length === 0,
    reasons,
    request_id: request.request_id,
    operation: request.operation,
    binding,
  };
}
