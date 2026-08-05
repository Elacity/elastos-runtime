/* Live local inference bridge (w1 + Sparks OpenAI-compat). Chat only — carries
   no tool, grant, or capsule authority; grant cards remain fail-closed preview
   (Principle 16). Path: gateway /api/provider/ai/* and /api/provider/llama/*
   (home-gui-only allowlist, home launch token).
   Tip: home-20260804ay */

import { fetchJson, getHomeGuiLaunchToken } from "./shell-core.js?v=home-20260804ay";

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
  /** Live model rows (llama GGUF and/or remote OpenAI-compat). */
  models: [],
};

let probePromise = null;

export function getLiveInferenceState() {
  return { ...liveState, models: liveState.models.slice() };
}

function providerUrl(scheme, op) {
  /* Opaque sandboxed home-gui resolves relative /api against the document URL
     (localhost gateway). Keep absolute for clarity. */
  return new URL(`/api/provider/${scheme}/${op}`, window.location.href).href;
}

async function providerCall(scheme, op, payload = {}) {
  const token = getHomeGuiLaunchToken();
  if (!token) {
    const error = new Error("missing home launch token in Home GUI shell");
    error.code = "missing-home-launch-token";
    throw error;
  }
  const response = await fetchJson(providerUrl(scheme, op), {
    method: "POST",
    body: JSON.stringify(payload),
    headers: {
      "x-elastos-home-token": token,
    },
  });
  if (!response || response.status !== "ok") {
    const error = new Error(response?.message || `${scheme}.${op} failed`);
    error.code = response?.code || "provider_error";
    throw error;
  }
  return response.data ?? {};
}

function liveModelsFromListing(listing) {
  const rows = Array.isArray(listing?.models) ? listing.models : [];
  return rows
    .map((row) => {
      const file = String(row?.filename || row?.path || "").split("/").pop() || "";
      const label = file.replace(/\.gguf$/i, "");
      if (!label) {
        return null;
      }
      return {
        id: `live:${label}`,
        label,
        detail: [
          row?.n_ctx ? `${row.n_ctx} ctx` : "",
          "llama-server on this machine",
        ]
          .filter(Boolean)
          .join(" · "),
      };
    })
    .filter(Boolean);
}

function localBackendRow(backends) {
  const rows = Array.isArray(backends?.backends) ? backends.backends : [];
  return (
    rows.find((b) => {
      const name = typeof b === "string" ? b : b?.name || b?.id || "";
      return name === "local";
    }) || null
  );
}

/** True when ai-provider local backend points at remote OpenAI-compat (Sparks). */
function isRemoteOpenAiCompat(local) {
  if (!local || typeof local !== "object") {
    return false;
  }
  const apiUrl = String(local.api_url || "").toLowerCase();
  if (!apiUrl.startsWith("http://") && !apiUrl.startsWith("https://")) {
    return false;
  }
  /* On-box llama / ollama defaults — not Sparks. */
  if (
    apiUrl.includes("127.0.0.1") ||
    apiUrl.includes("localhost") ||
    apiUrl.includes("::1") ||
    apiUrl.includes("0.0.0.0")
  ) {
    return false;
  }
  return true;
}

function remoteLiveFromLocal(local) {
  const remoteModel = String(local?.default_model || "").trim() || "remote";
  const apiUrl = String(local?.api_url || "");
  return {
    model: remoteModel,
    models: [
      {
        id: `live:${remoteModel}`,
        label: remoteModel,
        detail: apiUrl
          ? `OpenAI-compat · ${apiUrl}`
          : "OpenAI-compat (OLLAMA_URL)",
      },
    ],
    endpointState: "openai-compat",
    reason: "ready",
  };
}

/**
 * Truth probe: live when ai-provider lists a `local` backend AND either
 * (a) that backend is remote OpenAI-compat (Sparks via OLLAMA_URL) and pings, or
 * (b) llama-server is healthy. Conservative on total failure — preview is the
 * honest default (§AL.3).
 */
export async function probeLiveInference({ force = false } = {}) {
  const now = Date.now();
  if (!force && (liveState.checking || now - liveState.checkedAt < PROBE_TTL_MS)) {
    return probePromise ? probePromise.then(getLiveInferenceState) : getLiveInferenceState();
  }
  liveState.checking = true;
  probePromise = (async () => {
    try {
      const backends = await providerCall("ai", "list_backends");
      const names = (Array.isArray(backends?.backends) ? backends.backends : [])
        .map((b) => (typeof b === "string" ? b : b?.name || b?.id || ""))
        .filter(Boolean);
      const hasLocal = names.includes("local");
      if (!hasLocal) {
        liveState = {
          live: false,
          checking: false,
          model: "",
          endpointState: "no-local-backend",
          reason: "no-local-backend",
          checkedAt: Date.now(),
          models: [],
        };
        return;
      }

      const local = localBackendRow(backends);

      /* Path B first when OLLAMA_URL points at Sparks — do not require llama. */
      if (isRemoteOpenAiCompat(local)) {
        await providerCall("ai", "ping");
        const remote = remoteLiveFromLocal(local);
        liveState = {
          live: true,
          checking: false,
          model: remote.model,
          endpointState: remote.endpointState,
          reason: remote.reason,
          checkedAt: Date.now(),
          models: remote.models,
        };
        return;
      }

      let healthy = false;
      let model = "";
      let models = [];
      let endpointState = "";
      let reason = "not-ready";

      /* Path A: on-machine llama-server (existing w1). */
      try {
        const health = await providerCall("llama", "health");
        if (health?.healthy === true) {
          const [status, listing] = await Promise.all([
            providerCall("llama", "status").catch(() => null),
            providerCall("llama", "list_models").catch(() => null),
          ]);
          model = String(status?.model || "").split("/").pop() || "";
          models = liveModelsFromListing(listing);
          if (!model && models[0]) {
            model = models[0].label;
          }
          healthy = true;
          endpointState = String(health?.state || "llama-ready");
          reason = "ready";
        } else {
          endpointState = String(health?.state || "llama-not-ready");
          reason = String(health?.reason || health?.state || "llama-not-ready");
        }
      } catch (error) {
        endpointState = "llama-absent";
        reason = String(error?.code || "llama-absent");
      }

      /* Fallback: any listed local backend that answers ping. */
      if (!healthy) {
        await providerCall("ai", "ping");
        const remote = remoteLiveFromLocal(local);
        model = remote.model;
        models = remote.models;
        endpointState = remote.endpointState;
        reason = remote.reason;
        healthy = true;
      }

      liveState = {
        live: healthy,
        checking: false,
        model,
        endpointState,
        reason,
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

/**
 * Assistant text from an OpenAI-compat message. Flash/vLLM may return
 * content:null with the only text in reasoning / reasoning_content.
 */
export function assistantTextFromMessage(message = {}) {
  const content = String(message?.content ?? "").trim();
  if (content) {
    return content;
  }
  for (const field of ["reasoning_content", "reasoning", "reasoning_text"]) {
    const value = message?.[field];
    if (typeof value === "string" && value.trim()) {
      return value.trim();
    }
  }
  return "";
}

/**
 * One live turn — full completion (legacy / probe fallback). Prefer
 * streamLiveChatCompletion for Agent turns.
 */
export async function requestLiveChatCompletion(messages) {
  const data = await providerCall("ai", "chat_completions", {
    backend: "local",
    messages,
    max_tokens: 2048,
  });
  const message = data?.choices?.[0]?.message || {};
  return {
    message,
    usage: data?.usage || null,
  };
}

/** Sparks pair for dogfood streaming — server maps to allowlisted upstream. */
let livePair = "a";
let liveAbort = null;
/** Cached GET /api/apps/home/agent/backends — UI literacy only. */
let backendsCache = null;
let backendsPromise = null;

export function getLiveChatPair() {
  return livePair === "b" ? "b" : "a";
}

export function setLiveChatPair(pair) {
  livePair = pair === "b" ? "b" : "a";
  try {
    sessionStorage.setItem("elastos.home-gui.agent-pair", livePair);
  } catch {
    /* opaque / blocked */
  }
  return livePair;
}

try {
  const stored = sessionStorage.getItem("elastos.home-gui.agent-pair");
  if (stored === "a" || stored === "b") {
    livePair = stored;
  }
} catch {
  /* ignore */
}

function backendsUrl() {
  return new URL("/api/apps/home/agent/backends", window.location.href).href;
}

export function getAgentBackendsCache() {
  return backendsCache;
}

export async function fetchAgentBackends({ force = false } = {}) {
  if (!force && backendsCache) {
    return backendsCache;
  }
  if (!force && backendsPromise) {
    return backendsPromise;
  }
  const token = getHomeGuiLaunchToken();
  if (!token) {
    return backendsCache;
  }
  backendsPromise = (async () => {
    const response = await fetch(backendsUrl(), {
      headers: {
        accept: "application/json",
        "x-elastos-home-token": token,
      },
    });
    if (!response.ok) {
      throw new Error(`backends failed: ${response.status}`);
    }
    const data = await response.json();
    backendsCache = data;
    return data;
  })()
    .catch((error) => {
      console.warn("home agent backends fetch failed", error);
      return backendsCache;
    })
    .finally(() => {
      backendsPromise = null;
    });
  return backendsPromise;
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

export async function saveAgentBackends(patch = {}) {
  const token = getHomeGuiLaunchToken();
  if (!token) {
    const error = new Error("missing home launch token in Home GUI shell");
    error.code = "missing-home-launch-token";
    throw error;
  }
  const response = await fetch(backendsUrl(), {
    method: "PUT",
    headers: {
      "content-type": "application/json",
      accept: "application/json",
      "x-elastos-home-token": token,
    },
    body: JSON.stringify(patch),
  });
  const data = await response.json().catch(() => ({}));
  if (!response.ok) {
    const error = new Error(data?.message || `backends save failed: ${response.status}`);
    error.code = data?.code || "backends_save_failed";
    error.status = response.status;
    throw error;
  }
  backendsCache = data?.backends || data;
  return backendsCache;
}

export function abortLiveChatStream() {
  if (liveAbort) {
    try {
      liveAbort.abort();
    } catch {
      /* ignore */
    }
    liveAbort = null;
  }
}

function streamUrl() {
  return new URL("/api/apps/home/agent/chat/stream", window.location.href).href;
}

function deltaFields(delta = {}) {
  const content =
    typeof delta.content === "string"
      ? delta.content
      : delta.content == null
        ? ""
        : String(delta.content);
  let reasoning = "";
  for (const field of ["reasoning", "reasoning_content", "thinking"]) {
    if (typeof delta[field] === "string" && delta[field]) {
      reasoning = delta[field];
      break;
    }
  }
  return { content, reasoning };
}

/**
 * Stream a Live turn through the Home gateway SSE proxy (OpenAI-compat).
 * onDelta({ reasoning, content, done }) is called as tokens arrive.
 * Stop = abortLiveChatStream() / AbortController.
 */
export async function streamLiveChatCompletion(
  messages,
  {
    onDelta,
    maxTokens = DEFAULT_LIVE_MAX_TOKENS,
    temperature = DEFAULT_LIVE_TEMPERATURE,
  } = {},
) {
  const token = getHomeGuiLaunchToken();
  if (!token) {
    const error = new Error("missing home launch token in Home GUI shell");
    error.code = "missing-home-launch-token";
    throw error;
  }
  abortLiveChatStream();
  const controller = new AbortController();
  liveAbort = controller;

  let response;
  try {
    response = await fetch(streamUrl(), {
      method: "POST",
      headers: {
        "content-type": "application/json",
        accept: "text/event-stream",
        "x-elastos-home-token": token,
      },
      body: JSON.stringify({
        messages,
        max_tokens: clampLiveMaxTokens(maxTokens),
        temperature: clampLiveTemperature(temperature),
        pair: getLiveChatPair(),
      }),
      signal: controller.signal,
    });
  } catch (err) {
    if (controller.signal.aborted) {
      const error = new Error("stopped");
      error.code = "aborted";
      throw error;
    }
    throw err;
  }

  if (!response.ok) {
    const detail = await response.text().catch(() => "");
    let message = detail;
    try {
      const parsed = JSON.parse(detail);
      message = parsed?.message || detail;
    } catch {
      /* raw */
    }
    const error = new Error(
      message || `stream failed: ${response.status} ${response.statusText}`,
    );
    error.code = "upstream_http";
    error.status = response.status;
    throw error;
  }

  if (!response.body) {
    const error = new Error("stream body unavailable");
    error.code = "no_body";
    throw error;
  }

  const reader = response.body.getReader();
  const decoder = new TextDecoder();
  let buffer = "";
  let reasoning = "";
  let content = "";
  let sawDone = false;
  let usage = null;
  const startedAt = Date.now();

  const emit = (done = false) => {
    onDelta?.({ reasoning, content, done, usage });
  };

  const finish = (extra = {}) => {
    const latencyMs = Date.now() - startedAt;
    return {
      reasoning,
      content,
      usage,
      latencyMs,
      aborted: false,
      ...extra,
    };
  };

  try {
    while (true) {
      const { done, value } = await reader.read();
      if (done) {
        break;
      }
      buffer += decoder.decode(value, { stream: true });
      const parts = buffer.split("\n");
      buffer = parts.pop() || "";
      for (const line of parts) {
        const trimmed = line.trim();
        if (!trimmed || trimmed.startsWith(":")) {
          continue;
        }
        if (!trimmed.startsWith("data:")) {
          continue;
        }
        const data = trimmed.slice(5).trim();
        if (!data) {
          continue;
        }
        if (data === "[DONE]") {
          sawDone = true;
          emit(true);
          return finish();
        }
        let chunk;
        try {
          chunk = JSON.parse(data);
        } catch {
          continue;
        }
        if (chunk?.usage && typeof chunk.usage === "object") {
          usage = chunk.usage;
        }
        const delta = chunk?.choices?.[0]?.delta || {};
        const fields = deltaFields(delta);
        if (fields.reasoning) {
          reasoning += fields.reasoning;
        }
        if (fields.content) {
          content += fields.content;
        }
        /* Some servers put final text on message instead of delta. */
        const message = chunk?.choices?.[0]?.message;
        if (message && typeof message === "object") {
          const fromMsg = assistantTextFromMessage(message);
          if (fromMsg && !content) {
            content = fromMsg;
          }
        }
        emit(false);
      }
    }
    emit(true);
    return finish({ incomplete: !sawDone });
  } catch (err) {
    if (controller.signal.aborted) {
      emit(true);
      return {
        reasoning,
        content,
        usage,
        latencyMs: Date.now() - startedAt,
        aborted: true,
      };
    }
    throw err;
  } finally {
    if (liveAbort === controller) {
      liveAbort = null;
    }
  }
}
