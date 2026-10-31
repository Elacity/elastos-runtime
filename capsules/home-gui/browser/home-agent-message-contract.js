const MAX_VIEWER_TEXT_BYTES = 256 * 1024;
const MAX_VIEWER_DATA_URL_LENGTH = 350_000;
const MAX_BROWSER_URL_LENGTH = 2_048;

function hasExactKeys(value, expected) {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    return false;
  }
  const actual = Object.keys(value).sort();
  const wanted = [...expected].sort();
  return actual.length === wanted.length && actual.every((key, index) => key === wanted[index]);
}

export function normalizeHomeAgentViewerPayload(message) {
  if (!hasExactKeys(message, ["type", "request"])) {
    return null;
  }
  const request = message.request;
  if (
    !hasExactKeys(request, ["target", "title", "kind", "query", "deliver"]) ||
    request.target !== "documents" ||
    request.kind !== "code" ||
    typeof request.title !== "string" ||
    request.title.length < 1 ||
    request.title.length > 128 ||
    !hasExactKeys(request.query, ["view"]) ||
    request.query.view !== "read"
  ) {
    return null;
  }
  const deliver = request.deliver;
  if (
    !hasExactKeys(deliver, ["type", "attachmentId", "fileName", "mimeType", "dataUrl"]) ||
    deliver.type !== "documents:open-chat-attachment" ||
    typeof deliver.attachmentId !== "string" ||
    !/^code-\d{1,20}$/.test(deliver.attachmentId) ||
    typeof deliver.fileName !== "string" ||
    deliver.fileName.length < 1 ||
    deliver.fileName.length > 128 ||
    !/^[A-Za-z0-9._ -]+$/.test(deliver.fileName) ||
    deliver.mimeType !== "text/plain" ||
    typeof deliver.dataUrl !== "string" ||
    deliver.dataUrl.length > MAX_VIEWER_DATA_URL_LENGTH ||
    !deliver.dataUrl.startsWith("data:text/plain;base64,")
  ) {
    return null;
  }
  const encoded = deliver.dataUrl.slice("data:text/plain;base64,".length);
  if (encoded.length % 4 !== 0 || !/^[A-Za-z0-9+/]*={0,2}$/.test(encoded)) {
    return null;
  }
  try {
    const decoded = globalThis.atob(encoded);
    if (decoded.length > MAX_VIEWER_TEXT_BYTES || globalThis.btoa(decoded) !== encoded) {
      return null;
    }
  } catch {
    return null;
  }
  return Object.freeze({ ...deliver });
}

export function normalizeHomeAgentBrowserUrl(message) {
  if (
    !hasExactKeys(message, ["type", "url"]) ||
    typeof message.url !== "string" ||
    message.url !== message.url.trim() ||
    message.url.length < 1 ||
    message.url.length > MAX_BROWSER_URL_LENGTH
  ) {
    return "";
  }
  try {
    const parsed = new URL(message.url);
    if (
      (parsed.protocol !== "https:" && parsed.protocol !== "http:") ||
      parsed.username ||
      parsed.password
    ) {
      return "";
    }
    return parsed.href;
  } catch {
    return "";
  }
}

const ASSISTANT_DOC_ID = /^[0-9a-f]{32}$/;

/* Each Assistant document announces itself: hello {docId} when its script
   starts, ready {docId} once its workspace has loaded. Messages from one
   source arrive in order, so a document's hello always precedes its ready.
   A hello means a new document started (also an in-frame reload): hide until
   that document's ready. A ready from any other document is ignored.
   An Assistant from an earlier release (kept after Undo with the Home tab
   open) sends only {type} ready; that shows the room as it always did.
   receive() returns "hide", "show" or "" (no change). */
export function createAssistantFrameReadiness() {
  let current = null;
  let shown = false;
  return Object.freeze({
    receive(message) {
      if (hasExactKeys(message, ["type", "docId"]) &&
        typeof message.docId === "string" && ASSISTANT_DOC_ID.test(message.docId)) {
        if (message.type === "home-agent:hello") {
          current = message.docId;
          shown = false;
          return "hide";
        }
        if (message.type === "home-agent:ready" && message.docId === current && !shown) {
          shown = true;
          return "show";
        }
        return "";
      }
      if (hasExactKeys(message, ["type"]) && message.type === "home-agent:ready") {
        /* A legacy document is now active (e.g. Undo, then an in-frame reload
           into the old Assistant). One source delivers in order, so an old
           document's ready always arrives before the next document's hello and
           cannot land late after a current hello; after Home changes src, a
           stale legacy ready can at most show the room until the new hello. */
        current = null;
        shown = true;
        return "show";
      }
      return "";
    },
    /* Home is about to set or change the frame src: no document is current. */
    reset() {
      current = null;
      shown = false;
    },
  });
}
