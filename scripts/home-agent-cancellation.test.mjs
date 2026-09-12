import assert from "node:assert/strict";
import test from "node:test";
import { execFileSync } from "node:child_process";

function fixtureWindow(location) {
  // Use real event delivery for workspace/session and beforeunload listeners.
  // Older Node versions need the browser's CustomEvent detail field supplied.
  globalThis.CustomEvent ??= class CustomEvent extends Event {
    constructor(type, options = {}) {
      super(type, options);
      this.detail = options.detail ?? null;
    }
  };
  return Object.assign(new EventTarget(), {
    location, setTimeout, clearTimeout, setInterval, clearInterval,
  });
}
globalThis.window = fixtureWindow({ href: "http://home.invalid/apps/assistant/", hash: "#home_token=fixture" });
const live = await import("../capsules/assistant/browser/agent-live.js");
const { turnStoreGet, cheapTurnSnapshot } = await import("../capsules/assistant/browser/agent-context.js");
const { recoverStalePersistedTurn } = await import("../capsules/assistant/browser/agent-stream-qos.js");
const controller = await import("../capsules/assistant/browser/agent-stream.js");
const workspace = await import("../capsules/assistant/browser/agent-workspace.js");
const sessions = await import("../capsules/assistant/browser/agent-sessions.js");
const defer = () => { let resolve, reject; const promise = new Promise((r, e) => { resolve = r; reject = e; }); return { promise, resolve, reject }; };
const terminal = (kind) => ({ events: [{ sequence: 1, kind, terminal: true, data: kind === "output" ? { schema: "elastos.model.output.text/v1", text: "Done" } : {} }], next_cursor: 1, has_more: false });

test("content intent survives missing catalog and overlapping refresh; hosted replacement is deliberate", async t => {
  t.after(() => live.selectLiveOffer(""));
  const cid = `bafybei${"a".repeat(52)}`;
  const chosen = { id: "chosen", title: "Chosen", operation: "text", input_modalities: ["text/plain"], output_modalities: ["text/plain"] };
  const other = { ...chosen, id: "other", title: "Other" };
  let offers = [chosen, other];
  let rows = [{ source: "signed-model-catalog", role: "content", installed: false, launchable: false, cid,
    title: "Chosen", signature_state: "catalog-signature-verified", model_runtime: { admitted: true, dispatch_ready: true, offer_id: "chosen" } }];
  let read = async () => offers;
  globalThis.fetch = async url => ({ ok: true, json: async () => new URL(url).pathname === "/api/capsules/catalog"
    ? { schema: "elastos.capsules.catalog/v1", model_catalog_state: "verified", capsules: rows }
    : { offers: await read() } });
  live.selectLiveOffer("chosen", cid);
  await live.probeLiveInference({ force: true });
  assert.equal(live.selectedLiveOffer().offerId, "chosen");
  assert.equal(live.liveContentModels().length, 1);
  rows = [];
  await live.probeLiveInference({ force: true });
  assert.equal(live.selectedLiveOffer(), null);
  assert.equal(live.liveContentChoice(), cid);
  assert.deepEqual(live.getLiveInferenceState().models.map(m => m.offerId), ["chosen", "other"], "available replacements remain discoverable");
  const pending = defer(), started = defer();
  read = () => { started.resolve(); return pending.promise; };
  const old = live.probeLiveInference({ force: true });
  await started.promise;
  offers = [other]; read = async () => offers;
  await live.probeLiveInference({ force: true });
  live.selectLiveOffer("other");
  pending.reject(new Error("old request failed")); await old;
  assert.equal(live.liveOfferChoice(), "other");
  assert.equal(live.liveContentChoice(), null);
  assert.equal(live.selectedLiveOffer().offerId, "other");
  const newer = defer(), newerStarted = defer();
  read = () => { newerStarted.resolve(); return newer.promise; };
  const fresh = live.probeLiveInference({ force: true });
  await newerStarted.promise;
  assert.equal(live.selectedLiveOffer(), null);
  const shared = live.probeLiveInference();
  newer.resolve([other]);
  await Promise.all([fresh, shared]);
  assert.equal(live.selectedLiveOffer().offerId, "other");
  live.selectLiveOffer("");
});

test("an unavailable chosen offer cannot become another model or cached readiness", async t => {
  t.after(() => live.selectLiveOffer(""));
  globalThis.fetch = async () => ({ ok: true, json: async () => ({ offers: [
    { id: "other", title: "Other", operation: "text", input_modalities: ["text/plain"], output_modalities: ["text/plain"] },
  ] }) });
  live.selectLiveOffer("chosen");
  await live.probeLiveInference({ force: true });
  assert.equal(live.liveOfferChoice(), "chosen");
  assert.equal(live.selectedLiveOffer(), null, "missing choice stays unavailable while another offer exists");
  globalThis.fetch = async () => { throw new Error("fixture unavailable"); };
  await live.probeLiveInference({ force: true });
  assert.equal(live.getLiveInferenceState().live, false, "failed refresh cannot reuse cached readiness");
  assert.equal(live.liveOfferChoice(), "chosen");
  live.selectLiveOffer("");
});

test("Agent selection pair roundtrips with drafts; invalid pair blocks hydration and new dispatch", async t => {
  t.after(() => { workspace.bindAgentWorkspaceStore(null); live.selectLiveOffer(""); });
  const cid = `bafybei${"a".repeat(52)}`;
  let hydrated = true, draft = { text: "Untouched draft", parts: [] };
  workspace.bindAgentWorkspaceStore({ getSessions: () => [], setSessions() {}, getActiveSessionId: () => null,
    getReasoningVisible: () => false,
    setReasoningVisible() {},
    setActiveSessionId() {}, getSessionMode: () => "chat", setSessionMode() {},
    getComposerDraft: () => draft, applyComposerDraft: value => { draft = value; },
    getWorkspaceHydrated: () => hydrated, setWorkspaceHydrated: value => { hydrated = value; } });
  live.selectLiveOffer("chosen", cid);
  const saved = workspace.getAgentWorkspaceSnapshot();
  live.selectLiveOffer("other");
  assert.equal(workspace.applyAgentWorkspaceSnapshot(saved), true);
  assert.equal(live.liveOfferChoice(), "chosen");
  assert.equal(live.liveContentChoice(), cid);
  assert.equal(workspace.getAgentWorkspaceSnapshot().composerDraft.text, "Untouched draft");
  for (const invalid of [{ ...saved, selectedModelCid: "bad" }, { ...saved, liveOfferId: "" }]) {
    assert.equal(workspace.applyAgentWorkspaceSnapshot(invalid), false);
    assert.equal(workspace.getAgentWorkspaceSnapshot(), null);
    assert.equal(live.selectedLiveOffer(), null);
    assert.equal(draft.text, "Untouched draft");
  }
  assert.equal(workspace.applyAgentWorkspaceSnapshot(saved), true);
  live.selectLiveOffer("other");
  assert.equal(workspace.getAgentWorkspaceSnapshot().selectedModelCid, null);
  live.selectLiveOffer("");
});

test("nested Agent Models intent registers top Home with exact origin and leaves draft alone", async () => {
  const { openModelsFromAgent } = await import("../capsules/assistant/browser/harness-host.js");
  const prior = { href: window.location.href, top: window.top };
  const messages = [];
  window.top = { postMessage: (message, origin) => messages.push({ message, origin }) };
  window.location.href = "https://home.example/apps/home-agent/?home_origin=https%3A%2F%2Fhome.example";
  assert.equal(openModelsFromAgent(), true);
  assert.deepEqual(messages, [
    { origin: "https://home.example", message: { type: "home:app-ready", homeToken: "fixture" } },
    { origin: "https://home.example", message: { type: "home:open-target", homeToken: "fixture", target: "system", query: { settings: "models" } } },
  ]);
  window.location.href = "https://home.example/apps/home-agent/?home_origin=null";
  assert.equal(openModelsFromAgent(), false);
  assert.equal(messages.length, 2);
  window.location.href = prior.href; window.top = prior.top;
});

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

test("an immediately failed create settles without polling events", async () => {
  const calls = [];
  globalThis.fetch = async (url) => {
    const op = new URL(url).pathname.split("/").pop();
    calls.push(op);
    if (op === "offers_list") return { ok: true, json: async () => ({ offers: [
      { id: "fixture", title: "Fixture", operation: "text", input_modalities: ["text/plain"], output_modalities: ["text/plain"] },
    ] }) };
    if (op === "runs_create") return { ok: true, json: async () => ({
      run_id: "run-full", sequence_cursor: 0, status: "failed",
      terminal: { status: "failed", error: { code: "selection_unavailable", message: "model offer is not available" } },
    }) };
    throw new Error(`unexpected operation ${op}`);
  };
  await live.probeLiveInference({ force: true });
  await assert.rejects(
    live.streamChatViaContract([{ role: "user", content: "fixture" }], { onAccepted() {}, onState() {} }),
    (error) => error.code === "selection_unavailable",
  );
  assert.deepEqual(calls.filter((op) => op.startsWith("runs_")), ["runs_create"]);
});

test("a retained unknown create settles from the create view without polling", async () => {
  const calls = [];
  globalThis.fetch = async (url) => {
    const op = new URL(url).pathname.split("/").pop();
    calls.push(op);
    if (op === "offers_list") return { ok: true, json: async () => ({ offers: [
      { id: "fixture", title: "Fixture", operation: "text", input_modalities: ["text/plain"], output_modalities: ["text/plain"] },
    ] }) };
    if (op === "runs_create") return { ok: true, json: async () => ({
      run_id: "run-pruned", sequence_cursor: 0, status: "settlement_unknown",
      terminal: { status: "settlement_unknown" }, settlement_source: "runtime_index",
    }) };
    throw new Error(`unexpected operation ${op}`);
  };
  await live.probeLiveInference({ force: true });
  const result = await live.streamChatViaContract([{ role: "user", content: "fixture" }], { onAccepted() {}, onState() {} });
  assert.equal(result.turnManifest.state, "settlement_unknown");
  assert.equal(result.turnManifest.providerRunId, "run-pruned");
  assert.ok(result.turnManifest.completedAt > 0);
  assert.deepEqual(calls.filter((op) => op.startsWith("runs_")), ["runs_create"]);
});

test("reload of a retained settlement settles from get without polling events", async () => {
  const calls = [];
  globalThis.fetch = async (url) => {
    const op = new URL(url).pathname.split("/").pop();
    calls.push(op);
    if (op === "runs_get") return { ok: true, json: async () => ({
      run_id: "exact-run", status: "settlement_unknown",
      terminal: { status: "settlement_unknown" }, settlement_source: "runtime_index",
    }) };
    throw new Error(`unexpected operation ${op}`);
  };
  const result = await live.streamChatViaContract([], {
    turnManifest: { turnId: "saved", providerRunId: "exact-run", state: "settlement_unknown" },
  });
  assert.equal(result.turnManifest.state, "settlement_unknown");
  assert.ok(result.turnManifest.completedAt > 0);
  assert.deepEqual(calls, ["runs_get"]);
});

test("an attached poll settles from a retained terminal event", async () => {
  const f = await fixture();
  f.events.resolve({ events: [{ sequence: 1, kind: "settlement_unknown", terminal: true, data: {} }], next_cursor: 1, has_more: false });
  const result = await f.result;
  assert.equal(result.turnManifest.state, "settlement_unknown");
  assert.ok(result.turnManifest.completedAt > 0);
});

test("an attached poll at cursor 7 settles from a retained terminal event", async () => {
  const calls = [];
  globalThis.fetch = async (url, init) => {
    const op = new URL(url).pathname.split("/").pop();
    const body = init?.body ? JSON.parse(init.body) : null;
    calls.push({ op, body });
    if (op === "offers_list") return { ok: true, json: async () => ({ offers: [
      { id: "fixture", title: "Fixture", operation: "text", input_modalities: ["text/plain"], output_modalities: ["text/plain"] },
    ] }) };
    if (op === "runs_create") return { ok: true, json: async () => ({
      run_id: "run-attached", sequence_cursor: 7, status: "running",
    }) };
    if (op === "runs_events") {
      assert.equal(body.after_sequence, 7);
      return { ok: true, json: async () => ({
        schema: "elastos.model.run-events/v1",
        run_id: "run-attached",
        events: [{ sequence: 8, kind: "settlement_unknown", terminal: true, data: {} }],
        next_cursor: 8, has_more: false, settlement_source: "runtime_index",
      }) };
    }
    throw new Error(`unexpected operation ${op}`);
  };
  await live.probeLiveInference({ force: true });
  const result = await live.streamChatViaContract([{ role: "user", content: "fixture" }], { onAccepted() {}, onState() {} });
  assert.equal(result.turnManifest.state, "settlement_unknown");
  assert.ok(result.turnManifest.completedAt > 0);
  assert.equal(calls.filter((c) => c.op === "runs_events").length, 1);
});

test("a completed pruned journal states that live output was not retained", async () => {
  const calls = [];
  const deltas = [];
  globalThis.fetch = async (url) => {
    const op = new URL(url).pathname.split("/").pop();
    calls.push(op);
    if (op === "offers_list") return { ok: true, json: async () => ({ offers: [
      { id: "fixture", title: "Fixture", operation: "text", input_modalities: ["text/plain"], output_modalities: ["text/plain"] },
    ] }) };
    if (op === "runs_create") return { ok: true, json: async () => ({
      run_id: "run-completed-pruned", sequence_cursor: 0, status: "completed",
      terminal: { status: "completed" }, settlement_source: "runtime_index",
      output_retained: false,
    }) };
    throw new Error(`unexpected operation ${op}`);
  };
  await live.probeLiveInference({ force: true });
  const result = await live.streamChatViaContract([{ role: "user", content: "fixture" }], {
    onAccepted() {}, onState() {}, onDelta: (delta) => deltas.push(delta),
  });
  assert.equal(result.turnManifest.state, "completed");
  assert.equal(result.turnManifest.outputRetained, false);
  assert.equal(deltas.length, 0);
  assert.deepEqual(calls.filter((op) => op.startsWith("runs_")), ["runs_create"]);
});

test("an attached poll of a completed pruned journal states that output was not retained", async () => {
  const calls = [];
  const deltas = [];
  globalThis.fetch = async (url, init) => {
    const op = new URL(url).pathname.split("/").pop();
    const body = init?.body ? JSON.parse(init.body) : null;
    calls.push({ op, body });
    if (op === "offers_list") return { ok: true, json: async () => ({ offers: [
      { id: "fixture", title: "Fixture", operation: "text", input_modalities: ["text/plain"], output_modalities: ["text/plain"] },
    ] }) };
    if (op === "runs_create") return { ok: true, json: async () => ({
      run_id: "run-completed-events", sequence_cursor: 2, status: "running",
    }) };
    if (op === "runs_events") {
      assert.equal(body.after_sequence, 2);
      return { ok: true, json: async () => ({
        schema: "elastos.model.run-events/v1",
        run_id: "run-completed-events",
        events: [{ sequence: 3, kind: "completed", terminal: true, data: { output_retained: false } }],
        next_cursor: 3, has_more: false, settlement_source: "runtime_index",
        output_retained: false,
      }) };
    }
    throw new Error(`unexpected operation ${op}`);
  };
  await live.probeLiveInference({ force: true });
  const result = await live.streamChatViaContract([{ role: "user", content: "fixture" }], {
    onAccepted() {}, onState() {}, onDelta: (delta) => deltas.push(delta),
  });
  assert.equal(result.turnManifest.state, "completed");
  assert.equal(result.turnManifest.outputRetained, false);
  assert.equal(deltas.length, 0);
  assert.equal(calls.filter((c) => c.op === "runs_events").length, 1);
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

for (const mode of ["lost_response", "invalid_json", "missing_run_id", "invalid_run_id", "server_error", "invalid_typed_response_400", "unauthorized", "forbidden", "remote_local_error_code"]) test(`uncertain create acceptance stays unresolved: ${mode}`, async () => {
  const { ctx, status } = controllerFixture();
  ctx.turnBusy = false;
  const settled = defer();
  controller.bindAgentStream(ctx, { streamEl: () => null, titleEl: () => null,
    persistAgentWorkspaceSoon: () => {
      if (ctx.sessions[0].lastTurn?.error) settled.resolve();
    } });
  const dispatched = [];
  globalThis.fetch = async (url, init) => {
    const op = new URL(url).pathname.split("/").pop();
    if (op === "offers_list") return { ok: true, json: async () => ({ offers: [
      { id: "fixture", title: "Fixture", operation: "text", input_modalities: ["text/plain"], output_modalities: ["text/plain"] },
    ] }) };
    assert.equal(op, "runs_create");
    const body = JSON.parse(init.body);
    assert.equal(ctx.sessions[0].lastTurn.createRequestId, body.request_id, "identity is retained before sending");
    dispatched.push(body);
    if (mode === "lost_response") throw new TypeError("acceptance response lost after dispatch");
    const responseStatus = { server_error: 502, invalid_typed_response_400: 400, unauthorized: 401, forbidden: 403, remote_local_error_code: 400 }[mode] || 200;
    return { ok: responseStatus === 200, status: responseStatus,
      json: async () => {
        if (mode === "invalid_json") throw new SyntaxError("incomplete JSON response");
        if (mode === "invalid_typed_response_400") return { code: "provider_error", message: "model provider returned an invalid typed response" };
        if (mode === "remote_local_error_code") return { code: "missing-home-launch-token" };
        return mode === "invalid_run_id" ? { run_id: {} } : {};
      } };
  };
  await live.probeLiveInference({ force: true });
  controller.startTurnForPrompt("one explicit request");
  await settled.promise;
  await new Promise(setImmediate);
  assert.equal(dispatched.length, 1);
  assert.equal(ctx.sessions[0].lastTurn.state, "settlement_unknown");
  assert.ok(!ctx.sessions[0].lastTurn.completedAt);
  assert.equal(live.unresolvedModelTurn(ctx.sessions[0].lastTurn), true);
  const saved = JSON.parse(JSON.stringify(workspace.serializeSessionForPersist(ctx.sessions[0])));
  assert.equal(saved.lastTurn.createRequestId, dispatched[0].request_id);
  assert.ok(saved.lastTurn.createRequestId.length <= 80);
  ctx.sessions[0] = recoverStalePersistedTurn(saved);
  controller.startTurnForPrompt("must not implicitly retry");
  await new Promise(setImmediate);
  assert.equal(dispatched.length, 1, "reload must not create again without a known run ID");
  assert.equal(controller.canSubmitNewTurn(), false);
  assert.match(status.textContent, /Run acceptance is unknown.*new chat/);
  assert.equal(status.children.length, 0, "no Check status that secretly recreates");
});

test("missing local launch token is proved refused before fetch", () => {
  execFileSync(process.execPath, ["--input-type=module", "-e", `
    import assert from "node:assert/strict";
    const fixtureWindow = ${fixtureWindow.toString()};
    globalThis.window = fixtureWindow({ hash: "", href: "http://home.invalid/apps/assistant/" });
    let calls = 0;
    globalThis.fetch = async () => { calls += 1; throw new Error("unexpected fetch"); };
    const { modelRunCall } = await import(${JSON.stringify(new URL("../capsules/assistant/browser/agent-live.js", import.meta.url).href)});
    await assert.rejects(modelRunCall("runs_create", {}),
      (error) => error.code === "missing-home-launch-token" && error.preDispatchRefusal === true);
    assert.equal(calls, 0);
  `], { timeout: 5000 });
});

function controllerFixture() {
  const status = { dataset: {}, textContent: "", hidden: true, children: [], append(...nodes) { this.children.push(...nodes); } };
  globalThis.document = { querySelector: (selector) => selector === "[data-agent-stream-status]" ? status : null,
    createElement: () => ({ addEventListener(type, callback) { this[type] = callback; } }),
    getElementById: () => null, addEventListener() {}, removeEventListener() {} };
  Object.assign(window, { setTimeout, clearTimeout, setInterval, clearInterval,
    requestAnimationFrame: () => 1, cancelAnimationFrame() {} });
  const ctx = { activeSessionId: "one", sessions: [{ id: "one", messages: [] }, { id: "two", messages: [] }],
    streamGeneration: 0, turnBusy: true, streamTimer: 0, followUpQueue: [] };
  controller.bindAgentStream(ctx, { streamEl: () => null, titleEl: () => null });
  sessions.bindAgentSessions(ctx, { sessionListEl: () => null });
  return { ctx, status };
}

function createDomNode(tag = "div") {
  const node = {
    tagName: String(tag).toUpperCase(),
    className: "",
    dataset: {},
    textContent: "",
    hidden: false,
    children: [],
    style: {},
    isConnected: true,
    classList: {
      _names: new Set(),
      add(...names) { names.forEach((name) => this._names.add(name)); node.className = [...this._names].join(" "); },
      remove(...names) { names.forEach((name) => this._names.delete(name)); node.className = [...this._names].join(" "); },
      toggle(name, force) {
        if (force === true) this.add(name);
        else if (force === false) this.remove(name);
        else if (this._names.has(name)) this.remove(name);
        else this.add(name);
      },
      contains(name) { return this._names.has(name); },
    },
    append(...nodes) {
      for (const child of nodes) {
        if (child == null) continue;
        const next = typeof child === "string" ? { textContent: child, children: [] } : child;
        next.parent = this;
        this.children.push(next);
      }
    },
    replaceChildren(...nodes) { this.children = []; this.append(...nodes); },
    remove() {
      this.removed = true;
      if (this.parent?.children) this.parent.children = this.parent.children.filter((child) => child !== this);
    },
    querySelector(selector) {
      const wanted = selector.startsWith(".") ? selector.slice(1) : "";
      const walk = (root) => {
        for (const child of root.children || []) {
          if (wanted && String(child.className || "").split(/\s+/).includes(wanted)) return child;
          const found = walk(child);
          if (found) return found;
        }
        return null;
      };
      return walk(this);
    },
    addEventListener() {},
    removeEventListener() {},
    setAttribute() {},
    scrollIntoView() {},
    contains() { return false; },
    querySelectorAll() { return []; },
    closest() { return null; },
    scrollHeight: 0,
    scrollTop: 0,
    clientHeight: 0,
  };
  Object.defineProperty(node, "innerHTML", {
    set(value) { this._html = String(value || ""); this.content = createDomNode("fragment"); },
    get() { return this._html || ""; },
  });
  return node;
}

function streamControllerFixture() {
  const { ctx, status } = controllerFixture();
  ctx.turnBusy = false;
  ctx.sessions[0].messages = [{ role: "user", text: "fixture", modelText: "fixture" }];
  const stream = createDomNode("div");
  const viewport = createDomNode("div");
  const priorQuery = document.querySelector.bind(document);
  document.createElement = (tag) => createDomNode(tag);
  document.createTextNode = (value = "") => {
    const text = String(value);
    return {
      data: text,
      textContent: text,
      nodeType: 3,
      children: [],
      appendData(extra) {
        this.data += String(extra);
        this.textContent = this.data;
      },
    };
  };
  document.querySelector = (selector) => selector === "[data-agent-stream-status]" ? status : priorQuery(selector);
  controller.bindAgentStream(ctx, {
    streamEl: () => stream,
    streamViewportEl: () => viewport,
    streamScrollEl: () => stream,
    titleEl: () => createDomNode("h1"),
    clearEmptyState() {},
    scrollStreamToEnd() {},
    renderSessions() {},
    persistAgentWorkspaceSoon() {},
  });
  return { ctx, status, stream };
}

async function runUnretainedLiveTurn(t, { create, eventsPages = [] }) {
  t.after(() => { live.detachLiveChatStream(); live.selectLiveOffer(""); });
  const { ctx, status, stream } = streamControllerFixture();
  controller.bindAgentStream(ctx, {
    streamEl: () => stream,
    streamViewportEl: () => createDomNode("div"),
    streamScrollEl: () => stream,
    titleEl: () => createDomNode("h1"),
    clearEmptyState() {},
    scrollStreamToEnd() {},
    renderSessions() {},
    persistAgentWorkspaceSoon() {},
  });
  let offerLists = 0;
  let eventPage = 0;
  globalThis.fetch = async (url, init) => {
    const op = new URL(url).pathname.split("/").pop();
    const body = init?.body ? JSON.parse(init.body) : null;
    if (op === "offers_list") {
      offerLists += 1;
      return { ok: true, json: async () => ({ offers: [
        { id: "fixture", title: "Fixture", operation: "text", input_modalities: ["text/plain"], output_modalities: ["text/plain"] },
      ] }) };
    }
    if (op === "runs_create") return { ok: true, json: async () => create };
    if (op === "runs_events") {
      const page = eventsPages[Math.min(eventPage, Math.max(eventsPages.length - 1, 0))];
      eventPage += 1;
      return { ok: true, json: async () => page.data };
    }
    throw new Error(`unexpected operation ${op}`);
  };
  await live.probeLiveInference({ force: true });
  const offersAfterProbe = offerLists;
  controller.startTurnForPrompt("fixture");
  const started = Date.now();
  while (Date.now() - started < 2000) {
    if (ctx.sessions[0].lastTurn?.outputRetained === false) break;
    await new Promise((resolve) => setTimeout(resolve, 20));
  }
  if (ctx.sessions[0].lastTurn?.outputRetained !== false) {
    throw new Error(`unretained turn missing: ${JSON.stringify(ctx.sessions[0].lastTurn)} status=${status.textContent}`);
  }
  await new Promise(setImmediate);
  return { ctx, status, stream, offerLists, offersAfterProbe };
}

test("live chat states completed pruned output with no deltas and restores the warning", async t => {
  const { ctx, status, offerLists, offersAfterProbe } = await runUnretainedLiveTurn(t, {
    create: {
      run_id: "run-completed-pruned", sequence_cursor: 0, status: "completed",
      terminal: { status: "completed" }, settlement_source: "runtime_index",
      output_retained: false,
    },
  });
  assert.equal(status.textContent, controller.OUTPUT_UNRETAINED_STATUS);
  assert.notEqual(status.textContent, "Model returned an empty reply");
  assert.equal(offerLists, offersAfterProbe, "completed pruned output must not force an inference probe");
  assert.equal(ctx.sessions[0].lastTurn.state, "completed");
  assert.equal(ctx.sessions[0].lastTurn.outputRetained, false);
  assert.ok(!ctx.sessions[0].messages.some((message) => message.role === "agent"));
  const snap = cheapTurnSnapshot(ctx.sessions[0].lastTurn);
  assert.equal(snap.outputRetained, false);
  assert.equal(cheapTurnSnapshot({ ...ctx.sessions[0].lastTurn, outputRetained: true }).outputRetained, undefined);
  const saved = JSON.parse(JSON.stringify(workspace.serializeSessionForPersist(ctx.sessions[0])));
  assert.equal(saved.lastTurn.outputRetained, false);
  ctx.sessions[0] = recoverStalePersistedTurn(saved);
  status.textContent = "";
  controller.renderActiveSession();
  assert.equal(ctx.sessions[0].lastTurn.outputRetained, false);
  assert.equal(status.textContent, controller.OUTPUT_UNRETAINED_STATUS);
});

test("live chat keeps partial deltas and restores completed-but-unavailable output", async t => {
  const { ctx, status, offerLists, offersAfterProbe } = await runUnretainedLiveTurn(t, {
    create: { run_id: "run-partial-pruned", sequence_cursor: 0, status: "running" },
    eventsPages: [
      { after: 0, data: {
        schema: "elastos.model.run-events/v1", run_id: "run-partial-pruned",
        events: [{ sequence: 1, kind: "text_delta", data: { text: "Partial prefix" } }],
        next_cursor: 1, has_more: true,
      } },
      { after: 1, data: {
        schema: "elastos.model.run-events/v1", run_id: "run-partial-pruned",
        events: [{ sequence: 2, kind: "completed", terminal: true, data: { output_retained: false } }],
        next_cursor: 2, has_more: false, settlement_source: "runtime_index",
        output_retained: false,
      } },
    ],
  });
  assert.equal(status.textContent, controller.OUTPUT_UNRETAINED_STATUS);
  assert.equal(offerLists, offersAfterProbe);
  assert.equal(ctx.sessions[0].lastTurn.state, "completed");
  assert.equal(ctx.sessions[0].lastTurn.outputRetained, false);
  const agent = ctx.sessions[0].messages.filter((message) => message.role === "agent");
  assert.equal(agent.length, 1);
  assert.equal(agent[0].text, "Partial prefix");
  assert.equal(agent[0].partial, undefined);
  const saved = JSON.parse(JSON.stringify(workspace.serializeSessionForPersist(ctx.sessions[0])));
  assert.equal(saved.lastTurn.outputRetained, false);
  assert.equal(saved.messages.at(-1).text, "Partial prefix");
  ctx.sessions[0] = recoverStalePersistedTurn(saved);
  status.textContent = "";
  controller.renderActiveSession();
  assert.equal(ctx.sessions[0].messages.at(-1).text, "Partial prefix");
  assert.equal(status.textContent, controller.OUTPUT_UNRETAINED_STATUS);
});

test("immediate failed create shows the provider result and restores it after save", async t => {
  t.after(() => { live.detachLiveChatStream(); live.selectLiveOffer(""); });
  const { ctx, status } = streamControllerFixture();
  const calls = [];
  globalThis.fetch = async (url) => {
    const op = new URL(url).pathname.split("/").pop();
    calls.push(op);
    if (op === "offers_list") return { ok: true, json: async () => ({ offers: [
      { id: "fixture", title: "Fixture", operation: "text", input_modalities: ["text/plain"], output_modalities: ["text/plain"] },
    ] }) };
    if (op === "runs_create") return { ok: true, json: async () => ({
      run_id: "run-overflow", sequence_cursor: 2, status: "failed",
      terminal: { status: "failed", error: { class: "selection_unavailable", code: "selection_unavailable", message: "model offer is not available" } },
    }) };
    throw new Error(`unexpected operation ${op}`);
  };
  await live.probeLiveInference({ force: true });
  controller.startTurnForPrompt("Reply with the word overflow");
  const started = Date.now();
  while (Date.now() - started < 2000) {
    if (ctx.sessions[0].lastTurn?.state === "failed" && status.textContent) break;
    await new Promise((resolve) => setTimeout(resolve, 20));
  }
  const visible = controller.formatStreamError({
    code: "selection_unavailable",
    message: "model offer is not available",
  });
  assert.equal(ctx.sessions[0].lastTurn.state, "failed");
  assert.equal(ctx.sessions[0].lastTurn.error, "selection_unavailable");
  assert.equal(ctx.sessions[0].lastTurn.providerRunId, "run-overflow");
  assert.ok(ctx.sessions[0].lastTurn.completedAt > 0);
  assert.equal(status.textContent, visible);
  assert.match(status.textContent, /model offer is not available/);
  assert.doesNotMatch(status.textContent, /busy/i);
  assert.equal(live.unresolvedModelTurn(ctx.sessions[0].lastTurn), false);
  assert.equal(controller.canSubmitNewTurn(), true);
  assert.deepEqual(calls.filter((op) => op.startsWith("runs_")), ["runs_create"]);
  const saved = JSON.parse(JSON.stringify(workspace.serializeSessionForPersist(ctx.sessions[0])));
  assert.equal(saved.lastTurn.state, "failed");
  assert.equal(saved.lastTurn.error, "selection_unavailable");
  ctx.sessions[0] = recoverStalePersistedTurn(saved);
  status.textContent = "";
  controller.renderActiveSession();
  assert.equal(ctx.sessions[0].lastTurn.state, "failed");
  assert.equal(status.textContent, visible);
  assert.match(status.textContent, /model offer is not available/);
  assert.doesNotMatch(status.textContent, /busy/i);
  assert.equal(controller.canSubmitNewTurn(), true);
  assert.deepEqual(calls.filter((op) => op.startsWith("runs_")), ["runs_create"]);
});

function renderedAgentText(stream) {
  const row = (stream.children || []).find((child) => child.dataset?.role === "agent");
  if (!row) return "";
  const parts = [];
  const walk = (node) => {
    if (!node || typeof node !== "object") return;
    if (typeof node.data === "string") parts.push(node.data);
    else if (String(node.className || "").includes("agent-stream-frozen-block") && node.textContent) {
      parts.push(node.textContent);
    }
    for (const child of node.children || []) walk(child);
  };
  walk(row);
  return parts.join("");
}

test("live chat flushes two consecutive deltas before reopen of unretained output", async t => {
  const { ctx, status, stream, offerLists, offersAfterProbe } = await runUnretainedLiveTurn(t, {
    create: { run_id: "run-two-deltas", sequence_cursor: 0, status: "running" },
    eventsPages: [
      { after: 0, data: {
        schema: "elastos.model.run-events/v1", run_id: "run-two-deltas",
        events: [
          { sequence: 1, kind: "text_delta", data: { text: "Hel" } },
          { sequence: 2, kind: "text_delta", data: { text: "lo" } },
          { sequence: 3, kind: "completed", terminal: true, data: { output_retained: false } },
        ],
        next_cursor: 3, has_more: false, settlement_source: "runtime_index",
        output_retained: false,
      } },
    ],
  });
  assert.equal(status.textContent, controller.OUTPUT_UNRETAINED_STATUS);
  assert.equal(offerLists, offersAfterProbe);
  assert.equal(renderedAgentText(stream), "Hello", "both deltas must paint before reopen");
  assert.equal(ctx.sessions[0].messages.filter((message) => message.role === "agent").at(-1)?.text, "Hello");
});

test("refresh after a refused new turn requires another deliberate Send", async t => {
  t.after(() => live.selectLiveOffer(""));
  const { ctx } = controllerFixture(); ctx.turnBusy = false;
  let offers = [];
  let created = 0;
  globalThis.fetch = async url => {
    const op = new URL(url).pathname.split("/").pop();
    if (op === "runs_create") created += 1;
    return { ok: true, json: async () => ({ offers }) };
  };
  live.selectLiveOffer("chosen");
  await live.probeLiveInference({ force: true });
  offers = [{ id: "chosen", title: "Chosen", operation: "text", input_modalities: ["text/plain"], output_modalities: ["text/plain"] }];
  controller.startTurnForPrompt("Keep this draft");
  await live.probeLiveInference();
  assert.equal(live.selectedLiveOffer().offerId, "chosen");
  assert.equal(created, 0);
  assert.deepEqual(ctx.sessions[0].messages, []);
});

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
    getActiveSessionId: () => ctx.activeSessionId,
    getSessionMode: () => ctx.sessionMode || "chat",
    setSessionMode: (value) => { ctx.sessionMode = value; },
    setWorkspaceHydrated() {}, setActiveSessionId: (value) => { ctx.activeSessionId = value; },
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

test("actual composer preserves draft for missing runs and unknown acceptance until explicit new chat", async () => {
  const { status } = controllerFixture();
  const node = () => ({ dataset: {}, style: {}, value: "", scrollHeight: 28,
    classList: { add() {}, remove() {}, toggle() {}, contains: () => false },
    setAttribute() {}, focus() {}, querySelector: () => null });
  const input = node();
  const nodes = new Map([["#agent-composer-input", input], [".taskbar", node()], ["#agent-harness", node()],
    ["[data-agent-stream-status]", status]]);
  Object.assign(document, { body: node(), documentElement: node(), querySelectorAll: () => [],
    querySelector: (selector) => nodes.get(selector) || null });
  Object.assign(window, { innerHeight: 800, matchMedia: () => ({ matches: true }) });
  globalThis.requestAnimationFrame = window.requestAnimationFrame;
  globalThis.cancelAnimationFrame = window.cancelAnimationFrame;
  const harness = await import("../capsules/assistant/browser/agent-harness.js");
  const shelf = await import("../capsules/assistant/browser/agent-shelf.js");
  const bridge = await import("../capsules/assistant/browser/agent-send.js");
  bridge.registerAgentHarnessApi({ sendToAgentHarness: harness.sendToAgentHarness });
  const calls = [];
  globalThis.fetch = async (url, init) => {
    const op = new URL(url).pathname.split("/").pop();
    calls.push({ op, body: JSON.parse(init.body) });
    if (op === "runs_get") return { ok: false, status: 404, json: async () => ({ code: "run_not_found", message: "model run not found" }) };
    return { ok: true, json: async () => op === "offers_list"
      ? { offers: [{ id: "fixture", title: "Fixture", operation: "text", input_modalities: ["text/plain"], output_modalities: ["text/plain"] }] }
      : op === "runs_create" ? { run_id: "new-draft-run", sequence_cursor: 0 } : terminal("settlement_unknown") };
  };
  const draft = { text: "new unsent prompt", parts: [{ id: "attachment", kind: "file", name: "notes.txt", text: "reference" }] };
  for (const acceptanceUnknown of [false, true]) {
    calls.length = 0;
    status.children.length = 0;
    const oldTurn = acceptanceUnknown
      ? { turnId: "unknown-turn", createRequestId: "old-create-request", state: "submitted" }
      : { turnId: "missing-turn", providerRunId: "missing-run", state: "settlement_unknown", error: "run_not_found" };
    workspace.applyAgentWorkspaceSnapshot({ v: 1, activeSessionId: "old", sessions: [{ id: "old", title: "Previous chat",
      messages: [{ role: "user", text: "old prompt" }], lastTurn: recoverStalePersistedTurn({ lastTurn: oldTurn }).lastTurn }], composerDraft: draft });
    shelf.applyComposerDraft(draft);
    harness.showAgentHarness({ restore: true });
    await new Promise(setImmediate);
    if (acceptanceUnknown) assert.equal(calls.filter((c) => c.op !== "offers_list").length, 0);
    calls.length = 0;
    const before = workspace.getAgentWorkspaceSnapshot().sessions[0];
    const accepted = await shelf.sendAgentComposerMessage();
    assert.deepEqual(shelf.getComposerDraft(), draft);
    assert.deepEqual(workspace.getAgentWorkspaceSnapshot().sessions[0], before);
    assert.equal(accepted, false);
    assert.deepEqual(calls, [], "Send must not silently resume or redispatch the old run");
    if (acceptanceUnknown) {
      assert.match(status.textContent, /Run acceptance is unknown.*new chat/i);
      assert.equal(status.children.length, 0);
      assert.equal(workspace.getAgentWorkspaceSnapshot().sessions[0].lastTurn.createRequestId, "old-create-request");
    } else {
      assert.match(status.textContent, /Run record is unavailable.*new chat/i);
      status.children.at(-1).click();
      await new Promise(setImmediate);
      assert.deepEqual(calls.map((c) => c.op), ["runs_get"]);
      assert.equal(calls[0].body.run_id, "missing-run");
      assert.equal(workspace.getAgentWorkspaceSnapshot().sessions[0].lastTurn.providerRunId, "missing-run");
    }
    assert.deepEqual(shelf.getComposerDraft(), draft);
    sessions.newChat();
    assert.equal(await shelf.sendAgentComposerMessage(), true);
    await new Promise(setImmediate);
    assert.deepEqual(calls.map((c) => c.op), acceptanceUnknown ? ["runs_create", "runs_events"] : ["runs_get", "runs_create", "runs_events"]);
    assert.notEqual(calls.find((c) => c.op === "runs_create").body.request_id, oldTurn.createRequestId);
    assert.equal(workspace.getAgentWorkspaceSnapshot().sessions[0].messages.filter((m) => m.role === "user").length, 1);
    assert.deepEqual(shelf.getComposerDraft(), { text: "", parts: [] });
  }

  // The same real composer keeps changes made while its existing send bridge awaits acceptance.
  for (const accepted of [false, true]) {
    const pending = defer();
    bridge.registerAgentHarnessApi({ sendToAgentHarness: () => pending.promise });
    shelf.applyComposerDraft(draft);
    const sending = shelf.sendAgentComposerMessage();
    input.value = "next draft";
    shelf.addComposerAttachment({ name: "later.txt", text: "later reference" });
    pending.resolve(accepted);
    assert.equal(await sending, accepted);
    assert.equal(input.value, "next draft");
    assert.deepEqual(shelf.getComposerDraft().parts.map((p) => p.name), accepted ? ["later.txt"] : ["notes.txt", "later.txt"]);
  }
});
