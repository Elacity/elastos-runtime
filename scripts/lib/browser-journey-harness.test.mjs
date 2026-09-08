import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";
import { createBrowserJourneyFixture } from "./browser-journey-fixture.mjs";
import { readBrowserViewerReloadDocument } from "./browser-journey-viewer-reload.mjs";
import { browserJourneyTargetConfig, readBrowserJourneyReceipt,
  browserJourneyEngineChoice, browserJourneyEngineRoute } from "./browser-journey-target.mjs";

const source = readFileSync(new URL("../home-passkey-virtual-auth-smoke.mjs", import.meta.url), "utf8");
function harnessFunction(name, globals = {}) {
  const start = source.search(new RegExp(`(?:async )?function ${name}\\(`));
  assert.ok(start >= 0, name);
  const next = source.slice(start + 1).search(/\n(?:async )?function /);
  const declaration = source.slice(start, start + 1 + next);
  return vm.runInNewContext(`(${declaration})`, { URL, URLSearchParams, Date, performance,
    CHECK_BROWSER_CONTROLLED_MEDIA: false, CHECK_BROWSER_CONTROLLED_INSPECTION: false,
    CHECK_BROWSER_CONTROLLED_OPERATOR: false, CHECK_BROWSER_CONTROLLED_JOURNEY: false,
    BROWSER_REMOTE_EXIT_ID: "", browserQualification: null, qualificationCancellation: null,
    BROWSER_JOURNEY_TARGET: browserJourneyTargetConfig(), browserJourneyEngineChoice, browserJourneyEngineRoute,
    readBrowserJourneyReceipt: (config, run, options) => readBrowserJourneyReceipt(config, run,
      { ...options, fetchImpl: globals.fetch || fetch }), ...globals });
}

test("controlled fixture isolates runs, records bounded events and rejects malformed requests", async () => {
  const server = createBrowserJourneyFixture();
  await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  const base = `http://127.0.0.1:${server.address().port}`;
  const run = "test-run-123";
  const event = { type: "input", page: "nav", value: "Browser-é", scroll_x: 0, scroll_y: 640,
    input_rect: { x: 32, y: 150, width: 480, height: 60 } };
  const post = body => fetch(`${base}/events?run=${run}`, {
    method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body),
  });
  try {
    assert.equal((await fetch(`${base}/health`).then(r => r.json())).ok, true);
    assert.equal((await fetch(`${base}/receipt?run=${run}`)).status, 404);
    assert.equal((await fetch(`${base}/main?run=<script>`)).status, 400);
    const html = await fetch(`${base}/main?run=${run}`).then(r => r.text());
    assert.match(html, /id="journey-input"/);
    assert.match(html, /min-height: 3600px/);
    assert.match(html, /\/nav\?run=test-run-123/);
    // Parse the actual page program without starting a browser.
    new vm.Script(html.match(/<script>([\s\S]*)<\/script>/)[1]);
    assert.equal((await fetch(`${base}/nav?run=${run}`)).status, 200);
    assert.equal((await post(null)).status, 400);
    assert.equal((await post({ ...event, value: "x".repeat(97) })).status, 400);
    assert.equal((await post({ ...event, value: "x".repeat(5000) })).status, 413);
    assert.equal((await post({ ...event, secret: "discard-me" })).status, 200);
    const receipt = await fetch(`${base}/receipt?run=${run}`).then(r => r.json());
    assert.equal(receipt.run, run);
    assert.equal(receipt.events[0].value, "Browser-é");
    assert.equal(receipt.events[0].secret, undefined);
    assert.equal(receipt.events[0].scroll_y, 640);
    assert.equal((await fetch(`${base}/receipt?run=another-run`)).status, 404);
    for (let i = 1; i < 128; i++) assert.equal((await post(event)).status, 200);
    assert.equal((await post(event)).status, 429);
    assert.equal((await fetch(`${base}/receipt?run=${run}`).then(r => r.json())).events.length, 128);
    for (let i = 0; i < 15; i++) assert.equal((await fetch(`${base}/main?run=other-run-${i}`)).status, 200);
    assert.equal((await fetch(`${base}/main?run=over-capacity`)).status, 429);
  } finally {
    server.closeAllConnections();
    await new Promise(resolve => server.close(resolve));
  }
});

test("page acquisition fails immediately on its terminal open settlement", async () => {
  let inspected = false;
  const frame = { evaluate: async () => { inspected = true; return ""; } };
  const wait = harnessFunction("waitForEmbeddedBrowserPage", {
    BROWSER_UI_PAGE_ID_TIMEOUT_MS: 180_000, smokeStage: "browser:page-acquisition",
  });
  await assert.rejects(wait(frame, [{ frame, at: Date.now(), status: 200,
    body: { schema: "elastos.browser.open-status/v1", status: "failed", error: { code: "launch-failed" } } }]),
  error => error.details.settlement.status === "failed" && error.details.stage === "browser:page-acquisition");
  assert.equal(inspected, false);
  const active = { evaluate: async () => "page-current" };
  assert.equal(await wait(active, [{ frame, at: Date.now(), body: {} }]), "page-current");
  assert.equal(await wait(active, [{ frame: active, open_id: "old-open", body: {} }], new Set(["old-open"])), "page-current");
  await assert.rejects(wait(active, [{ frame: active, open_id: "new-open", body: {} }], new Set(["old-open"])));
});

test("explicit Browser inventory deadline aborts a hung response body; default API calls keep their options", async () => {
  const api = harnessFunction("browserApi");
  const page = { evaluate: async (callback, args) => vm.runInNewContext(`(${callback.toString()})(args)`, {
    args, AbortSignal, fetch: async (_path, options) => {
      assert.equal(options.headers["x-elastos-home-token"], "exact-token");
      if (args.timeoutMs === null) {
        assert.equal(options.signal, undefined);
        return { ok: true, status: 200, text: async () => "{}" };
      }
      assert.ok(options.signal);
      return { ok: true, status: 200, text: () => new Promise((resolve, reject) => {
        options.signal.addEventListener("abort", () => reject(options.signal.reason), { once: true });
      }) };
    },
  }) };
  assert.equal((await api(page, "exact-token", "/summary")).ok, true);
  const keepAlive = setInterval(() => {}, 100);
  try { await assert.rejects(api(page, "exact-token", "/summary", { timeoutMs: 5 }), { name: "TimeoutError" }); }
  finally { clearInterval(keepAlive); }
});

test("error-state redaction covers frame fragments and direct token fields", () => {
  const redactString = harnessFunction("redactSensitiveString");
  const redact = harnessFunction("redactSensitive", { redactSensitiveString: redactString });
  const serialized = JSON.stringify(redact({
    activeShellFrameSrc: "http://localhost/apps/home-gui/#home_token=secret-value",
    nested: { homeToken: "secret-value", home_token: "secret-value" },
  }));
  assert.ok(!serialized.includes("secret-value"));
  assert.match(source, /JSON\.stringify\(redactSensitive\(state\), null, 2\)/);
});

test("launcher reuses a visible restored window without toggling its Shelf item", async () => {
  const actions = [];
  const frame = { url: () => "http://localhost/apps/browser/" };
  const frameElement = { waitFor: async () => {}, elementHandle: async () => ({ contentFrame: async () => frame }) };
  let active = true;
  const existing = { isVisible: async () => true,
    evaluate: async callback => callback({ classList: { contains: name => name === "window-active" && active } }),
  };
  const gui = {
    getByRole: () => ({ waitFor: async () => { const error = new Error(); error.name = "TimeoutError"; throw error; } }),
    locator: selector => {
      if (selector.includes("#taskbar-targets")) return { first: () => ({
        isVisible: async () => true, click: async () => actions.push("toggle-shelf"),
      }) };
      if (selector.includes("iframe.window-frame")) return { last: () => frameElement };
      return { last: () => existing };
    },
  };
  const open = harnessFunction("openDesktopAppWindow", {
    HOME_URL: "http://localhost/apps/home/", waitForSignedHome: async () => {},
    waitForCapsuleFrame: async () => gui, assert: (condition, message) => assert.ok(condition, message),
  });
  const page = { url: () => "http://localhost/apps/home/", goto: async () => actions.push("reload-home") };
  assert.equal(await open(page, "browser"), frame);
  assert.deepEqual(actions, []);
  active = false;
  assert.equal(await open(page, "browser"), frame);
  assert.deepEqual(actions, ["toggle-shelf"]);
});

function closeHarnessSurface() {
  const listeners = new Map();
  const messageListeners = new Set();
  const timers = new Map();
  const actions = [];
  const source = {};
  const gui = { origin: "null", url: () => "http://localhost/apps/home-gui/#home_token=parent-secret",
    evaluateHandle: async (callback, args) => {
      const observer = vm.runInNewContext(`(${callback.toString()})(args)`, { args, performance,
        setTimeout: callback => { timers.set(callback, callback); return callback; },
        clearTimeout: timer => timers.delete(timer),
        window: { addEventListener: (_, listener) => messageListeners.add(listener),
          removeEventListener: (_, listener) => messageListeners.delete(listener) },
      });
      return { evaluate: async callback => callback(observer), dispose: async () => actions.push("disposed") };
    },
  };
  const frame = { url: () => "http://localhost/apps/browser/?browser_instance=instance-one#home_token=browser-token",
    parentFrame: () => gui, frameElement: async () => iframe, waitForFunction: async () => {} };
  const node = { isConnected: true, dataset: { windowId: "browser--2" }, querySelector: () => iframe };
  const iframe = { contentWindow: source, isConnected: true, closest: () => node,
    getAttribute: async () => frame.url(), evaluateHandle: async callback => ({ asElement: () => {
      assert.equal(callback(iframe), node); return section;
    } }) };
  const section = { node, getAttribute: async () => node.dataset.windowId,
    evaluate: async (callback, args) => callback(node, args),
    $: async selector => {
      assert.equal(selector, '[data-action="close"]');
      return { getAttribute: async () => "Close", dispose: async () => {}, click: async () => {
        actions.push("ui-close"); await surface.click();
        if (surface.removeSection !== false) { node.isConnected = false; iframe.isConnected = false; }
      } };
    } };
  const sections = new Map([[node.dataset.windowId, section]]);
  gui.locator = selector => {
    const id = selector.match(/data-window-id="([^"]+)"/)?.[1];
    const live = () => sections.get(id)?.node.isConnected ? sections.get(id) : null;
    return { count: async () => live() ? 1 : 0,
      evaluate: async (callback, captured) => callback(live()?.node, captured.node) };
  };
  gui.waitForFunction = async (callback, captured) => {
    assert.ok(callback(captured.node), "captured Browser section is still connected");
    actions.push("detached"); await surface.emit("framedetached", frame);
  };
  const page = {
    on: (event, listener) => { if (!listeners.has(event)) listeners.set(event, new Set()); listeners.get(event).add(listener); },
    off: (event, listener) => listeners.get(event)?.delete(listener),
  };
  const surface = { gui, frame, page, actions, sections, section, iframe,
    emit: async (event, value) => { for (const listener of listeners.get(event) || []) await listener(value); },
    message: (data = {}, overrides = {}) => {
      const event = { source, origin: "null", data: { type: "elastos.browser.window-close.result/v1", browserInstance: "instance-one",
        requestId: "close-one", homeToken: "browser-token", state: "terminal", terminalKind: "no_page", reason: "",
        pageId: "", cleanupId: "", generation: 0, ...data }, ...overrides };
      for (const listener of messageListeners) listener(event);
    },
    expire: () => { for (const callback of [...timers.values()]) callback(); },
    assertClean: () => {
      assert.equal(timers.size, 0);
      assert.equal(messageListeners.size, 0);
      assert.ok([...listeners.values()].every(value => value.size === 0));
      assert.equal(actions.at(-1), "disposed");
    },
  };
  surface.window = { appFrame: frame, section, iframe, parent: gui, windowId: node.dataset.windowId,
    instance: "instance-one", token: "browser-token",
    locator: gui.locator('section.window[data-target="browser"][data-window-id="browser--2"]') };
  surface.click = async () => surface.message();
  return surface;
}

function closeHarnessFunction(globals = {}) {
  const identityGlobals = browserIdentityGlobals();
  return harnessFunction("closeControlledBrowserWindow", {
    clickBrowserWindowClose: harnessFunction("clickBrowserWindowClose", identityGlobals),
    waitForBrowserWindowDetached: harnessFunction("waitForBrowserWindowDetached"),
    BROWSER_UI_PAGE_ID_TIMEOUT_MS: 180_000,
    REQUIRE_BROWSER_VZ_TRANSPORT: false,
    markStage: () => {}, assert: (condition, message) => assert.ok(condition, message),
    redactSensitiveString: harnessFunction("redactSensitiveString"),
    browserJourneyRuntimeEmpty: harnessFunction("browserJourneyRuntimeEmpty"),
    waitForJourneyEvidence: async (read, predicate) => { const value = await read(); assert.ok(predicate(value)); return value; },
    ...globals,
  });
}

function browserIdentityGlobals() {
  const globals = { HOME_URL: "http://localhost/apps/home/",
    assert: (condition, message) => assert.ok(condition, message) };
  globals.launchTokenFromRoute = harnessFunction("launchTokenFromRoute", globals);
  globals.assertIsolatedLaunchRoute = harnessFunction("assertIsolatedLaunchRoute", globals);
  globals.assertBrowserWindowIdentity = harnessFunction("assertBrowserWindowIdentity", globals);
  return globals;
}

test("captured Browser B closes while A remains and newly inserted C never takes its token or chrome", async () => {
  const surface = closeHarnessSurface();
  const other = id => ({ node: { isConnected: true, dataset: { windowId: id } } });
  const a = other("browser--1"), c = other("browser--3");
  surface.sections.set("browser--1", a);
  const selectedFrame = surface.frame;
  surface.sections.set("browser--3", c); // A restore races after Frame B was selected.
  const globals = browserIdentityGlobals();
  const identity = await harnessFunction("captureBrowserWindowIdentity", globals)(selectedFrame);
  assert.equal(identity.windowId, "browser--2");
  assert.equal(identity.token, "browser-token");
  await harnessFunction("clickBrowserWindowClose", globals)(identity, selectedFrame, identity.token);
  await harnessFunction("waitForBrowserWindowDetached")(identity);
  assert.equal(surface.section.node.isConnected, false);
  assert.equal(a.node.isConnected, true);
  assert.equal(c.node.isConnected, true);
  assert.deepEqual(surface.actions, ["ui-close", "detached"]);
});

test("iframe-only replacement is not a detached Browser window; replaced section and foreign token reject", async () => {
  const surface = closeHarnessSurface();
  const globals = browserIdentityGlobals();
  const capture = harnessFunction("captureBrowserWindowIdentity", globals);
  const identity = await capture(surface.frame);
  surface.iframe.isConnected = false;
  await assert.rejects(harnessFunction("waitForBrowserWindowDetached")(identity), /section is still connected/);
  await assert.rejects(globals.assertBrowserWindowIdentity(identity, surface.frame, identity.token), /no longer belongs/);
  surface.iframe.isConnected = true;
  await assert.rejects(globals.assertBrowserWindowIdentity(identity, surface.frame, "foreign-token"), /authority identity/);
  surface.sections.set(identity.windowId, { node: { isConnected: true } });
  await assert.rejects(globals.assertBrowserWindowIdentity(identity, surface.frame, identity.token), /replaced or duplicated/);
});

test("startup failure can close through Home before a page id exists", async () => {
  const baseline = { principal_sessions: 0, total_sessions: 0, launching_sessions: 0,
    engine_cleanup_obligations: 0, launch_reconciliation_obligations: 0 };
  const surface = closeHarnessSurface();
  const otherWindow = { node: { isConnected: true } };
  surface.sections.set("browser--1", otherWindow);
  const close = closeHarnessFunction({
    browserApi: async frame => { assert.equal(frame, surface.gui); return { ok: true,
      body: { sessions: { ...baseline, recoverable_page: null } } }; },
  });
  const result = await close(surface.page, surface.frame, surface.window, "browser-token", baseline);
  assert.equal(otherWindow.node.isConnected, true);
  assert.equal(result.startup_close.terminal_kind, "no_page");
  assert.equal(result.close_evidence.messages[0].requestId, "close-one");
  assert.equal(result.close_evidence.frames[0].event, "detached");
  assert.deepEqual(surface.actions, ["ui-close", "detached", "disposed"]);
  surface.assertClean();
});

test("terminal no-page and iframe detachment still fail while the captured Home section remains", async () => {
  const surface = closeHarnessSurface();
  surface.removeSection = false;
  surface.click = async () => { surface.message(); surface.iframe.isConnected = false; };
  const baseline = { principal_sessions: 0, total_sessions: 0, launching_sessions: 0,
    engine_cleanup_obligations: 0, launch_reconciliation_obligations: 0 };
  const close = closeHarnessFunction({ browserApi: async () => ({ ok: true,
    body: { sessions: { ...baseline, recoverable_page: null } } }) });
  await assert.rejects(close(surface.page, surface.frame, surface.window, "browser-token", baseline), /section is still connected/);
  assert.equal(surface.section.node.isConnected, true);
  surface.assertClean();
});

test("launcher visibility failure retains exact Frame B for caller cleanup while A and restored C remain", async () => {
  const surface = closeHarnessSurface();
  const a = { node: { isConnected: true } }, c = { node: { isConnected: true } };
  surface.sections.set("browser--1", a);
  surface.page.url = () => "http://localhost/apps/home/";
  const original = new Error("selected Browser B visibility failed");
  surface.iframe.contentFrame = async () => surface.frame;
  surface.iframe.waitForElementState = async state => {
    assert.equal(state, "visible");
    surface.sections.set("browser--3", c);
    throw original;
  };
  surface.gui.getByRole = () => ({ waitFor: async () => {
    const error = new Error(); error.name = "TimeoutError"; throw error;
  } });
  const pinnedLocator = surface.gui.locator;
  surface.gui.locator = selector => {
    if (selector.includes("data-window-id=")) return pinnedLocator(selector);
    if (selector.includes("#taskbar-targets")) return { first: () => ({ isVisible: async () => true }) };
    if (selector.includes("iframe.window-frame")) return { last: () => ({
      waitFor: async options => { assert.equal(options.state, "attached"); },
      elementHandle: async () => surface.iframe,
    }) };
    return { last: () => ({ isVisible: async () => true,
      evaluate: async callback => callback({ classList: { contains: () => true } }),
    }) };
  };
  const globals = browserIdentityGlobals();
  const open = harnessFunction("openDesktopAppWindow", { ...globals,
    waitForSignedHome: async () => {}, waitForCapsuleFrame: async () => surface.gui,
  });
  const empty = { schema: "elastos.browser.session-capacity/v1", status: "configured", recoverable_page: null,
    lifecycle: { sessions: [] }, active_sessions: 0, principal_sessions: 0, total_sessions: 0,
    launching_sessions: 0, engine_cleanup_obligations: 0, launch_reconciliation_obligations: 0 };
  const close = closeHarnessFunction({ CHECK_BROWSER_CONTROLLED_JOURNEY: true,
    browserApi: async () => ({ ok: true, body: { sessions: empty } }) });
  const caller = harnessFunction("checkBrowserEmbeddedUiInput", { ...globals,
    CHECK_BROWSER_CONTROLLED_JOURNEY: true, openDesktopAppWindow: open,
    captureBrowserWindowIdentity: harnessFunction("captureBrowserWindowIdentity", globals),
    closeControlledBrowserWindow: close, markStage: () => {}, smokeStage: "browser:home-launch",
    console: { error: () => {} }, redactSensitive: value => value,
  });
  await assert.rejects(caller(surface.page, null), error => {
    assert.equal(error, original);
    assert.equal(error.details.startup_cleanup.startup_close.terminal_kind, "no_page");
    assert.equal(error.details.cleanup_error, undefined);
    return true;
  });
  assert.equal(surface.section.node.isConnected, false);
  assert.equal(a.node.isConnected, true); assert.equal(c.node.isConnected, true);
  assert.equal(surface.actions.filter(action => action === "ui-close").length, 1);
  surface.assertClean();
});

test("a successful journey cannot use no-page startup cleanup as fresh-close proof", async () => {
  const baseline = { principal_sessions: 0, total_sessions: 0, launching_sessions: 0,
    engine_cleanup_obligations: 0, launch_reconciliation_obligations: 0 };
  const surface = closeHarnessSurface();
  const close = closeHarnessFunction({
    REQUIRE_BROWSER_VZ_TRANSPORT: true,
    browserApi: async () => ({ ok: true, body: { sessions: { ...baseline, recoverable_page: null } } }),
  });
  await assert.rejects(close(surface.page, surface.frame, surface.window, "browser-token", baseline,
    { expectedPageId: "page-one" }));
  surface.assertClean();
});

test("journey preserves click/key timing and exact close even when diagnostic stop rejects or throws", async () => {
  for (const variant of ["success", "input-failure", "stop-reject", "stop-throw", "input-failure-stop-throw",
    "operator-success", "operator-media-success", "operator-before-close-failure", "operator-after-close-failure",
    "operator-pending-close-failure", "operator-qualification-success", "operator-qualification-failure",
    "remote-engine-success", "remote-engine-wrong-adapter", "remote-engine-unavailable"]) {
    const remote = variant.startsWith("remote-engine-");
    const routeFails = variant === "remote-engine-wrong-adapter";
    const setupFails = variant === "remote-engine-unavailable";
    const target = browserJourneyTargetConfig(remote ? { HOME_VIRTUAL_AUTH_BROWSER_ENGINE_ID: "remote-engine-test",
      HOME_VIRTUAL_AUTH_BROWSER_FIXTURE_ORIGIN: "http://fixture.example:61512",
      HOME_VIRTUAL_AUTH_BROWSER_FIXTURE_ADMIN_ORIGIN: "http://localhost:61512",
      HOME_VIRTUAL_AUTH_BROWSER_ALLOW_REMOTE_FIXTURE: "1" } : {});
    let selectedEngine = "", summaryReads = 0, addressWaits = 0;
    const stages = [];
    const inputFails = variant.includes("input-failure");
    const operatorEnabled = variant.startsWith("operator-");
    const qualification = variant.includes("qualification");
    const qualificationFails = variant === "operator-qualification-failure";
    const operatorFails = operatorEnabled && variant.endsWith("failure") && !qualificationFails;
    const media = variant === "operator-media-success" || qualification;
    const runId = "journey-test-run";
    const events = [];
    const closes = [], inputActions = [];
    const inputError = new Error("input delivery failed");
    const operatorError = Object.assign(new Error("operator probe failed"), { evidence: { ok: false, failure: "typed-fixture-error" } });
    const qualificationError = Object.assign(new Error("qualification probe failed"), { qualification: { failure: "fixture-gap" } });
    let url = "";
    let pageName = "";
    let value = "";
    let frames = 0, inspectReads = 0, operatorCalls = 0, finishClose;
    const pendingClose = new Promise(resolve => { finishClose = resolve; });
    const report = type => events.push({ sequence: events.length + 1, type, page: pageName, value, scroll_y: type === "scroll" ? 640 : 0,
      ...(type === "audio" ? { audio_state: "running", frequency_hz: 440 } : {}),
      input_rect: { x: 32, y: 150, width: 480, height: 60 } });
    const appFrame = {
      waitForFunction: async () => { addressWaits++; },
      evaluate: async () => ({}),
      locator: selector => ["#browser-settings", "#browser-settings-close", "#browser-engine"].includes(selector) ? {
        click: async () => {}, selectOption: async id => { selectedEngine = id; }, inputValue: async () => selectedEngine,
      } : ({ waitFor: async () => {}, hover: async () => {}, click: async () => { inputActions.push("click"); if (media) report("audio"); },
        evaluate: async fn => fn({ dataset: { visible: "false" }, querySelector: () => ({ textContent: "" }) }),
        fill: async target => { url = target; pageName = new URL(target).pathname.slice(1); value = ""; },
        press: async key => { assert.equal(key, "Enter"); report("load"); },
        pressSequentially: async character => { inputActions.push("key"); if (inputFails) throw inputError; value += character; report("input"); },
      }),
    };
    const page = { on: () => {}, off: () => {}, mouse: { wheel: async () => report("scroll") } };
    const baseline = {};
    const window = { instance: "instance-one" };
    const journey = harnessFunction("runControlledBrowserJourney", {
      CHECK_BROWSER_CONTROLLED_RECOVERY: false,
      CHECK_BROWSER_VIEWER_RELOAD: false,
      CHECK_BROWSER_CONTROLLED_INSPECTION: operatorEnabled,
      CHECK_BROWSER_CONTROLLED_OPERATOR: operatorEnabled,
      CHECK_BROWSER_CONTROLLED_MEDIA: media, controlledTonePresent: () => true,
      browserQualification: qualification ? { observe: async options => {
        assert.equal(options.pageId, "page-nav"); assert.equal(options.appFrame, appFrame);
        assert.equal(typeof options.interact, "function"); assert.equal((await options.readStatus()).body.direct_network, false);
        assert.ok((await options.readReceipt()).events.some(e => e.type === "scroll"));
        assert.equal(inspectReads, 2, "ordinary inspection and input precede long observation");
        assert.ok(url.endsWith("&media=1&qualification=1"));
        if (qualificationFails) throw qualificationError;
        return { schema: "elastos.browser.qualification-observation/v1", fixture: true };
      } } : null,
      BROWSER_JOURNEY_TARGET: target, BROWSER_OPEN_DISPLAY_MODE: "webrtc_remote_display",
      BROWSER_UI_PAGE_ID_TIMEOUT_MS: 180_000, BROWSER_REMOTE_VIDEO_TIMEOUT_MS: 30_000,
      randomUUID: () => runId, AbortSignal, smokeStage: "input", markStage: value => stages.push(value),
      assert: (condition, message) => assert.ok(condition, message),
      fetch: async (requestUrl, options) => {
        assert.equal(requestUrl, `${target.adminOrigin}/receipt?run=${runId}`);
        assert.equal(options.credentials, "omit");
        return { ok: true, status: 200,
          json: async () => ({ schema: "elastos.browser.journey-receipt/v1", run: runId, events }) };
      },
      waitForEmbeddedBrowserPage: async () => `page-${pageName}`,
      browserApi: async (_frame, _token, path, options) => {
        if (path.includes("/summary?")) {
          assert.equal(path, "/api/apps/browser/summary?browser_instance=instance-one");
          assert.equal(options.timeoutMs, 5_000);
          summaryReads++;
          if (setupFails) return { ok: true, body: { engine_adapter: { adapters: [] } } };
          const adapter = { id: target.engineId, direct_network: false, wallet_injection: false };
          return { ok: true, body: { engine_adapter: { adapters: [adapter], remote_services: { offers: [{ state: "approved",
            launch_available: true, selectable_adapters: [adapter], adapters: [{ ...adapter, id: "browser-vm-test" }] }] } },
            sessions: { recoverable_page: { schema: "elastos.browser.recoverable-page/v1", state: "active", page_id: `page-${pageName}`,
              service_selection: { schema: "elastos.browser.service-selection/v1", engine_id: selectedEngine },
              engine_page: { page_id: `page-${pageName}`, adapter: routeFails ? "wrong-adapter" : "browser-vm-test", provider: "browser-engine" } } } } };
        }
        if (path.endsWith("/inspect")) {
          if (!options) return { ok: true, body: { formats: ["accessibility_tree"] } };
          if (inspectReads++ >= 2) return { status: 409, body: { code: "stale_inspection" } };
          return { ok: true, body: { schema: "elastos.browser.inspect-result/v1", page_id: "page-nav",
            document_generation: "b".repeat(32), snapshot_id: "a".repeat(32),
            next_cursor: inspectReads === 1 ? "a".repeat(32) + ":1" : null,
            nodes: inspectReads === 1 ? [{ role: "textbox", name: "Test text", value }] : [] } };
        }
        return { ok: true, body: { schema: "elastos.browser.page-status/v1", actual_url: url,
          direct_network: false, display_session: { media_transport: "runtime_relay" } } };
      },
      runtimeRelayIceContractOk: () => true,
      waitForJourneyEvidence: async (read, predicate) => { const result = await read(); assert.ok(predicate(result)); return result; },
      waitForBrowserRemoteVideo: async () => ({ decoded_frames: frames }),
      browserRemoteVideoMetrics: async () => ({ decoded_frames: ++frames }),
      remoteVideoClickPositionForPagePoint: async () => { inputActions.push("geometry"); return { x: 50, y: 50 }; },
      observeControlledBrowserInput: async (actualPage, actualFrame, token, pageId) => {
        assert.equal(actualPage, page); assert.equal(actualFrame, appFrame);
        assert.equal(token, "browser-token"); assert.equal(pageId, "page-nav");
        inputActions.push("observe");
        return { evidence: { observer: {} }, stop: failure => {
          inputActions.push(failure ? "failure-stop" : "stop");
          if (variant.includes("stop-throw")) throw new Error("private diagnostic failure");
          if (variant === "stop-reject") return Promise.reject(new Error("private diagnostic failure"));
        } };
      },
      closeControlledBrowserWindow: async (...args) => {
        closes.push(args);
        return variant === "operator-pending-close-failure" ? pendingClose : { receipt: { closed: true } };
      },
      runControlledBrowserOperator: async (frame, token, pageId, expectedUrl, readReceipt, close) => {
        operatorCalls++;
        assert.equal(frame, appFrame); assert.equal(token, "browser-token"); assert.equal(pageId, "page-nav");
        assert.equal(expectedUrl, `http://localhost:61511/main?run=${runId}${media ? "&media=1" : ""}${qualification ? "&qualification=1" : ""}`);
        assert.equal(url, expectedUrl);
        assert.equal(inspectReads, 3, "ordinary inspection and stale-cursor checks finish first");
        assert.equal((await readReceipt()).events.at(-1).value, "");
        if (variant === "operator-before-close-failure") throw operatorError;
        if (variant === "operator-pending-close-failure") {
          void close();
          setImmediate(() => finishClose({ receipt: { closed: true } }));
          throw operatorError;
        }
        await close(); await close(); // The wrapper and outer finally share one UI attempt.
        if (operatorFails) throw operatorError;
        return { ok: true };
      },
    });
    if (setupFails) await assert.rejects(journey(page, appFrame, window, "browser-token", baseline, []), error => {
      assert.match(error.message, /Requested Browser Engine is absent from the Runtime inventory/);
      assert.equal(error.details.controlled_journey.requested_engine_id, target.engineId);
      return true;
    });
    else if (routeFails) await assert.rejects(journey(page, appFrame, window, "browser-token", baseline, []), /different Engine or page/);
    else if (inputFails || operatorFails || qualificationFails) await assert.rejects(journey(page, appFrame, window, "browser-token", baseline, []), error => {
      assert.equal(error, inputFails ? inputError : qualificationFails ? qualificationError : operatorError);
      if (qualificationFails) assert.equal(error.details.controlled_journey.qualification.failure, "fixture-gap");
      if (operatorFails) assert.equal(error.details.controlled_journey.operator.failure, "typed-fixture-error");
      if (variant.includes("stop-throw")) assert.equal(error.details.controlled_journey.input_observation.observer.stop_failed, true);
      return true;
    });
    else {
      const result = await journey(page, appFrame, window, "browser-token", baseline, []);
      assert.equal(result.page_id, "page-nav");
      assert.ok(result.controlled_journey.pages.every(row => new URL(row.url).origin === target.origin));
      if (remote) {
        assert.equal(result.controlled_journey.requested_engine_id, target.engineId);
        assert.equal(result.controlled_journey.engine_route.adapter_id, "browser-vm-test");
        assert.equal(result.controlled_journey.engine_route.engine_id, selectedEngine);
        assert.equal(result.controlled_journey.engine_route.page_id, "page-nav");
      }
      if (qualification) assert.equal(result.controlled_journey.qualification.fixture, true);
      if (variant.startsWith("stop-")) assert.equal(result.controlled_journey.input_observation.observer.stop_failed, true);
      assert.ok(!JSON.stringify(result).includes("private diagnostic failure"));
    }
    assert.equal(closes.length, 1);
    assert.equal(operatorCalls, operatorEnabled && !qualificationFails ? 1 : 0);
    assert.equal(summaryReads, remote ? setupFails ? 1 : routeFails ? 2 : 3 : 0);
    if (setupFails) {
      assert.equal(addressWaits, 0); assert.equal(selectedEngine, "");
      assert.equal(stages[0], "browser:controlled-engine-selection");
      assert.ok(!stages.includes("browser:controlled-address-ready"));
    }
    if (routeFails || setupFails) assert.deepEqual(inputActions, []);
    else {
      assert.deepEqual(inputActions.slice(0, 4), ["observe", "geometry", "click", "key"]);
      assert.equal(inputActions[4], inputFails ? "failure-stop" : "stop");
    }
    assert.deepEqual(closes[0].slice(0, 5), [page, appFrame, window, "browser-token", baseline]);
    assert.equal(closes[0][5]?.expectedPageId, setupFails || routeFails || inputFails || qualificationFails || variant === "operator-before-close-failure" ? null : "page-nav");
  }
});

test("UI close requires exact ownership, terminal Engine effects and Runtime baseline", async () => {
  const baseline = { principal_sessions: 0, total_sessions: 0, launching_sessions: 0,
    engine_cleanup_obligations: 0, launch_reconciliation_obligations: 0 };
  const owner = { page_id: "page-one", cleanup: { schema: "elastos.browser.cleanup-handle/v1", id: "cleanup-one" } };
  const good = { schema: "elastos.browser.close-result/v1", page_id: "page-one", cleanup_id: "cleanup-one",
    browser_instance: "instance-one", closed: true, already_closed: false,
    cleanup: { schema: "elastos.browser.runtime-session-cleanup/v1", ok: true, action: "released_exact_runtime_browser_ownership" },
    terminal_effects: { page_absent: true, child_absent: true, vm_absent: true, route_absent: true, socket_absent: true } };
  for (const variant of ["valid", "valid-vz", "wrong-expected-page", "wrong-owner", "vm-retained", "turn-retained",
    "session-retained", "missing-vz-proof", "already-closed", "wrong-response-frame", "wrong-response-origin",
    "wrong-response-token", "wrong-receipt-instance", "missing-receipt-instance", "wrong-receipt-page",
    "wrong-request-handle", "wrong-request-instance", "missing-response", "missing-pending", "missing-terminal",
    "wrong-message-source", "wrong-message-origin", "wrong-message-token", "wrong-request-id", "wrong-generation",
    "wrong-message-page", "wrong-message-cleanup", "wrong-terminal-kind",
    "controlled-restored-entry", "controlled-retained-session", "controlled-retained-launch",
    "controlled-retained-cleanup", "controlled-retained-reconciliation", "controlled-unavailable", "controlled-lifecycle-residue"]) {
    const controlled = variant.startsWith("controlled-");
    // Home restoration already acquired one session/open before the entry read.
    // A blanket <= baseline would hide each residual variant below.
    const entry = controlled ? { ...baseline, active_sessions: 1, principal_sessions: 1, total_sessions: 1,
      launching_sessions: 1, engine_cleanup_obligations: 1, launch_reconciliation_obligations: 1 } : baseline;
    const receipt = structuredClone(good);
    if (variant === "already-closed") { receipt.closed = false; receipt.already_closed = true; }
    if (variant === "wrong-owner") receipt.cleanup_id = "foreign-cleanup";
    if (variant === "vm-retained") receipt.terminal_effects.vm_absent = false;
    if (["valid-vz", "turn-retained"].includes(variant)) {
      receipt.transport_proof = { schema: "elastos.browser.vz-transport-public-proof/v1", page_id: owner.page_id };
      for (const key of ["transport_session_absent", "turn_process_absent", "turn_listener_absent", "turn_relay_ports_absent",
        "ordinary_vsock_bridge_absent", "media_vsock_bridge_absent", "bootstrap_vsock_bridge_absent", "hibernation_state_absent"]) {
        receipt.terminal_effects[key] = variant !== "turn-retained" || key !== "turn_process_absent";
      }
    }
    if (variant === "wrong-receipt-instance") receipt.browser_instance = "foreign-instance";
    if (variant === "missing-receipt-instance") delete receipt.browser_instance;
    if (variant === "wrong-receipt-page") receipt.page_id = "foreign-page";
    const surface = closeHarnessSurface();
    const actions = surface.actions;
    const opaqueGui = surface.gui;
    let reads = 0;
    const response = { ok: () => true, status: () => 200, json: async () => receipt,
      url: () => `http://${variant === "wrong-response-origin" ? "foreign" : "localhost"}/api/apps/browser/pages/page-one/close`,
      request: () => ({ method: () => "POST", frame: () => variant === "wrong-response-frame" ? {} : surface.frame,
        headers: () => ({ "x-elastos-home-token": variant === "wrong-response-token" ? "foreign-token" : "browser-token" }),
        postDataJSON: () => ({ schema: "elastos.browser.close-request/v2",
          cleanup_id: variant === "wrong-request-handle" ? "foreign-cleanup" : "cleanup-one",
          browser_instance: variant === "wrong-request-instance" ? "foreign-instance" : "instance-one",
        }) }) };
    const page = surface.page;
    // Keep the former broad response waiter available so the old code fails the
    // negative assertions instead of failing because a mock API is absent.
    page.waitForResponse = async predicate => { assert.equal(await predicate(response), true); return response; };
    const close = closeHarnessFunction({
      CHECK_BROWSER_CONTROLLED_JOURNEY: controlled,
      REQUIRE_BROWSER_VZ_TRANSPORT: ["valid-vz", "missing-vz-proof"].includes(variant),
      browserApi: async frame => { assert.equal(frame, opaqueGui); return { ok: true, body: { sessions: reads++ === 0 ? { recoverable_page: owner } : {
        schema: "elastos.browser.session-capacity/v1", status: variant === "controlled-unavailable" ? "unavailable" : "configured",
        ...baseline, active_sessions: variant === "controlled-retained-session" ? 1 : 0,
        principal_sessions: ["session-retained", "controlled-retained-session"].includes(variant) ? 1 : 0,
        total_sessions: variant === "controlled-retained-session" ? 1 : 0,
        launching_sessions: variant === "controlled-retained-launch" ? 1 : 0,
        engine_cleanup_obligations: variant === "controlled-retained-cleanup" ? 1 : 0,
        launch_reconciliation_obligations: variant === "controlled-retained-reconciliation" ? 1 : 0,
        lifecycle: { sessions: variant === "controlled-lifecycle-residue" ? [{ page_id: "foreign-page" }] : [] }, recoverable_page: null,
      } } }; },
      waitForJourneyEvidence: async (read, predicate) => { const value = await read(); assert.ok(predicate(value)); return value; },
    });
    surface.click = async () => {
      const overrides = variant === "wrong-message-source" ? { source: {} }
        : variant === "wrong-message-origin" ? { origin: "http://foreign" } : {};
      const binding = { pageId: owner.page_id, cleanupId: owner.cleanup.id, generation: 2,
        homeToken: variant === "wrong-message-token" ? "foreign-token" : "browser-token" };
      if (variant !== "missing-pending") surface.message({ ...binding, state: "pending", terminalKind: "" }, overrides);
      if (variant !== "missing-response") await surface.emit("response", response);
      if (variant !== "missing-terminal") surface.message({ ...binding, terminalKind: variant === "wrong-terminal-kind" ? "already_absent" : "closed",
        requestId: variant === "wrong-request-id" ? "foreign-request" : "close-one",
        generation: variant === "wrong-generation" ? 3 : 2,
        pageId: variant === "wrong-message-page" ? "foreign-page" : owner.page_id,
        cleanupId: variant === "wrong-message-cleanup" ? "foreign-cleanup" : owner.cleanup.id,
      }, overrides);
      surface.expire();
    };
    const options = { expectedPageId: variant === "wrong-expected-page" ? "last-exercised-page" : owner.page_id };
    if (["valid", "valid-vz", "controlled-restored-entry"].includes(variant)) {
      const result = await close(page, surface.frame, surface.window, "browser-token", entry, options);
      assert.equal(result.receipt.cleanup_id, owner.cleanup.id);
      assert.equal(result.receipt.page_id, options.expectedPageId);
      assert.equal(result.close_evidence.close_responses[0].authority_matches, true);
      assert.equal(result.close_evidence.messages.at(-1).terminalKind, "closed");
      assert.deepEqual(actions, ["ui-close", "detached", "disposed"]);
      if (controlled) {
        assert.equal(result.close_evidence.runtime_counts.entry.launching_sessions, 1);
        assert.equal(result.close_evidence.runtime_counts.terminal_requirement, "configured_empty_runtime");
        assert.equal(result.sessions_after_close.total_sessions, 0);
      }
    } else {
      await assert.rejects(close(page, surface.frame, surface.window, "browser-token", entry, options),
        error => error?.name !== "TypeError", variant);
    }
    surface.assertClean();
  }
});

test("close diagnostics keep bounded redacted pending/error/terminal messages and frame navigation", async () => {
  for (const terminal of [true, false]) {
    const surface = closeHarnessSurface();
    const baseline = { principal_sessions: 0, total_sessions: 0, launching_sessions: 0,
      engine_cleanup_obligations: 0, launch_reconciliation_obligations: 0 };
    const close = closeHarnessFunction({ browserApi: async () => ({ ok: true,
      body: { sessions: { ...baseline, recoverable_page: null } } }) });
    surface.click = async () => {
      surface.message({ requestId: "foreign-source" }, { source: {} });
      surface.message({ requestId: "foreign-instance", browserInstance: "other-instance" });
      surface.message({ requestId: "wrong-type", type: "unrelated/v1" });
      for (let i = 0; i < 40; i++) surface.message({ state: "pending", terminalKind: "", requestId: `close-${i}`,
        reason: `browser-token home_token=hidden-value https://localhost/?secret=hidden-url ${"x".repeat(400)}` });
      surface.message({ state: "error", terminalKind: "", reason: "close_error", extra: "discard-this-secret" });
      for (let i = 0; i < 40; i++) {
        surface.frame.url = () => `http://user:password@localhost/apps/browser/?browser_instance=instance-one&secret=query-secret#home_token=fragment-secret`;
        await surface.emit("framenavigated", surface.frame);
      }
      await surface.emit("framenavigated", surface.gui);
      await surface.emit("framenavigated", { url: () => "http://unrelated/" });
      if (terminal) surface.message();
      else surface.expire();
    };
    let evidence;
    if (terminal) evidence = (await close(surface.page, surface.frame, surface.window, "browser-token", baseline)).close_evidence;
    else await assert.rejects(close(surface.page, surface.frame, surface.window, "browser-token", baseline), error => {
      assert.match(error.message, /startup close remained pending/);
      evidence = error.details.close_evidence;
      return true;
    });
    assert.equal(evidence.messages.length, 32);
    assert.equal(evidence.frames.length, 32);
    assert.equal(evidence.dropped_messages, terminal ? 10 : 9);
    assert.equal(evidence.dropped_frames, terminal ? 10 : 9);
    assert.ok(evidence.messages.some(message => message.state === "pending"));
    assert.ok(evidence.messages.some(message => message.reason === "close_error"));
    assert.equal(evidence.messages.at(-1).state, terminal ? "terminal" : "error");
    assert.ok(evidence.messages.every(message => message.reason.length <= 256));
    assert.ok(evidence.frames.some(frame => frame.frame === "home"));
    const serialized = JSON.stringify(evidence);
    for (const secret of ["browser-token", "hidden-value", "hidden-url", "discard-this-secret", "password", "query-secret",
      "fragment-secret", "parent-secret", "foreign-source", "foreign-instance", "wrong-type", "unrelated"]) {
      assert.ok(!serialized.includes(secret), secret);
    }
    surface.assertClean();
  }
});

test("startup close accepts already_absent only with the exact UI handle and Runtime tombstone proof", async () => {
  const variants = ["valid", "retry-after-503", "expected-page-set", "message-only", "wrong-source", "wrong-path", "wrong-origin", "wrong-token", "wrong-request-schema",
    "wrong-request-instance", "wrong-receipt-instance", "wrong-receipt-handle", "wrong-receipt-page", "wrong-receipt-schema",
    "wrong-request-id", "wrong-generation", "missing-pending", "missing-page-effect", "false-page-effect", "false-vm-effect",
    "bad-cleanup", "wrong-action", "not-already-closed", "failed-http", "missing-summary", "retained-session", "retained-obligation"];
  for (const variant of variants) {
    const surface = closeHarnessSurface();
    const baseline = { principal_sessions: 0, total_sessions: 0, launching_sessions: 0,
      engine_cleanup_obligations: 0, launch_reconciliation_obligations: 0 };
    let reads = 0;
    const close = closeHarnessFunction({ browserApi: async () => ({ ok: variant !== "missing-summary" || reads < 2,
      body: { sessions: { ...baseline, recoverable_page: null,
        total_sessions: variant === "retained-session" && reads >= 2 ? 1 : 0,
        engine_cleanup_obligations: variant === "retained-obligation" && reads >= 2 ? 1 : 0, reads: reads++ } } }) });
    const body = { schema: "elastos.browser.close-request/v2", cleanup_id: "cleanup-prior", browser_instance: "instance-one" };
    const receipt = { schema: "elastos.browser.close-result/v1", page_id: "page:prior", cleanup_id: "cleanup-prior",
      browser_instance: "instance-one", closed: false, already_closed: true, terminal_effects: { page_absent: true },
      cleanup: { schema: "elastos.browser.runtime-session-cleanup/v1", ok: true, action: "already_absent" },
      home_token: "response-secret", logs: "private-log-secret" };
    if (variant === "wrong-request-schema") body.schema = "wrong";
    if (variant === "wrong-request-instance") body.browser_instance = "other";
    if (variant === "wrong-receipt-instance") receipt.browser_instance = "other";
    if (variant === "wrong-receipt-handle") receipt.cleanup_id = "wrong";
    if (variant === "wrong-receipt-page") receipt.page_id = "wrong";
    if (variant === "wrong-receipt-schema") receipt.schema = "wrong";
    if (variant === "missing-page-effect") delete receipt.terminal_effects.page_absent;
    if (variant === "false-page-effect") receipt.terminal_effects.page_absent = false;
    if (variant === "false-vm-effect") receipt.terminal_effects.vm_absent = false;
    if (variant === "bad-cleanup") receipt.cleanup.ok = false;
    if (variant === "wrong-action") receipt.cleanup.action = "cleanup_pending";
    if (variant === "not-already-closed") receipt.already_closed = false;
    surface.click = async () => {
      if (variant !== "missing-pending") surface.message({ state: "pending", reason: "cleanup_in_flight", terminalKind: "",
        pageId: "page:prior", cleanupId: "cleanup-prior", generation: 2 });
      const response = {
        ok: () => variant !== "failed-http", status: () => variant === "failed-http" ? 503 : 200,
        url: () => `http://${variant === "wrong-origin" ? "foreign" : "localhost"}/api/apps/browser/pages/${variant === "wrong-path" ? "foreign" : "page%3Aprior"}/close`,
        request: () => ({ frame: () => variant === "wrong-source" ? {} : surface.frame,
          method: () => "POST", postDataJSON: () => body,
          headers: () => ({ "x-elastos-home-token": variant === "wrong-token" ? "foreign-token" : "browser-token" }) }),
        json: async () => receipt,
      };
      if (variant === "retry-after-503") await surface.emit("response", { ...response, ok: () => false, status: () => 503,
        json: async () => ({ error: "cleanup pending", logs: "private-log-secret" }) });
      if (variant !== "message-only") await surface.emit("response", response);
      surface.message({ terminalKind: "already_absent", pageId: "page:prior", cleanupId: "cleanup-prior",
        requestId: variant === "wrong-request-id" ? "other-request" : "close-one", generation: variant === "wrong-generation" ? 3 : 2 });
    };
    if (["valid", "retry-after-503"].includes(variant)) {
      const result = await close(surface.page, surface.frame, surface.window, "browser-token", baseline);
      assert.equal(result.startup_close.terminal_kind, "already_absent");
      assert.equal(result.receipt.cleanup.action, "already_absent");
      assert.equal(result.receipt.terminal_effects.vm_absent, undefined);
      assert.equal(result.close_evidence.close_responses.length, variant === "retry-after-503" ? 2 : 1);
      assert.ok(!JSON.stringify(result).includes("response-secret"));
      assert.ok(!JSON.stringify(result).includes("private-log-secret"));
    } else {
      await assert.rejects(close(surface.page, surface.frame, surface.window, "browser-token", baseline,
        { expectedPageId: variant === "expected-page-set" ? "page:prior" : null }), error => {
        if (variant !== "expected-page-set") assert.ok(error.details.close_evidence, variant);
        assert.notEqual(error?.name, "TypeError", variant);
        return true;
      }, variant);
      if (!["retained-session", "retained-obligation", "missing-summary"].includes(variant)) assert.ok(!surface.actions.includes("detached"), variant);
    }
    surface.assertClean();
  }
});


test("recovery probe selects the owning CDP ancestor only for shared-frame sessions", async () => {
  const select = harnessFunction("browserRecoveryCdpSession");
  for (const owner of [0, 1, 2]) {
    const main = { parentFrame: () => null };
    const gui = { parentFrame: () => main };
    const browser = { parentFrame: () => gui };
    const seen = [];
    const session = {};
    const page = { context: () => ({ newCDPSession: async target => {
      seen.push(target);
      if (seen.length - 1 === owner) return session;
      throw new Error("This frame does not have a separate CDP session, it is a part of the parent frame's session");
    } }) };
    const result = await select(page, browser);
    assert.equal(result.session, session);
    assert.equal(result.ancestorDepth, owner);
    assert.deepEqual(seen, [browser, gui, page].slice(0, owner + 1));
  }
  const error = new Error("Browser connection terminated");
  let attempts = 0;
  const frame = { parentFrame: () => ({}) };
  const page = { context: () => ({ newCDPSession: async () => { attempts++; throw error; } }) };
  await assert.rejects(select(page, frame), value => value === error);
  assert.equal(attempts, 1);
});

test("recovery observer binds requests to the viewer and keeps their start generation after reload", async () => {
  const listeners = new Map(), messages = new Set(), records = [], disposed = [];
  const frameWindow = {};
  const rootWindow = { frames: [frameWindow], addEventListener: (_, fn) => messages.add(fn),
    removeEventListener: (_, fn) => messages.delete(fn) };
  const root = { parentFrame: () => null, evaluate: async () => 0 };
  const frame = { parentFrame: () => root,
    url: () => "http://localhost:61510/apps/browser/?browser_instance=instance-one#home_token=token-one",
    frameElement: async () => ({ dispose: async () => {} }) };
  const page = { mainFrame: () => root,
    on: (kind, fn) => { if (!listeners.has(kind)) listeners.set(kind, new Set()); listeners.get(kind).add(fn); },
    off: (kind, fn) => listeners.get(kind)?.delete(fn),
    exposeBinding: async (name, fn) => { rootWindow[name] = value => Promise.resolve(fn({ frame: root }, value)); },
    evaluateHandle: async (fn, args) => {
      const value = vm.runInNewContext(`(${fn.toString()})(args)`, { args, window: rootWindow });
      return { evaluate: async callback => callback(value), dispose: async () => disposed.push(true) };
    },
  };
  const emit = (kind, value) => { for (const fn of listeners.get(kind) || []) fn(value); };
  const request = (path, sourceFrame = frame, origin = "http://localhost:61510") => ({
    frame: () => sourceFrame, url: () => origin + path, method: () => "POST",
  });
  const observe = harnessFunction("observeControlledBrowserRequests", { assert: (value, message) => assert.ok(value, message) });
  const stop = await observe(page, frame, "token-one", value => records.push(value), { probeId: "probe-one", recordNavigation: true });
  try {
    emit("request", request("/api/apps/browser/open", root));
    emit("request", request("/api/apps/browser/open", frame, "http://foreign.invalid"));
    const old = request("/api/apps/browser/pages/page-one/status");
    emit("request", old);
    emit("framenavigated", root);
    emit("framenavigated", frame);
    emit("response", { request: () => old, status: () => 200 });
    emit("request", request("/api/apps/browser/pages/page-one/close"));
    assert.deepEqual(records.map(value => [value.kind, value.phase, value.document_generation]),
      [["status", "request", 0], ["navigation", "commit", 1], ["status", "response", 0], ["closing", "request", 1]]);
    assert.equal(records[0].request_id, records[2].request_id);
    const event = { source: frameWindow, origin: "null", data: {
      type: "elastos.home.browser-authority-renew.request/v1", homeToken: "token-one",
      browserInstance: "instance-one", requestId: "renew-one" } };
    for (const fn of messages) {
      fn({ ...event, source: {} });
      fn({ ...event, origin: "http://foreign.invalid" });
      fn({ ...event, data: { ...event.data, homeToken: "foreign-token" } });
      fn(event);
    }
    assert.equal(records.filter(value => value.kind === "renewal").length, 1);
    assert.ok(records.every(value => value.source_matches));
  } finally { await stop(); }
  assert.equal(messages.size, 0);
  assert.ok([...listeners.values()].every(value => value.size === 0));
  assert.equal(disposed.length, 1);
});

for (const observation of ["ready", "document-transition", "unexpected-viewer-error", "authority-error"]) {
  test(`viewer reload wrapper uses Runtime ownership and targets only the existing frame: ${observation}`, async () => {
    const requests = [], actions = [];
    const viewer = { page_id: "", browser_instance: "instance-one", actual_url: "", document_id: 100 };
    const frame = {
      url: () => "http://localhost:61510/apps/browser/?browser_instance=instance-one#home_token=token-one",
      evaluate: async fn => {
        if (fn.toString().includes("location.reload()")) { actions.push("reload-frame"); return; }
        if (observation === "document-transition") throw new Error("Execution context was destroyed");
        if (observation === "unexpected-viewer-error") throw new Error("unrelated viewer error");
        return { viewer, video: null };
      },
      waitForNavigation: async options => { assert.equal(options.waitUntil, "commit"); actions.push("wait-commit"); },
      locator: selector => ({ pressSequentially: async (suffix, options) => {
        assert.equal(selector, "#browser-keyboard-capture"); assert.equal(suffix, "-reload");
        assert.equal(options.timeout, 5000); actions.push("input");
      } }),
    };
    const page = {};
    const sessions = { recoverable_page: { page_id: "runtime-owner" } };
    const signal = new AbortController().signal;
    const wrapper = harnessFunction("runControlledBrowserViewerReload", {
      readBrowserViewerReloadDocument,
      markStage: () => {}, assert: (value, message) => assert.ok(value, message),
      fetch: async (url, options) => {
        requests.push({ url: String(url), options });
        assert.equal(options.signal, signal);
        assert.equal(options.headers.Origin, "null");
        assert.equal(options.headers["x-elastos-home-token"], "token-one");
        return { ok: observation !== "authority-error", status: observation === "authority-error" ? 401 : 200,
          json: async () => String(url).includes("/summary") ? { sessions } : { page_id: "runtime-owner" } };
      },
      observeControlledBrowserRequests: async (actualPage, actualFrame, token, record, options) => {
        assert.equal(actualPage, page); assert.equal(actualFrame, frame); assert.equal(token, "token-one");
        assert.equal(options.recordNavigation, true); return () => {};
      },
      diagnoseBrowserViewerReload: async callbacks => {
        const state = await callbacks.readState({ signal });
        assert.equal(state.sessions, sessions);
        assert.equal(state.page_status.page_id, "runtime-owner");
        assert.equal(state.viewer, observation === "document-transition" ? null : viewer);
        await callbacks.reloadViewer({ timeoutMs: 5000 });
        await callbacks.extendInput("-reload", { timeoutMs: 5000 });
        await callbacks.observeRequests(() => {}, { signal });
        return { ok: true };
      },
    });
    const run = wrapper(page, frame, "token-one", async () => {}, "http://localhost:61511/nav?run=fixture");
    if (["unexpected-viewer-error", "authority-error"].includes(observation)) {
      await assert.rejects(run, error => error.details.viewer_reload.failure === "probe_setup_or_observation_failed");
      assert.deepEqual(actions, []);
    } else {
      assert.equal((await run).ok, true);
      assert.deepEqual(actions, ["wait-commit", "reload-frame", "input"]);
      assert.match(requests[1].url, /\/pages\/runtime-owner\/status$/);
    }
  });
}

for (const variant of ["ready", "summary-rejected", "status-rejected", "viewer-rejected", "invalid-coordinates"]) {
  test(`operator wrapper binds existing owner, separate attach coordinates and UI callbacks: ${variant}`, async () => {
    const requests = [], actions = [];
    const sessions = { recoverable_page: { page_id: "page-one" } };
    const visible = { viewer: { page_id: "page-one" }, video: { decoded_frames: 10 } };
    const frame = {
      url: () => "http://localhost:61510/apps/browser/?browser_instance=instance-one#home_token=browser-token",
      evaluate: async fn => {
        assert.equal(fn, readBrowserViewerReloadDocument);
        if (variant === "viewer-rejected") throw new Error("viewer unavailable");
        return visible;
      },
      locator: selector => ({ pressSequentially: async (text, options) => {
        assert.equal(selector, "#browser-keyboard-capture"); assert.equal(text, "-human");
        assert.equal(options.timeout, 3000); actions.push("human-ui");
      } }),
    };
    const coords = { runtime_kind: "gateway", api_url: "http://127.0.0.1:61999", attach_secret: "private-attach-secret" };
    const receipt = async () => ({}), close = async () => { actions.push("ui-close"); return { closed: true }; };
    const signal = new AbortController().signal;
    const expectedUrl = "http://localhost:61511/main?run=fixture-run&media=1";
    const wrapper = harnessFunction("runControlledBrowserOperator", {
      BROWSER_OPERATOR_COORDS_PATH: "/explicit/task/runtime-coords.json", REQUIRE_BROWSER_VZ_TRANSPORT: true,
      readBrowserViewerReloadDocument, markStage: () => {}, assert: (value, message) => assert.ok(value, message),
      readFileSync: (path, encoding) => {
        assert.equal(path, "/explicit/task/runtime-coords.json"); assert.equal(encoding, "utf8");
        return variant === "invalid-coordinates" ? '{"private-attach-secret"' : JSON.stringify(coords);
      },
      fetch: async (url, options) => {
        const target = new URL(url); requests.push(target);
        assert.equal(target.origin, "http://localhost:61510");
        assert.equal(options.headers.Origin, "null");
        assert.equal(options.headers["x-elastos-home-token"], "browser-token");
        assert.equal(options.headers.Authorization, undefined); assert.equal(options.signal, signal);
        if (target.pathname.endsWith("summary")) {
          assert.equal(target.searchParams.get("browser_instance"), "instance-one");
          return { ok: variant !== "summary-rejected", json: async () => ({ sessions }) };
        }
        assert.equal(target.pathname, "/api/apps/browser/pages/page-one/status");
        return { ok: variant !== "status-rejected", json: async () => ({ page_id: "page-one" }) };
      },
      runBrowserOperatorJourney: async callbacks => {
        assert.equal(callbacks.runtimeCoords.api_url, coords.api_url);
        assert.equal(callbacks.runtimeCoords.attach_secret, coords.attach_secret);
        assert.equal(callbacks.runtimeOrigin, "http://localhost:61510");
        assert.equal(callbacks.homeToken, "browser-token"); assert.equal(callbacks.pageId, "page-one");
        assert.equal(callbacks.expectedUrl, expectedUrl); assert.equal(callbacks.requireVzTransport, true);
        assert.equal(callbacks.readReceipt, receipt); assert.equal(callbacks.closeWindow, close);
        const result = await callbacks.readState({ signal });
        assert.equal(result.sessions, sessions); assert.equal(result.page_status.page_id, "page-one");
        assert.equal(result.viewer, visible.viewer); assert.equal(result.video, visible.video);
        await callbacks.humanInput("-human", { timeoutMs: 2999.5 });
        await callbacks.closeWindow(); return { ok: true };
      },
    });
    const run = wrapper(frame, "browser-token", "page-one", expectedUrl, receipt, close);
    if (variant === "ready") {
      assert.equal((await run).ok, true); assert.deepEqual(actions, ["human-ui", "ui-close"]);
    } else {
      await assert.rejects(run, error => { assert.ok(!error.message.includes("private-attach-secret")); return true; });
      assert.deepEqual(actions, []);
      if (variant === "invalid-coordinates") assert.equal(requests.length, 0);
    }
  });
}

test("observer cancellation disposes a listener handle returned after its setup deadline", async () => {
  let begin, finish;
  const entered = new Promise(resolve => { begin = resolve; });
  const release = new Promise(resolve => { finish = resolve; });
  const controller = new AbortController(), listeners = new Map();
  let remoteListener = false, disposed = 0;
  const frame = { url: () => "http://localhost:61510/apps/browser/?browser_instance=instance-one", parentFrame: () => null };
  const page = {
    mainFrame: () => frame,
    on: (kind, fn) => { if (!listeners.has(kind)) listeners.set(kind, new Set()); listeners.get(kind).add(fn); },
    off: (kind, fn) => listeners.get(kind)?.delete(fn),
    exposeBinding: async () => {},
    evaluateHandle: async () => {
      remoteListener = true;
      begin();
      await release;
      return { evaluate: async callback => callback({ stop: async () => { remoteListener = false; } }),
        dispose: async () => { disposed++; } };
    },
  };
  const observe = harnessFunction("observeControlledBrowserRequests");
  const result = observe(page, frame, "token-one", () => {}, { probeId: "late-setup", signal: controller.signal })
    .then(value => ({ value }), error => ({ error }));
  await entered;
  controller.abort();
  finish();
  const outcome = await result;
  assert.match(outcome.error?.message || "", /observer setup canceled/);
  assert.equal(remoteListener, false);
  assert.equal(disposed, 1);
  assert.ok([...listeners.values()].every(value => value.size === 0));
});
