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
  bindShelfAttachHost,
  addComposerAttachment,
  getComposerDraft,
  applyComposerDraft,
} from "./agent-shelf.js?v=home-20260814a";
import {
  shellState,
  desktopObjects,
} from "./shell-core.js?v=home-20260814a";
import {
  enableHarnessMenubarReveal,
  clearHarnessMenubarReveal,
  agentStageId,
  desktopStageId,
  getActiveStageId,
  isAgentSpace,
  setActiveStage,
  syncSpacePager,
} from "./shell-stages.js?v=home-20260814a";
import { registerEscapeHandler } from "./shell-popovers.js?v=home-20260814a";
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
  getLastStreamFailure,
} from "./mock-agent-provider.js?v=home-20260814a";
import {
  bindAgentWorkspaceSnapshot,
} from "./shell-windows.js?v=home-20260814a";
import { TIP } from "./agent-tip.js?v=home-20260814a";
import {
  clampReasoningEffort,
  cycleReasoningEffort,
  effortLabel,
  stampMessageNode,
  supportedReasoningEfforts,
} from "./agent-context.js?v=home-20260814a";
import { registerAgentHarnessApi } from "./agent-send.js?v=home-20260814a";
import {
  bindAgentWorkspaceStore,
  getAgentWorkspaceSnapshot,
  applyAgentWorkspaceSnapshot,
  persistAgentWorkspaceSoon,
  MAX_DRAFT_PART_TEXT,
} from "./agent-workspace.js?v=home-20260814a";
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
} from "./agent-configure.js?v=home-20260814a";
import {
  bindAgentGrants,
  syncTruthStrip,
  appendGrantCard,
  paintGrantCardResolved,
  resolveGrantFromCard,
  sessionAlreadyHasGrant,
  maybeOfferToolAfterReply,
  hydrateCapabilitiesFromSession,
  getReadyLibraryReadGrant,
} from "./agent-grants.js?v=home-20260814a";
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
  abortAgentStreamNow,
  getLiveTurnCanonical,
  startTurnForPrompt,
  deleteMessageAt,
  beginEditUserMessage,
  cancelEditUserMessage,
  submitEditUserMessage,
  regenerateLastAgentTurn,
  updateJumpToLatestVisibility,
  ensureJumpToLatest,
  setStreamStatus,
  openCodeArtifact,
} from "./agent-stream.js?v=home-20260814a";
import {
  getLiveInferenceState,
  probeLiveInference,
  extractAgentLibraryRead,
} from "./agent-live.js?v=home-20260814a";
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
} from "./agent-sessions.js?v=home-20260814a";
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
/** Wave 2 — prompt + sampling prefs (host-persisted; gateway clamps). */
let systemPrompt = "";
/** Wave 7 — sticky On-Home notes (host-persisted; appended to Live system). */
let agentNotes = "";
let maxTokens = 8192;
let temperature = 0.7;
/** RunConfig — not user content. Flash cannot honor this yet. */
let reasoningEffort = "medium";


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
  getSystemPrompt: () => systemPrompt,
  setSystemPrompt: (v) => {
    systemPrompt = String(v ?? "");
  },
  getAgentNotes: () => agentNotes,
  setAgentNotes: (v) => {
    agentNotes = String(v ?? "");
  },
  getMaxTokens: () => maxTokens,
  setMaxTokens: (v) => {
    maxTokens = Number(v) || 8192;
  },
  getTemperature: () => temperature,
  setTemperature: (v) => {
    temperature = Number(v);
  },
  getReasoningEffort: () => reasoningEffort,
  setReasoningEffort: (v) => {
    reasoningEffort = clampReasoningEffort(v);
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
  getWorkspaceHydrated: () => workspaceHydrated,
  getComposerDraft,
  applyComposerDraft,
});

/* The workspace may have no saved blob to restore (restoreAgentSurface only runs
   when the Agent stage is active), which would leave workspaceHydrated=false and
   block every persist. Mark hydrated at bind so saving works from a clean boot;
   applyAgentWorkspaceSnapshot (when a saved blob exists) still overwrites this. */
export function ensureAgentWorkspaceHydrated() {
  if (!workspaceHydrated) {
    workspaceHydrated = true;
  }
}

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
    get systemPrompt() {
      return systemPrompt;
    },
    set systemPrompt(v) {
      systemPrompt = String(v ?? "");
    },
    get agentNotes() {
      return agentNotes;
    },
    set agentNotes(v) {
      agentNotes = String(v ?? "");
    },
    get maxTokens() {
      return maxTokens;
    },
    set maxTokens(v) {
      maxTokens = Number(v) || 8192;
    },
    get temperature() {
      return temperature;
    },
    set temperature(v) {
      temperature = Number(v);
    },
  },
  {
    closeApproveMenu,
    closeModelMenu,
    isNarrowHarness,
    closeHarnessDrawer,
    persistAgentWorkspaceSoon,
    syncModelMenu: () => {
      try {
        renderModelMenu();
      } catch {
        /* optional during early boot */
      }
    },
    renderLibraryWorkbench: () => {
      try {
        renderLibraryWorkbench();
      } catch {
        /* optional during early boot */
      }
    },
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
    persistAgentWorkspaceSoon,
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
    get systemPrompt() { return systemPrompt; },
    get agentNotes() { return agentNotes; },
    get maxTokens() { return maxTokens; },
    get temperature() { return temperature; },
    get reasoningEffort() { return reasoningEffort; },
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
    setComposerGeometrySuspended,
    ensureSessionForPrompt,
    renderSessions,
    syncInferenceStatus: () => syncAgentInferenceStatus(),
    persistAgentWorkspaceSoon,
    renderHarnessPage,
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
    refreshHarnessDomCache,
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





let lastComposerGeometryKey = "";
let composerGeometryRoRaf = 0;
let composerGeometrySyncing = false;
let composerGeometryBurstAt = 0;
let composerGeometryBurst = 0;

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
  /* Re-entrancy guard: writing --harness-composer-clearance changes stream
     padding and can re-fire the ResizeObserver → 100% CPU tab freeze. */
  if (composerGeometrySyncing) {
    return;
  }
  const now = performance.now();
  if (now - composerGeometryBurstAt > 250) {
    composerGeometryBurstAt = now;
    composerGeometryBurst = 0;
  }
  composerGeometryBurst += 1;
  if (composerGeometryBurst > 8) {
    stopDockGeometryObserver();
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
  const key = `${width}|${left}|${clearance}`;
  if (key === lastComposerGeometryKey) {
    return;
  }
  lastComposerGeometryKey = key;
  composerGeometrySyncing = true;
  const root = document.documentElement;
  root.style.setProperty("--harness-composer-clearance", `${clearance}px`);
  root.style.setProperty("--agent-column-width", `${width}px`);
  root.style.setProperty("--agent-column-left", `${left}px`);
  requestAnimationFrame(() => {
    composerGeometrySyncing = false;
  });
}

function observeDockGeometry() {
  const taskbar = document.querySelector(".taskbar");
  if (!taskbar || typeof ResizeObserver !== "function") {
    return;
  }
  if (dockResizeObserver) {
    dockResizeObserver.disconnect();
  }
  dockResizeObserver = new ResizeObserver(() => {
    if (turnBusy) {
      return;
    }
    if (composerGeometryRoRaf) {
      return;
    }
    composerGeometryRoRaf = requestAnimationFrame(() => {
      composerGeometryRoRaf = 0;
      syncComposerGeometry();
    });
  });
  /* Observe the dock only. Watching `main` re-fired on every stream height
     change while --harness-composer-clearance padding updated → CPU spin.
     Sidebar toggles already call syncComposerGeometry explicitly. */
  dockResizeObserver.observe(taskbar);
}

function stopDockGeometryObserver() {
  dockResizeObserver?.disconnect();
  dockResizeObserver = null;
}

/** Suspend dock geometry work for the whole live/mock turn (stream growth). */
function setComposerGeometrySuspended(suspended) {
  if (suspended) {
    if (composerGeometryRoRaf) {
      cancelAnimationFrame(composerGeometryRoRaf);
      composerGeometryRoRaf = 0;
    }
    stopDockGeometryObserver();
    return;
  }
  observeDockGeometry();
  syncComposerGeometry();
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
  /* Messages live in the dock-width column so edges match the Shelf composer.
     Drop detached cache nodes after template remounts — otherwise newChat /
     selectSession mutate a ghost tree and the visible transcript stays put. */
  if (cachedStreamColumnEl?.isConnected) {
    return cachedStreamColumnEl;
  }
  cachedStreamColumnEl = document.querySelector("#agent-harness-stream-column");
  if (cachedStreamColumnEl) {
    return cachedStreamColumnEl;
  }
  if (cachedStreamScrollEl?.isConnected) {
    return cachedStreamScrollEl;
  }
  cachedStreamScrollEl = document.querySelector("#agent-harness-stream");
  return cachedStreamScrollEl;
}

function streamScrollEl() {
  if (cachedStreamScrollEl?.isConnected) {
    return cachedStreamScrollEl;
  }
  cachedStreamScrollEl = document.querySelector("#agent-harness-stream");
  return cachedStreamScrollEl;
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
  scroller.scrollTop = scroller.scrollHeight;
  /* One follow-up frame is enough for layout settle. The old double-rAF
     pin-every-call pattern stacked dozens of scroll writes per second during
     live tokens and helped peg the Brave renderer at ~100% CPU. */
  if (!turnBusy) {
    requestAnimationFrame(() => {
      scroller.scrollTop = scroller.scrollHeight;
    });
  }
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

function syncReasoningEffortButton() {
  const btn = document.querySelector("[data-reasoning-effort]");
  if (!btn) {
    return;
  }
  const efforts = supportedReasoningEfforts();
  const effort = efforts.length ? clampReasoningEffort(reasoningEffort) : "medium";
  if (!efforts.length) {
    reasoningEffort = "medium";
  }
  btn.dataset.reasoningEffort = effort;
  btn.dataset.effectiveEffort = efforts.includes(effort) ? effort : "model-default";
  btn.textContent = effortLabel(effort);
  if (!efforts.length) {
    btn.disabled = true;
    btn.setAttribute("aria-disabled", "true");
    btn.setAttribute(
      "aria-label",
      "Reasoning Medium — Flash uses model-default; effort levels wait on the offer",
    );
    btn.title =
      "Requested Medium · Effective model-default · Provider Flash · High/Low are not offered yet";
    return;
  }
  btn.disabled = false;
  btn.removeAttribute("aria-disabled");
  btn.setAttribute(
    "aria-label",
    `Reasoning effort ${effortLabel(effort)} — run setting, not a user message`,
  );
  btn.title = `Reasoning effort: ${effortLabel(effort)}`;
}

function cycleComposerReasoningEffort() {
  if (!supportedReasoningEfforts().length) {
    syncReasoningEffortButton();
    return;
  }
  reasoningEffort = cycleReasoningEffort(reasoningEffort);
  syncReasoningEffortButton();
  persistAgentWorkspaceSoon();
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
    /* Contract era: when an offer is live, the trigger shows the real model. */
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
    ? `${liveInference.model} — live via model offer on this Home`
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
  /* Contract era: advertised model offers first — the only rows that are
     real inference; preview stubs stay labeled below. */
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
        model.detail || "Live · on this Home via gateway";
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
  /* Deterministic PRNG: visual-only particles, no security input. */
  let mistSeed = 0x2f6e2b1;
  const mistNext = () => {
    mistSeed |= 0;
    mistSeed = (mistSeed + 0x6d2b79f5) | 0;
    let t = Math.imul(mistSeed ^ (mistSeed >>> 15), 1 | mistSeed);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
  const colors = ["#9aa3b2", "#c9d0dc", "#6e7684", "#dde3ec"];
  const particles = Array.from({ length: PARTICLE_COUNT }, () => ({
    x: mistNext() * w,
    y: mistNext() * h,
    vx: (mistNext() - 0.5) * 0.35,
    vy: -0.15 - mistNext() * 0.45,
    size: 0.8 + mistNext() * 1.8,
    alpha: 0.08 + mistNext() * 0.18,
    color: colors[(mistNext() * colors.length) | 0],
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
  let streamFail = "";
  try {
    streamFail = getLastStreamFailure() || "";
  } catch {
    streamFail = "";
  }
  if (live && !streamFail) {
    /* Live is the expected dogfood path — don't billboard model/transport chrome. */
    el.hidden = true;
    el.dataset.state = "live";
    el.textContent = "";
    document.body.dataset.agentInference = "live";
    return;
  }
  if (live && streamFail) {
    el.hidden = false;
    el.dataset.state = "error";
    el.textContent = `Live · last stream failed · ${streamFail}`;
    document.body.dataset.agentInference = "live-degraded";
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
  const why = state.reason && state.reason !== "unprobed" ? ` · ${state.reason}` : "";
  const fail = streamFail ? ` · last error: ${streamFail}` : "";
  el.textContent =
    `Preview · not live inference — mock replies until a model is wired${why}${fail}`;
  document.body.dataset.agentInference = "preview";
}

export function showAgentHarness({
  prompt,
  displayText,
  parts,
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
    session.messages.push(
      makeUserSessionMessage(prompt, { displayText: displayText || prompt, parts, session }),
    );
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
  syncReasoningEffortButton();
  syncSessionModeUi();
  syncWorkbenchOpenUi();
  syncAgentInferenceStatus();
  void probeLiveInference({ force: true }).then(() => {
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
  /* Navigation detach: don't cancel a live run just because the user left the
     Agent Space — the contract run continues server-side; we only stop consuming it. */
  stopMockStream({ keepPartial: true, cancelRun: false });
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
  stopMockStream({ keepPartial: true, drainQueue: true });
  setStreamStatus("");
}

function compactUserParts(parts) {
  if (!Array.isArray(parts) || !parts.length) {
    return undefined;
  }
  const compact = parts
    .filter((p) => p && typeof p === "object")
    .slice(0, 16)
    .map((p) => ({
      id: String(p.id || "").slice(0, 40),
      kind: String(p.kind || "file").slice(0, 24),
      name: String(p.name || p.title || "").slice(0, 180),
      title: String(p.title || p.name || "").slice(0, 48),
      subtitle: String(p.subtitle || "").slice(0, 32),
      size: Number(p.size) || 0,
      uri: p.uri ? String(p.uri).slice(0, 1024) : "",
      text: String(p.text || "").slice(0, MAX_DRAFT_PART_TEXT),
      version: Number(p.version) || 1,
      hash: String(p.hash || "").slice(0, 16),
      semanticRole: p.semanticRole === "user_input" ? "user_input" : "reference_material",
      authority: p.authority === "user" ? "user" : "untrusted_content",
    }));
  return compact.length ? compact : undefined;
}

function makeUserSessionMessage(modelText, { displayText, parts, session } = {}) {
  const compact = compactUserParts(parts);
  const typed = String(displayText || "").trim();
  const text = typed || (compact ? "" : String(modelText || "").slice(0, 80));
  return stampMessageNode(
    {
      role: "user",
      text,
      modelText: String(modelText || ""),
      ...(compact ? { parts: compact } : {}),
    },
    { session },
  );
}

export function sendToAgentHarness(prompt, opts = {}) {
  const modelText = String(prompt || "").trim();
  const parts = Array.isArray(opts.parts) ? opts.parts : undefined;
  const typed = String(opts.displayText || "").trim();
  const displayText = typed || (parts?.length ? "" : modelText);
  if (!modelText) {
    if (active) {
      stopMockStream({ keepPartial: true });
      turnBusy = false;
    }
    return;
  }
  if (active) {
    /* While a turn streams, queue follow-ups instead of cutting the answer. */
    if (turnBusy) {
      enqueueFollowUp(modelText, { displayText, parts });
      return;
    }
    closeHarnessPage();
    const session = ensureSessionForPrompt(modelText);
    if (session.title === "New chat" || session.messages.length === 0) {
      session.title = titleFromPrompt(displayText || modelText);
    }
    session.messages.push(makeUserSessionMessage(modelText, { displayText, parts, session }));
    touchSession(session);
    renderSessions();
    setTitle(session.title);
    clearEmptyState();
    appendMessage("user", displayText, { parts, modelText });
    if (maybeUpdatePlanFromPrompt(modelText)) {
      openWorkbench({ tab: "plan" });
    } else {
      syncWorkbenchPanels();
    }
    startTurnForPrompt(modelText);
    return;
  }
  showAgentHarness({ prompt: modelText, displayText, parts });
}






let sessionActionsId = null;






/** When creating a project from a chat’s menu, file that chat into it. */
let pendingProjectAssignSessionId = null;

function renderDesktopAttachOptions(menu) {
  const list = menu?.querySelector?.("[data-attach-desktop-list]");
  if (!list) {
    return;
  }
  list.replaceChildren();
  const objects = desktopObjects(shellState.currentSummary).slice(0, 24);
  if (!objects.length) {
    const empty = document.createElement("p");
    empty.className = "agent-attach-menu-empty";
    empty.textContent = "No Desktop objects yet — choose a local file or open Library later.";
    list.append(empty);
    return;
  }
  for (const object of objects) {
    const btn = document.createElement("button");
    btn.type = "button";
    btn.className = "agent-attach-menu-item";
    btn.dataset.attachDesktopUri = object.uri;
    btn.dataset.attachDesktopName = object.name;
    btn.dataset.attachDesktopSize = String(object.size || object.byte_size || 0);
    btn.textContent = object.name;
    btn.title = object.uri;
    list.append(btn);
  }
}

export function renderLibraryWorkbench() {
  const host = document.querySelector("[data-library-chips]");
  if (!host) {
    return;
  }
  host.replaceChildren();
  const objects = desktopObjects(shellState.currentSummary).slice(0, 32);
  if (!objects.length) {
    const empty = document.createElement("p");
    empty.className = "agent-library-empty";
    empty.dataset.libraryEmpty = "1";
    const grant = getReadyLibraryReadGrant();
    empty.textContent = grant?.requestId
      ? "Desktop is empty — grant is ready; add a file then attach to extract."
      : "Desktop objects appear here. Attach from the composer — content extract needs a library.read grant (Inbox; UI ≠ authority).";
    host.append(empty);
    return;
  }
  const grant = getReadyLibraryReadGrant();
  for (const object of objects) {
    const btn = document.createElement("button");
    btn.type = "button";
    btn.className = "agent-library-chip";
    btn.dataset.libraryAttachUri = object.uri;
    btn.dataset.libraryAttachName = object.name;
    btn.textContent = object.name;
    btn.title = grant?.requestId
      ? `${object.uri} · attach with Inbox library.read extract`
      : `${object.uri} · attach (content needs library.read)`;
    host.append(btn);
  }
  const note = document.createElement("p");
  note.className = "agent-library-empty";
  note.textContent = grant?.requestId
    ? "Inbox library.read ready — attach extracts cited Desktop text into the next Live turn."
    : "Attach adds a Desktop reference. Extracted text waits on Inbox library.read (no ambient authority).";
  host.append(note);
}

export function bindAgentHarness() {
  if (bound) {
    return;
  }
  bound = true;
  bindShelfAttachHost({
    renderDesktopAttachOptions,
    persistComposer: persistAgentWorkspaceSoon,
  });
  renderLibraryWorkbench();
  refreshHarnessDomCache();
  const newChatBtn = document.querySelector("#agent-harness-new-chat");
  if (newChatBtn && newChatBtn.dataset.boundNewChat !== "1") {
    newChatBtn.dataset.boundNewChat = "1";
    newChatBtn.addEventListener("click", (event) => {
      event.preventDefault();
      event.stopPropagation();
      newChat();
      if (isNarrowHarness()) {
        closeHarnessDrawer();
      }
    });
  }
  registerAgentHarnessApi({
    agentHarnessActive,
    showAgentHarness,
    hideAgentHarness,
    sendToAgentHarness,
    stopAgentHarnessStream,
    abortAgentStreamNow,
  });
  bindAgentWorkspaceSnapshot(getAgentWorkspaceSnapshot);
  ensureAgentWorkspaceHydrated();
  syncReasoningEffortButton();
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

  /* Contract era: real probe — live flips only on offers_list truth (§AL.3). */
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
    const openCode = event.target.closest?.("[data-open-artifact]");
    if (openCode) {
      event.preventDefault();
      const block = openCode.closest(".agent-md-code");
      const code = block?.querySelector("code")?.textContent || "";
      const lang = openCode.dataset.lang || "code";
      openCodeArtifact(code, lang);
      return;
    }
    const copyCode = event.target.closest?.(".agent-md-copy");
    if (copyCode) {
      event.preventDefault();
      const block = copyCode.closest(".agent-md-code");
      const code = block?.dataset?.mdSource ?? block?.querySelector("code")?.textContent ?? "";
      navigator.clipboard?.writeText(code).catch(() => {});
      copyCode.classList.add("is-copied");
      copyCode.setAttribute("aria-label", "Copied");
      window.setTimeout(() => {
        copyCode.classList.remove("is-copied");
        copyCode.setAttribute("aria-label", "Copy code");
      }, 1500);
      return;
    }
    const copyMsg = event.target.closest?.("[data-copy-message]");
    if (copyMsg) {
      event.preventDefault();
      const row = copyMsg.closest(".agent-msg");
      const body = row?.querySelector(".agent-msg-body");
      const live = row?.classList.contains("is-streaming") ? getLiveTurnCanonical() : null;
      const text =
        live?.answer ||
        body?.dataset?.mdSource ||
        body?.innerText ||
        body?.textContent ||
        "";
      navigator.clipboard?.writeText(String(text)).catch(() => {});
      copyMsg.textContent = "Copied";
      window.setTimeout(() => {
        copyMsg.textContent = "Copy";
      }, 1200);
      return;
    }
    if (event.target.closest?.("[data-agent-jump-latest]")) {
      event.preventDefault();
      scrollStreamToEnd();
      updateJumpToLatestVisibility();
      return;
    }
    const editCancel = event.target.closest?.("[data-edit-cancel]");
    if (editCancel) {
      event.preventDefault();
      const idx = editCancel.closest(".agent-msg")?.dataset.msgIndex;
      cancelEditUserMessage(idx);
      return;
    }
    const editBtn = event.target.closest?.("[data-edit-message]");
    if (editBtn) {
      event.preventDefault();
      if (turnBusy) {
        return;
      }
      beginEditUserMessage(editBtn.closest(".agent-msg")?.dataset.msgIndex);
      return;
    }
    const deleteBtn = event.target.closest?.("[data-delete-message]");
    if (deleteBtn) {
      event.preventDefault();
      if (turnBusy) {
        return;
      }
      deleteMessageAt(deleteBtn.closest(".agent-msg")?.dataset.msgIndex);
      return;
    }
    if (event.target.closest?.("[data-retry]") || event.target.closest?.("[data-regenerate]")) {
      event.preventDefault();
      event.target.closest(".agent-msg-stopped")?.remove();
      if (turnBusy) {
        return;
      }
      regenerateLastAgentTurn();
      return;
    }
    const thinkToggle = event.target.closest?.("[data-truth-thinking-toggle]");
    if (thinkToggle) {
      event.preventDefault();
      toggleReasoningVisible();
      return;
    }
    const effortBtn = event.target.closest?.("[data-reasoning-effort]");
    if (effortBtn) {
      event.preventDefault();
      cycleComposerReasoningEffort();
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
      return;
    }
    const lib = event.target.closest?.("[data-library-attach-uri]");
    if (lib?.dataset.libraryAttachUri) {
      event.preventDefault();
      const uri = lib.dataset.libraryAttachUri;
      const name = lib.dataset.libraryAttachName || "Desktop object";
      void (async () => {
        const grant = getReadyLibraryReadGrant();
        let text = "";
        if (grant?.requestId) {
          try {
            const extracted = await extractAgentLibraryRead(grant.requestId, uri);
            text = String(extracted?.text || "");
          } catch (err) {
            console.warn("Workbench library.read extract failed", err);
          }
        }
        addComposerAttachment({
          kind: "desktop",
          name,
          uri,
          text,
        });
        openWorkbench({ tab: "library", force: true });
        const input = shelfComposerInput();
        input?.focus?.({ preventScroll: true });
        syncAgentSendButton();
      })();
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

  document.addEventListener("submit", (event) => {
    const form = event.target?.closest?.("[data-edit-form]");
    if (!form || !active) {
      return;
    }
    event.preventDefault();
    const idx = form.closest(".agent-msg")?.dataset.msgIndex;
    const text = form.querySelector(".agent-msg-edit-input")?.value || "";
    submitEditUserMessage(idx, text);
  });

  document.addEventListener(
    "scroll",
    (event) => {
      if (!active) {
        return;
      }
      if (event.target?.id === "agent-harness-stream") {
        updateJumpToLatestVisibility();
      }
    },
    true,
  );

  ensureJumpToLatest();
}
