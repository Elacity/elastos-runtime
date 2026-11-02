import assert from "node:assert/strict";
import fs from "node:fs";
import test from "node:test";
import vm from "node:vm";
import { setImmediate as nextTurn } from "node:timers/promises";
import { SelkiesPage } from "./browser-selkies-control-service.mjs";

const videoSdp = "v=0\r\nm=video 9 UDP/TLS/RTP/SAVPF 96\r\n";
const audioSdp = "v=0\r\nm=audio 9 UDP/TLS/RTP/SAVPF 111\r\n";
const source = fs.readFileSync(new URL("./browser-selkies-control-service.mjs", import.meta.url), "utf8");
const slice = (start, end) => source.slice(source.indexOf(start), source.indexOf(end));
function deferred() { let resolve, reject; const promise = new Promise((a, b) => { resolve = a; reject = b; }); return { promise, resolve, reject }; }

function launch() {
  const config = { selkiesWsUrl: new URL("ws://127.0.0.1:1"), browserControl: { timeoutMs: 20000 },
    displaySurface: { stream: { width: 1280, height: 720 } }, iceServers: [] };
  const page = new SelkiesPage(config, { page_id: "page:test", stream_id: "stream:test", adapter: "test",
    engine: "chromium", url: "https://requested.test/" }, () => {});
  const calls = [], released = [];
  const browserPage = deferred(), video = deferred();
  page.openLaunchBrowserPage = () => { calls.push("page"); return browserPage.promise; };
  page.releaseLaunchBrowserPage = async value => { released.push(value); };
  page.openWebRtcSession = () => { calls.push("video"); return video.promise; };
  page.openAudioWebRtcSession = async () => { calls.push("audio"); return { sdp: { sdp: audioSdp } }; };
  return { page, calls, released, browserPage, video };
}

test("display signaling starts while the initial page is still opening", async () => {
  const l = launch();
  const opened = l.page.open();
  await nextTurn();
  assert.deepEqual(l.calls, ["page", "video"], "video SDP must not wait for the page");
  l.video.resolve({ sdp: { sdp: videoSdp } });
  await nextTurn();
  assert.deepEqual(l.calls, ["page", "video", "audio"]);
  l.browserPage.resolve({ url: "https://final.test/", title: "Final" });
  const result = await opened;
  assert.equal(result.actual_url, "https://final.test/");
  assert.equal(result.title, "Final");
  assert.equal(result.display_session.initial_offer.sdp, videoSdp);
  assert.equal(l.page.browserPage.page_id, "page:test");
  assert.equal(l.page.displayAvailable, true);
});

test("a signaling failure closes the page that finishes opening later", async () => {
  const l = launch();
  const opened = l.page.open();
  await nextTurn();
  l.video.reject(new Error("selkies offer timed out"));
  await assert.rejects(opened, /selkies offer timed out/);
  l.page.close(); // The guest open handler closes a failed page.
  const late = { url: "https://late.test/", title: "Late" };
  l.browserPage.resolve(late);
  await nextTurn(); await nextTurn();
  assert.deepEqual(l.released, [late]);
  assert.ok(!l.page.browserPage, "a closed session must not adopt the late page");
});

test("a page open failure is reported once signaling settles, without an unhandled rejection", async () => {
  const l = launch();
  const opened = l.page.open();
  l.browserPage.reject(new Error("browser CDP initial navigation failed: net::ERR_NAME_NOT_RESOLVED"));
  await nextTurn(); await nextTurn(); // An unhandled rejection would fail this test here.
  l.video.resolve({ sdp: { sdp: videoSdp } });
  await assert.rejects(opened, /ERR_NAME_NOT_RESOLVED/);
  assert.equal(l.page.displayAvailable, false);
});

test("the launch navigation returns after DOMContentLoaded without waiting for load", async () => {
  const waits = [];
  const context = vm.createContext({ console: { error() {} },
    isRetryableRuntimeNavigationError: () => false, summarizeNetworkFailure: value => value, isNetworkChangedError: () => false });
  const wait = vm.runInContext(`${slice("async function waitForInitialNavigation(", "\nasync function observeInitialNavigation(")}\nwaitForInitialNavigation`, context);
  const cdp = { waitForEvent: async method => { waits.push(method); return {}; } };
  await wait(cdp, { timeoutMs: 20000 }, {}, "initial", { awaitLoad: false });
  assert.deepEqual(waits, ["Page.domContentEventFired"]);
  await wait(cdp, { timeoutMs: 20000 }, {}, "command");
  assert.deepEqual(waits.slice(1), ["Page.domContentEventFired", "Page.loadEventFired"], "user navigations keep the load wait");
});

function refresher() {
  const handlers = [], reads = [], logs = [];
  const state = { url: "https://final.test/after-load", title: "Loaded title" };
  const context = vm.createContext({ setTimeout, clearTimeout, JSON,
    console: { error: line => logs.push(JSON.parse(line)) },
    isBrowserErrorUrl: value => String(value).startsWith("chrome-error://"),
    readBrowserPageState: async () => { reads.push(1); return { ...state }; } });
  const refresh = vm.runInContext(`${slice("function refreshBrowserPageStateAfterLoad(", "\nfunction validatePasteText(")}\nrefreshBrowserPageStateAfterLoad`, context);
  const cdp = { closed: false, events: [], onEvent(method, handler) {
    handlers.push({ method, handler });
    return () => { handlers.splice(handlers.findIndex(entry => entry.handler === handler), 1); };
  } };
  const browserPage = { _cdp: cdp, url: "https://final.test/", title: "Early title", navigationInProgress: false };
  return { refresh, cdp, browserPage, handlers, reads, logs, state, fire: () => handlers.forEach(entry => entry.handler({})) };
}

test("the deferred load event refreshes the cached URL and title for page status", async () => {
  const r = refresher();
  r.refresh(r.cdp, r.browserPage, { timeoutMs: 20000 });
  assert.deepEqual(r.handlers.map(entry => entry.method), ["Page.loadEventFired"]);
  r.fire();
  await nextTurn();
  assert.equal(r.browserPage.url, r.state.url);
  assert.equal(r.browserPage.title, "Loaded title");
  assert.equal(r.handlers.length, 0, "the load handler is removed once settled");
  assert.equal(r.logs[0].deferred, true);
});

test("a load event that arrived before the refresh started settles at once", async () => {
  const r = refresher();
  r.cdp.events.push({ method: "Page.loadEventFired", params: {} });
  r.refresh(r.cdp, r.browserPage, { timeoutMs: 20000 });
  await nextTurn();
  assert.equal(r.browserPage.title, "Loaded title");
});

for (const change of ["navigation", "closed", "retargeted", "error-page"]) {
  test(`the deferred refresh leaves the cache alone after ${change}`, async () => {
    const r = refresher();
    r.refresh(r.cdp, r.browserPage, { timeoutMs: 20000 });
    if (change === "navigation") r.browserPage.navigationInProgress = true;
    if (change === "closed") r.cdp.closed = true;
    if (change === "retargeted") r.browserPage._cdp = {};
    if (change === "error-page") r.state.url = "chrome-error://chromewebdata/";
    r.fire();
    await nextTurn();
    assert.equal(r.browserPage.title, "Early title");
  });
}
