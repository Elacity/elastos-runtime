/* Agent message stream + turn theatre.
   Bound from agent-harness.js. Tip: home-20260804ap
   Live: gateway SSE (/api/apps/home/agent/chat/stream) with AbortController
   stop; mock remains the honest fallback when Live is down.
   UI ≠ authority (Principle 16) — chat carries no tool or grant power.
   Wave 1: edit/resubmit, per-msg delete, markdown, stream status, Stop persist. */

import {
  MOCK_REPLY,
  getMockTurn,
  noteMockTurnTokens,
  splitThinkTaggedContent,
} from "./mock-agent-provider.js?v=home-20260804ap";
import {
  getLiveInferenceState,
  probeLiveInference,
  buildLiveMessages,
  streamLiveChatCompletion,
  abortLiveChatStream,
} from "./agent-live.js?v=home-20260804ap";
import { setAgentComposerProcessing } from "./agent-shelf.js?v=home-20260804ap";
import {
  maybeOfferToolAfterReply,
  syncTruthStrip,
  appendGrantCard,
  hydrateCapabilitiesFromSession,
} from "./agent-grants.js?v=home-20260804ap";
import { syncWorkbenchPanels } from "./agent-configure.js?v=home-20260804ap";

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

/** GFM-safe markdown subset — escape first, then format.
 *  Tables, lists, links, headings, fences, inline code/bold/italic.
 *  UI must never treat model HTML as authority (Principle 16).
 *  `streaming: true` virtually closes an open fence so mid-reply code blocks
 *  paint as code instead of raw ``` until the model finishes the fence. */
export function renderMarkdown(text, { streaming = false } = {}) {
  let source = String(text ?? "");
  if (streaming) {
    const fenceMarks = source.split("```").length - 1;
    if (fenceMarks % 2 === 1) {
      source += "\n```";
    }
  }
  const parts = source.split(/```([\s\S]*?)```/g);
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
    html += renderMarkdownBlocks(parts[i]);
  }
  return html;
}

/** Keep raw markdown on the node so Stop/Copy don't lose markers after HTML paint. */
function paintAgentMessageBody(body, text, { streaming = false } = {}) {
  if (!body) {
    return;
  }
  const raw = String(text ?? "");
  body.dataset.mdSource = raw;
  body.innerHTML = renderMarkdown(raw, { streaming });
}

function formatInlineMarkdown(escaped) {
  return escaped
    .replaceAll(/\[([^\]]+)\]\((https?:\/\/[^)\s]+)\)/g, (_, label, href) => {
      return `<a class="agent-md-a" href="${href}" target="_blank" rel="noopener noreferrer">${label}</a>`;
    })
    .replaceAll(/\*\*(.+?)\*\*/g, "<strong>$1</strong>")
    .replaceAll(/\*([^*\n]+)\*/g, "<em>$1</em>")
    .replaceAll(/`([^`]+)`/g, '<code class="agent-md-inline">$1</code>');
}

function renderMarkdownBlocks(raw) {
  const lines = String(raw).replace(/\r\n/g, "\n").split("\n");
  let html = "";
  let i = 0;
  const flushParagraph = (buf) => {
    const trimmed = buf.join("\n").trim();
    if (!trimmed) {
      return;
    }
    html += `<p class="agent-md-p">${formatInlineMarkdown(escapeHtml(trimmed)).replaceAll("\n", "<br>")}</p>`;
  };
  while (i < lines.length) {
    const line = lines[i];
    if (!line.trim()) {
      i += 1;
      continue;
    }
    /* Table: header | --- | rows */
    if (
      line.includes("|") &&
      i + 1 < lines.length &&
      /^\s*\|?[\s:-|]+\|?\s*$/.test(lines[i + 1])
    ) {
      const rows = [];
      while (i < lines.length && lines[i].includes("|")) {
        rows.push(lines[i]);
        i += 1;
        if (rows.length === 1 && i < lines.length && /^\s*\|?[\s:-|]+\|?\s*$/.test(lines[i])) {
          i += 1; /* skip separator */
        } else if (rows.length > 1 && (!lines[i] || !lines[i].includes("|"))) {
          break;
        }
      }
      if (rows.length) {
        const cells = (row) =>
          row
            .trim()
            .replace(/^\|/, "")
            .replace(/\|$/, "")
            .split("|")
            .map((c) => formatInlineMarkdown(escapeHtml(c.trim())));
        const head = cells(rows[0]);
        html += `<div class="agent-md-table-wrap"><table class="agent-md-table"><thead><tr>${head
          .map((c) => `<th>${c}</th>`)
          .join("")}</tr></thead><tbody>`;
        for (const row of rows.slice(1)) {
          html += `<tr>${cells(row)
            .map((c) => `<td>${c}</td>`)
            .join("")}</tr>`;
        }
        html += `</tbody></table></div>`;
        continue;
      }
    }
    const heading = /^(#{1,3})\s+(.+)$/.exec(line.trim());
    if (heading) {
      const level = heading[1].length;
      html += `<h${level} class="agent-md-h agent-md-h${level}">${formatInlineMarkdown(
        escapeHtml(heading[2]),
      )}</h${level}>`;
      i += 1;
      continue;
    }
    if (/^\s*([-*+]|\d+\.)\s+/.test(line)) {
      const ordered = /^\s*\d+\.\s+/.test(line);
      const tag = ordered ? "ol" : "ul";
      html += `<${tag} class="agent-md-list">`;
      while (i < lines.length && /^\s*([-*+]|\d+\.)\s+/.test(lines[i])) {
        const item = lines[i].replace(/^\s*([-*+]|\d+\.)\s+/, "");
        html += `<li>${formatInlineMarkdown(escapeHtml(item))}</li>`;
        i += 1;
      }
      html += `</${tag}>`;
      continue;
    }
    if (/^>\s?/.test(line.trim())) {
      const quote = [];
      while (i < lines.length && /^>\s?/.test(lines[i].trim())) {
        quote.push(lines[i].replace(/^\s*>\s?/, ""));
        i += 1;
      }
      html += `<blockquote class="agent-md-quote">${formatInlineMarkdown(
        escapeHtml(quote.join("\n")),
      ).replaceAll("\n", "<br>")}</blockquote>`;
      continue;
    }
    const para = [];
    while (
      i < lines.length &&
      lines[i].trim() &&
      !lines[i].includes("|") &&
      !/^(#{1,3})\s+/.test(lines[i].trim()) &&
      !/^\s*([-*+]|\d+\.)\s+/.test(lines[i]) &&
      !/^>\s?/.test(lines[i].trim())
    ) {
      para.push(lines[i]);
      i += 1;
    }
    flushParagraph(para);
  }
  return html;
}

export function formatStreamError(err) {
  const code = err?.code || "";
  if (code === "aborted") {
    return "Stopped";
  }
  if (code === "missing-home-launch-token") {
    return "Live unavailable — Home launch token missing (unlock Home and reopen Agent)";
  }
  if (code === "upstream_http") {
    const status = err?.status ? ` (${err.status})` : "";
    return `Live upstream error${status} — check Sparks / OLLAMA_URL, then retry`;
  }
  if (code === "no_body") {
    return "Live stream had no body — gateway or upstream misconfigured";
  }
  if (String(err?.name || "") === "TimeoutError" || /timeout/i.test(String(err?.message || ""))) {
    return "Live timed out — retry, or switch pair A/B";
  }
  return err?.message
    ? `Live failed: ${String(err.message).slice(0, 160)}`
    : "Live failed — falling back to labeled Preview mock";
}

export function setStreamStatus(label, { tone = "idle" } = {}) {
  const el = document.querySelector("[data-agent-stream-status]");
  if (!el) {
    return;
  }
  const text = String(label || "").trim();
  el.hidden = !text;
  el.dataset.tone = tone;
  el.textContent = text;
}

export function ensureJumpToLatest() {
  let btn = document.querySelector("[data-agent-jump-latest]");
  if (btn) {
    return btn;
  }
  const viewport = host.streamViewportEl?.() || document.querySelector(".agent-harness-stream-viewport");
  if (!viewport) {
    return null;
  }
  btn = document.createElement("button");
  btn.type = "button";
  btn.className = "agent-jump-latest";
  btn.dataset.agentJumpLatest = "1";
  btn.hidden = true;
  btn.textContent = "Jump to latest";
  viewport.append(btn);
  return btn;
}

export function updateJumpToLatestVisibility() {
  const scroller = host.streamScrollEl?.();
  const btn = ensureJumpToLatest();
  if (!scroller || !btn) {
    return;
  }
  const distance = scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight;
  btn.hidden = distance < 96 || !ctx?.turnBusy && distance < 160;
  /* Show whenever user is meaningfully above bottom during/after turns. */
  btn.hidden = distance < 120;
}

export function persistPartialAgentReply(text, thinking = "") {
  const session = ctx.sessions.find((s) => s.id === ctx.activeSessionId);
  const answer = String(text || "").trim();
  if (!session || !answer) {
    return;
  }
  const last = session.messages[session.messages.length - 1];
  if (last?.role === "agent" && last.text === answer) {
    return;
  }
  session.messages.push(
    thinking
      ? { role: "agent", text: answer, thinking, partial: true }
      : { role: "agent", text: answer, partial: true },
  );
  session.updatedAt = Date.now();
  host.renderSessions?.();
  try {
    host.persistAgentWorkspaceSoon?.();
  } catch {
    /* optional */
  }
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
    requestAnimationFrame(() => host.syncComposerGeometry?.());
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
  requestAnimationFrame(() => host.syncComposerGeometry?.());
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
  const teach = getLiveInferenceState().live
    ? "Live on this Home · tools still ask once · Deny / Allow never mint Capsule power"
    : "Preview path when Live is down · Deny / Allow once never mint Capsule power";
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

function clearVisibleTranscript() {
  host.clearEmptyState?.();
  document.querySelectorAll(".agent-followups").forEach((node) => node.remove());
  const column = host.streamEl?.();
  const scroll = host.streamScrollEl?.();
  if (column) {
    column.replaceChildren();
  }
  /* Live/mock turns must never leave sibling nodes beside the column — those
     survive column.replaceChildren() and make New chat look like a no-op. */
  if (scroll && scroll !== column) {
    for (const node of [...scroll.children]) {
      if (node === column || node.id === "agent-harness-stream-column") {
        continue;
      }
      node.remove();
    }
  }
}

export function renderActiveSession() {
  const session = ctx.sessions.find((s) => s.id === ctx.activeSessionId);
  const stream = host.streamEl();
  if (!stream) {
    return;
  }
  if (!session) {
    setTitle("New chat");
    clearVisibleTranscript();
    showEmptyState();
    return;
  }
  setTitle(session.title);
  clearVisibleTranscript();
  if (!session.messages.length) {
    showEmptyState();
    return;
  }
  host.clearEmptyState();
  hydrateCapabilitiesFromSession(session);
  session.messages.forEach((msg, index) => {
    if (msg.role === "grant") {
      appendGrantCard(msg);
    } else {
      appendMessage(msg.role, msg.text, { msgIndex: index });
    }
  });
  syncTruthStrip();
  setStreamStatus("");
  updateJumpToLatestVisibility();
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

export function appendMessage(
  role,
  text,
  { streaming = false, asHtml = false, msgIndex = null } = {},
) {
  const stream = host.streamEl();
  if (!stream) {
    return null;
  }
  host.clearEmptyState();

  const row = document.createElement("div");
  row.className = `agent-msg agent-msg-${role}${streaming ? " is-streaming" : ""}`;
  row.dataset.role = role;
  if (msgIndex != null) {
    row.dataset.msgIndex = String(msgIndex);
  }

  const body = document.createElement("div");
  body.className = "agent-msg-body";
  if (asHtml) {
    body.innerHTML = text;
  } else if (role === "agent") {
    paintAgentMessageBody(body, text, { streaming });
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
  actions.append(copyBtn);
  if (role === "user" && !streaming) {
    const edit = document.createElement("button");
    edit.type = "button";
    edit.className = "agent-msg-action";
    edit.dataset.editMessage = "1";
    edit.textContent = "Edit";
    edit.title = "Edit and resubmit";
    edit.disabled = Boolean(ctx.turnBusy);
    actions.append(edit);
  }
  if (role === "agent") {
    const regen = document.createElement("button");
    regen.type = "button";
    regen.className = "agent-msg-action";
    regen.dataset.regenerate = "1";
    regen.disabled = streaming || Boolean(ctx.turnBusy);
    regen.title = "Regenerate reply";
    regen.textContent = "Regenerate";
    actions.append(regen);
  }
  if (!streaming) {
    const del = document.createElement("button");
    del.type = "button";
    del.className = "agent-msg-action is-danger";
    del.dataset.deleteMessage = "1";
    del.textContent = "Delete";
    del.title = "Delete message";
    del.disabled = Boolean(ctx.turnBusy);
    actions.append(del);
  }

  if (!streaming) {
    row.classList.add("agent-msg-enter");
  }
  row.append(body, actions);
  stream.append(row);
  host.scrollStreamToEnd();
  updateJumpToLatestVisibility();
  return row;
}

export function deleteMessageAt(msgIndex) {
  const session = ctx.sessions.find((s) => s.id === ctx.activeSessionId);
  if (!session || ctx.turnBusy) {
    return false;
  }
  const index = Number(msgIndex);
  if (!Number.isInteger(index) || index < 0 || index >= session.messages.length) {
    return false;
  }
  if (!window.confirm("Delete this message from the chat?")) {
    return false;
  }
  session.messages.splice(index, 1);
  session.updatedAt = Date.now();
  host.renderSessions?.();
  renderActiveSession();
  try {
    host.persistAgentWorkspaceSoon?.();
  } catch {
    /* optional */
  }
  return true;
}

export function beginEditUserMessage(msgIndex) {
  const session = ctx.sessions.find((s) => s.id === ctx.activeSessionId);
  if (!session || ctx.turnBusy) {
    return false;
  }
  const index = Number(msgIndex);
  const msg = session.messages[index];
  if (!msg || msg.role !== "user") {
    return false;
  }
  const row = host.streamEl()?.querySelector(`.agent-msg-user[data-msg-index="${index}"]`);
  if (!row || row.querySelector("[data-edit-form]")) {
    return false;
  }
  const body = row.querySelector(".agent-msg-body");
  const actions = row.querySelector(".agent-msg-actions");
  if (!body) {
    return false;
  }
  if (actions) {
    actions.hidden = true;
  }
  const form = document.createElement("form");
  form.className = "agent-msg-edit-form";
  form.dataset.editForm = "1";
  const ta = document.createElement("textarea");
  ta.className = "agent-msg-edit-input";
  ta.value = msg.text;
  ta.rows = Math.min(8, Math.max(2, msg.text.split("\n").length + 1));
  const bar = document.createElement("div");
  bar.className = "agent-msg-edit-bar";
  const cancel = document.createElement("button");
  cancel.type = "button";
  cancel.className = "agent-msg-action";
  cancel.dataset.editCancel = "1";
  cancel.textContent = "Cancel";
  const save = document.createElement("button");
  save.type = "submit";
  save.className = "agent-msg-action";
  save.textContent = "Save & submit";
  bar.append(cancel, save);
  form.append(ta, bar);
  body.replaceWith(form);
  ta.focus();
  ta.setSelectionRange(ta.value.length, ta.value.length);
  return true;
}

export function cancelEditUserMessage(msgIndex) {
  renderActiveSession();
  return true;
}

export function submitEditUserMessage(msgIndex, nextText) {
  const session = ctx.sessions.find((s) => s.id === ctx.activeSessionId);
  const text = String(nextText || "").trim();
  if (!session || !text || ctx.turnBusy) {
    return false;
  }
  const index = Number(msgIndex);
  const msg = session.messages[index];
  if (!msg || msg.role !== "user") {
    return false;
  }
  session.messages = session.messages.slice(0, index);
  session.messages.push({ role: "user", text });
  if (session.title === "New chat" || index === 0) {
    session.title = titleFromPrompt(text);
  }
  session.updatedAt = Date.now();
  host.renderSessions?.();
  renderActiveSession();
  try {
    host.persistAgentWorkspaceSoon?.();
  } catch {
    /* optional */
  }
  startTurnForPrompt(text);
  return true;
}

/** Drop trailing agent turns after the last user message, then restream. */
export function regenerateLastAgentTurn() {
  const session = ctx.sessions.find((s) => s.id === ctx.activeSessionId);
  if (!session || ctx.turnBusy) {
    return false;
  }
  let lastUser = -1;
  for (let i = session.messages.length - 1; i >= 0; i -= 1) {
    if (session.messages[i].role === "user") {
      lastUser = i;
      break;
    }
  }
  if (lastUser < 0) {
    return false;
  }
  const prompt = session.messages[lastUser].text;
  session.messages = session.messages.slice(0, lastUser + 1);
  session.updatedAt = Date.now();
  host.renderSessions?.();
  renderActiveSession();
  try {
    host.persistAgentWorkspaceSoon?.();
  } catch {
    /* optional */
  }
  startTurnForPrompt(prompt);
  return true;
}

export function stopMockStream({ keepPartial = true, drainQueue = false } = {}) {
  abortLiveChatStream();
  clearStreamTimer();
  ctx.streamGeneration += 1;
  ctx.turnBusy = false;
  setAgentComposerProcessing(false);
  setStreamStatus("");
  const thinking = host.streamEl()?.querySelector(".agent-thinking.is-streaming");
  let thinkingText = "";
  if (thinking) {
    thinkingText = thinking.querySelector(".agent-thinking-body")?.textContent || "";
    finishThinkingBlock(thinking, Number(thinking.dataset.startedAt) || Date.now());
  }
  const streaming = host.streamEl()?.querySelector(".agent-msg-agent.is-streaming");
  if (streaming) {
    streaming.classList.remove("is-streaming");
    if (!keepPartial) {
      streaming.remove();
      thinking?.remove();
    } else {
      const body = streaming.querySelector(".agent-msg-body");
      const raw = String(body?.dataset.mdSource || body?.textContent || "").trim();
      if (body && raw) {
        paintAgentMessageBody(body, raw, { streaming: false });
        const regen = streaming.querySelector("[data-regenerate]");
        if (regen) {
          regen.disabled = false;
        }
        persistPartialAgentReply(raw, thinkingText);
        if (!streaming.querySelector(".agent-msg-stopped")) {
          const note = document.createElement("div");
          note.className = "agent-msg-stopped";
          note.innerHTML =
            `<span>Stopped</span>` +
            `<button type="button" class="agent-msg-retry" data-retry="1">Retry</button>`;
          streaming.append(note);
        }
      } else {
        streaming.remove();
        if (!thinkingText.trim()) {
          thinking?.remove();
        }
      }
    }
  }
  if (drainQueue) {
    window.requestAnimationFrame(() => drainFollowUpQueue());
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

/** Live turn: gateway SSE proxy → incremental Thinking + answer (OWUI-feel,
 *  ElastOS authority path). Stop aborts the fetch. Failure → labeled mock. */
async function startLiveTurnForPrompt(userText) {
  abortLiveChatStream();
  clearStreamTimer();
  document.querySelectorAll(".agent-followups").forEach((node) => node.remove());
  const generation = (ctx.streamGeneration += 1);
  ctx.turnBusy = true;
  setAgentComposerProcessing(true);
  setStreamStatus("Connecting…", { tone: "connecting" });
  ensureJumpToLatest();

  const session = ctx.sessions.find((s) => s.id === ctx.activeSessionId);
  const thinkStartedAt = Date.now();
  let thinkingEl = appendThinkingBlock("", {
    streaming: true,
    open: Boolean(ctx.reasoningVisible),
  });
  if (thinkingEl && !ctx.reasoningVisible) {
    thinkingEl.hidden = true;
  }
  let answerRow = null;
  let answerBody = null;
  let lastPaint = 0;
  let finalReasoning = "";
  let finalContent = "";
  let sawFirstToken = false;

  const paint = (reasoning, content, { force = false } = {}) => {
    const now = Date.now();
    if (!force && now - lastPaint < 40) {
      return;
    }
    lastPaint = now;
    if ((reasoning || content) && !sawFirstToken) {
      sawFirstToken = true;
      setStreamStatus("Generating…", { tone: "generating" });
    }
    if (reasoning) {
      if (!thinkingEl) {
        thinkingEl = appendThinkingBlock(reasoning, {
          streaming: true,
          open: Boolean(ctx.reasoningVisible),
        });
      } else {
        const body = thinkingEl.querySelector(".agent-thinking-body");
        if (body) {
          body.textContent = reasoning;
        }
        thinkingEl.hidden = !ctx.reasoningVisible && !content;
        if (ctx.reasoningVisible) {
          thinkingEl.open = true;
        }
      }
    }
    if (content) {
      if (!answerRow) {
        answerRow = appendMessage("agent", content, { streaming: true });
        answerBody = answerRow?.querySelector(".agent-msg-body") || null;
      } else if (answerBody) {
        paintAgentMessageBody(answerBody, content, { streaming: true });
      }
      const scroller = host.streamScrollEl?.();
      if (scroller) {
        const distance =
          scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight;
        if (distance < 140) {
          host.scrollStreamToEnd?.();
        } else {
          updateJumpToLatestVisibility();
        }
      } else {
        host.scrollStreamToEnd?.();
      }
    }
  };

  try {
    const result = await streamLiveChatCompletion(
      buildLiveMessages(session?.messages || [], {
        systemPrompt: ctx.systemPrompt,
      }),
      {
        maxTokens: ctx.maxTokens,
        temperature: ctx.temperature,
        onDelta: ({ reasoning, content }) => {
          if (generation !== ctx.streamGeneration) {
            return;
          }
          finalReasoning = reasoning || finalReasoning;
          finalContent = content || finalContent;
          paint(finalReasoning, finalContent);
        },
      },
    );
    finalReasoning = result.reasoning || finalReasoning;
    finalContent = result.content || finalContent;
    paint(finalReasoning, finalContent, { force: true });

    let answer = String(finalContent || "").trim();
    let thinking = String(finalReasoning || "").trim();
    if (!answer && thinking) {
      /* Flash often spends the budget in reasoning — surface it honestly. */
      answer = thinking;
      thinking = "";
    }
    const split = splitThinkTaggedContent(answer);
    if (split.thinking && !thinking) {
      thinking = split.thinking;
      answer = split.answer || answer;
    }

    const superseded = generation !== ctx.streamGeneration;
    const stoppedEarly = result.aborted || superseded;
    /* Stop already finalized DOM + partial persist via stopMockStream. */
    if (superseded) {
      return;
    }

    if (thinkingEl) {
      if (thinking) {
        const body = thinkingEl.querySelector(".agent-thinking-body");
        if (body) {
          body.textContent = thinking;
        }
        thinkingEl.hidden = false;
        finishThinkingBlock(thinkingEl, thinkStartedAt);
      } else {
        thinkingEl.remove();
        thinkingEl = null;
      }
    }

    if (!answer) {
      thinkingEl?.remove();
      answerRow?.remove();
      if (!stoppedEarly) {
        setStreamStatus("Live returned empty — labeled Preview mock", {
          tone: "error",
        });
        void probeLiveInference({ force: true }).then(() => host.syncInferenceStatus?.());
        startMockStreamForPrompt(userText);
      }
      return;
    }

    if (!answerRow) {
      answerRow = appendMessage("agent", answer, { streaming: true });
      answerBody = answerRow?.querySelector(".agent-msg-body") || null;
    }
    if (answerBody) {
      paintAgentMessageBody(answerBody, answer, { streaming: false });
    }
    answerRow?.classList.remove("is-streaming");
    const regen = answerRow?.querySelector("[data-regenerate]");
    if (regen) {
      regen.disabled = false;
    }

    if (session) {
      session.messages.push(
        thinking
          ? { role: "agent", text: answer, thinking, ...(stoppedEarly ? { partial: true } : {}) }
          : { role: "agent", text: answer, ...(stoppedEarly ? { partial: true } : {}) },
      );
      session.updatedAt = Date.now();
      host.renderSessions?.();
      try {
        host.persistAgentWorkspaceSoon?.();
      } catch {
        /* optional */
      }
    }
    if (stoppedEarly && answerRow && !answerRow.querySelector(".agent-msg-stopped")) {
      const note = document.createElement("div");
      note.className = "agent-msg-stopped";
      note.innerHTML =
        `<span>Stopped</span>` +
        `<button type="button" class="agent-msg-retry" data-retry="1">Retry</button>`;
      answerRow.append(note);
    }
    /* Collapse thinking once an answer exists (Wave 1.07). */
    if (thinkingEl && answer && thinkingEl.open && ctx.reasoningVisible) {
      finishThinkingBlock(thinkingEl, thinkStartedAt);
    }
    syncTruthStrip();
    syncWorkbenchPanels();
    if (!stoppedEarly) {
      maybeOfferToolAfterReply(userText);
    }
  } catch (err) {
    if (generation !== ctx.streamGeneration) {
      return;
    }
    if (err?.code === "aborted") {
      const partial = String(finalContent || finalReasoning || "").trim();
      if (partial) {
        persistPartialAgentReply(
          String(finalContent || "").trim() || partial,
          String(finalReasoning || "").trim(),
        );
      }
      setStreamStatus("Stopped", { tone: "idle" });
      return;
    }
    const honest = formatStreamError(err);
    setStreamStatus(`${honest} · Preview mock`, { tone: "error" });
    thinkingEl?.remove();
    answerRow?.remove();
    void probeLiveInference({ force: true }).then(() => host.syncInferenceStatus?.());
    startMockStreamForPrompt(userText);
    return;
  } finally {
    if (generation === ctx.streamGeneration) {
      ctx.turnBusy = false;
      setAgentComposerProcessing(false);
      if (!document.querySelector("[data-agent-stream-status]")?.textContent?.includes("Preview mock")) {
        setStreamStatus("");
      }
      window.requestAnimationFrame(() => drainFollowUpQueue());
    }
  }
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
    const partial = replyText.slice(0, answerIndex);
    paintAgentMessageBody(answerBody, partial, {
      streaming: answerIndex < replyText.length,
    });
    if (scroller) {
      scroller.scrollTop = scroller.scrollHeight;
    }
    if (answerIndex >= replyText.length) {
      clearStreamTimer();
      answerRow?.classList.remove("is-streaming");
      answerRow?.classList.add("agent-msg-enter");
      paintAgentMessageBody(answerBody, replyText, { streaming: false });
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
