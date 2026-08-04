export const HOME_BROWSER_CONTEXT_STORAGE_KEY = "elastos.home.browser-context-id";
export const HOME_BROWSER_CONTEXT_PATTERN = /^browser:[0-9a-f]{32}$/;

export function isHomeBrowserContextId(value) {
  return typeof value === "string" && HOME_BROWSER_CONTEXT_PATTERN.test(value);
}

export function createHomeBrowserContextId(cryptoSource) {
  if (!cryptoSource || typeof cryptoSource.getRandomValues !== "function") {
    throw new Error("Home requires browser crypto for session isolation");
  }
  const bytes = new Uint8Array(16);
  cryptoSource.getRandomValues(bytes);
  const token = Array.from(
    bytes,
    (byte) => byte.toString(16).padStart(2, "0"),
  ).join("");
  return `browser:${token}`;
}

export function loadOrCreateHomeBrowserContextId(storage, cryptoSource) {
  if (
    !storage ||
    typeof storage.getItem !== "function" ||
    typeof storage.setItem !== "function"
  ) {
    return "";
  }
  try {
    const stored = storage.getItem(HOME_BROWSER_CONTEXT_STORAGE_KEY);
    if (isHomeBrowserContextId(stored)) {
      return stored;
    }
    const created = createHomeBrowserContextId(cryptoSource);
    storage.setItem(HOME_BROWSER_CONTEXT_STORAGE_KEY, created);
    return storage.getItem(HOME_BROWSER_CONTEXT_STORAGE_KEY) === created
      ? created
      : "";
  } catch (_error) {
    return "";
  }
}
