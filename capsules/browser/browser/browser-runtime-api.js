export const DEFAULT_URL = "https://ela.city/";

export function normalizeUrl(value, defaultUrl = DEFAULT_URL) {
  const trimmed = String(value || "").trim();
  const candidate = trimmed || defaultUrl;
  const withScheme = /^[a-z][a-z0-9+.-]*:/i.test(candidate)
    ? candidate
    : `https://${candidate}`;
  const parsed = new URL(withScheme);
  if (parsed.protocol !== "http:" && parsed.protocol !== "https:") {
    throw new Error("Only http and https addresses can be opened.");
  }
  return parsed.toString();
}

export function streamTargetForUrl(value) {
  const parsed = new URL(value);
  const port = parsed.port || (parsed.protocol === "https:" ? "443" : "80");
  const scheme = parsed.protocol === "https:" ? "tls" : "tcp";
  return `${scheme}://${parsed.hostname}:${port}`;
}

export function createRuntimeApi({ launchToken }) {
  function homeHeaders(hasBody = false) {
    const headers = {};
    if (launchToken) {
      headers["x-elastos-home-token"] = launchToken;
    }
    if (hasBody) {
      headers["content-type"] = "application/json";
    }
    return headers;
  }

  async function fetchJson(path, options = {}) {
    const body = options.body == null ? undefined : JSON.stringify(options.body);
    const response = await fetch(path, {
      ...options,
      body,
      headers: {
        ...homeHeaders(Boolean(body)),
        ...(options.headers || {}),
      },
    });
    const text = await response.text();
    let payload = null;
    if (text) {
      try {
        payload = JSON.parse(text);
      } catch {
        payload = text;
      }
    }
    if (!response.ok) {
      const message =
        typeof payload === "string"
          ? payload
          : payload?.error || payload?.message || `request failed: ${response.status}`;
      const error = new Error(message);
      error.status = response.status;
      error.payload = payload;
      throw error;
    }
    return payload;
  }

  return { fetchJson, homeHeaders };
}
