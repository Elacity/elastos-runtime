import assert from "node:assert/strict";
import fs from "node:fs";
import test from "node:test";
import vm from "node:vm";

const source = fs.readFileSync(new URL("./browser-selkies-control-service.mjs", import.meta.url), "utf8");
function declaration(name) {
  const start = source.search(new RegExp(`(?:async )?function ${name}\\(`));
  const end = source.slice(start).search(/\n\}(?=\n|$)/);
  assert.ok(start >= 0 && end > 0, `${name} must remain executable`);
  return source.slice(start, start + end + 2);
}

function harness({ cached = true, initiallyClosed = false, failRequest = null,
  failure = new Error("browser CDP WebSocket closed after request"), markClosed = false,
  connectFailure = null } = {}) {
  const requests = [], clients = [], interceptions = [];
  let constructed = 0, connected = 0, failed = false;
  function client(label) {
    const cdp = {
      label, closed: false, closes: 0,
      async request(method, params = {}) {
        requests.push({ client: label, method, params: JSON.parse(JSON.stringify(params)) });
        // This records an effectful request before its lost reply. It does not
        // claim a real browser or native effect was observed.
        if (!failed && failRequest?.(method, params, label)) {
          failed = true;
          if (markClosed) this.closed = true;
          throw failure;
        }
        return { client: label };
      },
      close() { this.closes++; this.closed = true; },
    };
    clients.push(cdp);
    return cdp;
  }
  const cachedClient = cached ? client("cached") : null;
  if (cachedClient) cachedClient.closed = initiallyClosed;
  const page = { debugger_url: "ws://fixture.invalid/page", _cdp: cachedClient };
  class CdpClient {
    constructor(url, timeout) {
      assert.equal(url, page.debugger_url);
      assert.equal(timeout, 1000);
      constructed++;
      const fresh = client("fresh");
      fresh.connect = async (wait) => {
        assert.equal(wait, 1000);
        connected++;
        if (connectFailure) { fresh.closed = true; throw connectFailure; }
      };
      return fresh;
    }
  }
  const context = vm.createContext({
    Error, CdpClient,
    ensureBrowserFileChooserInterception: async (cdp, current) => {
      assert.equal(current, page);
      interceptions.push(cdp.label);
    },
  });
  vm.runInContext(["ensureBrowserCdpDomains", "withBrowserCdp", "finiteCoordinate", "keyEventDefinition", "dispatchBrowserInputEvent"].map(declaration).join("\n"), context);
  return {
    page, cachedClient, clients, requests, interceptions, failure,
    constructed: () => constructed, connected: () => connected,
    run: (event, supplied = null) => context.dispatchBrowserInputEvent(page, event, 1000, supplied),
    observe: (action) => context.withBrowserCdp(page, 1000, action),
  };
}
const wheel = { type: "wheel", x: 0, y: 30, delta_x: 2, delta_y: 80 };
const click = { type: "click", x: 20, y: 30 };
const effectRequests = (h) => h.requests.filter(({ method }) => method.startsWith("Input.dispatch") || method === "Input.insertText");
const isMouse = (type) => (method, params) => method === "Input.dispatchMouseEvent" && params.type === type;
const isKey = (type) => (method, params) => method === "Input.dispatchKeyEvent" && params.type === type;
const failures = [
  { name: "wheel", event: wheel, at: isMouse("mouseWheel") },
  { name: "click press", event: click, at: isMouse("mousePressed") },
  { name: "click release", event: click, at: isMouse("mouseReleased") },
  { name: "text insertion", event: { type: "key", key: "a" }, at: method => method === "Input.insertText" },
  { name: "named key down", event: { type: "key", key: "Enter" }, at: isKey("keyDown") },
];
for (const { name, event, at } of failures) {
  test(`ordinary ${name} reports a cached connection failure without replay`, async () => {
    const h = harness({ failRequest: at });
    const outcome = await h.run(event).then(() => ({ succeeded: true }), error => ({ error }));
    assert.equal(h.requests.filter(r => at(r.method, r.params)).length, 1, "effectful request is attempted once");
    assert.equal(outcome.error, h.failure, "the original failure reaches the caller");
    assert.equal(h.constructed(), 0, "uncertain input cannot create a retry connection");
    assert.equal(h.cachedClient.closes, 1);
    assert.equal(h.page._cdp, null);
    if (event.type === "click") {
      assert.deepEqual(effectRequests(h).map(r => r.params.type), ["mouseMoved", "mousePressed", "mouseReleased"]);
      assert.ok(effectRequests(h).every(r => r.client === "cached"), "release cleanup stays on the original client");
    }
    if (name === "named key down") {
      // The existing helper does not reach keyUp after keyDown rejects. This
      // repair prevents replay; it does not prove native key-state rollback.
      assert.deepEqual(effectRequests(h).map(r => r.params.type), ["keyDown"]);
    }
  });
}

test("closed cached client suppresses replay even when the error text is not a connection marker", async () => {
  const failure = new Error("reply outcome unavailable");
  const h = harness({ failure, markClosed: true, failRequest: isMouse("mouseWheel") });
  const outcome = await h.run(wheel).then(() => ({ succeeded: true }), error => ({ error }));
  assert.equal(effectRequests(h).length, 1);
  assert.equal(outcome.error, failure);
  assert.equal(h.constructed(), 0);
  assert.equal(h.cachedClient.closes, 1);
  assert.equal(h.page._cdp, null);
});

test("nonconnection input error retains the live cache and original error", async () => {
  const failure = new Error("input rejected by page");
  const h = harness({ failure, failRequest: isMouse("mouseWheel") });
  await assert.rejects(h.run(wheel), error => error === failure);
  assert.equal(effectRequests(h).length, 1);
  assert.equal(h.cachedClient.closes, 0);
  assert.equal(h.page._cdp, h.cachedClient);
  assert.equal(h.constructed(), 0);
});

test("cached setup connection failure is explicit rather than guessed safe to replay", async () => {
  const h = harness({ failRequest: method => method === "Page.enable" });
  const outcome = await h.run(wheel).then(() => ({ succeeded: true }), error => ({ error }));
  assert.equal(effectRequests(h).length, 0);
  assert.equal(outcome.error, h.failure);
  assert.equal(h.constructed(), 0);
  assert.equal(h.cachedClient.closes, 1);
  assert.equal(h.page._cdp, null);
});

for (const [name, options, connections, label] of [
  ["live cached", {}, 0, "cached"],
  ["fresh", { cached: false }, 1, "fresh"],
  ["initially closed cached", { initiallyClosed: true }, 1, "fresh"],
]) {
  test(`${name} connection can dispatch one successful input`, async () => {
    const h = harness(options);
    await h.run(wheel);
    assert.equal(h.constructed(), connections);
    assert.equal(h.connected(), connections);
    assert.equal(effectRequests(h).length, 1);
    assert.equal(effectRequests(h)[0].client, label);
    assert.deepEqual(effectRequests(h)[0].params, { type: "mouseWheel", x: 0, y: 30, deltaX: 2, deltaY: 80 });
    assert.equal(h.page._cdp.label, label);
    assert.deepEqual(h.interceptions, [label]);
    if (options.initiallyClosed) assert.equal(h.requests.some(r => r.client === "cached"), false);
  });
}

test("initial connection failure reports its error without an input or second connection", async () => {
  const failure = new Error("browser CDP WebSocket error during connect");
  const h = harness({ cached: false, connectFailure: failure });
  await assert.rejects(h.run(wheel), error => error === failure);
  assert.equal(h.constructed(), 1);
  assert.equal(h.connected(), 1);
  assert.equal(h.requests.length, 0);
  assert.equal(h.clients[0].closes, 1);
  assert.equal(h.page._cdp, null);
});

test("first input on a fresh connection also reports uncertainty without another connection", async () => {
  const h = harness({ cached: false, failRequest: isMouse("mouseWheel") });
  await assert.rejects(h.run(wheel), error => error === h.failure);
  assert.equal(h.constructed(), 1);
  assert.equal(effectRequests(h).length, 1);
  assert.equal(h.page._cdp, null);
});

test("generic observation keeps its existing cached-connection retry", async () => {
  const h = harness({ failRequest: method => method === "Page.getLayoutMetrics" });
  const result = await h.observe(cdp => cdp.request("Page.getLayoutMetrics"));
  assert.equal(result.client, "fresh");
  assert.deepEqual(h.requests.map(r => r.client), ["cached", "fresh"]);
  assert.equal(h.cachedClient.closes, 1);
  assert.equal(h.constructed(), 1);
  assert.equal(h.connected(), 1);
  assert.equal(effectRequests(h).length, 0);
});

for (const rejects of [false, true]) {
  test(`supplied CDP ${rejects ? "failure" : "success"} keeps its existing caller-owned path`, async () => {
    const h = harness();
    const calls = [];
    const supplied = { async request(method, params) { calls.push({ method, params }); if (rejects && method === "Input.dispatchMouseEvent") throw h.failure; } };
    if (rejects) await assert.rejects(h.run(wheel, supplied), error => error === h.failure);
    else await h.run(wheel, supplied);
    assert.deepEqual(calls.map(r => r.method), ["Input.setIgnoreInputEvents", "Input.dispatchMouseEvent"]);
    assert.equal(h.constructed(), 0);
    assert.equal(h.requests.length, 0);
    assert.equal(h.interceptions.length, 0);
    assert.equal(h.page._cdp, h.cachedClient);
    assert.equal(h.cachedClient.closes, 0);
  });
}
