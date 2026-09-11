import assert from "node:assert/strict";
import { test as nodeTest } from "node:test";
import { readFileSync } from "node:fs";

const test = (name, callback) => nodeTest(name, { timeout: 5000 }, callback);

// Import the production controller with its browser-only absolute import mapped
// to the same local module used by the existing Assistant controller fixture.
const asset = path => new URL(`../capsules/${path}`, import.meta.url).href;
const source = readFileSync(new URL("../capsules/assistant/browser/assistant.js", import.meta.url), "utf8")
  .replace('"/apps/home/home-clipboard-client.js?v=home-20260726a"', JSON.stringify(asset("home/browser/home-clipboard-client.js")))
  .replace('"./vendor/katex/katex.mjs"', JSON.stringify(asset("assistant/browser/vendor/katex/katex.mjs")))
  .replace('"./model-selection.js"', JSON.stringify(asset("_shared/model-selection.js")));
const { createAssistantApp } = await import(`data:text/javascript,${encodeURIComponent(source)}`);
const clone = value => structuredClone(value);
const runId = `run:sha256:${"a".repeat(64)}`;
const output = { schema: "elastos.model.output.content/v1", resource_id: "elastos://content/studio-fixture" };
const offer = { id: "studio-fixture", title: "Fixture video", operation: "video.generate",
  input_modalities: ["application/json"], output_modalities: ["application/json"] };
const response = data => ({ ok: true, status: 200, json: async () => clone(data) });
const completed = () => response({ status: "ok", data: {
  schema: "elastos.model.run/v1", run_id: runId, status: "completed", sequence_cursor: 1,
  terminal: { output },
} });
const events = (cursor = 8) => response({ status: "ok", data: {
  schema: "elastos.model.run-events/v1", run_id: runId, next_cursor: cursor, has_more: false,
  events: [{ sequence: cursor, kind: "output", terminal: true, data: output }],
} });
const defer = () => {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
};
const flush = () => new Promise(setImmediate);

async function fixture(t, { studioState = {}, beforeCreate = async () => true,
  create = async () => completed(), readEvents = async () => events() } = {}) {
  const calls = [], savedBeforeCreate = [], snapshots = [], timers = new Map();
  let ordinal = 0, timerId = 0;
  const app = createAssistantApp({
    studioOnly: true, studioSessionId: "studio-session", studioState,
    homeToken: "fixture-token", homeOrigin: "https://home.example",
    cryptoRef: { randomUUID: () => `fixture-request-${++ordinal}` },
    nowFn: () => 1000,
    setTimeoutFn: callback => { const id = ++timerId; timers.set(id, callback); return id; },
    clearTimeoutFn: id => timers.delete(id),
    homeClipboardClientFactory: () => ({ start() {}, stop() {} }),
    onStateChange: view => snapshots.push(clone(view)),
    beforeRunCreate: async pending => {
      // This is the host's persistence seam. The complete pending identity must
      // be observable here before the HTTP side effect is allowed to begin.
      savedBeforeCreate.push({ pending: clone(pending), view: clone(app.snapshot()) });
      return beforeCreate(pending);
    },
    fetchFn: async (url, init = {}) => {
      assert.equal(init.headers["x-elastos-home-token"], "fixture-token");
      const body = init.body ? JSON.parse(init.body) : null;
      calls.push({ url, body });
      if (url === "/api/provider/model/offers_list") return response({ offers: [offer] });
      if (url === "/api/capsules/catalog") return response({ capsules: [] });
      if (url === "/api/provider/model/runs_create") {
        assert.ok(savedBeforeCreate.length, "create requires a prior persistence callback");
        assert.equal(body.request_id, savedBeforeCreate.at(-1).pending.createRequestId);
        assert.equal(savedBeforeCreate.at(-1).view.activeRun.createRequestId, body.request_id);
        return create(body);
      }
      if (url === "/api/provider/model/runs_events") return readEvents(body);
      throw new Error(`Unexpected request: ${url}`);
    },
  });
  t.after(() => app.dispose());
  await app.initialize();
  return { app, calls, savedBeforeCreate, snapshots, timers,
    operations: () => calls.filter(call => call.url.includes("/runs_")) };
}

for (const failure of ["refused", "throws"]) {
  test(`Studio persistence ${failure} prevents create and retains draft/request identity`, async t => {
    const prompt = "Keep this draft 世界 🧭";
    const f = await fixture(t, { studioState: { studioDraft: prompt }, beforeCreate: async () => {
      if (failure === "throws") throw new Error("workspace save failed");
      return false;
    } });
    assert.equal(await f.app.sendDraft(), false);
    assert.deepEqual(f.operations(), []);
    assert.equal(f.app.snapshot().studioDraft, prompt);
    const pending = f.savedBeforeCreate[0].pending;
    assert.equal(pending.prompt, prompt);
    assert.equal(pending.draft, prompt);
    assert.equal(pending.sessionId, "studio-session");
    assert.equal(pending.actorCapsule, "assistant");
    assert.equal(f.app.snapshot().activeRun.createRequestId, pending.createRequestId);
    assert.equal(f.app.snapshot().activeRun.status, "not_sent");
    assert.equal(f.app.snapshot().studioHistory[0].createRequestId, pending.createRequestId);
  });
}

test("Studio double-submit during a delayed create makes one request", async t => {
  const pending = defer(), started = defer();
  const f = await fixture(t, { studioState: { studioDraft: "One deliberate video" },
    create: body => { started.resolve(body); return pending.promise; } });
  const first = f.app.sendDraft();
  await started.promise;
  assert.equal(await f.app.sendDraft(), false);
  assert.equal(f.savedBeforeCreate.length, 1);
  assert.equal(f.operations().length, 1);
  assert.equal(f.app.snapshot().activeRun.status, "submitting");
  pending.resolve(completed());
  assert.equal(await first, true);
  assert.deepEqual(f.operations().map(call => call.url), ["/api/provider/model/runs_create"]);
});

test("lost Studio create response preserves its request across reload without redispatch", async t => {
  const prompt = "Uncertain submission remains editable 🌍";
  const f = await fixture(t, { studioState: { studioDraft: prompt }, create: async () => {
    throw new TypeError("response lost after dispatch");
  } });
  assert.equal(await f.app.sendDraft(), false);
  const saved = clone(f.app.snapshot());
  assert.equal(saved.activeRun.status, "settlement_unknown");
  assert.equal(saved.activeRun.terminal, false);
  assert.equal(saved.activeRun.runId, "");
  assert.equal(saved.activeRun.createRequestId, f.operations()[0].body.request_id);
  assert.equal(saved.studioDraft, prompt);
  assert.equal(saved.studioHistory[0].createRequestId, saved.activeRun.createRequestId);
  assert.equal(await f.app.sendDraft(), false);
  await f.app.resumeRun();
  assert.equal(f.operations().length, 1);
  const restored = await fixture(t, { studioState: saved });
  await flush();
  await restored.app.resumeRun();
  assert.equal(await restored.app.sendDraft(), false);
  assert.equal(await restored.app.stopRun(), false);
  assert.deepEqual(restored.operations(), []);
  assert.equal(restored.app.snapshot().activeRun.createRequestId, saved.activeRun.createRequestId);
  assert.equal(restored.app.snapshot().studioDraft, prompt);
});

const restoredRun = {
  runId, createRequestId: "original-request", sessionId: "studio-session", actorCapsule: "assistant",
  mode: "studio", offerId: offer.id, operation: offer.operation, afterSequence: 7,
  terminal: false, status: "running", prompt: "Original prompt", draft: "Original prompt",
  outputText: "", pollErrorCount: 0,
};

test("restored same-actor Studio run reads its exact cursor without another create", async t => {
  const f = await fixture(t, { studioState: { activeRun: restoredRun } });
  await flush();
  assert.deepEqual(f.operations().map(call => call.url), ["/api/provider/model/runs_events"]);
  assert.equal(f.operations()[0].body.run_id, runId);
  assert.equal(f.operations()[0].body.after_sequence, 7);
  assert.equal(f.app.snapshot().activeRun.afterSequence, 8);
  assert.equal(f.app.snapshot().activeRun.createRequestId, "original-request");
  assert.equal(f.app.snapshot().activeRun.status, "completed");
  assert.deepEqual(f.app.snapshot().studioHistory[0].output, output);
});

for (const [label, binding] of [["foreign actor", { actorCapsule: "home-agent" }],
  ["history copy", { attachmentAllowed: false }]]) {
  test(`Studio ${label} retains history without events or cancellation`, async t => {
    const original = { ...restoredRun, ...binding };
    const f = await fixture(t, { studioState: { activeRun: original } });
    await flush();
    await f.app.resumeRun();
    assert.equal(await f.app.stopRun(), false);
    assert.equal(await f.app.sendDraft(), false);
    assert.deepEqual(f.operations(), []);
    const saved = f.app.snapshot().activeRun;
    assert.equal(saved.runId, original.runId);
    assert.equal(saved.createRequestId, original.createRequestId);
    assert.equal(saved.actorCapsule, original.actorCapsule);
    assert.equal(saved.afterSequence, 7);
    assert.equal(saved.status, "settlement_unknown");
  });
}

test("Studio preserves the complete Unicode prompt and previous output history", async t => {
  const prompt = `Full prompt ${"世界🙂 café ".repeat(7000)} END`;
  assert.ok(Buffer.byteLength(prompt) > 40000);
  const history = [{ runId: "previous-run", createRequestId: "previous-request", terminal: true,
    prompt, output: { ...output, resource_id: "elastos://content/previous-output" }, futureMetadata: { retained: true } }];
  const f = await fixture(t, { studioState: { studioHistory: history } });
  f.app.setDraft(prompt);
  assert.equal(f.app.snapshot().studioDraft, prompt);
  assert.equal(await f.app.sendDraft(), true);
  assert.equal(f.operations()[0].body.input.prompt, prompt);
  assert.equal(f.operations()[0].body.input.schema, "elastos.model.input.video/v1");
  assert.equal(f.savedBeforeCreate[0].pending.prompt, prompt);
  assert.equal(f.savedBeforeCreate[0].view.studioDraft, prompt);
  const saved = f.app.snapshot();
  assert.deepEqual(saved.studioHistory[0], history[0]);
  assert.equal(saved.studioHistory.length, 2);
  assert.equal(saved.studioHistory[1].prompt, prompt);
  assert.equal(saved.studioHistory[1].draft, prompt);
  assert.deepEqual(saved.studioHistory[1].output, output);
  assert.equal(saved.studioResult.resourceId, output.resource_id);
  const restored = await fixture(t, { studioState: clone(saved) });
  await flush();
  assert.deepEqual(restored.app.snapshot().studioHistory, saved.studioHistory);
  assert.deepEqual(restored.operations(), []);
});
