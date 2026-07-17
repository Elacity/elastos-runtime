export function createLibraryRuntime({ getHomeToken }) {
  const CHUNKED_UPLOAD_THRESHOLD_BYTES = 512 * 1024;
  const CHUNKED_UPLOAD_BYTES = 512 * 1024;
  const CHUNKED_UPLOAD_TRANSPORT = "http-chunk-session";
  const homeParentOrigin = new URLSearchParams(window.location.search).get("home_origin") || "";

  async function providerApi(op, payload) {
    const response = await fetch("/api/provider/object/" + encodeURIComponent(op), {
      method: "POST",
      headers: {
        "content-type": "application/json",
        "x-elastos-home-token": getHomeToken(),
      },
      body: JSON.stringify(payload || {}),
    });
    const envelope = await response.json().catch(() => ({}));
    if (!response.ok) {
      throw new Error(envelope.message || envelope.error || `Library request failed: ${response.status}`);
    }
    if (envelope.status === "error") {
      throw new Error(envelope.message || "Runtime object provider failed.");
    }
    return envelope.data || envelope;
  }

  function uploadObject({ uri, file, mime, onProgress }) {
    if (file?.size > CHUNKED_UPLOAD_THRESHOLD_BYTES) {
      return uploadObjectChunked({ uri, file, mime, onProgress });
    }
    return uploadObjectRaw({ uri, file, mime, onProgress });
  }

  function uploadObjectRaw({ uri, file, mime, onProgress }) {
    return new Promise((resolve, reject) => {
      const url = new URL("/api/provider/object/upload", window.location.origin);
      url.searchParams.set("uri", uri);
      const xhr = new XMLHttpRequest();
      xhr.open("PUT", url.pathname + url.search);
      xhr.setRequestHeader("x-elastos-home-token", getHomeToken());
      xhr.setRequestHeader("content-type", mime || file?.type || "application/octet-stream");
      xhr.upload.onprogress = (event) => {
        if (event.lengthComputable && typeof onProgress === "function") {
          onProgress(event.loaded / Math.max(event.total, 1));
        }
      };
      xhr.onerror = () => reject(new Error("Library upload failed before Runtime accepted the object."));
      xhr.onload = () => {
        let envelope = {};
        try {
          envelope = JSON.parse(xhr.responseText || "{}");
        } catch {
          envelope = { message: xhr.responseText || "" };
        }
        if (xhr.status < 200 || xhr.status >= 300) {
          reject(new Error(uploadFailureMessage(xhr, envelope)));
          return;
        }
        if (envelope.status === "error") {
          reject(new Error(envelope.message || "Runtime object provider upload failed."));
          return;
        }
        resolve(envelope.data || envelope);
      };
      xhr.send(file);
    });
  }

  async function uploadObjectChunked({ uri, file, mime, onProgress }) {
    const contentType = mime || file?.type || "application/octet-stream";
    const start = await uploadJsonRequest("/api/provider/object/upload/start", {
      uri,
      mime: contentType,
      size_bytes: file.size,
      transport: CHUNKED_UPLOAD_TRANSPORT,
    });
    const uploadId = start.upload_id;
    try {
      let offset = 0;
      while (offset < file.size) {
        const end = Math.min(offset + CHUNKED_UPLOAD_BYTES, file.size);
        const chunk = file.slice(offset, end, contentType);
        await uploadChunkRequest({
          uploadId,
          offset,
          chunk,
          mime: contentType,
          onProgress: (loaded) => {
            if (typeof onProgress === "function") {
              onProgress((offset + loaded) / Math.max(file.size, 1));
            }
          },
        });
        offset = end;
        if (typeof onProgress === "function") {
          onProgress(offset / Math.max(file.size, 1));
        }
      }
      return await uploadJsonRequest(`/api/provider/object/upload/${encodeURIComponent(uploadId)}/finish`, null);
    } catch (error) {
      await cancelUploadSession(uploadId);
      throw error;
    }
  }

  async function uploadJsonRequest(path, payload) {
    const response = await fetch(path, {
      method: "POST",
      headers: {
        "content-type": "application/json",
        "x-elastos-home-token": getHomeToken(),
      },
      body: payload ? JSON.stringify(payload) : undefined,
    });
    const text = await response.text();
    const envelope = parseEnvelope(text);
    if (!response.ok || envelope.status === "error") {
      throw new Error(uploadFailureMessage(responseLike(response, text), envelope));
    }
    return envelope.data || envelope;
  }

  function uploadChunkRequest({ uploadId, offset, chunk, mime, onProgress }) {
    return new Promise((resolve, reject) => {
      const xhr = new XMLHttpRequest();
      xhr.open("PUT", `/api/provider/object/upload/${encodeURIComponent(uploadId)}/chunk`);
      xhr.setRequestHeader("x-elastos-home-token", getHomeToken());
      xhr.setRequestHeader("x-elastos-upload-offset", String(offset));
      xhr.setRequestHeader("content-type", mime || "application/octet-stream");
      xhr.upload.onprogress = (event) => {
        if (event.lengthComputable && typeof onProgress === "function") {
          onProgress(event.loaded);
        }
      };
      xhr.onerror = () => reject(new Error("Library chunk upload failed before Runtime accepted the object."));
      xhr.onload = () => {
        const envelope = parseEnvelope(xhr.responseText || "");
        if (xhr.status < 200 || xhr.status >= 300 || envelope.status === "error") {
          reject(new Error(uploadFailureMessage(xhr, envelope)));
          return;
        }
        resolve(envelope.data || envelope);
      };
      xhr.send(chunk);
    });
  }

  async function cancelUploadSession(uploadId) {
    if (!uploadId) return;
    try {
      await fetch(`/api/provider/object/upload/${encodeURIComponent(uploadId)}`, {
        method: "DELETE",
        headers: {
          "x-elastos-home-token": getHomeToken(),
        },
      });
    } catch {
      // Best-effort cleanup; Runtime also expires stale provider-private sessions on new upload starts.
    }
  }

  function parseEnvelope(text) {
    try {
      return JSON.parse(text || "{}");
    } catch {
      return { message: text || "" };
    }
  }

  function responseLike(response, text) {
    return {
      status: response.status,
      responseText: text,
    };
  }

  function uploadFailureMessage(xhr, envelope) {
    const body = String(envelope.message || envelope.error || xhr.responseText || "");
    if (xhr.status === 413 || /request entity too large|nginx/i.test(body)) {
      return "This file is too large for the current upload service.";
    }
    return body || `Library upload failed: ${xhr.status}`;
  }

  async function downloadObjectRaw({ uri, uris, archive }) {
    const url = new URL("/api/provider/object/download/raw", window.location.origin);
    const list = Array.isArray(uris) ? uris : [uri];
    for (const item of list) {
      if (item) url.searchParams.append("uri", item);
    }
    if (archive) {
      url.searchParams.set("archive", archive);
    }
    const response = await fetch(url.pathname + url.search, {
      method: "GET",
      headers: {
        "x-elastos-home-token": getHomeToken(),
      },
    });
    if (!response.ok) {
      throw new Error((await response.text()) || `Library download failed: ${response.status}`);
    }
    const blob = await response.blob();
    return {
      blob,
      filename: filenameFromContentDisposition(response.headers.get("content-disposition")),
      requestId: response.headers.get("x-elastos-request-id") || "",
      receipt: transferReceiptFromHeader(response.headers.get("x-elastos-transfer-receipt")),
    };
  }

  function transferReceiptFromHeader(header) {
    try {
      return header ? JSON.parse(header) : null;
    } catch {
      return null;
    }
  }

  function filenameFromContentDisposition(header) {
    const match = String(header || "").match(/filename="([^"]+)"/);
    return match ? match[1] : "";
  }

  function shellMessage(message) {
    if (homeParentOrigin && window.top && window.top !== window) {
      window.top.postMessage({ ...message, homeToken: getHomeToken() }, homeParentOrigin);
      return true;
    }
    return false;
  }

  function openTarget(target, query) {
    return shellMessage({ type: "home:open-target", target, query: query || {} });
  }

  function openPublishedUri(uri, preferredViewer) {
    return shellMessage({ type: "home:open-uri", uri, preferredViewer: preferredViewer || "documents" });
  }

  function deliverToTarget(target, payload) {
    return shellMessage({ type: "home:deliver-to-target", target, payload: payload || {} });
  }

  function closeSelf() {
    return shellMessage({ type: "home:close-self" });
  }

  return {
    providerApi,
    uploadObject,
    downloadObjectRaw,
    openTarget,
    openPublishedUri,
    deliverToTarget,
    closeSelf,
  };
}
