import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";
import { createBrowserJourneyFixture } from "./browser-journey-fixture.mjs";

const source = readFileSync(new URL("../home-passkey-virtual-auth-smoke.mjs", import.meta.url), "utf8");
function harnessFunction(name, globals = {}) {
  const start = source.search(new RegExp(`(?:async )?function ${name}\\(`));
  assert.ok(start >= 0, name);
  const next = source.slice(start + 1).search(/\n(?:async )?function /);
  const declaration = source.slice(start, start + 1 + next);
  return vm.runInNewContext(`(${declaration})`, { URL, Date, performance, ...globals });
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
    parentFrame: () => gui, frameElement: async () => ({ contentWindow: source }), waitForFunction: async () => {} };
  const page = {
    on: (event, listener) => { if (!listeners.has(event)) listeners.set(event, new Set()); listeners.get(event).add(listener); },
    off: (event, listener) => listeners.get(event)?.delete(listener),
  };
  const surface = { gui, frame, page, actions,
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
  surface.window = { getByRole: () => ({ click: async () => { actions.push("ui-close"); await surface.click(); } }),
    waitFor: async () => { actions.push("detached"); await surface.emit("framedetached", frame); } };
  surface.click = async () => surface.message();
  return surface;
}

function closeHarnessFunction(globals = {}) {
  return harnessFunction("closeControlledBrowserWindow", {
    BROWSER_UI_PAGE_ID_TIMEOUT_MS: 180_000,
    REQUIRE_BROWSER_VZ_TRANSPORT: false,
    markStage: () => {}, assert: (condition, message) => assert.ok(condition, message),
    redactSensitiveString: harnessFunction("redactSensitiveString"),
    waitForJourneyEvidence: async (read, predicate) => { const value = await read(); assert.ok(predicate(value)); return value; },
    ...globals,
  });
}

test("startup failure can close through Home before a page id exists", async () => {
  const baseline = { principal_sessions: 0, total_sessions: 0, launching_sessions: 0,
    engine_cleanup_obligations: 0, launch_reconciliation_obligations: 0 };
  const surface = closeHarnessSurface();
  const close = closeHarnessFunction({
    browserApi: async frame => { assert.equal(frame, surface.gui); return { ok: true,
      body: { sessions: { ...baseline, recoverable_page: null } } }; },
  });
  const result = await close(surface.page, surface.frame, surface.window, "browser-token", baseline);
  assert.equal(result.startup_close.terminal_kind, "no_page");
  assert.equal(result.close_evidence.messages[0].requestId, "close-one");
  assert.equal(result.close_evidence.frames[0].event, "detached");
  assert.deepEqual(surface.actions, ["ui-close", "detached", "disposed"]);
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

test("journey success pins close to its last page while earlier failure retains cleanup fallback", async () => {
  for (const inputFails of [false, true]) {
    const runId = "journey-test-run";
    const events = [];
    const closes = [];
    const inputError = new Error("input delivery failed");
    let url = "";
    let pageName = "";
    let value = "";
    let frames = 0;
    const report = type => events.push({ type, page: pageName, value, scroll_y: type === "scroll" ? 640 : 0,
      input_rect: { x: 32, y: 150, width: 480, height: 60 } });
    const appFrame = {
      waitForFunction: async () => {},
      locator: () => ({ waitFor: async () => {}, hover: async () => {}, click: async () => {},
        fill: async target => { url = target; pageName = new URL(target).pathname.slice(1); },
        press: async key => { assert.equal(key, "Enter"); report("load"); },
        pressSequentially: async character => { if (inputFails) throw inputError; value += character; report("input"); },
      }),
    };
    const page = { mouse: { wheel: async () => report("scroll") } };
    const baseline = {};
    const window = {};
    const journey = harnessFunction("runControlledBrowserJourney", {
      BROWSER_JOURNEY_FIXTURE_ORIGIN: "http://localhost:61511", BROWSER_OPEN_DISPLAY_MODE: "webrtc_remote_display",
      BROWSER_UI_PAGE_ID_TIMEOUT_MS: 180_000, BROWSER_REMOTE_VIDEO_TIMEOUT_MS: 30_000,
      randomUUID: () => runId, AbortSignal, smokeStage: "input", markStage: () => {},
      assert: (condition, message) => assert.ok(condition, message),
      fetch: async () => ({ ok: true, status: 200,
        json: async () => ({ schema: "elastos.browser.journey-receipt/v1", run: runId, events }) }),
      waitForEmbeddedBrowserPage: async () => `page-${pageName}`,
      browserApi: async () => ({ ok: true, body: { schema: "elastos.browser.page-status/v1", actual_url: url,
        direct_network: false, display_session: { media_transport: "runtime_relay" } } }),
      runtimeRelayIceContractOk: () => true,
      waitForJourneyEvidence: async (read, predicate) => { const result = await read(); assert.ok(predicate(result)); return result; },
      waitForBrowserRemoteVideo: async () => ({ decoded_frames: frames }),
      browserRemoteVideoMetrics: async () => ({ decoded_frames: ++frames }),
      remoteVideoClickPositionForPagePoint: async () => ({ x: 50, y: 50 }),
      closeControlledBrowserWindow: async (...args) => { closes.push(args); return { receipt: { closed: true } }; },
    });
    if (inputFails) await assert.rejects(journey(page, appFrame, window, "browser-token", baseline, []),
      error => error === inputError);
    else assert.equal((await journey(page, appFrame, window, "browser-token", baseline, [])).page_id, "page-nav");
    assert.equal(closes.length, 1);
    assert.deepEqual(closes[0].slice(0, 5), [page, appFrame, window, "browser-token", baseline]);
    assert.equal(closes[0][5]?.expectedPageId, inputFails ? null : "page-nav");
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
    "wrong-message-page", "wrong-message-cleanup", "wrong-terminal-kind"]) {
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
      REQUIRE_BROWSER_VZ_TRANSPORT: ["valid-vz", "missing-vz-proof"].includes(variant),
      browserApi: async frame => { assert.equal(frame, opaqueGui); return { ok: true, body: { sessions: reads++ === 0 ? { recoverable_page: owner } : {
        schema: "elastos.browser.session-capacity/v1", ...baseline,
        principal_sessions: variant === "session-retained" ? 1 : 0, recoverable_page: null,
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
    if (["valid", "valid-vz"].includes(variant)) {
      const result = await close(page, surface.frame, surface.window, "browser-token", baseline, options);
      assert.equal(result.receipt.cleanup_id, owner.cleanup.id);
      assert.equal(result.receipt.page_id, options.expectedPageId);
      assert.equal(result.close_evidence.close_responses[0].authority_matches, true);
      assert.equal(result.close_evidence.messages.at(-1).terminalKind, "closed");
      assert.deepEqual(actions, ["ui-close", "detached", "disposed"]);
    } else {
      await assert.rejects(close(page, surface.frame, surface.window, "browser-token", baseline, options),
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
