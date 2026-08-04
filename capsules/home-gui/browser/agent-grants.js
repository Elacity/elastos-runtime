/* Agent grant cards + truth strip (preview mock).
   Bound from agent-harness.js. Tip: home-20260804ar
   UI ≠ authority (Principle 16): Allow once is one-shot; never ambient. */

import {
  requestTool,
  resolveMockApproval,
  resetMockCapabilities,
  applyCapabilityState,
  wantsLibraryTool,
  wantsWalletTool,
} from "./mock-agent-provider.js?v=home-20260804ar";

/** @type {null | Record<string, Function>} */
let store = null;
/** @type {null | Record<string, Function>} */
let host = null;

export function bindAgentGrants(nextStore = {}, nextHost = {}) {
  store = nextStore;
  host = nextHost;
}

export function syncTruthStrip() {
  /* Truth facts live on Status / Workbench — not a sticky chat chrome bar.
     This sync keeps composer model + Think toggle honest. */
  host.syncModelTrigger();

  const thinkToggle = document.querySelector("[data-truth-thinking-toggle]");
  if (thinkToggle) {
    thinkToggle.setAttribute("aria-pressed", store.getReasoningVisible() ? "true" : "false");
    thinkToggle.setAttribute(
      "aria-label",
      store.getReasoningVisible() ? "Hide model thinking" : "Show model thinking"
    );
    thinkToggle.title = store.getReasoningVisible()
      ? "Hide model thinking blocks"
      : "Show model thinking blocks";
  }
  document.documentElement.dataset.agentReasoning = store.getReasoningVisible() ? "on" : "off";
}

export function appendGrantCard(spec) {
  const stream = host.streamEl();
  if (!stream || !spec?.toolId) {
    return null;
  }
  host.clearEmptyState();

  let state = spec.state || "pending";
  let approvalId = spec.approvalId || "";
  let label = spec.label || spec.toolId;
  let summary = spec.summary || "";
  let scope = spec.scope || "";

  if (state === "pending" && !approvalId) {
    const req = requestTool({
      toolId: spec.toolId,
      args: spec.args || { path: "Downloads" },
    });
    if (req.status === "denied") {
      state = "denied";
      label = req.label;
      summary = req.summary;
      scope = req.scope;
    } else if (req.status === "needs_approval") {
      approvalId = req.approvalId || "";
      label = req.label;
      summary = req.summary;
      scope = req.scope;
      spec.approvalId = approvalId;
      spec.label = label;
      spec.summary = summary;
      spec.scope = scope;
    } else if (req.status === "ok") {
      state = "granted";
      label = req.label;
      summary = req.summary;
      scope = req.scope;
      spec.result = req.result;
    }
  }

  const card = document.createElement("article");
  card.className = `agent-grant-card is-${state}`;
  card.dataset.role = "grant";
  card.dataset.toolId = spec.toolId;
  card.dataset.state = state;
  card.dataset.preview = "1";
  if (approvalId) {
    card.dataset.approvalId = approvalId;
  }

  const head = document.createElement("div");
  head.className = "agent-grant-card-head";
  const title = document.createElement("span");
  title.className = "agent-grant-card-title";
  title.textContent = label;
  const badge = document.createElement("span");
  badge.className = "agent-grant-card-preview";
  badge.textContent = "preview · mock";
  head.append(title, badge);

  const body = document.createElement("p");
  body.className = "agent-grant-card-summary";
  body.textContent = summary;

  const scopeEl = document.createElement("p");
  scopeEl.className = "agent-grant-card-scope";
  scopeEl.textContent = `Scope: ${scope}`;

  card.append(head, body, scopeEl);

  if (state === "pending") {
    const teach = document.createElement("p");
    teach.className = "agent-grant-card-teach";
    teach.textContent =
      "Preview ask — Allow once is one-shot and does not call Capsules yet.";
    card.append(teach);
    const actions = document.createElement("div");
    actions.className = "agent-grant-card-actions";
    const deny = document.createElement("button");
    deny.type = "button";
    deny.className = "agent-grant-btn agent-grant-btn-deny";
    deny.dataset.grantDecision = "deny";
    deny.textContent = "Deny";
    const allow = document.createElement("button");
    allow.type = "button";
    allow.className = "agent-grant-btn agent-grant-btn-allow";
    allow.dataset.grantDecision = "allow_once";
    allow.textContent = "Allow once";
    actions.append(deny, allow);
    card.append(actions);
  } else {
    const chip = document.createElement("div");
    chip.className = "agent-grant-card-chip";
    chip.textContent =
      state === "granted"
        ? "Allowed once · preview mock — no Capsule call"
        : "Denied · fail-closed";
    card.append(chip);
    if (state === "granted" && spec.result) {
      const result = document.createElement("pre");
      result.className = "agent-grant-card-result";
      result.textContent = spec.result;
      card.append(result);
    }
  }

  stream.append(card);
  syncTruthStrip();
  host.scrollStreamToEnd();
  return card;
}

export function paintGrantCardResolved(card, outcome) {
  if (!card) {
    return;
  }
  const state = outcome.status === "ok" ? "granted" : "denied";
  card.dataset.state = state;
  card.className = `agent-grant-card is-${state}`;
  card.querySelector(".agent-grant-card-actions")?.remove();
  card.querySelector(".agent-grant-card-chip")?.remove();
  card.querySelector(".agent-grant-card-result")?.remove();
  const chip = document.createElement("div");
  chip.className = "agent-grant-card-chip";
  chip.textContent =
    state === "granted"
      ? "Allowed once · preview mock — no Capsule call"
      : "Denied · fail-closed";
  card.append(chip);
  if (state === "granted" && outcome.result) {
    const result = document.createElement("pre");
    result.className = "agent-grant-card-result";
    result.textContent = outcome.result;
    card.append(result);
  }
  syncTruthStrip();
  host.scrollStreamToEnd();
}

export function resolveGrantFromCard(card, decision) {
  if (!card || card.dataset.state !== "pending") {
    return;
  }
  const outcome = resolveMockApproval({
    approvalId: card.dataset.approvalId,
    toolId: card.dataset.toolId,
    decision,
  });
  const session = store.getSessions().find((s) => s.id === store.getActiveSessionId());
  const grantMsg = session?.messages?.find(
    (m) =>
      m.role === "grant" &&
      m.toolId === card.dataset.toolId &&
      (m.state === "pending" || !m.state),
  );
  if (grantMsg) {
    grantMsg.state = outcome.status === "ok" ? "granted" : "denied";
    if (outcome.result) {
      grantMsg.result = outcome.result;
    }
  }
  paintGrantCardResolved(card, outcome);
}

export function sessionAlreadyHasGrant(session, toolId) {
  return Boolean(
    session?.messages?.some((m) => m.role === "grant" && m.toolId === toolId),
  );
}

export function maybeOfferToolAfterReply() {
  const session = store.getSessions().find((s) => s.id === store.getActiveSessionId());
  if (!session) {
    return;
  }
  const lastUser = [...session.messages]
    .reverse()
    .find((m) => m.role === "user");
  const text = lastUser?.text || "";
  if (wantsWalletTool(text) && !sessionAlreadyHasGrant(session, "wallet.sign")) {
    const req = requestTool({ toolId: "wallet.sign", args: {} });
    const grant = {
      role: "grant",
      toolId: "wallet.sign",
      state: req.status === "denied" ? "denied" : "pending",
      approvalId: req.approvalId,
      label: req.label,
      summary: req.summary,
      scope: req.scope,
    };
    session.messages.push(grant);
    appendGrantCard(grant);
    return;
  }
  if (wantsLibraryTool(text) && !sessionAlreadyHasGrant(session, "library.read")) {
    const req = requestTool({
      toolId: "library.read",
      args: { path: "Downloads" },
    });
    if (req.status === "needs_approval" || req.status === "denied") {
      const grant = {
        role: "grant",
        toolId: "library.read",
        state: req.status === "denied" ? "denied" : "pending",
        approvalId: req.approvalId,
        label: req.label,
        summary: req.summary,
        scope: req.scope,
        args: { path: "Downloads" },
      };
      session.messages.push(grant);
      appendGrantCard(grant);
    }
  }
}

export function hydrateCapabilitiesFromSession(session) {
  resetMockCapabilities();
  for (const msg of session?.messages || []) {
    if (msg.role !== "grant" || !msg.toolId) {
      continue;
    }
    if (msg.state === "pending") {
      const req = requestTool({
        toolId: msg.toolId,
        args: msg.args || { path: "Downloads" },
      });
      if (req.approvalId) {
        msg.approvalId = req.approvalId;
      }
      if (req.label) {
        msg.label = req.label;
      }
      if (req.summary) {
        msg.summary = req.summary;
      }
      if (req.scope) {
        msg.scope = req.scope;
      }
      if (req.status === "denied") {
        msg.state = "denied";
      }
    } else if (msg.state === "granted" || msg.state === "denied") {
      applyCapabilityState(msg.toolId, msg.state);
    }
  }
}
