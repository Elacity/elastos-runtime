export const HOME_CLIPBOARD_READY_TYPE = "home:clipboard-ready";
export const HOME_CLIPBOARD_READY_SCHEMA =
  "elastos.home.clipboard.ready/v1";
export const HOME_CLIPBOARD_REQUEST_TYPE = "home:clipboard-request";
export const HOME_CLIPBOARD_REQUEST_SCHEMA =
  "elastos.home.clipboard.request/v1";
export const HOME_CLIPBOARD_RESULT_TYPE = "home:clipboard-result";
export const HOME_CLIPBOARD_RESULT_SCHEMA =
  "elastos.home.clipboard.result/v1";
export const HOME_CLIPBOARD_CANCEL_TYPE = "home:clipboard-cancel";
export const HOME_CLIPBOARD_CANCEL_SCHEMA =
  "elastos.home.clipboard.cancel/v1";
export const HOME_CLIPBOARD_RETIRE_TYPE = "home:clipboard-retire";
export const HOME_CLIPBOARD_RETIRE_SCHEMA =
  "elastos.home.clipboard.retire/v1";
export const HOME_CLIPBOARD_MIME_TYPE = "text/plain";
export const MAX_HOME_CLIPBOARD_REQUEST_ID_BYTES = 128;
export const MAX_HOME_CLIPBOARD_GENERATION_BYTES = 128;
export const MAX_HOME_CLIPBOARD_TARGET_ID_CODE_UNITS = 64;
export const MAX_HOME_CLIPBOARD_PURPOSE_CODE_UNITS = 64;
export const MAX_HOME_CLIPBOARD_TEXT_UTF8_BYTES = 65_536;
export const MAX_HOME_CLIPBOARD_IDENTIFIER_UTF8_BYTES = 2_048;

const TOKEN_PATTERN = /^[A-Za-z0-9:_-]+$/;
const RESOURCE_URI_PATTERN =
  /^[A-Za-z][A-Za-z0-9+.-]{0,31}:[^\u0000-\u0020\u007f]+$/;
const RESOURCE_IDENTIFIER_PATTERN =
  /^[A-Za-z0-9][A-Za-z0-9._~:/?#@!$&'()*+,;=%\[\]-]*$/u;

const HOME_CLIPBOARD_RESULT_ERRORS = new Set([
  "busy",
  "cancelled",
  "denied",
  "malformed",
  "replay",
  "retired",
  "timeout",
  "unavailable",
]);

const HOME_CLIPBOARD_TARGET_PURPOSE_POLICY = Object.freeze({
  browser: Object.freeze({
    "browser.text": Object.freeze({
      operations: Object.freeze(["read", "write"]),
      maxUtf8Bytes: MAX_HOME_CLIPBOARD_TEXT_UTF8_BYTES,
    }),
  }),
  wallet: Object.freeze({
    "wallet.address": Object.freeze({
      operations: Object.freeze(["write"]),
      maxUtf8Bytes: 256,
      kind: "address",
    }),
    "wallet.recovery-key": Object.freeze({
      operations: Object.freeze(["write"]),
      maxUtf8Bytes: MAX_HOME_CLIPBOARD_TEXT_UTF8_BYTES,
    }),
  }),
  "wallet-metamask": Object.freeze({
    "wallet.address": Object.freeze({
      operations: Object.freeze(["write"]),
      maxUtf8Bytes: 256,
      kind: "address",
    }),
  }),
  "wallet-unisat": Object.freeze({
    "wallet.address": Object.freeze({
      operations: Object.freeze(["write"]),
      maxUtf8Bytes: 256,
      kind: "address",
    }),
  }),
  "wallet-walletconnect": Object.freeze({
    "wallet.address": Object.freeze({
      operations: Object.freeze(["write"]),
      maxUtf8Bytes: 256,
      kind: "address",
    }),
  }),
  library: Object.freeze({
    "resource.identifier": Object.freeze({
      operations: Object.freeze(["write"]),
      maxUtf8Bytes: MAX_HOME_CLIPBOARD_IDENTIFIER_UTF8_BYTES,
      kind: "resource-identifier",
    }),
    "resource.uri": Object.freeze({
      operations: Object.freeze(["write"]),
      maxUtf8Bytes: 16_384,
      kind: "resource-uri",
    }),
  }),
  documents: Object.freeze({
    "resource.uri": Object.freeze({
      operations: Object.freeze(["write"]),
      maxUtf8Bytes: 16_384,
      kind: "resource-uri",
    }),
  }),
  "chat-room": Object.freeze({
    "conversation.invite": Object.freeze({
      operations: Object.freeze(["write"]),
      maxUtf8Bytes: 16_384,
      kind: "resource-uri",
    }),
  }),
  system: Object.freeze({
    "identity.did": Object.freeze({
      operations: Object.freeze(["write"]),
      maxUtf8Bytes: MAX_HOME_CLIPBOARD_IDENTIFIER_UTF8_BYTES,
    }),
  }),
});

export function homeClipboardExactKeys(value, expected) {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    return false;
  }
  const actual = Object.keys(value).sort();
  const sortedExpected = [...expected].sort();
  return (
    actual.length === sortedExpected.length &&
    actual.every((key, index) => key === sortedExpected[index])
  );
}

export function homeClipboardValidToken(value, maxBytes) {
  return (
    typeof value === "string" &&
    value.length > 0 &&
    value.length <= maxBytes &&
    TOKEN_PATTERN.test(value)
  );
}

function utf8Bytes(value, TextEncoderConstructor = globalThis.TextEncoder) {
  return typeof value === "string"
    ? new TextEncoderConstructor().encode(value).byteLength
    : Number.POSITIVE_INFINITY;
}

export function homeClipboardPolicyFor(targetId, purpose) {
  if (
    typeof targetId !== "string" ||
    targetId.length === 0 ||
    targetId.length > MAX_HOME_CLIPBOARD_TARGET_ID_CODE_UNITS ||
    typeof purpose !== "string" ||
    purpose.length === 0 ||
    purpose.length > MAX_HOME_CLIPBOARD_PURPOSE_CODE_UNITS ||
    !Object.hasOwn(HOME_CLIPBOARD_TARGET_PURPOSE_POLICY, targetId)
  ) {
    return null;
  }
  const targetPolicy = HOME_CLIPBOARD_TARGET_PURPOSE_POLICY[targetId];
  return Object.hasOwn(targetPolicy, purpose) ? targetPolicy[purpose] : null;
}

export function homeClipboardOperationAllowed(targetId, purpose, operation) {
  return (
    homeClipboardPolicyFor(targetId, purpose)?.operations.includes(operation) ===
    true
  );
}

export function homeClipboardValidWriteText(
  targetId,
  purpose,
  text,
  TextEncoderConstructor = globalThis.TextEncoder,
) {
  const policy = homeClipboardPolicyFor(targetId, purpose);
  if (
    !policy ||
    !policy.operations.includes("write") ||
    typeof text !== "string" ||
    text.length > policy.maxUtf8Bytes ||
    utf8Bytes(text, TextEncoderConstructor) > policy.maxUtf8Bytes
  ) {
    return false;
  }
  if (policy.kind === "address") {
    return (
      text.length > 0 &&
      text === text.trim() &&
      !/[\s\u0000-\u001f\u007f]/u.test(text)
    );
  }
  if (policy.kind === "resource-uri") {
    return (
      text.length > 0 &&
      text === text.trim() &&
      RESOURCE_URI_PATTERN.test(text)
    );
  }
  if (policy.kind === "resource-identifier") {
    return (
      text.length > 0 &&
      text === text.trim() &&
      RESOURCE_IDENTIFIER_PATTERN.test(text)
    );
  }
  return true;
}

export function homeClipboardValidPayload(
  targetId,
  purpose,
  operation,
  text,
) {
  return Boolean(
    homeClipboardOperationAllowed(targetId, purpose, operation) &&
      (operation !== "write" ||
        homeClipboardValidWriteText(targetId, purpose, text)),
  );
}

export function homeClipboardTargetSupported(targetId) {
  return (
    typeof targetId === "string" &&
    targetId.length > 0 &&
    targetId.length <= MAX_HOME_CLIPBOARD_TARGET_ID_CODE_UNITS &&
    Object.hasOwn(HOME_CLIPBOARD_TARGET_PURPOSE_POLICY, targetId)
  );
}

export function homeClipboardResultErrorSupported(error) {
  return HOME_CLIPBOARD_RESULT_ERRORS.has(error);
}
