import { createHash, randomBytes } from "node:crypto";
import { browserJourneyBinding, browserJourneyVideo } from "./browser-journey-recovery.mjs";

const id = () => randomBytes(16).toString("hex");
const hex = value => typeof value === "string" && /^[a-f0-9]{32}$/.test(value);
const hash = value => `sha256:${createHash("sha256").update(value).digest("hex").slice(0, 16)}`;
const effects = ["page_absent", "child_absent", "vm_absent", "route_absent", "socket_absent",
  "transport_session_absent", "turn_process_absent", "turn_listener_absent", "turn_relay_ports_absent",
  "ordinary_vsock_bridge_absent", "media_vsock_bridge_absent", "bootstrap_vsock_bridge_absent", "hibernation_state_absent"];
class OperatorFailure extends Error {}
function requireEvidence(ok, code) { if (!ok) throw new OperatorFailure(code); }

/**
 * Representative installed B04 probe, called after the ordinary controlled
 * journey returns to /main (empty textbox), before its outer close finally.
 * No side effects on import. Parent supplies the exact Runtime coordinates and
 * Browser launch token; only the owner requests carry that token. The separately
 * attached client receives a short owner-approved capability via Runtime.
 *
 * readState uses the existing viewer-reload shape {sessions,page_status,viewer,
 * video}; readReceipt uses the existing task fixture. humanInput(text,budget)
 * types through #browser-keyboard-capture, never through a direct input API.
 * closeWindow(budget) calls the existing closeControlledBrowserWindow with the
 * exact expectedPageId, and assigns its return to the enclosing journey.close.
 * Callbacks must honor {signal,timeoutMs,deadlineMs}. Close runs even if admission,
 * input, revocation, or observation fails. The enclosing finally can retry a
 * failed close; skip its duplicate close when journey.close is already present.
 * This exercises API approval; the human approval UI remains a separate gate.
 */
export async function runBrowserOperatorJourney({ runtimeOrigin, runtimeCoords, homeToken,
  pageId, expectedUrl, readState, readReceipt, humanInput, closeWindow, markStage = () => {},
  requireVzTransport = false, fetchImpl = fetch,
  clock = { now: () => performance.now(), setTimeout, clearTimeout },
}) {
  const start = clock.now(), deadline = start + 20_000;
  const evidence = { schema: "elastos.browser.journey-operator/v1", ok: false,
    approval: "owner_api", attached_session_lifetime: "runtime_process", steps: [], actions: [], revocation: { attempted: false, ok: false },
    close: { attempted: false, ok: false } };
  let stage = "setup", admissionId, capability, sessionToken, original, initialDocument, failure;
  let route, inputPath, fixturePage, fixtureRun;
  const step = name => {
    stage = name;
    try { markStage(`browser:operator-${name}`); } catch { evidence.stage_log_failed = true; }
  };
  async function within(end, action, cap = 3000) {
    const callDeadline = Math.min(end, clock.now() + cap);
    const timeoutMs = callDeadline - clock.now();
    requireEvidence(timeoutMs > 0, "deadline");
    const controller = new AbortController();
    let timer;
    try {
      const result = await Promise.race([
        Promise.resolve().then(() => {
          requireEvidence(!controller.signal.aborted && clock.now() < callDeadline, "deadline");
          return action({ signal: controller.signal, deadlineMs: callDeadline, timeoutMs });
        }),
        new Promise((_, reject) => { timer = clock.setTimeout(() => {
          controller.abort(); reject(new OperatorFailure("deadline"));
        }, timeoutMs); }),
      ]);
      requireEvidence(clock.now() < callDeadline && !controller.signal.aborted, "deadline");
      return result;
    } finally { clock.clearTimeout(timer); controller.abort(); }
  }
  async function request(url, headers, { method = "GET", body, end = deadline } = {}) {
    return within(end, async ({ signal }) => {
      const response = await fetchImpl(url, { method, signal, redirect: "error",
        headers: { ...headers, ...(body == null ? {} : { "content-type": "application/json" }) },
        ...(body == null ? {} : { body: JSON.stringify(body) }) });
      requireEvidence(response.body, "response_body_missing");
      const reader = response.body.getReader();
      let size = 0;
      const chunks = [];
      try {
        for (;;) {
          const { done, value } = await reader.read();
          if (done) break;
          size += value.byteLength;
          requireEvidence(size <= 32768, "response_too_large");
          chunks.push(value);
        }
      } finally { await reader.cancel().catch(() => {}); reader.releaseLock(); }
      let result;
      try { result = JSON.parse(Buffer.concat(chunks).toString("utf8")); }
      catch { throw new OperatorFailure("response_invalid"); }
      evidence.steps.push({ stage, at_ms: Math.round(clock.now() - start), status: response.status });
      return { status: response.status, body: result };
    });
  }
  const owner = (path, options) => request(new URL(path, runtimeOrigin),
    { Origin: "null", "x-elastos-home-token": homeToken }, options);
  const operator = (path, options) => request(new URL(path, runtimeOrigin),
    { Authorization: `Bearer ${sessionToken}`, ...(path === inputPath ? { "x-elastos-capability": capability } : {}) }, options);
  function admission(response, status) {
    requireEvidence(response.status === 200 && response.body?.schema === "elastos.browser.operator-admission/v1" &&
      response.body.request_id === admissionId && response.body.status === status, "admission_mismatch");
    return response.body;
  }
  async function state(end = deadline) {
    const raw = await within(end, readState);
    let binding, video;
    try {
      binding = browserJourneyBinding({ ...raw, ...raw.viewer });
      video = browserJourneyVideo(raw.video, true);
    } catch { throw new OperatorFailure("owner_or_video_invalid"); }
    requireEvidence(raw.viewer?.page_id === pageId && raw.viewer.actual_url === expectedUrl &&
      Number.isFinite(raw.viewer.document_id) && raw.viewer.document_id > 0 && video.visible, "viewer_mismatch");
    if (original) requireEvidence(JSON.stringify(binding) === JSON.stringify(original) &&
      raw.viewer.document_id === initialDocument, "owner_changed");
    else { original = binding; initialDocument = raw.viewer.document_id; }
    return video;
  }
  async function receipt(end = deadline) {
    const raw = await within(end, readReceipt);
    requireEvidence(raw?.schema === "elastos.browser.journey-receipt/v1" && raw.run === fixtureRun &&
      Array.isArray(raw.events) && raw.events.length > 0 && raw.events.length <= 128 &&
      raw.events.every((e, i) => Number.isSafeInteger(e.sequence) && e.sequence > (raw.events[i - 1]?.sequence || 0)), "fixture_invalid");
    return raw.events;
  }
  async function inputEvidence(expected, afterSequence, beforeVideo) {
    const end = Math.min(deadline, clock.now() + 5000);
    try {
      while (clock.now() < end) {
        const events = await receipt(end), video = await state(end);
        const event = events.find(e => e.sequence > afterSequence && e.type === "input" &&
          e.page === fixturePage && e.value === expected);
        if (event && video.decoded_frames > beforeVideo.decoded_frames &&
          video.video_bytes_received > beforeVideo.video_bytes_received) {
          return { sequence: event.sequence, value_length: expected.length, value_hash: hash(expected),
            decoded_frames: video.decoded_frames, video_bytes_received: video.video_bytes_received };
        }
        await within(end, ({ signal }) => new Promise(resolve => {
          const finish = () => { clock.clearTimeout(timer); signal.removeEventListener("abort", finish); resolve(); };
          const timer = clock.setTimeout(finish, 100);
          signal.addEventListener("abort", finish, { once: true });
        }));
      }
    } catch (error) {
      if (!(error instanceof OperatorFailure && error.message === "deadline" && clock.now() >= end)) throw error;
    }
    throw new OperatorFailure("input_or_video_unproven");
  }
  function completed(response, event) {
    const body = response.body;
    requireEvidence(response.status === 200 && body?.schema === "elastos.browser.ref-input-result/v1" &&
      body.page_id === pageId && body.admission_id === admissionId && body.request_id === event.request_id &&
      body.document_generation === event.document_generation && body.accepted === true &&
      body.outcome === "completed", "input_not_completed");
    evidence.actions.push({ action: event.action, request_hash: hash(event.request_id), accepted: true });
  }
  async function revoke() {
    evidence.revocation.attempted = true;
    const body = admission(await owner(`${route}/${encodeURIComponent(admissionId)}`,
      { method: "DELETE", end: clock.now() + 3000 }), "revoked");
    requireEvidence(body.writer_acquired === false, "writer_not_released");
    evidence.revocation.ok = true;
  }
  try {
    const origin = new URL(runtimeOrigin), attach = new URL(runtimeCoords?.api_url), fixture = new URL(expectedUrl);
    requireEvidence(origin.href === `${origin.origin}/` && origin.protocol === "http:" &&
      ["localhost", "127.0.0.1", "[::1]"].includes(origin.hostname) &&
      attach.origin + "/" === attach.href && attach.protocol === "http:" && attach.hostname === "127.0.0.1" &&
      typeof runtimeCoords.attach_secret === "string" && runtimeCoords.attach_secret.length > 0 &&
      runtimeCoords.runtime_kind === "gateway" && typeof homeToken === "string" && homeToken.length > 0 &&
      typeof pageId === "string" && pageId.length > 0 && pageId.length <= 256 &&
      ["/main", "/nav"].includes(fixture.pathname) && /^[a-zA-Z0-9_-]{8,64}$/.test(fixture.searchParams.get("run")), "setup_invalid");
    route = `/api/apps/browser/pages/${encodeURIComponent(pageId)}/operator-requests`;
    inputPath = `/api/apps/browser/pages/${encodeURIComponent(pageId)}/input`;
    fixturePage = fixture.pathname.slice(1); fixtureRun = fixture.searchParams.get("run");
    await state();
    const initialEvents = await receipt();
    requireEvidence(initialEvents.at(-1).page === fixturePage && initialEvents.at(-1).value === "",
      "baseline_textbox_not_empty");
    step("inspect");
    const inspectPath = `/api/apps/browser/pages/${encodeURIComponent(pageId)}/inspect`;
    const capabilities = await owner(inspectPath);
    requireEvidence(capabilities.status === 200 && capabilities.body?.schema === "elastos.browser.inspect-capabilities/v1" &&
      capabilities.body.page_id === pageId && capabilities.body.formats?.includes("accessibility_tree"), "inspection_unsupported");
    let cursor = null, snapshotId, generation, textbox;
    const seen = new Set();
    for (let count = 0; count < 8; count++) {
      const response = await owner(inspectPath, { method: "POST",
        body: { schema: "elastos.browser.inspect-request/v1", limit: 64, cursor } });
      const body = response.body;
      requireEvidence(response.status === 200 && body?.schema === "elastos.browser.inspect-result/v1" &&
        body.page_id === pageId && hex(body.snapshot_id) && hex(body.document_generation) &&
        Array.isArray(body.nodes) && body.nodes.length <= 64 && body.truncated === false, "inspection_invalid");
      snapshotId ||= body.snapshot_id; generation ||= body.document_generation;
      requireEvidence(body.snapshot_id === snapshotId && body.document_generation === generation, "inspection_changed");
      for (const node of body.nodes) {
        requireEvidence(typeof node.ref === "string" && node.ref.startsWith(`${snapshotId}:`) &&
          /^(0|[1-9][0-9]{0,2})$/.test(node.ref.slice(33)) && Number(node.ref.slice(33)) < 512 &&
          !seen.has(node.ref), "inspection_ref_invalid");
        seen.add(node.ref);
        if (node.role === "textbox" && node.name === "Test text") {
          requireEvidence(!textbox && node.value === "", "inspection_textbox_mismatch"); textbox = node;
        }
      }
      cursor = body.next_cursor;
      requireEvidence(cursor === null || (typeof cursor === "string" && cursor === `${snapshotId}:${seen.size}`), "cursor_invalid");
      if (cursor === null) break;
    }
    requireEvidence(cursor === null && textbox, "inspection_incomplete");
    evidence.inspection = { node_count: seen.size, snapshot_hash: hash(snapshotId), generation_hash: hash(generation) };
    step("attach");
    const attached = await request(new URL("/api/auth/attach", attach), {},
      { method: "POST", body: { secret: runtimeCoords.attach_secret, scope: "client" } });
    requireEvidence(attached.status === 200 && attached.body?.session_type === "capsule" &&
      typeof attached.body.token === "string" && attached.body.token.length > 0 && attached.body.token.length <= 128 &&
      attached.body.token !== homeToken, "attach_invalid");
    sessionToken = attached.body.token;
    step("request");
    const requested = { schema: "elastos.browser.operator-request/v1", document_generation: generation,
      actions: ["click", "type"], duration_ms: 30000, max_actions: 3, reason: "Controlled Browser operator journey" };
    const pending = await operator(route, { method: "POST", body: requested });
    requireEvidence(typeof pending.body?.request_id === "string" && pending.body.request_id.length > 0 &&
      pending.body.request_id.length <= 64, "admission_id_missing");
    admissionId = pending.body.request_id; admission(pending, "pending");
    const review = await owner(route);
    const reviewed = review.body?.requests?.find(row => row.request_id === admissionId);
    requireEvidence(review.status === 200 && review.body?.schema === "elastos.browser.operator-pending/v1" &&
      reviewed && Object.keys(requested).every(k => JSON.stringify(reviewed.request?.[k]) === JSON.stringify(requested[k])), "approval_scope_mismatch");
    step("approve");
    requireEvidence(admission(await owner(`${route}/${encodeURIComponent(admissionId)}`, { method: "POST" }), "active")
      .writer_acquired === true, "writer_not_acquired");
    const active = admission(await operator(`${route}/${encodeURIComponent(admissionId)}`), "active");
    requireEvidence(active.writer_acquired === true && typeof active.capability === "string" &&
      active.capability.length > 0 && active.capability.length <= 8192, "capability_missing");
    capability = active.capability;
    evidence.admission_hash = hash(admissionId);
    await state();
    step("click");
    const event = { schema: "elastos.browser.ref-input/v1", admission_id: admissionId,
      document_generation: generation, ref: textbox.ref, action: "click", request_id: id() };
    completed(await operator(inputPath, { method: "POST", body: { event } }), event);
    const beforeOperatorVideo = await state();
    step("type");
    const operatorText = `Operator-${fixtureRun.slice(0, 8)}`;
    const typed = { ...event, request_id: id(), action: "type", text: operatorText };
    completed(await operator(inputPath, { method: "POST", body: { event: typed } }), typed);
    evidence.operator_input = await inputEvidence(operatorText, initialEvents.at(-1).sequence, beforeOperatorVideo);
    // Unused quota and an active lease make this a real owner takeover, rather
    // than a successful human input after automatic operator expiry.
    const beforeHuman = admission(await operator(`${route}/${encodeURIComponent(admissionId)}`), "active");
    requireEvidence(beforeHuman.writer_acquired === true, "handoff_not_active");
    const beforeHumanVideo = await state();
    step("human-handoff");
    await within(deadline, budget => humanInput("-human", budget));
    evidence.human_input = await inputEvidence(`${operatorText}-human`, evidence.operator_input.sequence, beforeHumanVideo);
    const handedOff = admission(await operator(`${route}/${encodeURIComponent(admissionId)}`), "revoked");
    requireEvidence(handedOff.writer_acquired === false && handedOff.capability === null, "handoff_unproven");
    step("revoke");
    await revoke();
    step("revoked-input");
    const rejected = await operator(inputPath, { method: "POST",
      body: { event: { ...typed, request_id: id(), text: "-denied" } } });
    requireEvidence(rejected.status === 403 && rejected.body?.schema === "elastos.browser.operator-error/v1" &&
      rejected.body.code === "operator_admission_inactive", "revoked_input_accepted");
    const finalEvents = await receipt();
    requireEvidence(finalEvents.at(-1).value === `${operatorText}-human`, "revoked_input_changed_page");
    evidence.revoked_input_rejected = true;
    await state();
  } catch (error) {
    failure = error instanceof OperatorFailure ? error.message : "request_or_callback_failed";
    evidence.failure_stage = stage;
  } finally {
    if (admissionId && !evidence.revocation.ok) {
      try { step("cleanup-revoke"); await revoke(); }
      catch { evidence.revocation.failure = "revocation_unconfirmed"; failure ||= "revocation_unconfirmed"; }
    }
    try {
      step("close"); evidence.close.attempted = true;
      // The existing UI closer owns its longer cleanup budget. No probe request
      // or failed revocation can prevent this independent final action.
      const closed = await within(clock.now() + 30000, closeWindow, 30000);
      const raw = closed?.receipt, observed = closed?.close_evidence?.close_responses?.find(r =>
        r.status === 200 && r.authority_matches === true && r.receipt?.page_id === pageId);
      const response = observed?.receipt;
      requireEvidence(closed?.window_detached === true && raw?.schema === "elastos.browser.close-result/v1" &&
        raw.closed === true && raw.page_id === pageId && original &&
        raw.cleanup_id === original[2] && raw.browser_instance === original[9] &&
        response?.schema === "elastos.browser.close-result/v1" && response.closed === true && response.already_closed === false &&
        response.cleanup_id === raw.cleanup_id && response.browser_instance === raw.browser_instance &&
        observed.request?.cleanup_id === raw.cleanup_id && observed.request.browser_instance === raw.browser_instance &&
        raw.cleanup?.schema === "elastos.browser.runtime-session-cleanup/v1" && raw.cleanup.ok === true &&
        effects.slice(0, 5).every(key => raw.terminal_effects?.[key] === true), "close_unproven");
      const messages = closed.close_evidence?.messages || [];
      const terminal = messages.find(m => m.state === "terminal" && m.terminalKind === "closed" &&
        m.pageId === pageId && m.cleanupId === raw.cleanup_id);
      requireEvidence(terminal && typeof terminal.requestId === "string" && terminal.requestId &&
        Number.isSafeInteger(terminal.generation) && terminal.generation >= 0 && messages.some(m =>
          m.state === "pending" && m.pageId === pageId && m.cleanupId === raw.cleanup_id &&
          m.requestId === terminal.requestId && m.generation === terminal.generation), "close_handshake_unproven");
      if (requireVzTransport) requireEvidence(raw.transport_proof?.schema === "elastos.browser.vz-transport-public-proof/v1" &&
        raw.transport_proof.page_id === pageId && effects.every(key => raw.terminal_effects?.[key] === true), "vz_cleanup_unproven");
      requireEvidence(closed.sessions_after_close?.schema === "elastos.browser.session-capacity/v1" &&
        closed.sessions_after_close.recoverable_page === null &&
        ["active_sessions", "total_sessions", "principal_sessions", "launching_sessions", "engine_cleanup_obligations",
          "launch_reconciliation_obligations"].every(key => closed.sessions_after_close[key] === 0), "runtime_cleanup_unproven");
      evidence.close = { attempted: true, ok: true, effects: requireVzTransport ? 13 : 5 };
    } catch { evidence.close.failure = "close_unconfirmed"; failure ||= "close_unconfirmed"; evidence.failure_stage ||= stage; }
    capability = null; sessionToken = null;
  }
  evidence.ok = !failure;
  if (failure) throw Object.assign(new OperatorFailure(failure), { evidence });
  return evidence;
}
