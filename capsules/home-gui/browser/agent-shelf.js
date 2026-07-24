/* Agent Shelf morph (preview) — presentation only.
   UI ≠ authority: morphing never mints grants.

   Geometry is FLIP’d in pixels. CSS cannot interpolate width:max-content →
   width:720px or height:auto, which looked like an instant jump + empty wait. */

import { registerEscapeHandler } from "./shell-popovers.js?v=home-20260724m";
import { hideLauncher } from "./shell-surface.js?v=home-20260724m";

let bound = false;
let morphGeneration = 0;
let morphTimer = 0;
let streamTimer = 0;
let streamGeneration = 0;

/** Real dock width/height stretch — both directions, even ease-in-out. */
const MORPH_STRETCH_MS = 950;
const MORPH_EASE = "cubic-bezier(0.42, 0, 0.58, 1)";
/** Icons begin clearing just before stretch — same cadence as restore, reversed. */
const MORPH_EXIT_MS = 90;
/** Chat chrome fills near end of the stretch (quick — not a placeholder wait). */
const MORPH_ENTER_AT_MS = Math.round(MORPH_STRETCH_MS * 0.82);
/** Chrome out before reverse stretch (mirrors enter fill). */
const MORPH_LEAVE_MS = 140;
/** Composer → taller workspace panel. */
const WORKSPACE_EXPAND_MS = 520;

const MOCK_REPLY =
  "I’m a local preview on this machine — not live inference yet.\n\n" +
  "I start with no tools. If you need Downloads or other capsule access, " +
  "you’ll grant it explicitly (Inbox-style). Nothing ambient.";

function clearMorphTimer() {
  if (morphTimer) {
    window.clearTimeout(morphTimer);
    morphTimer = 0;
  }
}

function clearStreamTimer() {
  if (streamTimer) {
    window.clearInterval(streamTimer);
    streamTimer = 0;
  }
}

function scheduleMorph(generation, delayMs, work) {
  clearMorphTimer();
  morphTimer = window.setTimeout(() => {
    morphTimer = 0;
    if (generation !== morphGeneration) {
      return;
    }
    work();
  }, delayMs);
}

function composerMaxHeightPx() {
  const taskbar = taskbarEl();
  if (taskbar?.classList.contains("is-agent-workspace")) {
    return Math.min(140, Math.round(window.innerHeight * 0.22));
  }
  return Math.min(320, Math.round(window.innerHeight * 0.42));
}

function taskbarEl() {
  return document.querySelector(".taskbar");
}

function workspaceScrim() {
  return document.querySelector("#agent-workspace-scrim");
}

function streamEl() {
  return document.querySelector("#agent-workspace-stream");
}

function sessionTitleEl() {
  return document.querySelector("#agent-session-title");
}

export function agentShelfFaceActive() {
  const taskbar = taskbarEl();
  if (!taskbar || taskbar.dataset.agentPreview !== "1") {
    return false;
  }
  if (taskbar.classList.contains("is-agent-face")) {
    return true;
  }
  const phase = taskbar.dataset.agentMorph || "";
  return phase === "exit" || phase === "grow" || phase === "enter" || phase === "leave" || phase === "shrink";
}

function sendButton() {
  return document.querySelector("#agent-composer-send");
}

function composerInput() {
  return document.querySelector("#agent-composer-input");
}

export function syncAgentSendButton(input = composerInput()) {
  const btn = sendButton();
  if (!btn) {
    return;
  }
  if (btn.dataset.mode === "stop") {
    btn.disabled = false;
    btn.setAttribute("aria-label", "Stop");
    btn.title = "Stop generating";
    return;
  }
  const hasText = Boolean(input?.value?.trim());
  btn.dataset.mode = "send";
  btn.disabled = !hasText;
  btn.setAttribute("aria-label", "Send");
  btn.title = hasText ? "Send" : "Enter a message to send";
}

export function setAgentComposerProcessing(isProcessing) {
  const btn = sendButton();
  if (!btn) {
    return;
  }
  if (isProcessing) {
    btn.dataset.mode = "stop";
    btn.disabled = false;
    btn.setAttribute("aria-label", "Stop");
    btn.title = "Stop generating";
    return;
  }
  btn.dataset.mode = "send";
  syncAgentSendButton();
}

function autosizeComposer(input) {
  if (!input) {
    return;
  }
  const max = composerMaxHeightPx();
  input.style.height = "auto";
  input.style.height = `${Math.min(max, Math.max(28, input.scrollHeight))}px`;
  input.style.maxHeight = `${max}px`;
  syncAgentSendButton(input);
}

function setMorphPhase(taskbar, phase) {
  if (!phase) {
    delete taskbar.dataset.agentMorph;
    return;
  }
  taskbar.dataset.agentMorph = phase;
}

function syncFaceAria(agentVisible) {
  const agentFace = document.querySelector(".shelf-face-agent");
  const appsFace = document.querySelector(".shelf-face-apps");
  if (agentFace) {
    agentFace.setAttribute("aria-hidden", agentVisible ? "false" : "true");
  }
  if (appsFace) {
    appsFace.setAttribute("aria-hidden", agentVisible ? "true" : "false");
  }
}

function setWorkspaceScrim(visible) {
  const scrim = workspaceScrim();
  if (!scrim) {
    return;
  }
  if (visible) {
    scrim.hidden = false;
    scrim.setAttribute("aria-hidden", "false");
    requestAnimationFrame(() => {
      scrim.classList.add("is-visible");
    });
    return;
  }
  scrim.classList.remove("is-visible");
  scrim.setAttribute("aria-hidden", "true");
  window.setTimeout(() => {
    if (!scrim.classList.contains("is-visible")) {
      scrim.hidden = true;
    }
  }, 300);
}

function readBox(taskbar) {
  const rect = taskbar.getBoundingClientRect();
  const cs = window.getComputedStyle(taskbar);
  return {
    width: rect.width,
    height: rect.height,
    padding: `${cs.paddingTop} ${cs.paddingRight} ${cs.paddingBottom} ${cs.paddingLeft}`,
    borderRadius: cs.borderRadius,
    boxShadow: cs.boxShadow,
  };
}

function lockBox(taskbar, box) {
  taskbar.style.boxSizing = "border-box";
  taskbar.style.width = `${box.width}px`;
  taskbar.style.height = `${box.height}px`;
  taskbar.style.minHeight = `${box.height}px`;
  taskbar.style.maxWidth = "none";
  taskbar.style.padding = box.padding;
  taskbar.style.borderRadius = box.borderRadius;
  taskbar.style.boxShadow = box.boxShadow;
}

function clearBoxLock(taskbar) {
  taskbar.style.transition = "";
  taskbar.style.boxSizing = "";
  taskbar.style.width = "";
  taskbar.style.height = "";
  taskbar.style.minHeight = "";
  taskbar.style.maxWidth = "";
  taskbar.style.padding = "";
  taskbar.style.borderRadius = "";
  taskbar.style.boxShadow = "";
}

function geometryTransition(durationMs = MORPH_STRETCH_MS) {
  const t = `${durationMs}ms ${MORPH_EASE}`;
  return [
    `width ${t}`,
    `height ${t}`,
    `min-height ${t}`,
    `padding ${t}`,
    `border-radius ${t}`,
    `box-shadow ${t}`,
  ].join(", ");
}

/**
 * FLIP the Shelf glass between apps size and Agent size.
 * Returns the measured end box (for debugging / future use).
 */
function flipTaskbarGeometry(taskbar, applyEndState, durationMs = MORPH_STRETCH_MS) {
  const from = readBox(taskbar);
  applyEndState();
  const to = readBox(taskbar);

  /* Invert — paint still at apps/agent start size this frame. */
  taskbar.style.transition = "none";
  lockBox(taskbar, from);
  void taskbar.offsetWidth;

  /* Play — real interpolated width/height (the motion we want). */
  taskbar.style.transition = geometryTransition(durationMs);
  lockBox(taskbar, to);
  return to;
}

function appendMessage(role, text, { streaming = false } = {}) {
  const stream = streamEl();
  if (!stream) {
    return null;
  }
  const row = document.createElement("div");
  row.className = `agent-msg agent-msg-${role}${streaming ? " is-streaming" : ""}`;
  row.dataset.role = role;

  const meta = document.createElement("div");
  meta.className = "agent-msg-meta";
  meta.textContent = role === "user" ? "You" : "Agent";

  const body = document.createElement("div");
  body.className = "agent-msg-body";
  body.textContent = text;

  row.append(meta, body);
  stream.append(row);
  stream.scrollTop = stream.scrollHeight;
  return row;
}

function titleFromPrompt(prompt) {
  const cleaned = prompt.replace(/\s+/g, " ").trim();
  if (!cleaned) {
    return "New chat";
  }
  return cleaned.length > 42 ? `${cleaned.slice(0, 41)}…` : cleaned;
}

function setSessionTitle(title) {
  const el = sessionTitleEl();
  if (el) {
    el.textContent = title;
  }
}

function stopMockStream({ keepPartial = true } = {}) {
  clearStreamTimer();
  streamGeneration += 1;
  setAgentComposerProcessing(false);
  const streaming = streamEl()?.querySelector(".agent-msg-agent.is-streaming");
  if (streaming) {
    streaming.classList.remove("is-streaming");
    if (!keepPartial) {
      streaming.remove();
    }
  }
}

function startMockStream(replyText) {
  stopMockStream({ keepPartial: true });
  const generation = (streamGeneration += 1);
  const row = appendMessage("agent", "", { streaming: true });
  const body = row?.querySelector(".agent-msg-body");
  if (!body) {
    return;
  }

  setAgentComposerProcessing(true);
  let index = 0;
  streamTimer = window.setInterval(() => {
    if (generation !== streamGeneration) {
      clearStreamTimer();
      return;
    }
    index = Math.min(replyText.length, index + 2 + (index % 3));
    body.textContent = replyText.slice(0, index);
    const stream = streamEl();
    if (stream) {
      stream.scrollTop = stream.scrollHeight;
    }
    if (index >= replyText.length) {
      clearStreamTimer();
      row.classList.remove("is-streaming");
      setAgentComposerProcessing(false);
    }
  }, 18);
}

export function expandAgentWorkspace() {
  const taskbar = taskbarEl();
  if (!taskbar || !taskbar.classList.contains("is-agent-face")) {
    return;
  }
  if (taskbar.classList.contains("is-agent-workspace")) {
    setWorkspaceScrim(true);
    return;
  }

  const generation = (morphGeneration += 1);
  clearMorphTimer();
  setWorkspaceScrim(true);

  flipTaskbarGeometry(
    taskbar,
    () => {
      taskbar.classList.add("is-agent-workspace");
    },
    WORKSPACE_EXPAND_MS
  );

  window.setTimeout(() => {
    if (generation !== morphGeneration) {
      return;
    }
    clearBoxLock(taskbar);
    autosizeComposer(composerInput());
  }, WORKSPACE_EXPAND_MS + 32);
}

function resetWorkspaceTranscript() {
  stopMockStream({ keepPartial: false });
  const stream = streamEl();
  if (stream) {
    stream.replaceChildren();
  }
  setSessionTitle("New chat");
  document.querySelectorAll(".agent-session-item.is-active").forEach((item) => {
    item.classList.remove("is-active");
  });
}

function runAfterAgentFaceReady(work) {
  const taskbar = taskbarEl();
  if (!taskbar) {
    return;
  }
  if (taskbar.classList.contains("is-agent-face") && !taskbar.dataset.agentMorph) {
    work();
    return;
  }
  if (taskbar.classList.contains("is-agent-face") && taskbar.dataset.agentMorph === "enter") {
    work();
    return;
  }
  let tries = 0;
  const tick = window.setInterval(() => {
    tries += 1;
    const ready =
      taskbar.classList.contains("is-agent-face") &&
      (taskbar.dataset.agentMorph === "enter" || !taskbar.dataset.agentMorph);
    if (ready || tries > 40) {
      window.clearInterval(tick);
      if (taskbar.classList.contains("is-agent-face")) {
        work();
      }
    }
  }, 40);
}

export function sendAgentComposerMessage() {
  const input = composerInput();
  const taskbar = taskbarEl();
  const btn = sendButton();
  if (!input || !taskbar) {
    return;
  }

  if (btn?.dataset.mode === "stop") {
    stopMockStream({ keepPartial: true });
    return;
  }

  const prompt = input.value.trim();
  if (!prompt) {
    return;
  }

  if (!taskbar.classList.contains("is-agent-face")) {
    showAgentShelfFace();
  }

  input.value = "";
  autosizeComposer(input);

  runAfterAgentFaceReady(() => {
    expandAgentWorkspace();

    const titleEl = sessionTitleEl();
    const stream = streamEl();
    if (titleEl && (titleEl.textContent === "New chat" || !stream?.children.length)) {
      setSessionTitle(titleFromPrompt(prompt));
    }

    appendMessage("user", prompt);
    startMockStream(MOCK_REPLY);
  });
}

export function showAgentShelfFace() {
  const taskbar = taskbarEl();
  const toggle = document.querySelector("#agent-shelf-toggle");
  const input = composerInput();
  if (!taskbar || taskbar.dataset.agentPreview !== "1") {
    return;
  }
  if (taskbar.classList.contains("is-agent-face") && taskbar.dataset.agentMorph === "enter") {
    return;
  }

  hideLauncher();
  const generation = (morphGeneration += 1);
  clearMorphTimer();
  taskbar.querySelectorAll(".taskbar-icon").forEach((icon) => {
    icon.style.transform = "";
  });
  taskbar.querySelectorAll(".taskbar-item").forEach((item) => {
    item.style.removeProperty("--dock-shift");
  });
  toggle?.setAttribute("aria-pressed", "true");
  setAgentComposerProcessing(false);

  /* Icons start dissolving while we still hold apps geometry. */
  setMorphPhase(taskbar, "exit");
  syncFaceAria(false);

  scheduleMorph(generation, MORPH_EXIT_MS, () => {
    setMorphPhase(taskbar, "grow");
    syncFaceAria(true);
    autosizeComposer(input);

    flipTaskbarGeometry(taskbar, () => {
      taskbar.classList.add("is-agent-face");
    });

    /* Chrome fills as the stretch finishes — not a long empty hold. */
    scheduleMorph(generation, MORPH_ENTER_AT_MS, () => {
      if (generation !== morphGeneration) {
        return;
      }
      setMorphPhase(taskbar, "enter");
      window.setTimeout(() => {
        if (generation === morphGeneration && agentShelfFaceActive()) {
          input?.focus({ preventScroll: true });
        }
      }, 40);
    });

    /* After stretch, drop inline locks so CSS/end-state + autosize own layout. */
    window.setTimeout(() => {
      if (generation !== morphGeneration) {
        return;
      }
      clearBoxLock(taskbar);
    }, MORPH_STRETCH_MS + 32);
  });
}

export function hideAgentShelfFace() {
  const taskbar = taskbarEl();
  const toggle = document.querySelector("#agent-shelf-toggle");
  if (!taskbar) {
    return;
  }

  const generation = (morphGeneration += 1);
  clearMorphTimer();
  stopMockStream({ keepPartial: true });
  setAgentComposerProcessing(false);
  toggle?.setAttribute("aria-pressed", "false");
  setWorkspaceScrim(false);

  const finishClosed = () => {
    clearBoxLock(taskbar);
    taskbar.classList.remove("is-agent-face", "is-agent-workspace");
    setMorphPhase(taskbar, "");
    syncFaceAria(false);
    toggle?.focus({ preventScroll: true });
  };

  if (!taskbar.classList.contains("is-agent-face")) {
    finishClosed();
    return;
  }

  /* Chrome out, then equal reverse stretch (pixel FLIP). */
  setMorphPhase(taskbar, "leave");
  syncFaceAria(true);

  scheduleMorph(generation, MORPH_LEAVE_MS, () => {
    setMorphPhase(taskbar, "shrink");
    syncFaceAria(false);

    flipTaskbarGeometry(taskbar, () => {
      taskbar.classList.remove("is-agent-face", "is-agent-workspace");
    });

    scheduleMorph(generation, MORPH_STRETCH_MS, () => {
      clearBoxLock(taskbar);
      setMorphPhase(taskbar, "");
      toggle?.focus({ preventScroll: true });
    });
  });
}

export function toggleAgentShelfFace() {
  if (agentShelfFaceActive()) {
    hideAgentShelfFace();
  } else {
    showAgentShelfFace();
  }
}

export function bindAgentShelf() {
  if (bound) {
    return;
  }
  bound = true;

  registerEscapeHandler("agent-shelf", {
    priority: 75,
    isActive: () => agentShelfFaceActive(),
    dismiss: () => hideAgentShelfFace(),
  });

  document.addEventListener("click", (event) => {
    const toggle = event.target.closest?.("#agent-shelf-toggle");
    if (toggle) {
      event.preventDefault();
      toggleAgentShelfFace();
      return;
    }
    if (event.target.closest?.("#agent-shelf-flip-back")) {
      event.preventDefault();
      hideAgentShelfFace();
      return;
    }
    if (event.target.closest?.("#agent-workspace-scrim")) {
      event.preventDefault();
      hideAgentShelfFace();
      return;
    }
    if (event.target.closest?.("#agent-new-chat")) {
      event.preventDefault();
      resetWorkspaceTranscript();
      composerInput()?.focus({ preventScroll: true });
      return;
    }
    const session = event.target.closest?.(".agent-session-item");
    if (session) {
      event.preventDefault();
      document.querySelectorAll(".agent-session-item.is-active").forEach((item) => {
        item.classList.remove("is-active");
      });
      session.classList.add("is-active");
      setSessionTitle(session.dataset.title || session.textContent.trim());
      const stream = streamEl();
      if (stream) {
        stopMockStream({ keepPartial: false });
        stream.replaceChildren();
        appendMessage("user", session.dataset.title || "Earlier chat");
        appendMessage(
          "agent",
          "Preview session — open a new chat or send from the composer to stream a mock reply."
        );
      }
      return;
    }
    const send = event.target.closest?.("#agent-composer-send");
    if (send) {
      event.preventDefault();
      sendAgentComposerMessage();
      return;
    }
    if (event.target.closest?.(".agent-approve-btn")) {
      event.preventDefault();
      return;
    }
    if (event.target.closest?.("#agent-model-picker")) {
      event.preventDefault();
    }
  });

  document.addEventListener("input", (event) => {
    if (event.target?.id === "agent-composer-input") {
      autosizeComposer(event.target);
    }
  });

  window.addEventListener("resize", () => {
    if (agentShelfFaceActive()) {
      autosizeComposer(composerInput());
    }
  });

  document.addEventListener("keydown", (event) => {
    if (event.target?.id !== "agent-composer-input") {
      return;
    }
    if (event.key === "Enter" && !event.shiftKey) {
      event.preventDefault();
      sendAgentComposerMessage();
    }
  });
}
