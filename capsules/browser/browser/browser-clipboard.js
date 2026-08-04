export const MAX_CLIPBOARD_TEXT_UTF8_BYTES = 65_536;
export const MAX_CLIPBOARD_ENCODED_BYTES =
  Math.ceil(MAX_CLIPBOARD_TEXT_UTF8_BYTES / 3) * 4;
export const MAX_CLIPBOARD_ENCODED_CHUNK_BYTES =
  MAX_CLIPBOARD_TEXT_UTF8_BYTES;
export const MAX_CLIPBOARD_CHUNK_COUNT = 128;
export const CLIPBOARD_ASSEMBLY_TIMEOUT_MS = 5_000;
export const CLIPBOARD_COPY_INTENT_TIMEOUT_MS = 5_000;

const CLIPBOARD_TEXT_LIMIT_LABEL = "65,536";
const BASE64_PATTERN =
  /^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/;
const BASE64_CHUNK_PATTERN = /^[A-Za-z0-9+/=]+$/;

export function clipboardTextUtf8Bytes(
  text,
  TextEncoderConstructor = globalThis.TextEncoder,
) {
  if (typeof text !== "string") {
    throw new TypeError("Clipboard text must be a string.");
  }
  return new TextEncoderConstructor().encode(text).byteLength;
}

export function assertBoundedClipboardText(text) {
  const byteLength = clipboardTextUtf8Bytes(text);
  if (byteLength > MAX_CLIPBOARD_TEXT_UTF8_BYTES) {
    throw new Error(
      `Clipboard text exceeds ${CLIPBOARD_TEXT_LIMIT_LABEL} UTF-8 bytes.`,
    );
  }
  return text;
}

export function decodeClipboardBase64Text(
  content,
  {
    atobFn = globalThis.atob,
    btoaFn = globalThis.btoa,
    TextDecoderConstructor = globalThis.TextDecoder,
  } = {},
) {
  if (
    typeof content !== "string" ||
    !content ||
    content.length > MAX_CLIPBOARD_ENCODED_BYTES ||
    !BASE64_PATTERN.test(content)
  ) {
    throw new Error("Clipboard content is not canonical bounded base64.");
  }

  const binary = atobFn(content);
  if (btoaFn(binary) !== content) {
    throw new Error("Clipboard content is not canonical bounded base64.");
  }
  if (binary.length > MAX_CLIPBOARD_TEXT_UTF8_BYTES) {
    throw new Error(
      `Clipboard text exceeds ${CLIPBOARD_TEXT_LIMIT_LABEL} UTF-8 bytes.`,
    );
  }

  const bytes = Uint8Array.from(binary, (character) =>
    character.charCodeAt(0),
  );
  return new TextDecoderConstructor("utf-8", { fatal: true }).decode(bytes);
}

export function createBrowserClipboardBridge({
  friendlyOpenError,
  getCurrentPage,
  sendBrowserInput,
  showStatus,
  writeHostClipboardTextFn,
  createClipboardRequestIdFn,
  cancelHostClipboardRequestFn = () => false,
  decodeClipboardTextFn = decodeClipboardBase64Text,
  setTimeoutFn = globalThis.setTimeout?.bind(globalThis),
  clearTimeoutFn = globalThis.clearTimeout?.bind(globalThis),
}) {
  if (
    typeof writeHostClipboardTextFn !== "function" ||
    typeof createClipboardRequestIdFn !== "function"
  ) {
    throw new TypeError("Trusted Home Clipboard functions are required");
  }
  let remoteClipboardChunks = null;
  let pendingRemoteCopy = null;

  function resetRemoteClipboardAssembly() {
    if (
      remoteClipboardChunks?.timeoutId != null &&
      typeof clearTimeoutFn === "function"
    ) {
      clearTimeoutFn(remoteClipboardChunks.timeoutId);
    }
    remoteClipboardChunks = null;
  }

  function resetRemoteCopyIntent({ cancelHost = false } = {}) {
    resetRemoteClipboardAssembly();
    if (
      pendingRemoteCopy?.timeoutId != null &&
      typeof clearTimeoutFn === "function"
    ) {
      clearTimeoutFn(pendingRemoteCopy.timeoutId);
    }
    if (cancelHost && pendingRemoteCopy?.hostPending) {
      cancelHostClipboardRequestFn(pendingRemoteCopy.requestId);
    }
    pendingRemoteCopy = null;
  }

  function armRemoteClipboardTimeout() {
    if (
      !remoteClipboardChunks ||
      typeof setTimeoutFn !== "function"
    ) {
      return;
    }
    if (
      remoteClipboardChunks.timeoutId != null &&
      typeof clearTimeoutFn === "function"
    ) {
      clearTimeoutFn(remoteClipboardChunks.timeoutId);
    }
    const assembly = remoteClipboardChunks;
    assembly.timeoutId = setTimeoutFn(() => {
      if (remoteClipboardChunks === assembly) {
        resetRemoteClipboardAssembly();
      }
    }, CLIPBOARD_ASSEMBLY_TIMEOUT_MS);
  }

  async function writeHostClipboardText(text) {
    assertBoundedClipboardText(text);
    const intent = pendingRemoteCopy;
    if (!intent || !intent.remoteReadRequested || intent.hostPending) {
      return false;
    }
    if (getCurrentPage()?.page_id !== intent.pageId) {
      resetRemoteCopyIntent();
      return false;
    }
    if (
      intent.timeoutId != null &&
      typeof clearTimeoutFn === "function"
    ) {
      clearTimeoutFn(intent.timeoutId);
      intent.timeoutId = null;
    }
    intent.hostPending = true;
    try {
      await writeHostClipboardTextFn(text, {
        requestId: intent.requestId,
      });
      if (pendingRemoteCopy !== intent) {
        return false;
      }
      showStatus("Copied from Browser.", { sticky: false });
      return true;
    } catch {
      if (pendingRemoteCopy === intent) {
        showStatus("Clipboard copy was cancelled or unavailable.", {
          sticky: true,
        });
      }
      return false;
    } finally {
      if (pendingRemoteCopy === intent) {
        resetRemoteCopyIntent();
      }
    }
  }

  async function consumeEncodedClipboard(content) {
    if (
      !pendingRemoteCopy ||
      !pendingRemoteCopy.remoteReadRequested ||
      pendingRemoteCopy.hostPending
    ) {
      return false;
    }
    let text = "";
    try {
      text = decodeClipboardTextFn(content);
      assertBoundedClipboardText(text);
    } catch {
      return false;
    }
    if (!text) {
      return false;
    }
    return writeHostClipboardText(text);
  }

  function startRemoteClipboardAssembly(message) {
    resetRemoteClipboardAssembly();
    if (
      !pendingRemoteCopy ||
      !pendingRemoteCopy.remoteReadRequested ||
      pendingRemoteCopy.hostPending ||
      !message.data ||
      typeof message.data !== "object" ||
      Array.isArray(message.data) ||
      message.data.mime_type !== "text/plain"
    ) {
      return;
    }
    remoteClipboardChunks = {
      chunks: [],
      chunkCount: 0,
      encodedBytes: 0,
      timeoutId: null,
    };
    armRemoteClipboardTimeout();
  }

  function appendRemoteClipboardChunk(message) {
    if (
      !pendingRemoteCopy?.remoteReadRequested ||
      !remoteClipboardChunks
    ) {
      resetRemoteClipboardAssembly();
      return;
    }
    const content = message.data?.content;
    if (
      typeof content !== "string" ||
      !content ||
      content.length > MAX_CLIPBOARD_ENCODED_CHUNK_BYTES ||
      !BASE64_CHUNK_PATTERN.test(content)
    ) {
      resetRemoteClipboardAssembly();
      return;
    }

    const chunkCount = remoteClipboardChunks.chunkCount + 1;
    const encodedBytes = remoteClipboardChunks.encodedBytes + content.length;
    if (
      chunkCount > MAX_CLIPBOARD_CHUNK_COUNT ||
      encodedBytes > MAX_CLIPBOARD_ENCODED_BYTES
    ) {
      resetRemoteClipboardAssembly();
      return;
    }

    remoteClipboardChunks.chunks.push(content);
    remoteClipboardChunks.chunkCount = chunkCount;
    remoteClipboardChunks.encodedBytes = encodedBytes;
    armRemoteClipboardTimeout();
  }

  function completeRemoteClipboardAssembly(message) {
    if (
      !pendingRemoteCopy?.remoteReadRequested ||
      !remoteClipboardChunks
    ) {
      resetRemoteClipboardAssembly();
      return;
    }
    if (
      message.data != null &&
      (typeof message.data !== "object" || Array.isArray(message.data))
    ) {
      resetRemoteClipboardAssembly();
      return;
    }
    const content = remoteClipboardChunks.chunks.join("");
    resetRemoteClipboardAssembly();
    if (!content) {
      return;
    }
    return consumeEncodedClipboard(content);
  }

  function handleSelkiesClipboardMessage(message) {
    if (
      !message ||
      typeof message !== "object" ||
      Array.isArray(message) ||
      typeof message.type !== "string"
    ) {
      resetRemoteClipboardAssembly();
      return;
    }

    if (message.type === "clipboard-msg") {
      resetRemoteClipboardAssembly();
      const content = message.data?.content;
      if (
        !pendingRemoteCopy ||
        !pendingRemoteCopy.remoteReadRequested ||
        pendingRemoteCopy.hostPending ||
        !message.data ||
        typeof message.data !== "object" ||
        Array.isArray(message.data) ||
        message.data.mime_type !== "text/plain" ||
        typeof content !== "string" ||
        !content ||
        content.length > MAX_CLIPBOARD_ENCODED_BYTES
      ) {
        return;
      }
      return consumeEncodedClipboard(content);
    }
    if (message.type === "clipboard-msg-start") {
      startRemoteClipboardAssembly(message);
      return;
    }
    if (message.type === "clipboard-msg-data") {
      appendRemoteClipboardChunk(message);
      return;
    }
    if (message.type === "clipboard-msg-end") {
      return completeRemoteClipboardAssembly(message);
    }
    if (message.type.startsWith("clipboard-")) {
      resetRemoteClipboardAssembly();
    }
  }

  function handleRemoteInputChannelMessage(event) {
    if (typeof event?.data !== "string") {
      resetRemoteClipboardAssembly();
      return;
    }
    let message = null;
    try {
      message = JSON.parse(event.data);
    } catch {
      resetRemoteClipboardAssembly();
      return;
    }
    return handleSelkiesClipboardMessage(message);
  }

  async function pasteHostClipboardIntoRemote(text) {
    if (!getCurrentPage() || !text) {
      return;
    }
    assertBoundedClipboardText(text);
    await sendBrowserInput(
      { type: "paste_text", text },
      { history: "replace" },
    );
  }

  async function copyRemoteClipboardToHost() {
    const page = getCurrentPage();
    if (!page?.page_id) {
      return;
    }
    if (pendingRemoteCopy) {
      throw new Error("A Browser copy is already pending.");
    }
    const intent = {
      hostPending: false,
      pageId: page.page_id,
      remoteReadRequested: false,
      requestId: createClipboardRequestIdFn(),
      timeoutId: null,
    };
    pendingRemoteCopy = intent;
    intent.timeoutId = setTimeoutFn(() => {
      if (pendingRemoteCopy === intent) {
        resetRemoteCopyIntent({ cancelHost: true });
        showStatus("Browser copy timed out.", { sticky: true });
      }
    }, CLIPBOARD_COPY_INTENT_TIMEOUT_MS);
    try {
      await sendBrowserInput(
        { type: "key_combo", keysyms: [65507, 99] },
        { history: "replace" },
      );
      setTimeoutFn(() => {
        if (pendingRemoteCopy !== intent || intent.hostPending) {
          return;
        }
        intent.remoteReadRequested = true;
        sendBrowserInput(
          { type: "clipboard_read" },
          { focus: false, history: "replace" },
        ).catch((error) => {
          if (pendingRemoteCopy === intent) {
            resetRemoteCopyIntent();
            showStatus(friendlyOpenError(error), { sticky: true });
          }
        });
      }, 150);
    } catch (error) {
      if (pendingRemoteCopy === intent) {
        resetRemoteCopyIntent();
      }
      throw error;
    }
  }

  return {
    copyRemoteClipboardToHost,
    handleRemoteInputChannelMessage,
    pasteHostClipboardIntoRemote,
    teardownRemoteClipboard: () =>
      resetRemoteCopyIntent({ cancelHost: true }),
  };
}
