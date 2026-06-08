export function localBrowserInstanceId() {
  if (window.crypto && typeof window.crypto.randomUUID === "function") {
    return `browser:${window.crypto.randomUUID()}`;
  }
  return `browser:${Date.now()}:${Math.random().toString(16).slice(2)}`;
}

export function rememberedRuntimePage(storageKey) {
  try {
    const pageId = window.sessionStorage.getItem(storageKey);
    return pageId ? { page_id: pageId } : null;
  } catch {
    return null;
  }
}

export function publishRuntimePageForHost(storageKey, page) {
  const pageId = page?.page_id || "";
  window.__elastosBrowserCurrentPageId = pageId;
  try {
    if (pageId) {
      window.sessionStorage.setItem(storageKey, pageId);
    } else {
      window.sessionStorage.removeItem(storageKey);
    }
  } catch {
    // Session storage can be blocked by the embedding environment; in-memory cleanup still runs.
  }
}

export function commitHistoryState({ entries, index }, url, mode) {
  if (mode === "none") {
    return { entries, index };
  }
  if (mode === "replace" && index >= 0) {
    const nextEntries = [...entries];
    nextEntries[index] = url;
    return { entries: nextEntries, index };
  }
  if (entries[index] === url) {
    return { entries, index };
  }
  const nextEntries = entries.slice(0, index + 1);
  nextEntries.push(url);
  return { entries: nextEntries, index: nextEntries.length - 1 };
}
