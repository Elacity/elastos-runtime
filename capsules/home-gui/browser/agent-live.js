/* Live local inference bridge (w1). Chat only — carries no tool, grant, or
   capsule authority; grant cards remain fail-closed preview (Principle 16).
   Path: gateway /api/provider/ai/* and /api/provider/llama/* (home-gui-only
   allowlist, home launch token). §AL trust note: re-validate this route
   against runtime launch-token/Bus semantics at w0. Tip: home-20260728ag */

import { fetchJson } from "./shell-core.js?v=home-20260728ag";

/** Re-probe at most this often unless forced (online event, harness open). */
const PROBE_TTL_MS = 15000;
/** Bound the history we send — local models have small contexts. */
const LIVE_HISTORY_LIMIT = 12;

const LIVE_SYSTEM_PROMPT =
  "You are the ElastOS Home Agent, running privately on this machine. " +
  "You have no tools and no capsule authority in this session; answer from " +
  "knowledge only, and say so plainly when a task would need a tool or grant.";

let liveState = {
  live: false,
  checking: false,
  model: "",
  endpointState: "",
  reason: "unprobed",
  checkedAt: 0,
  /** Real llama-server models (one configured GGUF today). */
  models: [],
};

let probePromise = null;

export function getLiveInferenceState() {
  return { ...liveState, models: liveState.models.slice() };
}

async function providerCall(scheme, op, payload = {}) {
  const response = await fetchJson(`/api/provider/${scheme}/${op}`, {
    method: "POST",
    body: JSON.stringify(payload),
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

/**
 * Truth probe: live only when ai-provider lists a `local` backend AND
 * llama-server reports healthy. Conservative on any failure — the honest
 * default is preview, never an assumed live claim (§AL.3).
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
      /* Conservative: live requires a declared `local` backend — never assumed. */
      const hasLocal = names.includes("local");
      const health = await providerCall("llama", "health");
      const healthy = hasLocal && health?.healthy === true;
      let model = "";
      let models = [];
      if (healthy) {
        const [status, listing] = await Promise.all([
          providerCall("llama", "status").catch(() => null),
          providerCall("llama", "list_models").catch(() => null),
        ]);
        model = String(status?.model || "").split("/").pop() || "";
        models = liveModelsFromListing(listing);
        if (!model && models[0]) {
          model = models[0].label;
        }
      }
      liveState = {
        live: healthy,
        checking: false,
        model,
        endpointState: String(health?.state || ""),
        reason: healthy ? "ready" : String(health?.reason || health?.state || "not-ready"),
        checkedAt: Date.now(),
        models,
      };
    } catch (error) {
      liveState = {
        live: false,
        checking: false,
        model: "",
        endpointState: "unreachable",
        reason: String(error?.code || "unreachable"),
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
export function buildLiveMessages(sessionMessages = []) {
  const turns = sessionMessages
    .filter((m) => (m.role === "user" || m.role === "agent") && String(m.text || "").trim())
    .slice(-LIVE_HISTORY_LIMIT)
    .map((m) => ({
      role: m.role === "agent" ? "assistant" : "user",
      content: String(m.text),
    }));
  return [{ role: "system", content: LIVE_SYSTEM_PROMPT }, ...turns];
}

/**
 * One live turn — full completion (no token stream; providers are one-shot,
 * see §AL.2). Throws with `code` on provider errors (`local_unavailable`,
 * `model_loading`, `timeout`, …) so callers can fall back to mock honestly.
 */
export async function requestLiveChatCompletion(messages) {
  const data = await providerCall("ai", "chat_completions", {
    backend: "local",
    messages,
  });
  const message = data?.choices?.[0]?.message || {};
  return {
    message,
    usage: data?.usage || null,
  };
}
