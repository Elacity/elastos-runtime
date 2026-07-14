// Browser-compatible ESP projection adapter for System.
// Keep this pure display code: Runtime/provider gates remain authoritative.
// scripts/check-system-esp-projections.mjs verifies parity with @elastos/esp.

export function provenanceView(object) {
  const provenance = object && typeof object === "object" ? object.provenance : null;
  if (!provenance || typeof provenance !== "object" || Array.isArray(provenance)) {
    return {
      state: "absent",
      author: null,
      cid: null,
      signature_present: false,
      signature_fingerprint: null,
      signer_known: false,
    };
  }
  const signature_present = provenance.signature_present === true;
  const signature_fingerprint =
    typeof provenance.signature_fingerprint === "string"
      ? provenance.signature_fingerprint
      : null;
  const state = signature_present
    ? signature_fingerprint
      ? "signed"
      : "incomplete"
    : "unsigned";
  return {
    state,
    author: provenance.author ?? null,
    cid: typeof provenance.cid === "string" ? provenance.cid : null,
    signature_present,
    signature_fingerprint,
    signer_known: typeof provenance.signed_by === "string" && provenance.signed_by.length > 0,
  };
}

export function gatePreviewAuditView(preview) {
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

export function requestBindingView(binding) {
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
  const incomplete =
    binding.schema !== "elastos.esp.request-binding/v1" ||
    typeof binding.request_id !== "string" ||
    binding.request_id.length === 0 ||
    typeof binding.principal !== "string" ||
    binding.principal.length === 0 ||
    typeof binding.capsule !== "string" ||
    binding.capsule.length === 0 ||
    typeof binding.method !== "string" ||
    binding.method.length === 0 ||
    !Array.isArray(binding.resources) ||
    hash.length === 0 ||
    typeof binding.bytes !== "number";
  return {
    state: incomplete ? "incomplete" : truncated ? "truncated" : "bound",
    present: true,
    bytes,
    truncated,
    hash_short: hash.slice(0, 12),
    preview_available: binding.preview !== null,
  };
}

const isObject = (value) =>
  typeof value === "object" && value !== null && !Array.isArray(value);

export function gatePreviewIsPreviewOnly(plan) {
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

export function inspectActionRequestValidation(request, expectedBody) {
  const reasons = [];
  if (request.status !== "pending") reasons.push("status_not_pending");
  if (!request.request_id) reasons.push("missing_request_id");
  if (!request.id) reasons.push("missing_target");
  if (!request.operation) reasons.push("missing_operation");
  if (!gatePreviewIsPreviewOnly(request.plan)) reasons.push("plan_not_preview_only");
  const binding = requestBindingView(request.request_binding);
  if (!binding.present) reasons.push("missing_request_binding");
  if (binding.state === "incomplete") reasons.push("incomplete_request_binding");
  if (binding.present && binding.hash_short.length === 0) reasons.push("missing_binding_hash");
  const exact = request.request_binding;
  if (exact) {
    if (exact.request_id !== request.request_id) reasons.push("request_id_binding_mismatch");
    if (exact.capsule !== request.id) reasons.push("capsule_binding_mismatch");
    if (exact.interface !== null) reasons.push("interface_binding_mismatch");
    if (exact.method !== request.operation) reasons.push("method_binding_mismatch");
    if (!exact.principal) reasons.push("missing_binding_principal");
    const plannedResources = isObject(request.plan) && Array.isArray(request.plan.capabilities)
      ? request.plan.capabilities
          .map((capability) =>
            isObject(capability) && typeof capability.resource === "string"
              ? capability.resource
              : ""
          )
          .filter(Boolean)
          .sort()
      : [];
    const boundResources = Array.isArray(exact.resources)
      ? exact.resources.filter((resource) => typeof resource === "string").sort()
      : [];
    if (JSON.stringify(boundResources) !== JSON.stringify([...new Set(plannedResources)])) {
      reasons.push("resource_binding_mismatch");
    }
    if (
      expectedBody !== undefined
    ) {
      const expectedCanonical = canonicalJson(expectedBody);
      const expectedBytes = new TextEncoder().encode(expectedCanonical).byteLength;
      if (
        exact.bytes !== expectedBytes ||
        exact.truncated !== (expectedBytes > 1024) ||
        exact.truncated ||
        exact.preview === null ||
        canonicalJson(exact.preview) !== expectedCanonical
      ) {
        reasons.push("body_binding_mismatch");
      }
    }
  }
  return {
    ok: reasons.length === 0,
    reasons,
    request_id: request.request_id,
    operation: request.operation,
    binding,
  };
}

function canonicalJson(value) {
  if (Array.isArray(value)) {
    return `[${value.map(canonicalJson).join(",")}]`;
  }
  if (isObject(value)) {
    return `{${Object.keys(value)
      .sort()
      .map((key) => `${JSON.stringify(key)}:${canonicalJson(value[key])}`)
      .join(",")}}`;
  }
  return JSON.stringify(value) ?? "null";
}
