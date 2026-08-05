/* Agent grant cards + truth strip.
   Bound from agent-harness.js. Tip: home-20260804av
   UI ≠ authority (Principle 16): Allow once does not mint Capsule power.
   library.read → real Inbox capability (Wave 5.01); wallet.sign stays preview mock. */

import {
  requestTool,
  resolveMockApproval,
  resetMockCapabilities,
  applyCapabilityState,
  wantsLibraryTool,
  wantsWalletTool,
} from "./mock-agent-provider.js?v=home-20260804av";
import {
  requestAgentLibraryRead,
  fetchAgentLibraryReadStatus,
  cancelAgentLibraryRead,
} from "./agent-live.js?v=home-20260804av";
import { showInboxRail } from "./shell-inbox-rail.js?v=home-20260804av";

/** @type {null | Record<string, Function>} */
let store = null;
/** @type {null | Record<string, Function>} */
let host = null;
/** @type {Map<string, number>} */
const libraryPollTimers = new Map();
/** @type {null | { requestId: string, resource: string, result: string }} */
let readyLibraryGrant = null;

export function bindAgentGrants(nextStore = {}, nextHost = {}) {
  store = nextStore;
  host = nextHost;
}

/** Wave 6.01 — latest Inbox-ready library.read (for Desktop extract + citations). */
export function getReadyLibraryReadGrant() {
  return readyLibraryGrant ? { ...readyLibraryGrant } : null;
}

function setReadyLibraryGrant(next) {
  readyLibraryGrant = next
    ? {
        requestId: String(next.requestId || "").slice(0, 80),
        resource: String(next.resource || "").slice(0, 512),
        result: String(next.result || "").slice(0, 12_000),
      }
    : null;
}

/** Cited On-Home Library listing for Live context (fail-closed when not granted). */
export function formatLibraryKbContext() {
  if (!readyLibraryGrant?.result) {
    return "";
  }
  return (
    `On-Home Library (Inbox library.read · cited paths):\n` +
    `${readyLibraryGrant.result}`
  );
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

function clearLibraryPoll(requestId) {
  const timer = libraryPollTimers.get(requestId);
  if (timer) {
    window.clearInterval(timer);
    libraryPollTimers.delete(requestId);
  }
}

function updateSessionGrant(toolId, patch) {
  const session = store.getSessions().find((s) => s.id === store.getActiveSessionId());
  const grantMsg = session?.messages?.find(
    (m) =>
      m.role === "grant" &&
      m.toolId === toolId &&
      (m.state === "pending" || !m.state || m.requestId === patch.requestId),
  );
  if (!grantMsg) {
    return;
  }
  Object.assign(grantMsg, patch);
  try {
    host.persistAgentWorkspaceSoon?.();
  } catch {
    /* optional */
  }
}

function paintInboxGrantCard(card, spec) {
  const state = spec.state || "pending";
  card.className = `agent-grant-card is-${state}`;
  card.dataset.role = "grant";
  card.dataset.toolId = spec.toolId || "library.read";
  card.dataset.state = state;
  card.dataset.inbox = "1";
  delete card.dataset.preview;
  if (spec.requestId) {
    card.dataset.requestId = spec.requestId;
  }
  card.replaceChildren();

  const head = document.createElement("div");
  head.className = "agent-grant-card-head";
  const title = document.createElement("span");
  title.className = "agent-grant-card-title";
  title.textContent = spec.label || "Library · Read";
  const badge = document.createElement("span");
  badge.className = "agent-grant-card-preview";
  badge.textContent = "Inbox · once";
  head.append(title, badge);

  const body = document.createElement("p");
  body.className = "agent-grant-card-summary";
  body.textContent =
    spec.summary ||
    "Approve in Inbox — one Desktop list on this Home. Agent UI does not mint the grant.";

  const scopeEl = document.createElement("p");
  scopeEl.className = "agent-grant-card-scope";
  scopeEl.textContent = `Scope: ${spec.scope || "Desktop"}`;

  card.append(head, body, scopeEl);

  if (state === "pending") {
    const teach = document.createElement("p");
    teach.className = "agent-grant-card-teach";
    teach.textContent =
      "Open Inbox to Allow once (session-mint stays elsewhere). Deny here cancels the pending request.";
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
    allow.textContent = "Open Inbox";
    actions.append(deny, allow);
    card.append(actions);
  } else {
    const chip = document.createElement("div");
    chip.className = "agent-grant-card-chip";
    chip.textContent =
      state === "granted"
        ? "Allowed once · Inbox — Desktop list below"
        : state === "error"
          ? `Error · ${spec.error || "tool failed"}`
          : "Denied · fail-closed";
    card.append(chip);
    if (state === "granted" && spec.result) {
      const result = document.createElement("pre");
      result.className = "agent-grant-card-result";
      result.textContent = spec.result;
      card.append(result);
    }
  }
}

function startLibraryReadPoll(card, requestId) {
  if (!requestId || libraryPollTimers.has(requestId)) {
    return;
  }
  const tick = async () => {
    try {
      const status = await fetchAgentLibraryReadStatus(requestId);
      if (!status) {
        return;
      }
      if (status.status === "ready") {
        clearLibraryPoll(requestId);
        const outcome = {
          status: "ok",
          result: status.result || "(empty Desktop list)",
          inbox: true,
        };
        setReadyLibraryGrant({
          requestId,
          resource: status.resource || "",
          result: outcome.result,
        });
        updateSessionGrant("library.read", {
          state: "granted",
          requestId,
          result: outcome.result,
        });
        applyCapabilityState("library.read", "granted");
        paintInboxGrantCard(card, {
          toolId: "library.read",
          state: "granted",
          requestId,
          label: "Library · Read",
          scope: status.resource,
          result: outcome.result,
        });
        syncTruthStrip();
        host.scrollStreamToEnd?.();
        return;
      }
      if (status.status === "denied") {
        clearLibraryPoll(requestId);
        setReadyLibraryGrant(null);
        updateSessionGrant("library.read", { state: "denied", requestId });
        applyCapabilityState("library.read", "denied");
        paintInboxGrantCard(card, {
          toolId: "library.read",
          state: "denied",
          requestId,
          label: "Library · Read",
          scope: status.resource,
        });
        syncTruthStrip();
        return;
      }
      if (status.status === "error") {
        clearLibraryPoll(requestId);
        setReadyLibraryGrant(null);
        updateSessionGrant("library.read", {
          state: "denied",
          requestId,
          result: status.error || "tool error",
        });
        paintInboxGrantCard(card, {
          toolId: "library.read",
          state: "error",
          requestId,
          label: "Library · Read",
          scope: status.resource,
          error: status.error || "tool error",
        });
        syncTruthStrip();
      }
    } catch (err) {
      console.warn("library.read poll failed", err);
    }
  };
  void tick();
  libraryPollTimers.set(requestId, window.setInterval(tick, 1500));
}

export function appendGrantCard(spec) {
  const stream = host.streamEl();
  if (!stream || !spec?.toolId) {
    return null;
  }
  host.clearEmptyState();

  if (spec.inbox || spec.requestId) {
    const card = document.createElement("article");
    paintInboxGrantCard(card, spec);
    stream.append(card);
    if (spec.state === "pending" && spec.requestId) {
      startLibraryReadPoll(card, spec.requestId);
    }
    syncTruthStrip();
    host.scrollStreamToEnd();
    return card;
  }

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
  if (card.dataset.inbox === "1") {
    paintInboxGrantCard(card, {
      toolId: card.dataset.toolId,
      state: outcome.status === "ok" ? "granted" : "denied",
      requestId: card.dataset.requestId,
      label: "Library · Read",
      result: outcome.result,
      error: outcome.error,
      scope: card.querySelector(".agent-grant-card-scope")?.textContent?.replace(/^Scope:\s*/, ""),
    });
    syncTruthStrip();
    host.scrollStreamToEnd();
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

  if (card.dataset.inbox === "1" && card.dataset.toolId === "library.read") {
    const requestId = card.dataset.requestId;
    if (decision === "allow_once") {
      try {
        showInboxRail();
      } catch {
        /* rail optional */
      }
      if (requestId) {
        startLibraryReadPoll(card, requestId);
      }
      return;
    }
    if (decision === "deny") {
      void (async () => {
        if (requestId) {
          clearLibraryPoll(requestId);
          try {
            await cancelAgentLibraryRead(requestId);
          } catch (err) {
            console.warn("library.read cancel failed", err);
          }
        }
        setReadyLibraryGrant(null);
        updateSessionGrant("library.read", { state: "denied", requestId });
        applyCapabilityState("library.read", "denied");
        paintInboxGrantCard(card, {
          toolId: "library.read",
          state: "denied",
          requestId,
          label: "Library · Read",
        });
        syncTruthStrip();
      })();
      return;
    }
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

export async function maybeOfferToolAfterReply() {
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
    try {
      const req = await requestAgentLibraryRead();
      const grant = {
        role: "grant",
        toolId: "library.read",
        state: "pending",
        inbox: true,
        requestId: req.request_id,
        label: req.label || "Library · Read",
        summary: req.summary || "Approve in Inbox for one Desktop list.",
        scope: req.scope || req.resource || "Desktop",
        args: { uri: req.resource },
      };
      session.messages.push(grant);
      try {
        host.persistAgentWorkspaceSoon?.();
      } catch {
        /* optional */
      }
      appendGrantCard(grant);
      try {
        showInboxRail();
      } catch {
        /* optional */
      }
    } catch (err) {
      const grant = {
        role: "grant",
        toolId: "library.read",
        state: "denied",
        inbox: true,
        label: "Library · Read",
        summary: err?.message || "Could not create Inbox library.read request",
        scope: "Desktop",
      };
      session.messages.push(grant);
      appendGrantCard(grant);
    }
  }
}

export function hydrateCapabilitiesFromSession(session) {
  resetMockCapabilities();
  setReadyLibraryGrant(null);
  for (const msg of session?.messages || []) {
    if (msg.role !== "grant" || !msg.toolId) {
      continue;
    }
    if (msg.toolId === "library.read" && (msg.inbox || msg.requestId)) {
      if (msg.state === "pending" && msg.requestId) {
        /* Card re-render path in harness will call appendGrantCard — poll starts there. */
        continue;
      }
      if (msg.state === "granted") {
        applyCapabilityState(msg.toolId, "granted");
        setReadyLibraryGrant({
          requestId: msg.requestId || "",
          resource: msg.scope || msg.args?.uri || "",
          result: msg.result || "",
        });
      } else if (msg.state === "denied") {
        applyCapabilityState(msg.toolId, "denied");
      }
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
