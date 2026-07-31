/* Agent Harness (preview) — Home drops, Shelf stays as composer hinge.
   UI ≠ authority (Principle 16): never mints grants, never opens Carrier/
   capsule ambient paths. Mock stream only until agentic/runtime waves wire
   explicit, revocable tools (fail-closed). */

import {
  syncAgentSendButton,
  composerInput as shelfComposerInput,
  hideAgentShelfFace,
  agentShelfFaceActive,
  snapAgentShelfFace,
  snapAppsShelfFace,
} from "./agent-shelf.js?v=home-20260728ag";
import {
  enableHarnessMenubarReveal,
  clearHarnessMenubarReveal,
  agentStageId,
  desktopStageId,
  getActiveStageId,
  isAgentSpace,
  setActiveStage,
  syncSpacePager,
} from "./shell-stages.js?v=home-20260728ag";
import { registerEscapeHandler } from "./shell-popovers.js?v=home-20260728ag";
import {
  resetMockCapabilities,
  getSelectedModel,
  setSelectedModelId,
  listInstalledModels,
  listRecommendedModels,
  mockInstallModel,
  probeHardwareEstimate,
  fitForModel,
  recommendedModel,
  loadReasoningVisible,
  setReasoningVisible,
  setPlanMarkdown,
  maybeUpdatePlanFromPrompt,
  requestModelGet,
  removeProject,
} from "./mock-agent-provider.js?v=home-20260728ag";
import {
  bindAgentWorkspaceSnapshot,
} from "./shell-windows.js?v=home-20260728ag";
import { TIP } from "./agent-tip.js?v=home-20260728ag";
import { registerAgentHarnessApi } from "./agent-send.js?v=home-20260728ag";
import {
  bindAgentWorkspaceStore,
  getAgentWorkspaceSnapshot,
  applyAgentWorkspaceSnapshot,
  persistAgentWorkspaceSoon,
} from "./agent-workspace.js?v=home-20260728ag";
import {
  bindAgentConfigure,
  harnessPageOpen,
  openHarnessPage,
  closeHarnessPage,
  renderHarnessPage,
  openConfigureSection,
  openWorkbench,
  closeWorkbench,
  setWorkbenchTab,
  syncWorkbenchPanels,
  syncWorkbenchOpenUi,
} from "./agent-configure.js?v=home-20260728ag";
import {
  bindAgentGrants,
  syncTruthStrip,
  appendGrantCard,
  paintGrantCardResolved,
  resolveGrantFromCard,
  sessionAlreadyHasGrant,
  maybeOfferToolAfterReply,
  hydrateCapabilitiesFromSession,
} from "./agent-grants.js?v=home-20260728ag";
import {
  bindAgentStream,
  clearStreamTimer,
  titleFromPrompt,
  escapeHtml,
  renderMarkdown,
  setTitle,
  appendThinkingBlock,
  finishThinkingBlock,
  appendToolTimelineRow,
  finishToolTimelineRow,
  appendFollowUpChips,
  renderFollowUpQueue,
  enqueueFollowUp,
  drainFollowUpQueue,
  appendMessage,
  showEmptyState,
  renderActiveSession,
  stopMockStream,
  startTurnForPrompt,
} from "./agent-stream.js?v=home-20260728ag";
import {
  getLiveInferenceState,
  probeLiveInference,
} from "./agent-live.js?v=home-20260728ag";
import {
  bindAgentSessions,
  relativeTime,
  touchSession,
  exportActiveSessionMarkdown,
  sessionSearchOpen,
  renderSessionSearchResults,
  openSessionSearch,
  closeSessionSearch,
  appendSessionRow,
  renderProjectsNav,
  renderSessions,
  projectCreateEl,
  closeProjectCreate,
  openProjectCreate,
  submitProjectCreate,
  ensureSessionForPrompt,
  selectSession,
  newChat,
  renameSession,
  deleteSession,
  assignSessionProject,
  sessionActionsEl,
  sessionActionsOpen,
  closeSessionActions,
  openSessionActions,
  runSessionAction,
} from "./agent-sessions.js?v=home-20260728ag";
export { getAgentWorkspaceSnapshot, applyAgentWorkspaceSnapshot };

let workbenchTab = "outputs";
/** Right rail closed by default in Chat (ChatGPT/Claude-like); opens on substance. */
let workbenchOpen = false;
/** User closed it — don't auto-reopen until Build / new substance nudge. */
let workbenchUserClosed = false;



const HOME_BREATHE_MS = 780;
const HOME_RISE_MS = 720;
const HARNESS_CONTENT_AT_MS = 180;
const PARTICLE_COUNT = 120;
/** Part X — drawer / pill composer breakpoint (matches Outputs-hide). */
const HARNESS_NARROW_MQ = "(max-width: 900px)";

const SEED_SESSIONS = [
  {
    id: "planning",
    title: "Planning weekend",
    group: "Today",
    messages: [
      { role: "user", text: "Help me plan a calm weekend at home." },
      {
        role: "agent",
        text: "Preview session — send from the Shelf composer to stream a mock reply.",
      },
    ],
  },
  {
    id: "wallet",
    title: "Wallet permissions?",
    group: "Today",
    messages: [
      { role: "user", text: "Can the agent touch my Wallet?" },
      {
        role: "agent",
        text:
          "Not without an explicit human ceremony. Wallet tools stay fail-closed — " +
          "never via Approve for me.",
      },
      {
        role: "grant",
        toolId: "wallet.sign",
        state: "denied",
        label: "Wallet · Sign",
        summary: "Agent wants Wallet signing power",
        scope: "human ceremony only · never ambient",
      },
    ],
  },
  {
    id: "downloads",
    title: "Downloads summary",
    group: "Earlier",
    messages: [
      { role: "user", text: "Summarize my Downloads folder." },
      {
        role: "agent",
        text:
          "I can only do that if you grant Library read access. " +
          "Review the grant below — **Preview · mock**, no real Capsule call.",
      },
      {
        role: "grant",
        toolId: "library.read",
        state: "pending",
        args: { path: "Downloads" },
        label: "Library · Read",
        summary: "Agent wants to list files in Downloads",
        scope: "read-only · this session · revocable",
      },
    ],
  },
];

let bound = false;
let active = false;
let streamTimer = 0;
let streamGeneration = 0;
let harnessMotionGen = 0;
let particleRaf = 0;
let dockResizeObserver = null;
let sessions = structuredClone(SEED_SESSIONS);
let activeSessionId = null;
/** True after host session.agent was applied — skip seed overwrite. */
let workspaceHydrated = false;
/** Follow-up prompts queued while a mock turn is streaming (fx7). */
let followUpQueue = [];
let reasoningVisible = loadReasoningVisible();
/** True while thinking or answer mock stream is in flight. */
let turnBusy = false;
/** Session skin: chat | build — presentation only (fx8). */
let sessionMode = "chat";
/** Tool intent chips: read | ask | full — not grants (fx8). */
let toolMode = "read";


bindAgentWorkspaceStore({
  getSessions: () => sessions,
  setSessions: (v) => {
    sessions = v;
  },
  getActiveSessionId: () => activeSessionId,
  setActiveSessionId: (v) => {
    activeSessionId = v;
  },
  getSessionMode: () => sessionMode,
  setSessionMode: (v) => {
    sessionMode = v;
  },
  getToolMode: () => toolMode,
  setToolMode: (v) => {
    toolMode = v;
  },
  getWorkbenchOpen: () => workbenchOpen,
  setWorkbenchOpen: (v) => {
    workbenchOpen = v;
  },
  getWorkbenchTab: () => workbenchTab,
  setWorkbenchTab: (v) => {
    workbenchTab = v;
  },
  getWorkbenchUserClosed: () => workbenchUserClosed,
  setWorkbenchUserClosed: (v) => {
    workbenchUserClosed = v;
  },
  getReasoningVisible: () => reasoningVisible,
  setReasoningVisible: (v) => {
    reasoningVisible = v;
  },
  setWorkspaceHydrated: (v) => {
    workspaceHydrated = v;
  },
});

bindAgentConfigure(
  {
    get workbenchTab() {
      return workbenchTab;
    },
    set workbenchTab(v) {
      workbenchTab = v;
    },
    get workbenchOpen() {
      return workbenchOpen;
    },
    set workbenchOpen(v) {
      workbenchOpen = v;
    },
    get workbenchUserClosed() {
      return workbenchUserClosed;
    },
    set workbenchUserClosed(v) {
      workbenchUserClosed = v;
    },
    get sessionMode() {
      return sessionMode;
    },
    set sessionMode(v) {
      sessionMode = v;
    },
    get toolMode() {
      return toolMode;
    },
    set toolMode(v) {
      toolMode = v;
    },
  },
  {
    closeApproveMenu,
    closeModelMenu,
    isNarrowHarness,
    closeHarnessDrawer,
  },
);

bindAgentGrants(
  {
    getReasoningVisible: () => reasoningVisible,
    getSessions: () => sessions,
    getActiveSessionId: () => activeSessionId,
  },
  {
    syncModelTrigger,
    streamEl,
    clearEmptyState,
    scrollStreamToEnd,
  },
);

bindAgentStream(
  {
    get streamTimer() { return streamTimer; },
    set streamTimer(v) { streamTimer = v; },
    get streamGeneration() { return streamGeneration; },
    set streamGeneration(v) { streamGeneration = v; },
    get turnBusy() { return turnBusy; },
    set turnBusy(v) { turnBusy = v; },
    get followUpQueue() { return followUpQueue; },
    set followUpQueue(v) { followUpQueue = v; },
    get reasoningVisible() { return reasoningVisible; },
    set reasoningVisible(v) { reasoningVisible = v; },
    get sessionMode() { return sessionMode; },
    set sessionMode(v) { sessionMode = v; },
    get sessions() { return sessions; },
    set sessions(v) { sessions = v; },
    get activeSessionId() { return activeSessionId; },
    set activeSessionId(v) { activeSessionId = v; },
    get active() { return active; },
    set active(v) { active = v; },
  },
  {
    streamEl,
    streamScrollEl,
    streamViewportEl,
    clearEmptyState,
    scrollStreamToEnd,
    titleEl,
    signedInFirstName,
    prefersReducedMotion,
    syncComposerGeometry,
    ensureSessionForPrompt,
    renderSessions,
    syncInferenceStatus: () => syncAgentInferenceStatus(),
  },
);

bindAgentSessions(
  {
    get sessions() { return sessions; },
    set sessions(v) { sessions = v; },
    get activeSessionId() { return activeSessionId; },
    set activeSessionId(v) { activeSessionId = v; },
    get sessionMode() { return sessionMode; },
    set sessionMode(v) { sessionMode = v; },
    get toolMode() { return toolMode; },
    set toolMode(v) { toolMode = v; },
    get workbenchOpen() { return workbenchOpen; },
    set workbenchOpen(v) { workbenchOpen = v; },
    get workbenchUserClosed() { return workbenchUserClosed; },
    set workbenchUserClosed(v) { workbenchUserClosed = v; },
    get followUpQueue() { return followUpQueue; },
    set followUpQueue(v) { followUpQueue = v; },
    get active() { return active; },
    set active(v) { active = v; },
  },
  {
    clearFloatingMenuStyle,
    closeApproveMenu,
    closeModelMenu,
    positionFloatingMenu,
    sessionListEl,
  },
);






function setHarnessChromeInert(inert) {
  const nodes = [
    document.querySelector(".desktop-workspace"),
    document.querySelector(".desktop-backdrop"),
    document.querySelector("#wallet-rail"),
    document.querySelector("#inbox-rail"),
  ].filter(Boolean);
  for (const node of nodes) {
    if (inert) {
      node.dataset.harnessInert = node.inert ? "1" : "0";
      node.inert = true;
    } else if (node.dataset.harnessInert != null) {
      node.inert = node.dataset.harnessInert === "1";
      delete node.dataset.harnessInert;
    }
  }
}

function prefersReducedMotion() {
  return window.matchMedia("(prefers-reduced-motion: reduce)").matches;
}

/**
 * Lock stream column to the live Shelf composer box — same left + width to the px.
 * Also keeps the under-dock fade flush with the composer top.
 */
function isNarrowHarness() {
  return (
    typeof window.matchMedia === "function" &&
    window.matchMedia(HARNESS_NARROW_MQ).matches
  );
}

function setHarnessDrawerOpen(open) {
  const next = Boolean(open) && active && isNarrowHarness();
  document.body.classList.toggle("agent-harness-drawer-open", next);
  if (next) {
    document.body.classList.remove("agent-harness-sidebar-collapsed");
  }
  const scrim = document.querySelector("#agent-harness-scrim");
  if (scrim) {
    /* Push layout — scrim unused on narrow; keep hidden. */
    scrim.hidden = true;
    scrim.setAttribute("aria-hidden", "true");
  }
  const toggle = document.querySelector("#agent-harness-drawer-open");
  toggle?.setAttribute("aria-expanded", next ? "true" : "false");
  /* Do not re-run syncComposerGeometry here — transform push must not
     recompute --agent-column-* or the stream/composer alignment jumps. */
}

function closeHarnessDrawer() {
  setHarnessDrawerOpen(false);
}

function openHarnessDrawer() {
  if (!active || !isNarrowHarness()) {
    return;
  }
  setHarnessDrawerOpen(true);
}

function setSidebarCollapsed(collapsed) {
  if (!active || isNarrowHarness()) {
    document.body.classList.remove("agent-harness-sidebar-collapsed");
    return;
  }
  document.body.classList.toggle("agent-harness-sidebar-collapsed", Boolean(collapsed));
  /* Main width jumps; taskbar size often does not — force column realign. */
  requestAnimationFrame(() => {
    requestAnimationFrame(syncComposerGeometry);
  });
}

function toggleSidebarCollapsed() {
  if (!active) {
    return;
  }
  if (isNarrowHarness()) {
    closeHarnessDrawer();
    return;
  }
  const collapsed = document.body.classList.contains("agent-harness-sidebar-collapsed");
  setSidebarCollapsed(!collapsed);
}





function syncComposerGeometry() {
  const taskbar = document.querySelector(".taskbar");
  const main = document.querySelector(".agent-harness-main");
  if (!taskbar || !main || !document.body.classList.contains("agent-harness-active")) {
    return;
  }
  /* While the push drawer is open, skip — transformed rects would skew
     --agent-column-* and misalign stream vs Shelf composer. */
  if (document.body.classList.contains("agent-harness-drawer-open")) {
    return;
  }
  const dock = taskbar.getBoundingClientRect();
  const band = main.getBoundingClientRect();
  /* Round to device pixels so left/right edges don’t drift by a subpixel. */
  const dpr = window.devicePixelRatio || 1;
  const snap = (n) => Math.round(n * dpr) / dpr;
  const width = snap(dock.width);
  const left = snap(dock.left - band.left);
  const clearance = Math.max(96, Math.round(window.innerHeight - dock.top));
  const root = document.documentElement;
  root.style.setProperty("--harness-composer-clearance", `${clearance}px`);
  root.style.setProperty("--agent-column-width", `${width}px`);
  root.style.setProperty("--agent-column-left", `${left}px`);
}

function observeDockGeometry() {
  const taskbar = document.querySelector(".taskbar");
  const main = document.querySelector(".agent-harness-main");
  if (!taskbar || typeof ResizeObserver !== "function") {
    return;
  }
  if (dockResizeObserver) {
    dockResizeObserver.disconnect();
  }
  dockResizeObserver = new ResizeObserver(() => {
    syncComposerGeometry();
  });
  dockResizeObserver.observe(taskbar);
  /* Sidebar open/close resizes main without changing the dock pill. */
  if (main) {
    dockResizeObserver.observe(main);
  }
}

function stopDockGeometryObserver() {
  dockResizeObserver?.disconnect();
  dockResizeObserver = null;
}

/** Cached at bind — avoid querySelector spam on stream/geometry hot paths. */
let cachedHarnessEl = null;
let cachedStreamColumnEl = null;
let cachedStreamScrollEl = null;

function refreshHarnessDomCache() {
  cachedHarnessEl = document.querySelector("#agent-harness");
  cachedStreamColumnEl = document.querySelector("#agent-harness-stream-column");
  cachedStreamScrollEl = document.querySelector("#agent-harness-stream");
}

function harnessEl() {
  return cachedHarnessEl || document.querySelector("#agent-harness");
}

function streamEl() {
  /* Messages live in the dock-width column so edges match the Shelf composer. */
  return (
    cachedStreamColumnEl ||
    document.querySelector("#agent-harness-stream-column") ||
    cachedStreamScrollEl ||
    document.querySelector("#agent-harness-stream")
  );
}

function streamScrollEl() {
  return cachedStreamScrollEl || document.querySelector("#agent-harness-stream");
}

function streamViewportEl() {
  return document.querySelector(".agent-harness-stream-viewport");
}

function signedInFirstName() {
  const raw = document.querySelector("#toolbar-identity-menu-name")?.textContent?.trim() || "";
  if (!raw) {
    return "";
  }
  const first = raw.split(/\s+/)[0] || "";
  if (first.includes("@")) {
    return first.split("@")[0] || "";
  }
  return first;
}

function clearEmptyState() {
  document.querySelector(".agent-harness-empty")?.remove();
}

/** Pin the transcript to the end after layout settles (markdown/code can grow). */
function scrollStreamToEnd() {
  const scroller = streamScrollEl();
  if (!scroller) {
    return;
  }
  const pin = () => {
    scroller.scrollTop = scroller.scrollHeight;
  };
  pin();
  requestAnimationFrame(() => {
    pin();
    requestAnimationFrame(pin);
  });
}

function titleEl() {
  return document.querySelector("#agent-harness-title");
}

function sessionListEl() {
  return document.querySelector("#agent-harness-session-list");
}

function dropCanvas() {
  return document.querySelector("#agent-home-drop-canvas");
}

export function agentHarnessActive() {
  return active;
}


function stopParticles() {
  if (particleRaf) {
    window.cancelAnimationFrame(particleRaf);
    particleRaf = 0;
  }
  const canvas = dropCanvas();
  if (canvas) {
    const ctx = canvas.getContext("2d");
    ctx?.clearRect(0, 0, canvas.width, canvas.height);
    canvas.hidden = true;
  }
}

















function toggleReasoningVisible() {
  reasoningVisible = setReasoningVisible(!reasoningVisible);
  syncTruthStrip();
  for (const block of document.querySelectorAll(".agent-thinking")) {
    if (!reasoningVisible) {
      block.open = false;
      block.hidden = true;
    } else {
      block.hidden = false;
    }
  }
}

const TOOL_MODE_LABELS = {
  ask: "Ask",
  read: "Read only",
  full: "Full access",
};

function approveMenuEl() {
  return document.getElementById("agent-approve-menu");
}

function approveBtnEl() {
  return document.getElementById("agent-approve-btn");
}

function approveMenuOpen() {
  const menu = approveMenuEl();
  return Boolean(menu && !menu.hidden);
}

function clearFloatingMenuStyle(menu) {
  if (!menu) {
    return;
  }
  menu.style.removeProperty("left");
  menu.style.removeProperty("top");
  menu.style.removeProperty("right");
  menu.style.removeProperty("bottom");
  menu.style.removeProperty("width");
  menu.style.removeProperty("z-index");
  menu.setAttribute("aria-hidden", "true");
}

function positionFloatingMenu(menu, btn, { minWidth = 280, maxWidth = 360, preferRight = false } = {}) {
  if (!menu || !btn || menu.hidden) {
    return;
  }
  /* Menus are on document.body — fixed coords escape dock transform/overflow. */
  if (menu.parentElement !== document.body) {
    document.body.appendChild(menu);
  }
  const rect = btn.getBoundingClientRect();
  const width = Math.min(maxWidth, Math.max(minWidth, window.innerWidth - 24));
  let left = preferRight ? rect.right - width : rect.left;
  left = Math.min(Math.max(12, left), window.innerWidth - width - 12);
  menu.style.zIndex = "200090";
  menu.style.width = `${width}px`;
  menu.style.left = `${left}px`;
  menu.style.right = "auto";
  menu.style.bottom = "auto";
  menu.hidden = false;
  menu.removeAttribute("inert");
  menu.setAttribute("aria-hidden", "false");
  const menuH = menu.getBoundingClientRect().height || 220;
  let top = rect.top - menuH - 10;
  if (top < 12) {
    top = Math.min(window.innerHeight - menuH - 12, rect.bottom + 10);
  }
  menu.style.top = `${Math.max(12, top)}px`;
}

function closeApproveMenu() {
  const menu = approveMenuEl();
  const btn = approveBtnEl();
  if (menu) {
    menu.hidden = true;
    clearFloatingMenuStyle(menu);
  }
  if (btn) {
    btn.setAttribute("aria-expanded", "false");
  }
}

function openApproveMenu() {
  const menu = approveMenuEl();
  const btn = approveBtnEl();
  if (!menu || !btn) {
    return;
  }
  closeModelMenu();
  btn.setAttribute("aria-expanded", "true");
  menu.hidden = false;
  positionFloatingMenu(menu, btn, { minWidth: 300, maxWidth: 360 });
  menu.focus?.({ preventScroll: true });
}

function toggleApproveMenu() {
  if (approveMenuOpen()) {
    closeApproveMenu();
  } else {
    openApproveMenu();
  }
}

function modelMenuEl() {
  return document.getElementById("agent-model-menu");
}

function modelBtnEl() {
  return document.getElementById("agent-model-picker");
}

function modelMenuOpen() {
  const menu = modelMenuEl();
  return Boolean(menu && !menu.hidden);
}

/** Menu can be summoned from the composer trigger or the sidebar Models row. */
let modelMenuAnchor = null;

function closeModelMenu() {
  const menu = modelMenuEl();
  const btn = modelBtnEl();
  if (menu) {
    menu.hidden = true;
    clearFloatingMenuStyle(menu);
  }
  if (btn) {
    btn.setAttribute("aria-expanded", "false");
  }
  modelMenuAnchor = null;
}

function syncModelTrigger() {
  const btn = modelBtnEl();
  const selected = getSelectedModel();
  if (!btn || !selected) {
    return;
  }
  const name = btn.querySelector(".agent-model-name");
  const tier = btn.querySelector(".agent-model-tier");
  const liveInference = getLiveInferenceState();
  if (name) {
    /* w1: when llama-server is live, the trigger shows the real model. */
    name.textContent = liveInference.live && liveInference.model
      ? liveInference.model
      : selected.tier === "preview"
        ? "Local preview"
        : selected.label.replace(/\s*·\s*/g, " ");
  }
  if (tier) {
    /* Keep the trigger quiet — one label, no "mock" suffix in the chrome. */
    tier.textContent = "";
    tier.hidden = true;
  }
  btn.title = liveInference.live && liveInference.model
    ? `${liveInference.model} — live local model via llama-server on this machine`
    : selected.tier === "preview"
      ? `${selected.label} — preview path, not a downloaded weight file`
      : `${selected.label} — preview stub (Get is in-session only until runtime downloads)`;
}

function renderModelHero(host) {
  const pick = recommendedModel();
  if (!pick) {
    host.hidden = true;
    return;
  }
  host.hidden = false;
  host.replaceChildren();
  const selectedId = getSelectedModel()?.id;
  const isSelected = pick.status === "installed" && pick.id === selectedId;
  const card = document.createElement("div");
  card.className = "agent-model-hero-card";
  card.innerHTML =
    `<span class="agent-model-option-copy">` +
    `<span class="agent-model-hero-kicker">Best for this device</span>` +
    `<span class="agent-model-option-title"></span>` +
    `<span class="agent-model-option-desc"></span>` +
    `</span>` +
    `<button type="button" class="agent-model-download-btn agent-model-hero-btn"></button>`;
  card.querySelector(".agent-model-option-title").textContent = pick.label;
  card.querySelector(".agent-model-option-desc").textContent =
    [pick.sizeLabel, pick.detail].filter(Boolean).join(" · ");
  const action = card.querySelector(".agent-model-hero-btn");
  if (isSelected) {
    action.textContent = "In use";
    action.disabled = true;
  } else if (pick.status === "installed") {
    action.textContent = "Use";
    action.dataset.modelUse = pick.id;
  } else {
    action.textContent = "Get";
    action.dataset.modelDownload = pick.id;
    action.title = "Preview install only — no real download yet";
  }
  host.append(card);
}

function buildInstalledModelRows(host, emptyText) {
  const selectedId = getSelectedModel()?.id;
  host.replaceChildren();
  /* w1: real llama-server models first (one configured GGUF today) — the only
     rows that are weights on disk; preview stubs stay labeled below. */
  const liveInference = getLiveInferenceState();
  if (liveInference.live) {
    for (const model of liveInference.models) {
      const row = document.createElement("button");
      row.type = "button";
      row.className = "agent-model-option is-live";
      row.setAttribute("role", "option");
      row.setAttribute("aria-selected", "true");
      row.disabled = true;
      row.innerHTML =
        `<span class="agent-model-option-copy">` +
        `<span class="agent-model-option-title"></span>` +
        `<span class="agent-model-option-desc"></span>` +
        `</span>` +
        `<span class="agent-approve-option-check" aria-hidden="true"></span>`;
      row.querySelector(".agent-model-option-title").textContent = model.label;
      row.querySelector(".agent-model-option-desc").textContent =
        model.detail || "Live · llama-server on this machine";
      host.append(row);
    }
  }
  for (const model of listInstalledModels()) {
    const btn = document.createElement("button");
    btn.type = "button";
    btn.className = "agent-model-option";
    btn.setAttribute("role", "option");
    btn.dataset.modelId = model.id;
    btn.setAttribute("aria-selected", model.id === selectedId ? "true" : "false");
    if (model.id === selectedId) {
      btn.classList.add("is-active");
    }
    btn.innerHTML =
      `<span class="agent-model-option-copy">` +
      `<span class="agent-model-option-title"></span>` +
      `<span class="agent-model-option-desc"></span>` +
      `</span>` +
      `<span class="agent-model-option-meta"></span>` +
      `<span class="agent-approve-option-check" aria-hidden="true"></span>`;
    btn.querySelector(".agent-model-option-title").textContent = model.label;
    btn.querySelector(".agent-model-option-desc").textContent =
      model.tier === "preview"
        ? "Preview path · not weights on disk"
        : "Preview stub · Get was in-session only (not a real download)";
    const meta = btn.querySelector(".agent-model-option-meta");
    if (model.sizeLabel) {
      meta.textContent = model.sizeLabel;
    } else {
      meta.remove();
    }
    host.append(btn);
  }
  if (!host.children.length) {
    const empty = document.createElement("p");
    empty.className = "agent-model-menu-empty";
    empty.textContent = emptyText;
    host.append(empty);
  }
}

function buildDiscoverModelRows(host, emptyText) {
  host.replaceChildren();
  for (const model of listRecommendedModels()) {
    const row = document.createElement("div");
    row.className = "agent-model-download-row";
    row.dataset.modelId = model.id;
    const blocked = fitForModel(model) === "blocked";
    row.innerHTML =
      `<span class="agent-model-option-copy">` +
      `<span class="agent-model-option-title"></span>` +
      `<span class="agent-model-option-desc"></span>` +
      `</span>` +
      `<button type="button" class="agent-model-download-btn"></button>`;
    row.querySelector(".agent-model-option-title").textContent = model.label;
    row.querySelector(".agent-model-option-desc").textContent =
      [model.sizeLabel, model.detail].filter(Boolean).join(" · ");
    const action = row.querySelector(".agent-model-download-btn");
    action.dataset.modelDownload = model.id;
    if (blocked) {
      action.textContent = "Needs Spark";
      action.disabled = true;
      action.title = "Too large for this device's estimated memory";
    } else {
      action.textContent = "Get";
      action.title = "Preview install only — no real download yet";
    }
    host.append(row);
  }
  if (!host.children.length) {
    const empty = document.createElement("p");
    empty.className = "agent-model-menu-empty";
    empty.textContent = emptyText;
    host.append(empty);
  }
}

/* Cursor-style quick switcher: just what's installed + "Add models",
   which opens the full Models page. Browsing/downloading lives there. */
function renderModelMenu() {
  const listHost = document.querySelector("[data-model-menu-list]");
  if (!listHost) {
    return;
  }
  buildInstalledModelRows(listHost, "No preview models yet — see Add models.");
}





function openModelMenu(anchor = null) {
  const menu = modelMenuEl();
  const btn = modelBtnEl();
  const at = anchor || btn;
  if (!menu || !at) {
    return;
  }
  closeApproveMenu();
  renderModelMenu();
  btn?.setAttribute("aria-expanded", "true");
  modelMenuAnchor = at;
  menu.hidden = false;
  positionFloatingMenu(menu, at, { minWidth: 230, maxWidth: 280, preferRight: at === btn });
  menu.focus?.({ preventScroll: true });
}

function toggleModelMenu() {
  if (modelMenuOpen()) {
    closeModelMenu();
  } else {
    openModelMenu();
  }
}

function selectAgentModel(modelId) {
  setSelectedModelId(modelId);
  syncTruthStrip();
  syncWorkbenchPanels();
  closeModelMenu();
}

function repositionModelMenu() {
  const menu = modelMenuEl();
  const anchor = modelMenuAnchor || modelBtnEl();
  if (menu && anchor && !menu.hidden) {
    positionFloatingMenu(menu, anchor, {
      minWidth: 230,
      maxWidth: 280,
      preferRight: anchor === modelBtnEl(),
    });
  }
}

function previewGetModel(modelId) {
  const req = requestModelGet(modelId);
  if (req.kind === "blocked") {
    window.alert(req.reason || "This model does not fit this device estimate.");
    return;
  }
  if (req.kind === "already") {
    selectAgentModel(modelId);
    return;
  }
  if (req.kind !== "preview-theatre") {
    return;
  }
  /* Preview theatre — not a Carrier grant. Real Get asks Inbox later. */
  const result = mockInstallModel(modelId);
  if (!result.ok) {
    return;
  }
  syncTruthStrip();
  syncWorkbenchPanels();
  renderModelMenu();
  repositionModelMenu();
}

function syncApproveTrigger() {
  const btn = approveBtnEl();
  if (!btn) {
    return;
  }
  const label = btn.querySelector("[data-approve-label]");
  if (label) {
    label.textContent = TOOL_MODE_LABELS[toolMode] || TOOL_MODE_LABELS.ask;
  }
  btn.dataset.toolMode = toolMode;
  btn.classList.toggle("is-danger", toolMode === "full");
  const mark = btn.querySelector("[data-approve-mark]");
  if (mark) {
    mark.dataset.icon = toolMode;
  }
  for (const opt of document.querySelectorAll(".agent-approve-option[data-tool-mode]")) {
    const on = opt.dataset.toolMode === toolMode;
    opt.classList.toggle("is-active", on);
    opt.setAttribute("aria-selected", on ? "true" : "false");
  }
}

function syncSessionModeUi() {
  const harness = harnessEl();
  harness?.setAttribute("data-session-mode", sessionMode);
  document.body.dataset.agentSessionMode = sessionMode;
  document.body.dataset.agentToolMode = toolMode;

  for (const btn of document.querySelectorAll("[data-segment]")) {
    const on = btn.dataset.segment === sessionMode;
    btn.classList.toggle("is-active", on);
    btn.setAttribute("aria-pressed", on ? "true" : "false");
  }
  syncApproveTrigger();
  const title = document.querySelector("[data-workbench-title]");
  if (title) {
    title.textContent = sessionMode === "build" ? "Workbench · Build" : "Workbench";
  }
  for (const tab of document.querySelectorAll("[data-build-only]")) {
    tab.hidden = sessionMode !== "build";
  }
  if (sessionMode === "build") {
    if (workbenchTab === "outputs" || workbenchTab === "browser" || workbenchTab === "terminal") {
      setWorkbenchTab("plan");
    } else {
      setWorkbenchTab(workbenchTab);
    }
    openWorkbench({ tab: workbenchTab, force: true });
  } else if (sessionMode === "chat" && workbenchTab === "diff") {
    setWorkbenchTab("outputs");
    syncWorkbenchOpenUi();
  } else {
    setWorkbenchTab(workbenchTab);
    syncWorkbenchOpenUi();
  }
  syncWorkbenchPanels();
}

function setSessionMode(mode) {
  sessionMode = mode === "build" ? "build" : "chat";
  if (sessionMode === "build" && toolMode === "read") {
    toolMode = "ask";
  }
  if (sessionMode === "chat" && toolMode === "full") {
    toolMode = "ask";
  }
  syncSessionModeUi();
  const stream = streamEl();
  if (stream && !stream.querySelector(".agent-msg, .agent-grant-card, .agent-thinking")) {
    showEmptyState();
  }
}

function setToolMode(mode) {
  if (mode !== "read" && mode !== "ask" && mode !== "full") {
    return;
  }
  toolMode = mode;
  syncSessionModeUi();
  closeApproveMenu();
}







function runParticleDrop(durationMs) {
  if (prefersReducedMotion()) {
    return;
  }
  const canvas = dropCanvas();
  if (!canvas) {
    return;
  }
  stopParticles();
  const dpr = Math.min(window.devicePixelRatio || 1, 2);
  const w = window.innerWidth;
  const h = window.innerHeight;
  canvas.width = Math.floor(w * dpr);
  canvas.height = Math.floor(h * dpr);
  canvas.style.width = `${w}px`;
  canvas.style.height = `${h}px`;
  canvas.hidden = false;
  const ctx = canvas.getContext("2d");
  if (!ctx) {
    return;
  }
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);

  /* Soft mist — drifts and dissolves; not a shatter fall. */
  const colors = ["#9aa3b2", "#c9d0dc", "#6e7684", "#dde3ec"];
  const particles = Array.from({ length: PARTICLE_COUNT }, () => ({
    x: Math.random() * w,
    y: Math.random() * h,
    vx: (Math.random() - 0.5) * 0.35,
    vy: -0.15 - Math.random() * 0.45,
    size: 0.8 + Math.random() * 1.8,
    alpha: 0.08 + Math.random() * 0.18,
    color: colors[(Math.random() * colors.length) | 0],
  }));

  const started = performance.now();
  const tick = (now) => {
    const t = Math.min(1, (now - started) / durationMs);
    const breathe = Math.sin(t * Math.PI);
    ctx.clearRect(0, 0, w, h);
    for (const p of particles) {
      p.x += p.vx;
      p.y += p.vy;
      ctx.globalAlpha = p.alpha * breathe * (1 - t * 0.35);
      ctx.fillStyle = p.color;
      ctx.beginPath();
      ctx.arc(p.x, p.y, p.size, 0, Math.PI * 2);
      ctx.fill();
    }
    ctx.globalAlpha = 1;
    if (t < 1 && document.visibilityState !== "hidden") {
      particleRaf = window.requestAnimationFrame(tick);
      return;
    }
    stopParticles();
  };
  particleRaf = window.requestAnimationFrame(tick);
}



/** Track A honesty: live · preview · offline — the live flag flips only on a
 *  real probe result (agent-live.js), never assumed (§AL.3). */
function syncAgentInferenceStatus() {
  const el = document.querySelector("[data-agent-inference-status]");
  if (!el) {
    return;
  }
  const offline = typeof navigator !== "undefined" && navigator.onLine === false;
  const state = getLiveInferenceState();
  const live = state.live === true;
  if (live) {
    el.hidden = false;
    el.dataset.state = "live";
    el.textContent = state.model
      ? `Live · local model on this machine — ${state.model}`
      : "Live · local model on this machine";
    document.body.dataset.agentInference = "live";
    return;
  }
  if (offline) {
    el.hidden = false;
    el.dataset.state = "offline";
    el.textContent =
      "Offline · preview path only — no live model on this machine yet";
    document.body.dataset.agentInference = "offline-preview";
    return;
  }
  el.hidden = false;
  el.dataset.state = "preview";
  el.textContent =
    "Preview · not live inference — mock replies until a local model is wired";
  document.body.dataset.agentInference = "preview";
}

export function showAgentHarness({
  prompt,
  fromShelf = false,
  syncStage = true,
  restore = false,
} = {}) {
  const harness = harnessEl();
  if (!harness) {
    return;
  }

  /* Already open (e.g. Agent button while harness visible) — keep room, optional send. */
  if (active && !prompt) {
    harness.classList.add("is-visible");
    if (document.body.classList.contains("agent-harness-settled")) {
      enableHarnessMenubarReveal();
    }
    if (syncStage && !isAgentSpace(getActiveStageId())) {
      setActiveStage(agentStageId(), {
        announce: false,
        focus: false,
        animate: false,
        syncHarness: false,
      });
    }
    syncComposerGeometry();
    return;
  }

  if (prompt) {
    const session = ensureSessionForPrompt(prompt);
    session.messages.push({ role: "user", text: prompt });
    if (maybeUpdatePlanFromPrompt(prompt)) {
      openWorkbench({ tab: "plan" });
    }
  } else if (restore || workspaceHydrated) {
    /* Refresh / host restore — keep persisted chats; don't mint a blank New chat. */
    if (!activeSessionId) {
      activeSessionId = sessions[0]?.id || null;
    }
  } else if (fromShelf) {
    /* Entering with the Shelf morph — land on a clean New chat so the room is visible. */
    const fresh = {
      id: `s-${Date.now()}`,
      title: "New chat",
      group: "Today",
      messages: [],
    };
    sessions = [fresh, ...sessions.filter((s) => s.title !== "New chat" || s.messages.length > 0)];
    activeSessionId = fresh.id;
  } else if (!activeSessionId) {
    activeSessionId = sessions[0]?.id || null;
  }

  const motionGen = (harnessMotionGen += 1);
  active = true;
  closeHarnessDrawer();
  clearHarnessMenubarReveal();
  document.body.classList.add("agent-harness-active");
  setHarnessChromeInert(true);
  if (!prefersReducedMotion()) {
    document.body.classList.add("agent-harness-dropping");
  }

  /* Space id tracks the dance; morph is owned by Shelf (avoid a second enter). */
  if (syncStage && !isAgentSpace(getActiveStageId())) {
    setActiveStage(agentStageId(), {
      announce: false,
      focus: false,
      animate: false,
      syncHarness: false,
    });
  }

  /*
    Space switches should enter via showAgentShelfFace (morph). If we still land
    here without a face (e.g. Send), settle the composer face so the dock is never empty.
  */
  if (!fromShelf) {
    if (!agentShelfFaceActive()) {
      snapAgentShelfFace();
    }
    syncComposerGeometry();
  }

  harness.hidden = false;
  harness.setAttribute("aria-hidden", "false");
  syncTruthStrip();
  syncSessionModeUi();
  syncWorkbenchOpenUi();
  syncAgentInferenceStatus();
  void probeLiveInference().then(() => {
    if (active) {
      syncAgentInferenceStatus();
      syncModelTrigger();
    }
  });
  renderSessions();
  renderActiveSession();

  /* Paint harness next frames — never during Shelf FLIP. */
  requestAnimationFrame(() => {
    if (motionGen !== harnessMotionGen || !active) {
      return;
    }
    harness.classList.add("is-visible");
    observeDockGeometry();
    syncComposerGeometry();
    if (!prefersReducedMotion()) {
      runParticleDrop(HOME_BREATHE_MS);
    }
    requestAnimationFrame(syncComposerGeometry);
  });

  if (prompt) {
    const openPrompt = String(prompt).trim();
    window.setTimeout(() => {
      if (motionGen !== harnessMotionGen || !active) {
        return;
      }
      startTurnForPrompt(openPrompt);
    }, prefersReducedMotion() ? 40 : HARNESS_CONTENT_AT_MS);
  }

  scheduleHarnessSettled(motionGen);

  if (!fromShelf) {
    shelfComposerInput()?.focus({ preventScroll: true });
  }
}

/** Generation-safe settle after Home breathe — menubar edge-reveal only then. */
function scheduleHarnessSettled(motionGen) {
  window.setTimeout(() => {
    if (motionGen !== harnessMotionGen || !active) {
      return;
    }
    document.body.classList.remove("agent-harness-dropping");
    document.body.classList.add("agent-harness-settled");
    enableHarnessMenubarReveal();
    syncComposerGeometry();
  }, prefersReducedMotion() ? 40 : HOME_BREATHE_MS);
}

/** Finish leave: clear room chrome; optional Shelf reverse morph. */
function teardownHarnessDom(motionGen, { restoreShelfApps = true } = {}) {
  if (motionGen !== harnessMotionGen) {
    return;
  }
  const harness = harnessEl();
  stopDockGeometryObserver();
  clearHarnessMenubarReveal();
  setHarnessChromeInert(false);
  document.body.classList.remove("agent-harness-active", "agent-harness-rising");
  /* Column CSS vars stay until Shelf morph finishes (shelf clears them). */
  if (harness) {
    harness.hidden = true;
    harness.setAttribute("aria-hidden", "true");
  }
  if (restoreShelfApps) {
    /* Reverse morph back to Home Shelf — same dance as Dock Agent leave. */
    if (agentShelfFaceActive()) {
      hideAgentShelfFace();
    } else {
      snapAppsShelfFace();
    }
  }
}

function scheduleHarnessTeardown(motionGen, opts) {
  if (prefersReducedMotion()) {
    teardownHarnessDom(motionGen, opts);
    return;
  }
  window.setTimeout(() => teardownHarnessDom(motionGen, opts), HOME_RISE_MS);
}

export function hideAgentHarness({ restoreShelfApps = true, syncStage = true } = {}) {
  /* Persist Desktop even if the room is already torn down — Home/Esc used to
     leave active_stage stuck on "agent", so refresh re-opened Agent. */
  if (syncStage && isAgentSpace(getActiveStageId())) {
    setActiveStage(desktopStageId(), {
      announce: false,
      focus: false,
      animate: false,
      syncHarness: false,
    });
  }
  if (!active && !document.body.classList.contains("agent-harness-active")) {
    return;
  }
  const motionGen = (harnessMotionGen += 1);
  stopMockStream({ keepPartial: true });
  stopParticles();
  active = false;
  closeHarnessDrawer();
  closeSessionSearch();
  closeApproveMenu();
  closeModelMenu();
  closeHarnessPage();
  document.body.classList.remove("agent-harness-sidebar-collapsed");
  resetMockCapabilities();
  syncTruthStrip();

  const harness = harnessEl();
  harness?.classList.remove("is-visible");
  clearHarnessMenubarReveal();
  document.body.classList.remove("agent-harness-settled", "agent-harness-dropping");
  document.body.classList.add("agent-harness-rising");

  scheduleHarnessTeardown(motionGen, { restoreShelfApps });
}
/* resetMockCapabilities on leave — session grant messages still hold preview state;
   hydrateCapabilitiesFromSession re-binds when a session is painted again. */

export function stopAgentHarnessStream() {
  stopMockStream({ keepPartial: true });
}

export function sendToAgentHarness(prompt) {
  const text = String(prompt || "").trim();
  if (!text) {
    if (active) {
      stopMockStream({ keepPartial: true });
      turnBusy = false;
    }
    return;
  }
  if (active) {
    /* While a turn streams, queue follow-ups instead of cutting the answer. */
    if (turnBusy) {
      enqueueFollowUp(text);
      return;
    }
    closeHarnessPage();
    const session = ensureSessionForPrompt(text);
    if (session.title === "New chat" || session.messages.length === 0) {
      session.title = titleFromPrompt(text);
    }
    session.messages.push({ role: "user", text });
    touchSession(session);
    renderSessions();
    setTitle(session.title);
    clearEmptyState();
    appendMessage("user", text);
    if (maybeUpdatePlanFromPrompt(text)) {
      openWorkbench({ tab: "plan" });
    } else {
      syncWorkbenchPanels();
    }
    startTurnForPrompt(text);
    return;
  }
  showAgentHarness({ prompt: text });
}






let sessionActionsId = null;






/** When creating a project from a chat’s menu, file that chat into it. */
let pendingProjectAssignSessionId = null;

export function bindAgentHarness() {
  if (bound) {
    return;
  }
  bound = true;
  refreshHarnessDomCache();
  registerAgentHarnessApi({
    agentHarnessActive,
    showAgentHarness,
    hideAgentHarness,
    sendToAgentHarness,
    stopAgentHarnessStream,
  });
  bindAgentWorkspaceSnapshot(getAgentWorkspaceSnapshot);
  window.addEventListener("online", () => {
    syncAgentInferenceStatus();
    void probeLiveInference({ force: true }).then(() => {
      if (active) {
        syncAgentInferenceStatus();
        syncModelTrigger();
      }
    });
  });
  window.addEventListener("offline", () => {
    if (active) {
      syncAgentInferenceStatus();
    }
  });

  /* w1: real probe — live flips only on llama-server truth (§AL.3). */
  void probeLiveInference().then(() => {
    if (active) {
      syncAgentInferenceStatus();
      syncModelTrigger();
    }
  });

  /* Warm the hardware estimate so Settings / model menu stay honest. */
  void probeHardwareEstimate().then(() => {
    syncTruthStrip();
    syncWorkbenchPanels();
    if (harnessPageOpen()) {
      renderHarnessPage();
    }
    if (modelMenuOpen()) {
      renderModelMenu();
    }
  });

  /* Esc: search (90) → drawer (85) → Shelf reverse dance (75). */
  registerEscapeHandler("agent-session-search", {
    priority: 90,
    isActive: () => active && sessionSearchOpen(),
    dismiss: () => closeSessionSearch(),
  });
  registerEscapeHandler("agent-harness-drawer", {
    priority: 85,
    isActive: () =>
      active && document.body.classList.contains("agent-harness-drawer-open"),
    dismiss: () => closeHarnessDrawer(),
  });
  registerEscapeHandler("agent-approve-menu", {
    priority: 88,
    isActive: () => approveMenuOpen(),
    dismiss: () => closeApproveMenu(),
  });
  registerEscapeHandler("agent-model-menu", {
    priority: 87,
    isActive: () => modelMenuOpen(),
    dismiss: () => closeModelMenu(),
  });
  registerEscapeHandler("agent-harness-page", {
    priority: 84,
    isActive: () => active && harnessPageOpen(),
    dismiss: () => closeHarnessPage(),
  });
  registerEscapeHandler("agent-session-actions", {
    priority: 89,
    isActive: () => sessionActionsOpen(),
    dismiss: () => closeSessionActions(),
  });
  registerEscapeHandler("agent-workbench", {
    priority: 83,
    isActive: () => active && workbenchOpen && !harnessPageOpen(),
    dismiss: () => closeWorkbench(),
  });

  window.addEventListener("resize", () => {
    if (approveMenuOpen()) {
      positionFloatingMenu(approveMenuEl(), approveBtnEl(), {
        minWidth: 300,
        maxWidth: 360,
      });
    }
    if (modelMenuOpen()) {
      const anchor = modelMenuAnchor || modelBtnEl();
      positionFloatingMenu(modelMenuEl(), anchor, {
        minWidth: 230,
        maxWidth: 280,
        preferRight: anchor === modelBtnEl(),
      });
    }
  });

  if (typeof window.matchMedia === "function") {
    const narrowMq = window.matchMedia(HARNESS_NARROW_MQ);
    const onNarrowChange = () => {
      closeHarnessDrawer();
      closeSessionSearch();
      document.body.classList.remove("agent-harness-sidebar-collapsed");
      if (active) {
        syncComposerGeometry();
      }
      syncSpacePager();
    };
    if (typeof narrowMq.addEventListener === "function") {
      narrowMq.addEventListener("change", onNarrowChange);
    } else if (typeof narrowMq.addListener === "function") {
      narrowMq.addListener(onNarrowChange);
    }
  }

  document.addEventListener("input", (event) => {
    if (event.target?.matches?.("[data-plan-markdown]")) {
      setPlanMarkdown(event.target.value);
      return;
    }
    if (event.target?.id === "agent-session-search-input") {
      renderSessionSearchResults(event.target.value);
    }
  });

  /* Capture-phase: opaque Home frames must never navigate on form submit
     (default GET reloads the sandbox and wipes in-memory projects). */
  document.addEventListener(
    "submit",
    (event) => {
      const form = event.target?.closest?.("[data-project-create]") || event.target;
      if (form?.matches?.("[data-project-create]") || form?.closest?.("[data-project-create]")) {
        event.preventDefault();
        event.stopPropagation();
        submitProjectCreate();
      }
    },
    true
  );
  document.addEventListener(
    "keydown",
    (event) => {
      const inCreate = event.target?.closest?.("[data-project-create]");
      if (!inCreate) {
        return;
      }
      const form = projectCreateEl();
      if (!form || form.hidden) {
        return;
      }
      if (event.key === "Enter" && !event.isComposing) {
        event.preventDefault();
        event.stopPropagation();
        submitProjectCreate();
        return;
      }
      if (event.key === "Escape") {
        event.preventDefault();
        event.stopPropagation();
        closeProjectCreate();
      }
    },
    true
  );

  document.addEventListener("click", (event) => {
    if (
      event.target.closest?.("#agent-harness-search-open") ||
      event.target.closest?.("#agent-harness-search-open-main")
    ) {
      event.preventDefault();
      openSessionSearch();
      return;
    }
    if (
      event.target.closest?.("#agent-session-search-close") ||
      (event.target.id === "agent-session-search" &&
        !event.target.closest?.(".agent-session-search-panel"))
    ) {
      event.preventDefault();
      closeSessionSearch();
      return;
    }
    const searchRow = event.target.closest?.(".agent-session-search-row[data-session-id]");
    if (searchRow) {
      event.preventDefault();
      const id = searchRow.dataset.sessionId;
      closeSessionSearch();
      if (id) {
        selectSession(id);
        if (isNarrowHarness()) {
          closeHarnessDrawer();
        }
      }
      return;
    }
    if (event.target.closest?.("#agent-harness-panel-toggle")) {
      event.preventDefault();
      toggleSidebarCollapsed();
      return;
    }
    if (event.target.closest?.("#agent-harness-drawer-open")) {
      event.preventDefault();
      if (isNarrowHarness()) {
        if (document.body.classList.contains("agent-harness-drawer-open")) {
          closeHarnessDrawer();
        } else {
          openHarnessDrawer();
        }
      } else {
        setSidebarCollapsed(false);
      }
      return;
    }
    /* Push mode: tap nudged main (not sidebar) to close drawer. */
    if (
      document.body.classList.contains("agent-harness-drawer-open") &&
      event.target.closest?.(".agent-harness-main") &&
      !event.target.closest?.("#agent-harness-drawer-open")
    ) {
      closeHarnessDrawer();
      return;
    }
    if (event.target.closest?.("#agent-harness-scrim")) {
      event.preventDefault();
      closeHarnessDrawer();
      return;
    }
    if (event.target.closest?.("#agent-harness-home")) {
      event.preventDefault();
      closeHarnessDrawer();
      closeSessionSearch();
      hideAgentShelfFace();
      return;
    }
    if (event.target.closest?.("#agent-harness-new-chat")) {
      event.preventDefault();
      newChat();
      if (isNarrowHarness()) {
        closeHarnessDrawer();
      }
      return;
    }
    const copyCode = event.target.closest?.(".agent-md-copy");
    if (copyCode) {
      event.preventDefault();
      const code = copyCode.closest(".agent-md-code")?.querySelector("code")?.textContent || "";
      navigator.clipboard?.writeText(code).catch(() => {});
      copyCode.textContent = "Copied";
      window.setTimeout(() => {
        copyCode.textContent = "Copy";
      }, 1200);
      return;
    }
    const copyMsg = event.target.closest?.("[data-copy-message]");
    if (copyMsg) {
      event.preventDefault();
      const body = copyMsg.closest(".agent-msg")?.querySelector(".agent-msg-body");
      const text = body?.innerText || body?.textContent || "";
      navigator.clipboard?.writeText(text).catch(() => {});
      copyMsg.textContent = "Copied";
      window.setTimeout(() => {
        copyMsg.textContent = "Copy";
      }, 1200);
      return;
    }
    if (event.target.closest?.("[data-retry]") || event.target.closest?.("[data-regenerate]")) {
      event.preventDefault();
      event.target.closest(".agent-msg-stopped")?.remove();
      if (turnBusy) {
        return;
      }
      const session = sessions.find((s) => s.id === activeSessionId);
      const lastUser = [...(session?.messages || [])]
        .reverse()
        .find((m) => m.role === "user");
      startTurnForPrompt(lastUser?.text || "");
      return;
    }
    const thinkToggle = event.target.closest?.("[data-truth-thinking-toggle]");
    if (thinkToggle) {
      event.preventDefault();
      toggleReasoningVisible();
      return;
    }
    const segment = event.target.closest?.("[data-segment]");
    if (segment?.dataset.segment) {
      event.preventDefault();
      setSessionMode(segment.dataset.segment);
      return;
    }
    const approveBtn = event.target.closest?.("#agent-approve-btn");
    if (approveBtn) {
      event.preventDefault();
      toggleApproveMenu();
      return;
    }
    const approveOpt = event.target.closest?.(".agent-approve-option[data-tool-mode]");
    if (approveOpt?.dataset.toolMode) {
      event.preventDefault();
      setToolMode(approveOpt.dataset.toolMode);
      return;
    }
    const modelBtn = event.target.closest?.("#agent-model-picker");
    if (modelBtn) {
      event.preventDefault();
      toggleModelMenu();
      return;
    }
    if (event.target.closest?.("[data-model-add]")) {
      event.preventDefault();
      closeModelMenu();
      openHarnessPage("configure", { section: "models" });
      return;
    }
    const sidebarNav = event.target.closest?.("[data-sidebar-nav]");
    if (sidebarNav?.dataset.sidebarNav) {
      event.preventDefault();
      /* Sidebar rows open center pages — tap again to return to the chat.
         Use DOM active state (harnessPage lives in agent-configure.js). */
      const dest = sidebarNav.dataset.sidebarNav;
      if (sidebarNav.classList.contains("is-active")) {
        closeHarnessPage();
      } else {
        openHarnessPage(dest, dest === "configure" ? { section: "overview" } : {});
      }
      return;
    }
    const configureChip = event.target.closest?.("[data-configure-section]");
    if (configureChip?.dataset.configureSection) {
      event.preventDefault();
      openConfigureSection(configureChip.dataset.configureSection);
      return;
    }
    const overviewRow = event.target.closest?.("[data-open-configure-section]");
    if (overviewRow?.dataset.openConfigureSection) {
      event.preventDefault();
      openConfigureSection(overviewRow.dataset.openConfigureSection);
      return;
    }
    if (event.target.closest?.("[data-project-add]")) {
      event.preventDefault();
      openProjectCreate();
      return;
    }
    if (event.target.closest?.("[data-project-create-cancel]")) {
      event.preventDefault();
      closeProjectCreate();
      return;
    }
    /* Click away: save if named (so “save” isn’t a silent cancel), else dismiss. */
    const createOpen = projectCreateEl() && !projectCreateEl().hidden;
    if (
      createOpen &&
      !event.target.closest?.("[data-project-create]") &&
      !event.target.closest?.("[data-project-add]")
    ) {
      const pending = String(
        document.querySelector("[data-project-create-input]")?.value || ""
      ).trim();
      if (pending) {
        submitProjectCreate();
      } else {
        closeProjectCreate();
      }
    }
    const projectRemove = event.target.closest?.("[data-project-remove]");
    if (projectRemove?.dataset.projectRemove) {
      event.preventDefault();
      const id = projectRemove.dataset.projectRemove;
      removeProject(id);
      for (const session of sessions) {
        if (session.projectId === id) {
          session.projectId = null;
        }
      }
      renderSessions();
      persistAgentWorkspaceSoon();
      return;
    }
    const projectToggle = event.target.closest?.("[data-project-toggle]");
    if (projectToggle?.dataset.projectToggle) {
      event.preventDefault();
      const item = projectToggle.closest(".agent-projects-item");
      const nest = item?.querySelector(".agent-projects-sessions");
      if (nest) {
        const open = nest.hidden;
        nest.hidden = !open;
        projectToggle.setAttribute("aria-expanded", open ? "true" : "false");
      }
      return;
    }
    if (event.target.closest?.("[data-page-close]")) {
      event.preventDefault();
      closeHarnessPage();
      return;
    }
    const modelUse = event.target.closest?.("[data-model-use]");
    if (modelUse?.dataset.modelUse) {
      event.preventDefault();
      selectAgentModel(modelUse.dataset.modelUse);
      return;
    }
    const modelOpt = event.target.closest?.(".agent-model-option[data-model-id]");
    if (modelOpt?.dataset.modelId) {
      event.preventDefault();
      selectAgentModel(modelOpt.dataset.modelId);
      return;
    }
    const modelGet = event.target.closest?.("[data-model-download]");
    if (modelGet?.dataset.modelDownload && !modelGet.disabled) {
      event.preventDefault();
      previewGetModel(modelGet.dataset.modelDownload);
      return;
    }
    if (
      approveMenuOpen() &&
      !event.target.closest?.(".agent-approve-wrap") &&
      !event.target.closest?.("#agent-approve-menu")
    ) {
      closeApproveMenu();
    }
    if (
      modelMenuOpen() &&
      !event.target.closest?.(".agent-model-wrap") &&
      !event.target.closest?.("#agent-model-menu")
    ) {
      closeModelMenu();
    }
    if (event.target.closest?.("[data-workbench-open]")) {
      event.preventDefault();
      openWorkbench({ force: true });
      return;
    }
    if (event.target.closest?.("[data-workbench-close]")) {
      event.preventDefault();
      closeWorkbench();
      return;
    }
    const wbTab = event.target.closest?.("[data-workbench-tab]");
    if (wbTab?.dataset.workbenchTab) {
      event.preventDefault();
      openWorkbench({ tab: wbTab.dataset.workbenchTab, force: true });
      return;
    }
    if (event.target.closest?.("[data-tools-demo-grant]")) {
      event.preventDefault();
      const grant = {
        toolId: "library.read",
        state: "pending",
        args: { path: "Downloads" },
      };
      const session = sessions.find((s) => s.id === activeSessionId);
      if (session) {
        session.messages.push(grant);
      }
      appendGrantCard(grant);
      syncTruthStrip();
      syncWorkbenchPanels();
      return;
    }
    const lib = event.target.closest?.("[data-library-attach]");
    if (lib?.dataset.libraryAttach) {
      event.preventDefault();
      openWorkbench({ tab: "library", force: true });
      const noun = lib.dataset.libraryAttach;
      const input = shelfComposerInput();
      if (input) {
        const chip = `@${noun}`;
        input.value = input.value ? `${input.value.trim()} ${chip}` : chip;
        input.dispatchEvent(new Event("input", { bubbles: true }));
        syncAgentSendButton();
        input.focus({ preventScroll: true });
      }
      return;
    }
    const out = event.target.closest?.("[data-output-id]");
    if (out?.dataset.outputId) {
      event.preventDefault();
      openWorkbench({ tab: "outputs", force: true });
      window.alert(
        `Preview artifact: ${out.dataset.outputId}\n\nMock only — not written to disk. ADE sandbox later.`,
      );
      return;
    }
    const queueRemove = event.target.closest?.(".agent-queue-item-remove");
    if (queueRemove?.dataset.queueId) {
      event.preventDefault();
      followUpQueue = followUpQueue.filter((q) => q.id !== queueRemove.dataset.queueId);
      renderFollowUpQueue();
      return;
    }
    const grantBtn = event.target.closest?.("[data-grant-decision]");
    if (grantBtn) {
      event.preventDefault();
      const card = grantBtn.closest(".agent-grant-card");
      const decision = grantBtn.dataset.grantDecision;
      if (card && (decision === "deny" || decision === "allow_once")) {
        resolveGrantFromCard(card, decision);
      }
      return;
    }
    const sessionBtn = event.target.closest?.(".agent-harness-session-btn");
    if (sessionBtn) {
      event.preventDefault();
      stopMockStream({ keepPartial: true });
      activeSessionId = sessionBtn.closest(".agent-harness-session")?.dataset.sessionId || null;
      renderSessions();
      renderActiveSession();
      if (isNarrowHarness()) {
        closeHarnessDrawer();
      }
      return;
    }
    const sessionMenuBtn = event.target.closest?.("[data-session-menu]");
    if (sessionMenuBtn?.dataset.sessionMenu) {
      event.preventDefault();
      const id = sessionMenuBtn.dataset.sessionMenu;
      if (sessionActionsOpen() && sessionActionsId === id) {
        closeSessionActions();
      } else {
        openSessionActions(id, sessionMenuBtn);
      }
      return;
    }
    const sessionAction = event.target.closest?.("[data-session-action]");
    if (sessionAction?.dataset.sessionAction) {
      event.preventDefault();
      runSessionAction(sessionAction.dataset.sessionAction);
      return;
    }
    const sessionProject = event.target.closest?.("[data-session-project]");
    if (sessionProject?.dataset.sessionProject) {
      event.preventDefault();
      const id = sessionActionsId;
      const projectId = sessionProject.dataset.sessionProject;
      closeSessionActions();
      assignSessionProject(id, projectId);
      return;
    }
    if (
      sessionActionsOpen() &&
      !event.target.closest?.("#agent-session-actions") &&
      !event.target.closest?.("[data-session-menu]")
    ) {
      closeSessionActions();
    }
  });

  document.addEventListener("visibilitychange", () => {
    if (document.visibilityState === "hidden") {
      stopParticles();
    }
  });

  window.addEventListener("resize", () => {
    if (active) {
      syncComposerGeometry();
    }
  });

  document.addEventListener("input", (event) => {
    if (active && event.target?.id === "agent-composer-input") {
      requestAnimationFrame(syncComposerGeometry);
    }
  });
}
