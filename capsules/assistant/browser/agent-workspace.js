/* Agent workspace snapshot — host session.agent persist seam.
   Harness binds store accessors at boot (avoids circular state imports).
   UI ≠ authority (Principle 16). */

import {
  listProjects,
  replaceProjects,
  setReasoningVisible,
  getUsageLedger,
  applyUsageLedger,
} from "./agent-state.js";
import {
  clampLiveMaxTokens,
  normalizeLiveSystemPrompt,
  normalizeAgentNotes,
  selectLiveOffer,
  liveOfferChoice,
  liveContentChoice,
} from "./agent-live.js";
import { recoverStalePersistedTurn } from "./agent-stream-qos.js";
import { scheduleAgentWorkspacePersist } from "./harness-host.js";
import { validModelCid } from "./model-selection.js";
import { cloneWorkspace } from "./workspace-transition.js";

export const AGENT_WORKSPACE_V = 1;
export const MAX_DRAFT_PART_TEXT = 40_000;

/** @type {null | Record<string, Function>} */
let store = null;

export function bindAgentWorkspaceStore(next = {}) {
  store = next;
}

// Durable history is a complete JSON record. Context and display budgets apply
// at inference/render time; a save must never shorten existing user work.
export function serializeSessionForPersist(session) {
  if (!session || typeof session !== "object" || !session.id) return null;
  return JSON.parse(JSON.stringify({
    ...session,
    title: session.title ?? "Chat",
    group: session.group ?? "Today",
    messages: Array.isArray(session.messages) ? session.messages : [],
  }));
}

function serializeComposerDraft(raw) {
  return raw && typeof raw === "object" ? JSON.parse(JSON.stringify(raw)) : undefined;
}

let originalDocument = {};
let hasAppliedWorkspace = false;

/** Capture before changing active identity, including unsent attachments. */
export function captureActiveSessionState() {
  if (!store || store.getWorkspaceHydrated?.() === false) return;
  const session = store.getSessions().find((item) => item.id === store.getActiveSessionId());
  if (!session) return;
  const draft = store.getSessionMode() === "studio"
    ? { ...session.composerDraft, text: session.studio?.studioDraft ?? session.composerDraft?.text ?? "", parts: session.composerDraft?.parts ?? [] }
    : serializeComposerDraft(store.getComposerDraft?.());
  if (draft) session.composerDraft = { ...session.composerDraft, ...draft };
  if (draft && store.getSessionMode() !== "studio") session.chatComposerDraft = cloneWorkspace(session.composerDraft);
  session.modelSelection = {
    ...session.modelSelection,
    liveOfferId: liveOfferChoice() || "",
    selectedModelCid: liveContentChoice() ?? null,
  };
}

/** Imported source selections remain available until the user chooses another. */
export function restoreSessionState(session, { draft = session?.composerDraft } = {}) {
  if (!store) return;
  const mode = session?.mode === "studio" ? "studio" : session?.mode === "build" ? "build" : "chat";
  if (mode === "studio" && typeof session?.studio?.studioDraft === "string") draft = { ...draft, text: session.studio.studioDraft, parts: draft?.parts ?? [] };
  if (mode !== "studio" && session?.chatComposerDraft) draft = session.chatComposerDraft;
  store.setSessionMode(mode);
  store.applyComposerDraft?.(serializeComposerDraft(draft) || { text: "", parts: [] });
  const selection = session?.modelSelection ?? originalDocument.legacyImports?.[session?.legacyOrigin?.source]?.selection;
  if (selection) selectLiveOffer(selection.liveOfferId ?? "", selection.selectedModelCid ?? null);
  globalThis.window?.dispatchEvent(new CustomEvent("assistant:session-selected", {
    detail: { mode, sessionId: session?.id ?? null, composerDraft: serializeComposerDraft(draft) || { text: "", parts: [] }, studio: cloneWorkspace(session?.studio) },
  }));
}
export function setAssistantWorkspaceField(key, value) {
  if (JSON.stringify(originalDocument[key]) === JSON.stringify(value)) return;
  originalDocument[key] = structuredClone(value);
  scheduleAgentWorkspacePersist();
}

export function getAssistantSession(sessionId = store?.getActiveSessionId()) {
  return cloneWorkspace(store?.getSessions().find(session => session.id === sessionId));
}

/** Chat and Studio keep separate editable drafts within the same session. */
export function applyAssistantModeDraft(mode) {
  const session = store?.getSessions().find(item => item.id === store.getActiveSessionId());
  if (!session) return;
  const draft = mode === "studio"
    ? { text: session.studio?.studioDraft ?? (session.mode === "studio" ? session.composerDraft?.text : "") ?? "", parts: session.mode === "studio" ? session.composerDraft?.parts ?? [] : [] }
    : session.chatComposerDraft ?? (session.mode !== "studio" ? session.composerDraft : null) ?? { text: "", parts: [] };
  session.composerDraft = cloneWorkspace(draft);
  if (mode !== "studio") store.applyComposerDraft?.(cloneWorkspace(draft));
}

export function ensureAssistantStudioSession() {
  if (!store || store.getWorkspaceHydrated?.() === false) return null;
  let session = store.getSessions().find(item => item.id === store.getActiveSessionId());
  if (!session) {
    session = { id: `studio-${globalThis.crypto.randomUUID()}`, title: "Studio", mode: "studio", group: "Today", messages: [] };
    store.setSessions([session, ...store.getSessions()]);
    store.setActiveSessionId(session.id);
    scheduleAgentWorkspacePersist();
  }
  return cloneWorkspace(session);
}

export function setAssistantSessionStudio(sessionId, studio, originalSession = null) {
  if (!store || !sessionId) return false;
  let session = store.getSessions().find(item => item.id === sessionId);
  if (!session) {
    if (!originalSession) return false;
    session = { ...cloneWorkspace(originalSession), id: sessionId, archived: false };
    store.setSessions([session, ...store.getSessions()]);
  }
  if (JSON.stringify(session.studio) === JSON.stringify(studio)) return true;
  session.studio = { ...session.studio, ...cloneWorkspace(studio) };
  if (session.mode === "studio") session.composerDraft = { ...session.composerDraft, text: studio.studioDraft ?? "", parts: session.composerDraft?.parts ?? [] };
  scheduleAgentWorkspacePersist();
  return true;
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
  captureActiveSessionState();
  const sessionMode = store.getSessionMode();
  const active = store.getSessions().find((session) => session.id === store.getActiveSessionId());
  const composerDraft = {
    ...(active?.composerDraft ?? originalDocument.composerDraft),
    ...(sessionMode === "studio" ? active?.composerDraft || { text: "", parts: [] } : serializeComposerDraft(store.getComposerDraft?.()) || { text: "", parts: [] }),
  };
  const snapshot = cloneWorkspace({
    ...originalDocument,
    v: AGENT_WORKSPACE_V,
    activeSessionId: store.getActiveSessionId(),
    sessionMode: sessionMode === "studio" ? "studio" : sessionMode === "build" ? "build" : "chat",
    liveOfferId: String(liveOfferChoice() || ""),
    selectedModelCid: liveContentChoice() ?? null,
    systemPrompt: normalizeLiveSystemPrompt(store.getSystemPrompt?.() || ""),
    agentNotes: normalizeAgentNotes(store.getAgentNotes?.() || ""),
    maxTokens: clampLiveMaxTokens(store.getMaxTokens?.()),
    reasoningEffort:
      store.getReasoningEffort?.() === "low" || store.getReasoningEffort?.() === "high"
        ? store.getReasoningEffort()
        : "medium",
    reasoningVisible: Boolean(store.getReasoningVisible()),
    projects: listProjects(),
    usageTurns: getUsageLedger(),
    sessions: store
      .getSessions()
      .map(serializeSessionForPersist),
    composerDraft,
  });
  if (!validWorkspace(snapshot)) {
    store.setWorkspaceHydrated(false);
    return null;
  }
  return snapshot;
}

const isRecord = (value) => value !== null && typeof value === "object" && !Array.isArray(value);
const validDraft = (value) => value === undefined || (isRecord(value) &&
  (value.text === undefined || typeof value.text === "string") &&
  (value.parts === undefined || Array.isArray(value.parts)));
const validSelection = (value) => isRecord(value) &&
  (value.liveOfferId === undefined || typeof value.liveOfferId === "string") &&
  (value.selectedModelCid == null || (validModelCid(value.selectedModelCid) &&
    typeof value.liveOfferId === "string" && Boolean(value.liveOfferId.trim()) && value.liveOfferId.length <= 200));
const validStudio = (value) => value === undefined || (isRecord(value) &&
  (value.studioDraft === undefined || typeof value.studioDraft === "string") &&
  (value.activeRun == null || isRecord(value.activeRun)) &&
  (value.studioHistory === undefined || (Array.isArray(value.studioHistory) && value.studioHistory.every(isRecord))));
function validWorkspace(raw) {
  if (!isRecord(raw) || raw.v !== AGENT_WORKSPACE_V || !validDraft(raw.composerDraft) || !validSelection(raw)) return false;
  for (const key of ["sessions", "projects", "usageTurns"]) {
    if (raw[key] !== undefined && (!Array.isArray(raw[key]) || raw[key].some((item) => !isRecord(item)))) return false;
  }
  for (const key of ["sessions", "projects"]) {
    const records = raw[key] || [];
    if (records.some((item) => typeof item.id !== "string" || !item.id || (item.title !== undefined && typeof item.title !== "string")) ||
        new Set(records.map((item) => item.id)).size !== records.length) return false;
  }
  return (raw.sessions || []).every((session) =>
    (session.messages === undefined || (Array.isArray(session.messages) && session.messages.every(isRecord))) &&
    validDraft(session.composerDraft) && validDraft(session.chatComposerDraft) && validStudio(session.studio) &&
    (session.modelSelection === undefined || validSelection(session.modelSelection)));
}

export function applyAgentWorkspaceSnapshot(raw) {
  if (!store) {
    return false;
  }
  // Only null means a fresh workspace. Unsupported records block persistence.
  raw = raw === null ? { v: AGENT_WORKSPACE_V, sessions: [], projects: [] } : raw;
  if (!validWorkspace(raw)) {
    store.setWorkspaceHydrated(false);
    return false;
  }
  store.setWorkspaceHydrated(false);
  originalDocument = cloneWorkspace(raw);
  replaceProjects(raw.projects || []);
  let sessions = (raw.sessions || []).map(serializeSessionForPersist);
  // A reload recovers stale turns; a same-page 409 retains the live run state.
  if (!hasAppliedWorkspace) sessions = sessions.map(recoverStalePersistedTurn);
  store.setSessions(sessions);
  if (raw.activeSessionId && sessions.some((s) => s.id === raw.activeSessionId)) {
    store.setActiveSessionId(raw.activeSessionId);
  } else {
    store.setActiveSessionId(sessions[0]?.id || null);
  }
  selectLiveOffer(raw.liveOfferId ?? "", raw.selectedModelCid ?? null);
  if (typeof raw.systemPrompt === "string") {
    store.setSystemPrompt?.(normalizeLiveSystemPrompt(raw.systemPrompt));
  }
  if (typeof raw.agentNotes === "string") {
    store.setAgentNotes?.(normalizeAgentNotes(raw.agentNotes));
  }
  if (raw.maxTokens != null) {
    store.setMaxTokens?.(clampLiveMaxTokens(raw.maxTokens));
  }
  if (raw.reasoningEffort === "low" || raw.reasoningEffort === "medium" || raw.reasoningEffort === "high") {
    store.setReasoningEffort?.(raw.reasoningEffort);
  }
  if (typeof raw.reasoningVisible === "boolean") {
    store.setReasoningVisible(setReasoningVisible(raw.reasoningVisible));
  }
  applyUsageLedger(raw.usageTurns || []);
  const active = sessions.find((session) => session.id === store.getActiveSessionId());
  const draft = raw.composerDraft ?? active?.composerDraft;
  if (active && draft) active.composerDraft = { ...active.composerDraft, ...cloneWorkspace(draft) };
  restoreSessionState(active ? { ...active, mode: active.mode ?? raw.sessionMode } : { mode: raw.sessionMode },
    { draft });
  hasAppliedWorkspace = true;
  store.setWorkspaceHydrated(true);
  return true;
}

export function persistAgentWorkspaceSoon() {
  scheduleAgentWorkspacePersist();
}
