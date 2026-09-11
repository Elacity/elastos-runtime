import { createAssistantApp } from "./assistant.js";
import { getAgentWorkspaceSnapshot, setAssistantMode } from "./agent-harness.js";
import { setAssistantWorkspaceField, getAssistantSession, ensureAssistantStudioSession, setAssistantSessionStudio, captureActiveSessionState, applyAssistantModeDraft } from "./agent-workspace.js";
import { getHomeGuiLaunchToken, persistAgentWorkspaceNow, flushAgentWorkspace, bindWorkspaceMergeGuard } from "./harness-host.js";
import { getLiveTurnCanonical } from "./agent-stream.js";
import { createHomeClipboardClient } from "/apps/home/home-clipboard-client.js?v=home-20260726a";

export async function bindAssistantModes(saved = {}) {
  const panel = document.querySelector("#assistant-studio");
  const notice = document.querySelector("#assistant-workspace-notice");
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
  const clipboard = createHomeClipboardClient({
    targetId: "assistant", homeToken: getHomeGuiLaunchToken(),
    homeOrigin: new URL(location.href).searchParams.get("home_origin") || "null",
    targetWindow: window.top, sourceWindow: window,
  });
  clipboard.start();

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
  bindWorkspaceMergeGuard(() => {
    const turn = getLiveTurnCanonical();
    return (!turn?.turnId || ["completed", "failed", "stopped", "settlement_unknown", "interrupted"].includes(turn.state)) &&
      [...controllers.values()].every(({ app }) => !app.snapshot().modeSwitchDisabled || app.snapshot().activeRun?.status === "settlement_unknown");
  });
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
    for (const item of document.querySelectorAll("button[data-assistant-mode]")) item.setAttribute("aria-pressed", String(item.dataset.assistantMode === mode));
    setAssistantMode(mode);
    setAssistantWorkspaceField("activeMode", mode);
  }
  window.addEventListener("assistant:session-selected", event => {
    visibleSessionId = event.detail?.sessionId ?? getAssistantSession()?.id ?? null;
    showMode(event.detail?.mode);
  });
  showMode(saved.activeMode || saved.sessionMode || "chat");
  for (const button of document.querySelectorAll("button[data-assistant-mode]")) {
    button.addEventListener("click", () => {
      const turn = getLiveTurnCanonical();
      if ((turn?.turnId && !["completed", "failed", "stopped", "settlement_unknown", "interrupted"].includes(turn.state)) || visibleController()?.snapshot().modeSwitchDisabled) {
        notice.hidden = false; notice.textContent = "Finish or stop the current run before changing mode."; return;
      }
      captureActiveSessionState();
      showMode(button.dataset.assistantMode);
    });
  }
  window.addEventListener("assistant:workspace-save", event => {
    notice.hidden = !event.detail.error; notice.textContent = event.detail.error || "";
  });
  notice.addEventListener("click", () => persistAgentWorkspaceNow());
  document.querySelector("#assistant-copy-conversation").addEventListener("click", async () => {
    const session = getAssistantSession();
    const conversation = (session?.messages || []).map(message => `## ${message.role}\n\n${message.text ?? message.content ?? ""}`);
    for (const run of session?.studio?.studioHistory || []) conversation.push(`## Studio\n\n${run.prompt || ""}\n\n${run.output?.resource_id || run.status || "Outcome unknown"}`);
    try {
      if (!conversation.length) return;
      await clipboard.writeText(conversation.join("\n\n"), {purpose: "transcript.markdown"});
      notice.hidden = false; notice.textContent = "Conversation copied.";
    } catch { notice.hidden = false; notice.textContent = "Could not copy the conversation."; }
  });
}
