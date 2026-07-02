export const DEFAULT_URL = "https://ela.city/";

export function normalizeUrl(value, defaultUrl = DEFAULT_URL) {
  const trimmed = String(value || "").trim();
  const candidate = trimmed || defaultUrl;
  if (/^tls:\/\//i.test(candidate) || /^tcp:\/\//i.test(candidate)) {
    const transport = new URL(candidate);
    const scheme = transport.protocol === "tls:" ? "https" : "http";
    const defaultPort = transport.protocol === "tls:" ? "443" : "80";
    const port = transport.port && transport.port !== defaultPort ? `:${transport.port}` : "";
    const suffix = `${transport.pathname || "/"}${transport.search}${transport.hash}`;
    return `${scheme}://${transport.hostname}${port}${suffix}`;
  }
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

export function sameBrowserStreamTarget(currentValue, nextValue) {
  try {
    return (
      streamTargetForUrl(normalizeUrl(currentValue)) ===
      streamTargetForUrl(normalizeUrl(nextValue))
    );
  } catch {
    return false;
  }
}

export function isBrowserErrorUrl(value) {
  const text = String(value || "").trim().toLowerCase();
  return text === "chrome-error://chromewebdata/" || text.startsWith("chrome-error://");
}

export function visibleAddressForUrl(value) {
  const parsed = new URL(value);
  if (parsed.protocol === "https:" && (parsed.hostname === "ela.city" || parsed.hostname.endsWith(".ela.city"))) {
    const suffix = `${parsed.pathname || "/"}${parsed.search}${parsed.hash}`;
    return suffix === "/" ? `${parsed.hostname}://` : `${parsed.hostname}://${suffix.replace(/^\/+/, "")}`;
  }
  return parsed.toString();
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
