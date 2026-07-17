export function createLibraryRealtime({
  loadCurrentFolder,
  parentUri,
  showError,
  state,
}) {
  async function startLibraryEventStream() {
    if (state.eventSource || !state.homeToken) {
      return;
    }
    window.clearTimeout(state.eventReconnectTimer);
    const controller = new AbortController();
    const source = { close: () => controller.abort() };
    state.eventSource = source;
    try {
      const response = await fetch("/api/provider/object/events/stream", {
        headers: { "x-elastos-home-token": state.homeToken },
        signal: controller.signal,
      });
      if (!response.ok || !response.body) {
        throw new Error(`Library event stream failed: ${response.status}`);
      }
      await readLibraryEventStream(response.body);
    } catch (error) {
      if (error?.name !== "AbortError") {
        console.warn("Library event stream interrupted", error);
      }
    } finally {
      if (state.eventSource === source) {
        state.eventSource = null;
        state.eventReconnectTimer = window.setTimeout(startLibraryEventStream, 2_000);
      }
    }
  }

  async function readLibraryEventStream(body) {
    const reader = body.getReader();
    const decoder = new TextDecoder();
    let buffer = "";
    while (true) {
      const { done, value } = await reader.read();
      buffer += decoder.decode(value || new Uint8Array(), { stream: !done }).replace(/\r\n/g, "\n");
      let boundary = buffer.indexOf("\n\n");
      while (boundary >= 0) {
        const block = buffer.slice(0, boundary);
        buffer = buffer.slice(boundary + 2);
        handleLibraryEventBlock(block);
        boundary = buffer.indexOf("\n\n");
      }
      if (done) {
        return;
      }
    }
  }

  function handleLibraryEventBlock(block) {
    const lines = block.split("\n");
    const event = lines.find((line) => line.startsWith("event:"))?.slice(6).trim();
    if (event !== "library-events") {
      return;
    }
    const data = lines
      .filter((line) => line.startsWith("data:"))
      .map((line) => line.slice(5).trimStart())
      .join("\n");
    try {
      handleLibraryEventsPayload(JSON.parse(data || "{}"));
    } catch (error) {
      console.warn("Library event stream returned invalid payload", error);
    }
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
