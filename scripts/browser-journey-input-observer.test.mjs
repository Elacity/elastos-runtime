import assert from "node:assert/strict";
import { EventEmitter } from "node:events";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";

const source = readFileSync(new URL("./home-passkey-virtual-auth-smoke.mjs", import.meta.url), "utf8");
const start = source.indexOf("async function observeControlledBrowserInput(");
const end = source.indexOf("\nasync function runControlledBrowserJourney(", start);
assert.ok(start >= 0 && end > start);
const declaration = source.slice(start, end);
const plain = value => JSON.parse(JSON.stringify(value));
function deferred() {
  let resolve;
  const promise = new Promise(done => { resolve = done; });
  return { promise, resolve };
}

function surface({ setupGate, setupFailure = false, fastDeadline = false } = {}) {
  const page = new EventEmitter(), keys = new Set(), timers = new Set();
  const pageId = "page:vz-owned", token = "private-authority-token";
  const href = "http://localhost:61510/apps/browser/?browser_instance=owned#home_token=private-authority-token";
  const state = { pageId, href, status: "Browser remote-display input channel is not open.",
    active: { id: "browser-keyboard-capture", value: "private-text" }, disposed: 0 };
  const doc = { addEventListener: (_, fn) => keys.add(fn), removeEventListener: (_, fn) => keys.delete(fn),
    get activeElement() { return state.active; }, hasFocus: () => true, body: { dataset: { loading: "false" } },
    querySelector: selector => selector === "#browser-status" ? { textContent: state.status } : { disabled: false } };
  const frame = { url: () => href,
    evaluateHandle: async (fn, args) => {
      if (setupFailure) throw new Error("private setup payload");
      const observer = vm.runInNewContext(`(${fn.toString()})(args)`, { args, URL, performance, document: doc,
        window: { get __elastosBrowserCurrentPageId() { return state.pageId; } },
        location: { get href() { return state.href; } },
        setTimeout: fn => { timers.add(fn); return fn; }, clearTimeout: fn => timers.delete(fn) });
      if (setupGate) await setupGate.promise;
      return { evaluate: async (callback, arg) => callback(observer, arg), dispose: async () => { state.disposed++; } };
    } };
  const observe = vm.runInNewContext(`(${declaration})`, { URL, performance, clearTimeout,
    setTimeout: fastDeadline ? fn => setTimeout(fn, 10) : setTimeout });
  const request = ({ event = { type: "paste_text", text: "private-text" },
    url = `http://localhost:61510/api/apps/browser/pages/${encodeURIComponent(pageId)}/input`,
    sourceFrame = frame, method = "POST", headers = { origin: "null", "x-elastos-home-token": token }, malformed = false } = {}) => ({
      frame: () => sourceFrame, url: () => url, method: () => method, headers: () => headers,
      postDataJSON: () => { if (malformed) throw new Error("private body"); return { event, token, sdp: "private-sdp" }; },
    });
  const response = (req, body = {}, status = 200) => ({ request: () => req, status: () => status,
    json: async () => ({ schema: "elastos.browser.input-result/v1", page_id: pageId, accepted: true,
      token, sdp: "private-sdp", event: { text: "private-text" }, ...body }) });
  const key = (overrides = {}) => {
    for (const fn of keys) fn({ target: state.active, key: "B", defaultPrevented: true,
      ctrlKey: false, metaKey: false, altKey: false, shiftKey: true, ...overrides });
  };
  const clean = () => {
    assert.equal(page.eventNames().length, 0);
    assert.equal(keys.size, 0);
    assert.equal(timers.size, 0);
  };
  return { page, frame, pageId, token, state, keys, timers, request, response, key, clean,
    observe: () => observe(page, frame, token, pageId) };
}

test("input observer admits only the exact Browser frame, Runtime origin, page and opaque authority", async () => {
  const f = surface(), observer = await f.observe();
  const inputUrl = `http://localhost:61510/api/apps/browser/pages/${encodeURIComponent(f.pageId)}/input`;
  for (const options of [
    { sourceFrame: {} }, { url: inputUrl.replace(":61510", ":61511") },
    { url: inputUrl.replace("localhost", "foreign.invalid") },
    { url: inputUrl.replace(encodeURIComponent(f.pageId), "page%3Aforeign") },
    { url: inputUrl.replace("/input", "/status") }, { method: "GET" },
    { headers: { origin: "http://foreign.invalid", "x-elastos-home-token": f.token } },
    { headers: { origin: "null", "x-elastos-home-token": "foreign-token" } },
  ]) {
    const req = f.request(options); f.page.emit("request", req); f.page.emit("response", f.response(req));
  }
  const click = f.request({ event: { type: "click", x: 286, y: 220 } });
  const text = f.request({ headers: { origin: "http://localhost:61510", "x-elastos-home-token": f.token } });
  for (const req of [click, text]) { f.page.emit("request", req); f.page.emit("response", f.response(req)); }
  const evidence = plain(await observer.stop(false));
  assert.deepEqual(evidence.requests.filter(x => x.phase === "request").map(x => x.event_type), ["click", "paste_text"]);
  const requests = evidence.requests.filter(x => x.phase === "request"), responses = evidence.requests.filter(x => x.phase === "response");
  assert.equal(requests[1].text_length, "private-text".length);
  assert.deepEqual(requests.map(x => x.request_id), responses.map(x => x.request_id));
  assert.ok(responses.every(x => x.accepted && x.page_matches && x.schema_matches && x.status === 200));
  assert.ok(evidence.requests.every(x => Number.isFinite(x.at_ms) && x.at_ms >= 0));
  assert.equal(evidence.viewer, undefined);
  assert.equal(evidence.observer.drained, true);
  assert.equal(evidence.observer.stopped, true);
  assert.doesNotMatch(JSON.stringify(evidence), /private-|home_token|sdp|286/);
  f.clean();
});

test("stop drains an outstanding HTTP response and its body without admitting another request", async () => {
  const f = surface(), observer = await f.observe(), body = deferred();
  const req = f.request(); f.page.emit("request", req);
  let done = false;
  const stopped = observer.stop(false).then(value => { done = true; return value; });
  await Promise.resolve(); assert.equal(done, false);
  const other = f.request(); f.page.emit("request", other); f.page.emit("response", f.response(other));
  f.page.emit("response", { ...f.response(req), json: () => body.promise });
  await Promise.resolve(); assert.equal(done, false);
  body.resolve({ schema: "elastos.browser.input-result/v1", page_id: f.pageId, accepted: true });
  const evidence = await stopped;
  assert.deepEqual(plain(evidence.requests.map(x => x.phase)), ["request", "response"]);
  assert.equal(evidence.observer.drained, true);
  assert.equal(await observer.stop(true), evidence);
  f.page.emit("request", req); assert.equal(evidence.requests.length, 2);
  assert.equal(f.state.disposed, 1); f.clean();
});

test("failed, malformed and mismatched input responses stay explicit and discard payloads", async () => {
  const f = surface(), observer = await f.observe();
  const failed = f.request(); f.page.emit("request", failed); f.page.emit("requestfailed", failed);
  const malformed = f.request({ malformed: true }); f.page.emit("request", malformed);
  f.page.emit("response", { ...f.response(malformed, {}, 500), json: async () => { throw new Error("private body error"); } });
  const wrong = f.request(); f.page.emit("request", wrong);
  f.page.emit("response", f.response(wrong, { schema: "wrong", page_id: "other", accepted: false }, 403));
  const evidence = plain(await observer.stop(false));
  assert.equal(evidence.requests[1].phase, "failed");
  assert.ok(evidence.requests.some(x => x.event_type === "other" && x.status === 500 && x.body_unavailable));
  assert.ok(evidence.requests.some(x => x.status === 403 && !x.accepted && !x.page_matches && !x.schema_matches));
  assert.doesNotMatch(JSON.stringify(evidence), /private-|other".*page_id/); f.clean();
});

test("failure captures passive key targets and fixed viewer status without text or keys", async () => {
  const f = surface(), observer = await f.observe();
  f.key({ key: "secret-key-value", target: { id: "private-selector" } });
  f.key();
  const evidence = plain(await observer.stop(true));
  assert.equal(evidence.viewer.status, "input_channel_unavailable");
  assert.equal(evidence.viewer.owner_matches, true);
  assert.equal(evidence.viewer.active_target, "browser-keyboard-capture");
  assert.equal(evidence.viewer.keys[0].target, "other");
  assert.equal(evidence.viewer.keys[0].printable, false);
  assert.equal(evidence.viewer.keys[1].printable, true);
  assert.equal(evidence.viewer.keys[1].default_prevented, true);
  assert.doesNotMatch(JSON.stringify(evidence), /private-|secret-key-value/); f.clean();
});

for (const change of ["page", "instance", "origin"]) test(`failure snapshot rejects replacement ${change}`, async () => {
  const f = surface(), observer = await f.observe();
  f.key();
  if (change === "page") f.state.pageId = "other";
  if (change === "instance") f.state.href = f.state.href.replace("instance=owned", "instance=other");
  if (change === "origin") f.state.href = f.state.href.replace(":61510", ":61511");
  f.key(); f.state.status = "private backend response";
  assert.deepEqual(plain((await observer.stop(true)).viewer), { owner_matches: false }); f.clean();
});

test("request and key floods remain bounded", async () => {
  const f = surface(), observer = await f.observe();
  for (let i = 0; i < 30; i++) { const req = f.request(); f.page.emit("request", req); f.page.emit("response", f.response(req)); f.key(); }
  const evidence = await observer.stop(true);
  assert.equal(evidence.requests.length, 32);
  assert.equal(evidence.dropped_requests, 14);
  assert.equal(evidence.viewer.keys.length, 8);
  assert.equal(evidence.viewer.dropped_keys, 22); f.clean();
});

test("missing response drains within its bound and late data cannot change evidence", async () => {
  const f = surface({ fastDeadline: true }), observer = await f.observe();
  const req = f.request(); f.page.emit("request", req);
  const evidence = await observer.stop(true);
  assert.equal(evidence.observer.drained, false);
  const before = JSON.stringify(evidence);
  f.page.emit("response", f.response(req)); assert.equal(JSON.stringify(evidence), before); f.clean();
});

test("late body after the drain deadline cannot append evidence", async () => {
  const f = surface({ fastDeadline: true }), observer = await f.observe(), body = deferred();
  const req = f.request(); f.page.emit("request", req);
  const stopped = observer.stop(true);
  f.page.emit("response", { ...f.response(req), json: () => body.promise });
  const evidence = await stopped, before = JSON.stringify(evidence);
  body.resolve({ accepted: true, text: "private-late-body" });
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(JSON.stringify(evidence), before); f.clean();
});

test("failed observer setup remains diagnostic and removes network listeners", async () => {
  const f = surface({ setupFailure: true }), observer = await f.observe();
  const evidence = await observer.stop(true);
  assert.equal(evidence.observer.setup_failed, true);
  assert.equal(evidence.observer.stopped, false);
  assert.deepEqual(plain(evidence.viewer), { unavailable: true }); f.clean();
});

test("a handle returned after setup timeout and stop is removed and disposed", async () => {
  const gate = deferred(), f = surface({ setupGate: gate, fastDeadline: true });
  const observer = await f.observe();
  assert.equal(observer.evidence.observer.setup_failed, true);
  await observer.stop(true);
  gate.resolve(); await new Promise(resolve => setImmediate(resolve));
  assert.equal(f.state.disposed, 1); f.clean();
});

test("late setup during response drain is disposed instead of escaping the stop snapshot", async () => {
  const gate = deferred(), f = surface({ setupGate: gate, fastDeadline: true });
  const observer = await f.observe(), req = f.request();
  f.page.emit("request", req);
  const stopped = observer.stop(true);
  gate.resolve(); await new Promise(resolve => setImmediate(resolve));
  f.page.emit("requestfailed", req);
  await stopped;
  assert.equal(f.state.disposed, 1); f.clean();
});

test("unknown viewer status is categorized without retaining a reflected payload", async () => {
  const f = surface(), observer = await f.observe();
  f.state.status = "private backend payload home_token=secret SDP candidate:private";
  const evidence = await observer.stop(true);
  assert.equal(evidence.viewer.status, "other");
  assert.doesNotMatch(JSON.stringify(evidence), /private|secret|home_token|SDP/); f.clean();
});
