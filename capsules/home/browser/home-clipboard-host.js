import {
  HOME_CLIPBOARD_CANCEL_SCHEMA,
  HOME_CLIPBOARD_CANCEL_TYPE,
  HOME_CLIPBOARD_MIME_TYPE,
  HOME_CLIPBOARD_READY_SCHEMA,
  HOME_CLIPBOARD_READY_TYPE,
  HOME_CLIPBOARD_REQUEST_SCHEMA,
  HOME_CLIPBOARD_REQUEST_TYPE,
  HOME_CLIPBOARD_RESULT_SCHEMA,
  HOME_CLIPBOARD_RESULT_TYPE,
  HOME_CLIPBOARD_RETIRE_SCHEMA,
  HOME_CLIPBOARD_RETIRE_TYPE,
  MAX_HOME_CLIPBOARD_GENERATION_BYTES,
  MAX_HOME_CLIPBOARD_PURPOSE_CODE_UNITS,
  MAX_HOME_CLIPBOARD_REQUEST_ID_BYTES,
  MAX_HOME_CLIPBOARD_TEXT_UTF8_BYTES,
  homeClipboardExactKeys,
  homeClipboardOperationAllowed,
  homeClipboardResultErrorSupported,
  homeClipboardTargetSupported,
  homeClipboardValidToken,
  homeClipboardValidWriteText,
} from "./home-clipboard-protocol.js?v=home-20260807a";

export {
  HOME_CLIPBOARD_CANCEL_SCHEMA,
  HOME_CLIPBOARD_CANCEL_TYPE,
  HOME_CLIPBOARD_MIME_TYPE,
  HOME_CLIPBOARD_READY_SCHEMA,
  HOME_CLIPBOARD_READY_TYPE,
  HOME_CLIPBOARD_REQUEST_SCHEMA,
  HOME_CLIPBOARD_REQUEST_TYPE,
  HOME_CLIPBOARD_RESULT_SCHEMA,
  HOME_CLIPBOARD_RESULT_TYPE,
  HOME_CLIPBOARD_RETIRE_SCHEMA,
  HOME_CLIPBOARD_RETIRE_TYPE,
  MAX_HOME_CLIPBOARD_GENERATION_BYTES,
  MAX_HOME_CLIPBOARD_PURPOSE_CODE_UNITS,
  MAX_HOME_CLIPBOARD_REQUEST_ID_BYTES,
  MAX_HOME_CLIPBOARD_TEXT_UTF8_BYTES,
  homeClipboardTargetSupported,
};

export const HOME_CLIPBOARD_TIMEOUT_MS = 15_000;
export const HOME_CLIPBOARD_REPLAY_TTL_MS = 5 * 60_000;
export const MAX_HOME_CLIPBOARD_REPLAY_IDS = 64;

const OPAQUE_CAPSULE_ORIGIN = "null";
const OPAQUE_FRAME_TARGET = "*";

function boundedRequestId(value) {
  return homeClipboardValidToken(value, MAX_HOME_CLIPBOARD_REQUEST_ID_BYTES);
}

function boundedGeneration(value) {
  return homeClipboardValidToken(value, MAX_HOME_CLIPBOARD_GENERATION_BYTES);
}

function validRequestPayload(targetId, data) {
  const expectedKeys = [
    "type",
    "schema",
    "requestId",
    "homeToken",
    "parentOrigin",
    "generation",
    "operation",
    "purpose",
    "mime_type",
  ];
  if (data?.operation === "write") {
    expectedKeys.push("text");
  }
  return Boolean(
    homeClipboardExactKeys(data, expectedKeys) &&
      data.type === HOME_CLIPBOARD_REQUEST_TYPE &&
      data.schema === HOME_CLIPBOARD_REQUEST_SCHEMA &&
      boundedRequestId(data.requestId) &&
      boundedGeneration(data.generation) &&
      data.mime_type === HOME_CLIPBOARD_MIME_TYPE &&
      homeClipboardOperationAllowed(targetId, data.purpose, data.operation) &&
      (data.operation !== "write" ||
        homeClipboardValidWriteText(targetId, data.purpose, data.text))
  );
}

function validContext(event, context) {
  const state = context?.clipboardState;
  return Boolean(
    context?.kind === "app-frame" &&
      homeClipboardTargetSupported(context.targetId) &&
      context.homeToken &&
      context.parentOrigin &&
      context.origin === OPAQUE_CAPSULE_ORIGIN &&
      event?.origin === OPAQUE_CAPSULE_ORIGIN &&
      context.source &&
      context.source === event?.source &&
      state &&
      !state.retired &&
      state.source === event.source &&
      state.targetId === context.targetId &&
      state.homeToken === context.homeToken &&
      state.parentOrigin === context.parentOrigin &&
      boundedGeneration(state.generation)
  );
}

function requestEnvelopeValid(event, context, data) {
  const state = context?.clipboardState;
  return Boolean(
    validContext(event, context) &&
      validRequestPayload(context.targetId, data) &&
      data.homeToken === context.homeToken &&
      data.parentOrigin === context.parentOrigin &&
      data.generation === state.generation
  );
}

function controlEnvelopeValid(event, context, data, { type, schema, keys }) {
  const state = context?.clipboardState;
  return Boolean(
    validContext(event, context) &&
      homeClipboardExactKeys(data, keys) &&
      data.type === type &&
      data.schema === schema &&
      data.homeToken === context.homeToken &&
      data.parentOrigin === context.parentOrigin &&
      data.generation === state.generation &&
      (!Object.hasOwn(data, "requestId") || boundedRequestId(data.requestId))
  );
}

function randomGeneration(cryptoRef = globalThis.crypto) {
  if (typeof cryptoRef?.randomUUID === "function") {
    const value = `clipboard:${cryptoRef.randomUUID()}`;
    if (boundedGeneration(value)) {
      return value;
    }
  }
  if (typeof cryptoRef?.getRandomValues === "function") {
    const bytes = new Uint8Array(16);
    cryptoRef.getRandomValues(bytes);
    return `clipboard:${Array.from(
      bytes,
      (byte) => byte.toString(16).padStart(2, "0"),
    ).join("")}`;
  }
  throw new Error("Home Clipboard requires browser crypto");
}

export function createHomeClipboardFrameState() {
  return {
    generation: "",
    homeToken: "",
    inFlight: null,
    parentOrigin: "",
    replayIds: new Map(),
    retired: true,
    source: null,
    targetId: "",
  };
}

export function createHomeClipboardPrompt({
  root,
  title,
  copy,
  allowButton,
  cancelButton,
} = {}) {
  if (!root || !title || !copy || !allowButton || !cancelButton) {
    throw new TypeError("Home Clipboard prompt nodes are required");
  }
  let active = null;

  function hide() {
    root.hidden = true;
    root.setAttribute("aria-hidden", "true");
    title.textContent = "Clipboard request";
    copy.textContent = "";
    allowButton.textContent = "Continue";
  }

  function settle(allowed) {
    const request = active;
    if (!request) {
      return;
    }
    active = null;
    hide();
    request.resolve(allowed);
  }

  allowButton.addEventListener("click", () => settle(true));
  cancelButton.addEventListener("click", () => settle(false));

  const promptCopy = {
    "browser:browser.text:read": [
      "Paste into Browser?",
      "Continue to read plain text from this device clipboard and paste it into Browser.",
      "Paste",
    ],
    "browser:browser.text:write": [
      "Copy from Browser?",
      "Continue to copy the Browser-selected plain text to this device clipboard.",
      "Copy",
    ],
    "wallet:wallet.address:write": [
      "Copy Wallet address?",
      "Continue to copy this Wallet address to this device clipboard.",
      "Copy",
    ],
    "wallet:wallet.recovery-key:write": [
      "Copy Wallet Recovery Key?",
      "This is secret material. Continue only if you can store the Wallet Recovery Key privately and offline.",
      "Copy secret",
    ],
    "wallet-metamask:wallet.address:write": [
      "Copy linked Wallet address?",
      "Continue to copy this linked Wallet address to this device clipboard.",
      "Copy",
    ],
    "wallet-unisat:wallet.address:write": [
      "Copy linked Wallet address?",
      "Continue to copy this linked Wallet address to this device clipboard.",
      "Copy",
    ],
    "wallet-walletconnect:wallet.address:write": [
      "Copy linked Wallet address?",
      "Continue to copy this linked Wallet address to this device clipboard.",
      "Copy",
    ],
    "library:resource.uri:write": [
      "Copy Library resource link?",
      "Continue to copy this resource URI to this device clipboard.",
      "Copy",
    ],
    "library:resource.identifier:write": [
      "Copy Library identifier?",
      "Continue to copy this bounded technical identifier to this device clipboard.",
      "Copy identifier",
    ],
    "documents:resource.uri:write": [
      "Copy Documents resource link?",
      "Continue to copy this resource URI to this device clipboard.",
      "Copy",
    ],
  };

  return {
    request({ requestId, targetId, operation, purpose }) {
      if (active) {
        const error = new Error("Home already has an active Clipboard prompt");
        error.code = "busy";
        return Promise.reject(error);
      }
      const promptKey = `${targetId}:${purpose}:${operation}`;
      const presentation = Object.hasOwn(promptCopy, promptKey)
        ? promptCopy[promptKey]
        : null;
      if (!presentation) {
        const error = new Error("Home denied the Clipboard purpose");
        error.code = "denied";
        return Promise.reject(error);
      }
      root.hidden = false;
      root.setAttribute("aria-hidden", "false");
      [title.textContent, copy.textContent, allowButton.textContent] = presentation;
      allowButton.focus({ preventScroll: true });
      return new Promise((resolve, reject) => {
        active = { requestId, resolve, reject };
      });
    },
    cancel(requestId, code = "cancelled") {
      if (!active || active.requestId !== requestId) {
        return false;
      }
      const request = active;
      active = null;
      hide();
      const error = new Error("Home Clipboard prompt was cancelled");
      error.code = code;
      request.reject(error);
      return true;
    },
  };
}

export function createHomeClipboardHost({
  clipboard = globalThis.navigator?.clipboard,
  prompt,
  cryptoRef = globalThis.crypto,
  now = () => Date.now(),
  setTimeoutFn = globalThis.setTimeout?.bind(globalThis),
  clearTimeoutFn = globalThis.clearTimeout?.bind(globalThis),
  timeoutMs = HOME_CLIPBOARD_TIMEOUT_MS,
  replayTtlMs = HOME_CLIPBOARD_REPLAY_TTL_MS,
  maxReplayIds = MAX_HOME_CLIPBOARD_REPLAY_IDS,
} = {}) {
  if (
    !prompt ||
    typeof prompt.request !== "function" ||
    typeof prompt.cancel !== "function"
  ) {
    throw new TypeError("Home Clipboard prompt is required");
  }
  if (
    typeof setTimeoutFn !== "function" ||
    typeof clearTimeoutFn !== "function"
  ) {
    throw new TypeError("Home Clipboard timers are required");
  }

  function pruneReplayIds(state) {
    const cutoff = now();
    for (const [requestId, expiresAt] of state.replayIds) {
      if (expiresAt <= cutoff) {
        state.replayIds.delete(requestId);
      }
    }
  }

  function rememberRequestId(state, requestId) {
    pruneReplayIds(state);
    while (state.replayIds.size >= maxReplayIds) {
      state.replayIds.delete(state.replayIds.keys().next().value);
    }
    state.replayIds.set(requestId, now() + replayTtlMs);
  }

  function reply(source, payload) {
    try {
      source?.postMessage(payload, OPAQUE_FRAME_TARGET);
    } catch (_error) {
      // The exact requester may have retired before its bounded result arrived.
    }
  }

  function readyPayload(state) {
    return {
      type: HOME_CLIPBOARD_READY_TYPE,
      schema: HOME_CLIPBOARD_READY_SCHEMA,
      targetId: state.targetId,
      homeToken: state.homeToken,
      parentOrigin: state.parentOrigin,
      generation: state.generation,
    };
  }

  function resultPayload(record, result) {
    const common = {
      type: HOME_CLIPBOARD_RESULT_TYPE,
      schema: HOME_CLIPBOARD_RESULT_SCHEMA,
      requestId: record.requestId,
      targetId: record.targetId,
      homeToken: record.homeToken,
      parentOrigin: record.parentOrigin,
      generation: record.generation,
      operation: record.operation,
      purpose: record.purpose,
      ok: result.ok === true,
    };
    if (result.ok === true && record.operation === "read") {
      return {
        ...common,
        mime_type: HOME_CLIPBOARD_MIME_TYPE,
        text: result.text,
      };
    }
    if (result.ok === true) {
      return common;
    }
    return {
      ...common,
      error: homeClipboardResultErrorSupported(result.error)
        ? result.error
        : "unavailable",
    };
  }

  function clearRecord(record, result, { sendReply = true } = {}) {
    if (!record || record.settled) {
      return false;
    }
    record.settled = true;
    clearTimeoutFn(record.timeoutId);
    prompt.cancel(record.requestId, result.error || "cancelled");
    if (record.state.inFlight === record) {
      record.state.inFlight = null;
    }
    if (sendReply) {
      reply(record.source, resultPayload(record, result));
    }
    record.source = null;
    record.state = null;
    record.requestId = "";
    record.homeToken = "";
    return true;
  }

  function errorCode(error) {
    return homeClipboardResultErrorSupported(error?.code)
      ? error.code
      : "unavailable";
  }

  async function perform(record, data) {
    let clipboardText = data.operation === "write" ? data.text : "";
    try {
      const allowed = await prompt.request({
        requestId: record.requestId,
        targetId: record.targetId,
        operation: record.operation,
        purpose: record.purpose,
      });
      if (!allowed) {
        clearRecord(record, { ok: false, error: "cancelled" });
        return;
      }
      if (record.operation === "read") {
        if (typeof clipboard?.readText !== "function") {
          clearRecord(record, { ok: false, error: "unavailable" });
          return;
        }
        clipboardText = await clipboard.readText();
        if (
          !homeClipboardValidWriteText(
            record.targetId,
            record.purpose,
            clipboardText,
          )
        ) {
          clearRecord(record, { ok: false, error: "unavailable" });
          return;
        }
        clearRecord(record, { ok: true, text: clipboardText });
        return;
      }
      if (typeof clipboard?.writeText !== "function") {
        clearRecord(record, { ok: false, error: "unavailable" });
        return;
      }
      await clipboard.writeText(clipboardText);
      clearRecord(record, { ok: true });
    } catch (error) {
      clearRecord(record, { ok: false, error: errorCode(error) });
    } finally {
      clipboardText = "";
    }
  }

  function replyRequestError(event, context, data, error) {
    if (
      !validContext(event, context) ||
      !data ||
      typeof data !== "object" ||
      Array.isArray(data) ||
      !Object.hasOwn(data, "requestId") ||
      !boundedRequestId(data.requestId)
    ) {
      return;
    }
    const operation =
      Object.hasOwn(data, "operation") &&
      (data.operation === "read" || data.operation === "write")
        ? data.operation
        : "write";
    const purpose =
      Object.hasOwn(data, "purpose") &&
      typeof data.purpose === "string" &&
      data.purpose.length <= MAX_HOME_CLIPBOARD_PURPOSE_CODE_UNITS &&
      homeClipboardOperationAllowed(
        context.targetId,
        data.purpose,
        operation,
      )
        ? data.purpose
        : "invalid";
    reply(event.source, {
      type: HOME_CLIPBOARD_RESULT_TYPE,
      schema: HOME_CLIPBOARD_RESULT_SCHEMA,
      requestId: data.requestId,
      targetId: context.targetId,
      homeToken: context.homeToken,
      parentOrigin: context.parentOrigin,
      generation: context.clipboardState.generation,
      operation,
      purpose,
      ok: false,
      error,
    });
  }

  function handleRequest(event, context, data) {
    if (!requestEnvelopeValid(event, context, data)) {
      replyRequestError(event, context, data, "malformed");
      return true;
    }
    const state = context.clipboardState;
    pruneReplayIds(state);
    if (state.replayIds.has(data.requestId)) {
      replyRequestError(event, context, data, "replay");
      return true;
    }
    rememberRequestId(state, data.requestId);
    if (state.inFlight) {
      replyRequestError(event, context, data, "busy");
      return true;
    }
    const record = {
      generation: state.generation,
      homeToken: context.homeToken,
      operation: data.operation,
      parentOrigin: context.parentOrigin,
      purpose: data.purpose,
      requestId: data.requestId,
      settled: false,
      source: event.source,
      state,
      targetId: context.targetId,
      timeoutId: 0,
    };
    state.inFlight = record;
    record.timeoutId = setTimeoutFn(() => {
      clearRecord(record, { ok: false, error: "timeout" });
    }, timeoutMs);
    void perform(record, data);
    return true;
  }

  function handleCancel(event, context, data) {
    if (
      !controlEnvelopeValid(event, context, data, {
        type: HOME_CLIPBOARD_CANCEL_TYPE,
        schema: HOME_CLIPBOARD_CANCEL_SCHEMA,
        keys: [
          "type",
          "schema",
          "requestId",
          "homeToken",
          "parentOrigin",
          "generation",
        ],
      })
    ) {
      return true;
    }
    const record = context.clipboardState.inFlight;
    if (record?.requestId === data.requestId) {
      clearRecord(record, { ok: false, error: "cancelled" });
    }
    return true;
  }

  function retireFrame(state) {
    if (!state) {
      return;
    }
    state.retired = true;
    state.source = null;
    state.replayIds.clear();
    if (state.inFlight) {
      clearRecord(
        state.inFlight,
        { ok: false, error: "retired" },
        { sendReply: false },
      );
    }
    state.generation = "";
    state.homeToken = "";
    state.parentOrigin = "";
    state.targetId = "";
  }

  function resetFrame(state, context) {
    if (
      !state ||
      !context?.source ||
      !context.homeToken ||
      !context.parentOrigin ||
      !homeClipboardTargetSupported(context.targetId)
    ) {
      return false;
    }
    if (
      !state.retired &&
      state.source === context.source &&
      state.homeToken === context.homeToken &&
      state.parentOrigin === context.parentOrigin &&
      state.targetId === context.targetId
    ) {
      reply(state.source, readyPayload(state));
      return true;
    }
    retireFrame(state);
    state.retired = false;
    state.source = context.source;
    state.homeToken = context.homeToken;
    state.parentOrigin = context.parentOrigin;
    state.targetId = context.targetId;
    state.generation = randomGeneration(cryptoRef);
    reply(state.source, readyPayload(state));
    return true;
  }

  function handleRetire(event, context, data) {
    if (
      !controlEnvelopeValid(event, context, data, {
        type: HOME_CLIPBOARD_RETIRE_TYPE,
        schema: HOME_CLIPBOARD_RETIRE_SCHEMA,
        keys: [
          "type",
          "schema",
          "homeToken",
          "parentOrigin",
          "generation",
        ],
      })
    ) {
      return true;
    }
    retireFrame(context.clipboardState);
    return true;
  }

  function handle(event, context, data) {
    if (
      !data ||
      typeof data !== "object" ||
      Array.isArray(data) ||
      !Object.hasOwn(data, "type")
    ) {
      return false;
    }
    if (data.type === HOME_CLIPBOARD_REQUEST_TYPE) {
      return handleRequest(event, context, data);
    }
    if (data.type === HOME_CLIPBOARD_CANCEL_TYPE) {
      return handleCancel(event, context, data);
    }
    if (data.type === HOME_CLIPBOARD_RETIRE_TYPE) {
      return handleRetire(event, context, data);
    }
    return false;
  }

  return {
    handle,
    resetFrame,
    retireFrame,
  };
}
