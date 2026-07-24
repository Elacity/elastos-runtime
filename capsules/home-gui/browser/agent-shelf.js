/* Agent Shelf morph (preview) — presentation only.
   UI ≠ authority: morphing never mints grants.

   Geometry is FLIP’d in pixels. CSS cannot interpolate width:max-content →
   width:720px or height:auto, which looked like an instant jump + empty wait.

   Send opens Agent Harness (Home drops, dock stays) — see agent-harness.js. */

import { registerEscapeHandler } from "./shell-popovers.js?v=home-20260724ai";
import { hideLauncher } from "./shell-surface.js?v=home-20260724ai";

let bound = false;
let morphGeneration = 0;
let morphTimer = 0;

/** Real dock width/height stretch — both directions, even ease-in-out. */
const MORPH_STRETCH_MS = 950;
const MORPH_EASE = "cubic-bezier(0.42, 0, 0.58, 1)";
/** Icons begin clearing just before stretch — same cadence as restore, reversed. */
const MORPH_EXIT_MS = 90;
/** Chat chrome fills near end of the stretch (quick — not a placeholder wait). */
const MORPH_ENTER_AT_MS = Math.round(MORPH_STRETCH_MS * 0.82);
/** Chrome out before reverse stretch (mirrors enter fill). */
const MORPH_LEAVE_MS = 140;

function clearMorphTimer() {
  if (morphTimer) {
    window.clearTimeout(morphTimer);
    morphTimer = 0;
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
  return Math.min(320, Math.round(window.innerHeight * 0.42));
}

function taskbarEl() {
  return document.querySelector(".taskbar");
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

export function composerInput() {
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

function flipTaskbarGeometry(taskbar, applyEndState, durationMs = MORPH_STRETCH_MS) {
  const from = readBox(taskbar);
  applyEndState();
  const to = readBox(taskbar);

  taskbar.style.transition = "none";
  lockBox(taskbar, from);
  void taskbar.offsetWidth;

  taskbar.style.transition = geometryTransition(durationMs);
  lockBox(taskbar, to);
  return to;
}

async function harnessApi() {
  return import("./agent-harness.js?v=home-20260724ai");
}

export async function sendAgentComposerMessage() {
  const input = composerInput();
  const taskbar = taskbarEl();
  const btn = sendButton();
  if (!input || !taskbar) {
    return;
  }

  const harness = await harnessApi();
  if (btn?.dataset.mode === "stop") {
    harness.stopAgentHarnessStream();
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

  const waitReady = () =>
    new Promise((resolve) => {
      if (taskbar.classList.contains("is-agent-face") && taskbar.dataset.agentMorph === "enter") {
        resolve();
        return;
      }
      if (taskbar.classList.contains("is-agent-face") && !taskbar.dataset.agentMorph) {
        resolve();
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
          resolve();
        }
      }, 40);
    });

  await waitReady();
  harness.sendToAgentHarness(prompt);
}

function openHarnessWithShelf() {
  void harnessApi().then((harness) => {
    if (!harness.agentHarnessActive()) {
      harness.showAgentHarness({ fromShelf: true });
    }
  });
}

function prefersReducedMotion() {
  return window.matchMedia?.("(prefers-reduced-motion: reduce)").matches === true;
}

export function showAgentShelfFace() {
  const taskbar = taskbarEl();
  const toggle = document.querySelector("#agent-shelf-toggle");
  const input = composerInput();
  if (!taskbar || taskbar.dataset.agentPreview !== "1") {
    return;
  }
  if (taskbar.classList.contains("is-agent-face") && taskbar.dataset.agentMorph === "enter") {
    openHarnessWithShelf();
    return;
  }
  if (taskbar.classList.contains("is-agent-face") && !taskbar.dataset.agentMorph) {
    openHarnessWithShelf();
    return;
  }

  if (prefersReducedMotion()) {
    snapAgentShelfFace();
    openHarnessWithShelf();
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

  setMorphPhase(taskbar, "exit");
  syncFaceAria(false);

  scheduleMorph(generation, MORPH_EXIT_MS, () => {
    setMorphPhase(taskbar, "grow");
    syncFaceAria(true);
    autosizeComposer(input);

    flipTaskbarGeometry(taskbar, () => {
      taskbar.classList.add("is-agent-face");
    });

    /* Separate timer — scheduleMorph is single-slot; don’t cancel enter phase. */
    window.setTimeout(() => {
      if (generation !== morphGeneration) {
        return;
      }
      openHarnessWithShelf();
    }, Math.round(MORPH_STRETCH_MS * 0.28));

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

    window.setTimeout(() => {
      if (generation !== morphGeneration) {
        return;
      }
      clearBoxLock(taskbar);
    }, MORPH_STRETCH_MS + 32);
  });
}

/**
 * Instant Agent composer face — used when Mission Control / Space switch
 * lands on Agent without the Dock morph dance (avoids empty dock).
 */
export function snapAgentShelfFace() {
  const taskbar = taskbarEl();
  const toggle = document.querySelector("#agent-shelf-toggle");
  const input = composerInput();
  if (!taskbar || taskbar.dataset.agentPreview !== "1") {
    return;
  }
  morphGeneration += 1;
  clearMorphTimer();
  clearBoxLock(taskbar);
  taskbar.querySelectorAll(".taskbar-icon").forEach((icon) => {
    icon.style.transform = "";
  });
  taskbar.querySelectorAll(".taskbar-item").forEach((item) => {
    item.style.removeProperty("--dock-shift");
  });
  taskbar.classList.add("is-agent-face");
  setMorphPhase(taskbar, "enter");
  syncFaceAria(true);
  toggle?.setAttribute("aria-pressed", "true");
  setAgentComposerProcessing(false);
  autosizeComposer(input);
}

/** Instant Apps face — Space leave / MC → Desktop without reverse morph glitch. */
export function snapAppsShelfFace() {
  const taskbar = taskbarEl();
  const toggle = document.querySelector("#agent-shelf-toggle");
  if (!taskbar) {
    return;
  }
  morphGeneration += 1;
  clearMorphTimer();
  clearBoxLock(taskbar);
  taskbar.classList.remove("is-agent-face");
  setMorphPhase(taskbar, "");
  syncFaceAria(false);
  toggle?.setAttribute("aria-pressed", "false");
  setAgentComposerProcessing(false);
  document.documentElement.style.removeProperty("--agent-column-width");
  document.documentElement.style.removeProperty("--agent-column-left");
  document.documentElement.style.removeProperty("--harness-composer-clearance");
}

export function hideAgentShelfFace() {
  const taskbar = taskbarEl();
  const toggle = document.querySelector("#agent-shelf-toggle");
  if (!taskbar) {
    return;
  }

  const generation = (morphGeneration += 1);
  clearMorphTimer();
  setAgentComposerProcessing(false);
  toggle?.setAttribute("aria-pressed", "false");

  const finishClosed = () => {
    clearBoxLock(taskbar);
    taskbar.classList.remove("is-agent-face");
    setMorphPhase(taskbar, "");
    syncFaceAria(false);
    document.documentElement.style.removeProperty("--agent-column-width");
    document.documentElement.style.removeProperty("--agent-column-left");
    document.documentElement.style.removeProperty("--harness-composer-clearance");
    toggle?.focus({ preventScroll: true });
  };

  if (!taskbar.classList.contains("is-agent-face")) {
    void harnessApi().then((harness) => {
      if (harness.agentHarnessActive()) {
        harness.hideAgentHarness({ restoreShelfApps: false, syncStage: false });
      }
    });
    finishClosed();
    return;
  }

  if (prefersReducedMotion()) {
    void harnessApi().then((harness) => {
      if (harness.agentHarnessActive()) {
        harness.hideAgentHarness({ restoreShelfApps: false, syncStage: false });
      }
    });
    snapAppsShelfFace();
    toggle?.focus({ preventScroll: true });
    return;
  }

  /* Chrome leaves first; harness exhales as dock begins shrink — not before. */
  setMorphPhase(taskbar, "leave");
  syncFaceAria(true);

  scheduleMorph(generation, MORPH_LEAVE_MS, () => {
    void harnessApi().then((harness) => {
      if (harness.agentHarnessActive()) {
        harness.hideAgentHarness({ restoreShelfApps: false, syncStage: false });
      }
    });

    setMorphPhase(taskbar, "shrink");
    syncFaceAria(false);

    flipTaskbarGeometry(taskbar, () => {
      taskbar.classList.remove("is-agent-face");
    });

    scheduleMorph(generation, MORPH_STRETCH_MS, () => {
      clearBoxLock(taskbar);
      setMorphPhase(taskbar, "");
      document.documentElement.style.removeProperty("--agent-column-width");
      document.documentElement.style.removeProperty("--agent-column-left");
      document.documentElement.style.removeProperty("--harness-composer-clearance");
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
    /* Esc always exits the full Agent dance (harness + Shelf) via hideAgentShelfFace. */
    isActive: () => agentShelfFaceActive(),
    dismiss: () => hideAgentShelfFace(),
  });

  document.addEventListener("click", (event) => {
    const toggle = event.target.closest?.("#agent-shelf-toggle");
    if (toggle) {
      event.preventDefault();
      /* Agent control toggles the full dance: Shelf morph ↔ harness room. */
      toggleAgentShelfFace();
      return;
    }
    if (event.target.closest?.("#agent-shelf-flip-back")) {
      event.preventDefault();
      hideAgentShelfFace();
      return;
    }
    const send = event.target.closest?.("#agent-composer-send");
    if (send) {
      event.preventDefault();
      void sendAgentComposerMessage();
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
      void sendAgentComposerMessage();
    }
  });
}
