import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import test from "node:test";
import { diagnoseBrowserJourneyRecovery } from "./browser-journey-recovery.mjs";

const hash = value => `sha256:${createHash("sha256").update(value).digest("hex").slice(0, 16)}`;
const secret = "private-token-turn://credential@private.invalid";
const fixtureUrl = "http://localhost:61511/nav?run=test-run";
const original = {
  page_id: "page-current", engine_id: "selected-engine", exit_id: "selected-exit",
  browser_instance: "instance-current", actual_url: fixtureUrl,
  page_status: { schema: "elastos.browser.page-status/v1", page_id: "page-current", actual_url: fixtureUrl },
  sessions: {
    schema: "elastos.browser.session-capacity/v1", launching_sessions: 0,
    active_sessions: 1, total_sessions: 1, principal_sessions: 1,
    engine_cleanup_obligations: 0, launch_reconciliation_obligations: 0,
    recoverable_page: { state: "active", page_id: "page-current",
      cleanup: { schema: "elastos.browser.cleanup-handle/v1", id: "private-cleanup-handle" },
      // Runtime retains the launch snapshot when this page later navigates.
      engine_page: { page_id: "page-current", adapter: "selected-engine", engine: "chromium",
        actual_url: "http://localhost:61511/main?run=test-run", token: secret } },
    lifecycle: { sessions: [{ page_id: hash("page-current"), profile_key_hash: "sha256:profile",
      exit_id: "remote-carrier:sha256:exit", phase: "ACTIVE_SESSION" }] },
  },
};

// Advance only to the next pending timer, after actual promise continuations run.
// The helper uses the same deadlines for polling and pending callback timeouts.
function virtualClock() {
  let time = 0, next = 0;
  const timers = new Map();
  const clock = {
    now: () => time,
    setTimeout: (fn, delay) => { const id = ++next; timers.set(id, { at: time + delay, fn }); return id; },
    clearTimeout: id => timers.delete(id),
  };
  return { clock, wait: delay => new Promise(resolve => clock.setTimeout(resolve, delay)),
    async run(promise) {
      let settled = false, value, error;
      promise.then(result => { value = result; settled = true; }, reason => { error = reason; settled = true; });
      for (let turns = 0; !settled; turns++) {
        assert.ok(turns < 200, "diagnostic must settle within bounded timer work");
        await new Promise(setImmediate);
        if (settled) break;
        const entry = [...timers].sort((a, b) => a[1].at - b[1].at)[0];
        assert.ok(entry, "pending work must have a deadline");
        timers.delete(entry[0]);
        time = entry[1].at;
        entry[1].fn();
      }
      assert.equal(timers.size, 0, "helper must clear its deadline timers");
      if (error) throw error;
      return value;
    } };
}

function fixture(options = {}) {
  const timer = virtualClock();
  const calls = [];
  let record, offline = false, restoredAt = null, frames = 10, bytes = 100, sent = false;
  const request = (kind, phase, overrides = {}) => record({ kind, phase,
    request_id: `request-${kind}`, source_matches: true, token: secret, url: secret, ...overrides });
  const args = {
    clock: timer.clock,
    expectedUrl: fixtureUrl,
    cdp: {
      async send(method, params) {
        calls.push({ method, params, at: timer.clock.now() });
        if (method === "Network.emulateNetworkConditions") {
          offline = params.offline;
          if (offline && options.cutError) throw new Error(secret);
          if (!offline) {
            restoredAt = timer.clock.now();
            if (options.restoreError) throw new Error(secret);
          }
        }
      },
      async detach() { calls.push({ method: "detach" }); if (options.detachError) throw new Error(secret); },
    },
    async observeRequests(callback) {
      record = callback;
      request("renewal", "request");
      return () => { calls.push({ method: "stop" }); if (options.closeAtStop) request("closing", "request"); };
    },
    async readBinding() {
      const binding = structuredClone(original);
      if (options.bindingError) throw new Error(secret);
      if (restoredAt !== null) options.changeBinding?.(binding);
      return binding;
    },
    async readVideo() {
      const recovering = restoredAt !== null;
      if ((!offline && (!recovering || !options.stalledRecovery)) || options.ineffectiveCut) { frames++; bytes += 100; }
      if (offline && options.bytesStillFlow) bytes += 100;
      return { present: true, hidden: recovering && options.hiddenRecovery === true, paused: false,
        ready_state: 4, video_width: 800, video_height: 600, client_width: 800, client_height: 600,
        decoded_frames: frames, ...(options.noBytes ? {} : { video_bytes_received: bytes }), token: secret };
    },
    async readReceipt() {
      const events = [{ sequence: 1, type: "load", page: "nav", value: "" },
        { sequence: 2, type: "input", page: "nav", value: "current-text" }];
      if (sent && !options.staleReceipt) events.push({ sequence: 3, type: "input", page: "nav", value: "current-text-recovered" });
      if (sent && options.reload) events.push({ sequence: 4, type: "load", page: "nav", value: "" });
      return { schema: "elastos.browser.journey-receipt/v1", run: "test-run", events, token: secret };
    },
    async extendInput(suffix, budget) {
      calls.push({ method: "input", suffix, deadline: budget.deadlineMs, at: timer.clock.now() });
      if (options.inputDelay) await timer.wait(options.inputDelay);
      if (!budget.signal.aborted) sent = true;
    },
    async probeViewerRequest() {
      request("probe", "request");
      if (options.probeHttpError) request("probe", "response", { status: 503 });
      else if (!options.noRequestFailure) request("probe", "failed", { source_matches: !options.foreignFailure });
      if (options.forbidden) request(options.forbidden, "request");
      if (options.overflow) {
        for (let i = 0; i < 1000; i++) request("renewal", "request", { request_id: `${secret}-${i}` });
        request("closing", "request");
      }
    },
  };
  return { args, calls, run: () => timer.run(diagnoseBrowserJourneyRecovery(args)) };
}

async function fails(options, expected) {
  const f = fixture(options);
  let evidence;
  await assert.rejects(f.run(), error => {
    evidence = error.evidence;
    assert.equal(evidence.ok, false);
    if (expected) assert.equal(evidence.failure, expected);
    const serialized = JSON.stringify({ message: error.message, evidence });
    assert.ok(!serialized.includes(secret));
    assert.ok(!serialized.includes("private-cleanup-handle"));
    assert.ok(serialized.length < 20_000);
    return true;
  });
  assert.equal(evidence.restore.attempted, true);
  assert.equal(evidence.detach.attempted, true);
  assert.deepEqual(f.calls.filter(call => call.params?.offline === false).map(call => call.params), [{
    offline: false, latency: 0, downloadThroughput: -1, uploadThroughput: -1, packetLoss: 0,
  }]);
  assert.equal(f.calls.at(-1).method, "stop");
  return { evidence, calls: f.calls };
}

test("five-second cut proves request failure and media stall, then same-owner video and extended input recover", async () => {
  const f = fixture();
  const evidence = await f.run();
  assert.equal(evidence.ok, true);
  assert.equal(evidence.cut_ms, 5000);
  assert.ok(evidence.stall_ms >= 1000);
  assert.equal(evidence.viewer_request_failed, true);
  assert.ok(evidence.recovery_ms <= 5000);
  assert.deepEqual(evidence.input, { before_length: 12, after_length: 22, sequence: 3 });
  assert.deepEqual(evidence.restore, { attempted: true, ok: true });
  assert.equal(evidence.detach.ok, true);
  assert.equal(evidence.requests[0].kind, "renewal");
  assert.equal(evidence.requests[0].phase, "baseline");
  assert.deepEqual(f.calls.find(call => call.params?.offline === true).params, {
    offline: true, latency: 0, downloadThroughput: -1, uploadThroughput: -1, packetLoss: 100,
  });
  assert.ok(!JSON.stringify(evidence).includes(secret));
  assert.ok(!JSON.stringify(evidence).includes("current-text"));
});

test("a separate media interruption has five seconds after both cut acknowledgements", async () => {
  const f = fixture();
  let cutAt, restoreAt;
  f.args.mediaInterruption = {
    async cut() {
      assert.ok(f.calls.some(call => call.params?.offline === true));
      await new Promise(resolve => f.args.clock.setTimeout(resolve, 200));
      cutAt = f.args.clock.now();
    },
    async restore() { restoreAt = f.args.clock.now(); },
  };
  const evidence = await f.run();
  assert.equal(restoreAt - cutAt, 5000);
  assert.equal(evidence.media_cut_acknowledged, true);
  assert.equal(evidence.restore.http_ok, true);
  assert.equal(evidence.restore.media_ok, true);
  assert.equal(evidence.ok, true);
  assertRestoredAndDetached(f);
});

for (const failing of ["mediaCut", "mediaRestore", "httpRestore", "baseline", "mediaCutTimeout"]) {
  test(`${failing} failure still restores both transports and detaches`, async () => {
    const f = fixture({ restoreError: failing === "httpRestore", bindingError: failing === "baseline" });
    let mediaRestored = 0;
    f.args.mediaInterruption = {
      async cut() {
        if (failing === "mediaCut") throw new Error(secret);
        if (failing === "mediaCutTimeout") await new Promise(() => {});
      },
      async restore() {
        mediaRestored++;
        if (failing === "mediaRestore") throw new Error(secret);
      },
    };
    await assert.rejects(f.run(), error => {
      assert.equal(error.evidence.ok, false);
      assert.equal(error.evidence.restore.http_ok, failing !== "httpRestore");
      assert.equal(error.evidence.restore.media_ok, failing !== "mediaRestore");
      assert.ok(!JSON.stringify(error.evidence).includes(secret));
      return true;
    });
    assert.equal(mediaRestored, 1);
    assertRestoredAndDetached(f);
  });
}

test("decoded frames remain sufficient when byte metrics are absent", async () => {
  assert.equal((await fixture({ noBytes: true }).run()).ok, true);
});

test("fresh page status accepts a navigated page with a stale launch snapshot", async () => {
  assert.notEqual(original.sessions.recoverable_page.engine_page.actual_url, fixtureUrl);
  assert.equal(original.page_status.actual_url, fixtureUrl);
  assert.equal((await fixture().run()).ok, true);
});

for (const option of ["noRequestFailure", "probeHttpError", "foreignFailure"]) {
  test(`${option} cannot substitute for an actual viewer network request failure`, () =>
    fails({ [option]: true }, "viewer_request_failure_unproven"));
}
for (const option of ["ineffectiveCut", "bytesStillFlow"]) {
  test(`${option} cannot prove the media interruption`, () =>
    fails({ [option]: true }, "viewer_media_cut_unproven"));
}
for (const option of ["hiddenRecovery", "stalledRecovery", "staleReceipt"]) {
  test(`${option} cannot pass recovery`, () => fails({ [option]: true }));
}

for (const [name, changeBinding] of Object.entries({
  page: value => { value.page_id = value.sessions.recoverable_page.page_id = "replacement"; },
  cleanup: value => { value.sessions.recoverable_page.cleanup.id = "replacement"; },
  profile: value => { value.sessions.lifecycle.sessions[0].profile_key_hash = "sha256:replacement"; },
  engineSelection: value => { value.engine_id = "replacement"; },
  engineAdapter: value => { value.sessions.recoverable_page.engine_page.adapter = "replacement"; },
  exitSelection: value => { value.exit_id = "replacement"; },
  runtimeExit: value => { value.sessions.lifecycle.sessions[0].exit_id = "replacement"; },
  browserInstance: value => { value.browser_instance = "replacement"; },
  actualUrl: value => { value.actual_url = value.page_status.actual_url = "http://localhost:61511/nav?run=other-run"; },
  runtimeUrl: value => { value.page_status.actual_url = "http://localhost:61511/nav?run=other-run"; },
  statusSchema: value => { value.page_status.schema = "unrelated-status/v1"; },
  statusPage: value => { value.page_status.page_id = "another-page"; },
  missingStatus: value => { delete value.page_status; },
  activeCount: value => { value.sessions.active_sessions = value.sessions.total_sessions = 2; },
  totalCount: value => { value.sessions.total_sessions = 2; },
  principalCount: value => { value.sessions.principal_sessions = 0; },
  cleanupPending: value => { value.sessions.recoverable_page.state = "cleanup_pending"; },
})) {
  test(`changed ${name} cannot pass as same-session recovery`, () => fails({ changeBinding }));
}

for (const kind of ["opening", "closing"]) {
  test(`${kind} request fails even when the final binding looks unchanged`, () =>
    fails({ forbidden: kind }, "cleanup_or_replacement_requested"));
}
test("late close during observer teardown also fails", () =>
  fails({ closeAtStop: true }, "cleanup_or_replacement_requested"));
test("bounded evidence still rejects a close after event overflow", async () => {
  const { evidence } = await fails({ overflow: true }, "cleanup_or_replacement_requested");
  assert.equal(evidence.requests.length, 64);
  assert.ok(evidence.dropped_requests > 0);
});
test("a new page load cannot be hidden by an input receipt", () => fails({ reload: true }, "page_reloaded"));
test("input receives only the remaining shared recovery budget", async () => {
  const f = fixture();
  const readVideo = f.args.readVideo;
  let afterRestore = 0;
  f.args.readVideo = async budget => {
    if (f.calls.some(call => call.params?.offline === false) && afterRestore++ === 1) {
      await new Promise(resolve => f.args.clock.setTimeout(resolve, 4500));
    }
    return readVideo(budget);
  };
  f.args.extendInput = async (_suffix, budget) => {
    assert.equal(budget.timeoutMs, 500);
    await new Promise(() => {});
  };
  await assert.rejects(f.run(), error => error.evidence.failure === "input_deadline");
});
for (const option of ["bindingError", "cutError", "restoreError", "detachError"]) {
  test(`${option} preserves unconditional restore and detach attempts with redacted errors`, () => fails({ [option]: true }));
}
test("restore failure keeps the original failure and reports both outcomes", async () => {
  const { evidence } = await fails({ noRequestFailure: true, restoreError: true }, "viewer_request_failure_unproven");
  assert.equal(evidence.restore.ok, false);
  assert.equal(evidence.restore.error, "network_restore_failed");
  assert.equal(evidence.detach.ok, true);
});
test("a hung viewer probe reaches the cut deadline, restores, and detaches", async () => {
  const f = fixture();
  f.args.probeViewerRequest = () => new Promise(() => {});
  await assert.rejects(f.run(), error => error.evidence.failure === "viewer_probe_deadline" &&
    error.evidence.restore.ok && error.evidence.detach.ok);
  const cut = f.calls.find(call => call.params?.offline === true);
  const restore = f.calls.find(call => call.params?.offline === false);
  assert.equal(restore.at - cut.at, 5000);
});
test("byte metrics cannot disappear after they supplied baseline evidence", async () => {
  const f = fixture();
  const readVideo = f.args.readVideo;
  f.args.readVideo = async budget => {
    const value = await readVideo(budget);
    if (f.calls.some(call => call.params?.offline === true)) delete value.video_bytes_received;
    return value;
  };
  await assert.rejects(f.run(), error => error.evidence.failure === "video_metrics_unavailable" &&
    error.evidence.restore.ok && error.evidence.detach.ok);
});
test("baseline requires the exact controlled fixture URL", async () => {
  const f = fixture();
  f.args.expectedUrl = fixtureUrl + "&unexpected=1";
  await assert.rejects(f.run(), error => error.evidence.failure === "fixture_url_mismatch" && error.evidence.restore.ok);
});
test("a receipt from a different fixture run cannot establish the baseline", async () => {
  const f = fixture();
  const readReceipt = f.args.readReceipt;
  f.args.readReceipt = async () => ({ ...await readReceipt(), run: "other-run" });
  await assert.rejects(f.run(), error => error.evidence.failure === "fixture_run_mismatch" && error.evidence.restore.ok);
});

function assertRestoredAndDetached(f, observerInstalled = true) {
  const restore = f.calls.findIndex(call => call.params?.offline === false);
  const detach = f.calls.findIndex(call => call.method === "detach");
  assert.ok(restore >= 0 && detach > restore, "restore must precede detach on every outcome");
  assert.deepEqual(f.calls[restore].params, {
    offline: false, latency: 0, downloadThroughput: -1, uploadThroughput: -1, packetLoss: 0,
  });
  assert.equal(f.calls.filter(call => call.params?.offline === false).length, 1);
  if (observerInstalled) assert.equal(f.calls.at(-1).method, "stop");
}

for (const [name, initialUrl] of [
  ["missing expectedUrl", fixtureUrl],
  ["unknown initial URL", "about:blank"],
  ["foreign initial origin with matching path and run", "https://foreign.invalid/nav?run=test-run"],
]) {
  test(`${name} cannot establish an authorized fixture baseline`, async () => {
    const f = fixture();
    if (name === "missing expectedUrl") delete f.args.expectedUrl;
    const readBinding = f.args.readBinding;
    f.args.readBinding = async budget => {
      const value = await readBinding(budget);
      // Agreement between viewer and Runtime is insufficient: expectedUrl must
      // come from the controlled fixture, not from this observed current page.
      value.actual_url = value.page_status.actual_url = initialUrl;
      return value;
    };
    try {
      await assert.rejects(f.run(), error => error.evidence?.failure === "fixture_url_mismatch");
    } finally {
      assertRestoredAndDetached(f);
      assert.ok(!f.calls.some(call => call.params?.offline === true), "reject before applying the cut");
      assert.ok(!f.calls.some(call => call.method === "input"));
    }
  });
}

for (const resumesAfterInput of [false, true]) {
  test(resumesAfterInput
    ? "post-input video may resume later within the shared recovery deadline"
    : "pre-input frame progress and a new input receipt cannot pass when video then freezes", async () => {
    const f = fixture();
    const readVideo = f.args.readVideo, extendInput = f.args.extendInput, readReceipt = f.args.readReceipt;
    let latest, preInputVideo, inputAt, receiptSeen = false, postInputProgress = false;
    f.args.readVideo = async budget => {
      if (preInputVideo && (!resumesAfterInput || f.args.clock.now() - inputAt < 1000)) return { ...preInputVideo };
      latest = await readVideo(budget);
      if (preInputVideo && latest.decoded_frames > preInputVideo.decoded_frames) postInputProgress = true;
      return latest;
    };
    f.args.extendInput = async (suffix, budget) => {
      preInputVideo = { ...latest };
      inputAt = f.args.clock.now();
      await extendInput(suffix, budget);
    };
    f.args.readReceipt = async budget => {
      const receipt = await readReceipt(budget);
      if (receipt.events.some(event => event.sequence === 3 && event.value === "current-text-recovered")) receiptSeen = true;
      return receipt;
    };
    try {
      if (resumesAfterInput) {
        const evidence = await f.run();
        assert.equal(evidence.ok, true);
        assert.equal(postInputProgress, true, "success must wait for a frame beyond the pre-input snapshot");
        assert.ok(evidence.recovery_ms >= 1000 && evidence.recovery_ms <= 5000);
      } else {
        await assert.rejects(f.run(), error => {
          assert.equal(error.evidence?.ok, false);
          assert.equal(error.evidence?.viewer_request_failed, true);
          assert.ok(error.evidence?.stall_ms >= 1000);
          assert.notEqual(error.evidence?.failure, "observation_failed", "fail the media proof, not fixture execution");
          return true;
        });
      }
    } finally {
      assertRestoredAndDetached(f);
      assert.equal(receiptSeen, true, "the input receipt must actually arrive");
      assert.equal(f.calls.filter(call => call.method === "input").length, 1);
      const restoredAt = f.calls.find(call => call.params?.offline === false).at;
      assert.ok(f.args.clock.now() - restoredAt <= 5000, "post-input media gets no new deadline");
    }
  });
}

for (const callback of ["readBinding", "readVideo"]) {
  for (const missingProof of [null, "noRequestFailure", "ineffectiveCut"]) {
    test(`${callback} pending at the cut deadline ${missingProof ? `still rejects ${missingProof}` : "restores and recovers with earlier cut proof"}`, async () => {
      const f = fixture(missingProof ? { [missingProof]: true } : {});
      const read = f.args[callback];
      let delayed = false, aborted = false;
      f.args[callback] = async budget => {
        const cut = f.calls.find(call => call.params?.offline === true);
        const restored = f.calls.some(call => call.params?.offline === false);
        if (cut && !restored && !delayed && f.args.clock.now() - cut.at >= 4500) {
          delayed = true;
          // This observation finishes at the cut boundary. Its budget expires
          // there too; cancellation clears the callback's own pending timer.
          await new Promise(resolve => {
            const timer = f.args.clock.setTimeout(resolve, budget.timeoutMs);
            budget.signal.addEventListener("abort", () => {
              aborted = true;
              f.args.clock.clearTimeout(timer);
              resolve();
            }, { once: true });
          });
        }
        return read(budget);
      };
      try {
        if (missingProof) {
          await assert.rejects(f.run(), error => error.evidence?.ok === false);
        } else {
          const evidence = await f.run();
          assert.equal(evidence.ok, true);
          assert.equal(evidence.viewer_request_failed, true);
          assert.ok(evidence.stall_ms >= 1000);
          assert.ok(evidence.recovery_ms <= 5000);
        }
      } finally {
        assertRestoredAndDetached(f);
        assert.equal(delayed, true);
        assert.equal(aborted, true, "the expired observation must be cancelled");
        const cut = f.calls.find(call => call.params?.offline === true);
        const restore = f.calls.find(call => call.params?.offline === false);
        assert.equal(restore.at - cut.at, 5000);
      }
    });
  }
}

for (const callback of ["observeRequests", "readBinding", "readVideo", "readReceipt", "probeViewerRequest", "extendInput"]) {
  test(`failure in ${callback} always restores the network and detaches`, async () => {
    const f = fixture();
    f.args[callback] = async () => { throw new Error(secret); };
    try {
      await assert.rejects(f.run(), error => {
        assert.equal(error.evidence?.ok, false);
        assert.equal(error.evidence?.restore.ok, true);
        assert.equal(error.evidence?.detach.ok, true);
        assert.ok(!JSON.stringify({ message: error.message, evidence: error.evidence }).includes(secret));
        return true;
      });
    } finally {
      assertRestoredAndDetached(f, callback !== "observeRequests");
    }
  });
}
