// Bounded presentation hints only. Runtime authorizes each restored launch.
export function createHomeNavigationClient({ homeToken, homeOrigin, enabled = true, windowRef = window }) {
  const documentNonce = windowRef.crypto.randomUUID();
  let requestId = "";
  let sequence = 0;
  let query = null;
  let unloading = false;
  const attached = () => enabled && homeToken && homeOrigin && windowRef.top !== windowRef;
  function publish(phase = "state") {
    if (!attached() || !requestId || (unloading && phase !== "unloading")) return;
    windowRef.top.postMessage({ type: "home:navigation-hint", homeToken,
      requestId, documentNonce, sequence: ++sequence, phase,
      query: phase === "unloading" ? null : query }, homeOrigin);
  }
  windowRef.addEventListener("message", (event) => {
    const data = event.data;
    if (!attached() || unloading || event.source !== windowRef.parent || event.origin !== "null" ||
        !data || Object.keys(data).sort().join(",") !== "homeToken,requestId,type" ||
        data.type !== "elastos.home.navigation.request/v1" || data.homeToken !== homeToken ||
        typeof data.requestId !== "string" || !/^[A-Za-z0-9-]{1,64}$/.test(data.requestId)) return;
    requestId = data.requestId;
    publish();
  });
  windowRef.addEventListener("pagehide", () => { publish("unloading"); unloading = true; });
  windowRef.addEventListener("pageshow", () => {
    unloading = false;
    if (attached()) windowRef.top.postMessage({ type: "home:app-ready", homeToken }, homeOrigin);
  });
  return { setQuery(value) { query = { ...value }; publish(); } };
}
