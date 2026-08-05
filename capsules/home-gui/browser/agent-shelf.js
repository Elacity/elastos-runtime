/* Agent Shelf morph (preview) — presentation only.
   UI ≠ authority (Principle 16): morphing never mints grants or Carrier
   authority. Live tools/grants stay fail-closed until Inbox/agentic waves.

   Geometry is FLIP’d in pixels. CSS cannot interpolate width:max-content →
   width:720px or height:auto, which looked like an instant jump + empty wait.

   Send opens Agent Harness (Home drops, dock stays) — see agent-harness.js. */

import { registerEscapeHandler } from "./shell-popovers.js?v=home-20260804av";
import { TIP } from "./agent-tip.js?v=home-20260804av";
import {
  agentHarnessActive,
  hideAgentHarness,
  sendToAgentHarness,
  showAgentHarness,
  stopAgentHarnessStream,
} from "./agent-send.js?v=home-20260804av";
import { extractAgentLibraryRead } from "./agent-live.js?v=home-20260804av";
import {
  formatLibraryKbContext,
  getReadyLibraryReadGrant,
} from "./agent-grants.js?v=home-20260804av";

let bound = false;
let morphGeneration = 0;
let morphTimer = 0;
/** @type {{ id: string, name: string, size: number }[]} */
let composerAttachments = [];
let attachSeq = 0;
const MAX_ATTACH_TEXT_CHARS = 24_000;
const MAX_ATTACH_READ_BYTES = 200_000;
const TEXT_ATTACH_RE =
  /^(text\/|application\/(json|xml|javascript|x-yaml|yaml|toml|csv|sql))/i;

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
  const hardCap = Math.min(320, Math.round(window.innerHeight * 0.42));
  const taskbar = taskbarEl();
  if (!taskbar?.classList.contains("is-agent-face")) {
    return hardCap;
  }
  /* Keep text inside the Shelf pill — dock max minus padding + toolbar. */
  const narrow =
    typeof window.matchMedia === "function" &&
    window.matchMedia("(max-width: 900px)").matches;
  const dockCap = Math.min(
    Math.round(window.innerHeight * (narrow ? 0.42 : 0.72)),
    narrow ? 280 : 520,
  );
  const cs = window.getComputedStyle(taskbar);
  const padY =
    (parseFloat(cs.paddingTop) || 0) + (parseFloat(cs.paddingBottom) || 0);
  const toolbar = taskbar.querySelector(".agent-composer-toolbar");
  const toolH = toolbar?.getBoundingClientRect().height || (narrow ? 36 : 32);
  const gap = narrow ? 8 : 12;
  const available = Math.floor(dockCap - padY - toolH - gap);
  return Math.max(48, Math.min(hardCap, available));
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

function syncFaceAria({ agent = false, launcher = false } = {}) {
  const agentFace = document.querySelector(".shelf-face-agent");
  const appsFace = document.querySelector(".shelf-face-apps");
  const launcherFace = document.querySelector(".shelf-face-launcher");
  if (agentFace) {
    agentFace.setAttribute("aria-hidden", agent ? "false" : "true");
  }
  if (launcherFace) {
    launcherFace.setAttribute("aria-hidden", launcher ? "false" : "true");
  }
  if (appsFace) {
    /* Idle dock row stays visible under Apps face (pinned icons + drag targets). */
    appsFace.setAttribute("aria-hidden", agent ? "true" : "false");
  }
}

function launcherEl() {
  return document.querySelector("#launcher");
}

export function launcherShelfFaceActive() {
  const taskbar = taskbarEl();
  if (!taskbar) {
    return false;
  }
  if (taskbar.classList.contains("is-launcher-face")) {
    return true;
  }
  const phase = taskbar.dataset.agentMorph || "";
  return (
    Boolean(taskbar.dataset.launcherMorphing) &&
    (phase === "exit" ||
      phase === "grow" ||
      phase === "enter" ||
      phase === "leave" ||
      phase === "shrink")
  );
}

function setLauncherDomOpen(open) {
  const launcher = launcherEl();
  const toggle = document.querySelector("#launcher-toggle");
  if (!launcher) {
    return;
  }
  launcher.hidden = !open;
  launcher.inert = !open;
  launcher.setAttribute("aria-hidden", open ? "false" : "true");
  launcher.dataset.open = open ? "true" : "false";
  toggle?.setAttribute("aria-expanded", open ? "true" : "false");
}

/** Idle dock width → Apps face width (height-only grow). Soft min ~320. */
function lockLauncherFaceWidth(taskbar) {
  if (!taskbar) {
    return 0;
  }
  const dockW = taskbar.getBoundingClientRect().width;
  const maxW = Math.max(200, window.innerWidth - 20);
  const minW = Math.min(320, maxW);
  const width = Math.round(Math.min(maxW, Math.max(minW, dockW)));
  /* Inline px width — custom props don't interpolate (was the width jump). */
  taskbar.style.setProperty("--shelf-launcher-w", `${width}px`);
  taskbar.style.width = `${width}px`;
  return width;
}

function clearLauncherFaceWidth(taskbar) {
  if (!taskbar) {
    return;
  }
  taskbar.style.removeProperty("--shelf-launcher-w");
  taskbar.style.removeProperty("width");
  taskbar.classList.remove("is-launcher-width-easing");
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

function geometryTransition(durationMs = MORPH_STRETCH_MS, { freezeHorizontal = false } = {}) {
  const t = `${durationMs}ms ${MORPH_EASE}`;
  /* Apps face grows height only — width/padding FLIP was the side-nudge. */
  if (freezeHorizontal) {
    return [`height ${t}`, `min-height ${t}`, `border-radius ${t}`, `box-shadow ${t}`].join(
      ", ",
    );
  }
  return [
    `width ${t}`,
    `height ${t}`,
    `min-height ${t}`,
    `padding ${t}`,
    `border-radius ${t}`,
    `box-shadow ${t}`,
  ].join(", ");
}

function flipTaskbarGeometry(
  taskbar,
  applyEndState,
  durationMs = MORPH_STRETCH_MS,
  { freezeHorizontal = false } = {},
) {
  const from = readBox(taskbar);
  applyEndState();
  const to = readBox(taskbar);
  if (freezeHorizontal) {
    to.width = from.width;
    to.padding = from.padding;
  }

  taskbar.style.transition = "none";
  lockBox(taskbar, from);
  void taskbar.offsetWidth;

  taskbar.style.transition = geometryTransition(durationMs, { freezeHorizontal });
  lockBox(taskbar, to);
  return to;
}

/** Drop dock-mag lift/shift for the morph only — mag returns on next hover. */
function calmDockIconsForMorph(taskbar) {
  taskbar?.querySelectorAll(".taskbar-icon").forEach((icon) => {
    icon.style.transform = "";
  });
  taskbar?.querySelectorAll(".taskbar-item").forEach((item) => {
    item.style.removeProperty("--dock-shift");
  });
}

function formatAttachmentContext(attachments) {
  const parts = [];
  for (const item of attachments) {
    const name = item.name || "file";
    if (item.text && item.kind === "desktop" && item.uri) {
      parts.push(
        `Attached Desktop «${name}» (${item.uri}) · Inbox library.read extract (cited):\n${item.text}`,
      );
      continue;
    }
    if (item.text) {
      parts.push(
        `Attached «${name}» (size-capped local extract · not a Library grant):\n${item.text}`,
      );
      continue;
    }
    if (item.uri) {
      parts.push(
        `Attached desktop object «${name}» (${item.uri}). ` +
          `Content was not extracted — a library.read grant is required (Inbox; UI ≠ authority).`,
      );
      continue;
    }
    parts.push(`Attached «${name}» (name only · binary/unsupported for extract).`);
  }
  return parts.join("\n\n");
}

export async function sendAgentComposerMessage() {
  const input = composerInput();
  const taskbar = taskbarEl();
  const btn = sendButton();
  if (!input || !taskbar) {
    return;
  }

  if (btn?.dataset.mode === "stop") {
    stopAgentHarnessStream();
    return;
  }

  const prompt = input.value.trim();
  if (!prompt) {
    return;
  }

  if (!taskbar.classList.contains("is-agent-face")) {
    showAgentShelfFace();
  }

  const attached = composerAttachments.slice();
  const context = formatAttachmentContext(attached);
  const libraryKb = formatLibraryKbContext();
  const blocks = [libraryKb, context].filter(Boolean).join("\n\n");
  const outbound = blocks ? `${prompt}\n\n---\n${blocks}` : prompt;

  input.value = "";
  autosizeComposer(input);
  clearComposerAttachments();
  closeAttachMenu();

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
  await sendToAgentHarness(outbound);
}

function openHarnessWithShelf() {
  if (!agentHarnessActive()) {
    showAgentHarness({ fromShelf: true });
  }
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

  /* Apps face must yield before Agent morph (shared FLIP slot). */
  snapIdleFromLauncherFace();

  setMorphPhase(taskbar, "exit");
  syncFaceAria({ agent: false, launcher: false });

  scheduleMorph(generation, MORPH_EXIT_MS, () => {
    setMorphPhase(taskbar, "grow");
    syncFaceAria({ agent: true, launcher: false });
    autosizeComposer(input);

    flipTaskbarGeometry(taskbar, () => {
      taskbar.classList.remove("is-launcher-face");
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
  snapIdleFromLauncherFace();
  taskbar.classList.add("is-agent-face");
  setMorphPhase(taskbar, "enter");
  syncFaceAria({ agent: true, launcher: false });
  toggle?.setAttribute("aria-pressed", "true");
  setAgentComposerProcessing(false);
  autosizeComposer(input);
}

function snapIdleFromLauncherFace() {
  const taskbar = taskbarEl();
  if (!taskbar) {
    return;
  }
  delete taskbar.dataset.launcherMorphing;
  taskbar.classList.remove("is-launcher-face", "is-launcher-closing");
  clearLauncherFaceWidth(taskbar);
  setLauncherDomOpen(false);
  document.querySelector("#launcher-toggle")?.setAttribute("aria-expanded", "false");
}

/** Instant idle Shelf — Space leave / MC → Desktop without reverse morph glitch. */
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
  snapIdleFromLauncherFace();
  setMorphPhase(taskbar, "");
  syncFaceAria({ agent: false, launcher: false });
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
    syncFaceAria({ agent: false, launcher: false });
    document.documentElement.style.removeProperty("--agent-column-width");
    document.documentElement.style.removeProperty("--agent-column-left");
    document.documentElement.style.removeProperty("--harness-composer-clearance");
    toggle?.focus({ preventScroll: true });
  };

  if (!taskbar.classList.contains("is-agent-face")) {
    hideAgentHarness({ restoreShelfApps: false, syncStage: true });
    finishClosed();
    return;
  }

  if (prefersReducedMotion()) {
    hideAgentHarness({ restoreShelfApps: false, syncStage: true });
    snapAppsShelfFace();
    toggle?.focus({ preventScroll: true });
    return;
  }

  /* Chrome leaves first; harness exhales as dock begins shrink — not before. */
  setMorphPhase(taskbar, "leave");
  syncFaceAria({ agent: true, launcher: false });

  scheduleMorph(generation, MORPH_LEAVE_MS, () => {
    hideAgentHarness({ restoreShelfApps: false, syncStage: true });

    setMorphPhase(taskbar, "shrink");
    syncFaceAria({ agent: false, launcher: false });

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

/** Morph idle Shelf → Apps launcher face (same FLIP principles as Agent). */
export function showLauncherShelfFace() {
  const taskbar = taskbarEl();
  if (!taskbar) {
    setLauncherDomOpen(true);
    return;
  }
  if (taskbar.dataset.agentPreview !== "1") {
    lockLauncherFaceWidth(taskbar);
    taskbar.classList.add("is-launcher-face");
    setLauncherDomOpen(true);
    syncFaceAria({ agent: false, launcher: true });
    return;
  }
  if (taskbar.classList.contains("is-launcher-face") && !taskbar.dataset.agentMorph) {
    setLauncherDomOpen(true);
    return;
  }
  if (agentShelfFaceActive()) {
    /* Snap Agent closed so Apps morph owns the dock. */
    morphGeneration += 1;
    clearMorphTimer();
    clearBoxLock(taskbar);
    taskbar.classList.remove("is-agent-face");
    setMorphPhase(taskbar, "");
    document.querySelector("#agent-shelf-toggle")?.setAttribute("aria-pressed", "false");
    setAgentComposerProcessing(false);
    hideAgentHarness({ restoreShelfApps: false, syncStage: true });
  }

  /* Capture idle dock width before any face class — morph grows height only. */
  lockLauncherFaceWidth(taskbar);

  if (prefersReducedMotion()) {
    morphGeneration += 1;
    clearMorphTimer();
    clearBoxLock(taskbar);
    taskbar.classList.remove("is-agent-face");
    taskbar.classList.add("is-launcher-face");
    taskbar.dataset.launcherMorphing = "1";
    setMorphPhase(taskbar, "enter");
    setLauncherDomOpen(true);
    syncFaceAria({ agent: false, launcher: true });
    delete taskbar.dataset.launcherMorphing;
    return;
  }

  const generation = (morphGeneration += 1);
  clearMorphTimer();
  taskbar.dataset.launcherMorphing = "1";
  /* Calm mag for the stretch only — otherwise lifted icons “fall into place”. */
  calmDockIconsForMorph(taskbar);

  setLauncherDomOpen(true);
  setMorphPhase(taskbar, "exit");
  syncFaceAria({ agent: false, launcher: false });

  scheduleMorph(generation, MORPH_EXIT_MS, () => {
    setMorphPhase(taskbar, "grow");
    syncFaceAria({ agent: false, launcher: true });

    flipTaskbarGeometry(
      taskbar,
      () => {
        taskbar.classList.remove("is-agent-face");
        taskbar.classList.add("is-launcher-face");
      },
      MORPH_STRETCH_MS,
      { freezeHorizontal: true },
    );

    scheduleMorph(generation, MORPH_ENTER_AT_MS, () => {
      if (generation !== morphGeneration) {
        return;
      }
      setMorphPhase(taskbar, "enter");
    });

    window.setTimeout(() => {
      if (generation !== morphGeneration) {
        return;
      }
      clearBoxLock(taskbar);
      /* Keep pixel width after FLIP unlock — CSS vars don't ease on pin. */
      const lockedW = taskbar.style.getPropertyValue("--shelf-launcher-w").trim();
      if (lockedW) {
        taskbar.style.width = lockedW;
      }
      delete taskbar.dataset.launcherMorphing;
    }, MORPH_STRETCH_MS + 32);
  });
}

export function hideLauncherShelfFace({ snap = false } = {}) {
  const taskbar = taskbarEl();
  const toggle = document.querySelector("#launcher-toggle");
  if (!taskbar) {
    setLauncherDomOpen(false);
    return;
  }

  if (!taskbar.classList.contains("is-launcher-face") && !taskbar.dataset.launcherMorphing) {
    setLauncherDomOpen(false);
    return;
  }

  const generation = (morphGeneration += 1);
  clearMorphTimer();

  const finishClosed = () => {
    /* Hold open-face width through the class swap — dismiss is height-only. */
    const heldW = Math.round(taskbar.getBoundingClientRect().width);
    clearBoxLock(taskbar);
    delete taskbar.dataset.launcherMorphing;
    taskbar.classList.remove("is-launcher-face", "is-launcher-closing");
    clearLauncherFaceWidth(taskbar);
    if (heldW > 0) {
      taskbar.style.width = `${heldW}px`;
    }
    setMorphPhase(taskbar, "");
    setLauncherDomOpen(false);
    syncFaceAria({ agent: false, launcher: false });
    toggle?.focus({ preventScroll: true });
  };

  if (snap || prefersReducedMotion() || !taskbar.classList.contains("is-launcher-face")) {
    finishClosed();
    return;
  }

  taskbar.dataset.launcherMorphing = "1";
  calmDockIconsForMorph(taskbar);
  setMorphPhase(taskbar, "leave");
  syncFaceAria({ agent: false, launcher: true });

  scheduleMorph(generation, MORPH_LEAVE_MS, () => {
    setMorphPhase(taskbar, "shrink");
    syncFaceAria({ agent: false, launcher: false });

    /*
      Keep is-launcher-face through the FLIP and only add is-launcher-closing
      (height → dock). Height-only FLIP — no width/padding (side-nudge).
    */
    flipTaskbarGeometry(
      taskbar,
      () => {
        taskbar.classList.add("is-launcher-closing");
      },
      MORPH_STRETCH_MS,
      { freezeHorizontal: true },
    );

    scheduleMorph(generation, MORPH_STRETCH_MS, () => {
      finishClosed();
    });
  });
}

function attachmentsHost() {
  return document.querySelector("[data-agent-attachments]");
}

function attachInput() {
  return document.getElementById("agent-attach-input");
}

function renderComposerAttachments() {
  const host = attachmentsHost();
  if (!host) {
    return;
  }
  host.replaceChildren();
  if (!composerAttachments.length) {
    host.hidden = true;
    const field = composerInput();
    if (field) {
      field.placeholder = "Ask on this machine";
    }
    return;
  }
  host.hidden = false;
  for (const file of composerAttachments) {
    const chip = document.createElement("button");
    chip.type = "button";
    chip.className = "agent-attach-chip";
    chip.dataset.attachId = file.id;
    chip.title = "Remove attachment";
    chip.innerHTML =
      `<span class="agent-attach-chip-name"></span>` +
      `<span class="agent-attach-chip-meta"></span>` +
      `<span class="agent-attach-chip-x" aria-hidden="true">×</span>`;
    chip.querySelector(".agent-attach-chip-name").textContent = file.name;
    const meta = file.text
      ? `extract · ${Math.max(1, Math.round(file.text.length / 1024))} KB`
      : file.uri
        ? "desktop · needs grant"
        : `${Math.max(1, Math.round((file.size || 0) / 1024))} KB`;
    chip.querySelector(".agent-attach-chip-meta").textContent = meta;
    host.append(chip);
  }
}

export function clearComposerAttachments() {
  composerAttachments = [];
  renderComposerAttachments();
}

export function addComposerAttachment(entry) {
  if (!entry?.name) {
    return;
  }
  attachSeq += 1;
  composerAttachments.push({
    id: `att-${attachSeq}`,
    name: String(entry.name).slice(0, 180),
    size: Number(entry.size) || 0,
    uri: entry.uri ? String(entry.uri).slice(0, 1024) : "",
    text: entry.text ? String(entry.text).slice(0, MAX_ATTACH_TEXT_CHARS) : "",
    kind: entry.kind || (entry.uri ? "desktop" : "file"),
  });
  if (composerAttachments.length > 8) {
    composerAttachments = composerAttachments.slice(-8);
  }
  renderComposerAttachments();
  const field = composerInput();
  if (field && !field.value.trim()) {
    field.placeholder = `Ask about ${entry.name}…`;
  }
}

function attachMenuEl() {
  return document.querySelector("[data-agent-attach-menu]");
}

function closeAttachMenu() {
  const menu = attachMenuEl();
  if (menu) {
    menu.hidden = true;
  }
}

function openDeviceFilePicker() {
  const input = attachInput();
  if (!input) {
    return;
  }
  input.value = "";
  input.click();
}

export function openAttachPicker() {
  const menu = attachMenuEl();
  if (!menu) {
    openDeviceFilePicker();
    return;
  }
  menu.hidden = !menu.hidden;
  if (!menu.hidden) {
    hostRenderDesktopAttachOptions?.(menu);
  }
}

/** Filled by harness bind — lists Desktop objects into the attach menu. */
let hostRenderDesktopAttachOptions = null;

export function bindShelfAttachHost(api = {}) {
  hostRenderDesktopAttachOptions = typeof api.renderDesktopAttachOptions === "function"
    ? api.renderDesktopAttachOptions
    : null;
}

async function readTextAttachment(file) {
  if (!file || file.size > MAX_ATTACH_READ_BYTES) {
    return "";
  }
  const type = String(file.type || "");
  const name = String(file.name || "");
  const looksText =
    TEXT_ATTACH_RE.test(type) ||
    /\.(txt|md|markdown|json|csv|tsv|log|yml|yaml|toml|rs|js|ts|tsx|jsx|py|go|c|h|cpp|html|css|svg)$/i.test(
      name,
    );
  if (!looksText) {
    return "";
  }
  try {
    const raw = await file.text();
    return String(raw || "").slice(0, MAX_ATTACH_TEXT_CHARS);
  } catch {
    return "";
  }
}

async function onAttachFilesSelected(event) {
  const input = event.target;
  const files = [...(input?.files || [])];
  if (!files.length) {
    return;
  }
  for (const file of files.slice(0, 8)) {
    const text = await readTextAttachment(file);
    addComposerAttachment({
      kind: "file",
      name: file.name || "file",
      size: Number(file.size) || 0,
      text,
    });
  }
  closeAttachMenu();
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

  registerEscapeHandler("launcher-shelf", {
    priority: 74,
    isActive: () => launcherShelfFaceActive(),
    dismiss: () => hideLauncherShelfFace(),
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
    if (event.target.closest?.("#agent-attach-btn")) {
      event.preventDefault();
      openAttachPicker();
      return;
    }
    if (event.target.closest?.("[data-attach-device-files]")) {
      event.preventDefault();
      closeAttachMenu();
      openDeviceFilePicker();
      return;
    }
    const desktopOpt = event.target.closest?.("[data-attach-desktop-uri]");
    if (desktopOpt?.dataset.attachDesktopUri) {
      event.preventDefault();
      const uri = desktopOpt.dataset.attachDesktopUri;
      const name = desktopOpt.dataset.attachDesktopName || "Desktop object";
      const size = Number(desktopOpt.dataset.attachDesktopSize) || 0;
      closeAttachMenu();
      void (async () => {
        const grant = getReadyLibraryReadGrant();
        let text = "";
        if (grant?.requestId) {
          try {
            const extracted = await extractAgentLibraryRead(grant.requestId, uri);
            text = String(extracted?.text || "");
          } catch (err) {
            console.warn("Desktop library.read extract failed", err);
          }
        }
        addComposerAttachment({
          kind: "desktop",
          name,
          uri,
          size,
          text,
        });
      })();
      return;
    }
    if (
      !event.target.closest?.("[data-agent-attach-menu]") &&
      !event.target.closest?.("#agent-attach-btn")
    ) {
      closeAttachMenu();
    }
    const chip = event.target.closest?.(".agent-attach-chip[data-attach-id]");
    if (chip?.dataset.attachId) {
      event.preventDefault();
      composerAttachments = composerAttachments.filter((f) => f.id !== chip.dataset.attachId);
      renderComposerAttachments();
      return;
    }
    const send = event.target.closest?.("#agent-composer-send");
    if (send) {
      event.preventDefault();
      void sendAgentComposerMessage();
      return;
    }
  });

  document.addEventListener("change", (event) => {
    if (event.target?.id === "agent-attach-input") {
      onAttachFilesSelected(event);
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
