/* Live inference bridge — contract era. Chat runs go through the
   model-provider (elastos://model/*, runs_create + runs_events cursor-poll);
   readiness = offers_list (an offer is advertised only when its backend is
   configured — never advertise what we can't serve). Chat carries no tool,
   grant, or capsule authority (Principle 16).
   Tip: home-20260814a */

import { getHomeGuiLaunchToken } from "./harness-host.js";
import { catalogModels, selectedModelOffer, readyModelChoices } from "./model-selection.js";
import { yieldToBrowser, YIELD_EVENT_SLICE, YIELD_MS } from "./agent-stream-qos.js";
import {
  eligibleTextOffers,
  textOfferRows,
  textRunCreateBody,
  applyRunEventsPage,
  terminalOutputText,
  contractError,
} from "./model-contract.js";
import {
  compileContext,
  attachProviderPayload,
  splitSessionMessages,
  makeBudget,
  logContextManifest,
  resolveReasoningPolicy,
  TEXT_CONTRACT_CAPABILITIES,
  EMERGENCY_HISTORY_MESSAGE_LIMIT,
  transcriptCharCount,
  assertProviderPayloadUnchanged,
  turnStorePut,
  turnStorePatch,
  TurnState,
  createTurnManifest,
  newTurnId,
} from "./agent-context.js";

/** Re-probe at most this often unless forced (online event, harness open). */
const PROBE_TTL_MS = 15000;

/** PRINCIPLES-safe default — no tools, no capsule authority (UI ≠ authority). */
export const DEFAULT_LIVE_SYSTEM_PROMPT =
  "You are the ElastOS Home Agent, running privately on this machine. " +
  "You have no tools and no capsule authority in this session; answer from " +
  "knowledge only, and say so plainly when a task would need a tool or grant.";

export const DEFAULT_LIVE_MAX_TOKENS = 8192;
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
  catalogModels: [],
};

let probePromise = null;
let probeEpoch = 0;

export function getLiveInferenceState() {
  return { ...liveState, models: liveState.models.slice() };
}

/** Chat offers → model-menu rows. An offer exists only when its backend is
 * configured (readiness-honest provider), so listing is the truth probe. */
function chatOfferRows(offers) {
  return textOfferRows(eligibleTextOffers(offers));
}

/* An existing choice stays exact. Only a workspace with no choice uses the
   initial first advertised offer; later absence never substitutes another. */
let selectedLiveOfferId = "";
let selectedContentCid = null;

export function selectLiveOffer(offerId, modelCid = null) {
  selectedLiveOfferId = typeof offerId === "string" ? offerId : "";
  selectedContentCid = modelCid;
  if (!liveState.checking) {
    liveState.live = Boolean(currentChoice(liveState.models, liveState.catalogModels));
  }
}

export function liveContentChoice() { return selectedContentCid; }
export function liveContentModels() {
  return readyModelChoices(liveState.catalogModels || [], liveState.models.map(m => ({ ...m, id: m.offerId })));
}
function currentChoice(models, content) {
  // Initial empty workspace may use the first offer. An existing choice stays exact.
  const id = selectedLiveOfferId || (selectedContentCid == null ? models[0]?.offerId : "");
  return selectedModelOffer(models.map(m => ({ ...m, id: m.offerId })), id, selectedContentCid, content || []);
}

export function liveOfferChoice() {
  return selectedLiveOfferId;
}

export function selectedLiveOffer() {
  if (!liveState.live || liveState.checking) return null;
  return currentChoice(liveState.models, liveState.catalogModels);
}

/** Cached offers_list — model menu + Configure panel + probe share it. */
let offersCache = null;
let offersPromise = null;

export async function fetchModelOffers({ force = false } = {}) {
  if (!force && offersCache) {
    return offersCache;
  }
  if (!force && offersPromise) {
    return offersPromise;
  }
  const request = (async () => {
    const data = await modelRunCall("offers_list");
    if (offersPromise === request) offersCache = data;
    return data;
  })()
    .finally(() => {
      if (offersPromise === request) offersPromise = null;
    });
  offersPromise = request;
  return request;
}

/**
 * A current successful offers read must contain the exact chosen text offer.
 * Refresh ownership applies only to readiness, never to accepted run settlement.
 */
export async function probeLiveInference({ force = false } = {}) {
  const now = Date.now();
  if (!force && (liveState.checking || now - liveState.checkedAt < PROBE_TTL_MS)) {
    return probePromise ? probePromise.then(getLiveInferenceState) : getLiveInferenceState();
  }
  const epoch = ++probeEpoch;
  liveState.checking = true;
  liveState.live = false;
  probePromise = (async () => {
    try {
      /* Reachability and offers in one call: the 0.7.1 model-provider contract
         is offers_list / runs_*; it has no ping. */
      const [offers, content] = await Promise.all([
        fetchModelOffers({ force: true }),
        fetch(new URL("/api/capsules/catalog", window.location.href).href, {
          headers: { "x-elastos-home-token": getHomeGuiLaunchToken() },
        }).then(async response => response.ok ? catalogModels(await response.json()) : []).catch(() => []),
      ]);
      if (epoch !== probeEpoch) return;
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
          catalogModels: content,
        };
        return;
      }
      liveState = {
        live: Boolean(currentChoice(models, content)),
        checking: false,
        model: currentChoice(models, content)?.label || "Chosen model unavailable",
        endpointState: "model-offers",
        reason: "ready",
        checkedAt: Date.now(),
        models,
        catalogModels: content,
      };
    } catch (error) {
      if (epoch !== probeEpoch) return;
      liveState = {
        live: false,
        checking: false,
        model: "",
        endpointState: "unreachable",
        reason: String(error?.code || error?.message || "unreachable"),
        checkedAt: Date.now(),
        models: [],
        catalogModels: [],
      };
    } finally {
      if (epoch === probeEpoch) probePromise = null;
    }
  })();
  await probePromise;
  return getLiveInferenceState();
}

/** Gather → compile → serialize. This is the only live context-policy owner. */
export function compileLiveContext({
  session = null,
  sessionMessages = null,
  currentInput = null,
  systemPrompt,
  notes,
  maxTokens,
  capabilities = TEXT_CONTRACT_CAPABILITIES,
  degradedFallback = false,
} = {}) {
  const raw = sessionMessages || session?.messages || [];
  let { history, currentInput: splitCurrent } = splitSessionMessages(raw);
  const current = currentInput || splitCurrent;
  if (degradedFallback) {
    history = history.slice(-EMERGENCY_HISTORY_MESSAGE_LIMIT);
  }
  const system = normalizeLiveSystemPrompt(systemPrompt);
  const constraints = [
    {
      id: "system",
      kind: "system_policy",
      role: "system",
      authority: "system",
      content: system,
      sourceRef: "systemPrompt",
      mustInclude: true,
      reason: "mandatory",
    },
  ];
  const noteText = normalizeAgentNotes(notes);
  if (noteText) {
    constraints.push({
      id: "runtime-notes",
      kind: "runtime_context",
      role: "system",
      authority: "runtime",
      content: `On-Home notes (runtime · host-persisted; not a user message):\n${noteText}`,
      sourceRef: "agentNotes",
      mustInclude: true,
      reason: "mandatory",
    });
  }
  const thinkingChars = (Array.isArray(raw) ? raw : []).reduce(
    (n, m) => n + String(m?.thinking || "").length,
    0,
  );
  const compiled = compileContext({
    history,
    currentInput: current,
    constraints,
    intent: "",
    budget: makeBudget(capabilities.context, clampLiveMaxTokens(maxTokens)),
    capabilities,
    transcriptChars: transcriptCharCount(raw),
    thinkingChars,
  });
  attachProviderPayload(compiled);
  logContextManifest(compiled.manifest, compiled.invariants);
  return compiled;
}

let liveStreamEpoch = 0;
let activeRun = null;

export function abortLiveChatStream() {
  const run = activeRun;
  if (!run) return Promise.resolve();
  run.cancelRequested = true;
  if (run.cancelPromise) return run.cancelPromise;
  run.patch({ state: TurnState.CANCEL_PENDING, completedAt: null });
  if (!run.id) return Promise.resolve();
  const runId = run.id;
  run.cancelPromise = modelRunCall("runs_cancel", { run_id: runId, request_id: newRequestId() })
    .catch(() => {
      if (activeRun === run) {
        run.patch({ state: TurnState.SETTLEMENT_UNKNOWN, completedAt: null });
      }
    });
  return run.cancelPromise;
}

/* Detach the UI from the in-flight run WITHOUT cancelling it. The model contract
   runs server-side (runs_events is a client cursor-poll, not a held connection),
   so the run continues; the UI simply stops consuming its events. Navigation-away
   uses this so a long generation isn't killed; explicit Stop still uses
   abortLiveChatStream (runs_cancel). */
export function detachLiveChatStream() {
  activeRun?.patch({ state: TurnState.SETTLEMENT_UNKNOWN, completedAt: null });
  liveStreamEpoch += 1;
  activeRun = null;
}

export function unresolvedModelTurn(turn) {
  return turn?.attachmentAllowed !== false && (!turn?.actorCapsule || turn.actorCapsule === "assistant") && Boolean(turn?.providerRunId || turn?.createRequestId) && !turn.completedAt && ![TurnState.COMPLETED, TurnState.FAILED, TurnState.STOPPED].includes(turn.state);
}

export async function modelRunCall(op, body = {}) {
  const token = getHomeGuiLaunchToken();
  if (!token) {
    const error = new Error("missing home launch token in Home GUI shell");
    error.code = "missing-home-launch-token";
    error.preDispatchRefusal = true;
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

function newRequestId() {
  return globalThis.crypto?.randomUUID
    ? globalThis.crypto.randomUUID()
    : `req-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 10)}`;
}

/**
 * Stream a chat turn through the typed model contract: runs_create on the
 * selected text offer, then runs_events cursor-poll by after_sequence.
 * onDelta({ reasoningDelta, contentDelta, done, seq }) is called as text_delta
 * events arrive; Stop requests cancellation and keeps reading Runtime settlement.
 * Full strings are joined once after unlock — not on every poll.
 */
export async function streamChatViaContract(
  messages,
  {
    onDelta,
    onAccepted,
    onState,
    maxTokens = DEFAULT_LIVE_MAX_TOKENS,
    requestedEffort = "medium",
    contextManifest = null,
    turnManifest = null,
    inputParts = [],
  } = {},
) {
  detachLiveChatStream();
  const epoch = liveStreamEpoch;
  if (contextManifest) {
    assertProviderPayloadUnchanged(messages, contextManifest.providerPayloadHash);
  }
  const startedAt = Date.now();
  /* Reasoning effort is recorded on the manifest, never faked via prompt text. */
  const reasoning = resolveReasoningPolicy(requestedEffort);
  const manifest = contextManifest
    ? {
        ...contextManifest,
        requestedEffort: reasoning.requested,
        effectiveEffort: reasoning.effective,
        effortDegraded: reasoning.degraded,
      }
    : { requestedEffort: reasoning.requested, effectiveEffort: reasoning.effective, effortDegraded: reasoning.degraded };
  const turn =
    turnManifest ||
    turnStorePut(
      createTurnManifest({
        turnId: newTurnId(),
        inputParts,
        reasoning,
        contextManifest: manifest,
        startedAt,
      }),
    );
  turnStorePut(turn);
  const patch = (fields) => {
    const next = turnStorePatch(turn.turnId, fields) || turn;
    onState?.(next);
    return next;
  };
  const run = { id: null, patch, cancelRequested: false, cancelPromise: null };
  activeRun = run;
  if (turn.createRequestId && !turn.providerRunId) {
    activeRun = null;
    throw contractError("run_acceptance_unknown", "Run acceptance is unknown. Start a new chat.");
  }
  /* The typed text input has no sampling knobs; the offer's policy owns them.
     maxTokens shaped the context budget upstream and stays on the manifest. */
  void clampLiveMaxTokens(maxTokens);
  const offer = selectedLiveOffer();
  if (!offer && !turn.providerRunId) {
    activeRun = null;
    throw contractError("no_model_offers", "no text model offer on this Home");
  }
  const resuming = Boolean(turn.providerRunId);
  let created;
  let runId;
  const createRequestId = resuming ? null : newRequestId();
  if (!resuming) patch({ createRequestId, state: TurnState.SUBMITTED, completedAt: null });
  try {
    created = resuming
      ? await modelRunCall("runs_get", { run_id: turn.providerRunId, request_id: newRequestId() })
      : await modelRunCall("runs_create", textRunCreateBody({ offer, messages, requestId: createRequestId }));
    runId = typeof created?.run_id === "string" ? created.run_id : "";
    if (!runId || runId.trim() !== runId || runId.length > 80 || (resuming && runId !== turn.providerRunId)) {
      throw contractError("no_run_id", "contract returned no run id");
    }
  } catch (error) {
    if (activeRun === run) activeRun = null;
    if (!resuming) {
      const refused = error.preDispatchRefusal === true;
      patch({ state: refused ? TurnState.FAILED : TurnState.SETTLEMENT_UNKNOWN,
        error: refused ? error.code : "run_acceptance_unknown", completedAt: refused ? Date.now() : null });
      if (!refused) throw contractError("run_acceptance_unknown", "Run acceptance is unknown. Start a new chat.");
    }
    throw error;
  }
  if (resuming && activeRun !== run) {
    return { detached: true, turnManifest: turn };
  }
  let afterSequence = !resuming && Number.isInteger(Number(created?.sequence_cursor))
    ? Number(created.sequence_cursor)
    : 0;
  run.id = runId;
  patch({
    providerRunId: runId,
    state: TurnState.SUBMITTED,
  });
  patch({ state: activeRun === run ? TurnState.STREAMING : TurnState.SETTLEMENT_UNKNOWN, completedAt: null });
  if (activeRun === run) onAccepted?.({ run_id: runId, turnId: turn.turnId });
  if (run.cancelRequested && activeRun === run) void abortLiveChatStream();

  let seq = 0;
  let streamedChars = 0;
  const emit = (reasoningDelta, contentDelta, done = false) =>
    onDelta?.({ reasoningDelta, contentDelta, done, seq });
  const finish = (extra = {}) => {
    const next = extra.detached ? turn : patch({
      state: extra.settlementUnknown ? TurnState.SETTLEMENT_UNKNOWN : extra.aborted ? TurnState.STOPPED : TurnState.COMPLETED,
      completedAt: Date.now(),
    });
    return {
      usage: null,
      latencyMs: Date.now() - startedAt,
      aborted: false,
      seq,
      reasoning,
      contextHash: manifest.semanticContextHash || manifest.contextHash,
      providerPayloadHash: manifest.providerPayloadHash,
      ...extra,
      turnManifest: next,
    };
  };

  try {
    for (;;) {
      if (epoch !== liveStreamEpoch || activeRun !== run) {
        return finish({ detached: true });
      }
      const page = await modelRunCall("runs_events", {
        run_id: runId,
        request_id: newRequestId(),
        after_sequence: afterSequence,
      });
      if (epoch !== liveStreamEpoch || activeRun !== run) {
        return finish({ detached: true });
      }
      const applied = applyRunEventsPage(page, afterSequence);
      afterSequence = applied.nextCursor;
      let eventsInSlice = 0;
      let sliceStart = Date.now();
      for (const delta of applied.textDeltas) {
        if (epoch !== liveStreamEpoch || activeRun !== run) {
          return finish({ detached: true });
        }
        seq += 1;
        streamedChars += delta.length;
        emit("", delta, false);
        eventsInSlice += 1;
        if (eventsInSlice >= YIELD_EVENT_SLICE || Date.now() - sliceStart >= YIELD_MS) {
          await yieldToBrowser();
          eventsInSlice = 0;
          sliceStart = Date.now();
        }
      }
      if (epoch !== liveStreamEpoch || activeRun !== run) {
        return finish({ detached: true });
      }
      const terminal = applied.terminal || (!applied.hasMore && resuming ? created.terminal : null);
      if (terminal) {
        if (terminal.status === "completed") {
          /* A provider that did not stream settles with the whole text once. */
          const finalText = terminalOutputText(terminal.output);
          if (finalText && streamedChars === 0) {
            seq += 1;
            emit("", finalText, false);
          }
          return finish();
        }
        if (terminal.status === "cancelled") {
          return finish({ aborted: true });
        }
        if (terminal.status === "settlement_unknown") {
          return finish({ settlementUnknown: true });
        }
        patch({ state: TurnState.FAILED, completedAt: Date.now() });
        const detail = terminal.error && typeof terminal.error === "object" ? terminal.error : {};
        throw contractError(
          String(detail.code || terminal.status || "run_failed"),
          String(detail.message || `run ${terminal.status}`),
        );
      }
      if (applied.hasMore) {
        continue;
      }
      await new Promise((resolve) => setTimeout(resolve, CONTRACT_POLL_MS));
    }
  } finally {
    if (activeRun === run) {
      activeRun = null;
    }
  }
}
