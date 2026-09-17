import { createAssistantApp, eligibleStudioOffers } from "./assistant.js";
import { getAgentWorkspaceSnapshot, setAssistantMode } from "./agent-harness.js";
import { setAssistantWorkspaceField, getAssistantSession, ensureAssistantStudioSession, setAssistantSessionStudio, captureActiveSessionState, applyAssistantModeDraft } from "./agent-workspace.js";
import { getHomeGuiLaunchToken, persistAgentWorkspaceNow, flushAgentWorkspace, bindWorkspaceMergeGuard } from "./harness-host.js";
import { fetchModelOffers, unresolvedModelTurn } from "./agent-live.js";

/* UI ≠ authority: a mode control is shown only when the Runtime backs it.
   Build is a saved label today (no run reads it), so its segment stays hidden;
   Studio appears once an image or video offer is advertised. */
const MODE_CAPABILITY = { chat: true, build: false };
const SETTLED_TURN_STATES = ["completed", "failed", "stopped", "settlement_unknown", "interrupted"];

export async function bindAssistantModes(saved = {}) {
  const panel = document.querySelector("#assistant-studio");
  const notice = document.querySelector("#assistant-workspace-notice");
  const segment = document.querySelector("[data-assistant-mode-segment]");
  const studioRow = document.querySelector('button[data-assistant-mode="studio"]');
  const offer = document.querySelector("#studio-offer");
  const draft = document.querySelector("#studio-draft");
  const status = document.querySelector("#studio-status");
  const result = document.querySelector("#studio-result");
  const send = document.querySelector("#studio-send");
  const stop = document.querySelector("#studio-stop");
  const history = document.createElement("ol");
  history.id = "studio-history";
  history.setAttribute("aria-label", "Studio history");
  panel.append(history);
  const check = document.createElement("button");
  check.type = "button"; check.textContent = "Check run status"; check.hidden = true;
  status.after(check);
  const controllers = new Map();
  let visibleSessionId = saved.activeSessionId || null;
  let studioAvailable = false;

  // The active session's saved turn record is the same authority the harness consults before
  // accepting a new turn; the streamed canonical text carries no turn identity or state.
  function turnInProgress() {
    const turn = getAssistantSession()?.lastTurn;
    return unresolvedModelTurn(turn) && !SETTLED_TURN_STATES.includes(turn.state);
  }
  // A session whose Studio run is unsettled keeps Studio reachable so the run can be stopped or checked.
  function modeAvailable(mode) {
    if (mode !== "studio") return Boolean(MODE_CAPABILITY[mode]);
    const run = getAssistantSession()?.studio?.activeRun;
    return studioAvailable || Boolean(run && !run.terminal);
  }
  function syncModeControls() {
    segment.hidden = !MODE_CAPABILITY.build;
    // While Studio is open its row stays: with the Chat|Build segment hidden it is the only way back to Chat.
    studioRow.hidden = !(modeAvailable("studio") || document.body.dataset.assistantMode === "studio");
  }
  function applyOffers(payload) {
    studioAvailable = eligibleStudioOffers(payload).length > 0;
    syncModeControls();
  }
  // Every offers read (boot, the chat menu's "Refresh models", liveness probes) reports here,
  // so a failed or empty first read stops hiding Studio as soon as a later read advertises an offer.
  window.addEventListener("assistant:model-offers", event => applyOffers(event.detail));

  function render(view) {
    if (draft.value !== view.studioDraft) draft.value = view.studioDraft;
    status.textContent = view.statusMessage || (view.studioUnavailable ? "No image or video model is available on this Home." : "Choose a model and describe what to create.");
    offer.replaceChildren(...view.offersReady.map(item => {
      const option = document.createElement("option");
      option.value = item.id; option.textContent = item.title;
      option.selected = item.id === view.selectedOfferId;
      return option;
    }));
    offer.disabled = view.offersLoading || !view.offersReady.length || view.modeSwitchDisabled;
    send.disabled = view.sendDisabled;
    const run = view.activeRun;
    const owned = run && (!run.actorCapsule || run.actorCapsule === "assistant") && run.attachmentAllowed !== false;
    stop.hidden = !owned || !run.runId || run.terminal;
    stop.disabled = Boolean(run?.cancelRequested);
    check.hidden = !owned || !run.runId || run.terminal;
    result.hidden = !view.studioResult;
    result.textContent = view.studioResult?.resourceId || "";
    history.replaceChildren(...(view.studioHistory || []).map(item => {
      const row = document.createElement("li");
      const prompt = document.createElement("p"); prompt.textContent = item.prompt || "Earlier Studio run";
      const outcome = document.createElement("p"); outcome.textContent = item.status || "Outcome unknown";
      row.append(prompt, outcome);
      if (item.output) {
        const output = document.createElement("pre");
        output.textContent = item.output.resource_id || JSON.stringify(item.output, null, 2);
        row.append(output);
      }
      return row;
    }));
  }

  function controllerFor(session) {
    if (!session) return null;
    const source = session.studio || {};
    let entry = controllers.get(session.id);
    if (entry && JSON.stringify(source) !== entry.persisted && !entry.app.snapshot().modeSwitchDisabled) {
      entry.app.dispose(); controllers.delete(session.id); entry = null;
    }
    if (entry) return entry.app;
    const studioState = structuredClone(source);
    studioState.studioDraft ??= session.mode === "studio" ? session.composerDraft?.text || "" : "";
    if (studioState.activeRun && (session.recoveryIdentity || session.forkedFrom) && studioState.activeRun.sessionId !== session.id) {
      studioState.activeRun.attachmentAllowed = false;
    }
    entry = { app: null, persisted: JSON.stringify(source) };
    controllers.set(session.id, entry);
    entry.app = createAssistantApp({
      homeToken: getHomeGuiLaunchToken(), homeOrigin: new URL(location.href).searchParams.get("home_origin") || "null",
      studioOnly: true, studioState, studioSessionId: session.id,
      async beforeRunCreate(pending) {
        if (!(await flushAgentWorkspace())) return false;
        const savedRun = getAssistantSession(session.id)?.studio?.activeRun;
        return savedRun?.createRequestId === pending.createRequestId && !savedRun.terminal;
      },
      onStateChange(view) {
        const next = {
          ...getAssistantSession(session.id)?.studio,
          studioDraft: view.studioDraft, selectedStudioOfferId: view.selectedStudioOfferId,
          studioResult: view.studioResult, studioProgress: view.studioProgress,
          studioHistory: view.studioHistory, activeRun: view.activeRun,
        };
        entry.persisted = JSON.stringify(next);
        setAssistantSessionStudio(session.id, next, session);
        if (visibleSessionId === session.id) render(view);
      },
    });
    void entry.app.initialize();
    return entry.app;
  }

  function visibleController() { return controllers.get(visibleSessionId)?.app || null; }
  function activateStudio(session = getAssistantSession()) {
    session ||= ensureAssistantStudioSession();
    if (!session) return;
    visibleSessionId = session.id;
    const app = controllerFor(session);
    render(app.snapshot());
  }
  // Adopt the interim global Studio record once into a conversation.
  if (saved.studio && !saved.sessions?.some(session => session.studio)) {
    const session = ensureAssistantStudioSession();
    if (session) setAssistantSessionStudio(session.id, saved.studio, session);
  }
  for (const session of getAgentWorkspaceSnapshot()?.sessions || []) {
    if (session.studio?.activeRun && !session.studio.activeRun.terminal) controllerFor(session);
  }
  bindWorkspaceMergeGuard(() => !turnInProgress() &&
    [...controllers.values()].every(({ app }) => !app.snapshot().modeSwitchDisabled || app.snapshot().activeRun?.status === "settlement_unknown"));
  draft.addEventListener("input", () => visibleController()?.setDraft(draft.value));
  offer.addEventListener("change", () => visibleController()?.setSelectedOfferId(offer.value));
  document.querySelector("#studio-refresh").addEventListener("click", () => visibleController()?.refreshModels());
  document.querySelector("#studio-form").addEventListener("submit", event => { event.preventDefault(); void visibleController()?.sendDraft(); });
  stop.addEventListener("click", () => visibleController()?.stopRun());
  check.addEventListener("click", () => visibleController()?.resumeRun());
  function showMode(mode) {
    mode = ["chat", "build", "studio"].includes(mode) ? mode : "chat";
    if (mode === "studio") activateStudio();
    applyAssistantModeDraft(mode);
    document.body.dataset.assistantMode = mode;
    panel.hidden = mode !== "studio";
    for (const item of document.querySelectorAll("button[data-assistant-mode]")) {
      const pressed = item.dataset.assistantMode === mode;
      item.setAttribute("aria-pressed", String(pressed));
      item.classList.toggle("is-active", pressed);
    }
    syncModeControls();
    setAssistantMode(mode);
    setAssistantWorkspaceField("activeMode", mode);
  }
  // A session saved in a mode this Home no longer backs opens as chat, never as an unreachable room.
  function showAvailableMode(mode) {
    const wanted = ["chat", "build", "studio"].includes(mode) ? mode : "chat";
    showMode(modeAvailable(wanted) ? wanted : "chat");
  }
  // Offers arrive after boot, so a saved Studio session reopens once its offer is confirmed. The
  // reopen belongs to the session it was saved in and lasts only while the person has chosen
  // nothing else: selecting a chat, typing in the composer or a turn still running keeps the view.
  const savedMode = saved.activeMode || saved.sessionMode || "chat";
  const restoreSessionId = getAssistantSession()?.id ?? null;
  let restorePending = savedMode === "studio";
  const keepCurrentView = () => { restorePending = false; };
  window.addEventListener("assistant:session-selected", event => {
    keepCurrentView();
    visibleSessionId = event.detail?.sessionId ?? getAssistantSession()?.id ?? null;
    showAvailableMode(event.detail?.mode);
  });
  document.addEventListener("input", event => {
    if (event.target?.id === "agent-composer-input") keepCurrentView();
  });
  showAvailableMode(savedMode);
  void fetchModelOffers().then(applyOffers, () => {}).then(() => {
    if (!restorePending) return;
    restorePending = false;
    if (studioAvailable && document.body.dataset.assistantMode === "chat" && !turnInProgress() &&
      getAssistantSession()?.id === restoreSessionId) showMode("studio");
  });
  for (const button of document.querySelectorAll("button[data-assistant-mode]")) {
    button.addEventListener("click", () => {
      keepCurrentView();
      if (turnInProgress() || visibleController()?.snapshot().modeSwitchDisabled) {
        notice.hidden = false; notice.textContent = "Finish or stop the current run before changing mode."; return;
      }
      captureActiveSessionState();
      // The Studio row is the only visible mode control while the Chat|Build segment is hidden, so it toggles.
      const leavingStudio = button.dataset.assistantMode === "studio" && document.body.dataset.assistantMode === "studio";
      showMode(leavingStudio ? "chat" : button.dataset.assistantMode);
    });
  }
  window.addEventListener("assistant:workspace-save", event => {
    notice.hidden = !event.detail.error; notice.textContent = event.detail.error || "";
  });
  notice.addEventListener("click", () => persistAgentWorkspaceNow());
}
