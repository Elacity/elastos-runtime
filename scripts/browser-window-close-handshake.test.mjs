#!/usr/bin/env node

import assert from "node:assert/strict";
import fs from "node:fs";
import test from "node:test";
import vm from "node:vm";

const browserSource = fs.readFileSync(
  new URL("../capsules/browser/browser/browser.js", import.meta.url),
  "utf8",
);
const homeGuiShellSource = fs.readFileSync(
  new URL("../capsules/home-gui/browser/home-gui-shell.js", import.meta.url),
  "utf8",
);
const homeShellHostSource = fs.readFileSync(
  new URL("../capsules/home/browser/home-shell-host.js", import.meta.url),
  "utf8",
);

function extractFunction(source, name) {
  const markers = [`async function ${name}(`, `function ${name}(`];
  const start = markers
    .map((marker) => source.indexOf(marker))
    .filter((index) => index >= 0)
    .sort((left, right) => left - right)[0];
  assert.notEqual(start, undefined, `${name} function not found`);
  const parametersOpen = source.indexOf("(", start);
  let parameterDepth = 0;
  let parametersClose = -1;
  for (let index = parametersOpen; index < source.length; index += 1) {
    if (source[index] === "(") parameterDepth += 1;
    if (source[index] === ")") parameterDepth -= 1;
    if (parameterDepth === 0) {
      parametersClose = index;
      break;
    }
  }
  const open = source.indexOf("{", parametersClose);
  let depth = 0;
  for (let index = open; index < source.length; index += 1) {
    if (source[index] === "{") depth += 1;
    if (source[index] === "}") depth -= 1;
    if (depth === 0) return source.slice(start, index + 1);
  }
  throw new Error(`${name} function body is not balanced`);
}

function numericConstant(source, name) {
  const match = source.match(
    new RegExp(`const ${name} = ([0-9_]+);`),
  );
  assert.ok(match, `${name} constant not found`);
  return Number(match[1].replaceAll("_", ""));
}

const handshakeSource = [
  "currentRuntimePageOwner",
  "finalizeRuntimePageClose",
  "hasExactMessageKeys",
  "isHomeBrowserWindowCloseRequest",
  "postHomeBrowserWindowCloseResult",
  "runtimeOpenResultPage",
  "pendingWindowCloseOwnership",
  "terminalRuntimeOpenOutcome",
  "terminalWindowCloseAbsence",
  "proveRuntimeOwnershipAbsentBeforeDispatch",
  "normalizeRuntimeOpenUrl",
  "settleUnresolvedRuntimeOpenForWindowClose",
  "classifyFreshWindowCloseSummary",
  "resolveRuntimeOwnershipForWindowClose",
  "settleInitialRuntimeOpenPostFailure",
  "handleHomeBrowserWindowCloseRequest",
]
  .map((name) => extractFunction(browserSource, name))
  .join("\n");

const authorityRenewalSource = [
  "hasExactMessageKeys",
  "browserAuthorityRenewalRequestId",
  "clearBrowserAuthorityRenewalRequest",
  "postBrowserAuthorityRenewalRequest",
  "scheduleBrowserAuthorityRenewalRetry",
  "settleBrowserAuthorityRenewal",
  "isHomeBrowserAuthorityRenewalResult",
  "handleHomeBrowserAuthorityRenewalResult",
  "requestHomeRelaunch",
  "requestFreshRuntimeAuthority",
  "startPageStatusPolling",
  "startPageHeartbeat",
]
  .map((name) => extractFunction(browserSource, name))
  .join("\n");

function createActiveAuthorityExpiryHarness(surface) {
  const error = new Error(`${surface} authority expired`);
  error.status = surface === "status" ? 401 : 403;
  const owner = Object.freeze({
    page_id: `page-${surface}`,
    generation: 4,
    runtime_cleanup: Object.freeze({
      schema: "elastos.browser.cleanup-handle/v1",
      id: `cleanup-${surface}`,
    }),
  });
  const page = {
    page_id: owner.page_id,
    runtime_cleanup: owner.runtime_cleanup,
  };
  const posts = [];
  const timers = new Map();
  let nextTimer = 1;
  let cleanupCalls = 0;
  const parent = {};
  const top = {
    postMessage(message, origin) {
      posts.push({ message, origin });
    },
  };
  let renewalRequestSerial = 0;
  const context = vm.createContext({
    BROWSER_AUTHORITY_RENEWAL_REQUEST_TYPE:
      "elastos.home.browser-authority-renew.request/v1",
    BROWSER_AUTHORITY_RENEWAL_RESULT_TYPE:
      "elastos.home.browser-authority-renew.result/v1",
    BROWSER_AUTHORITY_RENEWAL_ACK_TIMEOUT_MS: 40_000,
    BROWSER_AUTHORITY_RENEWAL_RETRY_DELAYS_MS: [1_200, 3_000, 10_000, 30_000],
    PAGE_HEARTBEAT_INTERVAL_MS: 60_000,
    PAGE_STATUS_FIRST_POLL_MS: 1_200,
    PAGE_STATUS_INTERVAL_MS: 2_500,
    currentPage: page,
    pageHeartbeatTimer: 0,
    pageStatusTimer: 0,
    relaunchRequested: false,
    browserAuthorityRenewal: null,
    browserAuthorityRenewalRetryTimer: 0,
    browserAuthorityRenewalAttempts: 0,
    browserInstanceId: "browser:authority-renewal-exact",
    launchToken: "expired-browser-authority",
    homeParentOrigin: "http://localhost:61180",
    document: { hidden: false },
    window: {
      parent,
      top,
      crypto: {
        randomUUID() {
          renewalRequestSerial += 1;
          return `browser-renewal-${renewalRequestSerial}`;
        },
      },
      setTimeout(callback, delay) {
        const id = nextTimer++;
        timers.set(id, { callback, delay });
        return id;
      },
      clearTimeout(id) {
        timers.delete(id);
      },
    },
    isAuthoritySessionError(candidate) {
      return candidate?.status === 401 || candidate?.status === 403;
    },
    friendlyOpenError() {
      return "Browser session expired. Reopening from Home...";
    },
    showStatus() {},
    stopPageStatusPolling() {
      for (const [id, timer] of timers) {
        if (timer.delay !== 60_000) timers.delete(id);
      }
      context.pageStatusTimer = 0;
    },
    stopPageHeartbeat() {
      for (const [id, timer] of timers) {
        if (timer.delay === 60_000) timers.delete(id);
      }
      context.pageHeartbeatTimer = 0;
    },
    isAddressEditing() {
      return false;
    },
    async fetchPageStatus() {
      if (surface === "status") throw error;
      return { schema: "elastos.browser.page-status/v1" };
    },
    async fetchJson() {
      if (surface === "heartbeat") throw error;
      return { ok: true };
    },
    async failRuntimeOwnedPage() {
      cleanupCalls += 1;
    },
    runtimePageCleanup: { status: () => null },
    currentRuntimePageOwner: () => owner,
  });
  vm.runInContext(
    `${authorityRenewalSource}\nthis.authority = { handleHomeBrowserAuthorityRenewalResult, requestHomeRelaunch, startPageStatusPolling, startPageHeartbeat };`,
    context,
  );
  return {
    owner,
    page,
    posts,
    cleanupCalls: () => cleanupCalls,
    handleRenewalResult(message, options = {}) {
      return context.authority.handleHomeBrowserAuthorityRenewalResult({
        origin: options.origin || "http://localhost:61180",
        source: options.source || top,
        data: message,
      });
    },
    requestAgain: () => context.authority.requestHomeRelaunch("duplicate"),
    start: surface === "status"
      ? context.authority.startPageStatusPolling
      : context.authority.startPageHeartbeat,
    async fire(delay) {
      const timer = [...timers.entries()].find(([, value]) => value.delay === delay);
      assert.ok(timer, `expected ${surface} timer ${delay}`);
      timers.delete(timer[0]);
      await timer[1].callback();
    },
    pendingTimers: () => [...timers.values()].map((timer) => timer.delay),
  };
}

function authorityRenewalResult(post, overrides = {}) {
  return {
    type: "elastos.home.browser-authority-renew.result/v1",
    requestId: post.message.requestId,
    homeToken: post.message.homeToken,
    browserInstance: post.message.browserInstance,
    ok: false,
    freshHomeToken: "",
    reason: "renewal_failed",
    ...overrides,
  };
}

function createHarness({
  outcome,
  closeError = null,
  openInFlight = 0,
  recoverable = false,
  ownerless = false,
  unsettledOpen = null,
  terminallyAbsent = false,
  fetchResponses = [],
} = {}) {
  const parent = {
    postMessage(message, origin) {
      posted.push({ message, origin });
    },
  };
  const page = {
    page_id: "page-exact",
    runtime_cleanup: {
      schema: "elastos.browser.cleanup-handle/v1",
      id: "cleanup-exact",
    },
  };
  const owner = Object.freeze({
    page_id: page.page_id,
    generation: recoverable || ownerless ? 0 : 7,
    runtime_cleanup: Object.freeze({ ...page.runtime_cleanup }),
  });
  const closeCalls = [];
  const fetchCalls = [];
  const posted = [];
  const statuses = [];
  const context = vm.createContext({
    BROWSER_WINDOW_CLOSE_REQUEST_TYPE:
      "elastos.browser.window-close.request/v1",
    BROWSER_WINDOW_CLOSE_RESULT_TYPE:
      "elastos.browser.window-close.result/v1",
    HOME_GUI_OPAQUE_ORIGIN: "null",
    browserInstanceId: "browser:0123456789abcdef0123456789abcdef",
    launchToken: "launch-token-exact",
    currentPage: recoverable || ownerless ? null : page,
    currentPageGeneration: 7,
    currentBrowserEngineId: "engine-exact",
    currentRemoteExitId: "exit-exact",
    browserSummaryPromise: null,
    browserSummary: null,
    runtimeOpenInFlight: openInFlight,
    unsettledRuntimeOpen: unsettledOpen,
    runtimeOwnershipTerminallyAbsent: terminallyAbsent,
    homeWindowCloseInFlight: false,
    homeWindowTerminalCloseConfirmed: false,
    window: { parent },
    recoverableRuntimePage() {
      return recoverable ? page : null;
    },
    runtimePageOwner(candidate, generation) {
      return candidate?.page_id === page.page_id &&
        candidate?.runtime_cleanup?.id === page.runtime_cleanup.id &&
        generation === owner.generation
        ? owner
        : null;
    },
    sameRuntimePageOwner(left, right) {
      return Boolean(
        left &&
          right &&
          left.page_id === right.page_id &&
          left.generation === right.generation &&
          left.runtime_cleanup?.id === right.runtime_cleanup?.id
      );
    },
    publishRuntimePageForHost() {},
    stopPageStatusPolling() {},
    stopPageHeartbeat() {},
    closeRemoteDisplay() {},
    updateMetricsNode() {},
    updateNavState() {},
    isAuthoritySessionError(error) {
      return error?.status === 401 || error?.status === 403;
    },
    normalizeUrl(value) {
      if (value === "invalid-initial-url") {
        throw new Error("invalid initial URL");
      }
      return String(value);
    },
    async closeRuntimePage(candidate, options) {
      closeCalls.push({ candidate, options });
      if (closeError) throw closeError;
      return outcome;
    },
    showStatus(message, options) {
      statuses.push({ message, options });
    },
    async fetchJson(path, options) {
      fetchCalls.push({ path, options });
      const response = fetchResponses.shift();
      if (response instanceof Error) throw response;
      return response;
    },
  });
  vm.runInContext(
    `${handshakeSource}\nthis.handshake = { finalizeRuntimePageClose, handleHomeBrowserWindowCloseRequest, normalizeRuntimeOpenUrl, settleInitialRuntimeOpenPostFailure };`,
    context,
  );
  const request = {
    type: "elastos.browser.window-close.request/v1",
    requestId: "request-exact",
    homeToken: "launch-token-exact",
    browserInstance: "browser:0123456789abcdef0123456789abcdef",
  };
  return {
    closeCalls,
    fetchCalls,
    owner,
    parent,
    posted,
    request,
    statuses,
    finalize: context.handshake.finalizeRuntimePageClose,
    handle: context.handshake.handleHomeBrowserWindowCloseRequest,
    normalizeOpenUrl: context.handshake.normalizeRuntimeOpenUrl,
    settleInitial: context.handshake.settleInitialRuntimeOpenPostFailure,
  };
}

test("exact parent request closes the exact owner and returns terminal receipt", async () => {
  const harness = createHarness({
    outcome: {
      state: "terminal",
      page_id: "page-exact",
      generation: 7,
      terminal_kind: "closed",
    },
  });
  const accepted = await harness.handle({
    origin: "null",
    source: harness.parent,
    data: harness.request,
  });
  assert.equal(accepted, true);
  assert.equal(harness.closeCalls.length, 1);
  assert.equal(harness.closeCalls[0].candidate, harness.owner);
  assert.deepEqual(
    { ...harness.closeCalls[0].options },
    { explicitRetry: true },
  );
  assert.deepEqual(Object.keys(harness.posted[0].message).sort(), [
    "browserInstance",
    "cleanupId",
    "generation",
    "homeToken",
    "pageId",
    "reason",
    "requestId",
    "state",
    "terminalKind",
    "type",
  ]);
  assert.deepEqual(
    { ...harness.posted[0].message },
    {
      type: "elastos.browser.window-close.result/v1",
      requestId: "request-exact",
      homeToken: "launch-token-exact",
      browserInstance: "browser:0123456789abcdef0123456789abcdef",
      state: "terminal",
      pageId: "page-exact",
      generation: 7,
      cleanupId: "cleanup-exact",
      terminalKind: "closed",
      reason: "",
    },
  );
  assert.equal(harness.posted[0].origin, "*");
});

test("timed-out open remains pending while its exact Runtime job is pending", async () => {
  const harness = createHarness({
    ownerless: true,
    unsettledOpen: {
      body: {
        url: "https://example.com/",
        async_open: true,
        browser_instance: "browser:0123456789abcdef0123456789abcdef",
      },
      failed: false,
      page: null,
      statusUrl: "/api/apps/browser/open/open-exact",
    },
    fetchResponses: [{
      schema: "elastos.browser.open-status/v1",
      open_id: "open-exact",
      status: "pending",
    }],
  });
  await harness.handle({
    origin: "null",
    source: harness.parent,
    data: harness.request,
  });
  assert.equal(harness.closeCalls.length, 0);
  assert.equal(harness.fetchCalls.length, 1);
  assert.equal(harness.fetchCalls[0].path, "/api/apps/browser/open/open-exact");
  assert.equal(harness.posted[0].message.state, "pending");
  assert.equal(harness.posted[0].message.terminalKind, "");
  assert.equal(harness.posted[0].message.reason, "runtime_open_status_pending");
});

test("timed-out open resolves its completed exact owner before terminal close", async () => {
  const harness = createHarness({
    ownerless: true,
    unsettledOpen: {
      body: {
        url: "https://example.com/",
        async_open: true,
        browser_instance: "browser:0123456789abcdef0123456789abcdef",
      },
      failed: false,
      page: null,
      statusUrl: "/api/apps/browser/open/open-exact",
    },
    fetchResponses: [{
      schema: "elastos.browser.open-status/v1",
      open_id: "open-exact",
      status: "completed",
      result: {
        schema: "elastos.browser.open-result/v1",
        engine_page: {
          schema: "elastos.browser.engine.page/v1",
          page_id: "page-exact",
        },
        runtime_cleanup: {
          schema: "elastos.browser.cleanup-handle/v1",
          id: "cleanup-exact",
        },
      },
    }],
    outcome: {
      state: "terminal",
      page_id: "page-exact",
      generation: 0,
      terminal_kind: "closed",
    },
  });
  await harness.handle({
    origin: "null",
    source: harness.parent,
    data: harness.request,
  });
  assert.equal(harness.fetchCalls.length, 1);
  assert.equal(harness.closeCalls.length, 1);
  assert.equal(harness.closeCalls[0].candidate, harness.owner);
  assert.deepEqual(
    { ...harness.closeCalls[0].options },
    { explicitRetry: true },
  );
  assert.equal(harness.posted[0].message.state, "terminal");
  assert.equal(harness.posted[0].message.pageId, "page-exact");
  assert.equal(harness.posted[0].message.cleanupId, "cleanup-exact");
});

test("ambiguous initial open replays the exact intent and remains nonterminal", async () => {
  const body = {
    url: "https://example.com/",
    async_open: true,
    browser_instance: "browser:0123456789abcdef0123456789abcdef",
  };
  const harness = createHarness({
    ownerless: true,
    unsettledOpen: {
      body,
      page: null,
      statusUrl: "",
    },
    fetchResponses: [{
      schema: "elastos.browser.open-accepted/v1",
      open_id: "open-exact",
      status: "pending",
      status_url: "/api/apps/browser/open/open-exact",
    }],
  });
  await harness.handle({
    origin: "null",
    source: harness.parent,
    data: harness.request,
  });
  assert.equal(harness.closeCalls.length, 0);
  assert.equal(harness.fetchCalls[0].path, "/api/apps/browser/open");
  assert.equal(harness.fetchCalls[0].options.method, "POST");
  assert.equal(harness.fetchCalls[0].options.body, body);
  assert.equal(harness.posted[0].message.state, "pending");
  assert.equal(harness.posted[0].message.reason, "runtime_open_status_pending");
});

test("only an exact typed terminal open outcome proves ownerless absence", async () => {
  const harness = createHarness({
    ownerless: true,
    unsettledOpen: {
      body: { url: "https://example.com/", async_open: true },
      page: null,
      statusUrl: "/api/apps/browser/open/open-exact",
    },
    fetchResponses: [{
      schema: "elastos.browser.open-status/v1",
      open_id: "open-exact",
      status: "failed",
      error: {
        outcome: {
          schema: "elastos.browser.open-outcome/v1",
          state: "terminal_post_effect_cleanup",
        },
      },
    }],
  });
  await harness.handle({
    origin: "null",
    source: harness.parent,
    data: harness.request,
  });
  assert.equal(harness.closeCalls.length, 0);
  assert.equal(harness.fetchCalls.length, 1);
  assert.equal(harness.posted[0].message.state, "terminal");
  assert.equal(harness.posted[0].message.terminalKind, "no_page");
  assert.equal(harness.posted[0].message.reason, "");
});

test("cleanup-pending outcome plus an empty summary never proves absence", async () => {
  const harness = createHarness({
    ownerless: true,
    unsettledOpen: {
      body: { url: "https://example.com/", async_open: true },
      page: null,
      statusUrl: "/api/apps/browser/open/open-exact",
    },
    fetchResponses: [
      {
        schema: "elastos.browser.open-status/v1",
        open_id: "open-exact",
        status: "failed",
        error: {
          outcome: {
            schema: "elastos.browser.open-outcome/v1",
            state: "cleanup_pending",
          },
        },
      },
      {
        schema: "elastos.browser.runtime/v1",
        sessions: {
          schema: "elastos.browser.session-capacity/v1",
          status: "configured",
          launching_sessions: 0,
          launch_reconciliation_obligations: 0,
          engine_cleanup_obligations: 0,
          recoverable_page: null,
        },
      },
    ],
  });
  await harness.handle({
    origin: "null",
    source: harness.parent,
    data: harness.request,
  });
  assert.equal(harness.closeCalls.length, 0);
  assert.equal(harness.fetchCalls.length, 2);
  assert.match(harness.fetchCalls[1].path, /browser_instance=/);
  assert.equal(harness.posted[0].message.state, "pending");
  assert.equal(harness.posted[0].message.terminalKind, "");
  assert.equal(harness.posted[0].message.reason, "runtime_ownership_unproven");
});

test("exact internal terminal cleanup proves absence to the later window close", async () => {
  const harness = createHarness();
  assert.equal(harness.finalize(harness.owner), true);
  await harness.handle({
    origin: "null",
    source: harness.parent,
    data: harness.request,
  });
  assert.equal(harness.fetchCalls.length, 0);
  assert.equal(harness.closeCalls.length, 0);
  assert.equal(harness.posted[0].message.state, "terminal");
  assert.equal(harness.posted[0].message.terminalKind, "no_page");
});

test("received initial authority failure proves pre-dispatch absence for relaunch close", async () => {
  const settlement = {
    body: { url: "https://example.com/", async_open: true },
    page: null,
    statusUrl: "",
  };
  const harness = createHarness({
    ownerless: true,
    unsettledOpen: settlement,
  });
  assert.equal(harness.settleInitial(settlement, { status: 401 }), true);
  await harness.handle({
    origin: "null",
    source: harness.parent,
    data: harness.request,
  });
  assert.equal(harness.fetchCalls.length, 0);
  assert.equal(harness.closeCalls.length, 0);
  assert.equal(harness.posted[0].message.state, "terminal");
  assert.equal(harness.posted[0].message.terminalKind, "no_page");
});

test("status-less initial transport failure remains unsettled", async () => {
  const settlement = {
    body: { url: "https://example.com/", async_open: true },
    page: null,
    statusUrl: "",
  };
  const harness = createHarness({
    ownerless: true,
    unsettledOpen: settlement,
    fetchResponses: [new Error("still offline")],
  });
  assert.equal(harness.settleInitial(settlement, new Error("offline")), false);
  await harness.handle({
    origin: "null",
    source: harness.parent,
    data: harness.request,
  });
  assert.equal(harness.fetchCalls.length, 1);
  assert.equal(harness.posted[0].message.state, "pending");
  assert.equal(
    harness.posted[0].message.reason,
    "runtime_open_status_unavailable",
  );
});

test("initial 409 conflict never proves Browser ownership absent", async () => {
  const settlement = {
    body: { url: "https://example.com/", async_open: true },
    page: null,
    statusUrl: "",
  };
  const conflict = new Error("Browser instance already owns an open");
  conflict.status = 409;
  const harness = createHarness({
    ownerless: true,
    unsettledOpen: settlement,
    fetchResponses: [conflict],
  });
  assert.equal(harness.settleInitial(settlement, conflict), false);
  await harness.handle({
    origin: "null",
    source: harness.parent,
    data: harness.request,
  });
  assert.equal(harness.fetchCalls.length, 1);
  assert.equal(harness.posted[0].message.state, "pending");
  assert.equal(harness.posted[0].message.terminalKind, "");
  assert.equal(
    harness.posted[0].message.reason,
    "runtime_open_status_unavailable",
  );
});

test("initial normalization rejection before dispatch proves local absence", async () => {
  const harness = createHarness({ ownerless: true });
  assert.throws(
    () => harness.normalizeOpenUrl("invalid-initial-url"),
    /invalid initial URL/,
  );
  await harness.handle({
    origin: "null",
    source: harness.parent,
    data: harness.request,
  });
  assert.equal(harness.fetchCalls.length, 0);
  assert.equal(harness.closeCalls.length, 0);
  assert.equal(harness.posted[0].message.state, "terminal");
  assert.equal(harness.posted[0].message.terminalKind, "no_page");
});

test("active authority expiry retries exact renewal and accepts only its bound success", async () => {
  for (const [surface, delay] of [
    ["status", 1_200],
    ["heartbeat", 60_000],
  ]) {
    const harness = createActiveAuthorityExpiryHarness(surface);
    harness.start();
    await harness.fire(delay);
    assert.equal(harness.cleanupCalls(), 0, `${surface} attempted cleanup`);
    assert.equal(harness.posts.length, 1, `${surface} did not request relaunch`);
    assert.deepEqual(
      { ...harness.posts[0].message },
      {
        type: "elastos.home.browser-authority-renew.request/v1",
        requestId: "browser-renewal-1",
        homeToken: "expired-browser-authority",
        browserInstance: "browser:authority-renewal-exact",
      },
    );
    assert.equal(harness.posts[0].origin, "http://localhost:61180");
    assert.equal(harness.requestAgain(), false, `${surface} duplicated renewal`);
    const forgedSuccess = authorityRenewalResult(harness.posts[0], {
      ok: true,
      freshHomeToken: "fresh-browser-authority",
      reason: "",
    });
    assert.equal(
      harness.handleRenewalResult(forgedSuccess, { source: {} }),
      false,
      `${surface} accepted a substituted result source`,
    );
    assert.deepEqual(harness.pendingTimers(), [40_000]);
    assert.equal(
      harness.handleRenewalResult(authorityRenewalResult(harness.posts[0])),
      true,
    );
    assert.deepEqual(harness.pendingTimers(), [1_200]);
    await harness.fire(1_200);
    assert.equal(harness.posts.length, 2, `${surface} did not retry renewal`);
    assert.equal(harness.posts[1].message.requestId, "browser-renewal-2");
    assert.equal(
      harness.handleRenewalResult(
        authorityRenewalResult(harness.posts[1], {
          ok: true,
          freshHomeToken: "fresh-browser-authority",
          reason: "",
        }),
      ),
      true,
    );
    assert.equal(harness.page.page_id, harness.owner.page_id);
    assert.deepEqual(harness.pendingTimers(), []);
  }
});

test("authority renewal remains live at the capped retry interval", async () => {
  const harness = createActiveAuthorityExpiryHarness("status");
  harness.start();
  await harness.fire(1_200);
  for (const retryDelay of [1_200, 3_000, 10_000, 30_000, 30_000]) {
    const current = harness.posts.at(-1);
    assert.equal(harness.handleRenewalResult(authorityRenewalResult(current)), true);
    assert.deepEqual(harness.pendingTimers(), [retryDelay]);
    await harness.fire(retryDelay);
    assert.equal(harness.requestAgain(), false);
  }
  assert.equal(harness.posts.length, 6);
  assert.equal(harness.cleanupCalls(), 0);
  assert.deepEqual(harness.pendingTimers(), [40_000]);
});

test("GUI launch timeout precedes host and Browser renewal timeouts", () => {
  const guiTimeout = numericConstant(
    homeGuiShellSource,
    "HOME_GUI_REQUEST_TIMEOUT_MS",
  );
  const hostTimeout = numericConstant(
    homeShellHostSource,
    "BROWSER_AUTHORITY_RENEWAL_TIMEOUT_MS",
  );
  const browserTimeout = numericConstant(
    browserSource,
    "BROWSER_AUTHORITY_RENEWAL_ACK_TIMEOUT_MS",
  );
  assert.ok(guiTimeout < hostTimeout);
  assert.ok(hostTimeout < browserTimeout);
});

test("late Home launch response cannot resume a timed-out GUI renewal", async () => {
  const timers = new Map();
  const posts = [];
  const pendingRequests = new Map();
  let nextTimer = 1;
  const context = vm.createContext({
    HOME_GUI_REQUEST_TIMEOUT_MS: 30_000,
    pendingRequests,
    requestId: () => "late-renewal",
    postToHome: (message) => posts.push(message),
    Error,
    window: {
      setTimeout(callback, delay) {
        const id = nextTimer++;
        timers.set(id, { callback, delay });
        return id;
      },
      clearTimeout(id) {
        timers.delete(id);
      },
    },
  });
  vm.runInContext(
    `${extractFunction(homeGuiShellSource, "requestHome")}\n${extractFunction(homeGuiShellSource, "settleRequest")}\nthis.guiRequest = { requestHome, settleRequest };`,
    context,
  );
  let applied = 0;
  const renewal = context.guiRequest
    .requestHome("home:launch-target", {
      target: "browser",
      query: { browser_instance: "browser:late-renewal" },
    }, { timeoutMs: 5_000 })
    .then(
      () => {
        applied += 1;
      },
      () => {},
    );
  const timeout = [...timers.values()].find(
    (timer) => timer.delay === 5_000,
  );
  assert.ok(timeout);
  timeout.callback();
  await renewal;
  assert.equal(pendingRequests.size, 0);
  assert.equal(
    context.guiRequest.settleRequest({
      type: "home:shell-response",
      requestId: "late-renewal",
      result: { route: "/apps/browser/#home_token=too-late" },
    }),
    false,
  );
  assert.equal(applied, 0);
  assert.equal(posts.length, 1);
});

test("expired GUI renewal command cannot start a Browser launch", async () => {
  const results = [];
  let renewalCalls = 0;
  const context = vm.createContext({
    BROWSER_AUTHORITY_RENEWAL_RESULT_TYPE:
      "elastos.home.browser-authority-renew.result/v1",
    HOME_GUI_REQUEST_TIMEOUT_MS: 30_000,
    postToHome: (message) => results.push(message),
    renewHomeGuiBrowserWindowAuthority() {
      renewalCalls += 1;
      return Promise.resolve({
        browserInstance: "browser:expired-command",
        freshHomeToken: "unexpected-fresh-token",
      });
    },
  });
  vm.runInContext(
    `${extractFunction(homeGuiShellSource, "hasExactKeys")}\n${extractFunction(homeGuiShellSource, "handleBrowserAuthorityRenewalCommand")}\nthis.guiRenewal = { handleBrowserAuthorityRenewalCommand };`,
    context,
  );
  const handled = await context.guiRenewal.handleBrowserAuthorityRenewalCommand({
    type: "home:gui-command",
    command: "renew-browser-authority",
    requestId: "expired-command",
    oldHomeToken: "expired-old-token",
    browserInstance: "browser:expired-command",
    expiresAt: 0,
  });
  assert.equal(handled, false);
  assert.equal(renewalCalls, 0);
  assert.deepEqual({ ...results[0] }, {
    type: "elastos.home.browser-authority-renew.result/v1",
    requestId: "expired-command",
    oldHomeToken: "expired-old-token",
    browserInstance: "browser:expired-command",
    ok: false,
    freshHomeToken: "",
    reason: "renewal_failed",
  });
});

test("pending cleanup returns a retryable nonterminal receipt", async () => {
  const harness = createHarness({
    outcome: {
      state: "pending",
      page_id: "page-exact",
      generation: 7,
      reason: "transport_failure",
    },
  });
  await harness.handle({
    origin: "null",
    source: harness.parent,
    data: harness.request,
  });
  assert.equal(harness.posted[0].message.state, "pending");
  assert.equal(harness.posted[0].message.reason, "transport_failure");
  assert.match(harness.statuses[0].message, /again to retry/);
  assert.equal(harness.statuses[0].options.sticky, true);
});

test("ownership-changing open returns pending without closing or claiming terminal", async () => {
  const harness = createHarness({
    openInFlight: 1,
    outcome: {
      state: "terminal",
      page_id: "page-exact",
      generation: 7,
      terminal_kind: "closed",
    },
  });
  await harness.handle({
    origin: "null",
    source: harness.parent,
    data: harness.request,
  });
  assert.equal(harness.closeCalls.length, 0);
  assert.equal(harness.posted[0].message.state, "pending");
  assert.equal(harness.posted[0].message.terminalKind, "");
  assert.equal(harness.posted[0].message.reason, "runtime_open_in_flight");
  assert.match(harness.statuses[0].message, /ownership is changing/);
});

test("recoverable Runtime owner is closed exactly when no current page exists", async () => {
  const harness = createHarness({
    recoverable: true,
    outcome: {
      state: "terminal",
      page_id: "page-exact",
      generation: 0,
      terminal_kind: "already_absent",
    },
  });
  await harness.handle({
    origin: "null",
    source: harness.parent,
    data: harness.request,
  });
  assert.equal(harness.closeCalls.length, 1);
  assert.equal(harness.closeCalls[0].candidate, harness.owner);
  assert.equal(harness.posted[0].message.state, "terminal");
  assert.equal(harness.posted[0].message.terminalKind, "already_absent");
  assert.equal(harness.posted[0].message.cleanupId, "cleanup-exact");
});

test("source, token, instance, origin, type, and shape substitutions fail closed", async () => {
  const mutations = [
    (harness) => ({ source: {} }),
    () => ({ origin: "http://localhost:61180" }),
    (harness) => ({ data: { ...harness.request, homeToken: "wrong" } }),
    (harness) => ({ data: { ...harness.request, browserInstance: "browser:wrong" } }),
    (harness) => ({ data: { ...harness.request, type: "browser:close" } }),
    (harness) => ({ data: { ...harness.request, extra: true } }),
  ];
  for (const mutate of mutations) {
    const harness = createHarness({ outcome: { state: "terminal" } });
    const event = {
      origin: "null",
      source: harness.parent,
      data: harness.request,
      ...mutate(harness),
    };
    assert.equal(await harness.handle(event), false);
    assert.equal(harness.closeCalls.length, 0);
    assert.equal(harness.posted.length, 0);
  }
});

test("unexpected close errors return error receipts without implying terminal", async () => {
  const harness = createHarness({ closeError: new Error("boom") });
  await harness.handle({
    origin: "null",
    source: harness.parent,
    data: harness.request,
  });
  assert.equal(harness.posted[0].message.state, "error");
  assert.equal(harness.posted[0].message.terminalKind, "");
  assert.equal(harness.posted[0].message.reason, "close_error");
});

test("iframe unload and refresh remain teardown-only", () => {
  const unloadBlock = extractFunction(browserSource, "releaseRuntimePageForUnload");
  assert.doesNotMatch(unloadBlock, /closeRuntimePage\s*\(/);
  assert.match(
    browserSource,
    /window\.addEventListener\("beforeunload", \(\) => \{\s*releaseRuntimePageForUnload\(\);\s*\}\);/,
  );
  assert.match(
    browserSource,
    /window\.addEventListener\("pagehide", releaseRuntimePageForUnload\);/,
  );
});
