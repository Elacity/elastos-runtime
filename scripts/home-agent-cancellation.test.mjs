import assert from "node:assert/strict";
import test from "node:test";

globalThis.window = { location: { href: "http://home.invalid/apps/home-agent/", hash: "#home_token=fixture" } };
const live = await import("../capsules/home-agent/browser/agent-live.js");
const { turnStoreGet } = await import("../capsules/home-agent/browser/agent-context.js");
const { recoverStalePersistedTurn } = await import("../capsules/home-agent/browser/agent-stream-qos.js");
const controller = await import("../capsules/home-agent/browser/agent-stream.js");
const workspace = await import("../capsules/home-agent/browser/agent-workspace.js");
const sessions = await import("../capsules/home-agent/browser/agent-sessions.js");
const defer = () => { let resolve; const promise = new Promise((r) => { resolve = r; }); return { promise, resolve }; };
const terminal = (kind) => ({ events: [{ sequence: 1, kind, terminal: true, data: kind === "output" ? { schema: "elastos.model.output.text/v1", text: "Done" } : {} }], next_cursor: 1, has_more: false });

async function fixture(cancelReject = false, saved = null) {
  const events = defer();
  const accepted = defer();
  const calls = [];
  globalThis.fetch = async (url, init) => {
    const op = new URL(url).pathname.split("/").pop();
    const body = JSON.parse(init.body);
    calls.push({ op, body });
    let data;
    if (op === "offers_list") data = { offers: [{ id: "fixture", title: "Fixture", operation: "text", input_modalities: ["text/plain"], output_modalities: ["text/plain"] }] };
    else if (op === "runs_create") data = { run_id: "run-fixture", sequence_cursor: 0 };
    else if (op === "runs_get") data = { run_id: "run-fixture", status: "running" };
    else if (op === "runs_events") data = await events.promise;
    else if (op === "runs_cancel") {
      if (cancelReject) return { ok: false, status: 403, json: async () => ({ code: "denied" }) };
      data = { run_id: "run-fixture", status: "running" };
    } else throw new Error(`unexpected operation ${op}`);
    return { ok: true, json: async () => data };
  };
  await live.probeLiveInference({ force: true });
  const states = [];
  const result = live.streamChatViaContract([{ role: "user", content: "fixture" }], {
    turnManifest: saved,
    onAccepted: accepted.resolve,
    onState: (turn) => states.push(turn.state),
  });
  const { turnId } = await accepted.promise;
  return { events, result, calls, states, turnId };
}

test("cancel rejection retains run identity and a later completion wins", async () => {
  const f = await fixture(true);
  const ctx = { streamGeneration: 7, turnBusy: true };
  controller.bindAgentStream(ctx);
  try {
    await Promise.all([controller.abortAgentStreamNow(), controller.stopAgentStream()]);
    assert.equal(ctx.streamGeneration, 7);
    assert.equal(ctx.turnBusy, true);
    assert.equal(turnStoreGet(f.turnId).state, "settlement_unknown");
    assert.equal(turnStoreGet(f.turnId).providerRunId, "run-fixture");
    assert.equal(f.calls.filter((c) => c.op === "runs_cancel").length, 1);
    assert.ok(f.states.includes("cancel_pending"));
  } finally { f.events.resolve(terminal("output")); }
  const result = await f.result;
  assert.equal(result.turnManifest.state, "completed");
  assert.equal(result.aborted, false);
});

test("only confirmed Runtime cancellation becomes stopped", async () => {
  const f = await fixture();
  await controller.stopAgentStream();
  assert.equal(turnStoreGet(f.turnId).state, "cancel_pending");
  f.events.resolve(terminal("cancelled"));
  const result = await f.result;
  assert.equal(result.turnManifest.state, "stopped");
  assert.equal(result.aborted, true);
  assert.ok(result.turnManifest.completedAt > 0);
});

test("Runtime unknown settlement stays unknown rather than stopped", async () => {
  const f = await fixture();
  await live.abortLiveChatStream();
  f.events.resolve(terminal("settlement_unknown"));
  const result = await f.result;
  assert.equal(result.turnManifest.state, "settlement_unknown");
  assert.equal(result.turnManifest.providerRunId, "run-fixture");
  assert.ok(result.turnManifest.completedAt > 0);
  assert.equal(live.unresolvedModelTurn(result.turnManifest), false);
  assert.equal(recoverStalePersistedTurn({ lastTurn: result.turnManifest }).lastTurn.completedAt, result.turnManifest.completedAt);
});

test("navigation and reload retain the exact run without another create or cancel", async () => {
  const f = await fixture();
  live.detachLiveChatStream();
  f.events.resolve(terminal("output"));
  const detached = await f.result;
  const serialized = workspace.serializeSessionForPersist({ id: "session", messages: [], lastTurn: detached.turnManifest });
  const saved = recoverStalePersistedTurn(serialized).lastTurn;
  assert.notEqual(saved.state, "stopped");
  assert.ok(!saved.completedAt);
  assert.equal(f.calls.filter((c) => c.op === "runs_cancel").length, 0);
  const resumed = await fixture(false, saved);
  resumed.events.resolve(terminal("output"));
  assert.equal((await resumed.result).turnManifest.state, "completed");
  assert.equal(resumed.calls.filter((c) => c.op === "runs_create").length, 0);
  assert.equal(resumed.calls.find((c) => c.op === "runs_get").body.run_id, saved.providerRunId);
});

test("denied and mismatched resume never redispatch", async () => {
  for (const mode of ["denied", "mismatch"]) {
    const calls = [];
    globalThis.fetch = async (url, init) => {
      calls.push({ op: new URL(url).pathname.split("/").pop(), body: JSON.parse(init.body) });
      return { ok: mode !== "denied", status: 403, json: async () => mode === "denied" ? { code: "access_denied" } : { run_id: "other-run", status: "running" } };
    };
    await assert.rejects(live.streamChatViaContract([], { turnManifest: { turnId: "saved", providerRunId: "exact-run", state: "settlement_unknown" } }),
      (error) => error.code === (mode === "denied" ? "access_denied" : "no_run_id"));
    assert.deepEqual(calls.map((c) => c.op), ["runs_get"]);
    assert.equal(calls[0].body.run_id, "exact-run");
  }
});

test("old detached poll cannot overwrite a resumed completion", async () => {
  const old = await fixture();
  live.detachLiveChatStream();
  const resumed = await fixture(false, { ...turnStoreGet(old.turnId) });
  resumed.events.resolve(terminal("output"));
  await resumed.result;
  const count = old.states.length;
  old.events.resolve(terminal("output"));
  await old.result;
  assert.equal(turnStoreGet(old.turnId).state, "completed");
  assert.equal(old.states.length, count, "stale continuation must not persist a state");
});

test("late creation acknowledgement after detach retains its run ID", async () => {
  const requested = defer();
  const acknowledgement = defer();
  const calls = [];
  globalThis.fetch = async (url) => {
    const op = new URL(url).pathname.split("/").pop();
    calls.push(op);
    assert.equal(op, "runs_create");
    requested.resolve();
    return { ok: true, json: async () => acknowledgement.promise };
  };
  let persisted;
  const result = live.streamChatViaContract([], { onState: (turn) => { persisted = { ...turn }; } });
  await requested.promise;
  live.detachLiveChatStream();
  acknowledgement.resolve({ run_id: "late-run", sequence_cursor: 0 });
  await result;
  assert.equal(persisted.providerRunId, "late-run");
  assert.equal(persisted.state, "settlement_unknown");
  assert.deepEqual(calls, ["runs_create"]);
});

function controllerFixture() {
  const status = { dataset: {}, textContent: "", hidden: true, children: [], append(...nodes) { this.children.push(...nodes); } };
  globalThis.document = { querySelector: (selector) => selector === "[data-agent-stream-status]" ? status : null,
    createElement: () => ({ addEventListener(type, callback) { this[type] = callback; } }),
    getElementById: () => null, addEventListener() {}, removeEventListener() {} };
  Object.assign(window, { setTimeout, clearTimeout, setInterval, clearInterval,
    requestAnimationFrame: () => 1, cancelAnimationFrame() {}, addEventListener() {}, removeEventListener() {} });
  const ctx = { activeSessionId: "one", sessions: [{ id: "one", messages: [] }, { id: "two", messages: [] }],
    streamGeneration: 0, turnBusy: true, streamTimer: 0, followUpQueue: [] };
  controller.bindAgentStream(ctx, { streamEl: () => null, titleEl: () => null });
  sessions.bindAgentSessions(ctx, { sessionListEl: () => null });
  return { ctx, status };
}

test("actual session selection detaches without a cancellation", async () => {
  const { ctx } = controllerFixture();
  const f = await fixture();
  ctx.sessions[0].lastTurn = turnStoreGet(f.turnId);
  sessions.selectSession("two");
  assert.equal(ctx.activeSessionId, "two");
  f.events.resolve(terminal("output"));
  await f.result;
  assert.equal(ctx.sessions[0].lastTurn.state, "settlement_unknown");
  assert.equal(f.calls.filter((c) => c.op === "runs_cancel").length, 0);
});

test("hydrated workspace uses the controller resume path and retains a real denial", async () => {
  const { ctx, status } = controllerFixture();
  ctx.turnBusy = false;
  workspace.bindAgentWorkspaceStore({
    getSessions: () => ctx.sessions, setSessions: (value) => { ctx.sessions = value; },
    setWorkspaceHydrated() {}, setSessionMode() {}, setActiveSessionId: (value) => { ctx.activeSessionId = value; },
  });
  workspace.applyAgentWorkspaceSnapshot({ v: 1, activeSessionId: "one", sessions: [{ id: "one", messages: [],
    lastTurn: { turnId: "saved-controller", providerRunId: "saved-runtime-run", state: "streaming" } }] });
  const settled = defer();
  controller.bindAgentStream(ctx, { streamEl: () => null, titleEl: () => null,
    persistAgentWorkspaceSoon: () => { if (ctx.sessions[0].lastTurn.error) settled.resolve(); } });
  const calls = [];
  globalThis.fetch = async (url, init) => {
    calls.push({ op: new URL(url).pathname.split("/").pop(), body: JSON.parse(init.body) });
    return { ok: false, status: 403, json: async () => ({ code: "access_denied" }) };
  };
  controller.startTurnForPrompt("must not redispatch");
  await settled.promise;
  assert.deepEqual(calls.map((c) => c.op), ["runs_get"]);
  assert.equal(calls[0].body.run_id, "saved-runtime-run");
  assert.equal(ctx.sessions[0].lastTurn.state, "settlement_unknown");
  assert.match(status.textContent, /access_denied/);
  const button = status.children.at(-1);
  assert.equal(button.textContent, "Check status");
  button.click();
  button.click();
  await new Promise(setImmediate);
  assert.deepEqual(calls.map((c) => c.op), ["runs_get", "runs_get"]);
});

test("terminal unknown survives reload and permits an explicit new turn", async () => {
  const { ctx, status } = controllerFixture();
  ctx.turnBusy = false;
  const settledTurn = { turnId: "settled", providerRunId: "old-run", state: "settlement_unknown", completedAt: 1 };
  ctx.sessions[0].lastTurn = recoverStalePersistedTurn({ lastTurn: settledTurn }).lastTurn;
  const settled = defer();
  controller.bindAgentStream(ctx, { streamEl: () => null, titleEl: () => null,
    persistAgentWorkspaceSoon: () => {
      const turn = ctx.sessions[0].lastTurn;
      if (turn.providerRunId === "new-run" && turn.completedAt) settled.resolve();
    } });
  const calls = [];
  globalThis.fetch = async (url) => {
    const op = new URL(url).pathname.split("/").pop();
    calls.push(op);
    assert.ok(["runs_create", "runs_events"].includes(op));
    return { ok: true, json: async () => op === "runs_create" ? { run_id: "new-run", sequence_cursor: 0 } : terminal("settlement_unknown") };
  };
  controller.renderActiveSession();
  assert.equal(calls.length, 0);
  controller.startTurnForPrompt("new explicit turn");
  await settled.promise;
  await new Promise(setImmediate);
  assert.deepEqual(calls, ["runs_create", "runs_events"]);
  assert.equal(ctx.sessions[0].lastTurn.state, "settlement_unknown");
  assert.ok(ctx.sessions[0].lastTurn.completedAt > 0);
  assert.equal(status.textContent, "Outcome unknown");
});
