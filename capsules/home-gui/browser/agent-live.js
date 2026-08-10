/* Live inference bridge — contract era. Chat runs go through the
   model-provider (elastos://model/*, runs_create + runs_events cursor-poll);
   readiness = offers_list (an offer is advertised only when its backend is
   configured — never advertise what we can't serve). Chat carries no tool,
   grant, or capsule authority (Principle 16).
   Tip: home-20260810mr */

import { fetchJson, getHomeGuiLaunchToken } from "./shell-core.js?v=home-20260810mr";

/** Re-probe at most this often unless forced (online event, harness open). */
const PROBE_TTL_MS = 15000;
/** Bound the history we send — local models have small contexts. */
const LIVE_HISTORY_LIMIT = 12;

/** PRINCIPLES-safe default — no tools, no capsule authority (UI ≠ authority). */
export const DEFAULT_LIVE_SYSTEM_PROMPT =
  "You are the ElastOS Home Agent, running privately on this machine. " +
  "You have no tools and no capsule authority in this session; answer from " +
  "knowledge only, and say so plainly when a task would need a tool or grant.";

export const DEFAULT_LIVE_MAX_TOKENS = 2048;
export const DEFAULT_LIVE_TEMPERATURE = 0.7;
export const MAX_LIVE_SYSTEM_PROMPT_CHARS = 8_000;
export const MAX_AGENT_NOTES_CHARS = 4_000;

export function normalizeAgentNotes(value) {
  const text = String(value ?? "").trim();
  if (!text) {
    return "";
  }
  return text.length > MAX_AGENT_NOTES_CHARS
    ? text.slice(0, MAX_AGENT_NOTES_CHARS)
    : text;
}

export function clampLiveMaxTokens(value) {
  const n = Number(value);
  if (!Number.isFinite(n)) {
    return DEFAULT_LIVE_MAX_TOKENS;
  }
  return Math.min(8192, Math.max(16, Math.round(n)));
}

export function clampLiveTemperature(value) {
  const n = Number(value);
  if (!Number.isFinite(n)) {
    return DEFAULT_LIVE_TEMPERATURE;
  }
  return Math.min(2, Math.max(0, Math.round(n * 100) / 100));
}

export function normalizeLiveSystemPrompt(value) {
  const text = String(value ?? "").trim();
  if (!text) {
    return DEFAULT_LIVE_SYSTEM_PROMPT;
  }
  return text.length > MAX_LIVE_SYSTEM_PROMPT_CHARS
    ? text.slice(0, MAX_LIVE_SYSTEM_PROMPT_CHARS)
    : text;
}

let liveState = {
  live: false,
  checking: false,
  model: "",
  endpointState: "",
  reason: "unprobed",
  checkedAt: 0,
  /** Live model rows (from advertised model offers). */
  models: [],
};

let probePromise = null;

export function getLiveInferenceState() {
  return { ...liveState, models: liveState.models.slice() };
}

/** Chat offers → model-menu rows. An offer exists only when its backend is
 * configured (readiness-honest provider), so listing is the truth probe. */
function chatOfferRows(offers) {
  const list = Array.isArray(offers?.offers) ? offers.offers : [];
  return list
    .filter((o) => {
      const ops = Array.isArray(o?.descriptor?.operations) ? o.descriptor.operations : [];
      return ops.some(
        (op) =>
          (op?.inputs || []).some((i) => (i?.modalities || []).includes("text")) &&
          (op?.outputs || []).some((out) => (out?.modalities || []).includes("text")),
      );
    })
    .map((o) => {
      const modelId = String(o?.descriptor?.model?.id || o?.offer_id || "model");
      return {
        id: `live:${modelId}`,
        label: modelId,
        detail: "Model offer · this Home",
      };
    });
}

/** Cached offers_list — model menu + Configure panel + probe share it. */
let offersCache = null;
let offersPromise = null;

export function getModelOffersCache() {
  return offersCache;
}

export async function fetchModelOffers({ force = false } = {}) {
  if (!force && offersCache) {
    return offersCache;
  }
  if (!force && offersPromise) {
    return offersPromise;
  }
  offersPromise = (async () => {
    const data = await modelRunCall("offers_list");
    offersCache = data;
    return data;
  })()
    .catch((error) => {
      console.warn("model offers fetch failed", error);
      return offersCache;
    })
    .finally(() => {
      offersPromise = null;
    });
  return offersPromise;
}

/**
 * Truth probe: live when the model-provider answers ping AND advertises at
 * least one text→text offer. Conservative on failure — preview is the honest
 * default (§AL.3).
 */
export async function probeLiveInference({ force = false } = {}) {
  const now = Date.now();
  if (!force && (liveState.checking || now - liveState.checkedAt < PROBE_TTL_MS)) {
    return probePromise ? probePromise.then(getLiveInferenceState) : getLiveInferenceState();
  }
  liveState.checking = true;
  probePromise = (async () => {
    try {
      await modelRunCall("ping");
      const offers = await fetchModelOffers({ force: true });
      const models = chatOfferRows(offers);
      if (!models.length) {
        liveState = {
          live: false,
          checking: false,
          model: "",
          endpointState: "no-model-offers",
          reason: "no-model-offers",
          checkedAt: Date.now(),
          models: [],
        };
        return;
      }
      liveState = {
        live: true,
        checking: false,
        model: models[0].label,
        endpointState: "model-offers",
        reason: "ready",
        checkedAt: Date.now(),
        models,
      };
    } catch (error) {
      liveState = {
        live: false,
        checking: false,
        model: "",
        endpointState: "unreachable",
        reason: String(error?.code || error?.message || "unreachable"),
        checkedAt: Date.now(),
        models: [],
      };
    } finally {
      probePromise = null;
    }
  })();
  await probePromise;
  return getLiveInferenceState();
}

/** Session history → OpenAI-compat messages (user/assistant only, bounded). */
export function buildLiveMessages(sessionMessages = [], { systemPrompt, notes } = {}) {
  const history = Array.isArray(sessionMessages) ? sessionMessages : [];
  const turns = history
    .filter((m) => (m.role === "user" || m.role === "agent") && String(m.text || "").trim())
    .slice(-LIVE_HISTORY_LIMIT)
    .map((m) => ({
      role: m.role === "agent" ? "assistant" : "user",
      content: String(m.text),
    }));
  let system = normalizeLiveSystemPrompt(systemPrompt);
  const noteText = normalizeAgentNotes(notes);
  if (noteText) {
    system = `${system}\n\nNotes on this Home (sticky · host-persisted):\n${noteText}`;
  }
  return [{ role: "system", content: system }, ...turns];
}

function libraryReadUrl(requestId) {
  const base = "/api/apps/home/agent/tools/library.read";
  if (requestId) {
    return new URL(`${base}/${encodeURIComponent(requestId)}`, window.location.href).href;
  }
  return new URL(base, window.location.href).href;
}

function homeAgentHeaders(extra = {}) {
  const token = getHomeGuiLaunchToken();
  if (!token) {
    const error = new Error("missing home launch token in Home GUI shell");
    error.code = "missing-home-launch-token";
    throw error;
  }
  return {
    accept: "application/json",
    "x-elastos-home-token": token,
    ...extra,
  };
}

/** Wave 5.01 — create Inbox-backed library.read capability request. */
export async function requestAgentLibraryRead({ uri } = {}) {
  const response = await fetch(libraryReadUrl(), {
    method: "POST",
    headers: homeAgentHeaders({ "content-type": "application/json" }),
    body: JSON.stringify(uri ? { uri } : {}),
  });
  const data = await response.json().catch(() => ({}));
  if (!response.ok) {
    const error = new Error(data?.message || `library.read request failed: ${response.status}`);
    error.code = data?.code || "library_read_request_failed";
    error.status = response.status;
    throw error;
  }
  return data;
}

export async function fetchAgentLibraryReadStatus(requestId) {
  const id = String(requestId || "").trim();
  if (!id) {
    return null;
  }
  const response = await fetch(libraryReadUrl(id), {
    headers: homeAgentHeaders(),
  });
  const data = await response.json().catch(() => ({}));
  if (!response.ok) {
    const error = new Error(data?.message || `library.read status failed: ${response.status}`);
    error.code = data?.code || "library_read_status_failed";
    error.status = response.status;
    throw error;
  }
  return data;
}

export async function cancelAgentLibraryRead(requestId) {
  const id = String(requestId || "").trim();
  if (!id) {
    return null;
  }
  const response = await fetch(`${libraryReadUrl(id)}/cancel`, {
    method: "POST",
    headers: homeAgentHeaders({ "content-type": "application/json" }),
    body: "{}",
  });
  const data = await response.json().catch(() => ({}));
  if (!response.ok) {
    const error = new Error(data?.message || `library.read cancel failed: ${response.status}`);
    error.code = data?.code || "library_read_cancel_failed";
    error.status = response.status;
    throw error;
  }
  return data;
}

/** Wave 6.02 — ask gateway for web.search; fail-closed until Exit/net exists. */
export async function requestAgentWebSearch(query = "") {
  const token = getHomeGuiLaunchToken();
  if (!token) {
    const error = new Error("missing home launch token in Home GUI shell");
    error.code = "missing-home-launch-token";
    throw error;
  }
  const response = await fetch(
    new URL("/api/apps/home/agent/tools/web.search", window.location.href).href,
    {
      method: "POST",
      headers: {
        accept: "application/json",
        "content-type": "application/json",
        "x-elastos-home-token": token,
      },
      body: JSON.stringify({ query: String(query || "").slice(0, 500) }),
    },
  );
  const data = await response.json().catch(() => ({}));
  if (!response.ok) {
    const error = new Error(data?.message || `web.search failed: ${response.status}`);
    error.code = data?.code || "web_search_failed";
    error.status = response.status;
    throw error;
  }
  return data;
}

/** Wave 6.01 — extract Desktop text after Inbox library.read is ready. */
export async function extractAgentLibraryRead(requestId, uri) {
  const id = String(requestId || "").trim();
  const target = String(uri || "").trim();
  if (!id || !target) {
    const error = new Error("library.read extract requires request_id and uri");
    error.code = "library_read_extract_args";
    throw error;
  }
  const response = await fetch(`${libraryReadUrl(id)}/extract`, {
    method: "POST",
    headers: homeAgentHeaders({ "content-type": "application/json" }),
    body: JSON.stringify({ uri: target }),
  });
  const data = await response.json().catch(() => ({}));
  if (!response.ok) {
    const error = new Error(data?.message || `library.read extract failed: ${response.status}`);
    error.code = data?.code || "library_read_extract_failed";
    error.status = response.status;
    throw error;
  }
  return data;
}

export function abortLiveChatStream() {
  if (liveContractRunId) {
    const runId = liveContractRunId;
    liveContractRunId = null;
    /* fire-and-forget: best-effort cancel of the contract run */
    modelRunCall("runs_cancel", { run_id: runId }).catch(() => {});
  }
}

/* ---- Chat via the model contract (runs.*) ---- */

let liveContractRunId = null;

export async function modelRunCall(op, body = {}) {
  const token = getHomeGuiLaunchToken();
  if (!token) {
    const error = new Error("missing home launch token in Home GUI shell");
    error.code = "missing-home-launch-token";
    throw error;
  }
  const res = await fetch(
    new URL(`/api/provider/model/${op}`, window.location.href).href,
    {
      method: "POST",
      headers: {
        "content-type": "application/json",
        "x-elastos-home-token": token,
      },
      body: JSON.stringify(body),
    },
  );
  const data = await res.json().catch(() => ({}));
  if (!res.ok || data?.status === "error") {
    const error = new Error(data?.message || data?.code || `model ${op} failed (${res.status})`);
    error.code = data?.code || "model_error";
    error.status = res.status;
    throw error;
  }
  return data?.data ?? data;
}

const CONTRACT_POLL_MS = 250;

/**
 * Stream a chat turn via runs_create + runs_events cursor-poll on
 * offer:flash-chat:pair-a. onDelta({ reasoning, content, done }) is called as
 * events arrive; Stop = abortLiveChatStream() (fires runs_cancel).
 */
export async function streamChatViaContract(
  messages,
  { onDelta, maxTokens = DEFAULT_LIVE_MAX_TOKENS, temperature = DEFAULT_LIVE_TEMPERATURE } = {},
) {
  abortLiveChatStream();
  const startedAt = Date.now();

  const created = await modelRunCall("runs_create", {
    offer_id: "offer:flash-chat:pair-a",
    operation: "generate",
    inputs: {
      messages,
      max_tokens: clampLiveMaxTokens(maxTokens),
      temperature: clampLiveTemperature(temperature),
    },
  });
  const runId = String(created?.run_id || "");
  if (!runId) {
    const error = new Error("contract returned no run id");
    error.code = "no_run_id";
    throw error;
  }
  liveContractRunId = runId;

  let cursor = 0;
  let reasoning = "";
  let content = "";
  const emit = (done = false) => onDelta?.({ reasoning, content, done });
  const finish = (extra = {}) => ({
    reasoning,
    content,
    usage: null,
    latencyMs: Date.now() - startedAt,
    aborted: false,
    ...extra,
  });

  try {
    for (;;) {
      const data = await modelRunCall("runs_events", { run_id: runId, cursor });
      cursor = data?.cursor ?? cursor;
      const state = String(data?.state || "");
      const events = Array.isArray(data?.events) ? data.events : [];
      for (const ev of events) {
        if (ev?.type === "thinking" && typeof ev.delta === "string") {
          reasoning += ev.delta;
        } else if (ev?.type === "text" && typeof ev.delta === "string") {
          content += ev.delta;
        }
      }
      emit(state === "succeeded" || state === "failed" || state === "cancelled");
      if (state === "succeeded") {
        return finish();
      }
      if (state === "failed") {
        const errEvent = events.find((e) => e?.type === "error");
        const error = new Error(errEvent?.message || "run failed");
        error.code = errEvent?.code || "run_failed";
        throw error;
      }
      if (state === "cancelled") {
        return finish({ aborted: true });
      }
      await new Promise((resolve) => setTimeout(resolve, CONTRACT_POLL_MS));
    }
  } finally {
    if (liveContractRunId === runId) {
      liveContractRunId = null;
    }
  }
}
