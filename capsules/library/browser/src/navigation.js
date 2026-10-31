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

  // On a phone stage the shell owns the system Back: an entry of this frame's
  // own would take Back before the shell sees it (joint session history). The
  // shell's layout message (elastos-theme.js) names the stage, so an embedded
  // Library adds no entry until it knows it is not on a phone, and when the
  // shell resets its stage to a phone it drops the entries it made (the
  // in-app Back and Forward keep their stacks). It acknowledges once its
  // traversal landed, and answers the shell's drop request ({ drop: true })
  // the same way, so the shell arms its own Back only after that
  // (APP_HISTORY_MESSAGE in home-gui/browser/shell-phone-stage.js).
  const APP_HISTORY_MESSAGE = "elastos:app-history";
  let dropping = false;

  function onPhoneStage() {
    return typeof document !== "undefined" && document.documentElement?.dataset.elFormFactor === "phone";
  }

  function stageKnown() {
    return window.parent === window || Boolean(document.documentElement?.dataset.elFormFactor);
  }

  function announceHistory(held) {
    if (window.parent !== window) {
      window.parent.postMessage({ type: APP_HISTORY_MESSAGE, held }, "*");
    }
  }

  function browserHistoryAvailable() {
    return !!(window.history && typeof window.history.pushState === "function" && typeof window.history.replaceState === "function")
      && !onPhoneStage();
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

  // Embedded: follow the shell's stage resets. A phone drops this frame's
  // entries; a tablet or desktop installs them.
  let watchingStage = false;
  function watchStage() {
    if (watchingStage || window.parent === window || typeof MutationObserver !== "function") return;
    watchingStage = true;
    new MutationObserver(() => {
      if (onPhoneStage()) {
        dropBrowserHistory();
      } else {
        installBrowserHistory();
      }
    }).observe(document.documentElement, { attributes: true, attributeFilter: ["data-el-form-factor"] });
    window.addEventListener("message", (event) => {
      if (event.source === window.parent && event.data?.type === APP_HISTORY_MESSAGE && event.data.drop === true) {
        answerDropRequest();
      }
    });
  }

  // Steps back over every entry this frame pushed, onto the entry it had
  // when it loaded (the guard); handleBrowserPopState finishes the drop.
  function dropBrowserHistory() {
    if (dropping || state.historyIndex < 0) return;
    dropping = true;
    window.history.go(-(state.historyIndex + 1));
  }

  function finishDrop() {
    dropping = false;
    state.historyEntries = [];
    state.historyIndex = -1;
    announceHistory(false);
  }

  // The drop has landed once the current entry is no longer one this frame
  // pushed (it is the guard, or older). A pop onto one of its own entries
  // (a Back pressed meanwhile) is not the drop.
  function dropLanded(entryState) {
    return entryState?.schema !== LIBRARY_HISTORY_SCHEMA;
  }

  // The shell asks again when an acknowledgement is late: answer from this
  // frame's own history, never from a guess. Nothing held: say so again. The
  // traversal already landed: finish the drop now. Otherwise start it (a
  // no-op while one is in flight; its popstate answers).
  function answerDropRequest() {
    if (state.historyIndex < 0) {
      announceHistory(false);
      return;
    }
    if (dropping && dropLanded(window.history.state)) {
      finishDrop();
      return;
    }
    dropBrowserHistory();
  }

  function installBrowserHistory() {
    if (!state.currentUri) return;
    watchStage();
    if (!browserHistoryAvailable() || !stageKnown() || state.historyIndex >= 0) return;
    const entry = createHistoryEntry(state.currentUri);
    state.historyEntries = [entry];
    state.historyIndex = 0;
    window.history.replaceState(libraryHistoryGuardState(state.currentUri), "", window.location.href);
    window.history.pushState(libraryHistoryState(entry), "", window.location.href);
    syncNavigationStacksFromHistory();
    announceHistory(true);
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
    if (dropping) {
      if (dropLanded(event.state)) {
        finishDrop();
      }
      return;
    }
    // The shell owns Back on a phone stage; this frame holds no entries there.
    if (onPhoneStage()) return;
    if (data.schema === LIBRARY_HISTORY_GUARD_SCHEMA) {
      const entry = state.historyEntries[state.historyIndex] || createHistoryEntry(state.currentUri);
      window.history.pushState(libraryHistoryState(entry), "", window.location.href);
      setStatus("Library kept focus.");
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
