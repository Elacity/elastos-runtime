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

/** Menu rows for the composer's model chip; the id carries the offer. */
export function textOfferRows(offers) {
  return offers.map((offer) => ({
    id: `live:${offer.id}`,
    offerId: offer.id,
    operation: offer.operation,
    label: offer.title,
    detail: "Model offer · this Home",
    streamOutput: offer.stream_output === true,
  }));
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

/**
 * Validate a runs_events page against the cursor we hold and reduce it to
 * what the stream needs. Throws on a page the provider should never send.
 * @returns {{ nextCursor: number, hasMore: boolean, textDeltas: string[],
 *             terminal: null | { status: string, output: unknown, error: unknown } }}
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
  let lastSequence = afterSequence;
  for (const event of page.events) {
    const sequence = parseCursor(event?.sequence);
    if (sequence === null || sequence <= lastSequence) {
      throw contractError("bad_sequence", "run events are not strictly increasing");
    }
    lastSequence = sequence;
    const kind = typeof event.kind === "string" ? event.kind : "";
    if (kind === "text_delta") {
      const text = event.data?.text;
      if (typeof text === "string" && text) {
        textDeltas.push(text);
      }
    } else if (kind === "output") {
      terminal = { status: "completed", output: event.data ?? null, error: null };
    } else if (kind === "failed" || kind === "cancelled" || kind === "settlement_unknown") {
      terminal = { status: kind, output: null, error: event.data ?? null };
    }
    if (event.terminal === true && !terminal) {
      terminal = { status: "completed", output: null, error: null };
    }
  }
  if (nextCursor < lastSequence) {
    throw contractError("bad_cursor", "run events cursor behind last sequence");
  }
  return { nextCursor, hasMore: page.has_more === true, textDeltas, terminal };
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
