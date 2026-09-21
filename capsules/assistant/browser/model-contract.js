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

function backendFactText(fact, fallback = "") {
  if (!fact || typeof fact !== "object") {
    return fallback;
  }
  if (fact.status === "unknown") {
    return fallback;
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

function reportedRunFact(fact) {
  if (!fact || typeof fact !== "object") {
    return "";
  }
  if (fact.status === "unknown") {
    return "Not reported";
  }
  return backendFactText(fact, "Not reported");
}

function offerPolicyLimits(policy) {
  if (!policy || typeof policy !== "object") {
    return "";
  }
  const parts = [];
  if (Number.isFinite(Number(policy.concurrency_limit))) {
    const count = Number(policy.concurrency_limit);
    parts.push(count === 1 ? "1 run at a time" : `${count} runs at a time`);
  }
  if (Number.isFinite(Number(policy.input_bytes_limit))) {
    parts.push(`prompts up to ${Number(policy.input_bytes_limit)} bytes`);
  }
  if (Number.isFinite(Number(policy.runtime_ms_limit))) {
    const ms = Number(policy.runtime_ms_limit);
    parts.push(ms % 1000 === 0 ? `${ms / 1000} seconds` : `${ms} ms`);
  }
  return parts.join("; ");
}

function hostedObject(offer) {
  return offer?.hosted && typeof offer.hosted === "object" ? offer.hosted : null;
}

function remoteServiceName(offer) {
  return typeof offer?.remote_service?.display_name === "string" && offer.remote_service.display_name.trim() !== ""
    ? offer.remote_service.display_name.trim()
    : "";
}

/** Route that will execute the selected offer. */
export function offerRouteKind(offer) {
  const hosted = hostedObject(offer);
  const remote = offer?.remote_service && typeof offer.remote_service === "object";
  if (hosted && remote) {
    return "remote_hosted";
  }
  if (hosted) {
    return "hosted";
  }
  if (remote) {
    return "remote";
  }
  return "local";
}

/** Short route line under an instance name. */
export function offerRouteSubtitle(offer) {
  const hosted = hostedObject(offer);
  const kind = offerRouteKind(offer);
  const remoteName = remoteServiceName(offer);
  const processor =
    typeof hosted?.backend_provider_label === "string" && hosted.backend_provider_label.trim() !== ""
      ? hosted.backend_provider_label.trim()
      : "";
  if (kind === "hosted") {
    return `${processor || "Hosted"} · hosted`;
  }
  if (kind === "remote") {
    return remoteName ? `via ${remoteName}` : "via another Home";
  }
  if (kind === "remote_hosted") {
    return remoteName ? `via ${remoteName}` : "via another Home";
  }
  return "This Home · local";
}

function promptDestination(kind, facts) {
  if (kind === "remote_hosted") {
    const home = facts.intermediary || "another Home";
    const processor = facts.processor || "a hosted processor";
    return `${home}, then ${processor}`;
  }
  if (kind === "hosted") {
    return facts.processor || "a hosted processor";
  }
  if (kind === "remote") {
    return facts.intermediary || "another Home";
  }
  return "This Home";
}

function privacyLabel(kind, facts) {
  if (kind === "hosted" || kind === "remote_hosted") {
    return facts.privacy || "Not reported";
  }
  if (kind === "remote") {
    return "The prompt leaves this Home";
  }
  return "This Home keeps the prompt";
}

function availabilityLabel(kind, facts) {
  if (kind === "remote_hosted") {
    return facts.intermediary ? `Through ${facts.intermediary}` : "Through another Home";
  }
  if (kind === "hosted") {
    return "Hosted";
  }
  if (kind === "remote") {
    return facts.intermediary ? `Through ${facts.intermediary}` : "Through another Home";
  }
  return "On this Home";
}

function offerDetailRows(kind, facts) {
  const rows = [];
  const push = (term, value) => {
    if (typeof value === "string" && value.trim() !== "") {
      rows.push({ term, value: value.trim() });
    }
  };
  push("Processor", kind === "local" ? "This Home" : facts.processor || facts.intermediary);
  push("Prompt destination", promptDestination(kind, facts));
  push("Payer", facts.payer === "this Home" ? "This Home" : facts.payer);
  push("Limits", facts.limits);
  push("Privacy", privacyLabel(kind, facts));
  push("Availability", availabilityLabel(kind, facts));
  push("Cost", facts.cost);
  return rows;
}

/** Selection facts the Assistant model picker must show. */
export function offerSelectionFacts(offer, backendReport = null) {
  const hosted = hostedObject(offer);
  const kind = offerRouteKind(offer);
  const remoteName = remoteServiceName(offer);
  const requestedModel =
    (typeof hosted?.requested_selector === "string" && hosted.requested_selector.trim() !== ""
      ? hosted.requested_selector.trim()
      : "") || (typeof offer?.title === "string" ? offer.title : "");
  const resolvedModel = reportedRunFact(backendReport?.resolved_model);
  const processor = typeof hosted?.backend_provider_label === "string" && hosted.backend_provider_label.trim() !== ""
    ? hosted.backend_provider_label.trim()
    : "";
  const intermediary = remoteName
    || (typeof hosted?.intermediary === "string" && hosted.intermediary.trim() !== ""
      ? hosted.intermediary.trim()
      : "");
  const payer = typeof hosted?.payer === "string" && hosted.payer.trim() !== ""
    ? hosted.payer.trim()
    : kind === "remote_hosted"
      ? "this Home"
      : kind === "hosted"
        ? "this Home"
        : "";
  const provider = kind === "remote_hosted"
    ? processor || "hosted provider"
    : kind === "hosted"
      ? processor || "hosted provider"
      : kind === "remote"
        ? remoteName || "remote Home"
        : "this Home";
  const execution = kind === "remote_hosted"
    ? `via ${intermediary || "remote Home"}; ${processor || "hosted provider"}; payer ${payer || "this Home"}`
    : kind === "hosted"
      ? `hosted via ${provider}`
      : kind === "remote"
        ? `via ${remoteName || "remote Home"}`
        : "this Home";
  const privacy = kind === "hosted" || kind === "remote_hosted"
    ? typeof hosted?.privacy_policy_ref === "string" && hosted.privacy_policy_ref.trim() !== ""
      ? hosted.privacy_policy_ref.trim()
      : ""
    : "";
  const cost = reportedRunFact(backendReport?.cost);
  const fallback = kind === "hosted" || kind === "remote_hosted"
    ? typeof hosted?.upstream_routing_fallback_assertion === "string" &&
      hosted.upstream_routing_fallback_assertion.trim() !== ""
      ? hosted.upstream_routing_fallback_assertion.trim()
      : ""
    : "";
  const limits = offerPolicyLimits(offer?.policy);
  const routeSubtitle = offerRouteSubtitle(offer);
  const facts = {
    requestedModel,
    resolvedModel,
    provider,
    execution,
    limits,
    privacy,
    cost,
    fallback,
    intermediary,
    processor,
    payer,
    routeSubtitle,
    summary: `${typeof offer?.title === "string" ? offer.title : requestedModel}. ${routeSubtitle}`,
  };
  facts.detailRows = offerDetailRows(kind, facts);
  return facts;
}

function offerRowDetail(offer, facts) {
  return facts.routeSubtitle || offerRouteSubtitle(offer);
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
