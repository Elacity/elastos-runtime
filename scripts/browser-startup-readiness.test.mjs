import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";

const source = readFileSync(new URL("../capsules/browser/browser/browser.js", import.meta.url), "utf8");
const html = readFileSync(new URL("../capsules/browser/browser/index.html", import.meta.url), "utf8");
const input = html.match(/<input\b[^>]*\bid="browser-url"[^>]*>/)?.[0];
const startup = source.slice(source.lastIndexOf("const requestedStartupUrl ="));
function deferred() {
  let resolve, reject;
  const promise = new Promise((ok, fail) => { resolve = ok; reject = fail; });
  return { promise, resolve, reject };
}
function start({ expired = false, params = new URLSearchParams() } = {}) {
  const summary = deferred(), open = deferred();
  const address = { disabled: /\sdisabled(?:\s|=|\/?>)/.test(input), value: "" };
  const calls = [], messages = [];
  const readiness = { loading: false };
  const setLoading = loading => { readiness.loading = loading; address.disabled = loading; };
  const settled = vm.runInNewContext(startup, {
    unloadCleanupStarted: false, homeWindowCloseInFlight: false, homeWindowTerminalCloseConfirmed: false,
    params, DEFAULT_URL: "https://ela.city/", addressInput: address,
    updateNavState() {}, setLoading, fetchBrowserSummary: () => summary.promise,
    restoreRuntimePageViewer: async () => false,
    requestRuntimeOpen: async url => { calls.push(url); setLoading(true); try { await open.promise; } finally { setLoading(false); } },
    isAuthoritySessionError: () => expired, requestHomeRelaunch: () => { calls.push("renew-authority"); return true; },
    friendlyOpenError: error => error.message, showStatus: message => messages.push(message),
  });
  return { summary, open, address, readiness, calls, messages, settled };
}

test("address entry waits for its controller before the Browser module starts", () => {
  assert.ok(input);
  assert.match(input, /\sdisabled(?:\s|=|\/?>)/);
});

test("a Home window without a URL keeps the address field for the first ordinary submit", async () => {
  const state = start();
  assert.equal(state.address.disabled, true);
  assert.equal(state.readiness.loading, true);
  assert.deepEqual(state.calls, []);
  state.summary.resolve({});
  await state.settled;
  assert.deepEqual(state.calls, []);
  assert.equal(state.address.disabled, false);
});

test("a launch URL still opens after the initial summary", async () => {
  const state = start({ params: new URLSearchParams("url=https://ela.city/") });
  assert.deepEqual(state.calls, []);
  state.summary.resolve({});
  await new Promise(setImmediate);
  assert.deepEqual(state.calls, ["https://ela.city/"]);
  assert.equal(state.address.disabled, true);
  state.open.resolve();
  await state.settled;
  assert.equal(state.address.disabled, false);
});

test("initial summary failure enables retry after showing the failure", async () => {
  const state = start();
  state.summary.reject(new Error("Runtime summary unavailable"));
  await state.settled;
  assert.deepEqual(state.calls, []);
  assert.deepEqual(state.messages, ["Runtime summary unavailable"]);
  assert.equal(state.address.disabled, false);
});

test("expired initial authority leaves controls pending while Home renews it", async () => {
  const state = start({ expired: true });
  state.summary.reject(new Error("Expired authority"));
  await state.settled;
  assert.deepEqual(state.calls, ["renew-authority"]);
  assert.equal(state.address.disabled, true);
});
