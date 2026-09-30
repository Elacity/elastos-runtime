import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";
import { censusBrowserViewerReload, classifyReloadCensus, diagnoseBrowserViewerReload } from "./browser-journey-viewer-reload.mjs";

const hash = value => `sha256:${createHash("sha256").update(value).digest("hex").slice(0, 16)}`;
const secret = "private-token-turn://credential@private.invalid";
const fixtureUrl = "http://localhost:61511/nav?run=test-run";
const original = {
  viewer: { page_id: "page-current", engine_id: "selected-engine", exit_id: "selected-exit",
    browser_instance: "instance-current", actual_url: fixtureUrl, document_id: 1788850000000.125 },
  page_status: { schema: "elastos.browser.page-status/v1", page_id: "page-current", actual_url: fixtureUrl },
  sessions: {
    schema: "elastos.browser.session-capacity/v1", launching_sessions: 0,
    active_sessions: 1, total_sessions: 1, principal_sessions: 1,
    engine_cleanup_obligations: 0, launch_reconciliation_obligations: 0,
    recoverable_page: { state: "active", page_id: "page-current",
      cleanup: { schema: "elastos.browser.cleanup-handle/v1", id: "private-cleanup-handle" },
      engine_page: { page_id: "page-current", adapter: "selected-engine", engine: "chromium",
        actual_url: "http://localhost:61511/main?run=test-run", token: secret } },
    lifecycle: { sessions: [{ page_id: hash("page-current"), profile_key_hash: "sha256:profile",
      exit_id: "remote-carrier:sha256:exit", phase: "ACTIVE_SESSION" }] },
  },
};

function virtualClock() {
  let time = 0, next = 0;
  const timers = new Map();
  const clock = { now: () => time,
    setTimeout: (fn, delay) => { const id = ++next; timers.set(id, { at: time + delay, fn }); return id; },
    clearTimeout: id => timers.delete(id) };
  const wait = (delay, signal) => new Promise(resolve => {
    const abort = () => { clock.clearTimeout(id); resolve(); };
    const id = clock.setTimeout(() => { signal?.removeEventListener("abort", abort); resolve(); }, delay);
    signal?.addEventListener("abort", abort, { once: true });
  });
  return { clock, wait, async run(promise) {
    let settled = false, value, error;
    promise.then(result => { value = result; settled = true; }, reason => { error = reason; settled = true; });
    for (let turns = 0; !settled; turns++) {
      assert.ok(turns < 200, "bounded diagnostic must settle");
      await new Promise(setImmediate);
      if (settled) break;
      const entry = [...timers].sort((a, b) => a[1].at - b[1].at)[0];
      assert.ok(entry, "pending callback must have a deadline");
      timers.delete(entry[0]);
      time = entry[1].at;
      entry[1].fn();
    }
    assert.equal(timers.size, 0, "all helper and callback timers must be cleared");
    if (error) throw error;
    return value;
  } };
}

function fixture(options = {}) {
  const timer = virtualClock(), calls = [];
  let record, reloaded = false, sent = false, frames = 100, bytes = 1000, reads = 0;
  const event = (kind, phase = "request", extra = {}) => record({ kind, phase, source_matches: true,
    request_id: secret, document_generation: 1, url: secret, token: secret, ...extra });
  const args = {
    expectedUrl: fixtureUrl, clock: timer.clock,
    async observeRequests(callback, budget) {
      record = callback;
      calls.push({ method: "observe", budget });
      event("renewal");
      if (options.forbiddenOnSetup) event("closing");
      return async budget => {
        calls.push({ method: "stop", budget });
        if (options.forbiddenOnStop) event("opening");
        if (options.stopError) throw new Error(secret);
        if (options.stopHangs) await new Promise(() => {});
      };
    },
    async readState(budget) {
      calls.push({ method: "state", budget, at: timer.clock.now(), reloaded, sent });
      if (options.stateError) throw new Error(secret);
      const raw = structuredClone(original);
      if (reloaded) {
        reads++;
        if (options.stateDelay) await timer.wait(options.stateDelay, budget.signal);
        options.changeRuntime?.(raw, reads);
        if (!options.sameDocument) raw.viewer.document_id += 1000;
        if (options.secondDocument && reads >= 3) raw.viewer.document_id += 1000;
        if (options.revertDocument && reads >= 3) raw.viewer.document_id = original.viewer.document_id;
        options.changeViewer?.(raw.viewer, reads);
        if (reads <= (options.pendingReads || 0)) raw.viewer = null;
        if (reads <= (options.pendingPageReads || 0)) raw.viewer.page_id = "";
        if (!options.freezeNewVideo && !(sent && options.freezeAfterInput)) { frames++; bytes += 100; }
        if (options.bytesFreeze) bytes = 0;
      } else {
        if (options.baselineHangs) await new Promise(() => {});
        if (options.baselineStateDelay) await timer.wait(options.baselineStateDelay, budget.signal);
        frames++; bytes += 100;
      }
      raw.video = raw.viewer ? { present: true, hidden: reloaded && options.hidden === true, paused: false,
        ready_state: 4, video_width: 800, video_height: 600, client_width: 800, client_height: 600,
        decoded_frames: frames, ...(options.noBytes ? {} : { video_bytes_received: bytes }), token: secret } : null;
      if (reloaded && options.noMedia) raw.video = null;
      if (reloaded && options.removeBytes && raw.video) delete raw.video.video_bytes_received;
      options.changeState?.(raw, { reloaded, sent, reads });
      return raw;
    },
    async readReceipt(budget) {
      calls.push({ method: "receipt", budget });
      const events = [{ sequence: 1, type: "load", page: "nav", value: "" },
        { sequence: 2, type: "input", page: "nav", value: "current-text" }];
      if (sent && !options.staleReceipt) events.push({ sequence: 3, type: "input", page: "nav",
        value: options.wrongInput ? "current-text-relaod" : "current-text-reload" });
      if (reloaded && options.guestLoad) events.push({ sequence: 4, type: "load", page: "nav", value: "" });
      const receipt = { schema: "elastos.browser.journey-receipt/v1", run: "test-run", events, token: secret };
      options.changeReceipt?.(receipt, { reloaded, sent });
      return receipt;
    },
    async reloadViewer(budget) {
      calls.push({ method: "reload", budget, at: timer.clock.now() });
      if (options.reloadError) throw new Error(secret);
      if (options.reloadDelay) await timer.wait(options.reloadDelay, budget.signal);
      reloaded = true;
      frames = 0; bytes = 0;
      event("navigation", "commit");
      if (options.overflow) for (let i = 0; i < 1000; i++) event("status");
      if (options.forbidden) event(options.forbidden, "request", { source_matches: !options.foreign });
      if (options.reloadHangs) await new Promise(() => {});
    },
    async extendInput(suffix, budget) {
      calls.push({ method: "input", suffix, budget, at: timer.clock.now(), frames });
      if (options.inputError) throw new Error(secret);
      if (options.inputDelay) await timer.wait(options.inputDelay, budget.signal);
      if (!budget.signal.aborted) sent = true;
    },
  };
  return { args, calls, event, run: () => timer.run(diagnoseBrowserViewerReload(args)) };
}

async function fails(options, expected, edit) {
  const f = fixture(options);
  edit?.(f);
  let evidence;
  await assert.rejects(f.run(), error => {
    evidence = error.evidence;
    assert.equal(evidence?.ok, false);
    if (expected) assert.equal(evidence.failure, expected);
    const serialized = JSON.stringify({ message: error.message, evidence });
    for (const privateValue of [secret, fixtureUrl, "current-text", "private-cleanup-handle",
      String(original.viewer.document_id)]) assert.ok(!serialized.includes(privateValue));
    assert.ok(serialized.length < 20000);
    return true;
  });
  assert.equal(f.calls.filter(call => call.method === "stop").length, 1);
  assert.ok(f.calls.filter(call => call.method === "reload").length <= 1);
  assert.ok(f.calls.filter(call => call.method === "input").length <= 1);
  return { ...f, evidence };
}

test("late observe samples report unmeasured bindings instead of claiming a match", async () => {
  const f = fixture();
  f.args.observeMs = 5500;
  const proof = await f.run();
  assert.equal(proof.ok, true);
  const observed = proof.samples.filter(sample => sample.phase === "observe");
  assert.ok(observed.length > 0);
  assert.ok(observed.every(sample => sample.binding_matches === "unmeasured"));
  assert.ok(proof.samples.filter(sample => sample.phase !== "observe")
    .every(sample => sample.binding_matches === true));
});

test("a new viewer retains fresh Runtime binding, resets counters, decodes and appends exact input within five seconds", async () => {
  const f = fixture();
  const proof = await f.run();
  assert.equal(proof.ok, true);
  assert.equal(proof.observer.stopped, true);
  assert.notEqual(proof.initial_document_hash, proof.new_document_hash);
  assert.ok(proof.reload_ms <= 5000);
  assert.deepEqual(proof.input, { before_length: 12, after_length: 19, sequence: 3 });
  assert.equal(proof.binding_hashes.length, 14);
  assert.ok(proof.requests.some(row => row.kind === "navigation" && row.event === "commit"));
  const input = f.calls.find(call => call.method === "input");
  const reload = f.calls.find(call => call.method === "reload");
  assert.equal(input.suffix, "-reload");
  assert.equal(input.budget.deadlineMs, reload.at + 5000);
  assert.ok(input.frames >= 2, "new document decoded before input");
  assert.ok(proof.samples.at(-1).decoded_frames > input.frames, "decoded after the pre-input snapshot");
  assert.ok(!JSON.stringify(proof).includes(secret));
  assert.ok(!JSON.stringify(proof).includes("current-text"));
});

for (const pending of ["pendingReads", "pendingPageReads"]) {
  test(`${pending}: pending viewer is allowed while fresh Runtime binding stays exact`, async () => {
    const f = fixture({ [pending]: 3 });
    assert.equal((await f.run()).ok, true);
    assert.equal(f.calls.filter(call => call.method === "reload").length, 1);
  });
}
test("decoded video works without optional byte counters", async () => {
  assert.equal((await fixture({ noBytes: true }).run()).ok, true);
});
for (const [name, options, code] of [
  ["old document despite navigation event", { sameDocument: true }, "reload_deadline"],
  ["frozen new document", { freezeNewVideo: true }, "reload_deadline"],
  ["video advances before input then freezes", { freezeAfterInput: true }, "reload_deadline"],
  ["invisible decoded frames", { hidden: true }, "reload_deadline"],
  ["viewer never gets media", { noMedia: true }, "reload_deadline"],
  ["viewer never becomes ready", { pendingReads: 100 }, "reload_deadline"],
  ["frozen exposed bytes", { bytesFreeze: true }, "reload_deadline"],
  ["missing exposed bytes stay pending until the deadline", { removeBytes: true }, "reload_deadline"],
  ["second new viewer document", { secondDocument: true }, "viewer_document_changed_again"],
  ["old document returns", { revertDocument: true }, "viewer_document_reverted"],
  ["stale receipt", { staleReceipt: true }, "reload_deadline"],
  ["reordered exact suffix", { wrongInput: true }, "input_receipt_mismatch"],
  ["guest page reload", { guestLoad: true }, "guest_page_reloaded"],
  ["reload callback rejects", { reloadError: true }, "reload_failed"],
  ["reload callback hangs", { reloadHangs: true }, "reload_deadline"],
  ["input rejects", { inputError: true }, "input_failed"],
  ["readState rejects privately", { stateError: true }, "state_failed"],
]) test(name, async () => { await fails(options, code); });

test("same-page new viewer waits for its first video readiness and byte stats before proving progress", async () => {
  const f = fixture({ changeState(raw, { reloaded, reads }) {
    if (reloaded && reads <= 2) {
      raw.video.decoded_frames = 0;
      raw.video.ready_state = 0;
      delete raw.video.video_bytes_received;
    }
  } });
  const proof = await f.run();
  assert.equal(proof.ok, true);
  assert.equal(proof.samples.filter(sample => sample.phase === "reload" && !sample.viewer_ready).length, 2);
  const input = f.calls.find(call => call.method === "input");
  assert.ok(input.frames >= 4, "two complete new-peer samples must advance before input");
  assert.ok(proof.samples.at(-1).decoded_frames > input.frames);
});
for (const key of ["decoded_frames", "video_bytes_received"]) {
  for (const invalid of [-1, NaN, "0", null]) {
    test(`reported ${key}=${String(invalid)} is malformed rather than pending`, async () => {
      await fails({ changeState(raw, { reloaded }) {
        if (reloaded) {
          raw.video[key] = invalid;
          if (key === "decoded_frames") delete raw.video.video_bytes_received;
        }
      } }, "video_metrics_unavailable");
    });
  }
}

for (const [name, mutate] of [
  ["cleanup handle", raw => { raw.sessions.recoverable_page.cleanup.id = "replacement"; }],
  ["profile", raw => { raw.sessions.lifecycle.sessions[0].profile_key_hash = "replacement"; }],
  ["Runtime Exit", raw => { raw.sessions.lifecycle.sessions[0].exit_id = "replacement"; }],
  ["Engine adapter", raw => { raw.sessions.recoverable_page.engine_page.adapter = "replacement"; }],
  ["Engine", raw => { raw.sessions.recoverable_page.engine_page.engine = "replacement"; }],
  ["session counts", raw => { raw.sessions.active_sessions = raw.sessions.total_sessions = raw.sessions.principal_sessions = 2; }],
  ["fresh status URL", raw => { raw.page_status.actual_url = "http://localhost:61511/main?run=test-run"; }],
]) test(`pending viewer cannot hide changed ${name}`, async () => {
  await fails({ pendingReads: 3, changeRuntime: mutate }, "binding_changed");
});

for (const [name, mutate] of [
  ["Runtime page", raw => { raw.sessions.recoverable_page.page_id = "replacement"; }],
  ["status page", raw => { raw.page_status.page_id = "replacement"; }],
  ["status schema", raw => { raw.page_status.schema = "foreign"; }],
  ["cleanup started", raw => { raw.sessions.recoverable_page.state = "cleanup_pending"; }],
  ["cleanup obligation", raw => { raw.sessions.engine_cleanup_obligations = 1; }],
  ["launch started", raw => { raw.sessions.launching_sessions = 1; }],
]) test(`${name} fails even while viewer is pending`, async () => {
  await fails({ pendingReads: 3, changeRuntime: mutate }, "binding_unavailable_or_cleanup");
});

for (const key of ["browser_instance", "engine_id", "exit_id", "actual_url", "page_id"]) {
  test(`actual reloaded viewer must retain ${key}`, async () => {
    await fails({ changeViewer: viewer => { viewer[key] = "foreign"; } },
      ["actual_url", "page_id"].includes(key) ? "binding_unavailable_or_cleanup" : "binding_changed");
  });
}

test("expectedUrl is required and must match the exact initial fixture URL", async () => {
  for (const expected of [undefined, "", "http://localhost:61511/nav?run=foreign"]) {
    const f = await fails({}, "fixture_url_mismatch", f => { f.args.expectedUrl = expected; });
    assert.equal(f.calls.some(call => call.method === "reload"), false);
  }
});
test("the initial receipt run must bind the supplied controlled URL", async () => {
  await fails({ changeReceipt: value => { value.run = "foreign"; } }, "fixture_run_mismatch");
});
test("document identity must be the numeric timeOrigin", async () => {
  await fails({ changeViewer: viewer => { viewer.document_id = "new-document"; } }, "viewer_document_invalid");
});
test("Runtime status uses the fresh URL despite the stale launch snapshot", async () => {
  assert.notEqual(original.sessions.recoverable_page.engine_page.actual_url, original.page_status.actual_url);
  assert.equal((await fixture().run()).ok, true);
});

test("reload callback, readiness, and input share one deadline from reload start", async () => {
  const f = await fails({ reloadDelay: 4400, inputDelay: 500 }, "input_deadline");
  const reload = f.calls.find(call => call.method === "reload");
  const input = f.calls.find(call => call.method === "input");
  assert.equal(input.budget.deadlineMs, reload.at + 5000);
  assert.ok(input.budget.signal.aborted);
  assert.equal(f.calls.find(call => call.method === "stop").budget.deadlineMs, reload.at + 10000);
});
test("readiness read hanging at the deadline aborts and removes the observer", async () => {
  const f = await fails({ stateDelay: 6000 }, "state_deadline");
  assert.ok(f.calls.find(call => call.method === "state" && call.reloaded).budget.signal.aborted);
  assert.equal(f.evidence.failure_phase, "reload");
  assert.equal(typeof f.evidence.reload_started_ms, "number");
});
test("slow baseline two serial samples can complete then reload stays on the five-second budget", async () => {
  const f = fixture({ baselineStateDelay: 2534 });
  const proof = await f.run();
  assert.equal(proof.ok, true);
  assert.ok(proof.reload_started_ms >= 5000);
  assert.ok(proof.reload_ms <= 5000);
  const reload = f.calls.find(call => call.method === "reload");
  const input = f.calls.find(call => call.method === "input");
  assert.equal(input.budget.deadlineMs, reload.at + 5000);
  const baselineReads = f.calls.filter(call => call.method === "state" && !call.reloaded);
  assert.ok(baselineReads.length >= 2);
  assert.ok(baselineReads[0].budget.deadlineMs >= 10000);
});
test("hung baseline is bounded and does not start reload", async () => {
  const f = await fails({ baselineHangs: true }, "state_deadline");
  assert.equal(f.evidence.failure_phase, "baseline");
  assert.equal(f.evidence.reload_started_ms, undefined);
  assert.equal(f.calls.some(call => call.method === "reload"), false);
  const state = f.calls.find(call => call.method === "state");
  assert.ok(state.budget.signal.aborted);
  assert.ok(state.budget.deadlineMs <= 10000);
});
for (const kind of ["opening", "closing"]) {
  test(`${kind} immediately interrupts a hanging reload and aborts its budget`, async () => {
    const f = await fails({ forbidden: kind, reloadHangs: true }, "page_open_or_close_observed");
    const reload = f.calls.find(call => call.method === "reload");
    assert.ok(reload.budget.signal.aborted);
    assert.equal(f.calls.find(call => call.method === "stop").budget.deadlineMs, reload.at + 5000);
    assert.equal(f.calls.some(call => call.method === "input"), false);
  });
}
test("bounded evidence overflow cannot conceal closing", async () => {
  const { evidence } = await fails({ overflow: true, forbidden: "closing" }, "page_open_or_close_observed");
  assert.equal(evidence.requests.length, 64);
  assert.ok(evidence.dropped_requests > 0);
});
test("foreign-frame opening does not count as exact viewer activity", async () => {
  assert.equal((await fixture({ forbidden: "opening", foreign: true }).run()).ok, true);
});
for (const options of [{ forbiddenOnSetup: true }, { forbiddenOnStop: true }]) {
  test(`forbidden activity during observer ${options.forbiddenOnSetup ? "setup" : "flush"} fails and removes it`, async () => {
    await fails(options, "page_open_or_close_observed");
  });
}
for (const options of [{ stopError: true }, { stopHangs: true }]) {
  test(`observer removal ${options.stopError ? "failure" : "deadline"} fails proof`, async () => {
    const { evidence } = await fails(options, "observer_stop_failed");
    assert.equal(evidence.observer.stopped, false);
  });
}

test("pending media evidence distinguishes retained viewer, absent media and missing byte reports", async () => {
  for (const noMedia of [true, false]) {
    const { evidence } = await fails(noMedia ? { noMedia: true } : { removeBytes: true }, "reload_deadline");
    const sample = evidence.samples.find(row => row.phase === "reload");
    assert.equal(sample.viewer_has_page, true);
    assert.equal(sample.viewer_ready, false);
    assert.equal(sample.video_present, !noMedia);
    assert.equal(sample.video_bytes_reported, false);
    if (noMedia) assert.equal(sample.video_ready_state, undefined);
    else { assert.equal(sample.video_ready_state, 4); assert.ok(sample.video_decoded_frames > 0); }
  }
});

test("a published page with a stale address fails with the exact viewer predicate", async () => {
  // Keep the strict classification for the run 68/81 state after repairing
  // the product's publication order. A stale visible owner still fails.
  const { evidence, calls } = await fails({ changeViewer: value => {
    value.page_id = original.viewer.page_id; value.actual_url = "https://ela.city/home";
  } }, "binding_unavailable_or_cleanup");
  assert.equal(evidence.binding_failure.source, "viewer");
  assert.equal(evidence.binding_failure.phase, "reload");
  assert.deepEqual(evidence.binding_failure.failed_checks, ["page_status_url_matches"]);
  assert.equal(evidence.binding_failure.owner_state, "active");
  assert.equal(evidence.binding_failure.counts.engine_cleanup_obligations, 0);
  assert.equal(evidence.samples.length, 1, "only the completed baseline is a passing sample");
  assert.equal(calls.filter(call => call.method === "state" && call.reloaded).length, 1, "failed owner check is not retried");
  assert.equal(calls.some(call => call.method === "input"), false);
});

test("failed Runtime predicate is retained before any pending viewer check", async () => {
  const { evidence } = await fails({ pendingReads: 3, changeRuntime(raw) {
    raw.sessions.recoverable_page.state = "cleanup_pending";
    raw.sessions.engine_cleanup_obligations = 1;
    raw.page_status = null;
  } }, "binding_unavailable_or_cleanup");
  assert.equal(evidence.binding_failure.source, "runtime");
  assert.equal(evidence.binding_failure.owner_state, "cleanup_pending");
  assert.equal(evidence.binding_failure.counts.engine_cleanup_obligations, 1);
  for (const check of ["owner_active", "no_engine_cleanup", "page_status_schema", "page_status_page_matches"])
    assert.ok(evidence.binding_failure.failed_checks.includes(check));
  assert.ok(evidence.binding_failure.at_ms >= evidence.reload_started_ms);
});

test("reload census records candidate creation, send, ICE, and first frame without the product gate", async () => {
  const timer = virtualClock();
  let reloaded = false, frames = 40, record, recordDiagnostic;
  const evidence = await timer.run(censusBrowserViewerReload({
    clock: timer.clock,
    async observeRequests(callback) {
      record = callback;
      return async () => {};
    },
    async observeDiagnostics(callback) {
      recordDiagnostic = callback;
      return async () => {};
    },
    async readState() {
      if (!reloaded) frames += 1;
      return {
        viewer: {
          page_id: "page-current", engine_id: "selected-engine", exit_id: "selected-exit",
          browser_instance: "instance-current", actual_url: fixtureUrl, document_id: reloaded ? 2 : 1,
        },
        video: {
          present: true, hidden: false, paused: false, ready_state: 4,
          video_width: 640, video_height: 360, client_width: 640, client_height: 360,
          decoded_frames: frames, video_bytes_received: 1000,
        },
      };
    },
    async reloadViewer() {
      reloaded = true;
      frames = 0;
      record({ kind: "navigation", phase: "commit", source_matches: true });
      record({ kind: "signaling", phase: "request", source_matches: true, signal_type: "display_attach" });
      record({ kind: "signaling", phase: "response", source_matches: true, signal_type: "display_attach", status: 200 });
      record({ kind: "signaling", phase: "request", source_matches: true, signal_type: "answer" });
      record({ kind: "signaling", phase: "response", source_matches: true, signal_type: "answer", status: 200 });
      recordDiagnostic({ schema: "elastos.browser.media-diagnostic/v1", event: "viewer_browser_candidate",
        candidate_type: "relay" });
      record({ kind: "signaling", phase: "request", source_matches: true, signal_type: "candidate" });
      record({ kind: "signaling", phase: "response", source_matches: true, signal_type: "candidate", status: 200 });
      recordDiagnostic({ schema: "elastos.browser.media-diagnostic/v1", event: "viewer_ice_state",
        ice_connection_state: "checking" });
      recordDiagnostic({ schema: "elastos.browser.media-diagnostic/v1", event: "viewer_ice_state",
        ice_connection_state: "connected" });
      frames = 8;
    },
  }));
  const classified = classifyReloadCensus(evidence);
  assert.equal(evidence.ok, true);
  assert.equal(evidence.first_frame_in_window, true);
  assert.ok(classified.display_attach_request_ms >= 0);
  assert.ok(classified.first_answer_request_ms >= classified.display_attach_request_ms);
  assert.ok(classified.first_local_candidate_created_ms >= classified.first_answer_request_ms);
  assert.ok(classified.first_candidate_request_ms >= classified.first_local_candidate_created_ms);
  assert.ok(classified.ice_connected_ms >= classified.ice_checking_ms);
  assert.ok(classified.first_decoded_frame_ms >= classified.ice_connected_ms);
});

test("reload census keeps recording past five seconds and classifies a late first frame", async () => {
  const timer = virtualClock();
  let reloaded = false, frames = 40, reloadAt = 0, record;
  const evidence = await timer.run(censusBrowserViewerReload({
    clock: timer.clock,
    watchMs: 20_000,
    async observeRequests(callback) {
      record = callback;
      return async () => {};
    },
    async readState() {
      const elapsed = reloaded ? timer.clock.now() - reloadAt : 0;
      if (!reloaded) frames += 1;
      else frames = elapsed >= 8_000 ? 4 : 0;
      return {
        viewer: {
          page_id: "page-current", engine_id: "selected-engine", exit_id: "selected-exit",
          browser_instance: "instance-current", actual_url: fixtureUrl, document_id: reloaded ? 2 : 1,
        },
        page_status: {
          webrtc_signaling: {
            selkies_offers_received: reloaded ? 2 : 1,
            browser_answers_received: reloaded && elapsed >= 1_000 ? 1 : 0,
          },
        },
        video: {
          present: true, hidden: false, paused: false, ready_state: reloaded && frames === 0 ? 0 : 4,
          video_width: 640, video_height: 360, client_width: 640, client_height: 360,
          decoded_frames: frames, video_bytes_received: 1000,
        },
      };
    },
    async reloadViewer() {
      reloaded = true;
      reloadAt = timer.clock.now();
      frames = 0;
      record({ kind: "navigation", phase: "commit", source_matches: true, document_generation: 1 });
      record({ kind: "signaling", phase: "response", source_matches: true, signal_type: "display_attach",
        status: 200, initial_offer_ice: { candidate_count: 1, relay_candidate_count: 1 } });
    },
  }));
  assert.equal(evidence.ok, true);
  assert.equal(evidence.classification.first_decoded_frame_after_deadline, true);
  assert.ok(evidence.classification.first_decoded_frame_ms >= 8_000);
  assert.ok(evidence.classification.last_sample_ms >= 15_000);
  assert.deepEqual(evidence.classification.attach_initial_offer_ice,
    { candidate_count: 1, relay_candidate_count: 1 });
  assert.equal(evidence.classification.document_generations[0], 1);
  assert.ok(evidence.samples.some((row) => row.engine_signaling?.selkies_offers_received === 2));
});

test("reload retains summary and attachment header timing without accepting bodies or changing its gate", async () => {
  const f = fixture(), reload = f.args.reloadViewer;
  f.args.reloadViewer = async budget => {
    await reload(budget);
    f.event("summary", "request"); f.event("summary", "response", { status: 200 });
    f.event("signaling", "headers", { signal_type: "display_attach", status: 200 });
  };
  const evidence = await f.run();
  assert.equal(evidence.started_monotonic_ms, 0);
  assert.equal(evidence.requests.filter(row => row.kind === "summary").length, 2);
  assert.ok(evidence.requests.some(row => row.kind === "signaling" && row.event === "headers" && row.status === 200));
  assert.ok(evidence.reload_ms <= 5000); assert.ok(!JSON.stringify(evidence).includes(secret));
});

test("reload census treats extra navigation events as observers when the document hash stays the same", () => {
  const classified = classifyReloadCensus({
    reload_started_ms: 100,
    requests: [
      { at_ms: 200, kind: "navigation", event: "commit", document_generation: 1 },
      { at_ms: 800, kind: "navigation", event: "commit", document_generation: 2 },
    ],
    samples: [
      { at_ms: 150, phase: "reload", document_hash: "sha256:samehash000001", visible: false, decoded_frames: 0 },
      { at_ms: 900, phase: "reload", document_hash: "sha256:samehash000001", visible: true, decoded_frames: 4 },
    ],
    diagnostics: [],
  });
  assert.deepEqual(classified.document_generations, [1, 2]);
  assert.equal(classified.observer_navigation_events, 2);
  assert.deepEqual(classified.document_hashes, ["sha256:samehash000001"]);
  assert.equal(classified.same_document, true);
  assert.equal(classified.second_document_ms, null);
});

test("reload census records a second document only when the sampled document hash changes", () => {
  const classified = classifyReloadCensus({
    reload_started_ms: 0,
    requests: [{ at_ms: 10, kind: "navigation", event: "commit", document_generation: 1 }],
    samples: [
      { at_ms: 20, phase: "reload", document_hash: "sha256:firstdocument01", visible: false },
      { at_ms: 80, phase: "reload", document_hash: "sha256:seconddocument1", visible: false },
    ],
    diagnostics: [],
  });
  assert.equal(classified.same_document, false);
  assert.equal(classified.second_document_ms, 80);
  assert.deepEqual(classified.document_hashes, ["sha256:firstdocument01", "sha256:seconddocument1"]);
});

test("reload census records dying-page and restore-boot request identity", () => {
  const classified = classifyReloadCensus({
    reload_started_ms: 100,
    requests: [],
    samples: [],
    diagnostics: [
      { at_ms: 101, event: "dying_page_attach", request_id: "a".repeat(32) },
      { at_ms: 180, event: "restore_boot", attach_from_url: true, request_id: "a".repeat(32) },
      { at_ms: 220, event: "restore_boot_ready", from_summary: true, request_id: "a".repeat(32) },
    ],
  });
  assert.equal(classified.dying_page_attach_ms, 1);
  assert.equal(classified.dying_page_attach_request_id, "a".repeat(32));
  assert.equal(classified.restore_boot_ms, 80);
  assert.equal(classified.restore_boot_attach_from_url, true);
  assert.equal(classified.restore_boot_request_id, "a".repeat(32));
  assert.equal(classified.restore_boot_ready_ms, 120);
  assert.equal(classified.restore_boot_from_summary, true);
});

test("reload census captures viewer failure status and the original exception fields", () => {
  const classified = classifyReloadCensus({
    reload_started_ms: 0,
    requests: [
      { at_ms: 100, kind: "signaling", event: "response", signal_type: "display_attach", status: 200 },
    ],
    samples: [
      {
        at_ms: 366,
        phase: "reload",
        viewer_status: "Browser could not complete the request. Refresh Browser and try again.",
        viewer_failure_stage: "peer_setup",
        viewer_failure_message: "Browser display could not connect.",
        viewer_failure_status: 400,
      },
    ],
    diagnostics: [],
  });
  assert.equal(classified.viewer_failure_ms, 366);
  assert.equal(classified.viewer_failure_stage, "peer_setup");
  assert.equal(classified.viewer_failure_message, "Browser display could not connect.");
  assert.equal(classified.viewer_failure_status, 400);
  assert.match(classified.viewer_failure_status_text, /could not complete/);
});
