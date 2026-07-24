/* Agent Harness (preview) — Home drops, Shelf stays as composer hinge.
   UI ≠ authority: never mints grants. */

import {
  setAgentComposerProcessing,
  syncAgentSendButton,
  composerInput as shelfComposerInput,
  hideAgentShelfFace,
} from "./agent-shelf.js?v=home-20260724ai";
import {
  enableHarnessMenubarReveal,
  clearHarnessMenubarReveal,
  agentStageId,
  desktopStageId,
  getActiveStageId,
  isAgentSpace,
  setActiveStage,
} from "./shell-stages.js?v=home-20260724ai";

const TIP = "home-20260724ai";
const HOME_BREATHE_MS = 780;
const HOME_RISE_MS = 720;
const HARNESS_CONTENT_AT_MS = 180;
const PARTICLE_COUNT = 420;

const MOCK_REPLY =
  "I'm a local preview on this machine — not live inference yet.\n\n" +
  "I start with **no tools**. If you need Downloads or other capsule access, " +
  "you'll grant it explicitly (Inbox-style). Nothing ambient.\n\n" +
  "```text\nTools: none\nLocality: this device\n```";

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
        text: "Not without an explicit ceremony. Wallet tools stay fail-closed.",
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
        text: "I'd need a Library read grant first — tools start at zero.",
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
function syncComposerGeometry() {
  const taskbar = document.querySelector(".taskbar");
  const main = document.querySelector(".agent-harness-main");
  if (!taskbar || !main || !document.body.classList.contains("agent-harness-active")) {
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
 *  When live model tokens arrive: sanitize before innerHTML (seam). */
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
  const groups = ["Today", "Earlier"];
  for (const group of groups) {
    const items = sessions.filter((s) => s.group === group);
    if (!items.length) {
      continue;
    }
    const label = document.createElement("div");
    label.className = "agent-harness-group-label";
    label.textContent = group;
    list.append(label);
    for (const session of items) {
      const row = document.createElement("div");
      row.className = `agent-harness-session${session.id === activeSessionId ? " is-active" : ""}`;
      row.dataset.sessionId = session.id;

      const btn = document.createElement("button");
      btn.type = "button";
      btn.className = "agent-harness-session-btn";
      btn.textContent = session.title;
      btn.title = session.title;

      const kebab = document.createElement("button");
      kebab.type = "button";
      kebab.className = "agent-harness-session-menu";
      kebab.setAttribute("aria-label", `Session actions for ${session.title}`);
      kebab.title = "Rename or delete";
      kebab.textContent = "···";

      row.append(btn, kebab);
      list.append(row);
    }
  }
}

function appendMessage(role, text, { streaming = false, asHtml = false } = {}) {
  const stream = streamEl();
  if (!stream) {
    return null;
  }
  const empty = stream.querySelector(".agent-harness-empty");
  empty?.remove();

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
  regen.disabled = true;
  regen.title = "Regenerate — later";
  regen.textContent = "Regenerate";
  actions.append(copyBtn, regen);

  row.append(meta, body, actions);
  stream.append(row);
  scrollStreamToEnd();
  return row;
}

function showEmptyState() {
  const stream = streamEl();
  if (!stream) {
    return;
  }
  stream.replaceChildren();
  const empty = document.createElement("div");
  empty.className = "agent-harness-empty";
  empty.innerHTML =
    `<p class="agent-harness-empty-title">Private on this machine</p>` +
    `<p class="agent-harness-empty-copy">Tools start at zero. Send from the Shelf composer below.</p>`;
  stream.append(empty);
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
  for (const msg of session.messages) {
    appendMessage(msg.role, msg.text);
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
    const scroller = streamScrollEl();
    if (scroller) {
      scroller.scrollTop = scroller.scrollHeight;
    }
    if (index >= replyText.length) {
      clearStreamTimer();
      row.classList.remove("is-streaming");
      body.innerHTML = renderMarkdown(replyText);
      setAgentComposerProcessing(false);
      const session = sessions.find((s) => s.id === activeSessionId);
      if (session) {
        session.messages.push({ role: "agent", text: replyText });
      }
      /* Markdown/code blocks grow after plain-text streaming — re-pin above the Shelf. */
      scrollStreamToEnd();
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
    window.setTimeout(() => {
      if (motionGen !== harnessMotionGen || !active) {
        return;
      }
      startMockStream(MOCK_REPLY);
    }, prefersReducedMotion() ? 40 : HARNESS_CONTENT_AT_MS);
  }

  window.setTimeout(() => {
    if (motionGen !== harnessMotionGen || !active) {
      return;
    }
    document.body.classList.remove("agent-harness-dropping");
    document.body.classList.add("agent-harness-settled");
    enableHarnessMenubarReveal();
    syncComposerGeometry();
  }, prefersReducedMotion() ? 40 : HOME_BREATHE_MS);

  if (!fromShelf) {
    shelfComposerInput()?.focus({ preventScroll: true });
  }
}

export function hideAgentHarness({ restoreShelfApps = true, syncStage = true } = {}) {
  if (!active && !document.body.classList.contains("agent-harness-active")) {
    return;
  }
  const motionGen = (harnessMotionGen += 1);
  stopMockStream({ keepPartial: true });
  stopParticles();
  active = false;

  const harness = harnessEl();
  harness?.classList.remove("is-visible");
  clearHarnessMenubarReveal();
  document.body.classList.remove("agent-harness-settled", "agent-harness-dropping");
  document.body.classList.add("agent-harness-rising");

  /* Leave Agent Space for Desktop (Agent stays in MC). Shelf owns reverse morph. */
  if (syncStage && isAgentSpace(getActiveStageId())) {
    setActiveStage(desktopStageId(), {
      announce: false,
      focus: false,
      animate: false,
      syncHarness: false,
    });
  }

  const finish = () => {
    if (motionGen !== harnessMotionGen) {
      return;
    }
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
        /* Reverse morph back to Apps — same dance as Dock Agent leave. */
        if (shelf.agentShelfFaceActive()) {
          shelf.hideAgentShelfFace();
        } else {
          shelf.snapAppsShelfFace();
        }
      });
    }
  };

  if (prefersReducedMotion()) {
    finish();
    return;
  }
  window.setTimeout(finish, HOME_RISE_MS);
}

export function stopAgentHarnessStream() {
  stopMockStream({ keepPartial: true });
}

export function sendToAgentHarness(prompt) {
  const text = String(prompt || "").trim();
  if (!text) {
    if (active) {
      stopMockStream({ keepPartial: true });
    }
    return;
  }
  if (active) {
    stopMockStream({ keepPartial: true });
    const session = ensureSessionForPrompt(text);
    if (session.title === "New chat" || session.messages.length === 0) {
      session.title = titleFromPrompt(text);
    }
    session.messages.push({ role: "user", text });
    renderSessions();
    setTitle(session.title);
    const empty = streamEl()?.querySelector(".agent-harness-empty");
    empty?.remove();
    appendMessage("user", text);
    startMockStream(MOCK_REPLY);
    return;
  }
  showAgentHarness({ prompt: text });
}

function newChat() {
  stopMockStream({ keepPartial: false });
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

  /* Esc owned by Shelf (`hideAgentShelfFace`) — one reverse dance, not harness-then-shelf. */

  document.addEventListener("click", (event) => {
    if (event.target.closest?.("#agent-harness-home")) {
      event.preventDefault();
      hideAgentShelfFace();
      return;
    }
    if (event.target.closest?.("#agent-harness-new-chat")) {
      event.preventDefault();
      newChat();
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
    if (event.target.closest?.("[data-retry]")) {
      event.preventDefault();
      event.target.closest(".agent-msg-stopped")?.remove();
      startMockStream(MOCK_REPLY);
      return;
    }
    const sessionBtn = event.target.closest?.(".agent-harness-session-btn");
    if (sessionBtn) {
      event.preventDefault();
      stopMockStream({ keepPartial: true });
      activeSessionId = sessionBtn.closest(".agent-harness-session")?.dataset.sessionId || null;
      renderSessions();
      renderActiveSession();
      return;
    }
    const menu = event.target.closest?.(".agent-harness-session-menu");
    if (menu) {
      event.preventDefault();
      const id = menu.closest(".agent-harness-session")?.dataset.sessionId;
      if (!id) {
        return;
      }
      const choice = window.prompt('Type "rename" or "delete"', "rename");
      if (choice === "delete") {
        deleteSession(id);
      } else if (choice === "rename" || choice === null) {
        if (choice === "rename") {
          renameSession(id);
        }
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
