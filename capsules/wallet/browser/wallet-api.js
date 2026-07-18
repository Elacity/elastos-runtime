import { readText } from "./wallet-format.js?v=wallet-20260718a";

export function readQueryParam(name) {
  const value = new URLSearchParams(window.location.search).get(name);
  return typeof value === "string" ? value.trim() : "";
}

export function createWalletApi({ getHomeToken }) {
  async function fetchJson(url, init) {
    const response = await fetch(url, init);
    if (!response.ok) {
      const detail = await response.text().catch(() => "");
      const suffix = detail.trim() ? ` ${detail.trim()}` : ` ${response.statusText}`;
      throw new Error(`request failed: ${response.status}${suffix}`);
    }
    return response.json();
  }

  async function requestFreshPasskeyHomeToken() {
    if (!window.PublicKeyCredential) {
      throw new Error("Passkey verification is unavailable in this browser.");
    }
    const begin = await fetchJson("/api/auth/passkey/authenticate/begin", { method: "POST" });
    const credential = await navigator.credentials.get(toRequestOptions(begin.options));
    if (!credential) {
      throw new Error("Passkey verification was cancelled.");
    }
    const complete = await fetchJson("/api/auth/passkey/authenticate/complete", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({
        ceremony_id: begin.ceremony_id,
        response: serializeAssertionCredential(credential),
      }),
    });
    const homeToken = readText(complete.home_token);
    if (!homeToken) {
      throw new Error("Fresh passkey token was not issued.");
    }
    return homeToken;
  }

  function shellHeaders(extra = {}) {
    return {
      ...extra,
      "x-elastos-home-token": getHomeToken(),
    };
  }

  function notifyHomeSummaryChanged() {
    const homeToken = getHomeToken();
    if (!homeToken || window.parent === window) {
      return;
    }
    window.parent.postMessage({
      type: "home:refresh-summary",
      homeToken,
    }, window.location.origin);
  }

  return {
    fetchJson,
    notifyHomeSummaryChanged,
    requestFreshPasskeyHomeToken,
    shellHeaders,
  };
}

function toRequestOptions(options) {
  const publicKey = { ...(options && options.publicKey ? options.publicKey : {}) };
  publicKey.challenge = base64UrlToBuffer(publicKey.challenge);
  publicKey.allowCredentials = (publicKey.allowCredentials || []).map((credential) => ({
    ...credential,
    id: base64UrlToBuffer(credential.id),
  }));
  return { publicKey };
}

function serializeAssertionCredential(credential) {
  return {
    id: credential.id,
    rawId: bufferToBase64Url(credential.rawId),
    type: credential.type,
    response: {
      clientDataJson: bufferToBase64Url(credential.response.clientDataJSON),
      authenticatorData: bufferToBase64Url(credential.response.authenticatorData),
      signature: bufferToBase64Url(credential.response.signature),
      userHandle: credential.response.userHandle
        ? bufferToBase64Url(credential.response.userHandle)
        : null,
    },
  };
}

function base64UrlToBuffer(value) {
  const text = readText(value);
  const padded = `${text.replace(/-/g, "+").replace(/_/g, "/")}${"=".repeat((4 - (text.length % 4)) % 4)}`;
  const binary = window.atob(padded);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i += 1) {
    bytes[i] = binary.charCodeAt(i);
  }
  return bytes.buffer;
}

function bufferToBase64Url(buffer) {
  const bytes = new Uint8Array(buffer || new ArrayBuffer(0));
  let binary = "";
  for (const byte of bytes) {
    binary += String.fromCharCode(byte);
  }
  return window.btoa(binary).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/g, "");
}
