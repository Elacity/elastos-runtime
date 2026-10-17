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
const plain = value => JSON.parse(JSON.stringify(value));
const wheel = { type: "wheel", x: 0, y: 30, delta_x: 2, delta_y: 80 };
const tick = () => new Promise(resolve => setImmediate(resolve));

async function harness(t) {
  const calls = [], sockets = [], logs = [], requestErrors = [];
  let hook = null, revision = 0;
  class Socket {
    constructor() { this.id = sockets.length; this.closed = true; sockets.push(this); }
    async connect() { this.closed = false; }
    onText(callback) { this.text = callback; }
    onError(callback) { this.error = callback; }
    onClose(callback) { this.end = callback; }
    close() { if (!this.closed) { this.closed = true; this.end?.(); } }
    sendText(raw) {
      const request = JSON.parse(raw);
      const call = { socket: this.id, method: request.method, params: request.params };
      calls.push(call);
      let result = {};
      if (request.method === "Runtime.evaluate") {
        result = { result: { value: JSON.stringify({ url: `https://fixture.test/${revision}`, title: `Page ${revision}` }) } };
      }
      if (request.method === "Page.getNavigationHistory") result = { currentIndex: revision, entries: [{ id: 1 }, { id: 2 }, { id: 3 }] };
      const reply = value => queueMicrotask(() => this.text(JSON.stringify({ id: request.id, result: value ?? result })));
      const reject = message => queueMicrotask(() => this.text(JSON.stringify({ id: request.id, error: { message, code: -32000 } })));
      if (hook?.({ ...call, reply, reject, socketObject: this }) !== "hold") reply();
    }
    event(method, params) { this.text(JSON.stringify({ method, params })); }
  }
  const context = vm.createContext({
    Error, URL, console, setTimeout, clearTimeout, MinimalWebSocketClient: Socket,
    nowIso: () => "2026-10-16T00:00:00.000Z",
    logControlEvent: (name, fields) => logs.push({ name, fields: plain(fields) }),
  });
  const start = source.indexOf("class CdpClient {");
  const end = source.indexOf("\nasync function fetchBrowserControlJson", start);
  assert.ok(start >= 0 && end > start);
  const names = ["createBrowserFileChooserState", "browserFileChooserState", "summarizeBrowserFileChooser",
    "noteBrowserFileChooserOpened", "ensureBrowserFileChooserInterception", "withBrowserCdp",
    "closeCachedBrowserCdp", "finiteCoordinate", "keyEventDefinition", "dispatchBrowserInputEvent",
    "browserPageStateFromCdp", "cachedBrowserPageState", "refreshBrowserPageState"];
  // The unchanged-source negative control has no readiness helper; its actual
  // dispatcher/refresh declarations remain executable without any substitute.
  if (source.includes("async function ensureBrowserCdpDomains(")) names.push("ensureBrowserCdpDomains");
  vm.runInContext(source.slice(start, end) + "\n" + names.map(declaration).join("\n") + "\nglobalThis.Client = CdpClient;", context);
  const clients = [];
  const client = async () => {
    const cdp = new context.Client("ws://fixture.invalid/page", 1000);
    clients.push(cdp);
    const request = cdp.request.bind(cdp);
    cdp.request = (...args) => request(...args).catch(error => { requestErrors.push(error); throw error; });
    await cdp.connect(1000);
    return cdp;
  };
  const initial = await client();
  const page = { debugger_url: "ws://fixture.invalid/page", _cdp: initial, url: "https://fixture.test/start", title: "Start" };
  t.after(() => {
    for (const cdp of clients) cdp.close();
    // Includes clients constructed by the actual withBrowserCdp wrapper.
    for (const socket of sockets) socket.close();
  });
  return {
    context, page, initial, calls, sockets, logs, requestErrors, client,
    hook: value => { hook = value; }, revision: value => { revision = value; },
    input: (event = wheel, supplied = null) => context.dispatchBrowserInputEvent(page, event, 1000, supplied),
    refresh: () => context.refreshBrowserPageState(page, 1000),
    close: () => context.closeCachedBrowserCdp(page),
    count: method => calls.filter(call => call.method === method).length,
  };
}

test("sequential input and refresh reuse domains while retaining exact input and fresh page state", async t => {
  const h = await harness(t);
  const states = [];
  for (let index = 0; index < 3; index++) {
    h.revision(index);
    await h.input({ ...wheel, delta_y: 20 + index });
    states.push(plain(await h.refresh()));
  }
  assert.equal(h.count("Input.setIgnoreInputEvents"), 3);
  assert.equal(h.count("Runtime.evaluate"), 3);
  assert.equal(h.count("Page.getNavigationHistory"), 3);
  assert.deepEqual(h.calls.filter(c => c.method === "Input.dispatchMouseEvent").map(c => c.params), [20, 21, 22].map(deltaY => ({ type: "mouseWheel", x: 0, y: 30, deltaX: 2, deltaY })));
  assert.deepEqual(states.map(s => [s.url, s.title, s.can_go_back, s.can_go_forward]), [
    ["https://fixture.test/0", "Page 0", false, true],
    ["https://fixture.test/1", "Page 1", true, true],
    ["https://fixture.test/2", "Page 2", true, false],
  ]);
  assert.equal(h.count("Page.setInterceptFileChooserDialog"), 1);
  assert.equal(h.initial.eventHandlers.get("Page.fileChooserOpened").length, 1);
  assert.equal(h.count("Page.enable"), 1);
  assert.equal(h.count("Runtime.enable"), 1);
});

test("concurrent callers share pending domain work and wait for both successful replies", async t => {
  const h = await harness(t), pages = [], runtimes = [];
  h.hook(call => {
    if (call.method === "Page.enable") { pages.push(call); return "hold"; }
    if (call.method === "Runtime.enable") { runtimes.push(call); return "hold"; }
  });
  const first = h.input(), second = h.refresh();
  await tick();
  const pageCount = pages.length;
  assert.equal(h.count("Input.dispatchMouseEvent"), 0);
  assert.equal(h.count("Runtime.evaluate"), 0);
  for (const call of pages) call.reply();
  await tick();
  const runtimeCount = runtimes.length;
  assert.equal(h.count("Input.dispatchMouseEvent"), 0);
  for (const call of runtimes) call.reply();
  await Promise.all([first, second]);
  assert.equal(pageCount, 1, "one Page.enable while preparation is pending");
  assert.equal(runtimeCount, 1, "one Runtime.enable while preparation is pending");
  assert.equal(h.count("Input.dispatchMouseEvent"), 1);
  assert.equal(h.count("Runtime.evaluate"), 1);
  // Chooser's existing concurrent installation policy is outside this cache.
});

for (const method of ["Page.enable", "Runtime.enable"]) {
  test(`${method} failure permits complete preparation retry without input replay`, async t => {
    const h = await harness(t);
    let failed = false;
    h.hook(call => { if (!failed && call.method === method) { failed = true; call.reject("preparation rejected"); return "hold"; } });
    await assert.rejects(h.input(), error => error === h.requestErrors[0] && error.message === "preparation rejected");
    assert.equal(h.count("Input.dispatchMouseEvent"), 0);
    assert.equal(h.page._cdp, h.initial);
    await h.input();
    await h.input();
    assert.equal(h.count("Page.enable"), 2);
    assert.equal(h.count("Runtime.enable"), method === "Page.enable" ? 1 : 2);
    assert.equal(h.count("Input.dispatchMouseEvent"), 2);
  });
}

test("concurrent failed preparation rejects every waiter and a later call retries both domains", async t => {
  const h = await harness(t), held = [];
  h.hook(call => { if (call.method === "Runtime.enable") { held.push(call); return "hold"; } });
  const results = Promise.allSettled([h.input(), h.refresh()]);
  await tick();
  const count = held.length;
  for (const call of held) call.reject("shared preparation failure");
  const outcomes = await results;
  assert.ok(outcomes.every(outcome => outcome.status === "rejected" && outcome.reason.message === "shared preparation failure"));
  assert.equal(h.count("Input.dispatchMouseEvent"), 0);
  h.hook(null);
  await h.input();
  assert.equal(count, 1);
  assert.equal(h.count("Page.enable"), 2);
  assert.equal(h.count("Runtime.enable"), 2);
});

test("chooser failure retries chooser without repeating successful domain preparation", async t => {
  const h = await harness(t);
  let failed = false;
  h.hook(call => { if (!failed && call.method === "Page.setInterceptFileChooserDialog") { failed = true; call.reject("chooser unavailable"); return "hold"; } });
  await assert.rejects(h.input(), /chooser unavailable/);
  assert.equal(h.count("Input.dispatchMouseEvent"), 0);
  await h.input();
  assert.equal(h.count("Page.enable"), 1);
  assert.equal(h.count("Runtime.enable"), 1);
  assert.equal(h.count("Page.setInterceptFileChooserDialog"), 2);
  assert.equal(h.initial.eventHandlers.get("Page.fileChooserOpened").length, 1);
});

test("a chooser event remains observable after warm input and metadata refresh", async t => {
  const h = await harness(t);
  await h.input();
  await h.refresh();
  h.sockets[0].event("Page.fileChooserOpened", { mode: "selectMultiple", backendNodeId: 42, frameId: "frame" });
  await tick();
  const state = plain(await h.refresh());
  assert.equal(state.file_chooser.pending, true);
  assert.equal(state.file_chooser.request_id, "file-chooser:1");
  assert.equal(state.file_chooser.multiple, true);
  assert.equal(h.page.file_chooser.pending.backend_node_id, 42);
  assert.equal(h.logs.filter(log => log.name === "file_chooser_opened").length, 1);
  assert.equal(h.count("Page.setInterceptFileChooserDialog"), 1);
});

test("closed cached connection is replaced and new connection prepares independently", async t => {
  const h = await harness(t);
  await h.input();
  h.close();
  await h.input();
  await h.refresh();
  assert.notEqual(h.page._cdp, h.initial);
  assert.equal(h.sockets.length, 2);
  assert.deepEqual(h.calls.filter(c => c.method === "Page.enable").map(c => c.socket), [0, 1]);
  assert.deepEqual(h.calls.filter(c => c.method === "Input.dispatchMouseEvent").map(c => c.socket), [0, 1]);
});

for (const pendingMethod of ["Page.enable", "Runtime.enable"]) {
test(`closing pending ${pendingMethod} prevents old input and leaves replacement preparation intact`, async t => {
  const h = await harness(t), held = [];
  h.hook(call => { if (call.socket === 0 && call.method === pendingMethod) { held.push(call); return "hold"; } });
  const first = h.input().then(() => null, error => error);
  await tick();
  h.close();
  await h.input();
  const replacement = h.page._cdp;
  const error = await first;
  assert.match(error.message, /WebSocket closed/);
  for (const call of held) call.reply();
  await h.refresh();
  assert.equal(h.page._cdp, replacement);
  assert.deepEqual(h.calls.filter(c => c.method === "Input.dispatchMouseEvent").map(c => c.socket), [1]);
  assert.equal(h.calls.filter(c => c.socket === 1 && c.method === "Page.enable").length, 1);
});

}

test("quiescent same-client connect resets only domain readiness", async t => {
  const h = await harness(t);
  await h.input();
  h.initial.close();
  await h.initial.connect(1000);
  h.page._cdp = h.initial;
  await h.input();
  await h.refresh();
  assert.equal(h.count("Page.enable"), 2);
  assert.equal(h.count("Runtime.enable"), 2);
  assert.equal(h.count("Input.dispatchMouseEvent"), 2);
  // Production reconnects with a new client. This only checks domain-cache
  // hygiene, not general same-client reconnect support or chooser reattachment.
});

for (const rejects of [false, true]) {
  test(`old preparation ${rejects ? "rejection" : "completion"} cannot replace or clear newer readiness`, async t => {
    const h = await harness(t);
    let held = null;
    h.hook(call => {
      if (!held && call.method === "Runtime.enable") { held = call; return "hold"; }
    });
    const first = h.input().then(() => null, error => error);
    await tick();
    // connect() invalidates the readiness field. The controlled socket lets
    // the old reply arrive later to falsify stale cache mutation/continuation.
    // This is not qualification of overlapping same-client real reconnects.
    await h.initial.connect(1000);
    await h.input();
    if (rejects) held.reject("late preparation failure");
    else held.reply();
    const error = await first;
    assert.ok(error, "a superseded preparation cannot continue into input");
    assert.match(error.message, rejects ? /late preparation failure/ : /preparation was replaced/);
    if (rejects) assert.equal(error, h.requestErrors[0]);
    await h.refresh();
    assert.equal(h.count("Input.dispatchMouseEvent"), 1);
    assert.equal(h.count("Page.enable"), 2, "old settlement must not invalidate the new successful pair");
    assert.equal(h.count("Runtime.enable"), 2);
  });
}

test("closure after enable replies is checked before downstream chooser or input", async t => {
  const h = await harness(t);
  h.hook(call => {
    if (call.method === "Runtime.enable") {
      call.reply();
      queueMicrotask(() => h.initial.close());
      return "hold";
    }
  });
  await assert.rejects(h.input(), /WebSocket closed/);
  assert.equal(h.count("Page.setInterceptFileChooserDialog"), 0);
  assert.equal(h.count("Input.dispatchMouseEvent"), 0);
  assert.equal(h.page._cdp, null);
});

test("supplied CDP retains caller-owned preparation and exact operation calls", async t => {
  const h = await harness(t), calls = [];
  await h.input(wheel, { request: async (method, params) => { calls.push({ method, params: plain(params) }); } });
  assert.deepEqual(calls.map(call => call.method), ["Input.setIgnoreInputEvents", "Input.dispatchMouseEvent"]);
  assert.equal(h.calls.length, 0);
});

test("fresh metadata failure remains an error after preparation has succeeded", async t => {
  const h = await harness(t);
  await h.input();
  h.hook(call => { if (call.method === "Runtime.evaluate") { call.reject("page evaluation unavailable"); return "hold"; } });
  await assert.rejects(h.refresh(), /page evaluation unavailable/);
  h.hook(null); h.revision(2);
  assert.equal((await h.refresh()).title, "Page 2");
  assert.equal(h.count("Page.enable"), 1);
  assert.equal(h.count("Runtime.evaluate"), 2);
});
