export function createBrowserClipboardBridge({
  friendlyOpenError,
  getCurrentPage,
  sendBrowserInput,
  showStatus,
  utf8FromBase64,
}) {
  let remoteClipboardChunks = null;

  async function writeHostClipboardText(text) {
    if (!text || !navigator.clipboard?.writeText) {
      return;
    }
    await navigator.clipboard.writeText(text);
    showStatus("Copied from Browser.", { sticky: false });
  }

  function clipboardTextFromData(data) {
    if (!data || data.mime_type !== "text/plain" || typeof data.content !== "string") {
      return "";
    }
    try {
      return utf8FromBase64(data.content);
    } catch {
      return "";
    }
  }

  function handleSelkiesClipboardMessage(message) {
    if (!message || typeof message !== "object") {
      return;
    }
    if (message.type === "clipboard-msg") {
      const text = clipboardTextFromData(message.data);
      if (text) {
        writeHostClipboardText(text).catch((error) => {
          showStatus("Clipboard access is unavailable.", { sticky: true });
        });
      }
      return;
    }
    if (message.type === "clipboard-msg-start") {
      remoteClipboardChunks = {
        mimeType: message.data?.mime_type || "text/plain",
        chunks: [],
      };
      return;
    }
    if (message.type === "clipboard-msg-data" && remoteClipboardChunks) {
      const content = typeof message.data?.content === "string" ? message.data.content : "";
      if (content) {
        remoteClipboardChunks.chunks.push(content);
      }
      return;
    }
    if (message.type === "clipboard-msg-end" && remoteClipboardChunks) {
      const data = {
        mime_type: remoteClipboardChunks.mimeType,
        content: remoteClipboardChunks.chunks.join(""),
      };
      remoteClipboardChunks = null;
      const text = clipboardTextFromData(data);
      if (text) {
        writeHostClipboardText(text).catch((error) => {
          showStatus("Clipboard access is unavailable.", { sticky: true });
        });
      }
    }
  }

  function handleRemoteInputChannelMessage(event) {
    if (typeof event.data !== "string") {
      return;
    }
    let message = null;
    try {
      message = JSON.parse(event.data);
    } catch {
      return;
    }
    handleSelkiesClipboardMessage(message);
  }

  async function pasteHostClipboardIntoRemote(text) {
    if (!getCurrentPage() || !text) {
      return;
    }
    await sendBrowserInput({ type: "paste_text", text }, { history: "replace" });
  }

  async function copyRemoteClipboardToHost() {
    if (!getCurrentPage()) {
      return;
    }
    await sendBrowserInput({ type: "key_combo", keysyms: [65507, 99] }, { history: "replace" });
    window.setTimeout(() => {
      sendBrowserInput({ type: "clipboard_read" }, { focus: false, history: "replace" }).catch(
        (error) => {
          showStatus(friendlyOpenError(error), { sticky: true });
        },
      );
    }, 150);
  }

  return {
    copyRemoteClipboardToHost,
    handleRemoteInputChannelMessage,
    pasteHostClipboardIntoRemote,
  };
}
