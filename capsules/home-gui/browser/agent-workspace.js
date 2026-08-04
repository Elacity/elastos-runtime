/* Agent workspace snapshot — host session.agent persist seam.
   Harness binds store accessors at boot (avoids circular state imports).
   UI ≠ authority (Principle 16). */

import {
  listProjects,
  replaceProjects,
  setReasoningVisible,
  getUsageLedger,
  applyUsageLedger,
} from "./mock-agent-provider.js?v=home-20260804at";
import {
  clampLiveMaxTokens,
  clampLiveTemperature,
  normalizeLiveSystemPrompt,
} from "./agent-live.js?v=home-20260804at";
import { scheduleAgentWorkspacePersist } from "./shell-windows.js?v=home-20260804at";

export const AGENT_WORKSPACE_V = 1;
const MAX_PERSISTED_SESSIONS = 24;
const MAX_PERSISTED_MESSAGES = 24;
const MAX_PERSISTED_TEXT = 4000;

const WORKBENCH_TABS = new Set([
  "outputs",
  "plan",
  "library",
  "diff",
  "browser",
  "terminal",
]);

/** @type {null | Record<string, Function>} */
let store = null;

export function bindAgentWorkspaceStore(next = {}) {
  store = next;
}

export function clampWorkbenchTab(tab) {
  return WORKBENCH_TABS.has(tab) ? tab : "outputs";
}

function truncatePersistedText(text) {
  const s = String(text || "");
  return s.length > MAX_PERSISTED_TEXT ? `${s.slice(0, MAX_PERSISTED_TEXT)}…` : s;
}

export function serializeSessionForPersist(session) {
  if (!session?.id) {
    return null;
  }
  const messages = Array.isArray(session.messages)
    ? session.messages
        .slice(-MAX_PERSISTED_MESSAGES)
        .map((m) => {
          if (!m || typeof m !== "object") {
            return null;
          }
          if (m.role === "grant") {
            return {
              role: "grant",
              toolId: m.toolId || "",
              state: m.state || "pending",
              label: m.label || "",
              summary: truncatePersistedText(m.summary || ""),
              scope: m.scope || "",
              args: m.args && typeof m.args === "object" ? m.args : undefined,
              inbox: Boolean(m.inbox),
              requestId: m.requestId ? String(m.requestId).slice(0, 80) : undefined,
              result: m.result ? truncatePersistedText(m.result) : undefined,
            };
          }
          return {
            role: m.role === "user" || m.role === "agent" ? m.role : "agent",
            text: truncatePersistedText(m.text || ""),
          };
        })
        .filter(Boolean)
    : [];
  return {
    id: String(session.id).slice(0, 80),
    title: String(session.title || "Chat").slice(0, 64),
    group: session.group === "Earlier" ? "Earlier" : "Today",
    pinned: Boolean(session.pinned),
    projectId: session.projectId ? String(session.projectId).slice(0, 80) : null,
    mode: session.mode === "build" ? "build" : "chat",
    updatedAt: Number(session.updatedAt) || Date.now(),
    messages,
  };
}

export function getAgentWorkspaceSnapshot() {
  if (!store) {
    return null;
  }
  const sessionMode = store.getSessionMode();
  const toolMode = store.getToolMode();
  return {
    v: AGENT_WORKSPACE_V,
    activeSessionId: store.getActiveSessionId(),
    sessionMode: sessionMode === "build" ? "build" : "chat",
    toolMode: toolMode === "ask" || toolMode === "full" ? toolMode : "read",
    systemPrompt: normalizeLiveSystemPrompt(store.getSystemPrompt?.() || ""),
    maxTokens: clampLiveMaxTokens(store.getMaxTokens?.()),
    temperature: clampLiveTemperature(store.getTemperature?.()),
    workbenchOpen: Boolean(store.getWorkbenchOpen()),
    workbenchTab: clampWorkbenchTab(store.getWorkbenchTab()),
    reasoningVisible: Boolean(store.getReasoningVisible()),
    projects: listProjects(),
    usageTurns: getUsageLedger(),
    sessions: store
      .getSessions()
      .map(serializeSessionForPersist)
      .filter(Boolean)
      .slice(0, MAX_PERSISTED_SESSIONS),
  };
}

export function applyAgentWorkspaceSnapshot(raw) {
  if (!store || !raw || typeof raw !== "object" || Number(raw.v) !== AGENT_WORKSPACE_V) {
    return false;
  }
  store.setWorkspaceHydrated(true);
  if (Array.isArray(raw.projects)) {
    replaceProjects(raw.projects);
  }
  let sessions = store.getSessions();
  if (Array.isArray(raw.sessions) && raw.sessions.length) {
    sessions = raw.sessions.map(serializeSessionForPersist).filter(Boolean);
    store.setSessions(sessions);
  }
  if (raw.activeSessionId && sessions.some((s) => s.id === raw.activeSessionId)) {
    store.setActiveSessionId(raw.activeSessionId);
  } else {
    store.setActiveSessionId(sessions[0]?.id || null);
  }
  store.setSessionMode(raw.sessionMode === "build" ? "build" : "chat");
  store.setToolMode(raw.toolMode === "ask" || raw.toolMode === "full" ? raw.toolMode : "read");
  if (typeof raw.systemPrompt === "string") {
    store.setSystemPrompt?.(normalizeLiveSystemPrompt(raw.systemPrompt));
  }
  if (raw.maxTokens != null) {
    store.setMaxTokens?.(clampLiveMaxTokens(raw.maxTokens));
  }
  if (raw.temperature != null) {
    store.setTemperature?.(clampLiveTemperature(raw.temperature));
  }
  if (typeof raw.workbenchOpen === "boolean") {
    store.setWorkbenchOpen(raw.workbenchOpen);
    store.setWorkbenchUserClosed(!raw.workbenchOpen);
  }
  if (typeof raw.workbenchTab === "string") {
    store.setWorkbenchTab(clampWorkbenchTab(raw.workbenchTab));
  }
  if (typeof raw.reasoningVisible === "boolean") {
    store.setReasoningVisible(setReasoningVisible(raw.reasoningVisible));
  }
  if (Array.isArray(raw.usageTurns)) {
    applyUsageLedger(raw.usageTurns);
  }
  return true;
}

export function persistAgentWorkspaceSoon() {
  scheduleAgentWorkspacePersist();
}
