export function createLibraryRealtime({
  loadCurrentFolder,
  parentUri,
  showError,
  state,
}) {
  function startLibraryEventStream() {
    if (!("EventSource" in window) || state.eventSource || !state.homeToken) {
      return;
    }
    window.clearTimeout(state.eventReconnectTimer);
    const url = "/api/provider/object/events/stream?home_token=" + encodeURIComponent(state.homeToken);
    const source = new EventSource(url);
    state.eventSource = source;
    source.addEventListener("library-events", (event) => {
      try {
        handleLibraryEventsPayload(JSON.parse(event.data || "{}"));
      } catch (error) {
        console.warn("Library event stream returned invalid payload", error);
      }
    });
    source.onerror = () => {
      source.close();
      if (state.eventSource === source) {
        state.eventSource = null;
      }
      state.eventReconnectTimer = window.setTimeout(startLibraryEventStream, 2_000);
    };
  }

  function stopLibraryEventStream() {
    if (state.eventSource) {
      state.eventSource.close();
      state.eventSource = null;
    }
    window.clearTimeout(state.eventReconnectTimer);
    window.clearTimeout(state.eventRefreshTimer);
  }

  function handleLibraryEventsPayload(payload) {
    if (payload?.schema !== "elastos.library.events/v1") {
      return;
    }
    const events = Array.isArray(payload.events) ? payload.events : [];
    if (events.some(eventTouchesCurrentFolder)) {
      scheduleLibraryEventRefresh();
    }
  }

  function scheduleLibraryEventRefresh() {
    window.clearTimeout(state.eventRefreshTimer);
    state.eventRefreshTimer = window.setTimeout(() => {
      loadCurrentFolder().catch(showError);
    }, 180);
  }

  function eventTouchesCurrentFolder(event) {
    if (!state.currentUri) return false;
    return eventUris(event).some((uri) => uriTouchesFolder(uri, state.currentUri));
  }

  function eventUris(event) {
    const details = event && typeof event.details === "object" ? event.details : {};
    return [
      event?.uri,
      details.old_uri,
      details.original_uri,
      details.source_uri,
      details.trash_uri,
      details.target_uri,
      details.object?.uri,
    ].filter(Boolean).map(String);
  }

  function uriTouchesFolder(uri, folderUri) {
    const folder = String(folderUri || "").replace(/\/+$/, "");
    const value = String(uri || "").replace(/\/+$/, "");
    return value === folder || value.startsWith(folder + "/") || parentUri(value) === folder;
  }

  return {
    startLibraryEventStream,
    stopLibraryEventStream,
  };
}
