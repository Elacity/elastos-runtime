import { timingSafeEqual } from "node:crypto";
import { BrowserOperatorError } from "./browser-operator-client.mjs";

export const PLAYWRIGHT_OPERATOR_CLIENT = Object.freeze({
  package: "playwright-core", version: "1.59.1",
  revision: "d466ac5358cae058cdc75d2ae3ab3ad220042730",
  profile: "runtime-reference-fill-v1",
  tarball: "https://registry.npmjs.org/playwright-core/-/playwright-core-1.59.1.tgz",
  integrity: "sha512-HBV/RJg81z5BiiZ9yPzIiClYV/QMsDCKUyogwH9p3MCP6IYjUFu/MActgYAvK0oWyV9NlwM3GLBjADyWgydVyg==",
  treeSha256: "b7195ef48f7c5fd966810f8f7a9e8e27d6bddfb6391c30c5e614e2e351fa7e2c",
});

const requireValue = (ok, code) => { if (!ok) throw new BrowserOperatorError(code); };
const object = value => value !== null && typeof value === "object" && !Array.isArray(value);
const only = (value, keys) => object(value) && Object.keys(value).every(key => keys.includes(key));
const channel = guid => ({ guid });
const unsupported = () => { throw new BrowserOperatorError("capability_unsupported", 501); };

// The caller owns the authenticated WebSocket transport. This adapter owns one
// operator connection to one existing Runtime page, with no provider transport.
export function createPlaywrightOperatorAdapter({ client, accessKey, now = () => performance.now() }) {
  requireValue(client && typeof client.pageId === "string" &&
    ["status", "inspect", "input", "detach"].every(key => typeof client[key] === "function") &&
    typeof accessKey === "string" && /^[a-zA-Z0-9_-]{16,128}$/.test(accessKey), "adapter_configuration_invalid");
  let peer, initialized = false, lastId = 0, active = null, stopped = false;
  let bindings = new Map(), expires = 0, disconnectPromise, lastUncertain, writeUncertain = false;
  let resolveClosed;
  const closed = new Promise(resolve => { resolveClosed = resolve; });
  const lifetime = new AbortController();
  const guids = new Set();

  function authenticate(headers) {
    const h = new Headers(headers);
    const actual = Buffer.from(h.get("authorization") || "");
    const expected = Buffer.from(`Bearer ${accessKey}`);
    requireValue(actual.length === expected.length && timingSafeEqual(actual, expected) &&
      !h.has("cookie") && !h.has("x-elastos-home-token"), "adapter_authority_required");
    requireValue(h.get("x-elastos-playwright-client") === PLAYWRIGHT_OPERATOR_CLIENT.version &&
      h.get("x-playwright-browser") === "chromium", "adapter_client_version_unsupported");
    requireValue(!h.get("x-playwright-proxy"), "capability_unsupported");
  }
  function send(message) {
    if (stopped) return;
    try { peer.send(JSON.stringify(message)); }
    catch { void disconnect(); }
  }
  function create(parent, type, guid, initializer = {}) {
    guids.add(guid);
    send({ guid: parent, method: "__create__", params: { type, guid, initializer } });
  }
  function graph() {
    // These initializers are required by the unchanged pinned SDK. Every command
    // on the support objects is rejected. No executable or website URL is claimed.
    for (const name of ["chromium", "firefox", "webkit"])
      create("", "BrowserType", name, { name, executablePath: "" });
    create("", "Android", "android");
    create("", "Electron", "electron");
    create("", "Browser", "browser", { name: "chromium", browserName: "chromium", version: "runtime-reference-fill-v1" });
    create("browser", "Tracing", "tracing");
    create("browser", "Debugger", "debugger");
    create("browser", "APIRequestContext", "request", { tracing: channel("tracing") });
    create("browser", "BrowserContext", "context", { debugger: channel("debugger"),
      requestContext: channel("request"), tracing: channel("tracing"), options: {} });
    create("context", "Frame", "frame", { url: "", name: "", loadStates: [] });
    create("context", "Page", "page", { mainFrame: channel("frame"), isClosed: false });
    send({ guid: "page", method: "__adopt__", params: { guid: "frame" } });
    send({ guid: "context", method: "page", params: { page: channel("page") } });
    send({ guid: "browser", method: "context", params: { context: channel("context") } });
    create("", "Playwright", "playwright", { chromium: channel("chromium"), firefox: channel("firefox"),
      webkit: channel("webkit"), android: channel("android"), electron: channel("electron"),
      preLaunchedBrowser: channel("browser") });
  }
  async function run(work, timeout = 10000) {
    requireValue(!stopped, "operator_detached");
    requireValue(!active, "adapter_busy");
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), timeout);
    const signal = AbortSignal.any([controller.signal, lifetime.signal]);
    const pending = Promise.resolve().then(() => {
      signal.throwIfAborted();
      return work(signal);
    });
    active = pending;
    try {
      const result = await pending;
      requireValue(!signal.aborted, "operator_deadline_exceeded");
      return result;
    } finally { clearTimeout(timer); active = null; }
  }
  async function inspect() {
    requireValue(initialized, "adapter_initialization_required");
    return run(async signal => {
      bindings.clear(); expires = 0;
      const started = now();
      const snapshot = await client.inspect({ signal });
      requireValue(!signal.aborted && !stopped, "operator_detached");
      const next = new Map();
      const nodes = snapshot.nodes.map(node => {
        const selector = `runtime-ref=${snapshot.document_generation}:${node.ref}`;
        next.set(selector, { document_generation: snapshot.document_generation, ref: node.ref });
        return { ...node, selector };
      });
      bindings = next; expires = started + 30000;
      return { ...snapshot, nodes };
    });
  }
  async function dispatch(message) {
    const { guid, method, params } = message;
    if (guid === "" && method === "initialize") {
      requireValue(!initialized, "adapter_already_initialized");
      requireValue(only(params, ["sdkLanguage"]) && params.sdkLanguage === "javascript", "capability_unsupported");
      return run(async signal => {
        const status = await client.status({ signal });
        requireValue(status.status === "active", "operator_admission_inactive");
        requireValue(!stopped && !signal.aborted, "operator_detached");
        graph(); initialized = true;
        return { playwright: channel("playwright") };
      });
    }
    requireValue(initialized && guids.has(guid), "adapter_object_invalid");
    if (guid !== "frame" || method !== "fill") unsupported();
    requireValue(only(params, ["selector", "value", "strict", "timeout", "force"]) && params.strict === true &&
      (params.force === undefined || params.force === false) && typeof params.selector === "string" &&
      Number.isFinite(params.timeout) && params.timeout >= 0, "capability_unsupported");
    const binding = bindings.get(params.selector);
    requireValue(binding && now() < expires, "stale_inspection");
    requireValue(!writeUncertain, "operator_reconciliation_required");
    return run(async signal => {
      try {
        const result = await client.input({ ...binding, action: "fill", text: params.value }, { signal });
        // A late response is observation, not permission to retry this write.
        if (signal.aborted || result.outcome !== "completed")
          throw new BrowserOperatorError("operator_reconciliation_required", 409, { request_id: result.request_id });
        return {};
      } catch (error) {
        if (error.code === "operator_reconciliation_required" || signal.aborted) writeUncertain = true;
        if (/^[a-f0-9]{32}$/.test(error.details?.request_id || "")) lastUncertain = error.details.request_id;
        throw error;
      }
    }, Math.min(params.timeout || 10000, 10000));
  }
  async function receive(raw) {
    if (stopped) return;
    let message;
    try {
      requireValue(typeof raw === "string" && Buffer.byteLength(raw) <= 16384, "adapter_message_invalid");
      message = JSON.parse(raw);
      requireValue(only(message, ["id", "guid", "method", "params", "metadata"]) &&
        Number.isSafeInteger(message.id) && message.id > lastId && typeof message.guid === "string" &&
        message.guid.length <= 128 && typeof message.method === "string" && message.method.length <= 80 &&
        (message.params === undefined || object(message.params)), "adapter_message_invalid");
      lastId = message.id;
    } catch { void disconnect(); return; }
    try { send({ id: message.id, result: await dispatch(message) }); }
    catch (error) {
      const code = error instanceof BrowserOperatorError ? error.code : "adapter_failed";
      const requestId = error instanceof BrowserOperatorError && error.details?.request_id;
      if (typeof requestId === "string" && /^[a-f0-9]{32}$/.test(requestId)) lastUncertain = requestId;
      send({ id: message.id, error: { error: { name: "Error",
        message: lastUncertain && code === "operator_reconciliation_required" ? `${code}: request_id=${lastUncertain}` : code } } });
    }
  }
  function disconnect() {
    if (disconnectPromise) return disconnectPromise;
    stopped = true; lifetime.abort(); bindings.clear();
    disconnectPromise = (async () => {
      // Yield before releasing so reentrant transport-close callbacks see the
      // same promise. Closing a socket alone never confirms Runtime release.
      await Promise.resolve();
      await active?.catch(() => {});
      let result;
      try { result = await client.detach(); }
      catch (error) {
        result = { detached: false, page_closed: false, requires_reconciliation: true,
          code: error instanceof BrowserOperatorError ? error.code : "adapter_detach_unconfirmed",
          ...(lastUncertain ? { request_id: lastUncertain } : {}) };
      }
      resolveClosed(result);
      return result;
    })();
    try { peer?.close(); } catch { /* Runtime release still runs after transport failure. */ }
    return disconnectPromise;
  }
  function connect({ headers, send: sendWire, close }) {
    authenticate(headers);
    requireValue(!peer && !stopped && typeof sendWire === "function" && typeof close === "function", "adapter_connection_unavailable");
    peer = { send: sendWire, close };
    return Object.freeze({ receive, disconnect });
  }
  return Object.freeze({ connect, inspect, disconnect, closed, pageId: client.pageId,
    profile: PLAYWRIGHT_OPERATOR_CLIENT.profile });
}
