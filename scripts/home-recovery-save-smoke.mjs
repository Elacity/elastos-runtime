import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import vm from "node:vm";
import nodeTest from "node:test";

const test = (name, run) => nodeTest(name, { timeout: 2000 }, run);

const read = (file) => readFileSync(new URL(`../${file}`, import.meta.url), "utf8");
const windows = read("capsules/home-gui/browser/shell-windows.js");
const system = read("capsules/system/browser/system.js");
function fn(source, name) {
  const start = source.search(new RegExp(`(?:async )?function ${name}\\(`));
  assert(start >= 0, name);
  return source.slice(start, source.indexOf("\n}", start) + 2);
}
const tick = () => new Promise(setImmediate);

function hostFixture({ invalid = false, fail = false, ordinary = false } = {}) {
  const messages = [];
  const listeners = new Map();
  const timers = new Map();
  let desktop = false;
  let launches = 0;
  let loads;
  let requestSerial = 0;
  const frame = {
    contentWindow: { postMessage: (data, origin) => messages.push({ data, origin }) },
    dataset: { route: "/apps/system/#home_token=fixture" },
    addEventListener: (_name, listener) => { loads = listener; },
    removeEventListener: () => { loads = null; },
  };
  const entry = { id: "system-1", targetId: "system", node: { querySelector: () => invalid ? null : frame, remove() {} } };
  const target = { target: "system", window_policy: "single" };
  const state = { windows: new Map(), currentSummary: { targets: [target] } };
  let focusCount = 0;
  const hooks = { holdHomeSetupAct: () => false };
  const context = vm.createContext({
    pendingSystemRecoverySave: null, SYSTEM_APP_ID: "system", shellState: state,
    HOME_AGENT_TARGET_ID: "assistant", windowHooks: hooks,
    requireWindowHooks: () => hooks,
    targetById: () => target,
    pendingWindowLaunches: new Map(), ignoreRepeatedAction: () => false,
    browserWindowCount: () => state.windows.size,
    focusWindow: () => { focusCount += 1; },
    topBrowserWindowEntryForTarget: () => [...state.windows.values()][0],
    renderTargetLaunchError: (_target, error) => { throw error; }, console,
    cleanupFrameAutoFit() {},
    ensureDesktopForNewLaunch: () => { desktop = true; },
    async createBrowserTargetWindow(target, options) {
      assert(desktop, "Save from a fullscreen Space returns to a visible Desktop");
      assert.equal(target, "system");
      if (!ordinary) assert.deepEqual(Object.keys(options.query || {}), []);
      launches += 1;
      if (fail) throw new Error("launch unavailable");
      state.windows.set(entry.id, entry);
      return entry;
    },
    removeWindowEntries: (entries) => entries.forEach((item) => state.windows.delete(item.id)),
    browserLaunchAuthority: (route) => route ? { homeToken: "fixture" } : null,
    window: {
      crypto: { randomUUID: () => ++requestSerial === 1 ? "fresh-request" : `request-${requestSerial}` },
      addEventListener: (name, listener) => listeners.set(name, listener),
      removeEventListener: (name) => listeners.delete(name),
      setTimeout: (callback) => { timers.set(1, callback); return 1; },
      clearTimeout: (id) => timers.delete(id),
    },
  });
  vm.runInContext(["openSystemRecoverySave", "requestSystemWindow", "probeSystemWindow", "currentSystemWindowFrame", "handleSystemWindowMessage", "finishSystemWindowRequest", "hasExactKeys", "tearDownWindowEntry", "isSingleWindowTarget", "launchBrowserTargetWindow", "openTarget", "normalizedLaunchQuery", "launchActionKey"].map(name => fn(windows, name)).join("\n"), context);
  return { context, frame, entry, state, messages, timers, listeners,
    launches: () => launches,
    focused: () => focusCount,
    load: () => loads?.(),
    event(data, overrides = {}) {
      listeners.get("message")?.({ source: frame.contentWindow, origin: "null", data: { homeToken: "fixture", documentNonce: "document-one", ...data }, ...overrides });
    },
  };
}
const ready = { type: "elastos.system.window-ready.result/v1", requestId: "fresh-request" };
const result = { type: "elastos.system.window.result/v1", requestId: "fresh-request", ok: true };
for (const outcome of ["success", "cancel", "reload", "close", "replaced", "frame-replaced", "new-document", "timeout"]) {
  test(`Home Save settles ${outcome} with exact frame ownership`, async () => {
    const f = hostFixture();
    const pending = f.context.openSystemRecoverySave();
    assert.equal(f.context.openSystemRecoverySave(), pending, "repeated click shares one launch");
    await tick();
    assert.equal(f.launches(), 1);
    f.event(ready, { source: {} });
    f.event(ready, { origin: "https://wrong.example" });
    f.event({ ...ready, homeToken: "wrong" });
    f.event(result);
    assert.equal(f.messages.length, 1, "wrong sender and result-before-ready add no request beyond the probe");
    if (outcome === "timeout") {
      f.timers.get(1)();
    } else {
      f.event(ready);
      f.event(ready);
      assert.equal(f.messages.length, 2, "duplicate readiness does not repeat or cancel Save");
      assert.equal(f.messages[1].origin, "*");
      assert.equal(f.messages[1].data.type, "elastos.system.window.request/v1");
      assert.equal(f.timers.size, 0, "human passkey choice is not timed out");
      f.load();
      if (outcome === "reload") f.event({ type: "home:app-unloading" });
      if (outcome === "close") f.context.tearDownWindowEntry(f.entry);
      if (outcome === "replaced") f.frame.dataset.route = "/apps/system/#home_token=replaced";
      if (outcome === "frame-replaced") f.entry.node.querySelector = () => ({ ...f.frame });
      if (outcome === "new-document") f.event({ ...ready, documentNonce: "document-two" });
      f.event({ ...result, documentNonce: "stale-document" });
      f.event({ ...result, ok: outcome !== "cancel" });
    }
    assert.equal(await pending, outcome === "success");
    f.event(ready);
    assert.equal(f.listeners.size, 0);
    assert.equal(f.timers.size, 0);
  });
}
for (const options of [{ invalid: true }, { fail: true }]) {
  test(`Home Save rejects ${JSON.stringify(options)}`, async () => {
    const f = hostFixture(options);
    assert.equal(await f.context.openSystemRecoverySave(), false);
    assert.equal(f.state.windows.size, 0, "invalid launch leaves no orphan window");
    assert.equal(f.context.pendingSystemRecoverySave, null);
  });
}
assert(!fn(windows, "snapshotBrowserSession").includes("RecoverySave"), "Save intent is never persisted");

test("ordinary System Open consumes the Runtime single policy", async () => {
  const f = hostFixture({ ordinary: true });
  f.context.openTarget("system");
  await tick();
  f.context.openTarget("system");
  await tick();
  assert.equal(f.launches(), 1, "ordinary Open focuses the existing System window");
  assert.equal(f.focused(), 2);
});

test("concurrent System Open, settings and Save entry points share a launch", async () => {
  const f = hostFixture({ ordinary: true });
  f.context.openTarget("system");
  f.context.openTarget("system", { query: { settings: "about" } });
  const save = f.context.openSystemRecoverySave();
  await tick();
  f.event(ready);
  f.event(result);
  await tick();
  f.event({ ...ready, requestId: "request-2" });
  f.event({ ...result, requestId: "request-2" });
  assert.equal(await save, true);
  assert.equal(f.launches(), 1, "different entry points share the existing pending launch");
});

test("sequential Home Saves reuse the ready System document with fresh requests", async () => {
  const f = hostFixture();
  const first = f.context.openSystemRecoverySave();
  await tick();
  f.event(ready);
  f.event(result);
  assert.equal(await first, true);
  const second = f.context.openSystemRecoverySave();
  await tick();
  assert.equal(f.launches(), 1, "completed Save keeps one System frame");
  f.event({ ...ready, requestId: "request-2" });
  assert.equal(f.messages.length, 4, "a reused document answers a fresh probe without a new load event");
  const nextRequest = f.messages[3].data;
  assert.notEqual(nextRequest.requestId, result.requestId);
  let settled = false;
  second.then(() => { settled = true; });
  f.event(result);
  await tick();
  assert.equal(settled, false, "old result cannot complete the next request");
  f.event({ ...result, requestId: nextRequest.requestId });
  assert.equal(await second, true);
  assert.equal(f.timers.size, 0);
});

function systemFixture(nonce = "document-one", download = async () => true) {
  const listeners = new Map();
  const messages = [];
  let initialized;
  let exports = 0;
  let focused = 0;
  let activeTab = "account";
  let importFocused = 0;
  const parent = { postMessage: (data, origin) => messages.push({ data, origin }) };
  const context = vm.createContext({
    frameHomeToken: "fixture", apiHomeToken: "fixture",
    recoveryDocumentNonce: nonce,
    initialRecoveryState: new Promise((resolve) => { initialized = resolve; }),
    window: { parent, top: {}, addEventListener: (name, callback) => listeners.set(name, callback) },
    activateSettingsTab: (tab) => { activeTab = tab; },
    recoveryDownloadButton: { focus: () => { focused += 1; } },
    recoveryImportInput: { focus: () => { importFocused += 1; } },
    onRecoveryDownload: async () => { exports += 1; return download(); },
    showRecoveryNote() {},
  });
  vm.runInContext(system.slice(system.indexOf('const SETTINGS_SEARCH_KEYWORDS'), system.indexOf('const requestedSettingsTab'))
    + ["readText", "normalizedRequestedSettingsTab", "hasExactKeys", "configureHomeRecoverySave"].map(name => fn(system, name)).join('\n'), context);
  context.configureHomeRecoverySave();
  return { context, initialized, messages, exports: () => exports, focused: () => focused,
    activeTab: () => activeTab, importFocused: () => importFocused,
    hide: () => listeners.get("pagehide")(),
    send: (data, overrides = {}) => listeners.get("message")({ source: parent, origin: "null", data, ...overrides }),
  };
}
const request = { type: "elastos.system.window.request/v1", homeToken: "fixture", requestId: "fresh-request", documentNonce: "document-one", sequence: 1, action: "save", query: {} };
for (const cancelled of [false, true]) {
  test(`System accepts explicit sequential Save requests; unloaded=${cancelled}`, async () => {
    const f = systemFixture();
    await f.send(request, { source: {} });
    await f.send(request, { origin: "https://wrong.example" });
    await f.send({ ...request, homeToken: "wrong" });
    await f.send({ ...request, documentNonce: "stale-document" });
    await f.send({ ...request, requestId: "" });
    await f.send({ ...request, extra: true });
    assert.equal(f.exports(), 0);
    const pending = f.send(request);
    await f.send(request);
    assert.equal(f.exports(), 0, "ready handlers still await verified initial state");
    if (cancelled) f.hide();
    f.initialized();
    await pending;
    assert.equal(f.exports(), cancelled ? 0 : 1);
    assert.equal(f.focused(), cancelled ? 0 : 1);
    await f.send({ ...request, requestId: "second-request", sequence: 2 });
    assert.equal(f.exports(), cancelled ? 0 : 2, "each explicit request may save in the current document");
    await f.send(request);
    await f.send({ ...request, requestId: "second-request", sequence: 2 });
    assert.equal(f.exports(), cancelled ? 0 : 2, "completed request replay never repeats an export");
  });
}
test("System rejects a concurrent Save and permits a later explicit request after cancellation", async () => {
  let complete;
  const f = systemFixture("document-one", () => new Promise((resolve) => { complete = resolve; }));
  f.initialized();
  const first = f.send(request);
  await tick();
  await f.send(request);
  await f.send({ ...request, requestId: "overlapping-request", sequence: 2 });
  assert.equal(f.exports(), 1, "one passkey decision at a time");
  assert.equal(f.messages.at(-1).data.requestId, 'overlapping-request');
  assert.equal(f.messages.at(-1).data.ok, false, 'busy rejection settles the exact fresh caller');
  complete(false);
  await first;
  assert.equal(f.messages.at(-1).data.ok, false);
  const second = f.send({ ...request, requestId: "later-request", sequence: 3 });
  await tick();
  assert.equal(f.exports(), 2, "cancellation does not consume this document permanently");
  complete(true);
  await second;
  assert.equal(f.messages.at(-1).data.requestId, "later-request");
});
test("restored System rejects an old-document Save", async () => {
  const restored = systemFixture("document-two");
  restored.initialized();
  await restored.send(request);
  await tick();
  assert.equal(restored.exports(), 0, "reload rejects queued old-document Save; URL and restore alone never export");
});

test("queued navigation callers and Save settle without overwriting request ownership", async () => {
  const f = hostFixture({ ordinary: true });
  f.state.windows.set(f.entry.id, f.entry);
  const calls = [
    f.context.requestSystemWindow(f.entry, "navigate", { settings: "about" }),
    f.context.requestSystemWindow(f.entry, "navigate", { settings: "account" }),
    f.context.requestSystemWindow(f.entry, "navigate", { settings: "security" }),
    f.context.openSystemRecoverySave(),
  ];
  for (let index = 0; index < calls.length; index += 1) {
    await tick();
    const id = index === 0 ? 'fresh-request' : `request-${index + 1}`;
    assert.equal(f.entry.systemRequest.requestId, id);
    f.event({ ...ready, requestId: id });
    f.event({ ...result, requestId: id });
  }
  assert.deepEqual(await Promise.all(calls), [true, true, true, true]);
  assert.equal(f.entry.systemRequest, null);
  assert.equal(f.timers.size, 0);
  assert.equal(f.listeners.size, 0);
});

test("an old document unload cannot cancel a request bound to the new document", async () => {
  const f = hostFixture();
  const pending = f.context.openSystemRecoverySave();
  await tick();
  f.event({ ...ready, documentNonce: 'document-two' });
  f.event({ type: 'home:app-unloading', documentNonce: 'document-one' });
  assert(f.entry.systemRequest, 'new document still owns its request');
  f.event({ ...result, documentNonce: 'document-one' });
  assert(f.entry.systemRequest, 'old result cannot complete it');
  f.event({ ...result, documentNonce: 'document-two' });
  assert.equal(await pending, true);
});

for (const [query, valid] of [
  [{ settings: 'about' }, true],
  [{ settings: 'security', recovery: 'import' }, true],
  [{ settings: 'unknown' }, false],
  [{ settings: ' security ' }, false],
  [{ settings: 'about', recovery: 'import' }, false],
  [{ settings: 'security', recovery: 'export' }, false],
  [{ settings: 'security', extra: 'value' }, false],
  [{ settings: 'security', home_token: 'replacement' }, false],
]) {
  test(`System validates explicit navigation ${JSON.stringify(query)}`, async () => {
    const f = systemFixture();
    f.initialized();
    await f.send({ ...request, action: 'navigate', query });
    assert.equal(f.messages.at(-1).data.ok, valid);
    assert.equal(f.activeTab(), valid ? query.settings : 'account');
    assert.equal(f.importFocused(), valid && query.recovery === 'import' ? 1 : 0);
    assert.equal(f.exports(), 0, 'navigation never exports a kit');
  });
}

test("System readiness answers a fresh challenge only after initial verified state", async () => {
  const f = systemFixture();
  const probe = { type: 'elastos.system.window-ready.request/v1', homeToken: 'fixture', requestId: 'challenge' };
  await f.send(probe, { source: {} });
  await f.send(probe, { origin: 'https://wrong.example' });
  const pending = f.send(probe);
  await tick();
  assert.equal(f.messages.length, 0);
  f.initialized();
  await pending;
  assert.equal(f.messages[0].data.requestId, 'challenge');
  assert.equal(f.messages[0].data.documentNonce, 'document-one');
  assert.equal(f.exports(), 0);
});

for (const query of [[], null, '', 1]) {
  test(`System rejects malformed Save query ${JSON.stringify(query)}`, async () => {
    const f = systemFixture();
    f.initialized();
    await f.send({ ...request, query });
    assert.equal(f.exports(), 0);
    assert.equal(f.messages.at(-1).data.ok, false);
  });
}
const boot = fn(system, "boot");
assert(boot.indexOf("configureHomeRecoverySave()") < boot.indexOf("homeClipboard.start()"));
assert.equal((system.match(/type: "home:app-ready"/g) || []).length, 1, "parent readiness uses the existing Clipboard contract once");
