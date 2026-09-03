/* Harness state that is real on this Runtime: what the user chose, what live
   turns cost, which projects exist. Persisted through the capsule workspace.
   Nothing here invents a model, a tool or a reply. */

const MAX_USAGE_TURNS = 200;

let reasoningVisibleStore = true;
let usageTurns = [];
let projectStore = [];

/** Thinking visibility preference (fx7). Default on. Host snapshot persists. */
export function loadReasoningVisible() {
  return reasoningVisibleStore !== false;
}

export function setReasoningVisible(visible) {
  reasoningVisibleStore = Boolean(visible);
  return reasoningVisibleStore;
}

/**
 * Split model text that may contain <think>…</think> / <thinking> / <analysis>.
 * @returns {{ thinking: string, answer: string }}
 */
export function splitThinkTaggedContent(raw = "") {
  const text = String(raw || "");
  const open = /<(think|thinking|analysis)(?:\s[^>]*)?>/i;
  const close = /<\/(think|thinking|analysis)>/i;
  const openMatch = text.match(open);
  if (!openMatch) {
    return { thinking: "", answer: text };
  }
  const afterOpen = text.slice(openMatch.index + openMatch[0].length);
  const closeMatch = afterOpen.match(close);
  if (!closeMatch) {
    return { thinking: afterOpen.trim(), answer: text.slice(0, openMatch.index).trim() };
  }
  const thinking = afterOpen.slice(0, closeMatch.index).trim();
  const before = text.slice(0, openMatch.index).trim();
  const after = afterOpen.slice(closeMatch.index + closeMatch[0].length).trim();
  return {
    thinking,
    answer: [before, after].filter(Boolean).join("\n\n"),
  };
}

function estimateTokensFromText(text) {
  const chars = String(text || "").length;
  return Math.max(1, Math.round(chars / 4));
}

export function noteLiveTurnUsage({
  usage = null,
  latencyMs = 0,
  model = "",
  content = "",
  reasoning = "",
  source = "live",
} = {}) {
  const promptTokens = Number(usage?.prompt_tokens);
  const completionTokens = Number(usage?.completion_tokens);
  const totalFromUpstream = Number(usage?.total_tokens);
  let total = Number.isFinite(totalFromUpstream) ? totalFromUpstream : NaN;
  let omitted = false;
  if (!Number.isFinite(total) || total <= 0) {
    const prompt = Number.isFinite(promptTokens) ? promptTokens : 0;
    const completion = Number.isFinite(completionTokens)
      ? completionTokens
      : estimateTokensFromText(`${reasoning || ""}${content || ""}`);
    total = prompt + completion;
    omitted = !Number.isFinite(promptTokens) && !Number.isFinite(completionTokens);
  }
  const day = new Date().toISOString().slice(0, 10);
  usageTurns.push({
    at: Date.now(),
    day,
    tokens: Math.max(0, Math.round(total)),
    promptTokens: Number.isFinite(promptTokens) ? Math.round(promptTokens) : null,
    completionTokens: Number.isFinite(completionTokens)
      ? Math.round(completionTokens)
      : null,
    latencyMs: Math.max(0, Math.round(Number(latencyMs) || 0)),
    model: String(model || "live").slice(0, 80),
    source: omitted ? "estimated" : "live",
    omitted,
  });
  if (usageTurns.length > MAX_USAGE_TURNS) {
    usageTurns = usageTurns.slice(-MAX_USAGE_TURNS);
  }
  return usageTurns[usageTurns.length - 1];
}

export function getUsageLedger() {
  return usageTurns.map((t) => ({ ...t }));
}

export function applyUsageLedger(rawTurns) {
  if (!Array.isArray(rawTurns)) {
    return;
  }
  usageTurns = rawTurns
    .filter((t) => t && typeof t === "object")
    .map((t) => ({
      at: Number(t.at) || Date.now(),
      day: String(t.day || "").slice(0, 10),
      tokens: Math.max(0, Math.round(Number(t.tokens) || 0)),
      promptTokens: t.promptTokens == null ? null : Math.round(Number(t.promptTokens) || 0),
      completionTokens:
        t.completionTokens == null ? null : Math.round(Number(t.completionTokens) || 0),
      latencyMs: Math.max(0, Math.round(Number(t.latencyMs) || 0)),
      model: String(t.model || "live").slice(0, 80),
      source: t.source === "estimated" ? "estimated" : "live",
      omitted: Boolean(t.omitted),
    }))
    .slice(-MAX_USAGE_TURNS);
}

function readProjectStore() {
  return Array.isArray(projectStore) ? projectStore : [];
}

function writeProjectStore(projects) {
  projectStore = Array.isArray(projects) ? projects : [];
}

export function listProjects() {
  return readProjectStore().map((p) => ({ ...p }));
}

export function createProject(title) {
  const name = String(title || "").trim();
  if (!name) {
    return null;
  }
  const projectBytes = new Uint8Array(4);
  globalThis.crypto.getRandomValues(projectBytes);
  const projectSuffix = Array.from(projectBytes, (byte) => byte.toString(16).padStart(2, "0")).join("");
  const project = {
    id: `proj-${Date.now()}-${projectSuffix}`,
    title: name,
    rootRef: null,
    createdAt: Date.now(),
  };
  writeProjectStore([project, ...readProjectStore()]);
  return { ...project };
}

export function removeProject(projectId) {
  const id = String(projectId || "");
  writeProjectStore(readProjectStore().filter((p) => p.id !== id));
  return true;
}

/** Replace the in-memory project list (host session restore). */
export function replaceProjects(projects) {
  const next = Array.isArray(projects)
    ? projects
        .filter((p) => p && typeof p === "object" && typeof p.id === "string" && p.title)
        .map((p) => ({
          id: String(p.id).slice(0, 80),
          title: String(p.title).trim().slice(0, 48),
          rootRef: p.rootRef && typeof p.rootRef === "object" ? p.rootRef : null,
          createdAt: Number(p.createdAt) || Date.now(),
        }))
        .filter((p) => p.title)
        .slice(0, 40)
    : [];
  writeProjectStore(next);
  return listProjects();
}
