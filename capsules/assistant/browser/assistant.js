import {
  createHomeClipboardClient,
  MAX_HOME_CLIPBOARD_TEXT_UTF8_BYTES,
} from "/apps/home/home-clipboard-client.js?v=home-20260726a";

const MODEL_TEXT_INPUT_SCHEMA = "elastos.model.input.text/v1";
const MODEL_TEXT_OUTPUT_SCHEMA = "elastos.model.output.text/v1";
const ASSISTANT_WORKSPACE_SCHEMA = "elastos.assistant.workspace/v1";
const ASSISTANT_CLIPBOARD_PURPOSE = "transcript.markdown";
const MODE_CHAT = "chat";
const MODE_BUILD = "build";
const MAX_SESSION_ID_BYTES = 128;
const MAX_SESSION_TITLE_BYTES = 160;
const MAX_MESSAGE_CONTENT_BYTES = 8 * 1024;
const MAX_DRAFT_BYTES = 16 * 1024;
const MAX_SESSIONS = 24;
const MAX_MESSAGES_PER_SESSION = 64;
const POLL_DELAY_MS = 400;
const SAVE_DEBOUNCE_MS = 180;
const MAX_POLL_ERROR_ATTEMPTS = 4;
const MAX_POLL_ERROR_WINDOW_MS = 12_000;
const MAX_IMMEDIATE_EVENT_PAGES = 8;
const TEXT_ENCODER = new TextEncoder();

const CHAT_STARTERS = Object.freeze([
  "Summarize the goal and the next best step.",
  "Draft a concise plan I can review.",
  "Turn this idea into a clean starting brief.",
]);

const BUILD_STARTERS = Object.freeze([
  "Break this build into the smallest safe steps.",
  "List the risks, guards, and first patch.",
  "Draft a focused implementation checklist.",
]);

function boundedText(value, maxBytes) {
  const text = String(value ?? "");
  if (TEXT_ENCODER.encode(text).length <= maxBytes) {
    return text;
  }
  let bytes = 0;
  let bounded = "";
  for (const codePoint of text) {
    const nextBytes = TEXT_ENCODER.encode(codePoint).length;
    if (bytes + nextBytes > maxBytes) {
      break;
    }
    bounded += codePoint;
    bytes += nextBytes;
  }
  return bounded;
}

function defaultSessionTitle(mode) {
  return mode === MODE_BUILD ? "New build" : "New chat";
}

function titleFromPrompt(prompt, mode) {
  const cleaned = String(prompt ?? "")
    .replace(/\s+/g, " ")
    .trim();
  if (!cleaned) {
    return defaultSessionTitle(mode);
  }
  return cleaned.length > 60 ? cleaned.slice(0, 59).trimEnd() : cleaned;
}

function sessionPreview(session) {
  const messages = Array.isArray(session?.messages) ? session.messages : [];
  for (let index = messages.length - 1; index >= 0; index -= 1) {
    const content = String(messages[index]?.content ?? "").trim();
    if (content) {
      return content;
    }
  }
  return session?.mode === MODE_BUILD ? "Build session" : "Chat session";
}

function normalizeWorkspace(workspace) {
  const sessions = Array.isArray(workspace?.sessions)
    ? workspace.sessions.slice(0, MAX_SESSIONS).map((session) => ({
        id: boundedText(session?.id ?? "", MAX_SESSION_ID_BYTES).trim(),
        title: boundedText(session?.title ?? "", MAX_SESSION_TITLE_BYTES),
        mode: session?.mode === MODE_BUILD ? MODE_BUILD : MODE_CHAT,
        pinned: Boolean(session?.pinned),
        messages: Array.isArray(session?.messages)
          ? session.messages.slice(0, MAX_MESSAGES_PER_SESSION).map((message) => ({
              role:
                message?.role === "assistant" ||
                message?.role === "system" ||
                message?.role === "tool"
                  ? message.role
                  : "user",
              content: boundedText(message?.content ?? "", MAX_MESSAGE_CONTENT_BYTES),
              ...(typeof message?.run_id === "string" ? { run_id: message.run_id } : {}),
            }))
          : [],
      }))
    : [];
  return {
    schema: ASSISTANT_WORKSPACE_SCHEMA,
    revision: Number(workspace?.revision) || 0,
    sessions: sessions.filter((session) => session.id.length > 0),
    draft: boundedText(workspace?.draft ?? "", MAX_DRAFT_BYTES),
    selected_offer_id:
      typeof workspace?.selected_offer_id === "string" && workspace.selected_offer_id.trim()
        ? workspace.selected_offer_id
        : null,
  };
}

function eligibleTextOffers(payload) {
  const offers = Array.isArray(payload?.offers)
    ? payload.offers
    : Array.isArray(payload?.data?.offers)
      ? payload.data.offers
      : [];
  return offers.filter((offer) =>
    offer &&
    typeof offer.id === "string" &&
    typeof offer.title === "string" &&
    typeof offer.operation === "string" &&
    Array.isArray(offer.input_modalities) &&
    Array.isArray(offer.output_modalities) &&
    offer.input_modalities.includes("text") &&
    offer.output_modalities.includes("text"),
  );
}

function readStatusMessage(payload, fallback) {
  return typeof payload?.message === "string" && payload.message.trim()
    ? payload.message
    : fallback;
}

function parseRunView(payload) {
  const view = payload?.data ?? payload;
  return view && typeof view === "object" ? view : null;
}

function parseRunEventsPage(payload) {
  const page = payload?.data ?? payload;
  return page && typeof page === "object" && Array.isArray(page.events) ? page : null;
}

function parseCursorValue(value) {
  const cursor = Number(value);
  return Number.isInteger(cursor) && cursor >= 0 ? cursor : null;
}

function createSessionRecord({ id, mode, title = "", pinned = false } = {}) {
  const nextMode = mode === MODE_BUILD ? MODE_BUILD : MODE_CHAT;
  return {
    id,
    title: title || defaultSessionTitle(nextMode),
    mode: nextMode,
    pinned: Boolean(pinned),
    messages: [],
  };
}

function transcriptMessages(session) {
  return Array.isArray(session?.messages)
    ? session.messages.filter((message) => {
        const role = message?.role;
        return (
          (role === "user" || role === "assistant") &&
          String(message?.content ?? "").trim().length > 0
        );
      })
    : [];
}

function transcriptMarkdown(session) {
  const messages = transcriptMessages(session);
  if (!messages.length) {
    return "";
  }
  const markdown = [
    "# Assistant transcript",
    "",
    ...messages.flatMap((message) => [
      `## ${message.role === "assistant" ? "Assistant" : "User"}`,
      "",
      String(message.content ?? ""),
      "",
    ]),
  ].join("\n").trimEnd();
  if (TEXT_ENCODER.encode(markdown).length <= MAX_HOME_CLIPBOARD_TEXT_UTF8_BYTES) {
    return markdown;
  }
  const note = "\n\n> Note: Transcript truncated to fit the trusted Home Clipboard limit.";
  const maxBytes = Math.max(
    0,
    MAX_HOME_CLIPBOARD_TEXT_UTF8_BYTES - TEXT_ENCODER.encode(note).length,
  );
  const trimmed = boundedText(markdown, maxBytes).trimEnd();
  return `${trimmed}${note}`;
}

function readClipboardStatus(error) {
  const code = typeof error?.code === "string" ? error.code : "";
  if (code === "unavailable") {
    return "Clipboard unavailable.";
  }
  if (code === "cancelled") {
    return "Transcript copy cancelled.";
  }
  if (code === "denied") {
    return "Transcript copy denied.";
  }
  if (code === "timeout") {
    return "Clipboard request timed out.";
  }
  return "Transcript copy failed.";
}

function sortSessions(sessions) {
  return [...sessions].sort((left, right) => {
    if (Boolean(left.pinned) !== Boolean(right.pinned)) {
      return left.pinned ? -1 : 1;
    }
    return left.title.localeCompare(right.title);
  });
}

function findSession(sessions, sessionId) {
  return sessions.find((session) => session.id === sessionId) || null;
}

function serializeWorkspace(state) {
  return {
    schema: ASSISTANT_WORKSPACE_SCHEMA,
    if_revision: state.workspaceRevision,
    sessions: state.sessions.slice(0, MAX_SESSIONS).map((session) => ({
      id: session.id,
      title: boundedText(session.title, MAX_SESSION_TITLE_BYTES),
      mode: session.mode,
      pinned: Boolean(session.pinned),
      messages: session.messages.slice(0, MAX_MESSAGES_PER_SESSION).map((message) => ({
        role: message.role,
        content: boundedText(message.content, MAX_MESSAGE_CONTENT_BYTES),
        ...(message.run_id ? { run_id: message.run_id } : {}),
      })),
    })),
    draft: boundedText(state.draft, MAX_DRAFT_BYTES),
    selected_offer_id: state.selectedOfferId,
  };
}

function initialState(homeToken) {
  return {
    homeToken,
    offersLoading: true,
    workspaceLoading: true,
    offersError: "",
    workspaceError: "",
    statusMessage: "Loading model offers...",
    conflictMessage: "",
    offers: [],
    sessions: [],
    draft: "",
    searchQuery: "",
    selectedOfferId: null,
    activeSessionId: "",
    deletingSessionId: "",
    renamingSessionId: "",
    renameValue: "",
    workspaceRevision: 0,
    activeRun: null,
    clipboardStatusMessage: "",
    copyingTranscript: false,
    pollTimer: 0,
    saveTimer: 0,
    saving: false,
    saveQueued: false,
    workspaceVersion: 0,
    savedWorkspaceVersion: 0,
  };
}

export function createAssistantApp({
  homeToken = "",
  homeOrigin = "null",
  fetchFn = globalThis.fetch,
  cryptoRef = globalThis.crypto,
  nowFn = () => Date.now(),
  setTimeoutFn = globalThis.setTimeout?.bind(globalThis),
  clearTimeoutFn = globalThis.clearTimeout?.bind(globalThis),
  targetWindow = globalThis.window?.top,
  sourceWindow = globalThis.window,
  homeClipboardClientFactory = createHomeClipboardClient,
  onStateChange = () => {},
} = {}) {
  const state = initialState(homeToken);
  const homeClipboard = homeClipboardClientFactory({
    targetId: "assistant",
    homeOrigin,
    homeToken,
    targetWindow,
    sourceWindow,
    cryptoRef,
    setTimeoutFn,
    clearTimeoutFn,
  });

  function snapshot() {
    const currentSession = findSession(state.sessions, state.activeSessionId);
    const canCopyTranscript = transcriptMessages(currentSession).length > 0;
    const needle = state.searchQuery.trim().toLowerCase();
    const filteredSessions = sortSessions(state.sessions).filter((session) => {
      if (!needle) {
        return true;
      }
      if (session.title.toLowerCase().includes(needle)) {
        return true;
      }
      return session.messages.some((message) =>
        String(message.content || "").toLowerCase().includes(needle),
      );
    });
    return {
      ...state,
      currentSession,
      filteredSessions,
      offersReady: state.offers.filter((offer) => offer.id && offer.operation),
      starters: (currentSession?.mode || MODE_CHAT) === MODE_BUILD ? BUILD_STARTERS : CHAT_STARTERS,
      copyStatusMessage: state.clipboardStatusMessage,
      copyDisabled:
        !homeToken ||
        !canCopyTranscript ||
        Boolean(state.activeRun && !state.activeRun.terminal) ||
        state.copyingTranscript,
      copyTitle:
        state.copyingTranscript
          ? "Copying transcript..."
          : state.activeRun && !state.activeRun.terminal
            ? "Finish the current run before copying."
            : canCopyTranscript
              ? "Copy this conversation as Markdown."
              : "Nothing to copy yet.",
      sendDisabled:
        !homeToken ||
        state.offersLoading ||
        state.workspaceLoading ||
        !state.selectedOfferId ||
        !state.draft.trim() ||
        Boolean(state.activeRun && !state.activeRun.terminal),
    };
  }

  function notify() {
    onStateChange(snapshot());
  }

  function markWorkspaceDirty() {
    state.workspaceVersion += 1;
    scheduleSave();
  }

  function setStatus(message) {
    state.statusMessage = message;
    notify();
  }

  function setClipboardStatus(message) {
    state.clipboardStatusMessage = message;
    notify();
  }

  function ensureActiveSession(mode = MODE_CHAT) {
    let session = findSession(state.sessions, state.activeSessionId);
    if (session) {
      return session;
    }
    session = createSessionRecord({
      id: `session-${cryptoRef.randomUUID()}`,
      mode,
    });
    state.sessions = [session, ...state.sessions].slice(0, MAX_SESSIONS);
    state.activeSessionId = session.id;
    notify();
    return session;
  }

  function resolveCurrentSessionMode() {
    return findSession(state.sessions, state.activeSessionId)?.mode || MODE_CHAT;
  }

  async function requestJson(url, body, method = "POST") {
    if (!homeToken) {
      throw new Error("Assistant requires a Home launch token.");
    }
    const response = await fetchFn(url, {
      method,
      headers: {
        ...(method === "POST" ? { "content-type": "application/json" } : {}),
        "x-elastos-home-token": homeToken,
      },
      ...(method === "POST" ? { body: JSON.stringify(body ?? {}) } : {}),
    });
    let payload = null;
    try {
      payload = await response.json();
    } catch {
      payload = null;
    }
    return { response, payload };
  }

  async function loadOffers() {
    if (!homeToken) {
      state.offersLoading = false;
      state.offersError = "Model provider unavailable.";
      state.statusMessage = state.offersError;
      notify();
      return;
    }
    const { response, payload } = await requestJson("/api/provider/model/offers_list", {});
    if (!response.ok || payload?.status === "error") {
      state.offersLoading = false;
      state.offersError = readStatusMessage(payload, "Model provider unavailable.");
      state.statusMessage = state.offersError;
      notify();
      return;
    }
    state.offers = eligibleTextOffers(payload);
    state.offersLoading = false;
    if (!state.offers.length) {
      state.selectedOfferId = null;
      state.statusMessage = "No model offers available.";
      notify();
      return;
    }
    if (!state.selectedOfferId || !state.offers.some((offer) => offer.id === state.selectedOfferId)) {
      state.selectedOfferId = state.offers[0].id;
    }
    if (!state.workspaceLoading && state.offers.length) {
      state.statusMessage = "";
    }
    notify();
  }

  async function loadWorkspace({ setNotice = false } = {}) {
    if (!homeToken) {
      state.workspaceLoading = false;
      state.workspaceError = "Assistant workspace unavailable.";
      notify();
      return;
    }
    const { response, payload } = await requestJson("/api/apps/assistant/workspace", null, "GET");
    if (!response.ok) {
      state.workspaceLoading = false;
      state.workspaceError = "Assistant workspace unavailable.";
      if (!state.statusMessage) {
        state.statusMessage = state.workspaceError;
      }
      notify();
      return;
    }
    const workspace = normalizeWorkspace(payload);
    state.workspaceRevision = workspace.revision;
    state.sessions = workspace.sessions;
    state.draft = workspace.draft;
    state.selectedOfferId = workspace.selected_offer_id;
    state.workspaceVersion = 0;
    state.savedWorkspaceVersion = 0;
    state.saveQueued = false;
    state.workspaceLoading = false;
    state.workspaceError = "";
    state.conflictMessage = setNotice
      ? "Workspace changed elsewhere. Reloaded the latest saved state."
      : "";
    if (!state.sessions.length) {
      const blank = createSessionRecord({
        id: `session-${cryptoRef.randomUUID()}`,
        mode: MODE_CHAT,
      });
      state.sessions = [blank];
      state.activeSessionId = blank.id;
    } else if (!findSession(state.sessions, state.activeSessionId)) {
      state.activeSessionId = state.sessions[0].id;
    }
    if (state.offers.length && (!state.selectedOfferId || !state.offers.some((offer) => offer.id === state.selectedOfferId))) {
      state.selectedOfferId = state.offers[0].id;
    }
    if (!state.offersLoading && !state.offersError && state.offers.length) {
      state.statusMessage = "";
    }
    notify();
  }

  async function saveWorkspace() {
    clearTimeoutFn(state.saveTimer);
    state.saveTimer = 0;
    if (!homeToken) {
      return false;
    }
    if (state.saving) {
      state.saveQueued = true;
      return false;
    }
    if (state.workspaceVersion === state.savedWorkspaceVersion) {
      return true;
    }
    state.saving = true;
    state.saveQueued = false;
    const savingVersion = state.workspaceVersion;
    const requestBody = serializeWorkspace(state);
    notify();
    const { response, payload } = await requestJson(
      "/api/apps/assistant/workspace",
      requestBody,
    );
    if (response.status === 409) {
      state.saving = false;
      await loadWorkspace({ setNotice: true });
      return false;
    }
    if (!response.ok) {
      state.saving = false;
      state.statusMessage = readStatusMessage(payload, "Workspace save failed.");
      notify();
      return false;
    }
    const saved = normalizeWorkspace(payload);
    state.workspaceRevision = saved.revision;
    state.saving = false;
    state.savedWorkspaceVersion = savingVersion;
    if (state.saveQueued || state.workspaceVersion !== savingVersion) {
      scheduleSave();
    }
    notify();
    return true;
  }

  function scheduleSave() {
    if (!homeToken) {
      return;
    }
    if (state.saving) {
      state.saveQueued = true;
      return;
    }
    clearTimeoutFn(state.saveTimer);
    state.saveTimer = setTimeoutFn(() => {
      void saveWorkspace();
    }, SAVE_DEBOUNCE_MS);
  }

  function replaceSession(nextSession) {
    state.sessions = state.sessions.map((session) =>
      session.id === nextSession.id ? nextSession : session,
    );
  }

  function updateStreamingMessage(sessionId, runId, content) {
    const session = findSession(state.sessions, sessionId);
    if (!session) {
      return;
    }
    const messages = [...session.messages];
    const index = messages.findIndex((message) => message.run_id === runId && message.role === "assistant");
    const nextMessage = {
      role: "assistant",
      content: boundedText(content, MAX_MESSAGE_CONTENT_BYTES),
      run_id: runId,
    };
    if (index >= 0) {
      messages[index] = nextMessage;
    } else {
      messages.push(nextMessage);
    }
    session.messages = messages.slice(-MAX_MESSAGES_PER_SESSION);
    replaceSession(session);
  }

  function appendUserMessage(session, prompt) {
    session.messages = [
      ...session.messages,
      {
        role: "user",
        content: boundedText(prompt, MAX_MESSAGE_CONTENT_BYTES),
      },
    ].slice(-MAX_MESSAGES_PER_SESSION);
    if (
      session.messages.filter((message) => message.role === "user").length === 1 &&
      session.title === defaultSessionTitle(session.mode)
    ) {
      session.title = titleFromPrompt(prompt, session.mode);
    }
    replaceSession(session);
  }

  function setSessionMode(mode) {
    const session = ensureActiveSession(mode);
    session.mode = mode === MODE_BUILD ? MODE_BUILD : MODE_CHAT;
    if (session.messages.length === 0) {
      session.title = defaultSessionTitle(session.mode);
    }
    replaceSession(session);
    markWorkspaceDirty();
    notify();
  }

  function createSession(mode = resolveCurrentSessionMode(), { persist = true } = {}) {
    if (state.sessions.length >= MAX_SESSIONS) {
      setStatus("Assistant workspace is full.");
      return null;
    }
    const session = createSessionRecord({
      id: `session-${cryptoRef.randomUUID()}`,
      mode,
    });
    state.sessions = [session, ...state.sessions];
    state.activeSessionId = session.id;
    state.deletingSessionId = "";
    state.renamingSessionId = "";
    state.renameValue = "";
    if (persist) {
      markWorkspaceDirty();
    }
    notify();
    return session;
  }

  function selectSession(sessionId) {
    if (!findSession(state.sessions, sessionId)) {
      return;
    }
    state.activeSessionId = sessionId;
    state.deletingSessionId = "";
    state.renamingSessionId = "";
    state.renameValue = "";
    notify();
  }

  function startRenameSession(sessionId) {
    const session = findSession(state.sessions, sessionId);
    if (!session) {
      return;
    }
    state.renamingSessionId = sessionId;
    state.renameValue = session.title;
    notify();
  }

  function cancelRenameSession() {
    state.renamingSessionId = "";
    state.renameValue = "";
    notify();
  }

  function commitRenameSession() {
    const session = findSession(state.sessions, state.renamingSessionId);
    if (!session) {
      cancelRenameSession();
      return;
    }
    const nextTitle = boundedText(state.renameValue, MAX_SESSION_TITLE_BYTES).trim();
    if (!nextTitle) {
      setStatus("Session title is required.");
      return;
    }
    session.title = nextTitle;
    replaceSession(session);
    state.renamingSessionId = "";
    state.renameValue = "";
    markWorkspaceDirty();
    notify();
  }

  function togglePinSession(sessionId) {
    const session = findSession(state.sessions, sessionId);
    if (!session) {
      return;
    }
    session.pinned = !session.pinned;
    replaceSession(session);
    markWorkspaceDirty();
    notify();
  }

  function requestDeleteSession(sessionId) {
    state.deletingSessionId = state.deletingSessionId === sessionId ? "" : sessionId;
    notify();
  }

  function confirmDeleteSession(sessionId) {
    state.sessions = state.sessions.filter((session) => session.id !== sessionId);
    state.deletingSessionId = "";
    state.renamingSessionId = "";
    state.renameValue = "";
    if (!state.sessions.length) {
      const session = createSessionRecord({
        id: `session-${cryptoRef.randomUUID()}`,
        mode: MODE_CHAT,
      });
      state.sessions = [session];
      state.activeSessionId = session.id;
    } else if (!findSession(state.sessions, state.activeSessionId)) {
      state.activeSessionId = state.sessions[0].id;
    }
    markWorkspaceDirty();
    notify();
  }

  function setSearchQuery(query) {
    state.searchQuery = String(query ?? "");
    notify();
  }

  function setDraft(text) {
    state.draft = boundedText(text, MAX_DRAFT_BYTES);
    markWorkspaceDirty();
    notify();
  }

  function setSelectedOfferId(nextOfferId) {
    state.selectedOfferId = nextOfferId || null;
    markWorkspaceDirty();
    notify();
  }

  function selectedOffer() {
    return state.offers.find((offer) => offer.id === state.selectedOfferId) || null;
  }

  function clearPollTimer() {
    if (state.pollTimer) {
      clearTimeoutFn(state.pollTimer);
      state.pollTimer = 0;
    }
  }

  function settleTerminal(runId, terminal) {
    const run = state.activeRun;
    if (!run || run.runId !== runId) {
      return false;
    }
    run.terminal = true;
    run.status = terminal.status;
    run.output = terminal.output ?? null;
    run.error = terminal.error ?? null;
    if (terminal.status === "completed") {
      if (terminal.output?.schema === MODEL_TEXT_OUTPUT_SCHEMA) {
        updateStreamingMessage(run.sessionId, runId, terminal.output.text || "");
        state.statusMessage = "";
      } else {
        state.statusMessage = "Model run completed without supported text output.";
      }
    } else if (terminal.status === "failed") {
      state.statusMessage = terminal.error?.message || "Model run failed.";
    } else if (terminal.status === "cancelled") {
      state.statusMessage = terminal.error?.message || "Model run cancelled.";
    } else if (terminal.status === "settlement_unknown") {
      state.statusMessage = terminal.error?.message || "Model run settlement unknown.";
    }
    markWorkspaceDirty();
    notify();
    return true;
  }

  function applyRunView(runView) {
    if (
      !runView ||
      !["completed", "failed", "cancelled", "settlement_unknown"].includes(runView.status)
    ) {
      return false;
    }
    return settleTerminal(runView.run_id, {
      status: runView.status,
      output: runView.terminal?.output ?? null,
      error: runView.terminal?.error ?? null,
    });
  }

  function applyRunEvents(events) {
    const run = state.activeRun;
    if (!run) {
      return false;
    }
    let nextText = run.outputText;
    let terminal = null;
    for (const event of events) {
      if (!event || typeof event !== "object") {
        continue;
      }
      if (event.kind === "text_delta") {
        nextText = boundedText(
          nextText + String(event.data?.text ?? ""),
          MAX_MESSAGE_CONTENT_BYTES,
        );
      } else if (event.kind === "output") {
        terminal = {
          status: "completed",
          output: event.data ?? null,
          error: null,
        };
      } else if (event.kind === "failed") {
        terminal = {
          status: "failed",
          output: null,
          error: event.data ?? null,
        };
      } else if (event.kind === "cancelled") {
        terminal = {
          status: "cancelled",
          output: null,
          error: event.data ?? null,
        };
      } else if (event.kind === "settlement_unknown") {
        terminal = {
          status: "settlement_unknown",
          output: null,
          error: event.data ?? null,
        };
      }
    }
    if (nextText !== run.outputText) {
      run.outputText = nextText;
      updateStreamingMessage(run.sessionId, run.runId, nextText);
      markWorkspaceDirty();
    }
    if (terminal) {
      return settleTerminal(run.runId, terminal);
    }
    notify();
    return false;
  }

  function stopPollingUnavailable(run, message) {
    clearPollTimer();
    if (state.activeRun === run && !run?.terminal) {
      state.statusMessage = message;
      notify();
    }
  }

  async function pollRun(run = state.activeRun, immediateDepth = 0) {
    if (!run || run.terminal) {
      clearPollTimer();
      return;
    }
    if (immediateDepth >= MAX_IMMEDIATE_EVENT_PAGES) {
      state.pollTimer = setTimeoutFn(() => {
        void pollRun(run);
      }, POLL_DELAY_MS);
      return;
    }
    const { response, payload } = await requestJson("/api/provider/model/runs_events", {
      run_id: run.runId,
      request_id: cryptoRef.randomUUID(),
      after_sequence: run.afterSequence,
    });
    if (!response.ok || payload?.status === "error") {
      run.pollErrorCount += 1;
      if (
        run.pollErrorCount >= MAX_POLL_ERROR_ATTEMPTS ||
        nowFn() >= run.pollDeadlineAt
      ) {
        stopPollingUnavailable(run, readStatusMessage(payload, "Model provider unavailable."));
        return;
      }
      state.statusMessage = readStatusMessage(payload, "Model provider unavailable.");
      notify();
      state.pollTimer = setTimeoutFn(() => {
        void pollRun(run);
      }, POLL_DELAY_MS);
      return;
    }
    const page = parseRunEventsPage(payload);
    if (!page) {
      stopPollingUnavailable(run, "Model provider unavailable.");
      return;
    }
    const previousCursor = run.afterSequence;
    const nextCursor = parseCursorValue(page.next_cursor);
    if (nextCursor === null || nextCursor < previousCursor) {
      stopPollingUnavailable(run, "Model provider unavailable.");
      return;
    }
    const events = [];
    let lastSequence = previousCursor;
    for (const event of page.events) {
      const sequence = parseCursorValue(event?.sequence);
      if (sequence === null || sequence <= lastSequence) {
        stopPollingUnavailable(run, "Model provider unavailable.");
        return;
      }
      events.push(event);
      lastSequence = sequence;
    }
    if (nextCursor < lastSequence) {
      stopPollingUnavailable(run, "Model provider unavailable.");
      return;
    }
    run.pollErrorCount = 0;
    run.afterSequence = nextCursor;
    const settled = applyRunEvents(events);
    if (settled || run.terminal) {
      clearPollTimer();
      return;
    }
    if (page.has_more) {
      await pollRun(run, immediateDepth + 1);
      return;
    }
    state.pollTimer = setTimeoutFn(() => {
      void pollRun(run);
    }, POLL_DELAY_MS);
  }

  async function sendDraft() {
    const prompt = boundedText(state.draft.trim(), MAX_MESSAGE_CONTENT_BYTES);
    const offer = selectedOffer();
    if (!prompt || !offer || state.offersLoading || state.workspaceLoading) {
      notify();
      return false;
    }
    const session = ensureActiveSession(resolveCurrentSessionMode());
    if (session.messages.length >= MAX_MESSAGES_PER_SESSION) {
      setStatus("Assistant session is full.");
      return false;
    }
    appendUserMessage(session, prompt);
    state.draft = "";
    const { response, payload } = await requestJson("/api/provider/model/runs_create", {
      offer_id: offer.id,
      operation: offer.operation,
      request_id: cryptoRef.randomUUID(),
      input: {
        schema: MODEL_TEXT_INPUT_SCHEMA,
        prompt,
      },
    });
    if (!response.ok || payload?.status === "error") {
      state.statusMessage = readStatusMessage(payload, "Model provider unavailable.");
      markWorkspaceDirty();
      notify();
      return false;
    }
    const runView = parseRunView(payload);
    const runId = runView?.run_id;
    if (typeof runId !== "string" || !runId) {
      state.statusMessage = "Model provider unavailable.";
      notify();
      return false;
    }
    state.activeRun = {
      runId,
      sessionId: session.id,
      afterSequence: parseCursorValue(runView.sequence_cursor) ?? 0,
      outputText: "",
      terminal: false,
      cancelRequested: false,
      status: runView.status || "running",
      output: null,
      error: null,
      pollErrorCount: 0,
      pollDeadlineAt: nowFn() + MAX_POLL_ERROR_WINDOW_MS,
    };
    updateStreamingMessage(session.id, runId, "");
    markWorkspaceDirty();
    notify();
    if (!applyRunView(runView)) {
      void pollRun(state.activeRun);
    }
    return true;
  }

  async function stopRun() {
    const run = state.activeRun;
    if (!run || run.terminal || run.cancelRequested) {
      return false;
    }
    run.cancelRequested = true;
    notify();
    const { response, payload } = await requestJson("/api/provider/model/runs_cancel", {
      run_id: run.runId,
      request_id: cryptoRef.randomUUID(),
    });
    if (!response.ok || payload?.status === "error") {
      state.statusMessage = readStatusMessage(payload, "Model provider unavailable.");
      notify();
      return false;
    }
    const runView = parseRunView(payload);
    if (!applyRunView(runView)) {
      void pollRun(run);
    }
    return true;
  }

  async function initialize() {
    if (homeToken) {
      homeClipboard.start();
    }
    await Promise.all([loadOffers(), loadWorkspace()]);
  }

  async function copyTranscript() {
    const session = findSession(state.sessions, state.activeSessionId);
    if (!session || (state.activeRun && !state.activeRun.terminal)) {
      notify();
      return false;
    }
    const markdown = transcriptMarkdown(session);
    if (!markdown) {
      setClipboardStatus("Nothing to copy yet.");
      return false;
    }
    state.copyingTranscript = true;
    state.clipboardStatusMessage = "";
    notify();
    try {
      await homeClipboard.writeText(markdown, {
        purpose: ASSISTANT_CLIPBOARD_PURPOSE,
      });
      state.copyingTranscript = false;
      state.clipboardStatusMessage = "Transcript copied.";
      notify();
      return true;
    } catch (error) {
      state.copyingTranscript = false;
      state.clipboardStatusMessage = readClipboardStatus(error);
      notify();
      return false;
    }
  }

  return {
    initialize,
    snapshot,
    createSession,
    selectSession,
    startRenameSession,
    cancelRenameSession,
    commitRenameSession,
    togglePinSession,
    requestDeleteSession,
    confirmDeleteSession,
    setSearchQuery,
    setDraft,
    setSelectedOfferId,
    setSessionMode,
    copyTranscript,
    sendDraft,
    stopRun,
    updateRenameValue(value) {
      state.renameValue = boundedText(value, MAX_SESSION_TITLE_BYTES);
      notify();
    },
  };
}

function escapeHtml(text) {
  return String(text ?? "")
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;");
}

function renderSessionList(view) {
  if (!view.filteredSessions.length) {
    return `<p class="assistant-session-count">No sessions match</p>`;
  }
  return view.filteredSessions
    .map((session) => {
      const active = session.id === view.activeSessionId;
      const renaming = session.id === view.renamingSessionId;
      const deleting = session.id === view.deletingSessionId;
      return `
        <article class="assistant-session-card${active ? " is-active" : ""}" data-session-id="${escapeHtml(session.id)}">
          <div class="assistant-session-row">
            <button type="button" class="assistant-session-main" data-action="select-session" data-session-id="${escapeHtml(session.id)}">
              <span class="assistant-session-title">${escapeHtml(session.title)}</span>
              <span class="assistant-session-meta">
                <span class="assistant-session-mode">${escapeHtml(session.mode)}</span>
                ${session.pinned ? '<span class="assistant-session-pin">Pinned</span>' : ""}
              </span>
            </button>
          </div>
          ${
            renaming
              ? `
                <div class="assistant-session-actions">
                  <input class="assistant-rename-input" id="assistant-rename-input" value="${escapeHtml(view.renameValue)}" maxlength="${MAX_SESSION_TITLE_BYTES}" />
                  <button type="button" class="assistant-icon-button" data-action="commit-rename">Save</button>
                  <button type="button" class="assistant-icon-button" data-action="cancel-rename">Cancel</button>
                </div>
              `
              : `
                <p class="assistant-session-preview">${escapeHtml(sessionPreview(session))}</p>
                <div class="assistant-session-actions">
                  <button type="button" class="assistant-icon-button" data-action="toggle-pin" data-session-id="${escapeHtml(session.id)}">
                    ${session.pinned ? "Unpin" : "Pin"}
                  </button>
                  <button type="button" class="assistant-icon-button" data-action="start-rename" data-session-id="${escapeHtml(session.id)}">Rename</button>
                  ${
                    deleting
                      ? `
                        <button type="button" class="assistant-icon-button is-danger" data-action="confirm-delete" data-session-id="${escapeHtml(session.id)}">Confirm delete</button>
                        <button type="button" class="assistant-icon-button" data-action="cancel-delete" data-session-id="${escapeHtml(session.id)}">Keep</button>
                      `
                      : `
                        <button type="button" class="assistant-icon-button is-danger" data-action="request-delete" data-session-id="${escapeHtml(session.id)}">Delete</button>
                      `
                  }
                </div>
              `
          }
        </article>
      `;
    })
    .join("");
}

function renderMessages(view) {
  const session = view.currentSession;
  if (!session || !session.messages.length) {
    return "";
  }
  return session.messages
    .map((message) => {
      const roleLabel =
        message.role === "assistant"
          ? "Assistant"
          : message.role === "system"
            ? "System"
            : message.role === "tool"
              ? "Tool"
              : "You";
      const streaming =
        view.activeRun &&
        !view.activeRun.terminal &&
        message.run_id === view.activeRun.runId;
      return `
        <article class="assistant-message assistant-message-${escapeHtml(message.role)}${streaming ? " is-streaming" : ""}">
          <div class="assistant-message-role">${roleLabel}</div>
          <p class="assistant-message-body">${escapeHtml(message.content)}</p>
        </article>
      `;
    })
    .join("");
}

function renderStarters(view) {
  return view.starters
    .map(
      (starter) => `
        <button type="button" class="assistant-starter" data-action="use-starter" data-starter="${escapeHtml(starter)}">
          ${escapeHtml(starter)}
        </button>
      `,
    )
    .join("");
}

export function mountAssistantApp(root, app) {
  const sessionListNode = root.querySelector("#assistant-session-list");
  const messageListNode = root.querySelector("#assistant-message-list");
  const emptyStateNode = root.querySelector("#assistant-empty-state");
  const emptyTitleNode = root.querySelector("#assistant-empty-title");
  const emptyCopyNode = root.querySelector("#assistant-empty-copy");
  const startersNode = root.querySelector("#assistant-starters");
  const statusNode = root.querySelector("#assistant-status");
  const conflictNode = root.querySelector("#assistant-conflict");
  const searchNode = root.querySelector("#assistant-session-search");
  const draftNode = root.querySelector("#assistant-composer-input");
  const offerNode = root.querySelector("#assistant-offer-select");
  const copyNode = root.querySelector("#assistant-copy-transcript");
  const copyStatusNode = root.querySelector("#assistant-copy-status");
  const metaNode = root.querySelector("#assistant-composer-meta");
  const sendNode = root.querySelector("#assistant-send");
  const stopNode = root.querySelector("#assistant-stop");
  const newSessionNode = root.querySelector("#assistant-new-session");
  const chatNode = root.querySelector("#assistant-mode-chat");
  const buildNode = root.querySelector("#assistant-mode-build");

  function render(view) {
    statusNode.hidden = !view.statusMessage;
    statusNode.textContent = view.statusMessage || "";
    conflictNode.hidden = !view.conflictMessage;
    conflictNode.textContent = view.conflictMessage || "";
    sessionListNode.innerHTML = renderSessionList(view);
    messageListNode.innerHTML = renderMessages(view);
    emptyStateNode.hidden = Boolean(view.currentSession?.messages?.length);
    emptyTitleNode.textContent =
      view.currentSession?.mode === MODE_BUILD ? "Start a build session" : "Start a chat session";
    emptyCopyNode.textContent =
      view.currentSession?.mode === MODE_BUILD
        ? "Choose a starter prompt or write the next step."
        : "Choose a starter prompt or write your own.";
    startersNode.innerHTML = renderStarters(view);
    searchNode.value = view.searchQuery;
    draftNode.value = view.draft;
    metaNode.textContent = view.saving
      ? "Saving..."
      : view.activeRun && !view.activeRun.terminal
        ? "Running..."
        : "Ready";
    sendNode.disabled = view.sendDisabled;
    stopNode.hidden = !(view.activeRun && !view.activeRun.terminal);
    chatNode.setAttribute(
      "aria-selected",
      String((view.currentSession?.mode || MODE_CHAT) === MODE_CHAT),
    );
    buildNode.setAttribute(
      "aria-selected",
      String((view.currentSession?.mode || MODE_CHAT) === MODE_BUILD),
    );
    offerNode.innerHTML = "";
    copyNode.disabled = view.copyDisabled;
    copyNode.title = view.copyTitle;
    copyStatusNode.textContent = view.copyStatusMessage || "";
    if (!view.offersReady.length) {
      const option = document.createElement("option");
      option.value = "";
      option.textContent = "No text offers";
      offerNode.append(option);
      offerNode.disabled = true;
    } else {
      for (const offer of view.offersReady) {
        const option = document.createElement("option");
        option.value = offer.id;
        option.textContent = offer.title;
        option.selected = offer.id === view.selectedOfferId;
        offerNode.append(option);
      }
      offerNode.disabled = false;
    }
  }

  const rerender = () => render(app.snapshot());
  rerender();

  root.addEventListener("click", (event) => {
    const button = event.target.closest("[data-action]");
    if (!button) {
      return;
    }
    const action = button.dataset.action;
    const sessionId = button.dataset.sessionId || "";
    if (action === "select-session") {
      app.selectSession(sessionId);
    } else if (action === "toggle-pin") {
      app.togglePinSession(sessionId);
    } else if (action === "start-rename") {
      app.startRenameSession(sessionId);
    } else if (action === "commit-rename") {
      app.commitRenameSession();
    } else if (action === "cancel-rename") {
      app.cancelRenameSession();
    } else if (action === "request-delete") {
      app.requestDeleteSession(sessionId);
    } else if (action === "confirm-delete") {
      app.confirmDeleteSession(sessionId);
    } else if (action === "cancel-delete") {
      app.requestDeleteSession(sessionId);
    } else if (action === "use-starter") {
      app.setDraft(button.dataset.starter || "");
    }
    rerender();
  });

  newSessionNode.addEventListener("click", () => {
    app.createSession();
    rerender();
  });

  offerNode.addEventListener("change", () => {
    app.setSelectedOfferId(offerNode.value);
    rerender();
  });

  searchNode.addEventListener("input", () => {
    app.setSearchQuery(searchNode.value);
    rerender();
  });

  draftNode.addEventListener("input", () => {
    app.setDraft(draftNode.value);
    rerender();
  });

  root.querySelector("#assistant-composer-form").addEventListener("submit", async (event) => {
    event.preventDefault();
    await app.sendDraft();
    rerender();
  });

  stopNode.addEventListener("click", async () => {
    await app.stopRun();
    rerender();
  });

  copyNode.addEventListener("click", async () => {
    await app.copyTranscript();
    rerender();
  });

  chatNode.addEventListener("click", () => {
    app.setSessionMode(MODE_CHAT);
    rerender();
  });

  buildNode.addEventListener("click", () => {
    app.setSessionMode(MODE_BUILD);
    rerender();
  });

  root.addEventListener("input", (event) => {
    if (event.target.id === "assistant-rename-input") {
      app.updateRenameValue(event.target.value);
      rerender();
    }
  });

  return { render: rerender };
}

function readHashParam(name) {
  const hashParams = new URLSearchParams(window.location.hash.replace(/^#/, ""));
  return hashParams.get(name) || "";
}

async function bootAssistantBrowser() {
  const root = document.getElementById("assistant-app");
  if (!root) {
    return;
  }
  let mounted = null;
  const app = createAssistantApp({
    homeToken: readHashParam("home_token"),
    homeOrigin: readHashParam("home_origin") || "null",
    onStateChange() {
      mounted?.render();
    },
  });
  mounted = mountAssistantApp(root, app);
  await app.initialize();
  mounted.render();
}

if (typeof window !== "undefined" && typeof document !== "undefined") {
  void bootAssistantBrowser();
}
