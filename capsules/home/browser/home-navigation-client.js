// Home renews registered frame authority; Runtime authorizes each launch.
// Navigation hints carry bounded presentation state.
export function createHomeNavigationClient({ homeToken, homeOrigin, enabled = true, target = "", onHomeToken = null, windowRef = window }) {
  const documentNonce = windowRef.crypto.randomUUID();
  let requestId = "";
  let sequence = 0;
  let query = null;
  let unloading = false;
  let pendingRenewal = null;
  function renewAuthority() {
    if (pendingRenewal) return pendingRenewal.promise;
    if (!attached() || !target || unloading) return Promise.reject(new Error("Open this app from Home to reconnect."));
    const id = windowRef.crypto.randomUUID();
    let resolve, reject;
    const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
    const record = { id, promise, reject, timer: 0 };
    const finish = (error, token) => {
      windowRef.clearTimeout(record.timer);
      windowRef.removeEventListener("message", receive);
      if (pendingRenewal === record) pendingRenewal = null;
      if (error) reject(error); else resolve(token);
    };
    const receive = event => {
      const data = event.data;
      if (event.source !== windowRef.top || event.origin !== homeOrigin ||
          data?.type !== "home:shell-response" || data.requestId !== id) return;
      if (data.error) {
        const error = new Error(data.status === 401 || data.status === 403
          ? "Sign in to Home. Your drafts are kept." : "Home could not reconnect this app. Try again.");
        error.status = data.status;
        finish(error);
        return;
      }
      const launched = data.result;
      let route;
      try { route = new URL(launched?.route, homeOrigin); } catch { finish(new Error("Home returned an invalid app launch.")); return; }
      const token = new URLSearchParams(route.hash.slice(1)).get("home_token") || "";
      if (launched?.target !== target || launched.attach_kind !== "iframe" ||
          (launched.launch_status && launched.launch_status !== "launched") ||
          route.origin !== homeOrigin || route.pathname !== `/apps/${target}/` ||
          route.username || route.password || !token || token === homeToken || token !== token.trim()) {
        finish(new Error("Home returned an invalid app launch.")); return;
      }
      try {
        onHomeToken?.(token);
        setHomeToken(token);
        finish(null, token);
      } catch (error) { finish(error); }
    };
    record.cancel = () => finish(new Error("App closed."));
    record.timer = windowRef.setTimeout(() => finish(new Error("Home did not answer. Try reconnecting again.")), 30000);
    pendingRenewal = record;
    windowRef.addEventListener("message", receive);
    try {
      windowRef.top.postMessage({ type: "home:launch-target", requestId: id,
        target, query: query || {}, homeToken }, homeOrigin);
    } catch (error) { finish(error); }
    return promise;
  }
  function setHomeToken(value) {
    homeToken = value;
    requestId = "";
    sequence = 0;
    if (attached()) windowRef.top.postMessage({ type: "home:frame-authority-ready", homeToken }, homeOrigin);
  }
  windowRef.addEventListener("message", event => {
    const data = event.data;
    if (attached() && !unloading && event.source === windowRef.top && event.origin === homeOrigin &&
        data?.type === "elastos.home.authority.request/v1" && data.homeToken === homeToken &&
        Object.keys(data).sort().join(",") === "homeToken,type") {
      renewAuthority().catch(() => {});
    }
  });
  const attached = () => homeToken && homeOrigin && homeOrigin !== "null" && homeOrigin !== "*" && windowRef.top !== windowRef;
  function publish(phase = "state") {
    if (!enabled || !attached() || !requestId || (unloading && phase !== "unloading")) return;
    windowRef.top.postMessage({ type: "home:navigation-hint", homeToken,
      requestId, documentNonce, sequence: ++sequence, phase,
      query: phase === "unloading" ? null : query }, homeOrigin);
  }
  windowRef.addEventListener("message", (event) => {
    const data = event.data;
    if (!enabled || !attached() || unloading || event.source !== windowRef.parent || event.origin !== "null" ||
        !data || Object.keys(data).sort().join(",") !== "homeToken,requestId,type" ||
        data.type !== "elastos.home.navigation.request/v1" || data.homeToken !== homeToken ||
        typeof data.requestId !== "string" || !/^[A-Za-z0-9-]{1,64}$/.test(data.requestId)) return;
    requestId = data.requestId;
    publish();
  });
  windowRef.addEventListener("pagehide", () => { publish("unloading"); unloading = true; pendingRenewal?.cancel(); });
  windowRef.addEventListener("pageshow", () => {
    unloading = false;
    if (attached()) windowRef.top.postMessage({ type: "home:app-ready", homeToken }, homeOrigin);
  });
  return {
    setQuery(value) { query = { ...value }; publish(); },
    setHomeToken,
    renewAuthority,
  };
}
