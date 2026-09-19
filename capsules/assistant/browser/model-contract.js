/* Typed model contract — pure functions, no DOM.

   The Home Agent talks to the model-provider only through the Runtime's
   typed operations: offers_list, runs_create, runs_events, runs_cancel.
   Offers come from the provider; nothing here names an offer. The event page
   is validated the way Assistant validates it: a monotonic cursor, strictly
   increasing sequences, a terminal event ends the run. */

export const MODEL_TEXT_INPUT_SCHEMA = "elastos.model.input.text/v1";
export const MODEL_TEXT_OUTPUT_SCHEMA = "elastos.model.output.text/v1";

const TEXT_MODALITY = "text/plain";

/** Offers that take text and answer with text, exactly as the provider lists them. */
export function eligibleTextOffers(payload) {
  const offers = Array.isArray(payload?.offers)
    ? payload.offers
    : Array.isArray(payload?.data?.offers)
      ? payload.data.offers
      : [];
  return offers.filter(
    (offer) =>
      offer &&
      typeof offer.id === "string" &&
      offer.id.trim() !== "" &&
      typeof offer.title === "string" &&
      typeof offer.operation === "string" &&
      Array.isArray(offer.input_modalities) &&
      Array.isArray(offer.output_modalities) &&
      offer.input_modalities.includes(TEXT_MODALITY) &&
      offer.output_modalities.includes(TEXT_MODALITY),
  );
}

function backendFactText(fact, fallback) {
  if (!fact || typeof fact !== "object") {
    return fallback;
  }
  if (fact.status === "unknown") {
    return "unknown";
  }
  if (fact.status === "reported") {
    const value = fact.value;
    if (typeof value === "string" && value.trim() !== "") {
      return value.trim();
    }
    if (value && typeof value === "object") {
      const amount = typeof value.value === "string" ? value.value.trim() : "";
      const unit = typeof value.unit === "string" ? value.unit.trim() : "";
      if (amount && unit) {
        return `${amount} ${unit}`;
      }
      if (amount) {
        return amount;
      }
    }
  }
  return fallback;
}

function offerPolicyLimits(policy) {
  if (!policy || typeof policy !== "object") {
    return "offer policy unavailable";
  }
  const parts = [];
  if (Number.isFinite(Number(policy.concurrency_limit))) {
    parts.push(`concurrency ${Number(policy.concurrency_limit)}`);
  }
  if (Number.isFinite(Number(policy.input_bytes_limit))) {
    parts.push(`input ${Number(policy.input_bytes_limit)} B`);
  }
  if (Number.isFinite(Number(policy.runtime_ms_limit))) {
    parts.push(`${Number(policy.runtime_ms_limit)} ms`);
  }
  return parts.length ? parts.join("; ") : "offer policy unavailable";
}

/** Selection facts the Assistant model picker must show. */
export function offerSelectionFacts(offer, backendReport = null) {
  const hosted = offer?.hosted && typeof offer.hosted === "object" ? offer.hosted : null;
  const remoteName =
    typeof offer?.remote_service?.display_name === "string" && offer.remote_service.display_name.trim() !== ""
      ? offer.remote_service.display_name.trim()
      : "";
  const requestedModel =
    (typeof hosted?.requested_selector === "string" && hosted.requested_selector.trim() !== ""
      ? hosted.requested_selector.trim()
      : "") || (typeof offer?.title === "string" ? offer.title : "");
  const resolvedModel = backendFactText(
    backendReport?.resolved_model,
    hosted ? "unknown" : requestedModel || "unknown",
  );
  const provider = hosted
    ? typeof hosted.backend_provider_label === "string" && hosted.backend_provider_label.trim() !== ""
      ? hosted.backend_provider_label.trim()
      : "hosted provider"
    : remoteName || "this Home";
  const privacy = hosted
    ? typeof hosted.privacy_policy_ref === "string" && hosted.privacy_policy_ref.trim() !== ""
      ? hosted.privacy_policy_ref.trim()
      : "hosted privacy policy unavailable"
    : "on this Home";
  const cost = backendFactText(backendReport?.cost, hosted ? "unknown" : "none");
  const fallback = hosted
    ? typeof hosted.upstream_routing_fallback_assertion === "string" &&
      hosted.upstream_routing_fallback_assertion.trim() !== ""
      ? hosted.upstream_routing_fallback_assertion.trim()
      : "unknown"
    : "none";
  const limits = offerPolicyLimits(offer?.policy);
  return {
    requestedModel,
    resolvedModel,
    provider,
    limits,
    privacy,
    cost,
    fallback,
    summary: `requested ${requestedModel}; resolved ${resolvedModel}; provider ${provider}; limits ${limits}; privacy ${privacy}; cost ${cost}; fallback ${fallback}`,
  };
}

function offerRowDetail(offer, facts) {
  if (offer?.hosted && typeof offer.hosted === "object") {
    return `Hosted · ${facts.provider}`;
  }
  if (
    typeof offer?.remote_service?.display_name === "string" &&
    offer.remote_service.display_name.trim() !== ""
  ) {
    return `Model offer · via ${offer.remote_service.display_name.trim()}`;
  }
  return "Model offer · this Home";
}

/** Menu rows for the composer's model chip; the id carries the offer. */
export function textOfferRows(offers, backendReports = {}) {
  return offers.map((offer) => {
    const facts = offerSelectionFacts(
      offer,
      backendReports && typeof backendReports === "object" ? backendReports[offer.id] : null,
    );
    return {
      id: `live:${offer.id}`,
      offerId: offer.id,
      operation: offer.operation,
      label: offer.title,
      detail: offerRowDetail(offer, facts),
      streamOutput: offer.stream_output === true,
      selectionFacts: facts,
    };
  });
}

/**
 * The typed text input is a single prompt. A chat turn is rendered as a
 * transcript so the model sees the conversation; the compiled message list
 * (system, user, agent) is the source of truth and is not reordered.
 */
export function transcriptPrompt(messages) {
  const lines = [];
  for (const message of Array.isArray(messages) ? messages : []) {
    const content = typeof message?.content === "string" ? message.content : "";
    if (!content.trim()) {
      continue;
    }
    const role = String(message?.role || "user");
    if (role === "system") {
      lines.push(content.trim());
    } else if (role === "assistant" || role === "agent") {
      lines.push(`Assistant: ${content.trim()}`);
    } else {
      lines.push(`User: ${content.trim()}`);
    }
  }
  lines.push("Assistant:");
  return lines.join("\n\n");
}

export function textRunCreateBody({ offer, messages, requestId }) {
  if (!offer || typeof offer.offerId !== "string" || typeof offer.operation !== "string") {
    throw contractError("no_offer", "no text model offer selected");
  }
  if (typeof requestId !== "string" || requestId.trim() === "") {
    throw contractError("no_request_id", "run request needs a request id");
  }
  return {
    offer_id: offer.offerId,
    operation: offer.operation,
    request_id: requestId,
    input: {
      schema: MODEL_TEXT_INPUT_SCHEMA,
      prompt: transcriptPrompt(messages),
    },
  };
}

export function parseCursor(value) {
  const cursor = Number(value);
  return Number.isInteger(cursor) && cursor >= 0 ? cursor : null;
}

/** Studio progress on a `progress` event. Null when the payload is malformed. */
export function parseStudioProgress(data) {
  const phase =
    typeof data?.phase === "string" && data.phase.trim() === data.phase ? data.phase : "";
  const completed = Number(data?.completed);
  const total = Number(data?.total);
  if (
    !phase ||
    !Number.isInteger(completed) ||
    !Number.isInteger(total) ||
    completed < 0 ||
    total < 0 ||
    completed > total
  ) {
    return null;
  }
  return { phase, completed, total };
}

/**
 * Validate a runs_events page against the cursor we hold and reduce it to
 * what Chat and Studio need. The provider may replay events at or below the
 * saved cursor; this function skips those. A new sequence must rise from
 * the last newly applied event. An unseen earlier sequence is rejected. A
 * cursor must not move backwards. Throws on a page the provider should
 * never send.
 * @returns {{ nextCursor: number, hasMore: boolean, textDeltas: string[],
 *             terminal: null | { status: string, output: unknown, error: unknown },
 *             studioProgress: null | { phase: string, completed: number, total: number } }}
 */
export function applyRunEventsPage(page, afterSequence) {
  if (!page || typeof page !== "object" || !Array.isArray(page.events)) {
    throw contractError("bad_events_page", "run events page is malformed");
  }
  const nextCursor = parseCursor(page.next_cursor);
  if (nextCursor === null || nextCursor < afterSequence) {
    throw contractError("bad_cursor", "run events cursor went backwards");
  }
  const textDeltas = [];
  let terminal = null;
  let studioProgress = null;
  let backendReport = null;
  let lastSequence = afterSequence;
  for (const event of page.events) {
    const sequence = parseCursor(event?.sequence);
    if (sequence === null) {
      throw contractError("bad_sequence", "run events are not strictly increasing");
    }
    if (sequence <= afterSequence) {
      continue;
    }
    if (sequence < lastSequence) {
      throw contractError("bad_sequence", "run events arrived out of order");
    }
    if (sequence === lastSequence) {
      continue;
    }
    lastSequence = sequence;
    const kind = typeof event.kind === "string" ? event.kind : "";
    if (kind === "text_delta") {
      const text = event.data?.text;
      if (typeof text === "string" && text) {
        textDeltas.push(text);
      }
    } else if (kind === "progress") {
      const progress = parseStudioProgress(event.data);
      if (!progress) {
        throw contractError("bad_progress", "studio progress event is malformed");
      }
      studioProgress = progress;
    } else if (kind === "output") {
      terminal = { status: "completed", output: event.data ?? null, error: null };
    } else if (kind === "completed") {
      const retained = event.data?.output_retained !== false;
      terminal = {
        status: "completed",
        output: retained ? event.data ?? null : null,
        error: null,
        ...(retained ? {} : { outputRetained: false }),
      };
    } else if (kind === "failed" || kind === "cancelled" || kind === "settlement_unknown") {
      terminal = { status: kind, output: null, error: event.data ?? null };
    }
    if (event.terminal === true && !terminal) {
      terminal = { status: "completed", output: null, error: null };
    }
    if (event.backend_report && typeof event.backend_report === "object") {
      backendReport = event.backend_report;
    }
  }
  if (terminal && page.output_retained === false) {
    terminal.outputRetained = false;
    terminal.output = null;
  }
  if (nextCursor < lastSequence) {
    throw contractError("bad_cursor", "run events cursor behind last sequence");
  }
  return {
    nextCursor,
    hasMore: page.has_more === true,
    textDeltas,
    terminal,
    studioProgress,
    backendReport,
  };
}

/** Final text the provider settles with, when the output is typed text. */
export function terminalOutputText(output) {
  if (output && typeof output === "object" && output.schema === MODEL_TEXT_OUTPUT_SCHEMA) {
    return typeof output.text === "string" ? output.text : "";
  }
  return "";
}

export function contractError(code, message) {
  const error = new Error(message);
  error.code = code;
  return error;
}
