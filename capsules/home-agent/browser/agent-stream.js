/* Agent message stream + turn rendering.
   Bound from agent-harness.js.
   Live: the typed model contract (offers_list / runs_create / runs_events)
   with Stop = runs_cancel. A turn runs on it or it does not run.
   UI ≠ authority (Principle 16) — chat carries no tool or grant power.
   Wave 1: edit/resubmit, per-msg delete, markdown, stream status, Stop persist. */

import {
  noteLiveTurnUsage,
  splitThinkTaggedContent,
} from "./agent-state.js";
import {
  getLiveInferenceState,
  probeLiveInference,
  compileLiveContext,
  streamChatViaContract,
  abortLiveChatStream,
  detachLiveChatStream,
  selectedLiveOffer,
} from "./agent-live.js";
import { stampMessageNode, contentHash, newTurnId, createTurnManifest, turnStorePut, turnStorePatch, turnStoreGet, TurnState, cheapTurnSnapshot, resolveReasoningPolicy } from "./agent-context.js";
import { setAgentComposerProcessing } from "./agent-shelf.js";
import { renderHarnessPage } from "./agent-configure.js";
import { postToHome } from "./harness-host.js";
import {
  createProgressController,
  snapshotProgress,
  ProgressPhase,
  progressGlyph,
  PROGRESS_REVEAL_DELAY_MS,
  MEANINGFUL_ANSWER_THRESHOLD,
} from "./agent-progress.js";
import { createStreamQos } from "./agent-stream-qos.js";
/* Vendored, self-hosted (no CDN — Principle: capsules are self-contained).
   Static asset: stable URL, immutable, cached forever. */
import { renderToString as renderMathToString } from "./vendor/katex/katex.mjs";

/** @type {null | object} */
let ctx = null;
/** @type {null | Record<string, Function>} */
let host = null;

/** Canonical live-turn buffers. The DOM is never the source of truth. */
const liveTurnCanonical = {
  generation: 0,
  answer: "",
  reasoning: "",
  seq: 0,
  appliedSeq: -1,
  phase: "idle",
};

const STREAM_BUDGET_MS = 8;
const STREAM_TRACE_LIMIT = 48;
const FOLLOW_RESUME_PX = 96;
const RICH_FINALIZE_BUDGET_MS = 48;
/** One-shot markdown+KaTeX above this freezes Brave. Stream freeze slices stay ~800. */
const RICH_MARKDOWN_MAX_CHARS = 8_000;
const LONG_TASK_TRACE_LIMIT = 24;
let streamGraphemeSegmenter = null;

function streamGraphemeTrim(text, maxGraphemes) {
  const source = String(text || "").trim();
  if (!source) {
    return "";
  }
  try {
    streamGraphemeSegmenter ||= new Intl.Segmenter(undefined, {
      granularity: "grapheme",
    });
    let out = "";
    let n = 0;
    for (const { segment } of streamGraphemeSegmenter.segment(source)) {
      if (n >= maxGraphemes) {
        return `${out.trim()}…`;
      }
      out += segment;
      n += 1;
    }
    return out;
  } catch {
    return source.length > maxGraphemes
      ? `${source.slice(0, maxGraphemes - 1).trim()}…`
      : source;
  }
}

function streamPrefersReducedMotion() {
  return window.matchMedia?.("(prefers-reduced-motion: reduce)")?.matches === true;
}

function streamTabHidden() {
  return document.visibilityState === "hidden";
}

export function getLiveTurnCanonical() {
  const answer =
    typeof liveTurnCanonical.getAnswer === "function"
      ? liveTurnCanonical.getAnswer()
      : liveTurnCanonical.answer;
  const reasoning =
    typeof liveTurnCanonical.getReasoning === "function"
      ? liveTurnCanonical.getReasoning()
      : liveTurnCanonical.reasoning;
  return { ...liveTurnCanonical, answer, reasoning };
}

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
  let cleaned = String(prompt || "")
    .replace(/```[\s\S]*?```/g, " ")
    .replace(/`([^`]+)`/g, "$1")
    .replace(/!\[[^\]]*\]\([^)]+\)/g, " ")
    .replace(/\[([^\]]+)\]\([^)]+\)/g, "$1")
    .replace(/[*_>#]+/g, " ")
    .replace(/\s+/g, " ")
    .trim();
  if (!cleaned) {
    return "New chat";
  }
  const sentence = cleaned.split(/(?<=[.!?])\s+/)[0] || cleaned;
  cleaned = sentence.length > 8 && sentence.length <= 64 ? sentence : cleaned;
  return cleaned.length > 42 ? `${cleaned.slice(0, 41)}…` : cleaned;
}

export function escapeHtml(text) {
  return String(text)
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;");
}

/* TeX arriving from escapeHtml'd text has entities; KaTeX wants real chars. */
function unescapeForMath(tex) {
  return String(tex)
    .replaceAll("&lt;", "<")
    .replaceAll("&gt;", ">")
    .replaceAll("&quot;", '"')
    .replaceAll("&amp;", "&");
}

/* Typeset via vendored KaTeX; on any parse error degrade to the raw TeX in a
   muted code pill — honest fallback, never broken HTML.

   Memoized: streaming re-paints the growing answer often; without a cache each
   unique formula is re-typeset every paint (the expensive part of the freeze). */
const MATH_CACHE_LIMIT = 400;
const mathCache = new Map();
function renderMath(tex, displayMode, sourceKind) {
  if (sourceKind === "reasoning") {
    throw new Error("Reasoning entered KaTeX path");
  }
  const key = `${displayMode ? "D" : "I"}:${tex}`;
  const hit = mathCache.get(key);
  if (hit !== undefined) {
    return hit;
  }
  let out;
  try {
    out =
      `<span class="agent-md-math${displayMode ? " agent-md-math-display" : ""}">` +
      renderMathToString(unescapeForMath(tex), { displayMode, throwOnError: true }) +
      `</span>`;
  } catch {
    out = `<code class="agent-md-math-raw">${escapeHtml(tex)}</code>`;
  }
  if (mathCache.size >= MATH_CACHE_LIMIT) {
    mathCache.delete(mathCache.keys().next().value);
  }
  mathCache.set(key, out);
  return out;
}

/** GFM-safe markdown subset — escape first, then format.
 *  Tables, lists, links, headings, fences, inline code/bold/italic.
 *  UI must never treat model HTML as authority (Principle 16).
 *  `streaming: true` virtually closes an open fence so mid-reply code blocks
 *  paint as code instead of raw ``` until the model finishes the fence. */
const MD_COPY_ICON =
  '<svg class="agent-md-copy-icon" viewBox="0 0 16 16" width="15" height="15" aria-hidden="true" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><rect x="5.5" y="5.5" width="8" height="8" rx="1.5"/><path d="M10.5 3.5v-.8A1.7 1.7 0 0 0 8.8 1H3.7A1.7 1.7 0 0 0 2 2.7v5.1a1.7 1.7 0 0 0 1.7 1.7h.8"/></svg>' +
  '<svg class="agent-md-copy-check" viewBox="0 0 16 16" width="15" height="15" aria-hidden="true" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><path d="M3.5 8.5 6.5 11.5 12.5 4.5"/></svg>';

export function renderMarkdown(text, { streaming = false, stream, kind } = {}) {
  if (stream === "thinking" || kind === "reasoning") {
    throw new Error("Raw reasoning must never enter Markdown renderer");
  }
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
      const raw = code.replace(/\n$/, "");
      const safe = escapeHtml(raw);
      const label = lang ? escapeHtml(lang) : "";
      html +=
        `<div class="agent-md-code" data-md-source="${escapeHtml(raw)}">` +
        `<div class="agent-md-code-head"><span class="agent-md-code-lang">${label}</span>` +
        `<span class="agent-md-code-actions">` +
        `<button type="button" class="agent-md-open" data-open-artifact="1" data-lang="${label || "code"}">Open</button>` +
        `<button type="button" class="agent-md-copy" data-copy="1" aria-label="Copy code" title="Copy">${MD_COPY_ICON}</button>` +
        `</span></div>` +
        `<pre><code>${safe}</code></pre></div>`;
      continue;
    }
    html += renderMarkdownBlocks(parts[i]);
  }
  return html;
}

/** Keep raw markdown on the node so Stop/Copy don't lose markers after HTML paint. */
function paintAgentMessageBody(body, text, { streaming = false, allowDegrade = false } = {}) {
  if (!body) {
    return;
  }
  const raw = String(text ?? "");
  if (body.dataset.mdSource === raw && (streaming || body.dataset.mdPainted === "1")) {
    return;
  }
  body.dataset.mdSource = raw;
  if (streaming || raw.length > RICH_MARKDOWN_MAX_CHARS) {
    body.textContent = raw;
    delete body.dataset.mdPainted;
    return;
  }
  const started = performance.now();
  const html = renderMarkdown(raw, { streaming: false });
  if (allowDegrade && performance.now() - started > RICH_FINALIZE_BUDGET_MS) {
    delete body.dataset.mdPainted;
    return;
  }
  const tmpl = document.createElement("template");
  tmpl.innerHTML = html;
  if (allowDegrade && performance.now() - started > RICH_FINALIZE_BUDGET_MS) {
    delete body.dataset.mdPainted;
    return;
  }
  body.replaceChildren(tmpl.content);
  body.dataset.mdPainted = "1";
}

function thinkingBodyRaw(body) {
  return String(body?.dataset?.mdSource || body?.textContent || "");
}

function syncThinkingHeader(details, { streaming = false, fromText = "" } = {}) {
  if (!details) {
    return;
  }
  const label = details.querySelector(".agent-thinking-label");
  if (!streaming || !label || details.classList.contains("is-complete")) {
    return;
  }
  /* Show latest activity as the header while streaming: prefer markdown headers
     (##, ###), else bold lines (**Analyzing…**). Convert to verb form. */
  const raw = String(
    fromText ||
      details.querySelector(".agent-thinking-body")?.dataset?.mdSource ||
      "",
  );
  if (!raw) {
    label.textContent = "Thinking";
    return;
  }
  let activity = "";
  const lines = raw.split(/\r?\n/);
  for (let i = lines.length - 1; i >= 0; i -= 1) {
    const line = lines[i].trim();
    const md = line.match(/^#{1,6}\s+(.+)$/);
    if (md) {
      activity = md[1];
      break;
    }
    const bold = line.match(/^\*\*(.+?)\*\*/);
    if (bold) {
      activity = bold[1];
      break;
    }
  }
  if (!activity) {
    activity = "Thinking";
  }
  /* Verbify: “Analyze” → “Analyzing”, “Draft” → “Drafting”, etc. */
  activity = activity.replace(/^([A-Z][a-z]+)\s/, (m, verb) => {
    const lower = verb.toLowerCase();
    const map = {
      analyze: "Analyzing", draft: "Drafting", translate: "Translating",
      deconstruct: "Deconstructing", construct: "Constructing", build: "Building",
      create: "Creating", generate: "Generating", write: "Writing",
      structure: "Structuring", plan: "Planning", review: "Reviewing",
      check: "Checking", verify: "Verifying", validate: "Validating",
      process: "Processing", parse: "Parsing", read: "Reading",
      think: "Thinking", consider: "Considering", evaluate: "Evaluating",
      summarize: "Summarizing", outline: "Outlining", sketch: "Sketching",
      design: "Designing", implement: "Implementing", fix: "Fixing",
      debug: "Debugging", test: "Testing", refine: "Refining",
      polish: "Polishing", finalize: "Finalizing", complete: "Completing",
    };
    return (map[lower] || verb + "ing") + " ";
  });
  /* Trim to ~48 graphemes so it stays one line (emoji/CJK ≠ 1 JS char). */
  label.textContent = streamGraphemeTrim(activity, 48);
}

function paintTurnUsageMeta(row, turn) {
  if (!row || !turn) {
    return;
  }
  let meta = row.querySelector(".agent-msg-meta");
  if (!meta) {
    meta = document.createElement("div");
    meta.className = "agent-msg-meta";
    row.append(meta);
  }
  const latency = turn.latencyMs
    ? turn.latencyMs >= 1000
      ? `${(turn.latencyMs / 1000).toFixed(1)}s`
      : `${turn.latencyMs}ms`
    : "";
  const tokens = turn.tokens ? `${turn.tokens} tok` : "";
  const source =
    turn.source === "estimated" || turn.omitted
      ? "est."
      : "live";
  meta.textContent = [latency, tokens, source].filter(Boolean).join(" · ");
}

function formatInlineMarkdown(escaped) {
  return escaped
    .replaceAll(/\\\((.+?)\\\)/g, (_, tex) => renderMath(tex, false))
    .replaceAll(/\\\[(.+?)\\\]/g, (_, tex) => renderMath(tex, true))
    .replaceAll(/\$\$([^$]+?)\$\$/g, (_, tex) => renderMath(tex, true))
    /* Pandoc-style $...$: no space inside the delimiters, close not followed
       by a digit — keeps "costs $5 and $10" as plain text. */
    .replaceAll(/\$([^\s$](?:[^$\n]*[^\s$])?)\$(?!\d)/g, (_, tex) => renderMath(tex, false))
    .replaceAll(/\[([^\]]+)\]\((https?:\/\/[^)\s]+)\)/g, (_, label, href) => {
      return `<button type="button" class="agent-md-a" data-open-browser-url="${href}">${label}</button>`;
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
    /* Display math block: \[ ... \] or $$ ... $$ starting at line start,
       single- or multi-line. Trailing text after the closer, or no closer yet
       (streaming), falls through to normal paragraph handling — the inline
       pass still catches it, or the next delta completes the block. */
    const mathOpen = /^\s*(\\\[|\$\$)/.exec(line);
    if (mathOpen) {
      const open = mathOpen[1];
      const close = open === "\\[" ? "\\]" : "$$";
      const trimmedLine = line.trim();
      const first = trimmedLine.slice(trimmedLine.indexOf(open) + open.length);
      let tex = null;
      let next = i + 1;
      const inlineAt = first.lastIndexOf(close);
      if (inlineAt !== -1 && !first.slice(inlineAt + close.length).trim()) {
        tex = first.slice(0, inlineAt);
      } else if (inlineAt === -1) {
        const buf = [first];
        let j = i + 1;
        while (j < lines.length) {
          const at = lines[j].indexOf(close);
          if (at !== -1) {
            if (!lines[j].slice(at + close.length).trim()) {
              buf.push(lines[j].slice(0, at));
              tex = buf.join("\n");
              next = j + 1;
            }
            break;
          }
          buf.push(lines[j]);
          j += 1;
        }
      }
      if (tex !== null) {
        if (tex.trim()) {
          html += `<div class="agent-md-math-block">${renderMath(tex, true)}</div>`;
        }
        i = next;
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
      !/^>\s?/.test(lines[i].trim()) &&
      !/^\s*(\\\[|\$\$)/.test(lines[i])
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
    return `Live upstream error${status} — check the model offer backend, then retry`;
  }
  if (code === "no_body") {
    return "Live stream had no body — gateway or upstream misconfigured";
  }
  if (String(err?.name || "") === "TimeoutError" || /timeout/i.test(String(err?.message || ""))) {
    return "Live timed out — retry, or check the offer backend";
  }
  if (code === "no_model_offers") {
    return NO_MODEL_OFFER_STATUS;
  }
  return err?.message
    ? `Model run failed: ${String(err.message).slice(0, 160)}`
    : "Model run failed";
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

function commitAgentReply(session, answer, thinking = "", { partial = false, quiet = false, progress = null, run = null, turn = null } = {}) {
  if (!session) {
    return;
  }
  const text = String(answer || "").trim();
  if (!text) {
    return;
  }
  const last = session.messages[session.messages.length - 1];
  const payload = thinking
    ? { role: "agent", text, thinking, ...(partial ? { partial: true } : {}) }
    : { role: "agent", text, ...(partial ? { partial: true } : {}) };
  if (progress) {
    payload.progress = progress;
  }
  if (run && typeof run === "object") {
    payload.run = {
      requestedEffort: String(run.requested || run.requestedEffort || ""),
      effectiveEffort: String(run.effective || run.effectiveEffort || ""),
      degraded: Boolean(run.degraded),
      contextHash: run.contextHash ? String(run.contextHash).slice(0, 16) : undefined,
    };
  }
  if (turn && typeof turn === "object") {
    payload.turn = cheapTurnSnapshot(turn);
  }
  if (last?.role === "agent" && (last.partial || last.text === text)) {
    last.text = text;
    last.contentHash = contentHash(text);
    if (thinking) {
      last.thinking = thinking;
    } else {
      delete last.thinking;
    }
    if (progress) {
      last.progress = progress;
    }
    if (payload.run) {
      last.run = payload.run;
    }
    if (payload.turn) {
      last.turn = payload.turn;
    }
    if (partial) {
      last.partial = true;
    } else {
      delete last.partial;
    }
  } else {
    session.messages.push(stampMessageNode(payload, { session }));
  }
  session.updatedAt = Date.now();
  if (!quiet) {
    host.renderSessions?.();
  }
  try {
    host.persistAgentWorkspaceSoon?.();
  } catch {
    /* optional */
  }
}

export function persistPartialAgentReply(text, thinking = "") {
  const session = ctx.sessions.find((s) => s.id === ctx.activeSessionId);
  commitAgentReply(session, text, thinking, { partial: true, quiet: true });
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
  if (details.dataset.block === "progress") {
    details.classList.add("is-secondary");
    return;
  }
  const ms = Math.max(400, Date.now() - (startedAt || Date.now()));
  const sec = (ms / 1000).toFixed(ms >= 10000 ? 0 : 1);
  const label = details.querySelector(".agent-thinking-label");
  if (label) {
    label.textContent = `Thought for ${sec}s`;
  }
  /* Stay collapsed by default; if the user expanded mid-stream, fold after a beat. */
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

export function enqueueFollowUp(text, opts = {}) {
  ctx.followUpQueue.push({
    id: `q-${Date.now()}-${ctx.followUpQueue.length}`,
    text,
    displayText: opts.displayText || "",
    parts: Array.isArray(opts.parts) ? opts.parts : undefined,
  });
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
  const displayText = String(next.displayText || "").trim();
  session.messages.push({
    role: "user",
    text: displayText || (Array.isArray(next.parts) && next.parts.length ? "" : next.text),
    modelText: next.text,
    ...(Array.isArray(next.parts) && next.parts.length ? { parts: next.parts } : {}),
  });
  host.renderSessions();
  appendMessage("user", displayText, { parts: next.parts, modelText: next.text });
  startTurnForPrompt(next.text);
}

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
  const sub = "Private on this Home";
  empty.innerHTML =
    `<p class="agent-harness-empty-greeting"></p>` +
    `<p class="agent-harness-empty-sub"></p>`;
  empty.querySelector(".agent-harness-empty-greeting").textContent = greeting;
  empty.querySelector(".agent-harness-empty-sub").textContent = sub;
  /* Viewport — not the dock-width column — so the hero sits in true room center. */
  viewport.append(empty);
}

function clearVisibleTranscript() {
  host.clearEmptyState?.();
  const column = host.streamEl?.();
  const scroll = host.streamScrollEl?.();
  if (column) {
    column.replaceChildren();
  }
  /* Turns must never leave sibling nodes beside the column — those
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
  session.messages.forEach((msg, index) => {
    if (msg.role === "grant") {
      /* Grant cards belonged to the URUX tool preview; sessions that still
         carry one show nothing for it. */
    } else {
      if (msg.role === "agent" && (msg.progress?.milestones?.length || msg.thinking)) {
        if (msg.progress?.milestones?.length) {
          const progressEl = appendProgressBlock();
          paintProgressView(progressEl, {
            revealed: true,
            secondary: true,
            phase: "done",
            currentText: msg.progress.label || `✓ ${msg.progress.milestones.length} steps`,
            current: null,
            milestones: msg.progress.milestones,
          }, { reasoning: msg.thinking, reasoningVisible: ctx.reasoningVisible });
        } else {
          const think = appendThinkingBlock(msg.thinking, {
            streaming: false,
            open: false,
          });
          if (think) {
            finishThinkingBlock(think, Number(msg.updatedAt) || Date.now());
          }
        }
      }
      appendMessage(msg.role, msg.text, {
        msgIndex: index,
        parts: msg.parts,
        modelText: msg.modelText,
      });
    }
  });
  host.syncTruthStrip?.();
  setStreamStatus("");
  updateJumpToLatestVisibility();
}

export function appendThinkingBlock(text, { streaming = false, open = false } = {}) {
  const stream = host.streamEl();
  if (!stream) {
    return null;
  }
  host.clearEmptyState();
  const details = document.createElement("details");
  details.className = `agent-thinking${streaming ? " is-streaming" : ""}`;
  details.dataset.block = "thinking";
  details.dataset.startedAt = String(Date.now());
  /* Collapsed by default — shimmer header stays live; expand for full stream. */
  if (open && ctx.reasoningVisible) {
    details.open = true;
  }
  const summary = document.createElement("summary");
  summary.className = "agent-thinking-summary";
  summary.title = "Show full thinking";
  summary.innerHTML =
    `<span class="agent-thinking-chevron" aria-hidden="true"></span>` +
    `<span class="agent-thinking-label">Thinking</span>`;
  const bodyWrap = document.createElement("div");
  bodyWrap.className = "agent-thinking-body-wrap";
  const body = document.createElement("div");
  body.className = "agent-thinking-body";
  body.dataset.mdSource = String(text ?? "");
  if (streaming) {
    body.textContent = "";
  }
  bodyWrap.append(body);
  details.append(summary, bodyWrap);
  if (!streaming && text) {
    details.addEventListener("toggle", () => {
      if (!details.open || body.dataset.mdPainted === "1") {
        return;
      }
      const raw = String(body.dataset.mdSource || "");
      if (!raw) {
        return;
      }
      body.textContent = raw;
      delete body.dataset.mdPainted;
    });
  }
  syncThinkingHeader(details, { streaming });
  stream.append(details);
  host.scrollStreamToEnd();
  return details;
}

export function appendProgressBlock() {
  const stream = host.streamEl();
  if (!stream) {
    return null;
  }
  host.clearEmptyState();
  const details = document.createElement("details");
  details.className = "agent-thinking agent-progress is-streaming";
  details.dataset.block = "progress";
  details.hidden = true;
  const summary = document.createElement("summary");
  summary.className = "agent-thinking-summary agent-progress-summary";
  summary.title = "Show activity";
  const chevron = document.createElement("span");
  chevron.className = "agent-thinking-chevron";
  chevron.setAttribute("aria-hidden", "true");
  const label = document.createElement("span");
  label.className = "agent-thinking-label agent-progress-label";
  summary.append(chevron, label);
  const wrap = document.createElement("div");
  wrap.className = "agent-thinking-body-wrap agent-progress-trace";
  const list = document.createElement("ol");
  list.className = "agent-progress-milestones";
  const raw = document.createElement("div");
  raw.className = "agent-thinking-body agent-progress-raw";
  raw.hidden = true;
  wrap.append(list, raw);
  details.append(summary, wrap);
  stream.append(details);
  return details;
}

function progressGlyphSvg(kind) {
  const common =
    'viewBox="0 0 16 16" width="12" height="12" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"';
  if (kind === "search") {
    return `<svg ${common}><circle cx="7" cy="7" r="4.5"/><path d="m10.5 10.5 3 3"/></svg>`;
  }
  if (kind === "read") {
    return `<svg ${common}><path d="M3.5 3.5h6.5v10H5a1.5 1.5 0 0 1-1.5-1.5V3.5Z"/><path d="M10 3.5h2.5V12A1.5 1.5 0 0 1 11 13.5"/><path d="M6 6.5h2.5M6 9h2.5"/></svg>`;
  }
  if (kind === "code") {
    return `<svg ${common}><path d="m5 5-3 3 3 3M11 5l3 3-3 3"/></svg>`;
  }
  if (kind === "verify") {
    return `<svg ${common}><path d="M3.5 8.5 6.5 11.5 12.5 4.5"/></svg>`;
  }
  if (kind === "finding") {
    return `<svg ${common}><path d="M3 8h10M9.5 4.5 13 8l-3.5 3.5"/></svg>`;
  }
  if (kind === "error") {
    return `<svg ${common}><circle cx="8" cy="8" r="5.5"/><path d="M8 5.5v3M8 10.8h.01"/></svg>`;
  }
  return `<svg ${common}><circle cx="8" cy="8" r="2.2"/></svg>`;
}

function appendProgressItem(list, { className, glyph, text }) {
  const li = document.createElement("li");
  li.className = className;
  li.dataset.glyph = glyph;
  const icon = document.createElement("span");
  icon.className = "agent-progress-glyph";
  icon.setAttribute("aria-hidden", "true");
  icon.innerHTML = progressGlyphSvg(glyph);
  const label = document.createElement("span");
  label.textContent = text;
  li.append(icon, label);
  list.append(li);
}

function paintProgressView(el, progress, { reasoning = "", reasoningVisible = false } = {}) {
  if (!el || !progress) {
    return;
  }
  const show = Boolean(progress.revealed && (progress.currentText || progress.milestones?.length));
  el.hidden = !show;
  const live =
    progress.phase !== ProgressPhase.DONE &&
    progress.phase !== ProgressPhase.ERROR &&
    progress.phase !== ProgressPhase.STOPPED;
  el.classList.toggle("is-streaming", live);
  el.classList.toggle("is-complete", !live);
  el.classList.toggle("is-secondary", Boolean(progress.secondary));
  el.classList.toggle("is-error", progress.phase === ProgressPhase.ERROR);
  const label = el.querySelector(".agent-progress-label");
  const nextLabel = progress.currentText || "";
  if (label && label.textContent !== nextLabel) {
    label.textContent = nextLabel;
  }
  const list = el.querySelector(".agent-progress-milestones");
  if (list) {
    const currentBit = live && progress.current ? `|${progress.current.key}:${progress.current.text}` : "";
    const keys = `${(progress.milestones || []).map((m) => m.key).join("|")}${currentBit}`;
    if (list.dataset.keys !== keys) {
      list.dataset.keys = keys;
      list.replaceChildren();
      for (const m of progress.milestones || []) {
        appendProgressItem(list, {
          className: m.kind === "finding" ? "is-finding" : "is-done",
          glyph: progressGlyph(m.phase, m.kind),
          text: m.text,
        });
      }
      if (live && progress.current && progress.current.kind !== "finding") {
        appendProgressItem(list, {
          className: "is-current",
          glyph: progressGlyph(progress.current.phase, progress.current.kind),
          text: progress.current.text,
        });
      }
    }
  }
  const raw = el.querySelector(".agent-progress-raw");
  if (raw) {
    const showRaw = Boolean(reasoningVisible && reasoning);
    raw.hidden = !showRaw;
    if (showRaw) {
      raw.dataset.mdSource = reasoning;
    }
  }
}

function paintUserMessageBody(body, text, { parts = null, modelText = "" } = {}) {
  body.replaceChildren();
  body.dataset.mdSource = String(modelText || text || "");
  const chips = Array.isArray(parts)
    ? parts.filter((p) => p && p.kind && p.kind !== "text")
    : [];
  if (chips.length) {
    const row = document.createElement("div");
    row.className = "agent-msg-parts";
    for (const part of chips) {
      const chip = document.createElement("span");
      chip.className = `agent-msg-part-chip${part.kind === "pasted_text" ? " is-paste" : ""}`;
      const name = document.createElement("span");
      name.className = "agent-msg-part-name";
      name.textContent = part.title || part.name || "Attachment";
      const meta = document.createElement("span");
      meta.className = "agent-msg-part-meta";
      meta.textContent =
        part.kind === "pasted_text"
          ? part.subtitle || "Pasted text"
          : part.kind === "image"
            ? "Image"
            : part.uri
              ? "Desktop"
              : "File";
      chip.append(name, meta);
      row.append(chip);
    }
    body.append(row);
  }
  const prose = String(text || "").trim();
  if (prose) {
    const block = document.createElement("div");
    block.className = "agent-msg-user-text";
    block.textContent = prose;
    body.append(block);
  }
}

export function appendMessage(
  role,
  text,
  { streaming = false, msgIndex = null, parts = null, modelText = "" } = {},
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
  if (role === "agent") {
    paintAgentMessageBody(body, text, { streaming });
  } else {
    paintUserMessageBody(body, text, { parts, modelText });
  }

  const actions = document.createElement("div");
  actions.className = "agent-msg-actions";
  const makeIcon = (svg, label, extra = "") => {
    const btn = document.createElement("button");
    btn.type = "button";
    btn.className = `agent-msg-action${extra ? ` ${extra}` : ""}`;
    btn.innerHTML = svg;
    btn.title = label;
    btn.setAttribute("aria-label", label);
    return btn;
  };
  const ICONS = {
    copy: `<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><rect x="5.5" y="5.5" width="8" height="8" rx="1.5"/><path d="M10.5 3.5v-.8A1.7 1.7 0 0 0 8.8 1H3.7A1.7 1.7 0 0 0 2 2.7v5.1a1.7 1.7 0 0 0 1.7 1.7h.8"/></svg>`,
    edit: `<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M11.5 2.5a1.7 1.7 0 0 1 2 2L5 13l-2.6.6L3 11l8.5-8.5Z"/></svg>`,
    regen: `<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M13.5 6.5A5.5 5.5 0 1 0 14 9.5"/><path d="M13.5 2.5v4h-4"/></svg>`,
    trash: `<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M2.5 4h11M6.5 2h3M4 4l.6 8.6A1.6 1.6 0 0 0 6.2 14h3.6a1.6 1.6 0 0 0 1.6-1.4L12 4"/></svg>`,
  };
  const copyBtn = makeIcon(ICONS.copy, "Copy");
  copyBtn.dataset.copyMessage = "1";
  actions.append(copyBtn);
  if (role === "user" && !streaming) {
    const edit = makeIcon(ICONS.edit, "Edit and resubmit");
    edit.dataset.editMessage = "1";
    edit.disabled = Boolean(ctx.turnBusy);
    actions.append(edit);
  }
  if (role === "agent") {
    const regen = makeIcon(ICONS.regen, "Regenerate reply");
    regen.dataset.regenerate = "1";
    regen.disabled = streaming || Boolean(ctx.turnBusy);
    actions.append(regen);
  }
  if (!streaming) {
    const del = makeIcon(ICONS.trash, "Delete", "is-danger");
    del.dataset.deleteMessage = "1";
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

/* UTF-8 safe base64 (btoa alone chokes on non-Latin-1). The chat-attachment
   path carries bytes as a dataUrl. */
function utf8ToBase64(text) {
  const bytes = new TextEncoder().encode(String(text ?? ""));
  let binary = "";
  for (let i = 0; i < bytes.length; i += 1) {
    binary += String.fromCharCode(bytes[i]);
  }
  return btoa(binary);
}

/* Open a code/text block from a reply in Home's viewer rail (Documents, via
   the chat-attachment path): content the agent already gave the user, opened
   in the panel — no Library write. */
export function openCodeArtifact(code, lang = "") {
  const body = String(code ?? "");
  if (!body.trim()) {
    return;
  }
  const language = String(lang || "code").trim() || "code";
  const title = `${language} snippet`;
  const mimeType = "text/plain";
  postToHome({
    type: "home-agent:open-viewer",
    request: {
      target: "documents",
      title,
      kind: "code",
      query: { view: "read" },
      deliver: {
        type: "documents:open-chat-attachment",
        attachmentId: `code-${Date.now()}`,
        fileName: `snippet.${language === "code" ? "txt" : language.replace(/[^a-z0-9]/gi, "") || "txt"}`,
        mimeType,
        dataUrl: `data:${mimeType};base64,${utf8ToBase64(body)}`,
      },
    },
  });
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
  ta.value = String(msg.modelText || msg.text || "");
  ta.rows = Math.min(8, Math.max(2, ta.value.split("\n").length + 1));
  const bar = document.createElement("div");
  bar.className = "agent-msg-edit-bar";
  const cancel = document.createElement("button");
  cancel.type = "button";
  cancel.className = "agent-msg-edit-cancel";
  cancel.dataset.editCancel = "1";
  cancel.textContent = "Cancel";
  const save = document.createElement("button");
  save.type = "submit";
  save.className = "agent-msg-edit-send";
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
  session.messages.push(
    stampMessageNode({ role: "user", text, modelText: text }, { session }),
  );
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
  const prompt = session.messages[lastUser].modelText || session.messages[lastUser].text;
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

/** Transport + generation bump only. UI reconciles afterward (Stop is P0). */
export function abortAgentStreamNow() {
  abortLiveChatStream();
  clearStreamTimer();
  if (!ctx) {
    return;
  }
  ctx.streamGeneration += 1;
}

export function stopAgentStream({ keepPartial = true, drainQueue = false, cancelRun = true } = {}) {
  /* cancelRun=false = navigation detach: leave the server-side run alive, just stop
     consuming its events. Explicit Stop (default) still fires runs_cancel. */
  if (cancelRun) {
    abortLiveChatStream();
  } else {
    detachLiveChatStream();
  }
  clearStreamTimer();
  ctx.streamGeneration += 1;
  ctx.turnBusy = false;
  setAgentComposerProcessing(false);
  host.setComposerGeometrySuspended?.(false);
  setStreamStatus("");
  const thinking = host.streamEl()?.querySelector(".agent-thinking.is-streaming");
  const streaming = host.streamEl()?.querySelector(".agent-msg-agent.is-streaming");
  /* Reconcile after abort returns. Join + persist on the next frame so Stop
     never blocks on a multi-megabyte string. Keep the streamed plain text. */
  const reconcile = () => {
    const canon = getLiveTurnCanonical();
    const thinkingText = String(
      canon.reasoning ||
        thinking?.dataset.mdSource ||
        thinkingBodyRaw(thinking?.querySelector(".agent-thinking-body")) ||
        "",
    ).trim();
    const raw = String(
      canon.answer || streaming?.querySelector(".agent-msg-body")?.dataset.mdSource || "",
    ).trim();
    if (thinking) {
      finishThinkingBlock(thinking, Number(thinking.dataset.startedAt) || Date.now());
      if (thinking.dataset.block === "progress") {
        const label = thinking.querySelector(".agent-progress-label");
        if (label) {
          label.textContent = "Stopped";
        }
      }
    }
    if (!streaming) {
      return;
    }
    streaming.classList.remove("is-streaming");
    streaming.dataset.streamPhase = "presentation_done";
    if (!keepPartial) {
      streaming.remove();
      thinking?.remove();
      return;
    }
    const body = streaming.querySelector(".agent-msg-body");
    if (body && raw) {
      body.dataset.mdSource = raw;
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
      return;
    }
    streaming.remove();
    if (!thinkingText) {
      thinking?.remove();
    }
  };
  window.requestAnimationFrame(reconcile);
  if (drainQueue) {
    window.requestAnimationFrame(() => drainFollowUpQueue());
  }
}

export const NO_MODEL_OFFER_STATUS =
  "No model offer on this Home — install a model provider (Store · Services) to chat";

/** One decision point (one canonical path): a turn runs on the typed model
 *  contract or it does not run. Nothing here invents a reply. */
export function startTurnForPrompt(userText) {
  if (getLiveInferenceState().live) {
    void startLiveTurnForPrompt(userText);
    return;
  }
  setStreamStatus(NO_MODEL_OFFER_STATUS, { tone: "error" });
  void probeLiveInference({ force: true }).then(() => {
    if (getLiveInferenceState().live) {
      setStreamStatus("");
      void startLiveTurnForPrompt(userText);
    }
  });
}

/** Live-turn renderer: tokens buffer independently of DOM.
 *  Per-flush work is bounded by new material + a small tail, never by the
 *  accumulated answer. Immutable frozen blocks (markdown once per closed
 *  slice) + one mutable tail (appendData). Think stays out of layout until
 *  opened after the turn. MODEL_DONE markdowns remaining tail only — never
 *  re-parses committed frozen blocks. */

const STREAM_HOLD_OPENERS = /(\*\*|`|\[|!\[|\$\$|\$)\s*$/;
const MAX_TAIL_CHARS = 1200;
const MAX_TAIL_OPEN_STRUCTURE = 24_000;
const TARGET_FROZEN_BLOCK = 800;
const MIN_FROZEN_BLOCK = 400;
const MARKDOWN_FREEZE_BUDGET_MS = 8;

function fenceOpen(text) {
  return (String(text).split("```").length - 1) % 2 === 1;
}

function displayMathOpen(text) {
  return (String(text).split("$$").length - 1) % 2 === 1;
}

function lastOpenFenceIndex(text) {
  const source = String(text);
  if (!fenceOpen(source)) {
    return -1;
  }
  return source.lastIndexOf("```");
}

function maxTailChars(text) {
  return fenceOpen(text) || displayMathOpen(text) ? MAX_TAIL_OPEN_STRUCTURE : MAX_TAIL_CHARS;
}

function cutIsBlockBoundary(text, cut) {
  if (cut <= 0 || cut > text.length) {
    return false;
  }
  return text[cut - 1] === "\n";
}

function streamHoldIncomplete(text) {
  return String(text).replace(STREAM_HOLD_OPENERS, "");
}

function chooseSemanticCut(text, target = TARGET_FROZEN_BLOCK) {
  const source = String(text || "");
  if (source.length <= target) {
    return 0;
  }
  const fenceAt = lastOpenFenceIndex(source);
  if (fenceAt >= 0) {
    return fenceAt >= MIN_FROZEN_BLOCK ? fenceAt : 0;
  }
  if (displayMathOpen(source)) {
    const mathAt = source.lastIndexOf("$$");
    return mathAt >= MIN_FROZEN_BLOCK ? mathAt : 0;
  }
  const windowEnd = Math.min(source.length, target + 400);
  const window = source.slice(0, windowEnd);
  const para = window.lastIndexOf("\n\n", target);
  if (para >= MIN_FROZEN_BLOCK) {
    return para + 2;
  }
  const nl = window.lastIndexOf("\n", target);
  if (nl >= MIN_FROZEN_BLOCK) {
    return nl + 1;
  }
  return target;
}

function streamChooseFlushDelay(charsPerSecond) {
  if (charsPerSecond < 100) {
    return 25;
  }
  if (charsPerSecond < 500) {
    return 50;
  }
  if (charsPerSecond < 1500) {
    return 75;
  }
  return 100;
}

function streamShouldFlush(buffer, elapsedMs) {
  if (elapsedMs > 100) {
    return true;
  }
  if (
    buffer.endsWith(". ") ||
    buffer.endsWith("? ") ||
    buffer.endsWith("! ") ||
    buffer.endsWith(", ") ||
    buffer.endsWith("\n")
  ) {
    return true;
  }
  return buffer.length > 120;
}

function streamPercentile(values, p) {
  if (!values.length) {
    return 0;
  }
  const sorted = [...values].sort((a, b) => a - b);
  const idx = Math.min(sorted.length - 1, Math.max(0, Math.ceil((p / 100) * sorted.length) - 1));
  return sorted[idx];
}

function streamPearson(xs, ys) {
  const n = Math.min(xs.length, ys.length);
  if (n < 8) {
    return null;
  }
  let sx = 0;
  let sy = 0;
  let sxx = 0;
  let syy = 0;
  let sxy = 0;
  for (let i = 0; i < n; i += 1) {
    const x = xs[i];
    const y = ys[i];
    sx += x;
    sy += y;
    sxx += x * x;
    syy += y * y;
    sxy += x * y;
  }
  const num = n * sxy - sx * sy;
  const den = Math.sqrt((n * sxx - sx * sx) * (n * syy - sy * sy));
  if (!(den > 1e-9)) {
    return 0;
  }
  return num / den;
}

function streamSlopePerChar(xs, ys) {
  const n = Math.min(xs.length, ys.length);
  if (n < 8) {
    return null;
  }
  let sx = 0;
  let sy = 0;
  let sxx = 0;
  let sxy = 0;
  for (let i = 0; i < n; i += 1) {
    sx += xs[i];
    sy += ys[i];
    sxx += xs[i] * xs[i];
    sxy += xs[i] * ys[i];
  }
  const den = n * sxx - sx * sx;
  if (!(Math.abs(den) > 1e-9)) {
    return 0;
  }
  return (n * sxy - sx * sy) / den;
}

function streamBucketP95(traces) {
  const edges = [0, 10000, 25000, 50000, 100000, Infinity];
  const labels = ["p95_0_10k", "p95_10_25k", "p95_25_50k", "p95_50_100k", "p95_100k_plus"];
  const out = {};
  for (let i = 0; i < labels.length; i += 1) {
    const lo = edges[i];
    const hi = edges[i + 1];
    const durs = traces
      .filter((row) => row.answerLen >= lo && row.answerLen < hi)
      .map((row) => row.dur);
    out[labels[i]] = durs.length ? Number(streamPercentile(durs, 95).toFixed(2)) : "n/a";
  }
  return out;
}

function scheduleIdleWork(fn) {
  const startedAt = performance.now();
  const invoke = (abandonRich) => {
    fn({ abandonRich: Boolean(abandonRich) || performance.now() - startedAt > 4000 });
  };
  if (typeof requestIdleCallback === "function") {
    requestIdleCallback((deadline) => {
      invoke(Boolean(deadline?.didTimeout));
    }, { timeout: 4000 });
    return;
  }
  window.setTimeout(() => invoke(false), 32);
}

/** Live turn: contract runs_events → scheduled DOM. Stop aborts the run. */
async function startLiveTurnForPrompt(userText) {
  abortLiveChatStream();
  clearStreamTimer();
  const generation = (ctx.streamGeneration += 1);
  ctx.turnBusy = true;
  setAgentComposerProcessing(true);
  host.setComposerGeometrySuspended?.(true);
  setStreamStatus("");
  ensureJumpToLatest();

  const session = ctx.sessions.find((s) => s.id === ctx.activeSessionId);
  let liveTurn = null;
  const t0 = Date.now();
  const timings = {
    t0,
    t1: 0,
    t2: 0,
    t3: 0,
    t4: 0,
    t5: 0,
    t6: 0,
  };
  const state = {
    pendingAnswer: "",
    pendingReasoning: "",
    frozenLen: 0,
    frozenBlocks: 0,
    complete: false,
    renderQueued: false,
    seq: 0,
    appliedSeq: -1,
    phase: "generating",
    quality: "plain",
    lastFlushMs: 0,
  };
  const answerChunks = [];
  const reasoningChunks = [];
  let answerLen = 0;
  let reasoningLen = 0;
  let tail = "";
  let cachedAnswer = null;
  let cachedReasoning = null;
  const getAnswer = () => {
    if (cachedAnswer != null) {
      return cachedAnswer;
    }
    cachedAnswer = answerChunks.join("");
    return cachedAnswer;
  };
  const getReasoning = () => {
    if (cachedReasoning != null) {
      return cachedReasoning;
    }
    cachedReasoning = reasoningChunks.join("");
    return cachedReasoning;
  };
  liveTurnCanonical.generation = generation;
  liveTurnCanonical.answer = "";
  liveTurnCanonical.reasoning = "";
  liveTurnCanonical.getAnswer = getAnswer;
  liveTurnCanonical.getReasoning = getReasoning;
  liveTurnCanonical.seq = 0;
  liveTurnCanonical.appliedSeq = -1;
  liveTurnCanonical.phase = "generating";
  const perf = {
    chunks: 0,
    renders: 0,
    renderTime: 0,
    skipped: 0,
    traces: [],
    longTasks: [],
    slowFrames: 0,
    tailInvariantViolations: 0,
    freezeMs: 0,
    appendMs: 0,
    followMs: 0,
    progressMs: 0,
    markdownMs: 0,
    markdownFreezes: 0,
    markdownDegraded: 0,
  };
  let renderTimer = 0;
  let renderAbort = null;
  let lastFlushAt = 0;
  let sawFirstToken = false;
  let stickToBottom = true;
  let answerNotifiedAt = 0;
  let reasoningRendered = false;
  let hiddenBuffered = false;
  let progressEl = null;
  let revealTimer = 0;
  let followRaf = 0;
  let followScrollQuiet = false;
  let streamAnchor = null;
  let longTaskObserver = null;
  let frameWatch = 0;
  let streamingFlush = false;
  let progressFlushTimer = 0;
  let heartbeatTimer = 0;
  let lastHeartbeat = performance.now();
  const qos = createStreamQos();
  heartbeatTimer = window.setInterval(() => {
    const now = performance.now();
    const lag = now - lastHeartbeat - 250;
    qos.metrics.maxEventLoopLagMs = Math.max(qos.metrics.maxEventLoopLagMs, lag);
    lastHeartbeat = now;
  }, 250);
  try {
    longTaskObserver = new PerformanceObserver((list) => {
      for (const entry of list.getEntries()) {
        if (perf.longTasks.length >= LONG_TASK_TRACE_LIMIT) {
          perf.longTasks.shift();
        }
        perf.longTasks.push({
          duration: Number(entry.duration.toFixed(1)),
          startTime: Number(entry.startTime.toFixed(0)),
        });
      }
    });
    longTaskObserver.observe({ type: "longtask", buffered: true });
  } catch {
    longTaskObserver = null;
  }
  const materializeCanonical = () => {
    const answer = getAnswer();
    const reasoning = getReasoning();
    liveTurnCanonical.answer = answer;
    liveTurnCanonical.reasoning = reasoning;
    delete liveTurnCanonical.getAnswer;
    delete liveTurnCanonical.getReasoning;
    return { answer, reasoning };
  };
  const progress = createProgressController({
    generationId: generation,
    onChange: (next) => {
      if (generation !== ctx.streamGeneration) {
        return;
      }
      if (!progressEl && next.revealed) {
        progressEl = appendProgressBlock();
        progressEl?.addEventListener("toggle", () => {
          pauseFollow();
          if (!progressEl.open || reasoningRendered || !state.complete) {
            return;
          }
          reasoningRendered = true;
          const raw = progressEl.querySelector(".agent-progress-raw");
          if (raw && ctx.reasoningVisible) {
            const thinking = getReasoning();
            raw.textContent = thinking;
            raw.dataset.mdSource = thinking;
            raw.hidden = false;
          }
        });
      }
      paintProgressView(progressEl, next, {
        reasoning: "",
        reasoningVisible: false,
      });
    },
  });
  revealTimer = window.setTimeout(() => {
    if (generation !== ctx.streamGeneration) {
      return;
    }
    if (answerLen >= MEANINGFUL_ANSWER_THRESHOLD) {
      return;
    }
    progress.dispatch({ type: "REVEAL" });
  }, PROGRESS_REVEAL_DELAY_MS);
  let answerRow = null;
  let answerBody = null;
  let frozenEl = null;
  let tailEl = null;
  let tailNode = null;
  let paintedTail = "";

  const setPhase = (phase) => {
    state.phase = phase;
    liveTurnCanonical.phase = phase;
    if (answerRow) {
      answerRow.dataset.streamPhase = phase;
    }
  };

  const syncCanonical = () => {
    liveTurnCanonical.seq = state.seq;
    liveTurnCanonical.appliedSeq = state.appliedSeq;
  };

  const adaptQuality = () => {
    /* Live tokens stay plain. Markdown/KaTeX on freeze slices is what pegs
       Brave on table replies (recursive JIT, 100% CPU, caret never moves). */
    if (!state.complete) {
      state.quality = "plain";
      return;
    }
    const avg = perf.renders ? perf.renderTime / perf.renders : 0;
    const last = state.lastFlushMs;
    let next = state.quality;
    if (avg > 20 || last > 16) {
      next = "plain";
    } else if (avg > 12 || last > STREAM_BUDGET_MS) {
      next = "simple";
    } else if (avg < 6 && last < 4 && next !== "rich" && !streamPrefersReducedMotion()) {
      next = next === "plain" ? "simple" : "rich";
    }
    if (next !== state.quality) {
      state.quality = next;
      answerRow?.classList.toggle("agent-stream-degraded", next !== "rich");
    }
  };

  const selectionInStream = () => {
    const sel = window.getSelection?.();
    if (!sel || sel.isCollapsed || !sel.rangeCount) {
      return false;
    }
    const node = sel.anchorNode;
    return Boolean(node && (answerRow?.contains(node) || progressEl?.contains(node)));
  };

  const followAfterFlush = () => {
    if (!stickToBottom || followRaf || selectionInStream()) {
      return;
    }
    followRaf = window.requestAnimationFrame(() => {
      followRaf = 0;
      if (!stickToBottom || selectionInStream() || !streamAnchor?.isConnected) {
        return;
      }
      followScrollQuiet = true;
      const t0 = performance.now();
      streamAnchor.scrollIntoView({ block: "end", inline: "nearest", behavior: "auto" });
      const took = performance.now() - t0;
      perf.followMs += took;
      const last = perf.traces[perf.traces.length - 1];
      if (last) {
        last.followMs = Number(took.toFixed(2));
      }
      window.requestAnimationFrame(() => {
        followScrollQuiet = false;
      });
    });
  };

  const setFollowBottom = (on) => {
    stickToBottom = on;
    scroller?.classList.toggle("is-follow-bottom", on);
    const btn = document.querySelector("[data-agent-jump-latest]");
    if (btn) {
      btn.hidden = on;
    }
    if (on) {
      followAfterFlush();
    }
  };

  const pauseFollow = () => {
    setFollowBottom(false);
  };

  const scroller = host.streamScrollEl?.();
  const streamRoot = host.streamEl?.();
  const onUserScroll = () => {
    if (followScrollQuiet || !scroller) {
      return;
    }
    const distance = scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight;
    if (distance > FOLLOW_RESUME_PX) {
      if (stickToBottom) {
        pauseFollow();
      }
      return;
    }
    if (!stickToBottom) {
      setFollowBottom(true);
    }
  };
  const onUserScrollUp = (event) => {
    if (event.deltaY < 0) {
      pauseFollow();
    }
  };
  const onKeyNav = (event) => {
    if (event.target?.closest?.("textarea, input, [contenteditable]")) {
      return;
    }
    if (event.key === "PageUp" || event.key === "Home" || event.key === "ArrowUp") {
      pauseFollow();
    }
  };
  const onSelectStart = (event) => {
    if (answerRow?.contains(event.target) || progressEl?.contains(event.target)) {
      pauseFollow();
    }
  };
  const onStreamPointer = (event) => {
    if (
      event.target.closest?.("pre, code, .agent-md-code, .agent-thinking, .agent-progress, a, button")
    ) {
      pauseFollow();
    }
  };
  const onContextMenu = (event) => {
    if (answerRow?.contains(event.target) || progressEl?.contains(event.target)) {
      pauseFollow();
    }
  };
  const onVisibility = () => {
    if (!streamTabHidden() && hiddenBuffered && generation === ctx.streamGeneration) {
      hiddenBuffered = false;
      flushStreamingUI(true);
    }
  };
  scroller?.classList.add("is-live-streaming");
  setFollowBottom(true);
  scroller?.addEventListener("scroll", onUserScroll, { passive: true });
  scroller?.addEventListener("wheel", onUserScrollUp, { passive: true });
  streamRoot?.addEventListener("selectstart", onSelectStart);
  streamRoot?.addEventListener("pointerdown", onStreamPointer);
  streamRoot?.addEventListener("contextmenu", onContextMenu);
  document.addEventListener("keydown", onKeyNav);
  document.addEventListener("visibilitychange", onVisibility);
  const jumpBtn = document.querySelector("[data-agent-jump-latest]");
  const onJumpLatest = () => setFollowBottom(true);
  jumpBtn?.addEventListener("click", onJumpLatest);
  const onResizeFollow = () => {
    if (stickToBottom) {
      followAfterFlush();
    }
  };
  window.addEventListener("resize", onResizeFollow, { passive: true });

  const ensureAnswerShell = () => {
    if (answerRow) {
      return;
    }
    answerRow = appendMessage("agent", "", { streaming: true });
    answerBody = answerRow?.querySelector(".agent-msg-body") || null;
    if (!answerBody) {
      return;
    }
    answerRow.dataset.streamPhase = state.phase;
    answerBody.textContent = "";
    delete answerBody.dataset.mdPainted;
    frozenEl = document.createElement("div");
    frozenEl.className = "agent-stream-frozen";
    tailEl = document.createElement("span");
    tailEl.className = "agent-stream-tail";
    tailNode = document.createTextNode("");
    tailEl.append(tailNode);
    streamAnchor = document.createElement("div");
    streamAnchor.className = "agent-stream-anchor";
    streamAnchor.setAttribute("aria-hidden", "true");
    answerBody.append(frozenEl, tailEl, streamAnchor);
  };

  const freezeSlice = (text, { markdown = false } = {}) => {
    if (!text || !frozenEl) {
      return;
    }
    const wrap = document.createElement("div");
    wrap.className = "agent-stream-frozen-block";
    const useMarkdown =
      markdown &&
      state.complete &&
      state.quality !== "plain" &&
      text.length <= RICH_MARKDOWN_MAX_CHARS &&
      !fenceOpen(text) &&
      !displayMathOpen(text);
    if (useMarkdown) {
      const t0 = performance.now();
      try {
        const html = renderMarkdown(text, { streaming: false });
        const ms = performance.now() - t0;
        perf.markdownMs += ms;
        perf.markdownFreezes += 1;
        if (ms > MARKDOWN_FREEZE_BUDGET_MS) {
          wrap.textContent = text;
          perf.markdownDegraded += 1;
        } else {
          const tmpl = document.createElement("template");
          tmpl.innerHTML = html;
          wrap.replaceChildren(tmpl.content);
          wrap.dataset.mdPainted = "1";
        }
      } catch {
        wrap.textContent = text;
        perf.markdownDegraded += 1;
      }
    } else {
      wrap.textContent = text;
    }
    frozenEl.append(wrap);
    state.frozenBlocks += 1;
  };

  const enforceTailBound = () => {
    let guard = 0;
    while (tail.length > maxTailChars(tail) && guard < 32) {
      guard += 1;
      let cut = chooseSemanticCut(tail, TARGET_FROZEN_BLOCK);
      if (cut < MIN_FROZEN_BLOCK) {
        cut = Math.min(TARGET_FROZEN_BLOCK, tail.length);
      }
      const slice = tail.slice(0, cut);
      const markdown =
        state.quality !== "plain" &&
        cutIsBlockBoundary(tail, cut) &&
        !fenceOpen(slice) &&
        !displayMathOpen(slice);
      freezeSlice(slice, { markdown });
      tail = tail.slice(cut);
      state.frozenLen += cut;
      paintedTail = "";
      if (tailNode) {
        tailNode.data = "";
      }
    }
  };

  const setTailDisplay = (next) => {
    if (!tailNode) {
      return;
    }
    if (next.startsWith(paintedTail)) {
      const extra = next.slice(paintedTail.length);
      if (extra) {
        tailNode.appendData(extra);
        paintedTail = next;
      }
      return;
    }
    tailNode.data = next;
    paintedTail = next;
  };

  const flushStreamingUI = (force = false) => {
    if (streamingFlush) {
      /* Never drain QoS with no-op handlers, and never recurse. */
      if (!state.renderQueued && (state.pendingAnswer || qos.queues.answer.length)) {
        scheduleRender();
      }
      return;
    }
    streamingFlush = true;
    try {
    if (generation !== ctx.streamGeneration) {
      return;
    }
    if (!force && state.seq <= state.appliedSeq) {
      return;
    }
    if (!force && streamTabHidden()) {
      hiddenBuffered = true;
      return;
    }
    const pending = state.pendingAnswer;
    const elapsed = Date.now() - lastFlushAt;
    if (!force && pending && !streamShouldFlush(pending, elapsed) && elapsed < 100) {
      scheduleRender();
      return;
    }
    if (!force && selectionInStream()) {
      perf.skipped += 1;
      return;
    }
    const started = performance.now();
    const quality = state.quality;
    let progressMs = 0;
    let freezeMs = 0;
    let appendMs = 0;
    let markdownBefore = perf.markdownMs;
    const qosFlush = qos.flushPresentation({
      applyAnswer: () => {},
      applyProgress: (item) => {
        if (item?.key === "thinking") {
          progress.dispatch({
            type: "PHASE_CHANGED",
            phase: ProgressPhase.ANALYZING,
            source: "system",
          });
          progress.dispatch({ type: "REVEAL" });
        } else if (item?.key === "answering" && !answerNotifiedAt && answerLen) {
          answerNotifiedAt = answerLen;
          progress.dispatch({
            type: "ANSWER_STARTED",
            chars: answerLen,
          });
        }
      },
    });
    if (qosFlush.reschedule && (state.pendingAnswer || qos.queues.answer.length) && !state.renderQueued) {
      scheduleRender();
    }
    if (state.pendingAnswer || force) {
      ensureAnswerShell();
      if (answerBody) {
        const f0 = performance.now();
        enforceTailBound();
        freezeMs = performance.now() - f0;
        if (tail.length > maxTailChars(tail) + 400) {
          perf.tailInvariantViolations += 1;
          console.error("STREAM TAIL INVARIANT VIOLATED", {
            tailChars: tail.length,
            answerChars: answerLen,
            fenceOpen: fenceOpen(tail),
          });
        }
        const a0 = performance.now();
        setTailDisplay(streamHoldIncomplete(tail));
        appendMs = performance.now() - a0;
      }
      state.pendingAnswer = "";
      if (!timings.t4 && answerLen) {
        timings.t4 = Date.now();
      }
      followAfterFlush();
    }
    state.appliedSeq = state.seq;
    syncCanonical();
    lastFlushAt = Date.now();
    const dur = performance.now() - started;
    state.lastFlushMs = dur;
    perf.renders += 1;
    perf.renderTime += dur;
    perf.freezeMs += freezeMs;
    perf.appendMs += appendMs;
    perf.progressMs += progressMs;
    if (perf.traces.length >= STREAM_TRACE_LIMIT) {
      perf.traces.shift();
    }
    perf.traces.push({
      t: lastFlushAt - t0,
      seq: state.seq,
      dur: Number(dur.toFixed(2)),
      freezeMs: Number(freezeMs.toFixed(2)),
      appendMs: Number(appendMs.toFixed(2)),
      progressMs: Number(progressMs.toFixed(2)),
      markdownMs: Number((perf.markdownMs - markdownBefore).toFixed(2)),
      quality,
      answerLen,
      tailLen: tail.length,
      frozenBlocks: state.frozenBlocks,
      hidden: streamTabHidden(),
      follow: stickToBottom,
    });
    adaptQuality();
    } finally {
      streamingFlush = false;
    }
  };

  const scheduleRender = () => {
    if (state.renderQueued) {
      return;
    }
    state.renderQueued = true;
    const elapsed = Date.now() - t0;
    const chars = answerLen + reasoningLen;
    const cps = elapsed > 0 ? (chars / elapsed) * 1000 : 0;
    let delay = streamChooseFlushDelay(cps);
    if (state.quality === "simple") {
      delay = Math.max(delay, 80);
    } else if (state.quality === "plain") {
      delay = Math.max(delay, 160);
    }
    if (state.lastFlushMs > STREAM_BUDGET_MS) {
      delay = Math.max(delay, 120);
    }
    if (streamTabHidden()) {
      delay = Math.max(delay, 800);
    }
    const run = () => {
      state.renderQueued = false;
      renderTimer = 0;
      flushStreamingUI(false);
    };
    /* Never scheduler.postTask(background): Brave starves it behind rAF/layout,
       renderQueued stays true, tokens never paint, caret sits empty. */
    renderTimer = window.setTimeout(run, delay);
  };

  try {
    const lastUser = [...(session?.messages || [])].reverse().find((m) => m.role === "user");
    const turnId = newTurnId();
    const reasoning = resolveReasoningPolicy(ctx.reasoningEffort || "medium");
    let turn = turnStorePut(
      createTurnManifest({
        turnId,
        inputParts: Array.isArray(lastUser?.parts) ? lastUser.parts : [],
        reasoning,
        startedAt: timings.t0,
      }),
    );
    const persistTurn = (next) => {
      turn = next || turn;
      liveTurn = turn;
      if (session) {
        session.lastTurn = cheapTurnSnapshot(turn);
      }
      try {
        host.persistAgentWorkspaceSoon?.();
      } catch {
        /* optional */
      }
    };
    persistTurn(turn);
    let compiled;
    try {
      turnStorePatch(turnId, { state: TurnState.COMPILING_CONTEXT });
      persistTurn(turnStoreGet(turnId) || turn);
      compiled = compileLiveContext({
        session,
        systemPrompt: ctx.systemPrompt,
        notes: ctx.agentNotes,
        maxTokens: ctx.maxTokens,
      });
      persistTurn(
        turnStorePatch(turnId, {
          state: TurnState.READY,
          contextManifestId: compiled.manifest.id,
          semanticContextHash: compiled.manifest.semanticContextHash,
          providerPayloadHash: compiled.manifest.providerPayloadHash,
          estimatedInputTokens: compiled.manifest.estimatedInputTokens,
          transcriptChars: compiled.manifest.transcriptChars,
        }),
      );
    } catch (err) {
      persistTurn(
        turnStorePatch(turnId, {
          state: TurnState.FAILED,
          error: String(err?.code || err?.message || "context").slice(0, 120),
          completedAt: Date.now(),
        }),
      );
      throw err;
    }
    const result = await streamChatViaContract(compiled.messages, {
      maxTokens: ctx.maxTokens,
      requestedEffort: ctx.reasoningEffort || "medium",
      contextManifest: compiled.manifest,
      turnManifest: turn,
      inputParts: Array.isArray(lastUser?.parts) ? lastUser.parts : [],
      onAccepted: ({ run_id } = {}) => {
          persistTurn(turnStoreGet(turnId));
          if (!timings.t1) {
            timings.t1 = Date.now();
          }
          progress.dispatch({
            type: "PHASE_CHANGED",
            phase: ProgressPhase.PLANNING,
            source: "system",
          });
        },
        onDelta: (payload) => {
          if (generation !== ctx.streamGeneration) {
            return;
          }
          const nextSeq = Number.isFinite(payload.seq) ? payload.seq : state.seq + 1;
          if (nextSeq <= state.seq) {
            return;
          }
          state.seq = nextSeq;
          perf.chunks += 1;
          if (!timings.t2) {
            timings.t2 = Date.now();
          }
          const reasoningDelta = payload.reasoningDelta
            || (payload.reasoning && payload.reasoning.length > reasoningLen
              ? payload.reasoning.slice(reasoningLen)
              : "");
          const contentDelta = payload.contentDelta
            || (payload.content && payload.content.length > answerLen
              ? payload.content.slice(answerLen)
              : "");
          if (!reasoningDelta && !contentDelta) {
            return;
          }
          if (reasoningDelta) {
            cachedReasoning = null;
            reasoningChunks.push(reasoningDelta);
            reasoningLen += reasoningDelta.length;
            qos.ingestEvent({ type: "thinking.delta", delta: reasoningDelta });
            if (!progressFlushTimer) {
              progressFlushTimer = window.setTimeout(() => {
                progressFlushTimer = 0;
                qos.flushPresentation({
                  applyAnswer: () => {},
                  applyProgress: (item) => {
                    if (item?.key !== "thinking") {
                      return;
                    }
                    progress.dispatch({
                      type: "PHASE_CHANGED",
                      phase: ProgressPhase.ANALYZING,
                      source: "system",
                    });
                    progress.dispatch({ type: "REVEAL" });
                  },
                });
              }, 0);
            }
          }
          if (contentDelta) {
            cachedAnswer = null;
            answerChunks.push(contentDelta);
            answerLen += contentDelta.length;
            tail += contentDelta;
            state.pendingAnswer += contentDelta;
            qos.ingestEvent({ type: "answer.delta", delta: contentDelta });
            if (/\S/.test(tail) && !answerRow) {
              ensureAnswerShell();
              setTailDisplay(streamHoldIncomplete(tail.length > 800 ? tail.slice(-800) : tail));
            }
            if (!answerNotifiedAt) {
              answerNotifiedAt = answerLen;
              progress.dispatch({
                type: "ANSWER_STARTED",
                chars: answerLen,
              });
            } else if (
              answerNotifiedAt < MEANINGFUL_ANSWER_THRESHOLD &&
              answerLen >= MEANINGFUL_ANSWER_THRESHOLD
            ) {
              answerNotifiedAt = answerLen;
              progress.dispatch({
                type: "ANSWER_STARTED",
                chars: answerLen,
              });
            }
          }
          syncCanonical();
          if (!sawFirstToken && (reasoningLen || answerLen)) {
            sawFirstToken = true;
            timings.t3 = Date.now();
          }
          if (contentDelta) {
            scheduleRender();
          }
        },
      },
    );
    timings.t5 = Date.now();
    if (generation !== ctx.streamGeneration) {
      return;
    }
    /* Unlock before joining the canonical string. */
    ctx.turnBusy = false;
    setAgentComposerProcessing(false);
    host.setComposerGeometrySuspended?.(false);
    setStreamStatus("");
    state.complete = true;
    setPhase("model_done");
    const joined = materializeCanonical();
    syncCanonical();
    state.renderQueued = false;
    renderAbort?.abort();
    renderAbort = null;
    if (renderTimer) {
      window.clearTimeout(renderTimer);
      renderTimer = 0;
    }
    setPhase("finalizing");
    qos.ingestEvent({ type: "completed" });
    flushStreamingUI(true);

    let answer = String(joined.answer || "").trim();
    let thinking = String(joined.reasoning || "").trim();
    if (!answer && thinking) {
      /* Reasoning models often spend the budget thinking — surface it honestly.
         Do not one-shot markdown a Think dump; that is what crashed Brave. */
      answer = thinking;
      thinking = "";
      state.quality = "plain";
    }
    const split = splitThinkTaggedContent(answer);
    if (split.thinking && !thinking) {
      thinking = split.thinking;
      answer = split.answer || answer;
    }

    const superseded = generation !== ctx.streamGeneration;
    const stoppedEarly = result.aborted || superseded;
    if (superseded) {
      return;
    }

    progress.dispatch({
      type: stoppedEarly ? "GENERATION_STOPPED" : "GENERATION_DONE",
    });
    const progressSnap = snapshotProgress(progress.getState());
    if (progressEl && !progressSnap?.milestones?.length && !thinking) {
      progressEl.remove();
      progressEl = null;
    }

    if (!answer) {
      progressEl?.remove();
      answerRow?.remove();
      if (!stoppedEarly) {
        setStreamStatus("Model returned an empty reply", { tone: "error" });
        void probeLiveInference({ force: true });
      }
      return;
    }

    if (!answerRow) {
      answerRow = appendMessage("agent", answer, { streaming: true });
      answerBody = answerRow?.querySelector(".agent-msg-body") || null;
    }
    answerRow?.classList.remove("is-streaming");
    const answerBodyRef = answerBody;
    const answerTextRef = answer;
    const finalize = ({ abandonRich = false } = {}) => {
      if (generation !== ctx.streamGeneration || !answerBodyRef?.isConnected) {
        return;
      }
      const unhealthy =
        abandonRich ||
        perf.slowFrames > 12 ||
        perf.longTasks.some((row) => row.duration >= 50);
      answerBodyRef.dataset.mdSource = answerTextRef;
      const skipRich = unhealthy || stoppedEarly || state.quality === "plain";
      if (!skipRich) {
        try {
          if (frozenEl?.isConnected) {
            const remaining = String(tailNode?.data || tail || "");
            if (remaining) {
              freezeSlice(remaining, { markdown: true });
            }
            tailEl?.remove();
            tailEl = null;
            tailNode = null;
          } else {
            paintAgentMessageBody(answerBodyRef, answerTextRef, {
              streaming: false,
              allowDegrade: true,
            });
          }
        } catch {
          /* keep streamed prefix — canonical survives presentation failure */
        }
      }
      timings.t6 = Date.now();
      setPhase("presentation_done");
      window.requestAnimationFrame(() => drainFollowUpQueue());
      if (perf.renders) {
        const ms = (a, b) => (a && b ? a - b : 0);
        const durs = perf.traces.map((row) => row.dur);
        const answerLens = perf.traces.map((row) => row.answerLen);
        const markdownLens = perf.traces.map((row) => row.markdownMs || 0);
        const corrAnswer = streamPearson(answerLens, durs);
        const corrMarkdown = streamPearson(answerLens, markdownLens);
        const corrTail = streamPearson(
          perf.traces.map((row) => row.tailLen),
          durs,
        );
        const slope = streamSlopePerChar(answerLens, durs);
        const slopeMarkdown = streamSlopePerChar(answerLens, markdownLens);
        const buckets = streamBucketP95(perf.traces);
        console.table({
          chunksReceived: perf.chunks,
          DOMRenders: perf.renders,
          skippedFlushes: perf.skipped,
          avgRenderMs: Number((perf.renderTime / perf.renders).toFixed(2)),
          p95RenderMs: Number(streamPercentile(durs, 95).toFixed(2)),
          maxRenderMs: Number(Math.max(0, ...durs).toFixed(2)),
          avgFreezeMs: Number((perf.freezeMs / perf.renders).toFixed(2)),
          avgAppendMs: Number((perf.appendMs / perf.renders).toFixed(2)),
          avgFollowMs: Number((perf.followMs / Math.max(1, perf.renders)).toFixed(2)),
          avgProgressMs: Number((perf.progressMs / perf.renders).toFixed(2)),
          markdownFreezes: perf.markdownFreezes,
          markdownDegraded: perf.markdownDegraded,
          avgMarkdownMs: perf.markdownFreezes
            ? Number((perf.markdownMs / perf.markdownFreezes).toFixed(2))
            : 0,
          longTasks: perf.longTasks.length,
          longTaskMaxMs: perf.longTasks.length
            ? Number(Math.max(...perf.longTasks.map((row) => row.duration)).toFixed(1))
            : 0,
          slowFrames: perf.slowFrames,
          tailInvariantViolations: perf.tailInvariantViolations,
          quality: state.quality,
          answerChars: answerLen,
          frozenChars: state.frozenLen,
          frozenBlocks: state.frozenBlocks,
          tailChars: tail.length,
          reasoningChars: reasoningLen,
          transcriptMsgs: host.streamEl?.()?.querySelectorAll(".agent-msg").length || 0,
          corrAnswerRender: corrAnswer == null ? "n/a" : Number(corrAnswer.toFixed(3)),
          corrAnswerMarkdown: corrMarkdown == null ? "n/a" : Number(corrMarkdown.toFixed(3)),
          corrTailRender: corrTail == null ? "n/a" : Number(corrTail.toFixed(3)),
          slopeRenderPer10kChars: slope == null ? "n/a" : Number((slope * 10000).toFixed(3)),
          slopeMarkdownPer10kChars:
            slopeMarkdown == null ? "n/a" : Number((slopeMarkdown * 10000).toFixed(3)),
          ...buckets,
          T1_accepted: ms(timings.t1, timings.t0),
          T2_started: ms(timings.t2, timings.t0),
          T3_firstToken: ms(timings.t3, timings.t0),
          T4_firstPaint: ms(timings.t4, timings.t0),
          T5_lastToken: ms(timings.t5, timings.t0),
          T6_uiComplete: ms(timings.t6, timings.t0),
          renderLag_T4minusT3: ms(timings.t4, timings.t3),
          finalizeLag_T6minusT5: ms(timings.t6, timings.t5),
        });
      }
    };
    scheduleIdleWork(finalize);
    const regen = answerRow?.querySelector("[data-regenerate]");
    if (regen) {
      regen.disabled = false;
    }

    const turnUsage = noteLiveTurnUsage({
      usage: result.usage || null,
      latencyMs: result.latencyMs || 0,
      model: selectedLiveOffer()?.label || getLiveInferenceState()?.model || "live",
      content: answer,
      reasoning: thinking,
      source: "live",
    });
    paintTurnUsageMeta(answerRow, turnUsage);
    try {
      host.renderHarnessPage?.();
    } catch {
      /* usage page refresh optional */
    }

    if (session) {
      commitAgentReply(session, answer, thinking, {
        partial: stoppedEarly,
        progress: progressSnap,
        run: result?.reasoning
          ? { ...result.reasoning, contextHash: result.contextHash }
          : null,
        turn: result?.turnManifest || liveTurn,
      });
      if (result?.turnManifest) {
        liveTurn = result.turnManifest;
        session.lastTurn = cheapTurnSnapshot(liveTurn);
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
    host.syncTruthStrip?.();
    renderHarnessPage();
  } catch (err) {
    if (generation !== ctx.streamGeneration) {
      return;
    }
    const partial = materializeCanonical();
    const partialAnswer = String(partial.answer || "").trim();
    const partialThinking = String(partial.reasoning || "").trim();
    if (err?.code === "aborted") {
      if (liveTurn?.turnId) {
        const stopped = turnStorePatch(liveTurn.turnId, {
          state: TurnState.STOPPED,
          completedAt: Date.now(),
        });
        if (session) {
          session.lastTurn = cheapTurnSnapshot(stopped || liveTurn);
        }
      }
      progress.dispatch({ type: "GENERATION_STOPPED" });
      if (partialAnswer || partialThinking) {
        persistPartialAgentReply(partialAnswer || partialThinking, partialThinking);
      }
      return;
    }
    const honest = formatStreamError(err);
    if (liveTurn?.turnId && liveTurn.state !== TurnState.FAILED) {
      const failed = turnStorePatch(liveTurn.turnId, {
        state: TurnState.FAILED,
        error: String(err?.code || honest).slice(0, 120),
        completedAt: Date.now(),
      });
      if (session) {
        session.lastTurn = cheapTurnSnapshot(failed || liveTurn);
      }
    }
    progress.dispatch({ type: "GENERATION_ERROR", text: honest });
    if (partialAnswer) {
      persistPartialAgentReply(partialAnswer, partialThinking);
      answerRow?.classList.remove("is-streaming");
      if (answerBody) {
        answerBody.dataset.mdSource = partialAnswer;
      }
      if (answerRow && !answerRow.querySelector(".agent-msg-stopped")) {
        const note = document.createElement("div");
        note.className = "agent-msg-stopped";
        note.innerHTML =
          `<span>${escapeHtml(honest)}. Continue?</span>` +
          `<button type="button" class="agent-msg-retry" data-retry="1">Retry</button>`;
        answerRow.append(note);
      }
      setStreamStatus(honest, { tone: "error" });
      return;
    }
    setStreamStatus(honest, { tone: "error" });
    progressEl?.remove();
    answerRow?.remove();
    void probeLiveInference({ force: true });
    return;
  } finally {
    longTaskObserver?.disconnect();
    if (heartbeatTimer) {
      window.clearInterval(heartbeatTimer);
      heartbeatTimer = 0;
    }
    if (progressFlushTimer) {
      window.clearTimeout(progressFlushTimer);
      progressFlushTimer = 0;
    }
    try {
      console.info("[home-stream-qos]", {
        ...qos.metrics,
        answerLen,
        reasoningLen,
        renders: perf.renders,
      });
    } catch {
      /* telemetry optional */
    }
    if (frameWatch) {
      window.cancelAnimationFrame(frameWatch);
      frameWatch = 0;
    }
    if (followRaf) {
      window.cancelAnimationFrame(followRaf);
      followRaf = 0;
    }
    window.removeEventListener("resize", onResizeFollow);
    document.removeEventListener("keydown", onKeyNav);
    scroller?.classList.remove("is-live-streaming", "is-follow-bottom");
    scroller?.removeEventListener("scroll", onUserScroll);
    scroller?.removeEventListener("wheel", onUserScrollUp);
    jumpBtn?.removeEventListener("click", onJumpLatest);
    streamRoot?.removeEventListener("selectstart", onSelectStart);
    streamRoot?.removeEventListener("pointerdown", onStreamPointer);
    streamRoot?.removeEventListener("contextmenu", onContextMenu);
    document.removeEventListener("visibilitychange", onVisibility);
    if (revealTimer) {
      window.clearTimeout(revealTimer);
    }
    progress.destroy();
    renderAbort?.abort();
    if (renderTimer) {
      window.clearTimeout(renderTimer);
    }
    host.setComposerGeometrySuspended?.(false);
    if (generation === ctx.streamGeneration) {
      ctx.turnBusy = false;
      setAgentComposerProcessing(false);
      const status = document.querySelector("[data-agent-stream-status]")?.textContent || "";
      if (status === "Generating…" || status === "Thinking…" || status === "Connecting…") {
        setStreamStatus("");
      }
      if (state.phase !== "finalizing") {
        window.requestAnimationFrame(() => drainFollowUpQueue());
      }
    }
  }
}
