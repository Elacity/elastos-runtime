/* Agent workspace snapshot — host session.agent persist seam.
   Harness binds store accessors at boot (avoids circular state imports).
   UI ≠ authority (Principle 16). */

import {
  listProjects,
  replaceProjects,
  setReasoningVisible,
  getUsageLedger,
  applyUsageLedger,
} from "./mock-agent-provider.js?v=home-20260814a";
import {
  clampLiveMaxTokens,
  clampLiveTemperature,
  normalizeLiveSystemPrompt,
  normalizeAgentNotes,
} from "./agent-live.js?v=home-20260814a";
import { cheapTurnSnapshot } from "./agent-context.js?v=home-20260814a";
import { recoverStalePersistedTurn } from "./agent-stream-qos.js?v=home-20260814a";
import { scheduleAgentWorkspacePersist } from "./shell-windows.js?v=home-20260814a";

export const AGENT_WORKSPACE_V = 1;
const MAX_PERSISTED_SESSIONS = 24;
const MAX_PERSISTED_MESSAGES = 24;
const MAX_PERSISTED_TEXT = 4000;
export const MAX_DRAFT_PART_TEXT = 40_000;

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
            id: m.id ? String(m.id).slice(0, 40) : undefined,
            parentId: m.parentId ? String(m.parentId).slice(0, 40) : undefined,
            branchId: m.branchId ? String(m.branchId).slice(0, 40) : undefined,
            contentHash: m.contentHash ? String(m.contentHash).slice(0, 16) : undefined,
            createdAt: Number(m.createdAt) || undefined,
            text: truncatePersistedText(m.text || ""),
            ...(m.modelText
              ? { modelText: truncatePersistedText(m.modelText) }
              : {}),
            ...(Array.isArray(m.parts) && m.parts.length
              ? {
                  parts: m.parts
                    .filter((p) => p && typeof p === "object")
                    .slice(0, 8)
                    .map((p) => ({
                      id: p.id != null ? String(p.id).slice(0, 40) : undefined,
                      kind: String(p.kind || "file").slice(0, 24),
                      name: String(p.name || "").slice(0, 80),
                      title: String(p.title || p.name || "").slice(0, 48),
                      subtitle: String(p.subtitle || "").slice(0, 32),
                      size: Number(p.size) || 0,
                      uri: typeof p.uri === "string" ? p.uri.slice(0, 256) : undefined,
                      text: String(p.text || "").slice(0, MAX_DRAFT_PART_TEXT),
                      version: Number(p.version) || 1,
                      hash: p.hash ? String(p.hash).slice(0, 16) : undefined,
                      semanticRole:
                        p.semanticRole === "user_input" ? "user_input" : "reference_material",
                      authority: p.authority === "user" ? "user" : "untrusted_content",
                    })),
                }
              : {}),
            ...(m.thinking
              ? { thinking: truncatePersistedText(m.thinking) }
              : {}),
            ...(m.run && typeof m.run === "object"
              ? {
                  run: {
                    requestedEffort: String(m.run.requestedEffort || "").slice(0, 16),
                    effectiveEffort: String(m.run.effectiveEffort || "").slice(0, 24),
                    degraded: Boolean(m.run.degraded),
                    contextHash: m.run.contextHash
                      ? String(m.run.contextHash).slice(0, 16)
                      : undefined,
                  },
                }
              : {}),
            ...(m.turn && typeof m.turn === "object"
              ? {
                  turn: {
                    turnId: String(m.turn.turnId || "").slice(0, 80),
                    providerRunId: m.turn.providerRunId
                      ? String(m.turn.providerRunId).slice(0, 80)
                      : undefined,
                    state: String(m.turn.state || "").slice(0, 24),
                    contextManifestId: String(m.turn.contextManifestId || "").slice(0, 16),
                    semanticContextHash: String(m.turn.semanticContextHash || "").slice(0, 16),
                    providerPayloadHash: String(m.turn.providerPayloadHash || "").slice(0, 16),
                    requestedReasoning: String(m.turn.requestedReasoning || "").slice(0, 24),
                    effectiveReasoning: String(m.turn.effectiveReasoning || "").slice(0, 24),
                    estimatedInputTokens: Number(m.turn.estimatedInputTokens ?? m.turn.inputTokens) || 0,
                    transcriptChars: Number(m.turn.transcriptChars) || 0,
                    provider: String(m.turn.provider || "").slice(0, 40),
                    model: String(m.turn.model || "").slice(0, 80),
                    startedAt: Number(m.turn.startedAt) || undefined,
                    completedAt: Number(m.turn.completedAt) || undefined,
                  },
                }
              : {}),
            /* Keep artifact cards across refresh: id/kind/title/subtitle +
               media pointer (mediaUrl/mediaType) or capsule target/query. */
            ...(Array.isArray(m.artifacts) && m.artifacts.length
              ? {
                  artifacts: m.artifacts
                    .slice(0, 6)
                    .map((a) => (a && typeof a === "object" ? a : null))
                    .filter(Boolean)
                    .map((a) => ({
                      id: a.id != null ? String(a.id).slice(0, 120) : undefined,
                      kind: String(a.kind || "document").slice(0, 24),
                      title: truncatePersistedText(a.title || ""),
                      subtitle: truncatePersistedText(a.subtitle || a.detail || ""),
                      mediaUrl: typeof a.mediaUrl === "string" ? a.mediaUrl : undefined,
                      mediaType: a.mediaType === "image" ? "image" : a.mediaType === "video" ? "video" : undefined,
                      target: typeof a.target === "string" ? a.target.slice(0, 80) : undefined,
                      query: a.query && typeof a.query === "object" ? a.query : undefined,
                      /* Text/code artifacts carry their bytes inline (bounded) so the
                         card can reopen into Documents after a refresh — the chat-
                         attachment path needs the content, no Library object. */
                      fileName: typeof a.fileName === "string" ? a.fileName.slice(0, 120) : undefined,
                      mimeType: typeof a.mimeType === "string" ? a.mimeType.slice(0, 80) : undefined,
                      text: typeof a.text === "string" ? truncatePersistedText(a.text) : undefined,
                    })),
                }
              : {}),
          };
        })
        .filter(Boolean)
    : [];
  const tags = Array.isArray(session.tags)
    ? session.tags
        .map((t) => String(t || "").trim().slice(0, 24))
        .filter(Boolean)
        .slice(0, 6)
    : [];
  return {
    id: String(session.id).slice(0, 80),
    title: String(session.title || "Chat").slice(0, 64),
    group: session.group === "Earlier" ? "Earlier" : "Today",
    pinned: Boolean(session.pinned),
    archived: Boolean(session.archived),
    projectId: session.projectId ? String(session.projectId).slice(0, 80) : null,
    mode: session.mode === "build" ? "build" : "chat",
    tags,
    forkedFrom: session.forkedFrom ? String(session.forkedFrom).slice(0, 80) : null,
    updatedAt: Number(session.updatedAt) || Date.now(),
    messages,
    ...(session.lastTurn && typeof session.lastTurn === "object"
      ? { lastTurn: cheapTurnSnapshot(session.lastTurn) }
      : {}),
  };
}

function serializeComposerDraft(raw) {
  if (!raw || typeof raw !== "object") {
    return undefined;
  }
  const parts = Array.isArray(raw.parts)
    ? raw.parts
        .filter((p) => p && typeof p === "object")
        .slice(0, 16)
        .map((p) => ({
          id: p.id != null ? String(p.id).slice(0, 40) : undefined,
          kind: String(p.kind || "text").slice(0, 24),
          name: String(p.name || "").slice(0, 180),
          title: String(p.title || p.name || "").slice(0, 48),
          subtitle: String(p.subtitle || "").slice(0, 32),
          size: Number(p.size) || String(p.text || "").length || 0,
          uri: typeof p.uri === "string" ? p.uri.slice(0, 1024) : "",
          text: String(p.text || "").slice(0, MAX_DRAFT_PART_TEXT),
          semanticRole: p.semanticRole === "user_input" ? "user_input" : "reference_material",
          authority: p.authority === "user" ? "user" : "untrusted_content",
          version: Number(p.version) || 1,
        }))
    : [];
  const text = String(raw.text || "").slice(0, MAX_DRAFT_PART_TEXT);
  if (!text && !parts.length) {
    return undefined;
  }
  return { text, parts };
}

export function getAgentWorkspaceSnapshot() {
  if (!store) {
    return null;
  }
  /* Never snapshot before the saved workspace has been applied: a pre-hydration
     persist would otherwise overwrite durable chat sessions with an empty list
     (data loss on refresh). */
  if (store.getWorkspaceHydrated?.() === false) {
    return null;
  }
  const sessionMode = store.getSessionMode();
  const toolMode = store.getToolMode();
  const composerDraft = serializeComposerDraft(store.getComposerDraft?.());
  return {
    v: AGENT_WORKSPACE_V,
    activeSessionId: store.getActiveSessionId(),
    sessionMode: sessionMode === "build" ? "build" : "chat",
    toolMode: toolMode === "ask" || toolMode === "full" ? toolMode : "read",
    systemPrompt: normalizeLiveSystemPrompt(store.getSystemPrompt?.() || ""),
    agentNotes: normalizeAgentNotes(store.getAgentNotes?.() || ""),
    maxTokens: clampLiveMaxTokens(store.getMaxTokens?.()),
    temperature: clampLiveTemperature(store.getTemperature?.()),
    reasoningEffort:
      store.getReasoningEffort?.() === "low" || store.getReasoningEffort?.() === "high"
        ? store.getReasoningEffort()
        : "medium",
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
    ...(composerDraft ? { composerDraft } : {}),
  };
}

export function applyAgentWorkspaceSnapshot(raw) {
  if (!store) {
    return false;
  }
  /* Mark hydrated even when there's no valid saved blob (fresh install): otherwise
     the hydration guard would stay on forever and the workspace would never persist. */
  store.setWorkspaceHydrated(true);
  if (!raw || typeof raw !== "object" || Number(raw.v) !== AGENT_WORKSPACE_V) {
    return false;
  }
  if (Array.isArray(raw.projects)) {
    replaceProjects(raw.projects);
  }
  let sessions = store.getSessions();
  if (Array.isArray(raw.sessions) && raw.sessions.length) {
    sessions = raw.sessions.map(serializeSessionForPersist).filter(Boolean).map(recoverStalePersistedTurn);
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
  if (typeof raw.agentNotes === "string") {
    store.setAgentNotes?.(normalizeAgentNotes(raw.agentNotes));
  }
  if (raw.maxTokens != null) {
    store.setMaxTokens?.(clampLiveMaxTokens(raw.maxTokens));
  }
  if (raw.temperature != null) {
    store.setTemperature?.(clampLiveTemperature(raw.temperature));
  }
  if (raw.reasoningEffort === "low" || raw.reasoningEffort === "medium" || raw.reasoningEffort === "high") {
    store.setReasoningEffort?.(raw.reasoningEffort);
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
  if (raw.composerDraft && typeof raw.composerDraft === "object") {
    store.applyComposerDraft?.(serializeComposerDraft(raw.composerDraft) || { text: "", parts: [] });
  }
  return true;
}

export function persistAgentWorkspaceSoon() {
  scheduleAgentWorkspacePersist();
}
