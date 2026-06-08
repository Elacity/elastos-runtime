const LIBRARY_HISTORY_SCHEMA = "elastos.library.history/v1";
const LIBRARY_HISTORY_GUARD_SCHEMA = "elastos.library.history.guard/v1";

export function createLibraryNavigation({
  backButton,
  forwardButton,
  loadCurrentFolder,
  parentUri,
  rootForUri,
  searchInput,
  setStatus,
  state,
  syncRouteChrome = () => {},
  upButton,
}) {
  function syncNavigationButtons() {
    backButton.disabled = state.backStack.length === 0;
    forwardButton.disabled = state.forwardStack.length === 0;
    const root = rootForUri(state.currentUri);
    upButton.disabled = !root || state.currentUri === root.uri;
  }

  function browserHistoryAvailable() {
    return !!(window.history && typeof window.history.pushState === "function" && typeof window.history.replaceState === "function");
  }

  function createHistoryEntry(uri) {
    return { key: `library:${++state.historyKeyCounter}`, uri };
  }

  function libraryHistoryState(entry) {
    return {
      schema: LIBRARY_HISTORY_SCHEMA,
      key: entry.key,
      uri: entry.uri,
    };
  }

  function libraryHistoryGuardState(uri) {
    return {
      schema: LIBRARY_HISTORY_GUARD_SCHEMA,
      uri,
    };
  }

  function syncNavigationStacksFromHistory() {
    if (state.historyIndex < 0) return;
    state.backStack = state.historyEntries.slice(0, state.historyIndex).map((entry) => entry.uri);
    state.forwardStack = state.historyEntries.slice(state.historyIndex + 1).map((entry) => entry.uri).reverse();
    syncNavigationButtons();
  }

  function installBrowserHistory() {
    if (!browserHistoryAvailable() || !state.currentUri) return;
    const entry = createHistoryEntry(state.currentUri);
    state.historyEntries = [entry];
    state.historyIndex = 0;
    window.history.replaceState(libraryHistoryGuardState(state.currentUri), "", window.location.href);
    window.history.pushState(libraryHistoryState(entry), "", window.location.href);
    syncNavigationStacksFromHistory();
  }

  function pushBrowserHistory(uri) {
    if (!browserHistoryAvailable() || !uri || state.historyIndex < 0) return false;
    state.historyEntries = state.historyEntries.slice(0, state.historyIndex + 1);
    const entry = createHistoryEntry(uri);
    state.historyEntries.push(entry);
    state.historyIndex = state.historyEntries.length - 1;
    window.history.pushState(libraryHistoryState(entry), "", window.location.href);
    syncNavigationStacksFromHistory();
    return true;
  }

  async function handleBrowserPopState(event) {
    const data = event.state || {};
    if (data.schema === LIBRARY_HISTORY_GUARD_SCHEMA) {
      const entry = state.historyEntries[state.historyIndex] || createHistoryEntry(state.currentUri);
      window.history.pushState(libraryHistoryState(entry), "", window.location.href);
      setStatus("Explorer kept focus.");
      return;
    }
    if (data.schema !== LIBRARY_HISTORY_SCHEMA || !data.key || !data.uri) return;
    const index = state.historyEntries.findIndex((entry) => entry.key === data.key);
    if (index === -1) return;
    state.historyIndex = index;
    syncNavigationStacksFromHistory();
    if (state.currentUri === data.uri) return;
    state.currentUri = data.uri;
    state.query = "";
    searchInput.value = "";
    syncRouteChrome();
    syncNavigationButtons();
    await loadCurrentFolder({ useCache: true });
  }

  async function navigate(uri, options = {}) {
    if (!uri || uri === state.currentUri) {
      return;
    }
    const previousUri = state.currentUri;
    if (options.record !== false && state.currentUri) {
      if (!pushBrowserHistory(uri)) {
        state.backStack.push(previousUri);
        state.forwardStack = [];
      }
    }
    state.currentUri = uri;
    state.query = "";
    searchInput.value = "";
    syncRouteChrome();
    syncNavigationButtons();
    await loadCurrentFolder({ useCache: true });
  }

  async function navigateBack() {
    if (browserHistoryAvailable() && state.historyIndex > 0) {
      window.history.back();
      return;
    }
    const uri = state.backStack.pop();
    if (!uri) return;
    if (state.currentUri) state.forwardStack.push(state.currentUri);
    await navigate(uri, { record: false });
  }

  async function navigateForward() {
    if (browserHistoryAvailable() && state.historyIndex >= 0 && state.historyIndex < state.historyEntries.length - 1) {
      window.history.forward();
      return;
    }
    const uri = state.forwardStack.pop();
    if (!uri) return;
    if (state.currentUri) state.backStack.push(state.currentUri);
    await navigate(uri, { record: false });
  }

  async function navigateUp() {
    const root = rootForUri(state.currentUri);
    if (!root || state.currentUri === root.uri) return;
    await navigate(parentUri(state.currentUri));
  }

  return {
    handleBrowserPopState,
    installBrowserHistory,
    navigate,
    navigateBack,
    navigateForward,
    navigateUp,
    syncNavigationButtons,
  };
}
