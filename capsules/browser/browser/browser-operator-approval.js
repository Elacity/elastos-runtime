const schema = "elastos.browser.operator-request/v1";
const validId = value => typeof value === "string" && /^[a-f0-9]{32}$/.test(value);
const safeId = value => typeof value === "string" && /^[a-zA-Z0-9:_-]{1,256}$/.test(value);

export function operatorApprovalSummary(record) {
  const request = record?.request;
  if (!safeId(record?.request_id) || !safeId(record?.operator_session_id) || request?.schema !== schema ||
    !validId(request.document_generation) || !Array.isArray(request.actions) || !request.actions.length ||
    request.actions.length > 2 || new Set(request.actions).size !== request.actions.length ||
    request.actions.some(a => !["click", "type", "fill"].includes(a)) ||
    (request.inspect !== undefined && typeof request.inspect !== "boolean") ||
    !Number.isInteger(request.duration_ms) || request.duration_ms < 2000 || request.duration_ms > 30000 ||
    !Number.isInteger(request.max_actions) || request.max_actions < 1 || request.max_actions > 16 ||
    typeof request.reason !== "string" || !request.reason.trim() || request.reason.length > 240) {
    throw new Error("The agent request is invalid.");
  }
  const actions = request.actions.map(a => a === "click" ? "click page controls" : a === "fill" ? "replace or clear the selected field" : "insert text into the focused field").join(" and ");
  return { title: request.inspect ? "Allow page inspection and control?" : "Allow page control?",
    inspection: request.inspect ? "Read page text, labels and form values, including information entered on this page." : "Page inspection is outside this request.",
    control: `Allow this agent to ${actions}, for up to ${request.max_actions} actions over ${request.duration_ms / 1000} seconds.`,
    reason: request.reason, operator: record.operator_session_id,
    scope: "This approval applies to the current document. Runtime keeps page close and Wallet approvals separate." };
}

export function createOperatorApprovalController({ fetchJson, getOwner, sameOwner, runtimeOrigin, now = Date.now }) {
  const capture = () => {
    const owner = getOwner(); if (!owner || !safeId(owner.page_id)) throw new Error("Open a Browser page first."); return owner;
  };
  const current = owner => { if (!sameOwner(owner, getOwner())) throw new Error("The Browser page changed. Check requests again."); };
  const route = owner => `/api/apps/browser/pages/${encodeURIComponent(owner.page_id)}/operator-requests`;
  const checked = new Map(); let active = null;
  async function bounded(path, options = {}) {
    return fetchJson(path, { ...options, signal: AbortSignal.timeout(5000) });
  }
  async function invite() {
    const owner = capture(), started = now();
    const result = await bounded(`/api/apps/browser/pages/${encodeURIComponent(owner.page_id)}/inspect`, {
      method: "POST", body: { schema: "elastos.browser.inspect-request/v1", limit: 1, cursor: null } });
    current(owner);
    if (result?.schema !== "elastos.browser.inspect-result/v1" || result.page_id !== owner.page_id ||
      !validId(result.document_generation) || now() - started >= 30000) throw new Error("Create a new page invitation.");
    return { schema: "elastos.browser.operator-invitation/v1", runtime_origin: runtimeOrigin, page_id: owner.page_id,
      document_generation: result.document_generation, expires_at_ms: started + 30000 };
  }
  async function pending() {
    checked.clear(); const owner = capture(); const result = await bounded(route(owner)); current(owner);
    if (result?.schema !== "elastos.browser.operator-pending/v1" || !Array.isArray(result.requests) || result.requests.length > 128)
      throw new Error("Browser could not read agent requests.");
    for (const record of result.requests) {
      operatorApprovalSummary(record); checked.set(record.request_id, { owner, record: structuredClone(record) });
    }
    return result.requests;
  }
  async function decide(id, approve) {
    const saved = checked.get(id); if (!saved) throw new Error("Check requests before deciding."); current(saved.owner);
    // Re-read the exact request shown to the owner. The click cannot approve a
    // different scope, document or operator substituted after rendering.
    const latest = await bounded(route(saved.owner)); current(saved.owner);
    if (latest?.schema !== "elastos.browser.operator-pending/v1" || !Array.isArray(latest.requests) ||
      JSON.stringify(latest.requests.find(r => r.request_id === id)) !== JSON.stringify(saved.record))
      throw new Error("The agent request changed. Check requests again.");
    const result = await bounded(`${route(saved.owner)}/${encodeURIComponent(id)}`, { method: approve ? "POST" : "DELETE" });
    current(saved.owner);
    if (result?.schema !== "elastos.browser.operator-admission/v1" || result.request_id !== id ||
      result.status !== (approve ? "active" : "revoked") || result.writer_acquired !== approve)
      throw new Error("Runtime could not confirm the decision. Check agent access before retrying.");
    checked.delete(id);
    if (approve) active = { id, owner: saved.owner };
    return result;
  }
  async function revoke() {
    if (!active) throw new Error("Select an approved agent first."); current(active.owner);
    const result = await bounded(`${route(active.owner)}/${encodeURIComponent(active.id)}`, { method: "DELETE" });
    if (result?.status !== "revoked" || result.writer_acquired !== false) throw new Error("Runtime could not confirm release.");
    active = null; return result;
  }
  return { invite, pending, decide, revoke };
}

export function mountOperatorApproval({ container, controller }) {
  if (!container) return;
  const doc = container.ownerDocument;
  const make = (tag, text) => { const node = doc.createElement(tag); node.textContent = text; return node; };
  const section = make("section", ""); section.setAttribute("aria-label", "Agent access");
  section.append(make("h3", "Agent access"), make("p", "Create a page invitation, give its metadata to your agent, then check and approve its requested access. The invitation expires after 30 seconds."));
  const output = make("textarea", ""); output.readOnly = true; output.rows = 5; output.setAttribute("aria-label", "Page invitation metadata"); output.hidden = true;
  const status = make("p", ""); status.setAttribute("role", "status");
  const requests = make("div", "");
  let busy = false;
  const button = (text, action) => {
    const node = make("button", text); node.type = "button"; node.className = "browser-settings-action";
    node.addEventListener("click", async () => {
      if (busy) return; busy = true; node.disabled = true;
      try { await action(); } catch { status.textContent = "Browser could not confirm this operation. Check the current page and agent requests before trying again."; }
      finally { busy = false; node.disabled = false; }
    }); return node;
  };
  section.append(button("Create page invitation", async () => {
    output.value = JSON.stringify(await controller.invite()); output.hidden = false;
    status.textContent = "Give this metadata to your agent. Review its access request before allowing it.";
  }), output, button("Check agent requests", async () => {
    const rows = await controller.pending(); requests.replaceChildren();
    for (const row of rows) {
      const summary = operatorApprovalSummary(row), card = make("div", "");
      card.append(make("strong", summary.title), make("p", `Agent session: ${summary.operator}`),
        make("p", summary.inspection), make("p", summary.control), make("p", `Agent reason: ${summary.reason}`), make("p", summary.scope));
      card.append(button(summary.title, async () => { await controller.decide(row.request_id, true); card.remove(); status.textContent = "Agent access approved. You can revoke it below."; }),
        button("Deny request", async () => { await controller.decide(row.request_id, false); card.remove(); status.textContent = "Agent request denied."; }));
      requests.append(card);
    }
    status.textContent = rows.length ? "Review the access requested below." : "There are no pending agent requests.";
  }), requests, button("Revoke agent access", async () => { await controller.revoke(); status.textContent = "Runtime confirmed that the agent released control."; }), status);
  container.append(section);
}
