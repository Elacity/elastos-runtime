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
  MAX_HOME_CLIPBOARD_REQUEST_ID_BYTES,
  MAX_HOME_CLIPBOARD_TEXT_UTF8_BYTES,
  homeClipboardExactKeys,
  homeClipboardResultErrorSupported,
  homeClipboardTargetSupported,
  homeClipboardValidPayload,
  homeClipboardValidToken,
  homeClipboardValidWriteText,
} from "./home-clipboard-protocol.js?v=home-20260726a";

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
  MAX_HOME_CLIPBOARD_REQUEST_ID_BYTES,
  MAX_HOME_CLIPBOARD_TEXT_UTF8_BYTES,
};

export const HOME_CLIPBOARD_CLIENT_TIMEOUT_MS = 17_000;

export function homeClipboardRequestId(cryptoRef = globalThis.crypto) {
  if (typeof cryptoRef?.randomUUID === "function") {
    const requestId = cryptoRef.randomUUID();
    if (
      homeClipboardValidToken(requestId, MAX_HOME_CLIPBOARD_REQUEST_ID_BYTES)
    ) {
      return requestId;
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
  throw new Error("Clipboard requires browser crypto");
}

export function createHomeClipboardClient({
  targetId,
  homeOrigin,
  homeToken,
  targetWindow = globalThis.window?.top,
  sourceWindow = globalThis.window,
  cryptoRef = globalThis.crypto,
  setTimeoutFn = globalThis.setTimeout?.bind(globalThis),
  clearTimeoutFn = globalThis.clearTimeout?.bind(globalThis),
  timeoutMs = HOME_CLIPBOARD_CLIENT_TIMEOUT_MS,
} = {}) {
  let generation = "";
  let pending = null;
  let retired = false;
  let started = false;

  function canReachHome() {
    return Boolean(
      !retired &&
        homeClipboardTargetSupported(targetId) &&
        homeOrigin &&
        homeToken &&
        targetWindow &&
        sourceWindow &&
        targetWindow !== sourceWindow &&
        typeof targetWindow.postMessage === "function",
    );
  }

  function available() {
    return (
      canReachHome() &&
      homeClipboardValidToken(
        generation,
        MAX_HOME_CLIPBOARD_GENERATION_BYTES,
      )
    );
  }

  function post(message) {
    if (!canReachHome()) {
      throw new Error("Trusted Home Clipboard is unavailable.");
    }
    targetWindow.postMessage(message, homeOrigin);
  }

  function clearPending(record, error = null, value = undefined) {
    if (!record || pending !== record) {
      return false;
    }
    pending = null;
    clearTimeoutFn(record.timeoutId);
    if (error) {
      record.reject(error);
    } else {
      record.resolve(value);
    }
    record.requestId = "";
    return true;
  }

  function invalidResult(record) {
    const error = new Error("Home returned an invalid Clipboard result.");
    error.code = "malformed";
    clearPending(record, error);
  }

  function handleReady(data) {
    if (
      !homeClipboardExactKeys(data, [
        "type",
        "schema",
        "targetId",
        "homeToken",
        "parentOrigin",
        "generation",
      ]) ||
      data.schema !== HOME_CLIPBOARD_READY_SCHEMA ||
      data.targetId !== targetId ||
      data.homeToken !== homeToken ||
      data.parentOrigin !== homeOrigin ||
      !homeClipboardValidToken(
        data.generation,
        MAX_HOME_CLIPBOARD_GENERATION_BYTES,
      )
    ) {
      return;
    }
    if (pending && generation && data.generation !== generation) {
      const error = new Error("Home Clipboard lifecycle changed.");
      error.code = "retired";
      clearPending(pending, error);
    }
    generation = data.generation;
  }

  function handleMessage(event) {
    if (event?.source !== targetWindow || event?.origin !== homeOrigin) {
      return false;
    }
    const data = event?.data;
    if (
      !data ||
      typeof data !== "object" ||
      Array.isArray(data) ||
      !Object.hasOwn(data, "type")
    ) {
      return false;
    }
    if (data.type === HOME_CLIPBOARD_READY_TYPE) {
      handleReady(data);
      return true;
    }
    if (data.type !== HOME_CLIPBOARD_RESULT_TYPE) {
      return false;
    }
    if (!pending || data.requestId !== pending.requestId) {
      return true;
    }
    const record = pending;
    const commonValid =
      data.schema === HOME_CLIPBOARD_RESULT_SCHEMA &&
      data.targetId === targetId &&
      data.homeToken === homeToken &&
      data.parentOrigin === homeOrigin &&
      data.generation === generation &&
      data.operation === record.operation &&
      data.purpose === record.purpose &&
      typeof data.ok === "boolean";
    if (!commonValid) {
      invalidResult(record);
      return true;
    }
    if (data.ok === false) {
      if (
        !homeClipboardExactKeys(data, [
          "type",
          "schema",
          "requestId",
          "targetId",
          "homeToken",
          "parentOrigin",
          "generation",
          "operation",
          "purpose",
          "ok",
          "error",
        ]) ||
        !homeClipboardResultErrorSupported(data.error)
      ) {
        invalidResult(record);
        return true;
      }
      const error = new Error(`Home Clipboard request failed: ${data.error}.`);
      error.code = data.error;
      clearPending(record, error);
      return true;
    }
    if (record.operation === "write") {
      if (
        !homeClipboardExactKeys(data, [
          "type",
          "schema",
          "requestId",
          "targetId",
          "homeToken",
          "parentOrigin",
          "generation",
          "operation",
          "purpose",
          "ok",
        ])
      ) {
        invalidResult(record);
        return true;
      }
      clearPending(record, null, true);
      return true;
    }
    if (
      !homeClipboardExactKeys(data, [
        "type",
        "schema",
        "requestId",
        "targetId",
        "homeToken",
        "parentOrigin",
        "generation",
        "operation",
        "purpose",
        "ok",
        "mime_type",
        "text",
      ]) ||
      data.mime_type !== HOME_CLIPBOARD_MIME_TYPE ||
      !homeClipboardValidWriteText(
        targetId,
        record.purpose,
        data.text,
      )
    ) {
      invalidResult(record);
      return true;
    }
    clearPending(record, null, data.text);
    return true;
  }

  function start() {
    if (started || !canReachHome()) {
      return false;
    }
    started = true;
    sourceWindow.addEventListener?.("message", handleMessage);
    sourceWindow.addEventListener?.("pagehide", teardown, { once: true });
    post({ type: "home:app-ready", homeToken });
    return true;
  }

  function request(operation, purpose, text, requestId) {
    if (!available()) {
      return Promise.reject(new Error("Trusted Home Clipboard is unavailable."));
    }
    if (pending) {
      return Promise.reject(new Error("A Clipboard request is already active."));
    }
    const resolvedRequestId = requestId || homeClipboardRequestId(cryptoRef);
    if (
      !homeClipboardValidToken(
        resolvedRequestId,
        MAX_HOME_CLIPBOARD_REQUEST_ID_BYTES,
      )
    ) {
      return Promise.reject(new Error("Clipboard request id is invalid."));
    }
    if (!homeClipboardValidPayload(targetId, purpose, operation, text)) {
      return Promise.reject(new Error("Clipboard operation or payload is denied."));
    }
    return new Promise((resolve, reject) => {
      const record = {
        operation,
        purpose,
        requestId: resolvedRequestId,
        resolve,
        reject,
        timeoutId: 0,
      };
      pending = record;
      record.timeoutId = setTimeoutFn(() => {
        const error = new Error("Home Clipboard request timed out.");
        error.code = "timeout";
        clearPending(record, error);
      }, timeoutMs);
      const message = {
        type: HOME_CLIPBOARD_REQUEST_TYPE,
        schema: HOME_CLIPBOARD_REQUEST_SCHEMA,
        requestId: resolvedRequestId,
        homeToken,
        parentOrigin: homeOrigin,
        generation,
        operation,
        purpose,
        mime_type: HOME_CLIPBOARD_MIME_TYPE,
      };
      if (operation === "write") {
        message.text = text;
      }
      try {
        post(message);
      } catch (error) {
        clearPending(record, error);
      }
    });
  }

  function cancel(requestId) {
    const record = pending;
    if (!record || record.requestId !== requestId) {
      return false;
    }
    try {
      post({
        type: HOME_CLIPBOARD_CANCEL_TYPE,
        schema: HOME_CLIPBOARD_CANCEL_SCHEMA,
        requestId,
        homeToken,
        parentOrigin: homeOrigin,
        generation,
      });
    } catch (_error) {}
    const error = new Error("Clipboard request was cancelled.");
    error.code = "cancelled";
    clearPending(record, error);
    return true;
  }

  function teardown() {
    if (retired) {
      return;
    }
    const record = pending;
    if (record) {
      const error = new Error("Clipboard frame retired.");
      error.code = "retired";
      clearPending(record, error);
    }
    if (available()) {
      try {
        post({
          type: HOME_CLIPBOARD_RETIRE_TYPE,
          schema: HOME_CLIPBOARD_RETIRE_SCHEMA,
          homeToken,
          parentOrigin: homeOrigin,
          generation,
        });
      } catch (_error) {}
    }
    retired = true;
    generation = "";
    sourceWindow.removeEventListener?.("message", handleMessage);
    sourceWindow.removeEventListener?.("pagehide", teardown);
  }

  return {
    canRequest: available,
    cancel,
    handleMessage,
    handleResult: handleMessage,
    newRequestId: () => homeClipboardRequestId(cryptoRef),
    readText: ({ purpose = "browser.text" } = {}) =>
      request("read", purpose),
    start,
    teardown,
    writeText: (
      text,
      {
        purpose = targetId === "browser" ? "browser.text" : undefined,
        requestId,
      } = {},
    ) => request("write", purpose, text, requestId),
  };
}
