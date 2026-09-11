import assert from "node:assert/strict";
import fs from "node:fs";
import test from "node:test";
import vm from "node:vm";
import crypto from "node:crypto";
import http from "node:http";
import { MinimalWebSocketClient } from "./browser-selkies-control-service.mjs";

const source = fs.readFileSync(new URL("./browser-selkies-control-service.mjs", import.meta.url), "utf8");

test("the existing guest page route negotiates inspection for the acquired Engine page", async () => {
  const start = source.indexOf("      const pageMatch = url.pathname.match(");
  const end = source.indexOf("      const body = await readJsonRequest(req", start);
  assert.ok(start > 0 && end > start);
  const ownedPage = { pageId: "owned-page", closed: false, browserPage: { debugger_url: "ws://private/page/1" } };
  const calls = [];
  const context = vm.createContext({
    pages: new Map([[ownedPage.pageId, ownedPage]]),
    config: {},
    httpJson: (_response, status, body) => calls.push({ status, body }),
    browserInspectionCapabilities: (page) => {
      assert.equal(page, ownedPage);
      return { schema: "elastos.browser.inspect-capabilities/v1", page_id: page.pageId };
    },
  });
  const route = vm.runInContext(`(async function(req, url, res) { ${source.slice(start, end)} })`, context);
  await route({ method: "GET" }, new URL("http://guest/pages/owned-page/inspect"), {});
  assert.equal(calls.length, 1);
  assert.equal(calls[0].status, 200);
  assert.equal(calls[0].body.schema, "elastos.browser.inspect-capabilities/v1");
  assert.equal(calls[0].body.page_id, ownedPage.pageId);
});

function inspectionHarness(count = 5) {
  let now = 1, active = true, intercepted = null;
  const frame = { id: "root", loaderId: "loader-1", url: "https://website.test/main" };
  const calls = [], handlers = new Map(), timers = new Set();
  const nodes = Array.from({ length: count }, (_, i) => ({
    nodeId: `ax-${i}`, backendDOMNodeId: i + 100, ignored: false,
    role: { value: "button" }, name: { value: `Website control ${i}` },
  }));
  const privateClients = [];
  const domTypes = new Map();
  const cdp = {
    closed: false,
    onEvent(method, callback) {
      handlers.set(method, callback);
      return () => handlers.delete(method);
    },
    async request(method, params, timeout) {
      calls.push({ method, params, timeout });
      if (intercepted) await intercepted(method);
      if (method === "Page.getFrameTree") return { frameTree: { frame: { ...frame } } };

      return {};
    },
  };
  const page = { pageId: "page:owned", closed: false,
    browserPage: { debugger_url: "ws://private/actual-website", _cdp: cdp },
    displaySession: { identity: "retained-media" }, launchRequest: { principal_id: "owner" } };
  class InspectionCdp {
    constructor(url, timeout, options) {
      assert.equal(url, page.browserPage.debugger_url);
      assert.equal(options.maxIncomingBytes, 262144);
      assert.equal(options.retainEvents, false);
      this.closed = true; privateClients.push(this);
    }
    async connect() { this.closed = false; }
    close() { this.closed = true; }
    async request(method, params, timeout) {
      calls.push({ method, params, timeout });
      if (intercepted) await intercepted(method);
      if (method === "Accessibility.getRootAXNode") return { node: { ...structuredClone(nodes[0]), childIds: nodes.slice(1).map(n => n.nodeId) } };
      if (method === "Accessibility.getChildAXNodes") return { nodes: structuredClone(nodes.slice(1)) };
      if (method === "DOM.describeNode") return { node: { backendNodeId: params.backendNodeId, nodeName: "INPUT", attributes: ["type", domTypes.get(params.backendNodeId) || "text"] } };
      return {};
    }
  }
  const context = vm.createContext({ crypto, Buffer, CdpClient: InspectionCdp, performance: { now: () => now },
    setTimeout: (callback, ms) => { const timer = { callback, ms, unref() {} }; timers.add(timer); return timer; },
    clearTimeout: timer => timers.delete(timer),
    withBrowserCdp: async (browserPage, _timeout, action) => {
      assert.equal(browserPage, page.browserPage);
      return action(browserPage._cdp);
    },
  });
  vm.runInContext(source.slice(source.indexOf("const BROWSER_INSPECTION_LIMITS ="), source.indexOf("async function withBrowserCdp(")), context);
  return { page, nodes, cdp, frame, calls, handlers, timers, privateClients, domTypes,
    run: (options = {}) => context.inspectBrowserPage(page, { schema: "elastos.browser.inspect-request/v1", ...options }, () => active),
    advance: ms => { now += ms; }, retire: () => { active = false; },
    intercept: callback => { intercepted = callback; },
  };
}

test("inspection paginates a retained website snapshot without touching display, input or navigation", async () => {
  const h = inspectionHarness();
  const display = h.page.displaySession;
  const first = await h.run({ limit: 2 });
  const second = await h.run({ limit: 2, cursor: first.next_cursor });
  const third = await h.run({ limit: 2, cursor: second.next_cursor });
  assert.deepEqual([...first.nodes, ...second.nodes, ...third.nodes].map(n => n.name), h.nodes.map(n => n.name.value));
  assert.equal(second.document_generation, first.document_generation);
  assert.equal(third.next_cursor, null);
  assert.equal(h.calls.filter(c => c.method === "Accessibility.getRootAXNode").length, 1);
  assert.ok(h.calls.every(c => ["Page.enable", "Page.getFrameTree", "Accessibility.enable", "Accessibility.disable",
    "Accessibility.getRootAXNode", "Accessibility.getChildAXNodes"].includes(c.method)));
  assert.equal(h.privateClients.length, 1);
  assert.equal(h.privateClients[0].closed, true);
  assert.equal(h.cdp.closed, false);
  assert.equal(h.page.browserPage._inspection.snapshot.backendNodes[2].backendDOMNodeId, 102);
  assert.ok(!JSON.stringify(first).includes("backendDOMNodeId"));
  assert.equal(h.page.displaySession, display);
  assert.equal(h.page.closed, false);
  assert.equal(h.timers.size, 1);
  for (const timer of h.timers) timer.callback();
  assert.equal(h.page.browserPage._inspection.snapshot, null);
});

for (const event of ["Page.frameNavigated", "Page.navigatedWithinDocument", "Page.frameStartedLoading"]) {
  test(`${event} invalidates a cursor even when URL and loader return to their previous values`, async () => {
    const h = inspectionHarness();
    const first = await h.run({ limit: 2 });
    h.handlers.get(event)({ frame: { ...h.frame }, frameId: h.frame.id });
    await assert.rejects(h.run({ cursor: first.next_cursor }), { code: "stale_inspection" });
    assert.notEqual((await h.run()).document_generation, first.document_generation);
  });
}

test("a child-frame event leaves the top document cursor intact", async () => {
  const h = inspectionHarness();
  const first = await h.run({ limit: 2 });
  h.handlers.get("Page.frameNavigated")({ frame: { id: "child", parentId: "root", loaderId: "child-loader" } });
  assert.equal((await h.run({ cursor: first.next_cursor })).document_generation, first.document_generation);
});

test("expired, foreign-page and replaced-snapshot cursors fail closed", async () => {
  const h = inspectionHarness();
  const first = await h.run({ limit: 2 });
  await assert.rejects(inspectionHarness().run({ cursor: first.next_cursor }), { code: "stale_inspection" });
  h.advance(30001);
  await assert.rejects(h.run({ cursor: first.next_cursor }), { code: "stale_inspection" });
  const second = await h.run({ limit: 2 });
  await h.run();
  await assert.rejects(h.run({ cursor: second.next_cursor }), { code: "stale_inspection" });
});

for (const change of ["close", "owner", "navigation", "deadline"]) {
  test(`${change} during actual collection prevents snapshot publication`, async () => {
    const h = inspectionHarness();
    h.intercept(method => {
      if (method !== "Accessibility.getRootAXNode") return;
      if (change === "close") h.page.closed = true;
      if (change === "owner") h.retire();
      if (change === "navigation") h.frame.loaderId = "new-loader";
      if (change === "deadline") h.advance(1501);
    });
    await assert.rejects(h.run(), { code: change === "navigation" ? "stale_inspection" : change === "deadline" ? "inspection_failed" : "inspection_owner_changed" });
    assert.equal(h.page.browserPage._inspection.snapshot, null);
    assert.equal(h.timers.size, 0);
    assert.equal(h.page.browserPage._inspection.busy, false);
  });
}

test("one pending collection bounds concurrent work and a failed read releases its slot", async () => {
  const h = inspectionHarness();
  let release, entered;
  const waiting = new Promise(resolve => { entered = resolve; });
  h.intercept(async method => {
    if (method === "Accessibility.getRootAXNode") { entered(); await new Promise(resolve => { release = resolve; }); }
  });
  const first = h.run(); await waiting;
  await assert.rejects(h.run(), { code: "inspection_busy" });
  release(); await first;
  h.intercept(() => { throw new Error("private CDP detail"); });
  await assert.rejects(h.run(), error => error.code === "inspection_failed" && !error.message.includes("private"));
  assert.equal(h.page.browserPage._inspection.busy, false);
});

test("request, retained snapshot and response byte limits are enforced", async () => {
  const h = inspectionHarness(64);
  await assert.rejects(h.run({ schema: "future" }), { code: "inspection_unsupported" });
  for (const options of [{ limit: 0 }, { limit: 65 }, { principal_id: "foreign" }, { cursor: "bad" }]) {
    await assert.rejects(h.run(options), { code: "invalid_inspection" });
  }
  assert.equal(h.calls.length, 0);
  for (const node of h.nodes) node.name.value = "界".repeat(256);
  const first = await h.run();
  assert.ok(Buffer.byteLength(JSON.stringify(first)) <= 32768);
  assert.ok(first.next_cursor && first.nodes.length < 64);
  for (const node of h.nodes) {
    node.description = { value: "界".repeat(1000) };
    node.value = { value: "界".repeat(1000) };
  }
  assert.equal((await h.run()).truncated, true);
  assert.ok(Buffer.byteLength(JSON.stringify(h.page.browserPage._inspection.snapshot)) <= 131072);
});


test("native AX role/name/ignored semantics replace DOM tag guesses and sensitive values stay private", async () => {
  const h = inspectionHarness(5);
  h.nodes[0].role.value = "generic"; // Chromium's A-without-href role is retained verbatim.
  h.nodes[0].name.value = "Engine-computed label";
  h.nodes[1].ignored = true; // Includes CSS-hidden/aria-hidden native AX nodes.
  h.nodes[1].name.value = "private hidden text";
  h.nodes[2].value = { value: "private password" };
  h.domTypes.set(102, "password");
  h.nodes[3].value = { value: "private file path" };
  h.domTypes.set(103, "file");
  h.nodes[4].value = { value: "Visible input" };
  const result = await h.run();
  assert.equal(result.nodes.length, 4);
  assert.equal(result.nodes[0].role, "generic");
  assert.equal(result.nodes[0].name, "Engine-computed label");
  assert.equal(result.nodes.at(-1).value, "Visible input");
  assert.ok(!JSON.stringify(result).includes("private"));
  assert.equal(h.privateClients[0].closed, true);
});

test("unsupported native AX fails explicitly and closes only its inspection connection", async () => {
  const h = inspectionHarness();
  h.intercept(method => { if (method === "Accessibility.enable") throw Object.assign(new Error("Method not found"), { cdpCode: -32601 }); });
  await assert.rejects(h.run(), { code: "inspection_unsupported" });
  assert.equal(h.privateClients[0].closed, true);
  assert.equal(h.cdp.closed, false);
  assert.equal(h.page.closed, false);
});

test("an oversized native reply closes the bounded inspection socket while the existing connection keeps working", { timeout: 3000 }, async t => {
  const sockets = new Map();
  const server = http.createServer();
  server.on("upgrade", (request, socket) => {
    sockets.set(request.url, socket);
    socket.on("error", () => {});
    socket.on("end", () => socket.end());
    socket.resume();
    socket.write("HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n");
  });
  let inspected, existing;
  t.after(async () => {
    inspected?.close(); existing?.close();
    for (const socket of sockets.values()) socket.destroy();
    if (server.listening) await new Promise(resolve => server.close(resolve));
  });
  await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  const base = `ws://127.0.0.1:${server.address().port}`;
  inspected = new MinimalWebSocketClient(new URL(`${base}/inspection`), { maxIncomingBytes: 64 });
  existing = new MinimalWebSocketClient(new URL(`${base}/existing`));
  await existing.connect(500); await inspected.connect(500);
  const failed = new Promise(resolve => inspected.onError(resolve));
  const ended = new Promise(resolve => sockets.get("/inspection").once("close", resolve));
  // The length header alone exceeds the limit; the large payload is never sent.
  sockets.get("/inspection").write(Buffer.from([0x81, 126, 0x10, 0x00]));
  assert.match((await failed).message, /frame is too large/);
  await ended;
  assert.equal(inspected.closed, true);
  assert.equal(existing.closed, false);
  const received = new Promise(resolve => existing.onText(resolve));
  sockets.get("/existing").write(Buffer.from([0x81, 2, 0x6f, 0x6b]));
  assert.equal(await received, "ok");
});
