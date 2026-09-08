import assert from "node:assert/strict";
import test from "node:test";
import vm from "node:vm";
import { PassThrough } from "node:stream";
import { EventEmitter } from "node:events";
import { mkdtemp, writeFile, readFile, rm, rename, utimes, realpath } from "node:fs/promises";
import { writeFileSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { validatePlan, validateJourney, validateAttempt, auditQualification, sha256,
  TERMINAL_EFFECTS, percentile, cleanControl, candidateFingerprint, validateInstallation, validateLiveIdentity, deriveMedia } from "../browser-qualification-audit.mjs";
import { freezeArtifacts, checkArtifacts, journeyEnvironment, parseProcesses, ownedProcesses, runJourneyChild, executableInode } from "../browser-qualification-runner.mjs";
import { qualificationOptions, installQualificationReceiver, observationCall, createQualificationHarness, createQualificationCancellation, qualificationInteraction, cleanupQualificationOpens } from "./browser-qualification-observer.mjs";
import { createBrowserJourneyFixture } from "./browser-journey-fixture.mjs";

function plan(mode = "lifecycle") {
  const p = { schema: "elastos.browser.qualification-plan/v1", mode, count: 100, duration_ms: 1_800_000,
    require_operator: true, candidate: { platform: "darwin-arm64", source_commit: "a".repeat(40),
      source_tree: "b".repeat(40), artifacts: ["runtime", "browser_ui", "engine_adapter", "image_manifest",
        "rootfs", "kernel", "initrd", "host_helper", "relay", "components", "installation_review"].map(role =>
        ({ role, path: "/fixture/" + role, sha256: "c".repeat(64) })) },
    runtime: { base_url: "http://localhost:61510", browser_ui_url: "http://localhost:61510/apps/browser/browser.js", fixture_origin: "http://localhost:61511",
      profile: "/fixture/profile", control_socket: "/fixture/control.sock", operator_coords: "/fixture/gateway-runtime-coords.json",
      viewer_version: "fixture-chromium", headed: true, engine_id: "local", exit_id: "runtime-default",
      resource_roots: [{ role: "runtime", pid: 10, start: "birth-a" }, { pid: 20, start: "birth-b" }] },
    network: { profile: "local", control_rtt_ms: 1 },
    limits: { rss_bytes: 1024 * 1024, cpu_percent: 400, fps: 30, width: 1920, height: 1080 } };
  p.candidate.artifacts.find(a => a.role === "installation_review").sha256 = sha256(JSON.stringify(installation(p)));
  return p;
}
function installation(p) {
  return { schema: "elastos.browser.qualification-installation/v1", reviewed: true, reviewer: "source fixture",
    source_commit: p.candidate.source_commit, source_tree: p.candidate.source_tree,
    artifacts: p.candidate.artifacts.filter(a => a.role !== "installation_review").map(a => ({ role: a.role, path: a.path,
      source_commit: p.candidate.source_commit, source_tree: p.candidate.source_tree,
      built_sha256: a.sha256, installed_sha256: a.sha256 })) };
}
function frozenInputs(p) {
  return [...p.candidate.artifacts, { role: "operator_coordinates", path: p.runtime.operator_coords, sha256: "e".repeat(64) },
    ...["scripts/browser-qualification-runner.mjs", "scripts/browser-qualification-audit.mjs",
      "scripts/lib/browser-qualification-observer.mjs", "scripts/home-passkey-virtual-auth-smoke.mjs"].map(role =>
      ({ role, path: "/fixture/" + role, sha256: "e".repeat(64) }))].map(a => ({ ...a, identity: "1:2:3:4:5" }));
}
function liveIdentity(p) {
  return { runtime_pid: 10, runtime_start: "birth-a", runtime_executable: "/fixture/runtime", runtime_inode: "2",
    listening_pids: [10], browser_ui_url: p.runtime.browser_ui_url, browser_ui_sha256: "c".repeat(64) };
}
function sealBundle(dir, receipt) {
  receipt.frozen_inputs = frozenInputs(receipt.plan);
  receipt.candidate_fingerprint = candidateFingerprint(validatePlan(receipt.plan), receipt.frozen_inputs);
  receipt.live_before = receipt.live_after = liveIdentity(receipt.plan);
  writeFileSync(join(dir, "installation-review.json"), JSON.stringify(installation(receipt.plan)));
  for (const ref of receipt.attempts) {
    const a = JSON.parse(readFileSync(join(dir, ref.file)));
    a.candidate_fingerprint = receipt.candidate_fingerprint; a.live_after = liveIdentity(receipt.plan);
    const bytes = JSON.stringify(a); writeFileSync(join(dir, ref.file), bytes); ref.sha256 = sha256(bytes);
  }
  return receipt;
}
const tone = () => ({ ok: true, receiver_unchanged: true, receiver_muted: false, receiver_paused: false,
  track_state: "live", context_state: "running", probe_context_closed: true,
  samples: Array.from({ length: 40 }, (_, i) => ({ at_ms: 500 + i * 50, rms: 0.03, peak_hz: 445.3 })) });
function journey(id = "fixture-run-0001") {
  const pageId = "page:vz-fixture";
  return { page_id: pageId, display_mode: "webrtc_remote_display", controlled_journey: {
    schema: "elastos.browser.controlled-journey/v1", run: id,
    pages: ["main", "nav"].map(name => ({ name, page_id: pageId, load: { type: "load" },
      video: { ready: { decoded_frames: 1 }, decoded: { decoded_frames: 2 } } })),
    audio: tone(), audio_after_recovery: tone(), input: { text: "Browser-test",
      receipt: { events: [{ type: "input", value: "Browser-test" }, { type: "scroll", value: "Browser-test", scroll_y: 640 }] } },
    inspection: { pages: [{}, {}], after_navigation: { status: 409, body: { code: "stale_inspection" } } },
    viewer_reload: { ok: true }, operator: { ok: true },
    qualification: { schema: "elastos.browser.qualification-observation/v1",
      open_attempts: [{ outcome: "completed" }],
      services: { engine_id: "local", exit_id: "runtime-default" },
      launch: { page_id: pageId, clock: "host_monotonic", usable_frame_ms: 1000 } },
    close: { window_detached: true,
      receipt: { schema: "elastos.browser.close-result/v1", closed: true, page_id: pageId, cleanup_id: "cleanup-fixture",
        cleanup: { ok: true, action: "released_exact_runtime_browser_ownership" },
        transport_proof: { page_id: pageId }, terminal_effects: Object.fromEntries(TERMINAL_EFFECTS.map(k => [k, true])) },
      sessions_after_close: { active_sessions: 0, principal_sessions: 0, total_sessions: 0, launching_sessions: 0,
        engine_cleanup_obligations: 0, launch_reconciliation_obligations: 0, recoverable_page: null } } } };
}
function control() {
  return { schema: "elastos.browser.vm-control-service.status/v1", ok: true, direct_network: false,
    network_mode: "runtime_net_only", active_pages: 0, active_vms: 0, warm_vms: 0, pending_launches: 0,
    page_ids: [], active_stream_ids: [], pending_stream_ids: [], lifecycle: { sessions: [] } };
}
function attempt(index = 1, mode = "lifecycle") {
  return { schema: "elastos.browser.qualification-attempt/v1", index, mode, ok: true, child_exit: 0,
    candidate_fingerprint: "f".repeat(64), candidate_unchanged: true, before: control(), after: control(),
    observed_page_ids: ["page:vz-fixture"], active_vm_keys: ["vm-fixture"],
    viewer: { version: "fixture-chromium", headed: true },
    resource_peak: { identity_verified: true, rss_bytes: 128, cpu_percent: 10 },
    journey: journey("fixture-run-" + index) };
}
test("qualification plan enforces production counts/durations and keeps probe runs separate", () => {
  assert.equal(validatePlan(plan()).count, 100);
  for (const change of [{ count: 99 }, { mode: "media", duration_ms: 1000 }, { mode: "mixed", duration_ms: 1_800_000 },
    { require_operator: false }, { count: 101 }]) assert.throws(() => validatePlan({ ...plan(), ...change }));
  assert.equal(validatePlan({ ...plan(), count: 1, probe: true }).probe, true);
  assert.throws(() => validatePlan({ ...plan(), candidate: { ...plan().candidate, platform: "linux-arm64" } }));
  assert.equal(qualificationOptions({}), null);
  assert.throws(() => qualificationOptions({ HOME_VIRTUAL_AUTH_BROWSER_QUALIFICATION_MODE: "mixed" }));
});
test("private installed journey flags select the embedded flow and remove conflicting inherited modes", () => {
  const old = process.env.HOME_VIRTUAL_AUTH_BROWSER_OPEN;
  process.env.HOME_VIRTUAL_AUTH_BROWSER_OPEN = "1";
  try {
    const env = journeyEnvironment(plan(), 3);
    assert.equal(env.HOME_VIRTUAL_AUTH_BROWSER_OPEN, undefined);
    for (const suffix of ["BROWSER", "BROWSER_SUMMARY", "BROWSER_EMBEDDED_UI_INPUT",
      "BROWSER_CONTROLLED_JOURNEY", "BROWSER_CONTROLLED_MEDIA", "BROWSER_CONTROLLED_INSPECTION",
      "BROWSER_CONTROLLED_OPERATOR", "BROWSER_CONTROLLED_VIEWER_RELOAD"]) assert.equal(env["HOME_VIRTUAL_AUTH_" + suffix], "1");
    assert.equal(env.HOME_VIRTUAL_AUTH_BROWSER_OPERATOR_COORDS, "/fixture/gateway-runtime-coords.json");
    assert.equal(env.HOME_VIRTUAL_AUTH_REUSE_SIGNED_HOME, "0");
    const reused = plan(); reused.runtime.reuse_signed_home = true;
    assert.equal(journeyEnvironment(reused, 3).HOME_VIRTUAL_AUTH_REUSE_SIGNED_HOME, "1");
    reused.runtime.reuse_signed_home = "true";
    assert.throws(() => validatePlan(reused), /reuse_option/);
  } finally {
    if (old === undefined) delete process.env.HOME_VIRTUAL_AUTH_BROWSER_OPEN;
    else process.env.HOME_VIRTUAL_AUTH_BROWSER_OPEN = old;
  }
});
test("current close receipt omits already_closed; every terminal effect remains mandatory", () => {
  validateJourney(journey());
  for (const key of TERMINAL_EFFECTS) {
    const j = journey(); j.controlled_journey.close.receipt.terminal_effects[key] = false;
    assert.throws(() => validateJourney(j), /thirteen_effect/);
  }
  for (const change of [j => { j.audio.samples.forEach(s => { s.rms = 0; }); },
    j => { j.operator.ok = false; }, j => { j.viewer_reload.ok = false; },
    j => { j.inspection.after_navigation.status = 200; },
    j => { j.close.receipt.already_closed = true; },
    j => { j.close.sessions_after_close.engine_cleanup_obligations = 1; }]) {
    const j = journey(); change(j.controlled_journey); assert.throws(() => validateJourney(j));
  }
});
test("attempts bind exact viewer, services, observed local VM and monotonic launch", () => {
  validateAttempt(attempt(), plan());
  for (const change of [a => { a.candidate_unchanged = false; }, a => { a.viewer.version = "other"; },
    a => { a.observed_page_ids = []; }, a => { a.active_vm_keys = []; },
    a => { a.journey.controlled_journey.qualification.services.exit_id = "other"; },
    a => { a.resource_peak.rss_bytes = 9e9; }]) {
    const a = attempt(); change(a); assert.throws(() => validateAttempt(a, plan()));
  }
  const cold = attempt(1, "cold");
  cold.journey.controlled_journey.qualification.launch.clock = "wall_clock";
  assert.throws(() => validateAttempt(cold, plan("cold")), /monotonic/);
});
test("warm conditioning stays unsupported, including an idle warm VM beside a cold allocation", () => {
  const a = attempt(1, "warm");
  a.before.active_vms = a.before.warm_vms = 1;
  a.before.lifecycle.sessions = [{ warm_vm: true, vm_key_hash: "idle-old-vm" }];
  a.active_vm_keys = ["idle-old-vm", "new-cold-vm"];
  assert.throws(() => validatePlan(plan("warm")), /warm_conditioning_unsupported/);
  assert.throws(() => validateAttempt(a, plan("warm")), /warm_conditioning_unsupported/);
});
test("percentiles retain outliers and reject missing/non-finite samples", () => {
  assert.equal(percentile([...Array(94).fill(1000), ...Array(6).fill(6000)]), 6000);
  assert.equal(percentile([...Array(95).fill(1000), ...Array(5).fill(6000)]), 1000);
  for (const values of [[], [NaN], [-1], [Infinity]]) assert.throws(() => percentile(values));
});
test("hash verification reuses unchanged identity but rejects same-size mutation, replacement and receipt changes", async () => {
  const dir = await realpath(await mkdtemp(join(tmpdir(), "browser-qualification-test-")));
  const path = join(dir, "artifact"), receipt = join(dir, "receipt");
  try {
    await writeFile(path, "same"); await writeFile(receipt, "v001");
    const frozen = await freezeArtifacts([{ role: "image", path, sha256: sha256("same") },
      { role: "receipt", path: receipt, sha256: sha256("v001") }]);
    await checkArtifacts(frozen); await checkArtifacts(frozen);
    const { mtime } = await (await import("node:fs/promises")).stat(path);
    await writeFile(path, "edit"); await utimes(path, mtime, mtime);
    await assert.rejects(checkArtifacts(frozen), /candidate_changed/);
    await assert.rejects(freezeArtifacts([{ role: "image", path, sha256: sha256("same") }]), /candidate_hash/);
    await writeFile(path, "same");
    const second = await freezeArtifacts([{ role: "image", path, sha256: sha256("same") }]);
    await writeFile(join(dir, "replacement"), "same"); await rename(join(dir, "replacement"), path);
    await assert.rejects(checkArtifacts(second), /candidate_changed/);
    const third = await freezeArtifacts([{ role: "receipt", path: receipt, sha256: sha256("v001") }]);
    await writeFile(receipt, "v002"); await assert.rejects(checkArtifacts(third), /candidate_changed/);
    const c = new AbortController(); c.abort();
    await assert.rejects(freezeArtifacts([{ role: "image", path, sha256: sha256("same") }], c.signal), /cancelled/);
  } finally { await rm(dir, { recursive: true, force: true }); }
});
test("process accounting follows owned descendants through orphaning and rejects reused root PID", () => {
  const rows = parseProcesses("10 1 100 2.0 Tue Sep 8 10:00:00 2026\n20 10 200 3.0 Tue Sep 8 10:00:01 2026\n30 1 900 1.0 Tue Sep 8 09:00:00 2026\n");
  const root = [{ pid: 10, start: rows[0].start }], tracked = new Set();
  assert.deepEqual(ownedProcesses(rows, root, tracked).map(p => p.pid), [10, 20]);
  rows[1].ppid = 1;
  assert.deepEqual(ownedProcesses(rows, root, tracked).map(p => p.pid), [10, 20]);
  rows[0].start = "replacement"; assert.throws(() => ownedProcesses(rows, root, tracked), /owner_replaced/);
});
test("bounded observer calls cancel and time out without widening their deadline", async () => {
  await assert.rejects(observationCall(() => new Promise(() => {}), new AbortController().signal, 5), /deadline/);
  const c = new AbortController();
  const pending = observationCall(() => new Promise(() => {}), c.signal); c.abort();
  await assert.rejects(pending, /cancelled/);
});
test("launch clock excludes Home auth and binds first frame before completed open poll", async () => {
  const page = new EventEmitter(); let callback, clock = 0;
  const context = { async exposeBinding(name, fn) { callback = fn; }, async addInitScript() {} };
  const frame = { async evaluate() { return { engine_id: "local", exit_id: "runtime-default" }; } };
  const harness = await createQualificationHarness(context, page, { mode: "cold", duration_ms: 0 }, () => clock);
  const req = (path, method = "POST") => ({ frame: () => frame, method: () => method,
    url: () => "http://localhost" + path });
  const respond = async (request, body) => {
    page.emit("response", { request: () => request, url: request.url, json: async () => body });
    await new Promise(resolve => setImmediate(resolve));
  };
  try {
    page.emit("request", req("/api/auth/passkey/sign-in"));
    clock = 37_000; // Complete Home setup before Browser allocation starts.
    const open = req("/api/apps/browser/open"); page.emit("request", open);
    await respond(open, { open_id: "open-current" });
    clock = 37_250;
    callback({ frame }, { page_id: "page:vz-fixture", width: 1920, height: 1080 });
    const result = { schema: "elastos.browser.open-result/v1", engine_page: { page_id: "page:vz-fixture" } };
    await respond(req("/api/apps/browser/open/open-old", "GET"), {
      schema: "elastos.browser.open-status/v1", status: "completed", result });
    await assert.rejects(harness.observe({ appFrame: frame, pageId: "page:vz-fixture" }), /open_retry_or_failure/);
    clock = 38_000;
    await respond(req("/api/apps/browser/open/open-current", "GET"), {
      schema: "elastos.browser.open-status/v1", status: "completed", result });
    const evidence = await harness.observe({ appFrame: frame, pageId: "page:vz-fixture" });
    assert.equal(evidence.launch.usable_frame_ms, 250);
    assert.equal(evidence.launch.clock, "host_monotonic");
  } finally { await harness.stop(); }
  assert.equal(page.listenerCount("request"), 0); assert.equal(page.listenerCount("response"), 0);
});
test("qualification fixture retains an active eight-hour run with a 128-event ring and monotonic cursor", async () => {
  let clock = 1_000_000;
  const server = createBrowserJourneyFixture({ now: () => clock });
  await new Promise(r => server.listen(0, "127.0.0.1", r));
  const base = "http://127.0.0.1:" + server.address().port, run = "qualified-run-123";
  try {
    assert.equal((await fetch(base + "/health").then(r => r.json())).qualification, "bounded-v1");
    await fetch(base + "/main?run=" + run + "&qualification=1");
    const event = { type: "input", page: "main", value: "test", scroll_x: 0, scroll_y: 0,
      input_rect: { x: 0, y: 0, width: 10, height: 10 } };
    for (let i = 0; i < 140; i++) {
      const response = await fetch(base + "/events?run=" + run, { method: "POST",
        headers: { "content-type": "application/json" }, body: JSON.stringify(event) });
      assert.equal(response.status, 200);
    }
    for (let i = 0; i < 54; i++) {
      clock += 9 * 60_000;
      const receipt = await fetch(base + "/receipt?run=" + run).then(r => r.json());
      assert.equal(receipt.events.length, 128); assert.equal(receipt.events[0].sequence, 13);
      assert.equal(receipt.total_events, 140); assert.equal(receipt.dropped_events, 12);
    }
    assert.equal((await fetch(base + "/nav?run=" + run)).status, 409);
    clock += 10 * 60_000;
    assert.equal((await fetch(base + "/receipt?run=" + run)).status, 404);
  } finally { server.closeAllConnections(); await new Promise(r => server.close(r)); }
});
test("qualification audit binds all 100 unique attempts, raw logs and samples; partial/probe/tampered sets fail", async () => {
  const dir = await mkdtemp(join(tmpdir(), "browser-qualification-audit-test-"));
  try {
    const p = plan(), refs = [], resources = [];
    for (let i = 1; i <= 100; i++) {
      const a = attempt(i), log = "fixture raw log " + i, file = "attempt-" + i + ".json";
      a.log = { file: "attempt-" + i + ".log", sha256: sha256(log) };
      writeFileSync(join(dir, a.log.file), log);
      const bytes = JSON.stringify(a); writeFileSync(join(dir, file), bytes);
      refs.push({ file, sha256: sha256(bytes) });
      resources.push(JSON.stringify({ sequence: i, at_ms: i * 60_000, attempt: i, kind: "resources",
        value: { identity_verified: true, rss_bytes: 128, cpu_percent: 10 } }));
    }
    const sampleBytes = resources.join("\n") + "\n"; writeFileSync(join(dir, "samples.jsonl"), sampleBytes);
    const planBytes = JSON.stringify(p); writeFileSync(join(dir, "plan.json"), planBytes);
    const receipt = { schema: "elastos.browser.qualification/v1", plan: p, plan_sha256: sha256(planBytes),
      candidate_fingerprint: "f".repeat(64), completed: true, candidate_unchanged: true, cancelled: false,
      attempts: refs, samples: { file: "samples.jsonl", bytes: Buffer.byteLength(sampleBytes),
        records: 100, sha256: sha256(sampleBytes) } };
    const path = join(dir, "qualification.json"), save = value => writeFileSync(path, JSON.stringify(value));
    sealBundle(dir, receipt); save(receipt);
    const accepted = auditQualification(path);
    assert.equal(accepted.ok, true); assert.equal(accepted.product_accepted, false);
    save({ ...receipt, frozen_inputs: undefined });
    assert.throws(() => auditQualification(path), /frozen_inputs/);
    const relabeled = structuredClone(receipt);
    relabeled.plan.candidate.source_commit = "d".repeat(40);
    relabeled.plan.candidate.source_tree = "e".repeat(40);
    relabeled.plan.candidate.artifacts.forEach(a => { a.sha256 = "9".repeat(64); });
    const changedPlan = JSON.stringify(relabeled.plan); writeFileSync(join(dir, "plan.json"), changedPlan);
    relabeled.plan_sha256 = sha256(changedPlan); save(relabeled);
    assert.throws(() => auditQualification(path), /candidate_frozen_input/);
    writeFileSync(join(dir, "plan.json"), planBytes);
    for (const variant of [
      { ...receipt, completed: false }, { ...receipt, cancelled: true },
      { ...receipt, attempts: refs.slice(0, 99) },
      { ...receipt, attempts: [...refs.slice(0, 99), refs[0]] },
      { ...receipt, plan: { ...p, probe: true } },
      { ...receipt, samples: { ...receipt.samples, records: 99 } },
    ]) { save(variant); assert.throws(() => auditQualification(path)); }
    save(receipt); await writeFile(join(dir, refs[0].file), "{}");
    assert.throws(() => auditQualification(path), /attempt_hash/);
  } finally { await rm(dir, { recursive: true, force: true }); }
});
test("installation and running Gateway identity reject stale source, binary inode, listener and served bytes", () => {
  const p = plan(), inputs = frozenInputs(p);
  validateInstallation(p, installation(p)); validateLiveIdentity(p, inputs, liveIdentity(p));
  const wrong = installation(p); wrong.artifacts[0].built_sha256 = "9".repeat(64);
  assert.throws(() => validateInstallation(p, wrong), /artifact_mismatch/);
  const source = installation(p); source.source_commit = "9".repeat(40);
  assert.throws(() => validateInstallation(p, source), /source_mismatch/);
  for (const change of [{ runtime_inode: "3" }, { runtime_pid: 11 }, { runtime_start: "reused-pid" },
    { runtime_executable: "/other/runtime" }, { listening_pids: [11] }, { browser_ui_sha256: "9".repeat(64) }]) {
    assert.throws(() => validateLiveIdentity(p, inputs, { ...liveIdentity(p), ...change }), /live_candidate/);
  }
  assert.equal(executableInode("ftxt\ni123\nn/fixture/runtime\n", "/fixture/runtime"), "123");
  assert.throws(() => executableInode("ftxt\ni122\nn/fixture/runtime (deleted)\n", "/fixture/runtime"), /not_mapped/);
});
test("a composite candidate retains separate lineage for extra installed helper artifacts", () => {
  const p = plan();
  for (const role of ["engine_js", "vm_control", "preflight", "python_writer", "browser_webrtc"]) {
    p.candidate.artifacts.push({ role, path: "/fixture/" + role, sha256: "d".repeat(64) });
  }
  validatePlan(p);
  const review = installation(p);
  for (const a of review.artifacts.filter(a => a.role !== "runtime")) {
    a.source_commit = "e".repeat(40); a.source_tree = "f".repeat(40);
  }
  validateInstallation(p, review);
  review.artifacts.find(a => a.role === "python_writer").installed_sha256 = "9".repeat(64);
  assert.throws(() => validateInstallation(p, review), /installation_artifact_mismatch:python_writer/);
  for (let i = p.candidate.artifacts.length; i <= 40; i++) p.candidate.artifacts.push({
    role: "helper_" + i, path: "/fixture/helper_" + i, sha256: "d".repeat(64) });
  assert.throws(() => validatePlan(p), /candidate_artifacts_invalid/);
});
test("a 6.2-second open retry retains the original clock and cannot qualify", async () => {
  const page = new EventEmitter(); let callback, clock = 0;
  const frame = { evaluate: async () => ({ engine_id: "local", exit_id: "runtime-default" }) };
  const context = { exposeBinding: async (name, fn) => { callback = fn; }, addInitScript: async () => {} };
  const h = await createQualificationHarness(context, page, { mode: "cold", duration_ms: 0 }, () => clock);
  const request = () => ({ method: () => "POST", url: () => "http://localhost/api/apps/browser/open", frame: () => frame });
  try {
    page.emit("request", request()); clock = 6000;
    const second = request(); page.emit("request", second);
    page.emit("response", { request: () => second, url: second.url,
      json: async () => ({ schema: "elastos.browser.open-result/v1", engine_page: { page_id: "page-retry" } }) });
    await new Promise(setImmediate); clock = 6200;
    callback({ frame }, { page_id: "page-retry", width: 1920, height: 1080 });
    await assert.rejects(h.observe({ appFrame: frame, pageId: "page-retry" }), error => {
      assert.equal(error.qualification.launch.usable_frame_ms, 6200);
      assert.deepEqual(error.qualification.open_attempts.map(a => a.outcome), ["interrupted_by_retry", "completed"]);
      return /retry_or_failure/.test(error.message);
    });
  } finally { await h.stop(); }
  const j = journey(); j.controlled_journey.prior_open_settlements = [{ open_id: "failed-first",
    settlement: { status: "failed", error: { outcome: { state: "terminal_post_effect_cleanup" } } } }];
  assert.throws(() => validateJourney(j), /prior_open_failure/);
});
test("cancellation closes the owned viewer across all stages, including late startup acquisition", async () => {
  for (const stage of ["viewer_startup", "home_auth", "allocation", "observation", "reload", "operator", "close"]) {
    const messages = new EventEmitter(), c = createQualificationCancellation(true, messages);
    let closed = 0, resolveContext;
    const context = { close: async () => { closed++; } };
    const pending = c.ownContext(stage === "viewer_startup" ? new Promise(r => { resolveContext = r; }) : Promise.resolve(context));
    if (stage !== "viewer_startup") await pending;
    messages.emit("message", { schema: "elastos.browser.qualification-cancel/v1" });
    if (stage === "viewer_startup") { await assert.rejects(pending, /cancelled/); resolveContext(context); await new Promise(setImmediate); }
    assert.throws(() => c.check(), /cancelled/, stage);
    await c.stop(); assert.equal(closed, 1, stage); assert.equal(c.evidence.cancelled, true);
    assert.equal(messages.listenerCount("message"), 0);
  }
});
test("cancelled child exits with held pipes: bounded drain kills verified descendants and keeps reused PIDs", async () => {
  const child = new EventEmitter(), birth = new Date(Math.floor(Date.now() / 1000) * 1000).toString();
  child.pid = 80001; child.connected = true; child.stdout = new PassThrough(); child.stderr = new PassThrough();
  const messages = [], signals = [];
  child.send = (m, callback) => { messages.push(m.schema); callback?.(); };
  child.disconnect = () => { child.connected = false; };
  let rows = [{ pid: 80001, ppid: 1, pgid: 80001, start: birth },
    { pid: 80002, ppid: 80001, pgid: 80001, start: birth },
    { pid: 80003, ppid: 80001, pgid: 80001, start: birth }];
  const c = new AbortController();
  const pending = runJourneyChild(plan(), 1, { signal: c.signal, log() {}, sample() {} }, {
    spawnChild: () => child, scan: async () => structuredClone(rows), graceMs: 5, killMs: 5, drainMs: 5, finalMs: 100,
    signalProcess: (pid, sig) => { signals.push([pid, sig]); rows = rows.filter(p => p.pid !== pid); },
  });
  await new Promise(setImmediate);
  rows = rows.filter(p => p.pid !== child.pid);
  rows.find(p => p.pid === 80003).start = "Mon Jan 1 00:00:00 2001";
  rows.find(p => p.pid === 80003).ppid = rows.find(p => p.pid === 80003).pgid = 1;
  c.abort(); child.emit("exit", null, "SIGKILL"); // Deliberately never emit close.
  const result = await pending;
  assert.equal(result.failure, "cancelled"); assert.equal(result.forced_cleanup, true);
  assert.ok(signals.some(([pid]) => pid === 80002)); assert.ok(signals.every(([pid]) => pid !== 80003));
  assert.deepEqual(result.child_cleanup.remaining, []);
  assert.equal(child.stdout.destroyed, true); assert.equal(child.stderr.destroyed, true);
  assert.deepEqual(messages, ["elastos.browser.qualification-cancel/v1"]);
});
test("stalled child cleanup still reaches a failed terminal result within its final deadline", async () => {
  const child = new EventEmitter(); child.pid = 80004; child.connected = false;
  child.stdout = new PassThrough(); child.stderr = new PassThrough();
  const pending = runJourneyChild(plan(), 1, { signal: new AbortController().signal, log() {}, sample() {} }, {
    spawnChild: () => child, scan: () => new Promise(() => {}), drainMs: 5, finalMs: 10,
    signalProcess() { assert.fail("unverified process signal"); },
  });
  child.emit("exit", null, "SIGKILL");
  const result = await pending;
  assert.match(result.failure, /pipe_drain_deadline/);
  assert.equal(child.stdout.destroyed, true);
});
test("exit before the first identity scan never authorizes a replacement PID or process group", async () => {
  const child = new EventEmitter(); child.pid = 81001; child.connected = false;
  child.stdout = new PassThrough(); child.stderr = new PassThrough();
  let firstScan, scans = 0;
  const rows = [{ pid: child.pid, ppid: 1, pgid: child.pid, start: new Date(Date.now() + 1100).toString() },
    { pid: 81002, ppid: child.pid, pgid: child.pid, start: new Date(Date.now() + 1100).toString() }];
  const signals = [];
  const pending = runJourneyChild(plan(), 1, { signal: new AbortController().signal, log() {}, sample() {} }, {
    spawnChild: () => child,
    scan: () => ++scans === 1 ? new Promise(resolve => { firstScan = resolve; }) : Promise.resolve(structuredClone(rows)),
    signalProcess: (...args) => { signals.push(args); }, drainMs: 5, finalMs: 100,
  });
  child.emit("exit", 0, null);
  firstScan(structuredClone(rows)); // The outstanding pre-exit scan returns only replacement identities.
  const result = await pending;
  assert.deepEqual(signals, []);
  assert.ok(result.child_cleanup.identity_errors.includes("child_identity_unobserved_before_exit"));
  assert.ok(result.failure); assert.equal(child.stdout.destroyed, true); assert.equal(child.stderr.destroyed, true);
});
test("a known child exit keeps newly observed group members unconfirmed without signaling them", async () => {
  const child = new EventEmitter(), birth = new Date(Math.floor(Date.now() / 1000) * 1000).toString();
  child.pid = 81003; child.connected = false; child.stdout = new PassThrough(); child.stderr = new PassThrough();
  let rows = [{ pid: child.pid, ppid: 1, pgid: child.pid, start: birth }];
  const signals = [];
  const pending = runJourneyChild(plan(), 1, { signal: new AbortController().signal, log() {}, sample() {} }, {
    spawnChild: () => child, scan: async () => structuredClone(rows), drainMs: 5, finalMs: 100,
    signalProcess: (...args) => { signals.push(args); },
  });
  await new Promise(setImmediate);
  child.emit("exit", 0, null);
  rows = [{ pid: 81004, ppid: 1, pgid: child.pid, start: birth }];
  const result = await pending;
  assert.deepEqual(signals, []);
  assert.ok(result.child_cleanup.identity_errors.includes("child_identity_unknown_after_exit"));
  assert.ok(result.failure);
});
test("cancellation captures a late open through viewer disposal before final Runtime cleanup", async () => {
  const messages = new EventEmitter(), c = createQualificationCancellation(true, messages), page = new EventEmitter();
  let finishClose, closed = false;
  const context = { close: () => new Promise(resolve => { finishClose = () => { closed = true; resolve(); }; }),
    exposeBinding: async () => {}, addInitScript: async () => {} };
  await c.ownContext(Promise.resolve(context));
  const h = await createQualificationHarness(context, page, { mode: "lifecycle", duration_ms: 0 }, undefined, c);
  messages.emit("message", { schema: "elastos.browser.qualification-cancel/v1" });
  const stopping = h.stop(); // Main finally can begin while disposal is still pending.
  await new Promise(setImmediate);
  assert.equal(closed, false); assert.equal(c.evidence.runtime_cleanup, undefined);
  assert.equal(page.listenerCount("request"), 1);
  page.emit("request", { method: () => "POST", url: () => "http://localhost:61510/api/apps/browser/open",
    frame: () => context, headers: () => ({}), postDataJSON: () => ({ browser_instance: "synthetic-instance" }) });
  finishClose(); await stopping;
  const evidence = await c.stop();
  assert.equal(h.snapshot().open_attempts.length, 1);
  assert.equal(h.snapshot().open_attempts[0].outcome, "pending");
  assert.equal(evidence.context_closed, true); assert.equal(evidence.runtime_cleanup.ok, false);
  assert.equal(evidence.runtime_cleanup.error, "cancel_cleanup_owner_unavailable");
  assert.equal(page.listenerCount("request"), 0); // No credentials or network are used by this negative fixture.
});
test("uncertain viewer disposal and post-disposal dispatch cannot retain empty cleanup success", async () => {
  for (const fails of [true, false]) {
    const messages = new EventEmitter(), c = createQualificationCancellation(true, messages);
    let cleaned = 0, closed = false;
    await c.ownContext(Promise.resolve({ close: async () => {
      if (fails) throw new Error("synthetic-disposal-failure"); closed = true;
    } }));
    c.setRuntimeCleanup(async () => { assert.equal(closed, true); cleaned++; return { ok: true, results: [] }; });
    messages.emit("message", { schema: "elastos.browser.qualification-cancel/v1" });
    await c.drainCancellation();
    assert.equal(cleaned, fails ? 0 : 1);
    if (!fails) { assert.equal(c.evidence.runtime_cleanup.ok, true); c.dispatchObserved(); }
    const evidence = await c.stop();
    assert.equal(evidence.runtime_cleanup.ok, false);
    assert.equal(evidence.runtime_cleanup.error, fails ? "viewer_disposal_unconfirmed" : "late_dispatch_after_disposal");
  }
});
test("cancellation settles an existing allocation and closes its exact Runtime handle after viewer disposal", async () => {
  const requests = []; let closed = false;
  const owner = { origin: "http://localhost:61510", token: "fixture-private-token", instance: "fixture-instance" };
  const empty = { schema: "elastos.browser.session-capacity/v1", recoverable_page: null,
    active_sessions: 0, principal_sessions: 0, total_sessions: 0, launching_sessions: 0,
    engine_cleanup_obligations: 0, launch_reconciliation_obligations: 0 };
  const fetchImpl = async (url, options) => {
    requests.push({ url, method: options.method });
    assert.equal(options.headers["x-elastos-home-token"], owner.token);
    if (url.includes("/open/")) return Response.json({ status: "completed" });
    if (url.includes("/summary?")) {
      assert.ok(url.endsWith("browser_instance=fixture-instance"));
      return Response.json({ sessions: { ...empty, ...(closed ? {} : { active_sessions: 1,
        recoverable_page: { schema: "elastos.browser.recoverable-page/v1", page_id: "page-own", cleanup: { id: "cleanup-own" } } }) } });
    }
    assert.equal(new URL(url).pathname, "/api/apps/browser/pages/page-own/close");
    assert.deepEqual(JSON.parse(options.body), { schema: "elastos.browser.close-request/v2", cleanup_id: "cleanup-own", browser_instance: owner.instance });
    closed = true;
    return Response.json({ schema: "elastos.browser.close-result/v1", closed: true, page_id: "page-own", cleanup_id: "cleanup-own",
      terminal_effects: Object.fromEntries(TERMINAL_EFFECTS.map(k => [k, true])) });
  };
  const result = await cleanupQualificationOpens([{ owner, open_id: "open-own", outcome: "pending" }], { fetchImpl, timeoutMs: 100, pollMs: 1 });
  assert.equal(result.ok, true); assert.equal(result.results.length, 1);
  assert.ok(requests.filter(r => r.method === "POST").every(r => r.url.endsWith("/close")));
  assert.ok(!JSON.stringify(result).includes(owner.token));
  const unknown = await cleanupQualificationOpens([{ owner, outcome: "pending" }], {
    fetchImpl: async () => Response.json({ sessions: empty }), timeoutMs: 5, pollMs: 1 });
  assert.equal(unknown.ok, false); assert.match(unknown.error, /deadline/);
  const held = await cleanupQualificationOpens([{ owner, outcome: "completed" }], {
    fetchImpl: () => new Promise(() => {}), timeoutMs: 5 });
  assert.equal(held.ok, false); assert.match(held.error, /deadline/);
});
test("sustained input confirms wheel movement and labels receipt time separately from visible response", async () => {
  const run = "fixture-input-run", expected = "Browser-tesA";
  const execute = async delivered => {
    const events = [{ sequence: 1, page: "nav", type: "scroll", scroll_y: 640 }];
    return qualificationInteraction({ index: 0, pageId: "page-fixture", run, text: "Browser-test",
      readReceipt: async () => ({ run, events }), signal: new AbortController().signal, timeoutMs: delivered ? 1000 : 10,
      key: async method => { if (method === "pressSequentially") events.push({ sequence: 2, type: "input", page: "nav", value: expected }); },
      wheel: async delta => { if (delivered) events.push({ sequence: 3, type: "scroll", page: "nav", value: expected, scroll_y: 640 + delta }); },
    });
  };
  const actual = await execute(true);
  assert.equal(actual.input.sequence, 2); assert.equal(actual.scroll.sequence, 3);
  assert.equal(actual.visible_response, null); assert.equal(actual.pending, "input_to_visible_latency_measurement");
  await assert.rejects(execute(false), /deadline/);
});
test("receiver bootstrap parses as a self-contained browser program", () => {
  new vm.Script("(" + installQualificationReceiver.toString() + ")()");
});

function receiverFixture(options = {}) {
  let clock = 0, moduleSource = "", Meter, meter, node, disconnected = 0, closed = 0, stopped = 0;
  const callbacks = new Map(); let serial = 0, frames = 0, dropped = 0;
  const audioTrack = { id: "audio", readyState: "live", stop() { stopped++; } };
  const videoTrack = { id: "video", readyState: "live", stop() { stopped++; } };
  const audioStream = { getAudioTracks: () => [audioTrack] };
  const videoStream = { getVideoTracks: () => [videoTrack] };
  const video = { videoWidth: 1920, videoHeight: 1080, paused: false, srcObject: videoStream,
    requestVideoFrameCallback(fn) { const id = ++serial; callbacks.set(id, fn); return id; },
    cancelVideoFrameCallback(id) { callbacks.delete(id); },
    getVideoPlaybackQuality() { return { totalVideoFrames: frames, droppedVideoFrames: dropped }; } };
  const window = { Audio: class { srcObject = audioStream; muted = false; paused = false; },
    __elastosBrowserCurrentPageId: "page-fixture" };
  const document = { hidden: false, querySelector: selector => ({
    "#browser-remote-display": video, "#browser-url": { disabled: false },
    "#browser-engine": { value: "local" }, "#browser-exit": { value: "runtime-default" },
  })[selector] };
  class FixtureURL extends URL {
    static createObjectURL(blob) { moduleSource = blob.source; return "blob:test"; }
    static revokeObjectURL() {}
  }
  class Context {
    state = "running";
    destination = {};
    audioWorklet = { addModule: async () => {
      await options.moduleGate;
      vm.runInNewContext(moduleSource, {
        AudioWorkletProcessor: class {
          port = { onmessage: null, postMessage(value) { node.port.onmessage?.({ data: value }); } };
        },
        registerProcessor(name, ctor) { assert.equal(name, "qualification-meter"); Meter = ctor; },
        sampleRate: 48000,
      });
    } };
    async resume() {}
    createMediaStreamSource(stream) {
      assert.equal(stream, audioStream);
      return { connect(to) { return to; }, disconnect() { disconnected++; } };
    }
    async close() { this.state = "closed"; closed++; }
  }
  class WorkletNode {
    constructor() {
      node = this; meter = new Meter();
      this.port = { onmessage: null, postMessage(data) { meter.port.onmessage({ data }); }, close() {} };
    }
    connect() { return this; }
    disconnect() { disconnected++; }
  }
  let initial;
  vm.runInNewContext("(" + installQualificationReceiver.toString() + ")()", {
    window, document, location: { href: "http://localhost/apps/browser/?browser_instance=test" },
    URL: FixtureURL, Blob: class { constructor(parts) { this.source = parts.join(""); } },
    AudioContext: Context, AudioWorkletNode: WorkletNode,
    performance: { now: () => clock, timeOrigin: 1 }, setTimeout, clearTimeout,
    setInterval(fn) { initial = fn; return 1; }, clearInterval() {},
  });
  const audio = new window.Audio();
  let sampleIndex = 0;
  const render = (blocks, silence = false, invalid = false) => {
    for (let block = 0; block < blocks; block++) {
      const data = Float32Array.from({ length: 128 }, () =>
        silence ? 0 : 0.05 * Math.sin(2 * Math.PI * 440 * sampleIndex++ / 48000));
      if (invalid) data[0] = NaN;
      meter.process([[data]]);
      clock += 128 / 48;
      if (block % 16 === 0) {
        frames++; const pending = [...callbacks]; callbacks.clear();
        for (const [, fn] of pending) fn(clock, { presentedFrames: frames });
      }
    }
  };
  return { window, document, audio, video, render, initial: () => initial(),
    counts: () => ({ disconnected, closed, stopped }), addDrops(n) { dropped += n; frames += n; } };
}
test("actual worklet counts decoded tone and flushes terminal silence before snapshot", async () => {
  const f = receiverFixture(); f.initial();
  try {
    await f.window.__browserQualification.start();
    f.render(750);
    let s = await f.window.__browserQualification.read();
    assert.equal(s.receiver_unchanged, true);
    assert.ok(Math.abs(s.audio.tone_hz - 440) < 2);
    assert.equal(s.audio.max_silence_ms, 0);
    assert.ok(s.audio.observed_ms >= 2000);
    f.render(40, true); // 106.7 ms at the end, shorter than two periodic meter messages.
    s = await f.window.__browserQualification.read();
    assert.ok(s.audio.max_silence_ms >= 100, "final snapshot must include the unreported silent tail");
    assert.ok(s.audio.resolution_ms > 0 && s.audio.resolution_ms < 6);
  } finally {
    const result = await f.window.__browserQualification.stop();
    assert.equal(result.probe_closed, true); assert.equal(result.observer_stopped, true);
    assert.deepEqual(f.counts(), { disconnected: 2, closed: 1, stopped: 0 });
  }
});
test("receiver replacements, mute and invalid PCM remain observable; hidden drops stay separate", async () => {
  const f = receiverFixture();
  await f.window.__browserQualification.start();
  try {
    f.render(64);
    f.audio.muted = true;
    assert.equal((await f.window.__browserQualification.read()).receiver_unchanged, false);
    f.audio.muted = false;
    const original = f.video.srcObject; f.video.srcObject = {};
    assert.equal((await f.window.__browserQualification.read()).receiver_unchanged, false);
    f.video.srcObject = original;
    f.window.__browserQualification.phase("hidden"); f.document.hidden = true;
    f.addDrops(100); f.render(32, true);
    f.window.__browserQualification.phase("active"); f.document.hidden = false;
    f.render(32, false, true);
    const value = await f.window.__browserQualification.read();
    assert.equal(value.video.dropped, 0);
    assert.ok(value.audio.invalid_samples > 0);
  } finally { await f.window.__browserQualification.stop(); }
});
test("late worklet setup cannot acquire probe effects after cancellation cleanup", async () => {
  let ready;
  const f = receiverFixture({ moduleGate: new Promise(resolve => { ready = resolve; }) });
  const pending = f.window.__browserQualification.start();
  await Promise.resolve(); await Promise.resolve();
  const closed = await f.window.__browserQualification.stop();
  assert.equal(closed.probe_closed, true);
  ready(); await assert.rejects(pending, /probe_stopped/);
  await f.window.__browserQualification.stop();
  assert.deepEqual(f.counts(), { disconnected: 0, closed: 1, stopped: 0 });
});
test("sustained summary rejects a shortened run, sparse sampling, missing cleanup and media budget failures", () => {
  const p = { ...plan("media"), probe: true, duration_ms: 10_000 };
  const a = attempt(1, "media"), q = a.journey.controlled_journey.qualification;
  q.observation = { ok: true, duration_ms: 10_000, active_ms: 10_000, interaction_ms: 10_000,
    samples: 2, interactions: 1, max_sample_gap_ms: 5000,
    cleanup: { probe_closed: true, observer_stopped: true },
    video: { fps: 30, width: 1920, height: 1080, total: 300, dropped: 0, drop_ratio: 0, max_frame_gap_ms: 40 },
    audio: { observed_ms: 10_000, max_silence_ms: 0, receiver_unchanged: true } };
  validateAttempt(a, p);
  for (const change of [o => { o.duration_ms = 9999; }, o => { o.samples = 1; },
    o => { o.max_sample_gap_ms = 10001; }, o => { o.cleanup.probe_closed = false; },
    o => { o.video.max_frame_gap_ms = 250; }, o => { o.video.drop_ratio = 0.01; },
    o => { o.video.fps = 28; }, o => { o.audio.max_silence_ms = 100; },
    o => { o.audio.observed_ms = 5000; }]) {
    const copy = structuredClone(a); change(copy.journey.controlled_journey.qualification.observation);
    assert.throws(() => validateAttempt(copy, p));
  }
});
test("raw thirty-minute media and interaction trace drives its summary and retains the two unmeasured gates", async () => {
  const dir = await mkdtemp(join(tmpdir(), "browser-qualification-media-test-"));
  try {
    const p = plan("media"), a = attempt(1, "media"); let sequence = 0, scrollY = 640;
    const trace = Array.from({ length: 361 }, (_, i) => {
      const duration = i === 0 ? 100 : i * 5000;
      const value = { schema: "elastos.browser.qualification-sample/v1", page_id: a.journey.page_id,
        run: a.journey.controlled_journey.run, initial_cursor: 0, events: [],
        clock_origin: { started_ms: 0, ready_ms: 0 }, snapshot_window: { started_ms: duration - 50, observed_ms: duration },
        action_window: { started_ms: i === 0 ? 0 : i === 1 ? 100 : (i - 1) * 5000, observed_ms: duration }, snapshot: {
          page_id: a.journey.page_id, document_id: 1, engine_id: "local", exit_id: "runtime-default",
          receiver_unchanged: true, phase: "active", phase_started_ms: 0, duration_ms: duration, active_ms: duration, interaction_ms: duration,
          video: { frames: duration * 0.03, total: duration * 0.03, dropped: 0, max_frame_gap_ms: 40, width: 1920, height: 1080 },
          audio: { observed_ms: duration, max_silence_ms: 0, invalid_samples: 0, tone_hz: 440, receiver_unchanged: true } } };
      if (i % 12 === 0 && i < 360) {
        const index = i / 12, delta = index % 2 ? 320 : -320, expected = "Browser-test" + index;
        const inputSequence = ++sequence, scrollSequence = ++sequence;
        value.events = [{ sequence: inputSequence, page: "nav", type: "input", value: expected },
          { sequence: scrollSequence, page: "nav", type: "scroll", value: expected, scroll_y: scrollY + delta }];
        value.interaction = { index, page_id: a.journey.page_id, run: value.run, expected,
          input: { sequence: inputSequence, started_ms: duration - 100, receipt_ms: duration - 99 },
          scroll: { sequence: scrollSequence, before_y: scrollY, delta, started_ms: duration - 99, receipt_ms: duration - 98 } };
        scrollY += delta;
      }
      value.fixture_cursor = sequence;
      return { sequence: i + 2, at_ms: duration, attempt: 1, kind: "media", value };
    });
    const binding = { page_id: a.journey.page_id, run: a.journey.controlled_journey.run, engine_id: "local", exit_id: "runtime-default" };
    const derived = deriveMedia(trace.map(r => r.value), binding);
    assert.equal(derived.video.fps, 30); assert.equal(derived.audio.observed_ms, 1_800_000); assert.equal(derived.interactions, 30);
    a.journey.controlled_journey.qualification.observation = { ...derived, ok: true,
      cleanup: { probe_closed: true, observer_stopped: true } };
    const planBytes = JSON.stringify(p); writeFileSync(join(dir, "plan.json"), planBytes);
    a.log = { file: "attempt-1.log", sha256: sha256("synthetic source test") };
    writeFileSync(join(dir, a.log.file), "synthetic source test");
    const path = join(dir, "qualification.json");
    const save = rows => {
      const attemptBytes = JSON.stringify(a); writeFileSync(join(dir, "attempt-1.json"), attemptBytes);
      const samples = [JSON.stringify({ sequence: 1, at_ms: 0, attempt: 1, kind: "resources",
        value: { identity_verified: true, rss_bytes: 128, cpu_percent: 10 } }), ...rows.map(r => JSON.stringify(r))].join("\n") + "\n";
      writeFileSync(join(dir, "samples.jsonl"), samples);
      const r = { schema: "elastos.browser.qualification/v1", plan: p, plan_sha256: sha256(planBytes),
        completed: true, candidate_unchanged: true, cancelled: false,
        attempts: [{ file: "attempt-1.json", sha256: sha256(attemptBytes) }],
        samples: { file: "samples.jsonl", bytes: Buffer.byteLength(samples), records: rows.length + 1, sha256: sha256(samples) } };
      writeFileSync(path, JSON.stringify(sealBundle(dir, r)));
    };
    save(trace); const result = auditQualification(path);
    assert.equal(result.completed_execution, true); assert.equal(result.ok, false);
    assert.deepEqual(result.pending, ["synchronized_receiver_av_offset_measurement", "input_to_visible_latency_measurement"]);
    for (const change of [s => { s.duration_ms += 10001; }, s => { s.document_id = 2; },
      s => { s.engine_id = "another"; }, s => { s.audio.observed_ms = 0; },
      s => { s.audio.max_silence_ms = 100; }, s => { s.video.total *= 2; }]) {
      const changed = structuredClone(trace); change(changed[100].value.snapshot); save(changed);
      assert.throws(() => auditQualification(path));
    }
    const zero = structuredClone(trace);
    for (const r of zero) { r.value.snapshot.video.frames = 0; r.value.snapshot.audio.observed_ms = 0; r.value.events = []; }
    save(zero); assert.throws(() => auditQualification(path), /fixture|media/);
    const missingInput = structuredClone(trace); delete missingInput[120].value.interaction;
    save(missingInput); assert.throws(() => auditQualification(path), /interaction/);
    const falseWheel = structuredClone(trace); falseWheel[120].value.events[1].scroll_y = falseWheel[120].value.interaction.scroll.before_y;
    save(falseWheel); assert.throws(() => auditQualification(path), /interaction/);
    const replayed = structuredClone(trace);
    for (const [i, { value }] of replayed.entries()) {
      value.events = i ? [] : value.events;
      value.fixture_cursor = 2;
      if (value.interaction) value.interaction = { ...structuredClone(trace[0].value.interaction), index: value.interaction.index };
    }
    save(replayed); assert.throws(() => auditQualification(path), /sustained_interaction_unconfirmed/);
    const staleTimes = structuredClone(trace);
    Object.assign(staleTimes[120].value.interaction.input, { started_ms: 0, receipt_ms: 1 });
    Object.assign(staleTimes[120].value.interaction.scroll, { started_ms: 1, receipt_ms: 2 });
    save(staleTimes); assert.throws(() => auditQualification(path), /sustained_interaction_unconfirmed/);
    const futureWitness = structuredClone(trace);
    futureWitness[120].value.interaction.scroll.receipt_ms = futureWitness[120].value.action_window.observed_ms + 1;
    save(futureWitness); assert.throws(() => auditQualification(path), /sustained_interaction_unconfirmed/);
    const capped = structuredClone(replayed);
    for (const [i, { value }] of capped.entries()) {
      const s = value.snapshot, active = Math.min(s.duration_ms, 1000);
      s.active_ms = s.interaction_ms = s.audio.observed_ms = active;
      s.video.frames = s.video.total = active * 0.03;
      if (i) delete value.interaction;
    }
    save(capped); assert.throws(() => auditQualification(path), /media_phase_coverage/);
    const stalled = structuredClone(trace);
    stalled[100].value.snapshot.active_ms = stalled[100].value.snapshot.interaction_ms = stalled[99].value.snapshot.active_ms;
    save(stalled); assert.throws(() => auditQualification(path), /media_phase_coverage/);
    const compressed = structuredClone(trace);
    for (const [i, { value }] of compressed.entries()) {
      const end = 100 + i * 10;
      value.action_window = { started_ms: i ? end - 10 : 0, observed_ms: end };
      value.snapshot_window = { started_ms: end - 4, observed_ms: end };
      if (value.interaction) {
        Object.assign(value.interaction.input, { started_ms: end - 9, receipt_ms: end - 8 });
        Object.assign(value.interaction.scroll, { started_ms: end - 8, receipt_ms: end - 7 });
      }
    }
    assert.equal(compressed.at(-1).value.action_window.observed_ms, 3700);
    save(compressed); assert.throws(() => auditQualification(path), /media_observation_clock/);
    const compressedParent = structuredClone(trace);
    compressedParent.forEach((row, i) => { row.at_ms = 100 + i * 10; });
    save(compressedParent); assert.throws(() => auditQualification(path), /media_parent_clock/);
    const distributed = targets => {
      const out = structuredClone(trace), witnesses = trace.filter(r => r.value.interaction); let cursor = 0;
      for (const [i, { value }] of out.entries()) {
        delete value.interaction; value.events = [];
        const actionIndex = targets.indexOf(i);
        if (actionIndex !== -1) {
          const witness = structuredClone(witnesses[actionIndex].value), end = value.action_window.observed_ms;
          value.events = witness.events; value.interaction = witness.interaction;
          Object.assign(value.interaction.input, { started_ms: end - 100, receipt_ms: end - 99 });
          Object.assign(value.interaction.scroll, { started_ms: end - 99, receipt_ms: end - 98 });
          cursor += 2;
        }
        value.fixture_cursor = cursor;
      }
      return out;
    };
    const regular = Array.from({ length: 30 }, (_, i) => i * 12);
    for (const targets of [Array.from({ length: 30 }, (_, i) => i), // All 30 actions in the first 145 seconds.
      regular.map((v, i) => i === 0 ? 3 : v), // Missed first active interval.
      regular.map((v, i) => i === 10 ? 126 : v), // A 90-second middle gap.
      regular.map((v, i) => i === 29 ? 337 : v)]) { // Last action leaves 115 seconds uncovered.
      const changed = distributed(targets); save(changed);
      assert.throws(() => auditQualification(path), /sustained_interaction_cadence/);
    }
    const frozenFrames = structuredClone(trace);
    for (const { value } of frozenFrames) {
      const s = value.snapshot, t = s.duration_ms;
      if (t >= 900_000 && t <= 960_000) s.video.frames = s.video.total = 27_000;
      else if (t > 960_000 && t <= 1_020_000) s.video.frames = s.video.total = 27_000 + (t - 960_000) * 0.06;
    }
    assert.equal(frozenFrames.at(-1).value.snapshot.video.frames / 1800, 30);
    save(frozenFrames); assert.throws(() => auditQualification(path), /media_frame_progress_gap/);
    const sparseFrames = structuredClone(trace);
    sparseFrames[181].value.snapshot.video.frames = sparseFrames[180].value.snapshot.video.frames + 1;
    save(sparseFrames); assert.throws(() => auditQualification(path), /media_frame_progress_gap/);
    // Different host origin and bounded RPC/IPC delay still describe the same media timeline.
    const overhead = structuredClone(trace);
    for (const [i, row] of overhead.entries()) {
      const v = row.value;
      v.clock_origin = { started_ms: 970, ready_ms: 1000 };
      v.action_window.started_ms += 1000; v.action_window.observed_ms += 1000;
      v.snapshot_window.started_ms += 1000; v.snapshot_window.observed_ms += 1000;
      for (const action of v.interaction ? [v.interaction.input, v.interaction.scroll] : []) {
        action.started_ms += 1000; action.receipt_ms += 1000;
      }
      row.at_ms += i % 2 ? 2000 : 0;
    }
    save(overhead); assert.equal(auditQualification(path).completed_execution, true);
    const wrongOrigin = structuredClone(trace); wrongOrigin[100].value.clock_origin.ready_ms = 1;
    save(wrongOrigin); assert.throws(() => auditQualification(path), /media_observation_clock/);
    // A complete mixed cycle derives 270 seconds of media and 240 of interaction.
    const mixed = structuredClone(trace.slice(0, 61)).map(r => r.value);
    for (const row of mixed) {
      const s = row.snapshot, t = s.duration_ms;
      s.phase = t >= 270_000 ? "hidden" : t >= 240_000 ? "idle" : "active";
      s.phase_started_ms = t >= 270_000 ? 270_000 : t >= 240_000 ? 240_000 : 0;
      s.hidden = s.phase === "hidden";
      s.active_ms = s.audio.observed_ms = Math.min(t, 270_000);
      s.interaction_ms = Math.min(t, 240_000); s.video.frames = s.video.total = s.active_ms * 0.03;
      if (t >= 240_000) { delete row.interaction; row.events = []; row.fixture_cursor = 8; }
    }
    const mixedDerived = deriveMedia(mixed, { ...binding, mode: "mixed" });
    assert.equal(mixedDerived.active_ms, 270_000); assert.equal(mixedDerived.interaction_ms, 240_000);
    assert.equal(mixedDerived.hidden_cycles, 1); assert.equal(mixedDerived.idle_cycles, 1);
    assert.throws(() => deriveMedia(mixed, binding), /media_phase_coverage/);
    const missingPhase = structuredClone(mixed); missingPhase[48].snapshot.phase = "hidden";
    missingPhase[48].snapshot.hidden = true;
    assert.throws(() => deriveMedia(missingPhase, { ...binding, mode: "mixed" }), /media_phase_transition/);
    const shifted = structuredClone(mixed);
    for (const row of shifted) {
      const s = row.snapshot;
      if (s.phase === "idle") {
        s.phase_started_ms += 200; s.duration_ms += 250;
        row.snapshot_window.started_ms += 250; row.snapshot_window.observed_ms += 250;
        row.action_window.observed_ms += 250;
        s.active_ms = s.audio.observed_ms = s.duration_ms; s.interaction_ms = 240_200;
        s.video.frames = s.video.total = s.active_ms * 0.03;
      }
      if (s.phase === "hidden") s.interaction_ms = 240_200;
    }
    shifted.forEach((row, i) => { if (i) row.action_window.started_ms = shifted[i - 1].action_window.observed_ms; });
    assert.equal(deriveMedia(shifted, { ...binding, mode: "mixed" }).interaction_ms, 240_200);
    shifted[49].snapshot.active_ms -= 100;
    assert.throws(() => deriveMedia(shifted, { ...binding, mode: "mixed" }), /media_phase_coverage/);
    const resumed = structuredClone(trace.slice(0, 79)).map(r => r.value); let cursor = 0, actionIndex = 0;
    for (const [i, row] of resumed.entries()) {
      const s = row.snapshot;
      if (i === 60) {
        s.duration_ms += 250; row.snapshot_window.started_ms += 250; row.snapshot_window.observed_ms += 250;
        row.action_window.observed_ms += 250;
        for (const action of [row.interaction.input, row.interaction.scroll]) { action.started_ms += 250; action.receipt_ms += 250; }
      }
      const t = s.duration_ms;
      s.phase = t >= 300_000 ? "active" : t >= 270_000 ? "hidden" : t >= 240_000 ? "idle" : "active";
      s.phase_started_ms = t >= 300_000 ? 300_000 : t >= 270_000 ? 270_000 : t >= 240_000 ? 240_000 : 0;
      s.hidden = s.phase === "hidden";
      s.active_ms = s.audio.observed_ms = t >= 300_000 ? t - 30_000 : Math.min(t, 270_000);
      s.interaction_ms = t >= 300_000 ? t - 60_000 : Math.min(t, 240_000);
      s.video.frames = s.video.total = s.active_ms * 0.03;
      if (s.phase !== "active") { delete row.interaction; row.events = []; }
      if (row.interaction) {
        row.interaction.index = actionIndex++;
        row.interaction.input.sequence = row.events[0].sequence = ++cursor;
        row.interaction.scroll.sequence = row.events[1].sequence = ++cursor;
      }
      row.fixture_cursor = cursor;
      if (i) row.action_window.started_ms = resumed[i - 1].action_window.observed_ms;
    }
    assert.equal(deriveMedia(resumed, { ...binding, mode: "mixed" }).interactions, 6);
    const lateResume = structuredClone(resumed), moved = lateResume[60].interaction;
    const movedEvents = lateResume[60].events;
    delete lateResume[60].interaction;
    for (let i = 60; i < 64; i++) { lateResume[i].events = []; lateResume[i].fixture_cursor = 8; }
    lateResume[64].interaction = moved; lateResume[64].events = movedEvents;
    Object.assign(moved.input, { started_ms: 319900, receipt_ms: 319901 });
    Object.assign(moved.scroll, { started_ms: 319901, receipt_ms: 319902 });
    assert.throws(() => deriveMedia(lateResume, { ...binding, mode: "mixed" }), /sustained_interaction_cadence/);
  } finally { await rm(dir, { recursive: true, force: true }); }
});
