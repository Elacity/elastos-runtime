/* Preview mock agent provider — stable seams for fx4 → w1/w3.
   UI ≠ authority (Principle 16): never mints Carrier/Capsule grants,
   never calls live ai-provider. Label everything Preview · mock. */

import { TIP } from "./agent-tip.js?v=home-20260804ay";

let planMarkdown = `### To-dos
- [ ] Clarify what to build
- [/] Sketch capsule surface
- [ ] Declare capabilities
- [ ] Local install (later)`;

/** Selected mock model id — presentation only until w1. */
let selectedModelId = "local-preview";

/** Mock context fill 0–1 for truth-strip meter (fx6). */
let mockContextRatio = 0.12;

/** In-memory only — host session.agent is the durable store. */
let reasoningVisibleStore = true;

export const MOCK_THINKING =
  "Check locality first — answer must stay on this device.\n" +
  "Tools start at zero; only surface a grant if the user asked for files.\n" +
  "Keep the reply calm, short, and honest that this is preview mock.";

export const MOCK_REPLY =
  "I'm a local preview on this machine — not live inference yet.\n\n" +
  "I start with **no tools**. If you need Downloads or other capsule access, " +
  "you'll grant it explicitly (Inbox-style). Nothing ambient.\n\n" +
  "```text\nTools: none\nLocality: this device\n```";

/** @typedef {"none"|"pending"|"granted"|"denied"} CapState */

/** @type {Map<string, { id: string, label: string, state: CapState, tier: number }>} */
const capabilities = new Map([
  [
    "library.read",
    {
      id: "library.read",
      label: "Library · Read",
      state: "none",
      tier: 1,
    },
  ],
  [
    "wallet.sign",
    {
      id: "wallet.sign",
      label: "Wallet · Sign",
      state: "none",
      tier: 4,
    },
  ],
]);

/** @type {Map<string, { approvalId: string, toolId: string, args: object, status: string }>} */
const pendingApprovals = new Map();
let approvalSeq = 0;

/**
 * Mock model catalog — presentation until Spark/`ai-provider` lists real weights.
 * status: installed (selectable) | available (recommend download) | blocked (needs hardware/Spark)
 */
const MODEL_CATALOG = [
  {
    id: "local-preview",
    label: "Local preview",
    tier: "preview",
    hwTier: "nano",
    status: "installed",
    fitsDevice: true,
    sizeLabel: "",
    detail: "On this device · preview path · not a downloaded weight file",
    tokensPerSecEstimate: null,
    vramGbEstimate: null,
  },
  {
    id: "qwen-stub-small",
    label: "Qwen 2.5 · 3B",
    tier: "fits",
    hwTier: "nano",
    status: "available",
    fitsDevice: true,
    sizeLabel: "~2 GB",
    detail: "Preview stub — Get installs in-session only, not real weights on disk",
    tokensPerSecEstimate: 28,
    vramGbEstimate: 4,
  },
  {
    id: "llama-stub-mini",
    label: "Llama 3.2 · 1B",
    tier: "fits",
    hwTier: "nano",
    status: "available",
    fitsDevice: true,
    sizeLabel: "~1.3 GB",
    detail: "Fast answers · best fit when RAM is tight",
    tokensPerSecEstimate: 40,
    vramGbEstimate: 2,
  },
  {
    id: "qwen-stub-coder",
    label: "Qwen 2.5 Coder · 7B",
    tier: "fits",
    hwTier: "mini",
    status: "available",
    fitsDevice: true,
    sizeLabel: "~4.5 GB",
    detail: "Better for Build / capsule drafts",
    tokensPerSecEstimate: 18,
    vramGbEstimate: 6,
  },
  {
    id: "spark-stub",
    label: "Large local · 70B",
    tier: "unsupported",
    hwTier: "medium",
    status: "blocked",
    fitsDevice: false,
    sizeLabel: "~40 GB",
    detail: "Needs Spark-class GPU — not this device",
    tokensPerSecEstimate: null,
    vramGbEstimate: 48,
  },
];

const HW_TIER_BLURB = {
  nano: "Nano · laptop / light local",
  mini: "Mini · 24–48 GB class",
  medium: "Medium · large unified / multi-GPU",
};

/* ---- Hardware estimate (browser-side, honest) ----------------------------
   The real probe belongs to the runtime (`ai-provider`, Spark/W2) — same split
   as Local Studio's controller-side diagnostics. Until then the browser can
   only estimate: CPU cores, coarse memory (Chrome caps deviceMemory at 8),
   GPU family via WebGPU. Everything here is labeled "estimate". */

/** @type {{ cores: number|null, poolGb: number|null, memLabel: string, gpuLabel: string, deviceLabel: string, source: "browser-estimate" }|null} */
let hardwareEstimate = null;
let hardwareProbePromise = null;

export function probeHardwareEstimate() {
  if (hardwareProbePromise) {
    return hardwareProbePromise;
  }
  hardwareProbePromise = (async () => {
    const cores = Number(navigator.hardwareConcurrency) || null;
    /* deviceMemory caps at 8 — treat as a conservative floor, label "8+". */
    const rawMem = Number(navigator.deviceMemory) || null;
    const poolGb = rawMem;
    const memLabel = rawMem ? (rawMem >= 8 ? "8+ GB" : `${rawMem} GB`) : "memory unknown";
    let gpuLabel = "";
    try {
      const adapter = await navigator.gpu?.requestAdapter?.();
      const info = adapter?.info;
      const arch = info?.architecture || info?.vendor || "";
      if (arch) {
        gpuLabel = arch.includes("apple") || arch.includes("metal")
          ? "Apple GPU"
          : arch.replace(/^./, (c) => c.toUpperCase());
      }
    } catch {
      /* WebGPU unavailable — leave blank, stay honest. */
    }
    const parts = [];
    if (gpuLabel) {
      parts.push(gpuLabel);
    }
    if (cores) {
      parts.push(`${cores} cores`);
    }
    parts.push(memLabel);
    hardwareEstimate = {
      cores,
      poolGb,
      memLabel,
      gpuLabel,
      deviceLabel: `This device · ${parts.join(" · ")}`,
      source: "browser-estimate",
    };
    return hardwareEstimate;
  })();
  return hardwareProbePromise;
}

export function getHardwareEstimate() {
  return hardwareEstimate;
}

/**
 * Fit vs the memory pool estimate (unified-memory budget, Local Studio style).
 * Falls back to the catalog's static flag when the browser gave us nothing.
 * @returns {"fits"|"blocked"}
 */
export function fitForModel(model) {
  if (model.status === "blocked" || model.fitsDevice === false) {
    return "blocked";
  }
  const pool = hardwareEstimate?.poolGb;
  if (!pool || model.vramGbEstimate == null) {
    return model.fitsDevice ? "fits" : "blocked";
  }
  return model.vramGbEstimate <= pool ? "fits" : "blocked";
}

/** Best single pick for this hardware — strongest model that fits; installed breaks ties. */
export function recommendedModel() {
  const candidates = listModels()
    .filter((m) => m.tier !== "preview" && fitForModel(m) === "fits")
    .sort((a, b) => {
      const sizeDelta = (b.vramGbEstimate || 0) - (a.vramGbEstimate || 0);
      if (sizeDelta !== 0) {
        return sizeDelta;
      }
      return (b.status === "installed" ? 1 : 0) - (a.status === "installed" ? 1 : 0);
    });
  return candidates[0] || listInstalledModels()[0] || null;
}

export function listModels() {
  return MODEL_CATALOG.map((m) => ({ ...m }));
}

export function listInstalledModels() {
  return listModels().filter((m) => m.status === "installed");
}

export function listRecommendedModels() {
  return listModels().filter((m) => m.status === "available" || m.status === "blocked");
}

export function getSelectedModelId() {
  return selectedModelId;
}

export function setSelectedModelId(modelId) {
  const match = MODEL_CATALOG.find((m) => m.id === modelId && m.status === "installed");
  if (match) {
    selectedModelId = match.id;
  }
  return getSelectedModel();
}

/** Preview-only: mark an available stub as installed + select it. No real download. */
export function mockInstallModel(modelId) {
  const match = MODEL_CATALOG.find((m) => m.id === modelId);
  if (!match) {
    return { ok: false, reason: "missing", model: null };
  }
  if (match.status === "blocked") {
    return { ok: false, reason: "blocked", model: { ...match } };
  }
  match.status = "installed";
  selectedModelId = match.id;
  return { ok: true, reason: "preview-install", model: { ...match } };
}

export function getSelectedModel() {
  return (
    listModels().find((m) => m.id === selectedModelId && m.status === "installed") ||
    listInstalledModels()[0] ||
    listModels()[0]
  );
}

/** Advance mock context after a turn (presentation only). */
export function noteMockTurnTokens(approxTokens = 400) {
  const bump = Math.min(0.08, Math.max(0.01, approxTokens / 12000));
  mockContextRatio = Math.min(0.92, mockContextRatio + bump);
}

export function resetMockContextMeter() {
  mockContextRatio = 0.12;
}

/**
 * Truth-strip snapshot for harness (fx6). UI ≠ authority.
 * @returns {{
 *   preview: true,
 *   locality: string,
 *   modelLabel: string,
 *   modelTier: string,
 *   toolsLabel: string,
 *   toolsState: "none"|"pending"|"live",
 *   contextRatio: number,
 *   contextLabel: string,
 *   hwLabel: string,
 *   hwState: "unknown"|"ok"|"warn"
 * }}
 */
export function getTruthSnapshot() {
  const model = getSelectedModel();
  const toolsLabel = toolsSummaryLabel();
  const toolsState = listCapabilities().some((c) => c.state === "granted")
    ? "live"
    : listCapabilities().some((c) => c.state === "pending")
      ? "pending"
      : "none";
  const pct = Math.round(mockContextRatio * 100);
  let hwLabel = "HW unmeasured";
  let hwState = "unknown";
  if (model.tier === "unsupported") {
    hwLabel = "Needs Spark";
    hwState = "warn";
  } else if (hardwareEstimate?.poolGb) {
    hwLabel = `${hardwareEstimate.memLabel} est`;
    hwState = "ok";
  } else if (model.tokensPerSecEstimate != null) {
    hwLabel = `~${model.tokensPerSecEstimate} tok/s stub`;
    hwState = "ok";
  } else if (model.vramGbEstimate != null) {
    hwLabel = `~${model.vramGbEstimate} GB stub`;
    hwState = "ok";
  }
  return {
    preview: true,
    locality: "On this device",
    modelLabel: model.label,
    modelTier: model.tier,
    toolsLabel,
    toolsState,
    contextRatio: mockContextRatio,
    contextLabel: `${pct}% context`,
    hwLabel,
    hwState,
  };
}

export function listCapabilities() {
  return [...capabilities.values()].map((cap) => ({ ...cap }));
}

export function toolsSummaryLabel() {
  const caps = listCapabilities();
  const granted = caps.filter((c) => c.state === "granted");
  const pending = caps.filter((c) => c.state === "pending");
  if (granted.length) {
    return `Tools: ${granted.map((c) => c.label).join(", ")}`;
  }
  if (pending.length) {
    return `Tools: pending review`;
  }
  return "Tools: none";
}

/**
 * @param {{ toolId: string, args?: object }} opts
 * @returns {{ status: "needs_approval"|"denied"|"ok", approvalId?: string, toolId: string, label: string, summary: string, scope: string, preview: true, result?: string, reason?: string }}
 */
export function requestTool({ toolId, args = {} }) {
  const cap = capabilities.get(toolId);
  if (!cap) {
    return {
      status: "denied",
      toolId,
      label: toolId,
      summary: "Unknown tool",
      scope: "",
      preview: true,
      reason: "Unknown tool",
    };
  }
  if (cap.tier >= 4) {
    cap.state = "denied";
    return {
      status: "denied",
      toolId,
      label: cap.label,
      summary: "Wallet / signing needs a human ceremony — never Approve for me.",
      scope: "human ceremony only · never ambient",
      preview: true,
      reason: "tier-4-forbidden",
    };
  }
  if (cap.state === "granted") {
    return {
      status: "ok",
      toolId,
      label: cap.label,
      summary: summaryFor(toolId, args),
      scope: scopeFor(cap),
      preview: true,
      result: mockToolResult(toolId, args),
    };
  }
  if (cap.state === "pending") {
    const existing = [...pendingApprovals.values()].find((r) => r.toolId === toolId);
    if (existing) {
      return {
        status: "needs_approval",
        approvalId: existing.approvalId,
        toolId,
        label: cap.label,
        summary: summaryFor(toolId, existing.args || args),
        scope: scopeFor(cap),
        preview: true,
      };
    }
  }
  const approvalId = `mock-approval-${(approvalSeq += 1)}`;
  cap.state = "pending";
  pendingApprovals.set(approvalId, {
    approvalId,
    toolId,
    args,
    status: "pending",
  });
  return {
    status: "needs_approval",
    approvalId,
    toolId,
    label: cap.label,
    summary: summaryFor(toolId, args),
    scope: scopeFor(cap),
    preview: true,
  };
}

/**
 * Mock-only resolve. Does not call Capsule/Carrier.
 * @param {{ approvalId?: string, toolId?: string, decision: "allow_once"|"deny" }} opts
 */
export function resolveMockApproval({ approvalId, toolId, decision }) {
  let record = approvalId ? pendingApprovals.get(approvalId) : null;
  if (!record && toolId) {
    record = [...pendingApprovals.values()].find((r) => r.toolId === toolId) || null;
  }
  const id = record?.toolId || toolId;
  const cap = id ? capabilities.get(id) : null;
  if (!cap) {
    return { status: "denied", preview: true, reason: "Unknown approval" };
  }
  if (cap.tier >= 4) {
    cap.state = "denied";
    if (record) {
      pendingApprovals.delete(record.approvalId);
    }
    return {
      status: "denied",
      preview: true,
      toolId: cap.id,
      label: cap.label,
      reason: "tier-4-forbidden",
    };
  }
  if (decision === "allow_once") {
    /* One-shot only — do not sticky-grant the session (label honesty). */
    cap.state = "none";
    if (record) {
      pendingApprovals.delete(record.approvalId);
    }
    return {
      status: "ok",
      preview: true,
      once: true,
      toolId: cap.id,
      label: cap.label,
      result: mockToolResult(cap.id, record?.args || {}),
    };
  }
  cap.state = "denied";
  if (record) {
    pendingApprovals.delete(record.approvalId);
  }
  return {
    status: "denied",
    preview: true,
    toolId: cap.id,
    label: cap.label,
  };
}

/** Reset mock grants (session hydrate / leave harness). */
export function resetMockCapabilities() {
  for (const cap of capabilities.values()) {
    cap.state = "none";
  }
  pendingApprovals.clear();
  resetMockContextMeter();
}

/** Paint mock capability state from a persisted session grant (preview only). */
export function applyCapabilityState(toolId, state) {
  const cap = capabilities.get(toolId);
  if (!cap) {
    return;
  }
  if (state === "granted" || state === "denied" || state === "pending" || state === "none") {
    cap.state = state;
  }
}

/** Heuristic: user text that should demo a Library grant card. */
export function wantsLibraryTool(text) {
  const t = String(text || "").toLowerCase();
  return (
    t.includes("download") ||
    t.includes("library") ||
    t.includes("folder") ||
    t.includes("files in") ||
    t.includes("desktop") ||
    t.includes("what's on my") ||
    t.includes("whats on my")
  );
}

export function wantsWalletTool(text) {
  const t = String(text || "").toLowerCase();
  return t.includes("wallet") || t.includes("sign") || t.includes("recovery");
}

/** Heuristic: user asks for open-web search (Exit/net — fail-closed until granted). */
export function wantsWebSearchTool(text) {
  const t = String(text || "").toLowerCase();
  return (
    t.includes("search the web") ||
    t.includes("web search") ||
    t.includes("google ") ||
    t.includes("look up online") ||
    t.includes("search online")
  );
}

function scopeFor(cap) {
  if (cap.tier >= 4) {
    return "human ceremony only · never ambient";
  }
  return "read-only · this session · revocable";
}

function summaryFor(toolId, args) {
  if (toolId === "library.read") {
    const path = args.path || "Downloads";
    return `Agent wants to list files in ${path}`;
  }
  if (toolId === "wallet.sign") {
    return "Agent wants Wallet signing power";
  }
  return `Agent wants ${toolId}`;
}

function mockToolResult(toolId, args) {
  if (toolId === "library.read") {
    const path = args.path || "Downloads";
    return (
      `Preview listing for ${path} (mock — not a real Library call):\n` +
      `· weekend-plan.md\n· photo-dump.zip\n· notes.txt`
    );
  }
  return "Preview result (mock).";
}

export function providerTip() {
  return TIP;
}

/** Thinking visibility preference (fx7). Default on. Host snapshot persists. */
export function loadReasoningVisible() {
  return reasoningVisibleStore !== false;
}

export function setReasoningVisible(visible) {
  reasoningVisibleStore = Boolean(visible);
  return reasoningVisibleStore;
}

/**
 * W1-ready: normalize OpenAI-compat reasoning fields + think tags
 * (Local Studio / pi-ai pattern — clean-room).
 */
export function firstReasoningField(record = {}) {
  for (const field of ["reasoning_content", "reasoning", "reasoning_text"]) {
    const value = record[field];
    if (typeof value === "string" && value.length > 0) {
      return value;
    }
  }
  return "";
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

/** Mock turn payload: thinking → optional tool → answer + follow-ups (fx7/fx11). */
export function getMockTurn(userText = "") {
  const wantsFiles = wantsLibraryTool(userText);
  const wantsWallet = wantsWalletTool(userText);
  const thinking = wantsFiles
    ? `${MOCK_THINKING}\nThey mentioned files — prepare a Library · Read grant after the answer.`
    : wantsWallet
      ? `${MOCK_THINKING}\nWallet came up — deny signing; human ceremony only.`
      : MOCK_THINKING;
  /** @type {{ id: string, label: string, kind: string } | null} */
  let toolPreview = null;
  if (wantsFiles) {
    toolPreview = {
      id: "library.read",
      label: "Library · Read",
      kind: "read",
      detail: "Downloads",
    };
  } else if (wantsWallet) {
    toolPreview = {
      id: "wallet.sign",
      label: "Wallet · Sign",
      kind: "deny",
      detail: "ceremony required",
    };
  }
  const followUps = wantsFiles
    ? [
        "Show me what grant looks like",
        "Stay read-only — just explain",
        "Switch to Build and draft a plan",
      ]
    : [
        "What tools can I grant later?",
        "Explain On this device",
        "Help me plan a small capsule",
      ];
  return {
    thinking,
    answer: MOCK_REPLY,
    toolPreview,
    followUps,
    preview: true,
  };
}

export function getPlanMarkdown() {
  return planMarkdown;
}

export function setPlanMarkdown(markdown) {
  planMarkdown = String(markdown || "");
  return planMarkdown;
}

/** Mock agent refreshes plan when user asks to plan/build. @returns {boolean} whether plan changed */
export function maybeUpdatePlanFromPrompt(userText) {
  const t = String(userText || "").toLowerCase();
  if (!t.includes("plan") && !t.includes("build") && !t.includes("capsule")) {
    return false;
  }
  planMarkdown = `### To-dos
- [x] Heard the request
- [/] Draft the approach (preview)
- [ ] List required capabilities
- [ ] ADE sandbox write (later)
- [ ] Local install (later)`;
  return true;
}

/* ---- Configure / Usage / Projects seams (preview literacy) -------------
   Machine probe ≠ agent ambient authority. Download Get ≠ silent network.
   Projects use ElastOS nouns (title + optional rootRef), never host cwd. */

export function getMachineProfile() {
  const hw = getHardwareEstimate();
  return {
    label: hw?.deviceLabel?.replace(/^This device ·\s*/i, "") || "This device",
    platform: hw?.gpuLabel?.includes("Apple") ? "Mac" : "This device",
    cores: hw?.cores ?? null,
    memoryGb: hw?.poolGb ?? null,
    memLabel: hw?.memLabel ?? "memory unknown",
    gpuLine: hw?.gpuLabel || "GPU unknown",
    isThisMachine: true,
    source: hw?.source ?? "browser-estimate",
    readyForFit: Boolean(hw),
    deviceLabel: hw?.deviceLabel ?? "This device",
  };
}

export function getRuntimeSnapshot() {
  const truth = getTruthSnapshot();
  return {
    locality: truth.locality,
    preview: true,
    backend: null,
    process: "none",
    modelLabel: truth.modelLabel,
    contextLabel: truth.contextLabel,
    toolsLabel: truth.toolsLabel,
    hwLabel: truth.hwLabel,
    note: "No model process loaded yet. Live health when llama-provider is wired.",
  };
}

export function getConfigureOverviewSnapshot() {
  const machine = getMachineProfile();
  const runtime = getRuntimeSnapshot();
  const model = getSelectedModel();
  return [
    {
      id: "machine",
      title: "Machine",
      status: machine.readyForFit ? "Ready" : "Estimate",
      statusTitle: "Profile available — not an agent grant",
      detail: `${machine.platform} · ${machine.memLabel} est`,
      section: "machine",
    },
    {
      id: "models",
      title: "Models",
      status: model?.label ?? "None",
      statusTitle: "Selected model for this room",
      detail: "Mine · Picks · Get",
      section: "models",
    },
    {
      id: "prompt",
      title: "Prompt",
      status: "Editable",
      statusTitle: "System prompt + sampling prefs",
      detail: "System prompt · temperature · max tokens",
      section: "prompt",
    },
    {
      id: "tools",
      title: "Tools",
      status: String(toolsSummaryLabel() || "").replace(/^Tools:\s*/i, "") || "none",
      statusTitle: "Capability state — grants only",
      detail: "Library · Wallet — nothing ambient",
      section: "tools",
    },
    {
      id: "runtime",
      title: "Runtime",
      status: "Preview",
      statusTitle: "Preview literacy — not live controller health",
      detail: `${runtime.locality} · ${runtime.process}`,
      section: "runtime",
    },
  ];
}

export function listPicksByTier() {
  const order = ["nano", "mini", "medium"];
  return order.map((hwTier) => ({
    hwTier,
    blurb: HW_TIER_BLURB[hwTier] || hwTier,
    models: listModels()
      .filter((m) => m.hwTier === hwTier && m.tier !== "preview")
      .map((m) => ({ ...m, fit: fitForModel(m) })),
  }));
}

/** Preview Get — never silent network. Later: real Inbox grant for fetch+store. */
export function requestModelGet(modelId) {
  const match = MODEL_CATALOG.find((m) => m.id === modelId);
  if (!match) {
    return { kind: "missing", modelId };
  }
  if (match.status === "installed") {
    return { kind: "already", modelId };
  }
  if (match.status === "blocked" || fitForModel(match) === "blocked") {
    return { kind: "blocked", modelId, reason: "Needs more memory or Spark-class hardware" };
  }
  return { kind: "preview-theatre", modelId, label: match.label };
}

/** ~53 weeks of UTC days — all zeros until live inference accounting (Studio-style grid). */
function emptyUsageDaily() {
  const days = [];
  const end = new Date();
  end.setUTCHours(0, 0, 0, 0);
  const start = new Date(end);
  start.setUTCDate(start.getUTCDate() - 53 * 7 + 1);
  for (let t = start.getTime(); t <= end.getTime(); t += 86400000) {
    const d = new Date(t);
    days.push({
      date: d.toISOString().slice(0, 10),
      total_tokens: 0,
      requests: 0,
    });
  }
  return days;
}

/** Wave 4 — on-Home usage ledger (host-persisted via session.agent). */
const MAX_USAGE_TURNS = 200;
let usageTurns = [];
let lastStreamFailure = "";

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
    source: source === "mock" ? "mock" : omitted ? "estimated" : "live",
    omitted,
  });
  if (usageTurns.length > MAX_USAGE_TURNS) {
    usageTurns = usageTurns.slice(-MAX_USAGE_TURNS);
  }
  lastStreamFailure = "";
  return usageTurns[usageTurns.length - 1];
}

export function noteStreamFailure(reason) {
  lastStreamFailure = String(reason || "").slice(0, 240);
}

export function getLastStreamFailure() {
  return lastStreamFailure;
}

export function clearLastStreamFailure() {
  lastStreamFailure = "";
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
      source: t.source === "mock" || t.source === "estimated" ? t.source : "live",
      omitted: Boolean(t.omitted),
    }))
    .slice(-MAX_USAGE_TURNS);
}

export function getUsageSnapshot() {
  const liveTurns = usageTurns.filter((t) => t.source !== "mock");
  const hasLive = liveTurns.length > 0;
  const turns = hasLive ? liveTurns : usageTurns;
  const tokens = turns.reduce((sum, t) => sum + (t.tokens || 0), 0);
  const requests = turns.length;
  const days = new Set(turns.map((t) => t.day).filter(Boolean));
  const byModelMap = new Map();
  for (const t of turns) {
    byModelMap.set(t.model, (byModelMap.get(t.model) || 0) + (t.tokens || 0));
  }
  const byModel = [...byModelMap.entries()]
    .sort((a, b) => b[1] - a[1])
    .slice(0, 6)
    .map(([name, count]) => `${name} (${count})`);
  const daily = emptyUsageDaily();
  const byDate = new Map(daily.map((d) => [d.date, d]));
  for (const t of turns) {
    const row = byDate.get(t.day);
    if (!row) {
      continue;
    }
    row.total_tokens += t.tokens || 0;
    row.requests += 1;
  }
  const last = turns[turns.length - 1];
  const omittedCount = turns.filter((t) => t.omitted).length;
  let note = "No Live turns yet — Usage stays empty until a Live reply lands";
  if (hasLive) {
    note = omittedCount
      ? `Live metering · ${omittedCount} turn(s) estimated (upstream omitted usage)`
      : "Live metering from gateway stream / estimates";
  }
  return {
    preview: !hasLive,
    tokens,
    requests,
    sessions: days.size,
    activeDays: days.size,
    byModel,
    daily,
    locality: "On this Home",
    note,
    lastLatencyMs: last?.latencyMs || 0,
    lastSource: last?.source || "",
    lastStreamFailure,
  };
}

/**
 * In-memory project list. Durable store is host session.agent via harness snapshot
 * (opaque sandbox has no localStorage — Principle 10: one canonical path).
 */
let projectStore = [];

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
  const project = {
    id: `proj-${Date.now()}-${Math.random().toString(36).slice(2, 7)}`,
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
