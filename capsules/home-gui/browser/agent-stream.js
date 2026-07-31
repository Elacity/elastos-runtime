/* Agent message stream + turn theatre.
   Bound from agent-harness.js. Tip: home-20260728ag
   w1: live local chat_completions when the probe says live (one-shot reply,
   progressive reveal — providers have no token stream, §AL.2); mock remains
   the honest fallback. UI ≠ authority (Principle 16) — chat carries no tool
   or grant power either way. */

import {
  MOCK_REPLY,
  getMockTurn,
  noteMockTurnTokens,
  firstReasoningField,
  splitThinkTaggedContent,
} from "./mock-agent-provider.js?v=home-20260728ag";
import {
  getLiveInferenceState,
  probeLiveInference,
  buildLiveMessages,
  requestLiveChatCompletion,
} from "./agent-live.js?v=home-20260728ag";
import { setAgentComposerProcessing } from "./agent-shelf.js?v=home-20260728ag";
import {
  maybeOfferToolAfterReply,
  syncTruthStrip,
  appendGrantCard,
  hydrateCapabilitiesFromSession,
} from "./agent-grants.js?v=home-20260728ag";
import { syncWorkbenchPanels } from "./agent-configure.js?v=home-20260728ag";

/** @type {null | object} */
let ctx = null;
/** @type {null | Record<string, Function>} */
let host = null;

export function bindAgentStream(nextCtx, nextHost = {}) {
  ctx = nextCtx;
  host = nextHost;
}

export function clearStreamTimer() {
  if (ctx.streamTimer) {
    window.clearInterval(ctx.streamTimer);
    window.cancelAnimationFrame(ctx.streamTimer);
    ctx.streamTimer = 0;
  }
}

export function titleFromPrompt(prompt) {
  const cleaned = prompt.replace(/\s+/g, " ").trim();
  if (!cleaned) {
    return "New chat";
  }
  return cleaned.length > 42 ? `${cleaned.slice(0, 41)}…` : cleaned;
}

export function escapeHtml(text) {
  return String(text)
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;");
}

/** Tiny markdown for MOCK_REPLY only — escapeHtml on fences/inlines.
 *  SEAM (live model / Carrier-backed replies): sanitize or use a text-safe
 *  path before innerHTML. UI must never treat model HTML as authority. */
export function renderMarkdown(text) {
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

export function setTitle(title) {
  const el = host.titleEl();
  if (el) {
    el.textContent = title;
  }
}


export function finishThinkingBlock(details, startedAt) {
  if (!details) {
    return;
  }
  details.classList.remove("is-streaming");
  details.classList.add("is-complete");
  const ms = Math.max(400, Date.now() - (startedAt || Date.now()));
  const sec = (ms / 1000).toFixed(ms >= 10000 ? 0 : 1);
  const label = details.querySelector(".agent-thinking-label");
  if (label) {
    label.textContent = `Thought for ${sec}s`;
  }
  /* Collapse after a beat — frontier-style; honor reduced motion by skipping delay animation only. */
  if (details.open && ctx.reasoningVisible) {
    const collapse = () => {
      if (details.isConnected && ctx.reasoningVisible) {
        details.open = false;
      }
    };
    if (host.prefersReducedMotion()) {
      collapse();
    } else {
      window.setTimeout(collapse, 900);
    }
  }
}

export function appendToolTimelineRow(tool) {
  const stream = host.streamEl();
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
  host.scrollStreamToEnd();
  return row;
}


export function appendFollowUpChips(prompts) {
  const stream = host.streamEl();
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
  host.scrollStreamToEnd();
  return root;
}

export function renderFollowUpQueue() {
  const root = document.querySelector("[data-agent-queue]");
  if (!root) {
    return;
  }
  root.replaceChildren();
  root.hidden = ctx.followUpQueue.length === 0;
  if (!ctx.followUpQueue.length) {
    requestAnimationFrame(syncComposerGeometry);
    return;
  }
  for (const item of ctx.followUpQueue) {
    const row = document.createElement("div");
    row.className = "agent-queue-item";
    row.dataset.queueId = item.id;
    const text = document.createElement("span");
    text.className = "agent-queue-item-text";
    text.textContent = item.text;
    text.title = item.text;
    const remove = document.createElement("button");
    remove.type = "button";
    remove.className = "agent-queue-item-remove";
    remove.dataset.queueId = item.id;
    remove.setAttribute("aria-label", "Remove from queue");
    remove.title = "Remove from queue";
    remove.innerHTML = `<span aria-hidden="true">×</span>`;
    row.append(text, remove);
    root.append(row);
  }
  /* Dock grew — remeasure stream clearance under the taller Shelf. */
  requestAnimationFrame(syncComposerGeometry);
}

export function enqueueFollowUp(text) {
  ctx.followUpQueue.push({ id: `q-${Date.now()}-${ctx.followUpQueue.length}`, text });
  renderFollowUpQueue();
}

export function drainFollowUpQueue() {
  if (!ctx.followUpQueue.length || !ctx.active) {
    return;
  }
  const next = ctx.followUpQueue.shift();
  renderFollowUpQueue();
  if (!next?.text) {
    return;
  }
  const session = ctx.sessions.find((s) => s.id === ctx.activeSessionId) || host.ensureSessionForPrompt(next.text);
  session.messages.push({ role: "user", text: next.text });
  host.renderSessions();
  appendMessage("user", next.text);
  startTurnForPrompt(next.text);
}


/**
 * Inbox-grammar grant card (preview mock). Deny / Allow once update mock
 * state only — never Capsule/Carrier (Principle 16).
 */





export function showEmptyState() {
  const column = host.streamEl();
  const viewport = host.streamViewportEl();
  if (!column || !viewport) {
    return;
  }
  column.replaceChildren();
  host.clearEmptyState();
  const name = host.signedInFirstName();
  const greeting = name
    ? `What's on your mind, ${name}?`
    : "What's on your mind?";
  const empty = document.createElement("div");
  empty.className = "agent-harness-empty";
  empty.setAttribute("role", "status");
  const sub =
    ctx.sessionMode === "build"
      ? "Build mode · plan & outputs theatre · no write authority yet"
      : "Private on this machine · tools start at zero · grants ask once";
  const teach =
    "Preview path · not live inference yet · Deny / Allow once never mint Capsule power";
  empty.innerHTML =
    `<p class="agent-harness-empty-greeting"></p>` +
    `<p class="agent-harness-empty-sub"></p>` +
    `<p class="agent-harness-empty-teach"></p>`;
  empty.querySelector(".agent-harness-empty-greeting").textContent = greeting;
  empty.querySelector(".agent-harness-empty-sub").textContent = sub;
  empty.querySelector(".agent-harness-empty-teach").textContent = teach;
  /* Viewport — not the dock-width column — so the hero sits in true room center. */
  viewport.append(empty);
}

export function renderActiveSession() {
  const session = ctx.sessions.find((s) => s.id === ctx.activeSessionId);
  const stream = host.streamEl();
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
  host.clearEmptyState();
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


export function appendThinkingBlock(text, { streaming = false, open = true } = {}) {
  const stream = host.streamEl();
  if (!stream) {
    return null;
  }
  host.clearEmptyState();
  const details = document.createElement("details");
  details.className = `agent-thinking${streaming ? " is-streaming" : ""}`;
  details.dataset.block = "thinking";
  details.dataset.startedAt = String(Date.now());
  if (open && ctx.reasoningVisible) {
    details.open = true;
  }
  const summary = document.createElement("summary");
  summary.className = "agent-thinking-summary";
  summary.title = "Preview reasoning — not authority";
  summary.innerHTML =
    `<span class="agent-thinking-chevron" aria-hidden="true"></span>` +
    `<span class="agent-thinking-label">Thinking</span>`;
  const bodyWrap = document.createElement("div");
  bodyWrap.className = "agent-thinking-body-wrap";
  const body = document.createElement("pre");
  body.className = "agent-thinking-body";
  body.textContent = text;
  bodyWrap.append(body);
  details.append(summary, bodyWrap);
  stream.append(details);
  host.scrollStreamToEnd();
  return details;
}

export function finishToolTimelineRow(row, { status = "done", statusLabel = "Done" } = {}) {
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

export function appendMessage(role, text, { streaming = false, asHtml = false } = {}) {
  const stream = host.streamEl();
  if (!stream) {
    return null;
  }
  host.clearEmptyState();

  const row = document.createElement("div");
  row.className = `agent-msg agent-msg-${role}${streaming ? " is-streaming" : ""}`;
  row.dataset.role = role;

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
  regen.title = role === "agent" ? "Regenerate reply" : "Regenerate — agent only";
  regen.textContent = "Regenerate";
  actions.append(copyBtn, regen);

  if (!streaming) {
    row.classList.add("agent-msg-enter");
  }
  row.append(body, actions);
  stream.append(row);
  host.scrollStreamToEnd();
  return row;
}

export function stopMockStream({ keepPartial = true } = {}) {
  clearStreamTimer();
  ctx.streamGeneration += 1;
  ctx.turnBusy = false;
  setAgentComposerProcessing(false);
  const streaming = host.streamEl()?.querySelector(".agent-msg-agent.is-streaming");
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

export function startMockStream(replyText) {
  startMockStreamForPrompt("", replyText);
}

/** One decision point (one canonical path): live when the probe says live,
 *  otherwise the honest mock theatre. All turn starters route through here. */
export function startTurnForPrompt(userText) {
  if (getLiveInferenceState().live) {
    void startLiveTurnForPrompt(userText);
    return;
  }
  startMockStreamForPrompt(userText);
}

/** w1 live turn: fetch the full local completion, then reveal it through the
 *  same theatre. Any failure falls back to mock with preview labels intact. */
async function startLiveTurnForPrompt(userText) {
  stopMockStream({ keepPartial: true });
  document.querySelectorAll(".agent-followups").forEach((node) => node.remove());
  const generation = (ctx.streamGeneration += 1);
  ctx.turnBusy = true;
  setAgentComposerProcessing(true);

  /* Honest wait shimmer while the local model computes the full reply. */
  const placeholder = appendThinkingBlock("", {
    streaming: true,
    open: false,
  });

  const session = ctx.sessions.find((s) => s.id === ctx.activeSessionId);
  let turn = null;
  try {
    const { message, usage } = await requestLiveChatCompletion(
      buildLiveMessages(session?.messages || []),
    );
    if (generation !== ctx.streamGeneration) {
      placeholder?.remove();
      return;
    }
    /* fx13: normalize reasoning_content / think-tags into the Thinking UI. */
    const reasoned = firstReasoningField(message);
    const split = splitThinkTaggedContent(String(message?.content || ""));
    turn = {
      thinking: reasoned || split.thinking,
      answer: split.answer,
      approxTokens: usage?.completion_tokens || null,
    };
  } catch {
    if (generation !== ctx.streamGeneration) {
      placeholder?.remove();
      return;
    }
  }
  placeholder?.remove();

  if (!turn || !turn.answer) {
    /* Model unreachable or empty — re-probe so status copy flips honestly,
       then fall back to the labeled mock path. */
    void probeLiveInference({ force: true }).then(() => host.syncInferenceStatus?.());
    startMockStreamForPrompt(userText);
    return;
  }
  startTurnReveal({
    thinkingText: turn.thinking,
    replyText: turn.answer,
    toolPreview: null,
    followUps: [],
    approxTokens: turn.approxTokens,
    live: true,
  });
}

export function startMockStreamForPrompt(userText, replyOverride) {
  const turn = getMockTurn(userText);
  startTurnReveal({
    thinkingText: turn.thinking,
    replyText: replyOverride || turn.answer || MOCK_REPLY,
    toolPreview: turn.toolPreview,
    followUps: turn.followUps || [],
    approxTokens: null,
    live: false,
  });
}

function startTurnReveal({
  thinkingText,
  replyText,
  toolPreview,
  followUps,
  approxTokens,
  live,
}) {
  stopMockStream({ keepPartial: true });
  document.querySelectorAll(".agent-followups").forEach((node) => node.remove());
  const generation = (ctx.streamGeneration += 1);
  const thinkStartedAt = Date.now();
  ctx.turnBusy = true;
  setAgentComposerProcessing(true);

  /* Live turns without reasoning content get no thinking block — never
     theatre an empty "Thought for…" over a real reply (§AL.3 honesty). */
  const showThinking = !live || Boolean(thinkingText);
  const thinking = showThinking
    ? appendThinkingBlock("", {
        streaming: ctx.reasoningVisible,
        open: ctx.reasoningVisible,
      })
    : null;
  const thinkBody = thinking?.querySelector(".agent-thinking-body");
  if (thinking && !ctx.reasoningVisible) {
    thinking.hidden = true;
    if (thinkBody) {
      thinkBody.textContent = thinkingText;
    }
  }

  let phase = showThinking && ctx.reasoningVisible ? "thinking" : "tool";
  let thinkIndex = 0;
  let toolRow = null;
  let toolTicks = 0;
  let answerRow = null;
  let answerBody = null;
  let answerIndex = 0;

  const beginToolOrAnswer = () => {
    finishThinkingBlock(thinking, thinkStartedAt);
    if (toolPreview) {
      phase = "tool";
      toolTicks = 0;
      toolRow = appendToolTimelineRow(toolPreview);
    } else {
      beginAnswer();
    }
  };

  const beginAnswer = () => {
    phase = "answer";
    answerRow = appendMessage("agent", "", { streaming: true });
    answerBody = answerRow?.querySelector(".agent-msg-body");
    answerIndex = 0;
  };

  if (phase === "tool") {
    beginToolOrAnswer();
  }

  ctx.streamTimer = window.setInterval(() => {
    if (generation !== ctx.streamGeneration) {
      clearStreamTimer();
      return;
    }
    const scroller = host.streamScrollEl();

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
        beginAnswer();
      }
      return;
    }

    if (!answerBody) {
      clearStreamTimer();
      ctx.turnBusy = false;
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
      ctx.turnBusy = false;
      setAgentComposerProcessing(false);
      const session = ctx.sessions.find((s) => s.id === ctx.activeSessionId);
      if (session) {
        session.messages.push({
          role: "agent",
          text: replyText,
          thinking: thinkingText,
          ...(live ? { live: true } : {}),
        });
      }
      noteMockTurnTokens(
        approxTokens || Math.max(200, Math.round(replyText.length / 3)),
      );
      maybeOfferToolAfterReply();
      appendFollowUpChips(followUps);
      syncTruthStrip();
      syncWorkbenchPanels();
      host.scrollStreamToEnd();
      drainFollowUpQueue();
    }
  }, 33);
}
