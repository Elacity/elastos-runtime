import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";
import { runtimePageOwner, sameRuntimePageOwner } from "../capsules/browser/browser/browser-page-cleanup.js";
import { isAuthoritySessionError } from "../capsules/browser/browser/browser-status.js";

const browserSource = readFileSync(new URL("../capsules/browser/browser/browser.js", import.meta.url), "utf8");
const apiSource = readFileSync(new URL("../capsules/browser/browser/browser-runtime-api.js", import.meta.url), "utf8");
function declaration(source, name) {
  const start = source.search(new RegExp(`(?:async )?function ${name}\\(`));
  const end = source.slice(start).search(/\n\}(?=\n|$)/);
  assert.ok(start >= 0 && end > 0, `missing actual function ${name}`);
  return source.slice(start, start + end + 2);
}
function response(body, status = 200) {
  return { ok: status >= 200 && status < 300, status, text: async () => JSON.stringify(body) };
}
function createApi(fetch) {
  const factory = vm.runInNewContext(`(${declaration(apiSource, "createRuntimeApi")})`, {
    fetch, requireBrowserViewer() {},
  });
  return factory({ launchToken: "fixture-only-token" });
}
function deferred() {
  let resolve, reject;
  const promise = new Promise((ok, fail) => { resolve = ok; reject = fail; });
  return { promise, resolve, reject };
}

const routes = {
  status: { start: "startPageStatusPolling", timer: "pageStatusTimer" },
  heartbeat: { start: "startPageHeartbeat", timer: "pageHeartbeatTimer" },
  afterInput: { start: "schedulePageStatusRefresh", timer: "pageStatusRefreshTimers" },
};
function harness() {
  const timers = new Map(), requests = [], failures = [], renewals = [], mutations = [], messages = [];
  let ordinal = 0, mediaOpen = true;
  const state = vm.createContext({
    currentPage: { page_id: "page-current", actual_url: "https://fixture.invalid/nav", title: "Before",
      runtime_cleanup: { schema: "elastos.browser.cleanup-handle/v1", id: "cleanup-current" } },
    unloadCleanupStarted: false, currentPageGeneration: 1, pageStatusTimer: 0, pageHeartbeatTimer: 0, pageStatusRefreshTimers: [],
    relaunchRequested: false, lastPageStatus: null, homeWindowCloseInFlight: false,
    homeWindowTerminalCloseConfirmed: false, pendingHomeWindowCloseDelivery: null,
    document: { hidden: false }, isAddressEditing: () => false,
    runtimePageOwner, sameRuntimePageOwner, isAuthoritySessionError,
    friendlyOpenError: error => error.message,
    showStatus: message => messages.push(message),
    closeRemoteDisplay: () => { mediaOpen = false; },
    requestHomeRelaunch: reason => { renewals.push(reason); state.relaunchRequested = true; return true; },
    runtimePageCleanup: {
      status: owner => failures.find(entry => sameRuntimePageOwner(entry.owner, owner)) || null,
      fail: async (owner, failure) => { failures.push({ owner, failure }); return { state: "pending" }; },
    },
    syncViewFromResponse: value => mutations.push(["view", value]),
    handleFileChooserFromStatus: value => mutations.push(["chooser", value]),
    syncBrowserLocation: (...args) => mutations.push(["location", ...args]),
    updateMetricsNode: value => mutations.push(["metrics", value]),
    window: {
      setTimeout: (callback, delay) => { const id = ++ordinal; timers.set(id, { callback, delay }); return id; },
      clearTimeout: id => timers.delete(id),
    },
  });
  const healthyStatus = () => ({ schema: "elastos.browser.page-status/v1", page_id: state.currentPage.page_id,
    direct_network: false, actual_url: state.currentPage.actual_url, title: "After" });
  let fetch = async () => response(healthyStatus());
  state.fetchJson = createApi((path, options) => {
    requests.push({ path, method: options.method });
    return fetch(path, options);
  }).fetchJson;
  const constants = browserSource.match(/^const PAGE_(?:STATUS|HEARTBEAT)_[A-Z_]+ = .*;$/gm);
  assert.ok(constants?.length);
  const functions = ["currentRuntimePageOwner", "runtimeViewerOwnerActive", "stopPageStatusPolling", "stopPageStatusRefresh", "stopPageHeartbeat",
    "requestFreshRuntimeAuthority", "runtimeOwnedFailureSummary", "failRuntimeOwnedPage", "fetchPageStatus",
    "handlePageObservationFailure", "schedulePageStatusRefresh", "startPageStatusPolling", "startPageHeartbeat"];
  vm.runInContext(constants.join("\n") + "\n" + functions.map(name => declaration(browserSource, name)).join("\n"), state);
  const timerFor = route => {
    const value = state[routes[route].timer];
    return Array.isArray(value) ? value[0] : value;
  };
  return { state, timers, requests, failures, renewals, mutations, messages, healthyStatus,
    mediaOpen: () => mediaOpen,
    setFetch: value => { fetch = value; },
    start: route => state[routes[route].start](),
    async fire(route) {
      const id = timerFor(route), timer = timers.get(id);
      assert.ok(timer, `missing ${route} timer`);
      timers.delete(id);
      await timer.callback();
    },
    nextDelay: route => timers.get(timerFor(route))?.delay,
    constant: name => vm.runInContext(name, state),
  };
}
function assertRetained(h, owner) {
  assert.equal(sameRuntimePageOwner(h.state.currentRuntimePageOwner(), owner), true);
  assert.equal(h.mediaOpen(), true);
  assert.deepEqual(h.failures, []);
  assert.deepEqual(h.renewals, []);
}

for (const boundary of ["fetch", "response.text"]) {
  test(`fetchJson marks only the actual ${boundary} rejection as transport failure`, async () => {
    const cause = new TypeError("fixture network failure");
    const api = createApi(async () => {
      if (boundary === "fetch") throw cause;
      return { ok: true, text: async () => { throw cause; } };
    });
    await assert.rejects(api.fetchJson("/fixture"), error => {
      assert.equal(error.runtimeTransportFailure, true);
      assert.equal(error.cause, cause);
      assert.equal(error.status, undefined);
      return true;
    });
  });
}

test("JSON encoding failures occur before fetch and retain their non-transport classification", async () => {
  let called = false;
  const api = createApi(async () => { called = true; return response({}); });
  await assert.rejects(api.fetchJson("/fixture", { body: { unsupported: 1n } }), error =>
    error.name === "TypeError" && error.runtimeTransportFailure !== true);
  assert.equal(called, false);
});
test("invalid response JSON remains payload validation work, not a transport failure", async () => {
  const api = createApi(async () => ({ ok: true, text: async () => "{invalid json" }));
  assert.equal(await api.fetchJson("/fixture"), "{invalid json");
});
for (const status of [401, 403, 404, 500, 502, 503, 504]) {
  test(`completed HTTP ${status} stays untagged`, async () => {
    const api = createApi(async () => response({ error: "fixture HTTP rejection" }, status));
    await assert.rejects(api.fetchJson("/fixture"), error => error.status === status && error.runtimeTransportFailure !== true);
  });
}
for (const status of [401, 403, 500]) {
  test(`HTTP ${status} headers survive a body-read failure without a transport tag`, async () => {
    const api = createApi(async () => ({ ok: false, status,
      text: async () => { throw new TypeError("body interrupted after headers"); } }));
    await assert.rejects(api.fetchJson("/fixture"), error => error.status === status && error.runtimeTransportFailure !== true);
  });
}

for (const route of Object.keys(routes)) {
  for (const boundary of ["fetch", "response.text"]) {
    test(`${route} retains owner, media and scheduling after its first ${boundary} failure`, async () => {
      const h = harness(), owner = h.state.currentRuntimePageOwner();
      h.setFetch(async () => {
        if (boundary === "fetch") throw new TypeError("Failed to fetch");
        return { ok: true, text: async () => { throw new TypeError("Failed to read body"); } };
      });
      h.start(route);
      await h.fire(route);
      assertRetained(h, owner);
      assert.ok(h.timers.size > 0, "observations must continue");
      assert.equal(h.requests.length, 1);
      if (route !== "afterInput") assert.equal(h.nextDelay(route), h.constant("PAGE_STATUS_INTERVAL_MS"));
      else assert.equal(h.timers.size, 3, "keep the remaining scheduled input observations");

      h.setFetch(async () => response(h.healthyStatus()));
      await h.fire(route);
      assertRetained(h, owner);
      assert.equal(h.requests.length, 2);
      if (route === "heartbeat") assert.equal(h.nextDelay(route), h.constant("PAGE_HEARTBEAT_INTERVAL_MS"));
      else assert.ok(h.mutations.length > 0, "the later healthy status still updates the viewer");
    });
  }

  test(`${route} preserves Home authority renewal on HTTP 401`, async () => {
    const h = harness(), owner = h.state.currentRuntimePageOwner();
    h.setFetch(async () => response({ error: "auth session is not active" }, 401));
    h.start(route);
    await h.fire(route);
    assert.equal(h.renewals.length, 1);
    assert.deepEqual(h.failures, []);
    assert.equal(h.mediaOpen(), true);
    assert.equal(sameRuntimePageOwner(h.state.currentRuntimePageOwner(), owner), true);
    assert.equal(h.timers.size, 0);
  });

  for (const status of [401, 403, 500]) {
    test(`${route} respects HTTP ${status} when its response body fails`, async () => {
      const h = harness(), owner = h.state.currentRuntimePageOwner();
      h.setFetch(async () => ({ ok: false, status,
        text: async () => { throw new TypeError("body interrupted after headers"); } }));
      h.start(route);
      await h.fire(route);
      assert.equal(sameRuntimePageOwner(h.state.currentRuntimePageOwner(), owner), true);
      assert.equal(h.timers.size, 0);
      if (status === 401) {
        assert.equal(h.renewals.length, 1);
        assert.equal(h.mediaOpen(), true);
        assert.deepEqual(h.failures, []);
      } else {
        assert.deepEqual(h.renewals, []);
        assert.equal(h.mediaOpen(), false);
        assert.equal(h.failures.length, 1);
        assert.equal(h.failures[0].failure.kind, "display_status");
      }
    });
  }

  for (const status of [403, 404, 503]) {
    test(`${route} retains terminal handling for completed HTTP ${status}`, async () => {
      const h = harness(), owner = h.state.currentRuntimePageOwner();
      h.setFetch(async () => response({ error: "fixture terminal observation" }, status));
      h.start(route);
      await h.fire(route);
      assert.equal(h.failures.length, 1);
      assert.equal(sameRuntimePageOwner(h.failures[0].owner, owner), true);
      assert.equal(h.failures[0].failure.kind, "display_status");
      assert.equal(h.mediaOpen(), false);
      assert.equal(h.timers.size, 0);
      assert.deepEqual(h.renewals, []);
    });
  }

  for (const failure of ["transport", "authority", "terminal"]) {
    test(`${route} ignores a ${failure} error from the same page's stale generation`, async () => {
      const h = harness(), pending = deferred();
      h.setFetch(() => pending.promise);
      h.start(route);
      const observation = h.fire(route);
      assert.equal(h.requests.length, 1);
      h.state.currentPageGeneration += 1;
      const newOwner = h.state.currentRuntimePageOwner();
      h.start(route);
      const newTimers = [...h.timers.keys()];
      if (failure === "transport") pending.reject(new TypeError("old network failure"));
      else pending.resolve(response({ error: "old observation failure" }, failure === "authority" ? 401 : 404));
      await observation;
      assertRetained(h, newOwner);
      assert.deepEqual(h.mutations, []);
      assert.deepEqual(h.messages, []);
      assert.deepEqual([...h.timers.keys()], newTimers, "old completion cannot add or cancel the new owner's polling");
    });
  }
}

for (const route of ["status", "afterInput"]) {
  for (const invalid of ["schema", "page_id", "direct_network", "json"]) {
    test(`${route} still handles malformed ${invalid} as a terminal observation error`, async () => {
      const h = harness();
      h.setFetch(async () => {
        const body = h.healthyStatus();
        if (invalid === "json") return { ok: true, text: async () => "{invalid json" };
        body[invalid] = invalid === "direct_network" ? true : "invalid";
        return response(body);
      });
      h.start(route);
      await h.fire(route);
      assert.equal(h.failures.length, 1);
      assert.equal(h.failures[0].failure.kind, "malformed_response");
      assert.equal(h.mediaOpen(), false);
      assert.equal(h.timers.size, 0);
      assert.deepEqual(h.renewals, []);
      assert.deepEqual(h.mutations, []);
    });
  }

  test(`${route} ignores a stale successful status before any viewer mutation`, async () => {
    const h = harness(), pending = deferred();
    h.setFetch(() => pending.promise);
    const oldStatus = { ...h.healthyStatus(), actual_url: "https://old.invalid/", title: "Old" };
    h.start(route);
    const observation = h.fire(route);
    h.state.currentPageGeneration += 1;
    h.state.currentPage.actual_url = "https://current.invalid/";
    h.start(route);
    const newOwner = h.state.currentRuntimePageOwner(), newTimers = [...h.timers.keys()];
    pending.resolve(response(oldStatus));
    await observation;
    assertRetained(h, newOwner);
    assert.equal(h.state.currentPage.actual_url, "https://current.invalid/");
    assert.equal(h.state.lastPageStatus, null);
    assert.deepEqual(h.mutations, []);
    assert.deepEqual([...h.timers.keys()], newTimers);
  });
}

test("an untagged programming TypeError does not become a transient observation", async () => {
  const h = harness();
  const retry = await h.state.handlePageObservationFailure(new TypeError("fixture programming error"), h.state.currentRuntimePageOwner());
  assert.equal(retry, false);
  assert.equal(h.failures.length, 1);
  assert.equal(h.mediaOpen(), false);
});

for (const route of Object.keys(routes)) {
  for (const outcome of ['success', 'authority', 'terminal']) {
    test(`${route} completion after viewer unload cannot change the page or resume observation`, async () => {
      const h = harness(), pending = deferred(), owner = h.state.currentRuntimePageOwner();
      h.setFetch(() => pending.promise);
      h.start(route);
      const observation = h.fire(route);
      assert.equal(h.requests.length, 1);
      h.state.unloadCleanupStarted = true;
      h.state.stopPageStatusPolling();
      h.state.stopPageHeartbeat();
      if (outcome === 'success') pending.resolve(response(h.healthyStatus()));
      else pending.resolve(response({ error: 'old document response' }, outcome === 'authority' ? 401 : 404));
      await observation;
      assertRetained(h, owner);
      assert.deepEqual(h.mutations, []);
      assert.deepEqual(h.messages, []);
      assert.equal(h.timers.size, 0);
    });
  }
}
