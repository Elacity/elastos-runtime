import { randomBytes } from "node:crypto";

export class BrowserOperatorError extends Error {
  constructor(code, status = 409, details = {}) {
    super(code); this.code = code; this.status = status; this.details = details;
  }
}
const requireValue = (ok, code = "operator_response_invalid") => {
  if (!ok) throw new BrowserOperatorError(code);
};
const id = value => typeof value === "string" && /^[a-f0-9]{32}$/.test(value);
const safe = value => typeof value === "string" && /^[a-zA-Z0-9:_-]{1,256}$/.test(value);

// A pre-existing Runtime session and page invitation are supplied by the caller.
// Runtime holds the owner's approval credentials. This client holds only its own.
export function createBrowserOperatorClient(options) {
  const allowed = ["origin", "sessionToken", "pageId", "admissionId", "invitation", "fetchImpl", "timeoutMs"];
  requireValue(Object.keys(options).every(k => allowed.includes(k)), "operator_configuration_invalid");
  const { origin, sessionToken, pageId, invitation, fetchImpl = fetch, timeoutMs = 3000 } = options;
  let admissionId = options.admissionId || null;
  const base = new URL(origin);
  requireValue(base.origin === origin && !base.username && !base.password &&
    (base.protocol === "https:" || (base.protocol === "http:" && ["localhost", "127.0.0.1", "[::1]"].includes(base.hostname))) &&
    typeof sessionToken === "string" && /^[\x21-\x7e]{1,128}$/.test(sessionToken) &&
    safe(pageId) && (safe(admissionId) ? !invitation : invitation?.schema === "elastos.browser.operator-invitation/v1" &&
      invitation.runtime_origin === origin && invitation.page_id === pageId && id(invitation.document_generation) &&
      Number.isFinite(invitation.expires_at_ms)) && Number.isInteger(timeoutMs) && timeoutMs > 0 && timeoutMs <= 10000,
  "operator_configuration_invalid");
  const pagePath = `/api/apps/browser/pages/${encodeURIComponent(pageId)}`;
  const admissionPath = () => `${pagePath}/operator-requests/${encodeURIComponent(admissionId)}`;
  let detached = false, busy = false, uncertain = null, admissionAttempted = false;

  async function request(path, { method = "GET", body, capability, signal } = {}) {
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), timeoutMs);
    let reader;
    try {
      const response = await fetchImpl(`${origin}${path}`, { method, redirect: "error", credentials: "omit",
        signal: signal ? AbortSignal.any([signal, controller.signal]) : controller.signal,
        headers: { Authorization: `Bearer ${sessionToken}`, "content-type": "application/json",
          ...(capability ? { "x-elastos-capability": capability } : {}) },
        ...(body === undefined ? {} : { body: JSON.stringify(body) }) });
      reader = response.body?.getReader(); requireValue(reader);
      let bytes = 0; const parts = [];
      for (;;) {
        const { done, value } = await reader.read(); if (done) break;
        bytes += value.byteLength; requireValue(bytes <= 32768, "operator_response_too_large"); parts.push(value);
      }
      let result; try { result = JSON.parse(Buffer.concat(parts).toString("utf8")); }
      catch { throw new BrowserOperatorError("operator_response_invalid"); }
      if (!response.ok) {
        const code = typeof result.code === "string" && /^[a-z_]{1,80}$/.test(result.code) ? result.code : "operator_request_rejected";
        throw new BrowserOperatorError(code, response.status);
      }
      return result;
    } catch (error) {
      if (error instanceof BrowserOperatorError) throw error;
      throw new BrowserOperatorError("operator_transport_failed", 503);
    } finally { clearTimeout(timer); await reader?.cancel().catch(() => {}); reader?.releaseLock(); }
  }
  async function status({ signal } = {}) {
    requireValue(admissionId, "operator_admission_required");
    requireValue(!detached, "operator_detached");
    const value = await request(admissionPath(), { signal });
    requireValue(value.schema === "elastos.browser.operator-admission/v1" && value.request_id === admissionId &&
      ["pending", "approving", "active", "revoked", "expired"].includes(value.status));
    return value;
  }
  async function requestAdmission({ actions = ["click"], reason = "Inspect this page and use the selected controls", signal } = {}) {
    requireValue(!admissionId && invitation && !admissionAttempted && !detached, "operator_admission_already_requested");
    requireValue(invitation.expires_at_ms > Date.now() && invitation.expires_at_ms <= Date.now() + 30000,
      "operator_invitation_expired");
    requireValue(Array.isArray(actions) && actions.length > 0 && actions.length <= 2 && new Set(actions).size === actions.length &&
      actions.every(a => ["click", "type", "fill"].includes(a)) && typeof reason === "string" && reason.trim() && Buffer.byteLength(reason) <= 240,
      "operator_request_invalid");
    admissionAttempted = true;
    const value = await request(`${pagePath}/operator-requests`, { method: "POST", signal,
      body: { schema: "elastos.browser.operator-request/v1", document_generation: invitation.document_generation,
        actions, duration_ms: 30000, max_actions: 3, reason, inspect: true } });
    requireValue(value.schema === "elastos.browser.operator-admission/v1" && value.status === "pending" && safe(value.request_id));
    admissionId = value.request_id;
    return { status: "pending", request_id: admissionId, inspection_requested: true };
  }
  async function grant(kind, signal) {
    const value = await status({ signal });
    requireValue(value.status === "active", `operator_${value.status}`);
    const capability = value[kind === "inspect" ? "inspection_capability" : "capability"];
    requireValue(typeof capability === "string" && capability.length > 0 && capability.length <= 8192,
      kind === "inspect" ? "operator_inspection_not_granted" : "operator_write_not_granted");
    return capability;
  }
  async function inspect({ signal } = {}) {
    const capability = await grant("inspect", signal);
    let cursor = null, snapshotId, generation, snapshotBytes = 0; const nodes = [], refs = new Set();
    for (let i = 0; i < 8; i++) {
      const value = await request(`${admissionPath()}/inspect`, { method: "POST", capability, signal,
        body: { schema: "elastos.browser.inspect-request/v1", limit: 64, cursor } });
      requireValue(value.schema === "elastos.browser.inspect-result/v1" && value.page_id === pageId &&
        id(value.snapshot_id) && id(value.document_generation) && Array.isArray(value.nodes) && value.nodes.length <= 64 &&
        value.truncated === false, "operator_snapshot_invalid");
      snapshotId ||= value.snapshot_id; generation ||= value.document_generation;
      requireValue(value.snapshot_id === snapshotId && value.document_generation === generation, "stale_inspection");
      for (const node of value.nodes) {
        requireValue(node.ref === `${snapshotId}:${nodes.length}` && !refs.has(node.ref) &&
          [node.role, node.name, node.description].every(v => typeof v === "string" && v.length <= 8192) &&
          (node.value === null || (typeof node.value === "string" && node.value.length <= 8192)), "operator_snapshot_invalid");
        const projected = { ref: node.ref, role: node.role, name: node.name, description: node.description, value: node.value };
        snapshotBytes += Buffer.byteLength(JSON.stringify(projected));
        requireValue(snapshotBytes <= 131072, "operator_snapshot_too_large");
        refs.add(node.ref); nodes.push(projected);
      }
      cursor = value.next_cursor;
      if (cursor === null) return { page_id: pageId, snapshot_id: snapshotId, document_generation: generation, nodes };
      requireValue(value.nodes.length > 0 && cursor === `${snapshotId}:${nodes.length}`, "operator_snapshot_invalid");
    }
    throw new BrowserOperatorError("operator_snapshot_incomplete");
  }
  async function input({ document_generation, ref, action, text }, { signal } = {}) {
    requireValue(!busy && !uncertain, uncertain ? "operator_reconciliation_required" : "operator_client_busy");
    requireValue(id(document_generation) && typeof ref === "string" && /^[a-f0-9]{32}:(0|[1-9][0-9]{0,2})$/.test(ref) &&
      Number(ref.split(":")[1]) < 512 && ["click", "type", "fill"].includes(action) &&
      (action === "click" ? text === undefined : typeof text === "string" && (action === "fill" || Buffer.byteLength(text) > 0) &&
        Buffer.byteLength(text) <= 1024 && !/[\x00-\x1f\x7f-\x9f]/.test(text)), "operator_input_invalid");
    busy = true; let sent = false;
    const requestId = randomBytes(16).toString("hex");
    try {
      const capability = await grant("input", signal);
      const event = { schema: "elastos.browser.ref-input/v1", admission_id: admissionId, document_generation,
        ref, action, request_id: requestId, ...(text === undefined ? {} : { text }) };
      sent = true;
      const result = await request(`${pagePath}/input`, { method: "POST", capability, signal, body: { event } });
      requireValue(result.schema === "elastos.browser.ref-input-result/v1" && result.page_id === pageId &&
        result.admission_id === admissionId && result.request_id === requestId && result.document_generation === document_generation &&
        result.accepted === true && result.outcome === "completed", "operator_input_uncertain");
      return { request_id: requestId, outcome: "completed" };
    } catch (error) {
      // Even an HTTP failure can follow an effect. Reconcile via admission receipts.
      if (sent) { uncertain = requestId; throw new BrowserOperatorError("operator_reconciliation_required", 409, { request_id: requestId }); }
      throw error;
    } finally { busy = false; }
  }
  async function reconcile({ signal } = {}) {
    const value = await status({ signal });
    const receipt = value.receipts?.find(r => r.request_id === uncertain);
    // Observation only. A new admission is required before further effects.
    return { request_id: uncertain, outcome: receipt?.outcome || "unknown", requires_new_admission: uncertain !== null };
  }
  async function detach({ signal } = {}) {
    if (detached) return { detached: true, page_closed: false };
    requireValue(!busy, "operator_client_busy");
    requireValue(admissionId, "operator_admission_required");
    const result = await request(`${admissionPath()}/detach`, { method: "POST", signal });
    requireValue(result.schema === "elastos.browser.operator-admission/v1" && result.request_id === admissionId &&
      result.status === "revoked" && result.writer_acquired === false, "operator_detach_unconfirmed");
    detached = true; return { detached: true, page_closed: false };
  }
  return Object.freeze({ pageId, requestAdmission, status, inspect, input, reconcile, detach });
}
