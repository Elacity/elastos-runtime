/* Agent Harness (preview) — Home drops, Shelf stays as composer hinge.
   UI ≠ authority (Principle 16): never mints grants, never opens Carrier/
   capsule ambient paths. Mock stream only until agentic/runtime waves wire
   explicit, revocable tools (fail-closed). */

import {
  setAgentComposerProcessing,
  syncAgentSendButton,
  composerInput as shelfComposerInput,
  hideAgentShelfFace,
} from "./agent-shelf.js?v=home-20260724co";
import {
  enableHarnessMenubarReveal,
  clearHarnessMenubarReveal,
  agentStageId,
  desktopStageId,
  getActiveStageId,
  isAgentSpace,
  setActiveStage,
  syncSpacePager,
} from "./shell-stages.js?v=home-20260724co";
import { registerEscapeHandler } from "./shell-popovers.js?v=home-20260724co";
import {
  MOCK_REPLY,
  listCapabilities,
  requestTool,
  resolveMockApproval,
  resetMockCapabilities,
  applyCapabilityState,
  wantsLibraryTool,
  wantsWalletTool,
  getTruthSnapshot,
  noteMockTurnTokens,
  getSelectedModel,
  getMockTurn,
  loadReasoningVisible,
  setReasoningVisible,
  getPlanMarkdown,
  setPlanMarkdown,
  maybeUpdatePlanFromPrompt,
} from "./mock-agent-provider.js?v=home-20260724co";

const TIP = "home-20260724co";
let workbenchTab = "outputs";

const STARTER_PROMPTS = [
  { label: "What can you do?", text: "What can you do on this device with tools at zero?" },
  { label: "Explain locality", text: "Explain On this device and why tools start at zero." },
  { label: "Draft a capsule plan", text: "Help me plan a small Notes capsule with library.read only." },
];

function relativeTime(ts) {
  if (!ts) {
    return "";
  }
  const delta = Date.now() - ts;
  if (delta < 60_000) {
    return "now";
  }
  if (delta < 3_600_000) {
    return `${Math.max(1, Math.round(delta / 60_000))}m`;
  }
  if (delta < 86_400_000) {
    return `${Math.max(1, Math.round(delta / 3_600_000))}h`;
  }
  return `${Math.max(1, Math.round(delta / 86_400_000))}d`;
}

function touchSession(session) {
  if (session) {
    session.updatedAt = Date.now();
    session.mode = sessionMode;
  }
}

function exportActiveSessionMarkdown() {
  const session = sessions.find((s) => s.id === activeSessionId);
  if (!session) {
    return;
  }
  const lines = [`# ${session.title}`, "", `Mode: ${session.mode || sessionMode}`, ""];
  for (const msg of session.messages) {
    if (msg.role === "user") {
      lines.push(`## You`, "", msg.text, "");
    } else if (msg.role === "agent") {
      if (msg.thinking) {
        lines.push(`## Thinking`, "", msg.thinking, "");
      }
      lines.push(`## Agent`, "", msg.text, "");
    } else if (msg.role === "grant") {
      lines.push(`## Grant · ${msg.label || msg.toolId}`, "", `${msg.state}: ${msg.summary || ""}`, "");
    }
  }
  const blob = new Blob([lines.join("\n")], { type: "text/markdown;charset=utf-8" });
  const url = URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.href = url;
  a.download = `${(session.title || "chat").replace(/[^\w\-]+/g, "_").slice(0, 48)}.md`;
  a.click();
  URL.revokeObjectURL(url);
}
const HOME_BREATHE_MS = 780;
const HOME_RISE_MS = 720;
const HARNESS_CONTENT_AT_MS = 180;
const PARTICLE_COUNT = 420;
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
/** Follow-up prompts queued while a mock turn is streaming (fx7). */
let followUpQueue = [];
let reasoningVisible = loadReasoningVisible();
/** True while thinking or answer mock stream is in flight. */
let turnBusy = false;
/** Session skin: chat | build — presentation only (fx8). */
let sessionMode = "chat";
/** Tool intent chips: read | ask | full — not grants (fx8). */
let toolMode = "read";

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

function sessionSearchOpen() {
  const root = document.querySelector("#agent-session-search");
  return Boolean(root) && !root.hidden;
}

function renderSessionSearchResults(query = "") {
  const host = document.querySelector("#agent-session-search-results");
  if (!host) {
    return;
  }
  host.replaceChildren();
  const q = String(query || "").trim().toLowerCase();
  const matches = sessions.filter((session) => {
    if (!q) {
      return true;
    }
    if (session.title.toLowerCase().includes(q)) {
      return true;
    }
    return (session.messages || []).some((m) =>
      String(m.text || "").toLowerCase().includes(q),
    );
  });
  if (!matches.length) {
    const empty = document.createElement("p");
    empty.className = "agent-session-search-empty";
    empty.textContent = q ? "No chats match" : "No chats yet";
    host.append(empty);
    return;
  }
  for (const session of matches) {
    const row = document.createElement("button");
    row.type = "button";
    row.className = `agent-session-search-row${
      session.id === activeSessionId ? " is-active" : ""
    }`;
    row.dataset.sessionId = session.id;
    row.setAttribute("role", "option");
    row.innerHTML =
      `<span class="agent-session-search-row-mark" aria-hidden="true"></span>` +
      `<span class="agent-session-search-row-title"></span>` +
      `<span class="agent-session-search-row-when"></span>`;
    row.querySelector(".agent-session-search-row-title").textContent = session.title;
    row.querySelector(".agent-session-search-row-when").textContent =
      session.group || "";
    host.append(row);
  }
}

function openSessionSearch() {
  if (!active) {
    return;
  }
  const root = document.querySelector("#agent-session-search");
  const input = document.querySelector("#agent-session-search-input");
  if (!root) {
    return;
  }
  root.hidden = false;
  root.inert = false;
  root.setAttribute("aria-hidden", "false");
  renderSessionSearchResults(input?.value || "");
  window.requestAnimationFrame(() => {
    input?.focus({ preventScroll: true });
    input?.select?.();
  });
}

function closeSessionSearch() {
  const root = document.querySelector("#agent-session-search");
  const input = document.querySelector("#agent-session-search-input");
  if (!root || root.hidden) {
    return;
  }
  root.hidden = true;
  root.inert = true;
  root.setAttribute("aria-hidden", "true");
  if (input) {
    input.value = "";
  }
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

function harnessEl() {
  return document.querySelector("#agent-harness");
}

function streamEl() {
  /* Messages live in the dock-width column so edges match the Shelf composer. */
  return (
    document.querySelector("#agent-harness-stream-column") ||
    document.querySelector("#agent-harness-stream")
  );
}

function streamScrollEl() {
  return document.querySelector("#agent-harness-stream");
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

function clearStreamTimer() {
  if (streamTimer) {
    window.clearInterval(streamTimer);
    streamTimer = 0;
  }
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

function titleFromPrompt(prompt) {
  const cleaned = prompt.replace(/\s+/g, " ").trim();
  if (!cleaned) {
    return "New chat";
  }
  return cleaned.length > 42 ? `${cleaned.slice(0, 41)}…` : cleaned;
}

function escapeHtml(text) {
  return String(text)
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;");
}

/** Tiny markdown for MOCK_REPLY only — escapeHtml on fences/inlines.
 *  SEAM (live model / Carrier-backed replies): sanitize or use a text-safe
 *  path before innerHTML. UI must never treat model HTML as authority. */
function renderMarkdown(text) {
  const parts = String(text).split(/```([\s\S]*?)```/g);
  let html = "";
  for (let i = 0; i < parts.length; i += 1) {
    if (i % 2 === 1) {
      const fence = parts[i];
      const nl = fence.indexOf("\n");
      const lang = nl === -1 ? "" : fence.slice(0, nl).trim();
      const code = nl === -1 ? fence : fence.slice(nl + 1);
      const safe = escapeHtml(code.replace(/\n$/, ""));
      html +=
        `<div class="agent-md-code">` +
        `<div class="agent-md-code-head"><span>${escapeHtml(lang || "code")}</span>` +
        `<button type="button" class="agent-md-copy" data-copy="1">Copy</button></div>` +
        `<pre><code>${safe}</code></pre></div>`;
      continue;
    }
    const blocks = parts[i].split(/\n{2,}/);
    for (const block of blocks) {
      const trimmed = block.trim();
      if (!trimmed) {
        continue;
      }
      let line = escapeHtml(trimmed)
        .replaceAll(/\*\*(.+?)\*\*/g, "<strong>$1</strong>")
        .replaceAll(/`([^`]+)`/g, "<code class=\"agent-md-inline\">$1</code>")
        .replaceAll(/\n/g, "<br>");
      html += `<p class="agent-md-p">${line}</p>`;
    }
  }
  return html;
}

function setTitle(title) {
  const el = titleEl();
  if (el) {
    el.textContent = title;
  }
}

function renderSessions() {
  const list = sessionListEl();
  if (!list) {
    return;
  }
  list.replaceChildren();
  const pinned = sessions.filter((s) => s.pinned);
  const groups = [
    { id: "Pinned", items: pinned },
    { id: "Today", items: sessions.filter((s) => !s.pinned && s.group === "Today") },
    { id: "Earlier", items: sessions.filter((s) => !s.pinned && s.group === "Earlier") },
  ];
  for (const group of groups) {
    if (!group.items.length) {
      continue;
    }
    const label = document.createElement("div");
    label.className = "agent-harness-group-label";
    label.textContent = group.id;
    list.append(label);
    for (const session of group.items) {
      const row = document.createElement("div");
      row.className = `agent-harness-session${session.id === activeSessionId ? " is-active" : ""}`;
      row.dataset.sessionId = session.id;

      const btn = document.createElement("button");
      btn.type = "button";
      btn.className = "agent-harness-session-btn";
      const title = document.createElement("span");
      title.className = "agent-harness-session-title";
      title.textContent = session.title;
      const meta = document.createElement("span");
      meta.className = "agent-harness-session-meta";
      const mode = session.mode || "chat";
      meta.textContent = [mode, relativeTime(session.updatedAt)].filter(Boolean).join(" · ");
      btn.append(title, meta);
      btn.title = session.title;

      const kebab = document.createElement("button");
      kebab.type = "button";
      kebab.className = "agent-harness-session-menu";
      kebab.setAttribute("aria-label", `Session actions for ${session.title}`);
      kebab.title = "Pin, export, rename, or delete";
      kebab.textContent = "···";

      row.append(btn, kebab);
      list.append(row);
    }
  }
}

function syncTruthStrip() {
  const root = document.querySelector(".agent-harness-truth");
  if (!root) {
    return;
  }
  const snap = getTruthSnapshot();
  const local = root.querySelector("[data-truth-local]");
  const model = root.querySelector("[data-truth-model]");
  const tools = root.querySelector("[data-truth-tools]");
  const context = root.querySelector("[data-truth-context]");
  const contextFill = root.querySelector("[data-truth-context-fill]");
  const hw = root.querySelector("[data-truth-hw]");
  if (local) {
    local.textContent = snap.locality;
  }
  if (model) {
    model.textContent = snap.modelLabel;
    model.title = `${snap.modelLabel} · ${snap.modelTier}`;
  }
  if (tools) {
    tools.textContent = snap.toolsLabel;
  }
  if (context) {
    context.textContent = snap.contextLabel;
    context.title = "Mock context window fill — not live tokenizer";
  }
  if (contextFill) {
    contextFill.style.width = `${Math.round(snap.contextRatio * 100)}%`;
  }
  if (hw) {
    hw.textContent = snap.hwLabel;
    hw.dataset.hw = snap.hwState;
    hw.title =
      snap.hwState === "unknown"
        ? "Hardware not probed yet (Spark/W2)"
        : "Stub estimate — not a live probe";
  }
  root.dataset.tools = snap.toolsState;
  root.dataset.hw = snap.hwState;
  root.dataset.modelTier = snap.modelTier;

  const modelBtnName = document.querySelector(".agent-model-name");
  const modelBtnTier = document.querySelector(".agent-model-tier");
  const selected = getSelectedModel();
  if (modelBtnName && selected) {
    modelBtnName.textContent = selected.tier === "preview" ? "Local" : selected.label;
  }
  if (modelBtnTier && selected) {
    modelBtnTier.textContent = selected.tier === "unsupported" ? "spark" : selected.tier;
  }

  const thinkToggle = root.querySelector("[data-truth-thinking-toggle]");
  if (thinkToggle) {
    thinkToggle.setAttribute("aria-pressed", reasoningVisible ? "true" : "false");
    thinkToggle.textContent = reasoningVisible ? "Thinking on" : "Thinking off";
    thinkToggle.title = reasoningVisible
      ? "Hide model thinking blocks"
      : "Show model thinking blocks";
  }
  document.documentElement.dataset.agentReasoning = reasoningVisible ? "on" : "off";
}

function appendTurnDivider(label) {
  const stream = streamEl();
  if (!stream) {
    return null;
  }
  const row = document.createElement("div");
  row.className = "agent-turn-divider";
  row.setAttribute("role", "separator");
  row.innerHTML = `<span class="agent-turn-divider-label"></span>`;
  row.querySelector(".agent-turn-divider-label").textContent = label;
  stream.append(row);
  return row;
}

/** Live turn timeline: Thinking → Tool → Answering → Done (fx11). */
function appendTurnTimeline() {
  const stream = streamEl();
  if (!stream) {
    return null;
  }
  const root = document.createElement("div");
  root.className = "agent-turn-timeline";
  root.dataset.timeline = "1";
  root.innerHTML =
    `<div class="agent-timeline-step" data-step="thinking" data-state="pending">` +
    `<span class="agent-timeline-dot" aria-hidden="true"></span>` +
    `<span class="agent-timeline-label">Thinking</span></div>` +
    `<div class="agent-timeline-step" data-step="tool" data-state="idle" hidden>` +
    `<span class="agent-timeline-dot" aria-hidden="true"></span>` +
    `<span class="agent-timeline-label">Tool</span>` +
    `<span class="agent-timeline-detail"></span></div>` +
    `<div class="agent-timeline-step" data-step="answer" data-state="pending">` +
    `<span class="agent-timeline-dot" aria-hidden="true"></span>` +
    `<span class="agent-timeline-label">Answering</span></div>` +
    `<div class="agent-timeline-step" data-step="done" data-state="pending">` +
    `<span class="agent-timeline-dot" aria-hidden="true"></span>` +
    `<span class="agent-timeline-label">Done</span></div>`;
  stream.append(root);
  return root;
}

function setTimelineStep(timeline, stepId, state, detail = "") {
  if (!timeline) {
    return;
  }
  const step = timeline.querySelector(`[data-step="${stepId}"]`);
  if (!step) {
    return;
  }
  if (state === "idle") {
    step.hidden = true;
    step.dataset.state = "idle";
    return;
  }
  step.hidden = false;
  step.dataset.state = state;
  const detailEl = step.querySelector(".agent-timeline-detail");
  if (detailEl && detail) {
    detailEl.textContent = detail;
  }
}

function appendThinkingBlock(text, { streaming = false, open = true } = {}) {
  const stream = streamEl();
  if (!stream) {
    return null;
  }
  clearEmptyState();
  const details = document.createElement("details");
  details.className = `agent-thinking${streaming ? " is-streaming" : ""}`;
  details.dataset.block = "thinking";
  details.dataset.startedAt = String(Date.now());
  if (open && reasoningVisible) {
    details.open = true;
  }
  const summary = document.createElement("summary");
  summary.className = "agent-thinking-summary";
  summary.innerHTML =
    `<span class="agent-thinking-chevron" aria-hidden="true"></span>` +
    `<span class="agent-thinking-label">Thinking</span>` +
    `<span class="agent-thinking-duration" data-thinking-duration></span>` +
    `<span class="agent-thinking-hint">preview · not authority</span>`;
  const bodyWrap = document.createElement("div");
  bodyWrap.className = "agent-thinking-body-wrap";
  const body = document.createElement("pre");
  body.className = "agent-thinking-body";
  body.textContent = text;
  bodyWrap.append(body);
  details.append(summary, bodyWrap);
  stream.append(details);
  scrollStreamToEnd();
  return details;
}

function finishThinkingBlock(details, startedAt) {
  if (!details) {
    return;
  }
  details.classList.remove("is-streaming");
  details.classList.add("is-complete");
  const ms = Math.max(400, Date.now() - (startedAt || Date.now()));
  const sec = (ms / 1000).toFixed(ms >= 10000 ? 0 : 1);
  const dur = details.querySelector("[data-thinking-duration]");
  if (dur) {
    dur.textContent = `Thought for ${sec}s`;
  }
  const label = details.querySelector(".agent-thinking-label");
  if (label) {
    label.textContent = "Thought";
  }
  /* Collapse after a beat — frontier-style; honor reduced motion by skipping delay animation only. */
  if (details.open && reasoningVisible) {
    const collapse = () => {
      if (details.isConnected && reasoningVisible) {
        details.open = false;
      }
    };
    if (prefersReducedMotion()) {
      collapse();
    } else {
      window.setTimeout(collapse, 900);
    }
  }
}

function appendToolTimelineRow(tool) {
  const stream = streamEl();
  if (!stream || !tool) {
    return null;
  }
  const row = document.createElement("div");
  row.className = "agent-tool-row is-running";
  row.dataset.toolId = tool.id;
  row.innerHTML =
    `<span class="agent-tool-row-spinner" aria-hidden="true"></span>` +
    `<div class="agent-tool-row-copy">` +
    `<span class="agent-tool-row-name"></span>` +
    `<span class="agent-tool-row-detail"></span>` +
    `</div>` +
    `<span class="agent-tool-row-status">Running</span>`;
  row.querySelector(".agent-tool-row-name").textContent = tool.label;
  row.querySelector(".agent-tool-row-detail").textContent = tool.detail || tool.kind || "";
  stream.append(row);
  scrollStreamToEnd();
  return row;
}

function finishToolTimelineRow(row, { status = "done", statusLabel = "Done" } = {}) {
  if (!row) {
    return;
  }
  row.classList.remove("is-running");
  row.classList.add(status === "error" || status === "denied" ? "is-denied" : "is-done");
  const statusEl = row.querySelector(".agent-tool-row-status");
  if (statusEl) {
    statusEl.textContent = statusLabel;
  }
}

function appendFollowUpChips(prompts) {
  const stream = streamEl();
  if (!stream || !prompts?.length) {
    return null;
  }
  document.querySelectorAll(".agent-followups").forEach((node) => node.remove());
  const root = document.createElement("div");
  root.className = "agent-followups";
  root.setAttribute("aria-label", "Suggested follow-ups");
  for (const text of prompts) {
    const chip = document.createElement("button");
    chip.type = "button";
    chip.className = "agent-followup-chip";
    chip.dataset.starter = text;
    chip.textContent = text;
    root.append(chip);
  }
  stream.append(root);
  scrollStreamToEnd();
  return root;
}

function renderFollowUpQueue() {
  const root = document.querySelector("[data-agent-queue]");
  if (!root) {
    return;
  }
  root.replaceChildren();
  root.hidden = followUpQueue.length === 0;
  if (!followUpQueue.length) {
    return;
  }
  const label = document.createElement("span");
  label.className = "agent-queue-label";
  label.textContent = "Queued";
  root.append(label);
  for (const item of followUpQueue) {
    const chip = document.createElement("button");
    chip.type = "button";
    chip.className = "agent-queue-chip";
    chip.dataset.queueId = item.id;
    chip.title = "Remove from queue";
    chip.textContent = item.text.length > 42 ? `${item.text.slice(0, 41)}…` : item.text;
    root.append(chip);
  }
}

function enqueueFollowUp(text) {
  followUpQueue.push({ id: `q-${Date.now()}-${followUpQueue.length}`, text });
  renderFollowUpQueue();
}

function drainFollowUpQueue() {
  if (!followUpQueue.length || !active) {
    return;
  }
  const next = followUpQueue.shift();
  renderFollowUpQueue();
  if (!next?.text) {
    return;
  }
  const session = sessions.find((s) => s.id === activeSessionId) || ensureSessionForPrompt(next.text);
  session.messages.push({ role: "user", text: next.text });
  renderSessions();
  appendMessage("user", next.text);
  startMockStreamForPrompt(next.text);
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
  for (const btn of document.querySelectorAll("[data-tool-mode]")) {
    const on = btn.dataset.toolMode === toolMode;
    btn.classList.toggle("is-active", on);
    btn.setAttribute("aria-pressed", on ? "true" : "false");
  }
  const note = document.querySelector("[data-tool-mode-note]");
  if (note) {
    if (sessionMode === "build") {
      note.textContent =
        toolMode === "full"
          ? "Build · Full intent — still preview; ADE sandbox grant required to write"
          : toolMode === "ask"
            ? "Build · Ask — plan/outputs theatre; grants via Inbox cards only"
            : "Build · Read — plan & inspect; no write authority";
    } else if (toolMode === "full") {
      note.textContent = "Chat · Full intent — UI ≠ authority; no ambient tools";
    } else if (toolMode === "ask") {
      note.textContent = "Chat · Ask — grant cards may appear; deny by default";
    } else {
      note.textContent = "Chat · Read — answers only · tools start at zero · preview";
    }
  }
  const title = document.querySelector("[data-workbench-title]");
  if (title) {
    title.textContent = sessionMode === "build" ? "Workbench · Build" : "Workbench";
  }
  for (const tab of document.querySelectorAll("[data-build-only]")) {
    tab.hidden = sessionMode !== "build";
  }
  if (sessionMode === "build" && (workbenchTab === "outputs" || workbenchTab === "status")) {
    setWorkbenchTab("plan");
  } else if (sessionMode === "chat" && (workbenchTab === "diff" || workbenchTab === "plan")) {
    setWorkbenchTab("status");
  } else {
    setWorkbenchTab(workbenchTab);
  }
  syncWorkbenchPanels();
}

function setWorkbenchTab(tabId) {
  const allowed = new Set([
    "outputs",
    "plan",
    "library",
    "status",
    "tools",
    "diff",
    "browser",
    "terminal",
  ]);
  workbenchTab = allowed.has(tabId) ? tabId : "outputs";
  if (workbenchTab === "diff" && sessionMode !== "build") {
    workbenchTab = "outputs";
  }
  for (const tab of document.querySelectorAll("[data-workbench-tab]")) {
    const on = tab.dataset.workbenchTab === workbenchTab;
    tab.classList.toggle("is-active", on);
    tab.setAttribute("aria-selected", on ? "true" : "false");
  }
  for (const panel of document.querySelectorAll("[data-workbench-panel]")) {
    const on = panel.dataset.workbenchPanel === workbenchTab;
    panel.classList.toggle("is-active", on);
    panel.hidden = !on;
  }
}

function syncWorkbenchPanels() {
  const plan = document.querySelector("[data-plan-markdown]");
  if (plan && document.activeElement !== plan) {
    plan.value = getPlanMarkdown();
  }
  const status = document.querySelector("[data-status-panel]");
  if (status) {
    const snap = getTruthSnapshot();
    status.innerHTML =
      `<div><dt>Locality</dt><dd></dd></div>` +
      `<div><dt>Model</dt><dd></dd></div>` +
      `<div><dt>Context</dt><dd></dd></div>` +
      `<div><dt>Tools</dt><dd></dd></div>` +
      `<div><dt>Hardware</dt><dd></dd></div>` +
      `<div><dt>Mode</dt><dd></dd></div>`;
    const dds = status.querySelectorAll("dd");
    dds[0].textContent = snap.locality;
    dds[1].textContent = snap.modelLabel;
    dds[2].textContent = snap.contextLabel;
    dds[3].textContent = snap.toolsLabel;
    dds[4].textContent = snap.hwLabel;
    dds[5].textContent = `${sessionMode} · ${toolMode} intent`;
  }
  const toolsList = document.querySelector("[data-tools-list]");
  if (toolsList) {
    toolsList.replaceChildren();
    for (const cap of listCapabilities()) {
      const li = document.createElement("li");
      li.className = "agent-tools-item";
      li.dataset.state = cap.state;
      li.textContent = `${cap.label} · ${cap.state}`;
      toolsList.append(li);
    }
  }
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
}

function appendMessage(role, text, { streaming = false, asHtml = false } = {}) {
  const stream = streamEl();
  if (!stream) {
    return null;
  }
  clearEmptyState();

  const row = document.createElement("div");
  row.className = `agent-msg agent-msg-${role}${streaming ? " is-streaming" : ""}`;
  row.dataset.role = role;

  const meta = document.createElement("div");
  meta.className = "agent-msg-meta";
  meta.textContent = role === "user" ? "You" : "Agent";

  const body = document.createElement("div");
  body.className = "agent-msg-body";
  if (asHtml) {
    body.innerHTML = text;
  } else if (role === "agent" && !streaming) {
    body.innerHTML = renderMarkdown(text);
  } else {
    body.textContent = text;
  }

  const actions = document.createElement("div");
  actions.className = "agent-msg-actions";
  const copyBtn = document.createElement("button");
  copyBtn.type = "button";
  copyBtn.className = "agent-msg-action";
  copyBtn.dataset.copyMessage = "1";
  copyBtn.textContent = "Copy";
  copyBtn.title = "Copy message";
  const regen = document.createElement("button");
  regen.type = "button";
  regen.className = "agent-msg-action";
  regen.dataset.regenerate = "1";
  regen.disabled = role !== "agent" || streaming;
  regen.title = role === "agent" ? "Regenerate mock reply" : "Regenerate — agent only";
  regen.textContent = "Regenerate";
  actions.append(copyBtn, regen);

  if (!streaming) {
    row.classList.add("agent-msg-enter");
  }
  row.append(meta, body, actions);
  stream.append(row);
  scrollStreamToEnd();
  return row;
}

/**
 * Inbox-grammar grant card (preview mock). Deny / Allow once update mock
 * state only — never Capsule/Carrier (Principle 16).
 */
function appendGrantCard(spec) {
  const stream = streamEl();
  if (!stream || !spec?.toolId) {
    return null;
  }
  clearEmptyState();

  let state = spec.state || "pending";
  let approvalId = spec.approvalId || "";
  let label = spec.label || spec.toolId;
  let summary = spec.summary || "";
  let scope = spec.scope || "";

  if (state === "pending" && !approvalId) {
    const req = requestTool({
      toolId: spec.toolId,
      args: spec.args || { path: "Downloads" },
    });
    if (req.status === "denied") {
      state = "denied";
      label = req.label;
      summary = req.summary;
      scope = req.scope;
    } else if (req.status === "needs_approval") {
      approvalId = req.approvalId || "";
      label = req.label;
      summary = req.summary;
      scope = req.scope;
      spec.approvalId = approvalId;
      spec.label = label;
      spec.summary = summary;
      spec.scope = scope;
    } else if (req.status === "ok") {
      state = "granted";
      label = req.label;
      summary = req.summary;
      scope = req.scope;
      spec.result = req.result;
    }
  }

  const card = document.createElement("article");
  card.className = `agent-grant-card is-${state}`;
  card.dataset.role = "grant";
  card.dataset.toolId = spec.toolId;
  card.dataset.state = state;
  card.dataset.preview = "1";
  if (approvalId) {
    card.dataset.approvalId = approvalId;
  }

  const head = document.createElement("div");
  head.className = "agent-grant-card-head";
  const title = document.createElement("span");
  title.className = "agent-grant-card-title";
  title.textContent = label;
  const badge = document.createElement("span");
  badge.className = "agent-grant-card-preview";
  badge.textContent = "preview · mock";
  head.append(title, badge);

  const body = document.createElement("p");
  body.className = "agent-grant-card-summary";
  body.textContent = summary;

  const scopeEl = document.createElement("p");
  scopeEl.className = "agent-grant-card-scope";
  scopeEl.textContent = `Scope: ${scope}`;

  card.append(head, body, scopeEl);

  if (state === "pending") {
    const actions = document.createElement("div");
    actions.className = "agent-grant-card-actions";
    const deny = document.createElement("button");
    deny.type = "button";
    deny.className = "agent-grant-btn agent-grant-btn-deny";
    deny.dataset.grantDecision = "deny";
    deny.textContent = "Deny";
    const allow = document.createElement("button");
    allow.type = "button";
    allow.className = "agent-grant-btn agent-grant-btn-allow";
    allow.dataset.grantDecision = "allow_once";
    allow.textContent = "Allow once";
    actions.append(deny, allow);
    card.append(actions);
  } else {
    const chip = document.createElement("div");
    chip.className = "agent-grant-card-chip";
    chip.textContent =
      state === "granted"
        ? "Allowed once · preview mock — no Capsule call"
        : "Denied · fail-closed";
    card.append(chip);
    if (state === "granted" && spec.result) {
      const result = document.createElement("pre");
      result.className = "agent-grant-card-result";
      result.textContent = spec.result;
      card.append(result);
    }
  }

  stream.append(card);
  syncTruthStrip();
  scrollStreamToEnd();
  return card;
}

function paintGrantCardResolved(card, outcome) {
  if (!card) {
    return;
  }
  const state = outcome.status === "ok" ? "granted" : "denied";
  card.dataset.state = state;
  card.className = `agent-grant-card is-${state}`;
  card.querySelector(".agent-grant-card-actions")?.remove();
  card.querySelector(".agent-grant-card-chip")?.remove();
  card.querySelector(".agent-grant-card-result")?.remove();
  const chip = document.createElement("div");
  chip.className = "agent-grant-card-chip";
  chip.textContent =
    state === "granted"
      ? "Allowed once · preview mock — no Capsule call"
      : "Denied · fail-closed";
  card.append(chip);
  if (state === "granted" && outcome.result) {
    const result = document.createElement("pre");
    result.className = "agent-grant-card-result";
    result.textContent = outcome.result;
    card.append(result);
  }
  syncTruthStrip();
  scrollStreamToEnd();
}

function resolveGrantFromCard(card, decision) {
  if (!card || card.dataset.state !== "pending") {
    return;
  }
  const outcome = resolveMockApproval({
    approvalId: card.dataset.approvalId,
    toolId: card.dataset.toolId,
    decision,
  });
  const session = sessions.find((s) => s.id === activeSessionId);
  const grantMsg = session?.messages?.find(
    (m) =>
      m.role === "grant" &&
      m.toolId === card.dataset.toolId &&
      (m.state === "pending" || !m.state),
  );
  if (grantMsg) {
    grantMsg.state = outcome.status === "ok" ? "granted" : "denied";
    if (outcome.result) {
      grantMsg.result = outcome.result;
    }
  }
  paintGrantCardResolved(card, outcome);
}

function sessionAlreadyHasGrant(session, toolId) {
  return Boolean(
    session?.messages?.some((m) => m.role === "grant" && m.toolId === toolId),
  );
}

function maybeOfferToolAfterReply() {
  const session = sessions.find((s) => s.id === activeSessionId);
  if (!session) {
    return;
  }
  const lastUser = [...session.messages]
    .reverse()
    .find((m) => m.role === "user");
  const text = lastUser?.text || "";
  if (wantsWalletTool(text) && !sessionAlreadyHasGrant(session, "wallet.sign")) {
    const req = requestTool({ toolId: "wallet.sign", args: {} });
    const grant = {
      role: "grant",
      toolId: "wallet.sign",
      state: req.status === "denied" ? "denied" : "pending",
      approvalId: req.approvalId,
      label: req.label,
      summary: req.summary,
      scope: req.scope,
    };
    session.messages.push(grant);
    appendGrantCard(grant);
    return;
  }
  if (wantsLibraryTool(text) && !sessionAlreadyHasGrant(session, "library.read")) {
    const req = requestTool({
      toolId: "library.read",
      args: { path: "Downloads" },
    });
    if (req.status === "needs_approval" || req.status === "denied") {
      const grant = {
        role: "grant",
        toolId: "library.read",
        state: req.status === "denied" ? "denied" : "pending",
        approvalId: req.approvalId,
        label: req.label,
        summary: req.summary,
        scope: req.scope,
        args: { path: "Downloads" },
      };
      session.messages.push(grant);
      appendGrantCard(grant);
    }
  }
}

function showEmptyState() {
  const column = streamEl();
  const viewport = streamViewportEl();
  if (!column || !viewport) {
    return;
  }
  column.replaceChildren();
  clearEmptyState();
  const name = signedInFirstName();
  const greeting = name
    ? `What's on your mind, ${name}?`
    : "What's on your mind?";
  const empty = document.createElement("div");
  empty.className = "agent-harness-empty";
  empty.setAttribute("role", "status");
  const sub =
    sessionMode === "build"
      ? "Build mode · plan & outputs theatre · no write authority yet"
      : "Private on this machine · tools start at zero";
  empty.innerHTML =
    `<p class="agent-harness-empty-greeting"></p>` +
    `<p class="agent-harness-empty-sub"></p>`;
  empty.querySelector(".agent-harness-empty-greeting").textContent = greeting;
  empty.querySelector(".agent-harness-empty-sub").textContent = sub;
  const starters = document.createElement("div");
  starters.className = "agent-starter-chips";
  for (const item of STARTER_PROMPTS) {
    const chip = document.createElement("button");
    chip.type = "button";
    chip.className = "agent-starter-chip";
    chip.dataset.starter = item.text;
    chip.textContent = item.label;
    starters.append(chip);
  }
  empty.append(starters);
  /* Viewport — not the dock-width column — so the hero sits in true room center. */
  viewport.append(empty);
}

function renderActiveSession() {
  const session = sessions.find((s) => s.id === activeSessionId);
  const stream = streamEl();
  if (!stream) {
    return;
  }
  if (!session) {
    setTitle("New chat");
    showEmptyState();
    return;
  }
  setTitle(session.title);
  stream.replaceChildren();
  if (!session.messages.length) {
    showEmptyState();
    return;
  }
  clearEmptyState();
  hydrateCapabilitiesFromSession(session);
  for (const msg of session.messages) {
    if (msg.role === "grant") {
      appendGrantCard(msg);
    } else {
      appendMessage(msg.role, msg.text);
    }
  }
  syncTruthStrip();
}

/** Re-bind mock capability map to this session’s grant messages (preview). */
function hydrateCapabilitiesFromSession(session) {
  resetMockCapabilities();
  for (const msg of session?.messages || []) {
    if (msg.role !== "grant" || !msg.toolId) {
      continue;
    }
    if (msg.state === "pending") {
      const req = requestTool({
        toolId: msg.toolId,
        args: msg.args || { path: "Downloads" },
      });
      if (req.approvalId) {
        msg.approvalId = req.approvalId;
      }
      if (req.label) {
        msg.label = req.label;
      }
      if (req.summary) {
        msg.summary = req.summary;
      }
      if (req.scope) {
        msg.scope = req.scope;
      }
      if (req.status === "denied") {
        msg.state = "denied";
      }
    } else if (msg.state === "granted" || msg.state === "denied") {
      applyCapabilityState(msg.toolId, msg.state);
    }
  }
}

function stopMockStream({ keepPartial = true } = {}) {
  clearStreamTimer();
  streamGeneration += 1;
  turnBusy = false;
  setAgentComposerProcessing(false);
  const streaming = streamEl()?.querySelector(".agent-msg-agent.is-streaming");
  if (streaming) {
    streaming.classList.remove("is-streaming");
    if (!keepPartial) {
      streaming.remove();
    } else {
      const body = streaming.querySelector(".agent-msg-body");
      if (body && body.textContent.trim()) {
        body.innerHTML = renderMarkdown(body.textContent);
        const note = document.createElement("div");
        note.className = "agent-msg-stopped";
        note.innerHTML =
          `<span>Stopped</span>` +
          `<button type="button" class="agent-msg-retry" data-retry="1">Retry</button>`;
        streaming.append(note);
      }
    }
  }
}

function startMockStream(replyText) {
  startMockStreamForPrompt("", replyText);
}

function startMockStreamForPrompt(userText, replyOverride) {
  stopMockStream({ keepPartial: true });
  document.querySelectorAll(".agent-followups").forEach((node) => node.remove());
  const turn = getMockTurn(userText);
  const thinkingText = turn.thinking;
  const replyText = replyOverride || turn.answer || MOCK_REPLY;
  const toolPreview = turn.toolPreview;
  const followUps = turn.followUps || [];
  const generation = (streamGeneration += 1);
  const thinkStartedAt = Date.now();
  turnBusy = true;
  setAgentComposerProcessing(true);
  appendTurnDivider("Turn");
  const timeline = appendTurnTimeline();
  setTimelineStep(timeline, "thinking", "running");

  const thinking = appendThinkingBlock("", {
    streaming: reasoningVisible,
    open: reasoningVisible,
  });
  const thinkBody = thinking?.querySelector(".agent-thinking-body");
  if (thinking && !reasoningVisible) {
    thinking.hidden = true;
    if (thinkBody) {
      thinkBody.textContent = thinkingText;
    }
  }

  let phase = reasoningVisible ? "thinking" : "tool";
  let thinkIndex = 0;
  let toolRow = null;
  let toolTicks = 0;
  let answerRow = null;
  let answerBody = null;
  let answerIndex = 0;

  const beginToolOrAnswer = () => {
    finishThinkingBlock(thinking, thinkStartedAt);
    setTimelineStep(timeline, "thinking", "done");
    if (toolPreview) {
      phase = "tool";
      toolTicks = 0;
      setTimelineStep(timeline, "tool", "running", toolPreview.label);
      toolRow = appendToolTimelineRow(toolPreview);
    } else {
      setTimelineStep(timeline, "tool", "idle");
      beginAnswer();
    }
  };

  const beginAnswer = () => {
    phase = "answer";
    setTimelineStep(timeline, "answer", "running");
    answerRow = appendMessage("agent", "", { streaming: true });
    answerBody = answerRow?.querySelector(".agent-msg-body");
    answerIndex = 0;
  };

  if (phase === "tool") {
    beginToolOrAnswer();
  }

  streamTimer = window.setInterval(() => {
    if (generation !== streamGeneration) {
      clearStreamTimer();
      return;
    }
    const scroller = streamScrollEl();

    if (phase === "thinking" && thinkBody) {
      thinkIndex = Math.min(thinkingText.length, thinkIndex + 3 + (thinkIndex % 2));
      thinkBody.textContent = thinkingText.slice(0, thinkIndex);
      if (scroller) {
        scroller.scrollTop = scroller.scrollHeight;
      }
      if (thinkIndex >= thinkingText.length) {
        beginToolOrAnswer();
      }
      return;
    }

    if (phase === "tool") {
      toolTicks += 1;
      if (toolTicks >= 14) {
        const denied = toolPreview?.kind === "deny";
        finishToolTimelineRow(toolRow, {
          status: denied ? "denied" : "done",
          statusLabel: denied ? "Needs ceremony" : "Ready to ask",
        });
        setTimelineStep(
          timeline,
          "tool",
          denied ? "denied" : "done",
          toolPreview?.label || "",
        );
        beginAnswer();
      }
      return;
    }

    if (!answerBody) {
      clearStreamTimer();
      turnBusy = false;
      setAgentComposerProcessing(false);
      drainFollowUpQueue();
      return;
    }

    answerIndex = Math.min(replyText.length, answerIndex + 2 + (answerIndex % 3));
    answerBody.textContent = replyText.slice(0, answerIndex);
    if (scroller) {
      scroller.scrollTop = scroller.scrollHeight;
    }
    if (answerIndex >= replyText.length) {
      clearStreamTimer();
      answerRow?.classList.remove("is-streaming");
      answerRow?.classList.add("agent-msg-enter");
      answerBody.innerHTML = renderMarkdown(replyText);
      const regen = answerRow?.querySelector("[data-regenerate]");
      if (regen) {
        regen.disabled = false;
      }
      setTimelineStep(timeline, "answer", "done");
      setTimelineStep(timeline, "done", "done");
      turnBusy = false;
      setAgentComposerProcessing(false);
      const session = sessions.find((s) => s.id === activeSessionId);
      if (session) {
        session.messages.push({
          role: "agent",
          text: replyText,
          thinking: thinkingText,
        });
      }
      noteMockTurnTokens(Math.max(200, Math.round(replyText.length / 3)));
      maybeOfferToolAfterReply();
      appendFollowUpChips(followUps);
      syncTruthStrip();
      syncWorkbenchPanels();
      scrollStreamToEnd();
      drainFollowUpQueue();
    }
  }, 18);
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

function ensureSessionForPrompt(prompt) {
  if (activeSessionId) {
    const existing = sessions.find((s) => s.id === activeSessionId);
    if (existing && existing.messages.length === 0) {
      existing.title = titleFromPrompt(prompt);
      return existing;
    }
    if (existing && existing.title === "New chat") {
      existing.title = titleFromPrompt(prompt);
      return existing;
    }
  }
  const session = {
    id: `s-${Date.now()}`,
    title: titleFromPrompt(prompt),
    group: "Today",
    messages: [],
  };
  sessions = [session, ...sessions];
  activeSessionId = session.id;
  return session;
}

export function showAgentHarness({ prompt, fromShelf = false, syncStage = true } = {}) {
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
    maybeUpdatePlanFromPrompt(prompt);
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
    void import(`./agent-shelf.js?v=${TIP}`).then((shelf) => {
      if (!shelf.agentShelfFaceActive()) {
        shelf.snapAgentShelfFace();
      }
      syncComposerGeometry();
    });
  }

  harness.hidden = false;
  harness.setAttribute("aria-hidden", "false");
  syncTruthStrip();
  syncSessionModeUi();
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
      startMockStreamForPrompt(openPrompt);
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
    void import(`./agent-shelf.js?v=${TIP}`).then((shelf) => {
      /* Reverse morph back to Home Shelf — same dance as Dock Agent leave. */
      if (shelf.agentShelfFaceActive()) {
        shelf.hideAgentShelfFace();
      } else {
        shelf.snapAppsShelfFace();
      }
    });
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
    maybeUpdatePlanFromPrompt(text);
    syncWorkbenchPanels();
    startMockStreamForPrompt(text);
    return;
  }
  showAgentHarness({ prompt: text });
}

function selectSession(sessionId) {
  if (!sessionId || !sessions.some((s) => s.id === sessionId)) {
    return;
  }
  stopMockStream({ keepPartial: true });
  activeSessionId = sessionId;
  renderSessions();
  renderActiveSession();
}

function newChat() {
  stopMockStream({ keepPartial: false });
  followUpQueue = [];
  renderFollowUpQueue();
  const session = {
    id: `s-${Date.now()}`,
    title: "New chat",
    group: "Today",
    messages: [],
  };
  sessions = [session, ...sessions];
  activeSessionId = session.id;
  renderSessions();
  renderActiveSession();
  shelfComposerInput()?.focus({ preventScroll: true });
  syncAgentSendButton();
}

function renameSession(sessionId) {
  const session = sessions.find((s) => s.id === sessionId);
  if (!session) {
    return;
  }
  const next = window.prompt("Rename chat", session.title);
  if (!next?.trim()) {
    return;
  }
  session.title = next.trim().slice(0, 64);
  if (session.id === activeSessionId) {
    setTitle(session.title);
  }
  renderSessions();
}

function deleteSession(sessionId) {
  sessions = sessions.filter((s) => s.id !== sessionId);
  if (activeSessionId === sessionId) {
    activeSessionId = sessions[0]?.id || null;
    renderActiveSession();
  }
  renderSessions();
}

export function bindAgentHarness() {
  if (bound) {
    return;
  }
  bound = true;

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
      startMockStreamForPrompt(lastUser?.text || "");
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
    const toolChip = event.target.closest?.("[data-tool-mode]");
    if (toolChip?.dataset.toolMode) {
      event.preventDefault();
      setToolMode(toolChip.dataset.toolMode);
      return;
    }
    const wbTab = event.target.closest?.("[data-workbench-tab]");
    if (wbTab?.dataset.workbenchTab) {
      event.preventDefault();
      setWorkbenchTab(wbTab.dataset.workbenchTab);
      syncWorkbenchPanels();
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
      window.alert(
        `Preview artifact: ${out.dataset.outputId}\n\nMock only — not written to disk. ADE sandbox later.`,
      );
      return;
    }
    const queueChip = event.target.closest?.(".agent-queue-chip");
    if (queueChip?.dataset.queueId) {
      event.preventDefault();
      followUpQueue = followUpQueue.filter((q) => q.id !== queueChip.dataset.queueId);
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
    const starter = event.target.closest?.("[data-starter]");
    if (starter?.dataset.starter) {
      event.preventDefault();
      sendToAgentHarness(starter.dataset.starter);
      return;
    }
    const menu = event.target.closest?.(".agent-harness-session-menu");
    if (menu) {
      event.preventDefault();
      const id = menu.closest(".agent-harness-session")?.dataset.sessionId;
      const session = sessions.find((s) => s.id === id);
      if (!session) {
        return;
      }
      const choice = window.prompt(
        'Type "pin", "export", "rename", or "delete"',
        session.pinned ? "export" : "pin",
      );
      if (choice === "delete") {
        deleteSession(id);
      } else if (choice === "rename") {
        renameSession(id);
      } else if (choice === "pin") {
        session.pinned = !session.pinned;
        renderSessions();
      } else if (choice === "export") {
        activeSessionId = id;
        exportActiveSessionMarkdown();
      }
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
