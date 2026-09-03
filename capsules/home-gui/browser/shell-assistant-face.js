/* Assistant face and Space: the Shelf morphs into the Assistant's composer and
   Home breathes into its room.

   Ownership: Home GUI owns the morph and the place the room occupies — nothing
   else. The room and the composer are one capsule in its own frame, launched
   through the same authority-carrying path as windows and rails (see
   shell-wallet-rail.js). No conversation state, draft or run lives in the
   shell, so the harness inside can be upgraded or swapped without touching
   Home.

   Motion, same cadence as the Apps face: the dock row clears (exit), the pill
   stretches to composer geometry as empty glass (grow) while the room breathes
   in behind it; near the end of the stretch Home hands the pill over: this
   glass goes clear as the capsule's identical pill, at the same place, fades
   in over it and its composer rises. Closing: the composer fades on the
   capsule's pill (leave), Home takes the pill back, the room breathes out as
   the pill shrinks, the dock row returns. Phase is `data-assistant-morph`.

   Contract with the frame (event.source pins the sender; the frame is
   opaque-sandboxed so target origin is "*"):
     Home → capsule   home-agent:open | home-agent:shelf-handover {on} | home-agent:close
     capsule → Home   home-agent:ready | home-agent:shelf-metrics {width,height,radius}
                      home-agent:close | home-agent:open-viewer */

import {
  closeOtherShellPopovers,
  registerShellPopover,
} from "./shell-popovers.js?v=home-20260813a";
import {
  agentStageId,
  bindAgentSpace,
  getActiveStageId,
  isAgentSpace,
  setActiveStage,
} from "./shell-stages.js?v=home-20260813a";

const FACE_ID = "assistant-face";
const TARGET_ID = "home-agent";
/* Dock row clears just before the stretch. */
const EXIT_MS = 90;
/* The room begins breathing in at this share of the stretch. */
const ROOM_AT = 0.28;
/* The pill is handed to the capsule at this share of the stretch. */
const ENTER_AT = 0.82;
/* Home's glass is back before the reverse stretch. */
const LEAVE_MS = 140;
/* Dock row fade back after the shrink. */
const RETURN_MS = 160;
/* Room breathe out (matches the harness's own rise). */
const ROOM_MS = 720;
const FACE_MIN_W = 320;
const FACE_MAX_W = 720;

let deps = null;
let generation = 0;
let timers = [];
let idleDockWidth = 0;

let spaceEl = null;
let frame = null;
let frameReady = false;
let launching = false;
let spaceHideTimer = 0;
let closing = false;
/* The Space the user came from; the ring returns there when the room closes. */
let returnStageId = "";

function taskbarEl() {
  return document.querySelector(".taskbar");
}

function faceEl() {
  return document.querySelector("#assistant-face");
}

function toggleEl() {
  return document.querySelector("#assistant-toggle");
}

function clearTimers() {
  for (const id of timers) {
    window.clearTimeout(id);
  }
  timers = [];
}

function after(ms, work) {
  const mine = generation;
  timers.push(
    window.setTimeout(() => {
      if (mine === generation) {
        work();
      }
    }, ms),
  );
}

function reducedMotion() {
  return window.matchMedia?.("(prefers-reduced-motion: reduce)")?.matches === true;
}

function faceMs(taskbar) {
  const value = parseFloat(
    window.getComputedStyle?.(taskbar)?.getPropertyValue?.("--shelf-face-ms"),
  );
  return Number.isFinite(value) && value > 0 ? value : 950;
}

function setPhase(taskbar, phase) {
  if (phase) {
    taskbar.dataset.assistantMorph = phase;
  } else {
    delete taskbar.dataset.assistantMorph;
  }
}

function setFaceOpen(open) {
  const face = faceEl();
  const toggle = toggleEl();
  if (face) {
    face.hidden = !open;
    face.inert = !open;
    face.setAttribute("aria-hidden", open ? "false" : "true");
  }
  toggle?.setAttribute("aria-expanded", open ? "true" : "false");
}

/* Default geometry is the composer's idle pill; the capsule reports the live
   one as soon as its pill lays out, and again whenever it grows. */
function lockFaceGeometry(taskbar) {
  if (taskbar.style.getPropertyValue("--assistant-face-w")) {
    return;
  }
  const maxW = Math.max(FACE_MIN_W, window.innerWidth - 48);
  taskbar.style.setProperty("--assistant-face-w", `${Math.round(Math.min(maxW, FACE_MAX_W))}px`);
}

/* The width the face class resolves to (its max-width is the viewport less
   the gutters), read without a layout flush so the stretch can start eased. */
function faceTargetWidth(taskbar) {
  const locked = parseFloat(taskbar.style.getPropertyValue("--assistant-face-w"));
  const width = Number.isFinite(locked) && locked >= FACE_MIN_W ? locked : FACE_MAX_W;
  return Math.round(Math.min(width, window.innerWidth - 48));
}

function applyFaceMetrics(metrics) {
  const taskbar = taskbarEl();
  if (!taskbar || !metrics) {
    return;
  }
  const width = Number(metrics.width);
  const height = Number(metrics.height);
  if (Number.isFinite(width) && width >= FACE_MIN_W) {
    taskbar.style.setProperty("--assistant-face-w", `${Math.round(width)}px`);
  }
  if (Number.isFinite(height) && height >= 44) {
    taskbar.style.setProperty("--assistant-face-h", `${Math.round(height)}px`);
  }
  if (typeof metrics.radius === "string" && /^\d+(\.\d+)?px$/.test(metrics.radius)) {
    taskbar.style.setProperty("--assistant-face-r", metrics.radius);
  }
}

function clearFaceGeometry(taskbar) {
  taskbar.style.removeProperty("--assistant-face-w");
  taskbar.style.removeProperty("--assistant-face-h");
  taskbar.style.removeProperty("--assistant-face-r");
}

function setHandover(on) {
  taskbarEl()?.classList.toggle("is-assistant-handover", on);
  postToFrame({ type: "home-agent:shelf-handover", on });
}

export function assistantFaceActive() {
  return taskbarEl()?.classList.contains("is-assistant-face") === true;
}

/* ---- Space ring: the room is the Agent Space, far left of the ring -------- */

const STAGE_QUIET = { announce: false, focus: false, animate: false, syncAgentSpace: false };

function enterAgentStage() {
  const active = getActiveStageId();
  if (isAgentSpace(active)) {
    return;
  }
  returnStageId = active;
  setActiveStage(agentStageId(), STAGE_QUIET);
}

function leaveAgentStage() {
  if (!isAgentSpace(getActiveStageId())) {
    return;
  }
  setActiveStage(returnStageId, STAGE_QUIET);
}

/* ---- Space: the room and the capsule frame inside it ---------------------- */

export function assistantSpaceActive() {
  return document.body.classList.contains("assistant-space-active");
}

function postToFrame(message) {
  const target = frame?.contentWindow;
  if (!target || !frame.dataset.route || !frameReady) {
    return;
  }
  target.postMessage(message, "*");
}

function markFrameReady() {
  if (frameReady) {
    return;
  }
  frameReady = true;
  frame?.classList.add("is-ready");
  deps.pushUiPreferencesToFrameWindow(frame?.contentWindow);
  if (assistantSpaceActive()) {
    postToFrame({ type: "home-agent:open" });
    if (taskbarEl()?.classList.contains("is-assistant-handover")) {
      postToFrame({ type: "home-agent:shelf-handover", on: true });
    }
  }
}

function showSpaceError(error) {
  const block = document.querySelector("#assistant-space-error");
  const detail = document.querySelector("#assistant-space-error-detail");
  if (block) {
    block.hidden = false;
  }
  if (detail) {
    detail.textContent = String(error?.message || error);
  }
}

/* One launch; the frame keeps its session across open/close (hide, don't
   unload) and is torn down only when the host retires the GUI surface. */
async function mountAssistantFrame() {
  if (!frame || launching || frame.dataset.route) {
    return;
  }
  launching = true;
  frameReady = false;
  const block = document.querySelector("#assistant-space-error");
  if (block) {
    block.hidden = true;
  }
  frame.hidden = false;
  frame.classList.remove("is-ready");
  try {
    const launched = await deps.launchHomeTarget(TARGET_ID, {});
    if (launched.attach_kind !== "iframe") {
      throw new Error(`unsupported attach kind: ${launched.attach_kind || "unknown"}`);
    }
    if (
      typeof launched.launch_status === "string" &&
      launched.launch_status.trim() !== "" &&
      launched.launch_status !== "launched"
    ) {
      throw new Error(
        typeof launched.launch_detail === "string" && launched.launch_detail.trim() !== ""
          ? launched.launch_detail.trim()
          : `launch status: ${launched.launch_status}`,
      );
    }
    frame.setAttribute("sandbox", deps.iframeSandboxForLaunch(launched));
    frame.setAttribute("allow", deps.iframeAllowForLaunch(launched));
    frame.title = deps.escapeHtml(launched.title || "Assistant");
    const route = new URL(String(launched.route || ""), window.location.origin);
    frame.src = route.href;
    frame.dataset.route = route.href;
  } catch (error) {
    frame.hidden = true;
    frameReady = false;
    showSpaceError(error);
  } finally {
    launching = false;
  }
}

function openAssistantSpace() {
  if (!spaceEl) {
    return;
  }
  window.clearTimeout(spaceHideTimer);
  spaceEl.hidden = false;
  spaceEl.inert = false;
  spaceEl.setAttribute("aria-hidden", "false");
  document.body.classList.remove("assistant-space-leaving");
  document.body.classList.add("assistant-space-active");
  spaceEl.classList.add("is-visible");
  void mountAssistantFrame();
  postToFrame({ type: "home-agent:open" });
}

function closeAssistantSpace({ instant = false } = {}) {
  if (!spaceEl) {
    return;
  }
  window.clearTimeout(spaceHideTimer);
  spaceEl.classList.remove("is-visible");
  /* Sent on every leave, including one the capsule asked for: the capsule
     lowers its room on this message and never on its own request, so both
     paths are the same morph. */
  postToFrame({ type: "home-agent:close" });
  const park = () => {
    document.body.classList.remove("assistant-space-leaving");
    spaceEl.hidden = true;
    spaceEl.inert = true;
    spaceEl.setAttribute("aria-hidden", "true");
  };
  if (instant || reducedMotion()) {
    document.body.classList.remove("assistant-space-active");
    park();
    return;
  }
  document.body.classList.add("assistant-space-leaving");
  document.body.classList.remove("assistant-space-active");
  spaceHideTimer = window.setTimeout(park, ROOM_MS + 40);
}

/* Host retires the GUI surface (sign-out, lock): drop the session with it. */
export function retireAssistantSpace() {
  hideAssistantFace({ instant: true });
  const taskbar = taskbarEl();
  if (taskbar) {
    clearFaceGeometry(taskbar);
  }
  frameReady = false;
  if (frame) {
    frame.removeAttribute("src");
    delete frame.dataset.route;
    frame.hidden = true;
    frame.classList.remove("is-ready");
  }
}

function onFrameMessage(event) {
  if (!frame || event.source !== frame.contentWindow) {
    return;
  }
  const message = event.data;
  if (!message || typeof message !== "object" || typeof message.type !== "string") {
    return;
  }
  switch (message.type) {
    case "home-agent:ready":
      markFrameReady();
      break;
    case "home-agent:shelf-metrics":
      applyFaceMetrics(message);
      break;
    case "home-agent:close":
      if (assistantFaceActive() || taskbarEl()?.dataset.assistantMorph) {
        hideAssistantFace();
      }
      break;
    default:
      break;
  }
}

/* ---- Face ------------------------------------------------------------------ */

/* The toggle exists only when the capsule is installed; it wears the capsule's
   own declared icon like every other dock item. */
export function syncAssistantFaceAvailability(summary) {
  const toggle = toggleEl();
  if (!toggle || !deps) {
    return;
  }
  const target = deps.targetById(summary, TARGET_ID);
  toggle.hidden = !target;
  if (target) {
    deps.mountGlyph(toggle.querySelector(".taskbar-item-icon"), TARGET_ID);
  } else if (assistantFaceActive()) {
    retireAssistantSpace();
  }
}

export function showAssistantFace() {
  const taskbar = taskbarEl();
  if (!taskbar || !deps || assistantFaceActive() || toggleEl()?.hidden) {
    return;
  }
  generation += 1;
  clearTimers();
  closing = false;
  closeOtherShellPopovers(FACE_ID);
  idleDockWidth = Math.round(taskbar.getBoundingClientRect().width);
  lockFaceGeometry(taskbar);

  if (reducedMotion()) {
    taskbar.classList.add("is-assistant-face");
    setFaceOpen(true);
    openAssistantSpace();
    setHandover(true);
    enterAgentStage();
    return;
  }

  setPhase(taskbar, "exit");
  after(EXIT_MS, () => {
    setPhase(taskbar, "grow");
    setFaceOpen(true);
    /* Pin the idle width with transitions off, then put the face class on and
       the target width in one flush: width (idle is max-content, so it rides
       an inline pixel value), height, radius and padding all ease together
       from the dock's own shape. Nothing may flush with transitions off after
       the class is on — that is what snaps height and radius to the face. */
    taskbar.style.transition = "none";
    taskbar.style.width = `${idleDockWidth}px`;
    void taskbar.offsetWidth;
    taskbar.style.removeProperty("transition");
    taskbar.classList.add("is-assistant-face");
    taskbar.style.width = `${faceTargetWidth(taskbar)}px`;
    const stretchMs = faceMs(taskbar);
    after(Math.round(stretchMs * ROOM_AT), () => {
      openAssistantSpace();
    });
    after(Math.round(stretchMs * ENTER_AT), () => {
      setPhase(taskbar, "enter");
      setHandover(true);
    });
    after(stretchMs + 32, () => {
      taskbar.style.removeProperty("width");
      setPhase(taskbar, "");
    });
    /* Stages hide off-Space windows and shortcuts by visibility, a cut. Flip
       the ring to Agent only once the floor has ghosted out under the room
       (ROOM_MS after the room opened) so the cut lands on nothing visible. */
    after(Math.round(stretchMs * ROOM_AT) + ROOM_MS, () => {
      enterAgentStage();
    });
  });
}

export function hideAssistantFace({ instant = false } = {}) {
  const taskbar = taskbarEl();
  if (!taskbar) {
    return;
  }
  const wasOpen = assistantFaceActive() || taskbar.dataset.assistantMorph;
  if (!wasOpen || (closing && !instant)) {
    return;
  }
  generation += 1;
  clearTimers();
  closing = true;
  leaveAgentStage();

  const finish = () => {
    taskbar.classList.remove("is-assistant-face");
    setFaceOpen(false);
    setPhase(taskbar, "");
    closing = false;
    toggleEl()?.focus({ preventScroll: true });
  };

  if (instant || reducedMotion()) {
    setHandover(false);
    closeAssistantSpace({ instant: true });
    finish();
    return;
  }

  const stretchMs = faceMs(taskbar);
  /* Leave: the capsule is told at once so its composer fades on its own pill
     through the leave; Home takes the glass back as the shrink begins, and
     the room breathes out with it — not before. */
  setPhase(taskbar, "leave");
  postToFrame({ type: "home-agent:shelf-handover", on: false });
  after(LEAVE_MS, () => {
    closeAssistantSpace();
    setPhase(taskbar, "shrink");
    setFaceOpen(false);
    const fromW = Math.round(taskbar.getBoundingClientRect().width);
    /* The dock row is still out of layout in this phase, so the pill cannot
       measure its idle width: ease explicitly to the width captured at open.
       Lock the start width before the face class goes, so radius, padding
       and height begin easing with transitions live. The handover class comes
       off after that flush, so the glass fades back in over the capsule's
       pill instead of snapping. */
    taskbar.style.transition = "none";
    taskbar.style.width = `${fromW}px`;
    void taskbar.offsetWidth;
    taskbar.style.removeProperty("transition");
    taskbar.classList.remove("is-assistant-face");
    taskbar.classList.remove("is-assistant-handover");
    taskbar.style.setProperty("--dock-width-ms", `${stretchMs}ms`);
    taskbar.classList.add("is-dock-width-easing");
    taskbar.style.width = `${idleDockWidth}px`;
    after(stretchMs + 16, () => {
      taskbar.classList.remove("is-dock-width-easing");
      taskbar.style.removeProperty("width");
      taskbar.style.removeProperty("--dock-width-ms");
      setPhase(taskbar, "return");
      /* The dock may have changed while the face was up: settle from the
         captured width to the live one. */
      deps.easeDockPillWidth(idleDockWidth);
      after(RETURN_MS + 16, () => {
        setPhase(taskbar, "");
        closing = false;
        toggleEl()?.focus({ preventScroll: true });
      });
    });
  });
}

export function toggleAssistantFace() {
  if (assistantFaceActive()) {
    hideAssistantFace();
  } else {
    showAssistantFace();
  }
}

/**
 * Wire the face once the GUI template is in the DOM.
 * @param {{
 *   easeDockPillWidth: (fromW: number, durationName?: string) => void,
 *   targetById: (summary: unknown, targetId: string) => unknown,
 *   mountGlyph: (container: Element | null, targetId: string) => void,
 *   launchHomeTarget: (targetId: string, query: object) => Promise<object>,
 *   iframeSandboxForLaunch: (launched: object) => string,
 *   iframeAllowForLaunch: (launched: object) => string,
 *   pushUiPreferencesToFrameWindow: (frameWindow: Window | null | undefined) => void,
 *   escapeHtml: (value: string) => string,
 * }} dependencies
 */
export function bindAssistantFace(dependencies) {
  deps = dependencies;
  spaceEl = document.querySelector("#assistant-space");
  frame = document.querySelector("#assistant-space-frame");
  registerShellPopover(FACE_ID, () => hideAssistantFace({ instant: true }));
  bindAgentSpace({
    available: () => Boolean(toggleEl()) && !toggleEl().hidden,
    busy: () => Boolean(taskbarEl()?.dataset.assistantMorph),
    open: () => showAssistantFace(),
    close: () => hideAssistantFace(),
  });
  toggleEl()?.addEventListener("click", () => toggleAssistantFace());
  document.querySelector("#assistant-space-retry")?.addEventListener("click", () => {
    void mountAssistantFrame();
  });
  window.addEventListener("message", onFrameMessage);
}
