import assert from "node:assert/strict";
import test from "node:test";
import { createHash } from "node:crypto";
import { runBrowserOperatorJourney } from "./browser-journey-operator.mjs";

const hash = value => `sha256:${createHash("sha256").update(value).digest("hex").slice(0, 16)}`;
const pageId = "page:vz-test", instance = "browser-instance", cleanupId = "private-cleanup";
const snapshot = "a".repeat(32), generation = "b".repeat(32), requestId = "admission-id";
const ownerSecret = "private-owner-token", attachSecret = "private-attach-secret";
const sessionSecret = "private-session-token", capSecret = "private-capability-token";
const fixtureRun = "controlled-run", url = `http://localhost:61511/main?run=${fixtureRun}`;
const effects = ["page_absent", "child_absent", "vm_absent", "route_absent", "socket_absent",
  "transport_session_absent", "turn_process_absent", "turn_listener_absent", "turn_relay_ports_absent",
  "ordinary_vsock_bridge_absent", "media_vsock_bridge_absent", "bootstrap_vsock_bridge_absent", "hibernation_state_absent"];

function virtualClock() {
  let now = 0, next = 0;
  const timers = new Map();
  const clock = { now: () => now,
    setTimeout: (fn, delay) => { const id = ++next; timers.set(id, { at: now + delay, fn }); return id; },
    clearTimeout: id => timers.delete(id) };
  return { clock, jump(ms) { now += ms; }, async run(promise) {
    let settled = false, result, error;
    promise.then(value => { result = value; settled = true; }, value => { error = value; settled = true; });
    for (let i = 0; !settled; i++) {
      assert.ok(i < 500, "probe is bounded");
      await new Promise(setImmediate);
      if (settled) break;
      const timer = [...timers].sort((a, b) => a[1].at - b[1].at)[0];
      assert.ok(timer, "pending callbacks need a deadline");
      timers.delete(timer[0]); now = timer[1].at; timer[1].fn();
    }
    assert.equal(timers.size, 0, "probe releases every owned timer");
    if (error) throw error;
    return result;
  } };
}

// Typed Runtime/fixture responses. This is a source consumer test: it does not
// replace the parent's installed Runtime, native effect, or Browser UI proof.
function fixture(options = {}) {
  const timer = virtualClock(), calls = [];
  let requested, approved = false, revoked = false, value = "", sequence = 1, frames = 100, bytes = 1000;
  let typeSent = false, humanSent = false, closed = false;
  const events = [{ sequence, type: "load", page: "main", value }];
  const emit = text => { value += text; events.push({ sequence: ++sequence, type: "input", page: "main", value }); };
  const admission = status => ({ schema: "elastos.browser.operator-admission/v1", request_id: requestId, status });
  const args = {
    pageId, expectedUrl: url, runtimeOrigin: "http://localhost:61510",
    runtimeCoords: { runtime_kind: "gateway", api_url: "http://127.0.0.1:61999", attach_secret: attachSecret },
    homeToken: ownerSecret, clock: timer.clock, requireVzTransport: true,
    markStage(stage) { calls.push({ stage }); if (options.stageThrows) throw new Error(ownerSecret); },
    async readState(budget) {
      assert.ok(budget.signal); assert.ok(budget.timeoutMs > 0);
      calls.push({ kind: "state" });
      if (options.stateHangs) return new Promise(() => {});
      if (!options.freezeFrames && !(typeSent && options.freezeAfterType) && !(humanSent && options.freezeAfterHuman)) {
        frames++; if (!options.freezeBytes) bytes += 100;
      }
      const raw = {
        viewer: { page_id: pageId, engine_id: "", exit_id: "", browser_instance: instance,
          actual_url: url, document_id: 12345 },
        page_status: { schema: "elastos.browser.page-status/v1", page_id: pageId, actual_url: url },
        sessions: {
          schema: "elastos.browser.session-capacity/v1", launching_sessions: 0,
          active_sessions: 1, total_sessions: 1, principal_sessions: 1,
          engine_cleanup_obligations: 0, launch_reconciliation_obligations: 0,
          recoverable_page: { state: "active", page_id: pageId,
            cleanup: { schema: "elastos.browser.cleanup-handle/v1", id: cleanupId },
            engine_page: { page_id: pageId, adapter: "vz", engine: "chromium" } },
          lifecycle: { sessions: [{ page_id: hash(pageId), profile_key_hash: "profile", exit_id: "local", phase: "ACTIVE_SESSION" }] },
        },
        video: { present: true, hidden: false, paused: false, ready_state: 4,
          video_width: 800, video_height: 600, client_width: 800, client_height: 600,
          decoded_frames: frames, video_bytes_received: bytes },
      };
      options.changeState?.(raw, { typeSent, approved, humanSent });
      return raw;
    },
    async readReceipt() {
      const raw = { schema: "elastos.browser.journey-receipt/v1", run: fixtureRun, events: structuredClone(events) };
      options.changeReceipt?.(raw, { typeSent, humanSent });
      return raw;
    },
    async humanInput(text, budget) {
      calls.push({ kind: "human" });
      assert.equal(text, "-human"); assert.ok(budget.signal);
      if (options.humanThrows) throw new Error(capSecret);
      if (!options.noHandoff) revoked = true;
      humanSent = true;
      for (const character of text) emit(character);
    },
    async closeWindow(budget) {
      calls.push({ kind: "close" }); closed = true;
      assert.ok(budget.signal); assert.ok(budget.timeoutMs <= 30000);
      if (options.closeHangs) return new Promise(() => {});
      const receipt = { schema: "elastos.browser.close-result/v1", page_id: pageId, browser_instance: instance,
        cleanup_id: cleanupId, closed: true, already_closed: false,
        cleanup: { schema: "elastos.browser.runtime-session-cleanup/v1", ok: true },
        terminal_effects: Object.fromEntries(effects.map(key => [key, true])),
        transport_proof: { schema: "elastos.browser.vz-transport-public-proof/v1", page_id: pageId } };
      const binding = { pageId, cleanupId, generation: 1, requestId: "close-id" };
      const result = { receipt, window_detached: true,
        sessions_after_close: { schema: "elastos.browser.session-capacity/v1", recoverable_page: null,
          active_sessions: 0, total_sessions: 0, principal_sessions: 0, launching_sessions: 0,
          engine_cleanup_obligations: 0, launch_reconciliation_obligations: 0 },
        close_evidence: {
          messages: [{ ...binding, state: "pending" }, { ...binding, state: "terminal", terminalKind: "closed" }],
          close_responses: [{ status: 200, authority_matches: true, receipt: structuredClone(receipt),
            request: { cleanup_id: cleanupId, browser_instance: instance } }],
        } };
      options.changeClose?.(result); return result;
    },
    async fetchImpl(rawUrl, init) {
      const target = new URL(rawUrl), body = init.body ? JSON.parse(init.body) : null;
      calls.push({ kind: "http", url: target.href, method: init.method, body, headers: init.headers });
      assert.ok(init.signal); assert.equal(init.redirect, "error");
      const collection = `/api/apps/browser/pages/${encodeURIComponent(pageId)}/operator-requests`;
      let response, status = 200;
      if (target.pathname === "/api/auth/attach") {
        assert.equal(target.origin, args.runtimeCoords.api_url);
        assert.deepEqual(body, { secret: attachSecret, scope: "client" });
        assert.equal(init.headers.Authorization, undefined); assert.equal(init.headers["x-elastos-home-token"], undefined);
        response = { token: sessionSecret, session_type: "capsule" };
      } else {
        assert.equal(target.origin, args.runtimeOrigin);
        const isOwner = init.headers["x-elastos-home-token"] === ownerSecret;
        if (isOwner) { assert.equal(init.headers.Origin, "null"); assert.equal(init.headers.Authorization, undefined); }
        else { assert.equal(init.headers.Authorization, `Bearer ${sessionSecret}`); assert.equal(init.headers.Cookie, undefined); }
        if (target.pathname.endsWith("/inspect")) {
          assert.ok(isOwner);
          response = init.method === "GET" ? { schema: "elastos.browser.inspect-capabilities/v1", page_id: pageId, formats: ["accessibility_tree"] } :
            { schema: "elastos.browser.inspect-result/v1", page_id: pageId, snapshot_id: snapshot,
              document_generation: generation, next_cursor: null, truncated: false,
              nodes: [{ ref: `${snapshot}:0`, role: "textbox", name: "Test text", value: "" }] };
        } else if (target.pathname === collection) {
          if (init.method === "POST") {
            assert.equal(isOwner, false); assert.equal(body.max_actions, 3);
            requested = body; response = admission("pending");
          } else {
            assert.ok(isOwner);
            response = { schema: "elastos.browser.operator-pending/v1", requests: [
              { request_id: requestId, operator_session_id: "session-id", request: requested }] };
          }
        } else if (target.pathname === `${collection}/${requestId}`) {
          if (init.method === "POST") {
            assert.ok(isOwner); approved = true;
            if (options.approvalHangs) return new Promise(() => {});
            response = { ...admission("active"), writer_acquired: true };
          } else if (init.method === "DELETE") {
            assert.ok(isOwner);
            if (options.revokeThrows) throw new Error(ownerSecret);
            if (options.revokeHangs) return new Promise(() => {});
            revoked = true; response = { ...admission("revoked"), writer_acquired: false };
          } else response = { ...admission(revoked ? "revoked" : "active"), writer_acquired: !revoked,
            capability: revoked ? null : capSecret, receipts: [] };
        } else if (target.pathname.endsWith("/input")) {
          assert.equal(isOwner, false); assert.equal(init.headers["x-elastos-capability"], capSecret);
          assert.equal(body.event.ref, `${snapshot}:0`); assert.equal(body.event.document_generation, generation);
          assert.match(body.event.request_id, /^[a-f0-9]{32}$/); assert.ok(approved);
          if (revoked) {
            status = 403; response = { schema: "elastos.browser.operator-error/v1", code: "operator_admission_inactive" };
          } else {
            if (body.event.action === "type") { typeSent = true; if (!options.noTypedEffect) emit(body.event.text); }
            response = { schema: "elastos.browser.ref-input-result/v1", page_id: pageId,
              request_id: body.event.request_id, admission_id: requestId, document_generation: generation,
              accepted: true, outcome: "completed" };
          }
        } else assert.fail("Unexpected route");
      }
      options.changeResponse?.(response, { body, method: init.method, path: target.pathname, typeSent });
      if (options.overdueType && body?.event?.action === "type" && !revoked) timer.jump(3100);
      if (options.hugeResponse && target.pathname.endsWith("/inspect")) response = { ignored: "x".repeat(32768) };
      return new Response(JSON.stringify(response), { status });
    },
  };
  return { args, calls, get closed() { return closed; }, run: () => timer.run(runBrowserOperatorJourney(args)) };
}

test("separate attach, scoped owner approval, exact ref actions, ordinary takeover, revoke and exact close", async () => {
  const f = fixture(), result = await f.run();
  assert.equal(result.ok, true); assert.equal(result.close.effects, 13);
  assert.equal(result.revoked_input_rejected, true);
  const inputs = f.calls.filter(c => c.kind === "http" && c.url.endsWith("/input"));
  assert.deepEqual(inputs.map(c => c.body.event.action), ["click", "type", "type"]);
  assert.equal(new Set(inputs.map(c => c.body.event.request_id)).size, 3);
  assert.deepEqual(f.calls.filter(c => ["human", "close"].includes(c.kind)).map(c => c.kind), ["human", "close"]);
  const text = JSON.stringify(result);
  for (const secret of [ownerSecret, attachSecret, sessionSecret, capSecret, `Operator-${fixtureRun.slice(0, 8)}`, cleanupId]) assert.ok(!text.includes(secret));
});

const failures = [
  ["foreign fixture", { changeReceipt: r => { r.run = "other-run"; } }, "fixture_invalid"],
  ["nonempty baseline", { changeReceipt: r => { r.events.at(-1).value = "already typed"; } }, "baseline_textbox_not_empty"],
  ["false native value", { changeResponse: r => { if (r.nodes) r.nodes[0].value = "wrong"; } }, "inspection_textbox_mismatch"],
  ["foreign native page", { changeResponse: r => { if (r.nodes) r.page_id = "foreign"; } }, "inspection_invalid"],
  ["wrong approved scope", { changeResponse: r => { if (r.requests) r.requests[0].request = { ...r.requests[0].request, max_actions: 16 }; } }, "approval_scope_mismatch"],
  ["owner changes after approval", { changeState: (r, s) => { if (s.approved) r.viewer.browser_instance = "replaced"; } }, "owner_changed"],
  ["uncertain click is never replayed", { changeResponse: (r, c) => { if (c.body?.event?.action === "click") { r.accepted = false; r.outcome = "uncertain"; } } }, "input_not_completed"],
  ["foreign input receipt", { changeResponse: r => { if (r.outcome) r.request_id = "foreign"; } }, "input_not_completed"],
  ["HTTP success without fixture effect", { noTypedEffect: true }, "input_or_video_unproven"],
  ["frames stop at type dispatch", { freezeAfterType: true }, "input_or_video_unproven"],
  ["bytes never advance", { freezeBytes: true }, "input_or_video_unproven"],
  ["human effect without handoff", { noHandoff: true }, "admission_mismatch"],
  ["frames stop at human dispatch", { freezeAfterHuman: true }, "input_or_video_unproven"],
  ["human callback fails", { humanThrows: true }, "request_or_callback_failed"],
  ["approval ACK hangs after acquiring", { approvalHangs: true }, "deadline"],
  ["type ACK exceeds monotonic budget before timer callback", { overdueType: true }, "deadline"],
  ["oversized response", { hugeResponse: true }, "response_too_large"],
  ["revocation throws", { revokeThrows: true }, "request_or_callback_failed"],
  ["revocation hangs", { revokeHangs: true }, "deadline"],
  ["state callback hangs", { stateHangs: true }, "deadline"],
  ["close callback hangs", { closeHangs: true }, "close_unconfirmed"],
];
for (const [name, options, code] of failures) test(`${name}: fails with cleanup and sanitized evidence`, async () => {
  const f = fixture(options);
  await assert.rejects(f.run(), error => {
    assert.equal(error.message, code);
    assert.equal(error.evidence.ok, false); assert.equal(error.evidence.close.attempted, true);
    for (const secret of [ownerSecret, attachSecret, sessionSecret, capSecret]) assert.ok(!JSON.stringify(error.evidence).includes(secret));
    return true;
  });
  assert.equal(f.closed, true);
  const successfulInputs = f.calls.filter(c => c.kind === "http" && c.body?.event?.text !== "-denied" && c.url.endsWith("/input"));
  assert.ok(successfulInputs.length <= 2, "no input replay after a failure");
});

for (const [name, changeClose] of [
  ["tombstone", r => { r.close_evidence.close_responses[0].receipt.already_closed = true; }],
  ["foreign authority", r => { r.close_evidence.close_responses[0].authority_matches = false; }],
  ["foreign cleanup owner", r => { r.receipt.cleanup_id = "other"; r.close_evidence.close_responses[0].receipt.cleanup_id = "other"; }],
  ["missing pending", r => { r.close_evidence.messages.shift(); }],
  ["different request generation", r => { r.close_evidence.messages[1].generation++; }],
  ["retained transport", r => { r.receipt.terminal_effects.turn_process_absent = false; }],
  ["retained obligation", r => { r.sessions_after_close.engine_cleanup_obligations = 1; }],
]) test(`close rejects ${name}`, async () => {
  const f = fixture({ changeClose });
  await assert.rejects(f.run(), /close_unconfirmed/);
  assert.equal(f.closed, true);
});

test("stage logging failure cannot prevent revoke or close", async () => {
  const f = fixture({ stageThrows: true });
  const result = await f.run();
  assert.equal(result.ok, true); assert.equal(result.stage_log_failed, true);
  assert.equal(result.revocation.ok, true); assert.equal(f.closed, true);
});
