import { readText } from "./wallet-format.js?v=wallet-20260720j";

export function readQueryParam(name) {
  const value = new URLSearchParams(window.location.search).get(name);
  return typeof value === "string" ? value.trim() : "";
}

export function readLaunchToken() {
  const value = new URLSearchParams(window.location.hash.replace(/^#/, "")).get("home_token");
  return typeof value === "string" ? value.trim() : "";
}

export function readHomeOrigin() {
  return readQueryParam("home_origin");
}

export function createWalletApi({ getHomeToken }) {
  const homeParentOrigin = readHomeOrigin();

  async function fetchJson(url, init) {
    const response = await fetch(url, init);
    if (!response.ok) {
      const detail = await response.text().catch(() => "");
      const suffix = detail.trim() ? ` ${detail.trim()}` : ` ${response.statusText}`;
      throw new Error(`request failed: ${response.status}${suffix}`);
    }
    return response.json();
  }

  async function requestFreshPasskeyHomeToken(operation, request) {
    return requestHomePasskeyAuthority(getHomeToken(), homeParentOrigin, operation, request);
  }

  function shellHeaders(extra = {}, authorityToken = getHomeToken()) {
    return {
      ...extra,
      "x-elastos-home-token": authorityToken,
    };
  }

  function notifyHomeSummaryChanged() {
    const homeToken = getHomeToken();
    if (!homeToken || !homeParentOrigin || window.top === window) {
      return;
    }
    window.top.postMessage({
      type: "home:refresh-summary",
      homeToken,
    }, homeParentOrigin);
  }

  return {
    fetchJson,
    notifyHomeSummaryChanged,
    requestFreshPasskeyHomeToken,
    shellHeaders,
  };
}

function requestHomePasskeyAuthority(homeToken, parentOrigin, operation, request) {
  if (!homeToken || window.top === window || !parentOrigin) {
    return Promise.reject(new Error("Open Wallet from Home to verify your passkey."));
  }
  const requestId = window.crypto?.randomUUID?.()
    || `passkey-${Date.now()}-${Math.random().toString(16).slice(2)}`;
  return new Promise((resolve, reject) => {
    const timeout = window.setTimeout(() => {
      window.removeEventListener("message", onResult);
      reject(new Error("Passkey verification timed out."));
    }, 120_000);
    const onResult = (event) => {
      if (event.source !== window.top || event.origin !== parentOrigin) {
        return;
      }
      const result = event.data && typeof event.data === "object" ? event.data : null;
      if (result?.type !== "home:passkey-authority-result" || result.requestId !== requestId) {
        return;
      }
      window.clearTimeout(timeout);
      window.removeEventListener("message", onResult);
      const freshToken = readText(result.homeToken);
      if (freshToken) {
        resolve(freshToken);
        return;
      }
      reject(new Error(readText(result.error) || "Passkey verification failed."));
    };
    window.addEventListener("message", onResult);
    window.top.postMessage({
      type: "home:request-passkey-authority",
      requestId,
      homeToken,
      operation,
      request,
    }, parentOrigin);
  });
}
