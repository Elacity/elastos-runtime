import assert from "node:assert/strict";
import test from "node:test";
import { readFileSync } from "node:fs";
import { createFillFixture } from "./browser-operator-fill-fixture.mjs";
import { createHash } from "node:crypto";
import { createBrowserOperatorClient } from "./browser-operator-client.mjs";
import { createCamofoxOperatorAdapter, CAMOFOX_CLIENT } from "./browser-operator-client-camofox.mjs";
import { runTool, adaptResponse } from "./browser-operator-client-fixtures/camofox/tool-contracts.mjs";
import { createOperatorApprovalController, operatorApprovalSummary, mountOperatorApproval } from "../../capsules/browser/browser/browser-operator-approval.js";

const origin = "https://runtime.invalid", adapterOrigin = "https://adapter.invalid";
const pageId = "page:existing", admissionId = "c".repeat(32), session = "operator-session";
const accessKey = "operator-adapter-access-key", userId = "operator-routing-label", tabId = "runtime-page";
const generation = "a".repeat(32), snapshotId = "b".repeat(32);
const ref = `${snapshotId}:0`;
const directory = new URL("./browser-operator-client-fixtures/camofox/", import.meta.url);

function fixture(options = {}) {
  const calls = [], wire = [];
  let phase = options.invitation ? "pending" : "active", currentGeneration = generation, readCount = 0;
  let now = 0;
  const fetchImpl = async (url, init) => {
    assert.ok(url.startsWith(origin + "/api/apps/browser/pages/page%3Aexisting/"));
    assert.equal(init.redirect, "error"); assert.equal(init.credentials, "omit");
    assert.equal(init.headers.Authorization, `Bearer ${session}`);
    assert.equal(init.headers["x-elastos-home-token"], undefined);
    const path = new URL(url).pathname, body = init.body ? JSON.parse(init.body) : null;
    calls.push({ path, method: init.method, body, capability: init.headers["x-elastos-capability"] });
    if (options.fetch) return options.fetch(url, init);
    if (path.endsWith("/operator-requests") && init.method === "POST") {
      assert.equal(body.document_generation, generation); assert.equal(body.inspect, true);
      return Response.json({ schema: "elastos.browser.operator-admission/v1", request_id: admissionId, status: "pending" });
    }
    if (path.endsWith("/operator-requests/" + admissionId)) {
      return Response.json({ schema: "elastos.browser.operator-admission/v1", request_id: admissionId, status: phase,
        capability: phase === "active" ? "write-capability" : null,
        inspection_capability: phase === "active" && !options.missingReadGrant ? "read-capability" : null,
        receipts: options.receipts || [] });
    }
    if (path.endsWith("/inspect")) {
      assert.equal(init.headers["x-elastos-capability"], "read-capability");
      assert.equal(body.schema, "elastos.browser.inspect-request/v1"); assert.equal(body.limit, 64);
      const offset = body.cursor ? Number(body.cursor.split(":")[1]) : 0;
      const count = options.nodes ?? 65;
      const value = { schema: "elastos.browser.inspect-result/v1", page_id: pageId,
        snapshot_id: snapshotId, document_generation: currentGeneration,
        nodes: Array.from({ length: Math.min(64, count - offset) }, (_, i) => ({ ref: `${snapshotId}:${offset+i}`,
          role: "textbox", name: options.largeNames ? "x".repeat(300) : `Field ${offset+i}`, description: "", value: options.engine?.field.value ?? "" })),
        next_cursor: offset + 64 < count ? `${snapshotId}:${offset+64}` : null, truncated: false };
      options.changeSnapshot?.(value, ++readCount);
      return Response.json(value);
    }
    if (path.endsWith("/input")) {
      assert.equal(init.headers["x-elastos-capability"], "write-capability");
      assert.equal(body.event.document_generation, currentGeneration);
      if (options.inputFailure) throw new Error("private transport location");
      if (options.engine) {
        try { return Response.json({ ...await options.engine.input(body.event), outcome: "completed" }); }
        catch (error) { return Response.json({ code: error.code || "operator_outcome_uncertain" }, { status: 409 }); }
      }
      const result = { schema: "elastos.browser.ref-input-result/v1", page_id: pageId, admission_id: admissionId,
        document_generation: generation, request_id: body.event.request_id, accepted: true, outcome: "completed" };
      options.changeInput?.(result); return Response.json(result);
    }
    if (path.endsWith("/detach")) {
      phase = "revoked";
      return Response.json({ schema: "elastos.browser.operator-admission/v1", request_id: admissionId,
        status: "revoked", writer_acquired: false });
    }
    throw new Error("unexpected Runtime route");
  };
  const client = createBrowserOperatorClient({ origin, sessionToken: session, pageId,
    ...(options.invitation ? { invitation: options.invitation } : { admissionId }), fetchImpl,
    ...(options.timeoutMs ? { timeoutMs: options.timeoutMs } : {}) });
  const adapter = createCamofoxOperatorAdapter({ client, accessKey, userId, tabId,
    snapshotProfile: options.strict ? "strict" : "inspection-only-v1", now: () => now });
  async function upstream(name, args = {}, ctx = {}) {
    const previous = globalThis.fetch;
    globalThis.fetch = async (url, init) => {
      assert.ok(url.startsWith(adapterOrigin + "/")); wire.push({ url, init });
      return adapter.handle(new Request(url, init));
    };
    try { return await runTool(name, args, { userId, sessionKey: "test", ...ctx }, adapterOrigin,
      { accessKey, cookiesDir: "/unused" }); }
    finally { globalThis.fetch = previous; }
  }
  return { client, adapter, calls, wire, upstream, expire() { now = 30001; }, revoke() { phase = "revoked"; },
    approve() { phase = "active"; }, navigate() { currentGeneration = "c".repeat(32); } };
}

test("visible owner invitation bootstraps an operator request and real upstream inspection", async () => {
  const owner = { page_id: pageId, generation: 1 }; let f, record;
  const controller = createOperatorApprovalController({ runtimeOrigin: origin, getOwner: () => owner,
    sameOwner: (a, b) => a === b, fetchJson: async (path, options) => {
      if (path.endsWith("/inspect")) return { schema: "elastos.browser.inspect-result/v1", page_id: pageId,
        document_generation: generation, nodes: [{ value: "owner-only page content" }] };
      if (path.endsWith("/operator-requests")) return { schema: "elastos.browser.operator-pending/v1", requests: [record] };
      assert.equal(options.method, "POST"); f.approve();
      return { schema: "elastos.browser.operator-admission/v1", request_id: admissionId, status: "active", writer_acquired: true };
    } });
  const invitation = await controller.invite();
  assert.equal(invitation.document_generation, generation); assert.ok(!JSON.stringify(invitation).includes("owner-only"));
  f = fixture({ invitation });
  await assert.rejects(f.upstream("camofox_snapshot", { tabId }), /operator_admission_required/);
  const pending = await f.client.requestAdmission({ actions: ["click", "fill"] }); assert.equal(pending.status, "pending");
  record = { request_id: admissionId, operator_session_id: session, request: f.calls[0].body };
  assert.equal(record.request.inspect, true); assert.equal(f.calls[0].capability, undefined);
  await assert.rejects(f.upstream("camofox_snapshot", { tabId }), /operator_pending/);
  const reviewed = await controller.pending();
  assert.match(operatorApprovalSummary(reviewed[0]).inspection, /Read page text, labels and form values/);
  await controller.decide(admissionId, true);
  const { spec, payload } = await f.upstream("camofox_snapshot", { tabId });
  assert.equal(JSON.parse(adaptResponse(spec, payload)[0].text).refsCount, 65);
});

test("operator bootstrap rejects expired or foreign-origin invitation metadata", async () => {
  const invitation = { schema: "elastos.browser.operator-invitation/v1", runtime_origin: origin, page_id: pageId,
    document_generation: generation, expires_at_ms: Date.now() - 1 };
  const f = fixture({ invitation });
  await assert.rejects(f.client.requestAdmission(), /operator_invitation_expired/); assert.equal(f.calls.length, 0);
  assert.throws(() => fixture({ invitation: { ...invitation, runtime_origin: "https://foreign.invalid" } }), /operator_configuration_invalid/);
});

test("approval rechecks the exact inspection scope shown to the owner", async () => {
  const row = { request_id: admissionId, operator_session_id: session, request: { schema: "elastos.browser.operator-request/v1",
    document_generation: generation, inspect: false, actions: ["click"], duration_ms: 30000, max_actions: 3, reason: "Form" } };
  const owner = { page_id: pageId }; let mutations = 0;
  const controller = createOperatorApprovalController({ runtimeOrigin: origin, getOwner: () => owner,
    sameOwner: (a, b) => a === b, fetchJson: async (_path, options) => {
      if (options.method) mutations++;
      return { schema: "elastos.browser.operator-pending/v1", requests: [structuredClone(row)] };
    } });
  await controller.pending(); row.request.inspect = true;
  await assert.rejects(controller.decide(admissionId, true), /request changed/); assert.equal(mutations, 0);
});

test("approval UI renders inspection meaning separately from input and escapes request text", async () => {
  class Element {
    constructor(tag, ownerDocument) { this.tag = tag; this.ownerDocument = ownerDocument; this.children = []; this.listeners = {}; }
    append(...children) { this.children.push(...children); }
    replaceChildren(...children) { this.children = children; }
    setAttribute() {}
    addEventListener(event, listener) { this.listeners[event] = listener; }
  }
  const doc = { createElement: tag => new Element(tag, doc) }, container = new Element("div", doc);
  const row = { request_id: admissionId, operator_session_id: session, request: { schema: "elastos.browser.operator-request/v1",
    document_generation: generation, inspect: true, actions: ["click", "fill"], duration_ms: 30000, max_actions: 3,
    reason: '<img src="x" onerror="bad()">' } };
  let approved = 0;
  mountOperatorApproval({ container, controller: { pending: async () => [row], decide: async () => { approved++; } } });
  const all = node => [node, ...node.children.flatMap(all)];
  await all(container).find(n => n.textContent === "Check agent requests").listeners.click();
  assert.ok(all(container).some(n => /Read page text, labels and form values/.test(n.textContent)));
  assert.ok(all(container).some(n => /replace or clear the selected field/.test(n.textContent)));
  assert.ok(all(container).some(n => n.textContent === `Agent reason: ${row.request.reason}`));
  assert.ok(all(container).every(n => n.tag !== "img")); assert.equal(approved, 0);
  assert.ok(all(container).some(n => n.tag === "button" && n.textContent === "Allow page inspection and control?"));
});

test("denying a second request preserves the owner's revoke control for the approved agent", async () => {
  const owner = { page_id: pageId }, decisions = [];
  const rows = [admissionId, "other-admission"].map(request_id => ({ request_id, operator_session_id: session,
    request: { schema: "elastos.browser.operator-request/v1", document_generation: generation,
      actions: ["fill"], inspect: true, duration_ms: 30000, max_actions: 3, reason: "Complete the field" } }));
  const controller = createOperatorApprovalController({ runtimeOrigin: origin, getOwner: () => owner,
    sameOwner: (a,b) => a === b, fetchJson: async (path, options) => {
      if (!options.method) return { schema: "elastos.browser.operator-pending/v1", requests: rows };
      const request_id = path.split("/").at(-1); decisions.push([options.method,request_id]);
      return { schema: "elastos.browser.operator-admission/v1", request_id,
        status: options.method === "POST" ? "active" : "revoked", writer_acquired: options.method === "POST" };
    } });
  await controller.pending(); await controller.decide(admissionId, true);
  await controller.decide("other-admission", false); await controller.revoke();
  assert.deepEqual(decisions, [["POST",admissionId],["DELETE","other-admission"],["DELETE",admissionId]]);
});

test("upstream client revision, license and unchanged module bytes are pinned", () => {
  const provenance = JSON.parse(readFileSync(new URL("provenance.json", directory)));
  assert.equal(provenance.revision, CAMOFOX_CLIENT.revision); assert.equal(provenance.client_version, "1.14.0");
  for (const file of provenance.vendored) {
    const bytes = readFileSync(new URL(file.local, directory));
    assert.equal(bytes.length, file.bytes); assert.equal(createHash("sha256").update(bytes).digest("hex"), file.sha256);
  }
});

test("unchanged upstream snapshot call requires screenshot in strict profile", async () => {
  const f = fixture({ strict: true });
  await assert.rejects(f.upstream("camofox_snapshot", { tabId }), /501:.*capability_unsupported.*screenshot/);
  assert.equal(new URL(f.wire[0].url).searchParams.get("includeScreenshot"), "true");
  assert.equal(f.calls.length, 0);
});

test("actual upstream client reads two native pages, shapes restricted snapshot and clicks its ref", async () => {
  const f = fixture();
  const { spec, payload } = await f.upstream("camofox_snapshot", { tabId });
  const content = adaptResponse(spec, payload);
  assert.deepEqual(content.map(c => c.type), ["text"]);
  assert.deepEqual(JSON.parse(content[0].text).runtime.omitted_requested, ["screenshot"]);
  assert.equal(payload.refsCount, 65); assert.equal(payload.hasMore, false);
  const reference = payload.snapshot.match(/\[(e\d+)\]/)[1];
  assert.equal((await f.upstream("camofox_click", { tabId, ref: reference })).payload.ok, true);
  const inspections = f.calls.filter(c => c.path.endsWith("/inspect"));
  assert.deepEqual(inspections.map(c => c.body.cursor), [null, `${snapshotId}:64`]);
  const effect = f.calls.find(c => c.path.endsWith("/input"));
  assert.equal(effect.body.event.ref, ref); assert.equal(effect.body.event.action, "click");
  assert.equal(f.wire[0].init.headers.Authorization, `Bearer ${accessKey}`);
  assert.ok(!JSON.stringify(payload).includes("capability"));
});

test("an admission without an explicit read grant cannot inspect", async () => {
  const f = fixture({ missingReadGrant: true });
  await assert.rejects(f.upstream("camofox_snapshot", { tabId }), /operator_inspection_not_granted/);
  assert.equal(f.calls.length, 1);
});

test("Camofox keyboard, selectors and submit options reject before any Runtime effect", async () => {
  const f = fixture();
  for (const args of [{ text: "keyboard", mode: "keyboard", ref: "e1" },
    { text: "submit", ref: "e1", pressEnter: true }, { text: "value", selector: "#field" },
    { text: "value", ref: "e1", submit: true }]) {
    await assert.rejects(f.upstream("camofox_type", { tabId, ...args }), /501:.*fill_options/);
  }
  assert.equal(f.calls.length, 0);
});

test("unchanged upstream client replaces, clears, inspects and rejects revoked fill through the Engine dispatcher", async () => {
  const engine = createFillFixture();
  try {
    await engine.acquire(); const f = fixture({ engine, nodes: 1 });
    let result = await f.upstream("camofox_snapshot", { tabId });
    assert.match(adaptResponse(result.spec, result.payload)[0].text, /prefilled/);
    result = await f.upstream("camofox_type", { tabId, ref: "e1", text: "Réplaced 🦊" });
    assert.equal(result.payload.ok, true); assert.equal(engine.field.value, "Réplaced 🦊");
    assert.equal(adaptResponse(result.spec, result.payload)[0].type, "text");
    const event = f.calls.find(c => c.path.endsWith("/input")).body.event;
    await engine.input(event); // Identical receipt replay has no second effect.
    assert.equal(engine.calls.filter(c => c.method === "Input.insertText").length, 1);
    result = await f.upstream("camofox_snapshot", { tabId });
    assert.match(adaptResponse(result.spec, result.payload)[0].text, /Réplaced/);
    await f.upstream("camofox_type", { tabId, ref: "e2", text: "", mode: "fill", pressEnter: false });
    assert.equal(engine.field.value, "");
    assert.deepEqual(engine.calls.filter(c => c.method === "Input.dispatchKeyEvent").map(c => c.params.type), ["keyDown", "keyUp"]);
    result = await f.upstream("camofox_snapshot", { tabId });
    assert.ok(!adaptResponse(result.spec, result.payload)[0].text.includes("Réplaced"));
    assert.equal(engine.calls.filter(c => c.method === "Page.createIsolatedWorld").length, 1);
    assert.equal(engine.calls.filter(c => c.method === "Runtime.releaseObject").length, 2);
    await engine.revoke(); f.revoke(); const before = engine.calls.length;
    await assert.rejects(f.upstream("camofox_type", { tabId, ref: "e3", text: "after revoke" }), /operator_revoked/);
    assert.equal(engine.calls.length, before);
  } finally { engine.close(); }
});

for (const condition of ["readonly", "disabled", "inert", "hidden", "detached", "email", "foreign document", "stale snapshot", "deadline"]) {
  test(`actual client fill rejects ${condition} before changing the field`, async () => {
    const engine = createFillFixture();
    try {
      await engine.acquire(); const f = fixture({ engine, nodes: 1 });
      await f.upstream("camofox_snapshot", { tabId });
      if (condition === "readonly") engine.field.readOnly = true;
      if (condition === "disabled") engine.field.disabled = true;
      if (condition === "inert") engine.field.inert = true;
      if (condition === "hidden") engine.field.visible = false;
      if (condition === "detached") engine.field.isConnected = false;
      if (condition === "email") engine.field.type = "email";
      if (condition === "foreign document") engine.frame.loaderId = "new-document";
      if (condition === "stale snapshot") engine.snapshot.expires = 0;
      if (condition === "deadline") engine.hook(method => { if (method === "DOM.resolveNode") engine.advance(1501); });
      await assert.rejects(f.upstream("camofox_type", { tabId, ref: "e1", text: "changed" }));
      assert.equal(engine.field.value, "prefilled");
      assert.equal(engine.calls.filter(c => c.method.startsWith("Input.")).length, 0);
    } finally { engine.close(); }
  });
}

for (const boundary of ["prepare ACK loss", "insert ACK loss", "clear prevented", "revoked during focus"]) {
  test(`actual client fill retains uncertainty and prevents retry after ${boundary}`, async () => {
    const engine = createFillFixture();
    try {
      await engine.acquire(); const f = fixture({ engine, nodes: 1 });
      await f.upstream("camofox_snapshot", { tabId });
      if (boundary === "clear prevented") engine.preventDelete();
      else engine.hook(method => {
        if (method === (boundary === "insert ACK loss" ? "Input.insertText" : "Runtime.callFunctionOn")) {
          if (boundary === "revoked during focus") engine.page.operatorLease.active = false;
          else throw new Error("applied effect, lost response");
        }
      });
      const text = boundary === "clear prevented" ? "" : "replacement";
      await assert.rejects(f.upstream("camofox_type", { tabId, ref: "e1", text }));
      const before = engine.calls.length;
      const receipt = await f.client.reconcile(); assert.ok(receipt.request_id);
      await assert.rejects(f.upstream("camofox_type", { tabId, ref: "e1", text }));
      assert.equal(engine.calls.length, before);
      assert.equal(engine.field.value, boundary === "insert ACK loss" ? "replacement" : "prefilled");
    } finally { engine.close(); }
  });
}

test("native ref type uses existing insertion contract with one bounded request", async () => {
  const f = fixture();
  const result = await f.client.input({ document_generation: generation, ref, action: "type", text: "Insert" });
  assert.equal(result.outcome, "completed");
  assert.equal(f.calls.filter(c => c.path.endsWith("/input")).length, 1);
  await assert.rejects(f.client.input({ document_generation: generation, ref, action: "type", text: "\n" }), /operator_input_invalid/);
});

test("actual upstream close stays distinct from adapter detach", async () => {
  const f = fixture();
  await assert.rejects(f.upstream("camofox_close_tab", { tabId }), /501:.*page_close/);
  assert.equal(f.calls.length, 0);
  const response = await f.adapter.handle(new Request(`${adapterOrigin}/tabs/${tabId}/detach`, { method: "POST",
    headers: { Authorization: `Bearer ${accessKey}` }, body: JSON.stringify({ userId }) }));
  assert.deepEqual(await response.json(), { detached: true, page_closed: false });
  assert.equal(f.calls.length, 1); assert.ok(f.calls[0].path.endsWith("/detach"));
  await assert.rejects(f.upstream("camofox_snapshot", { tabId }), /operator_detached/);
});

test("actual upstream list is limited to the invited page and declares its subset", async () => {
  const f = fixture(); const { payload } = await f.upstream("camofox_list_tabs");
  assert.equal(payload.tabs.length, 1); assert.equal(payload.tabs[0].tabId, tabId);
  assert.ok(payload.runtime.unsupported.includes("close"));
  assert.equal(payload.runtime.fill.max_utf8_bytes, 1024);
  assert.ok(!payload.runtime.fill.input_types.includes("email"));
});

test("foreign routing labels and owner credentials cannot authorize adapter access", async () => {
  const f = fixture();
  await assert.rejects(f.upstream("camofox_snapshot", { tabId }, { userId: "foreign" }), /403:.*adapter_session_mismatch/);
  for (const headers of [{}, { Authorization: "Bearer foreign" },
    { Authorization: `Bearer ${accessKey}`, "x-elastos-home-token": "owner" },
    { Authorization: `Bearer ${accessKey}`, Cookie: "owner-cookie" }]) {
    const r = await f.adapter.handle(new Request(`${adapterOrigin}/tabs?userId=${userId}`, { headers }));
    assert.equal(r.status, 401);
  }
  assert.equal(f.calls.length, 0);
});

test("unsupported selectors, navigation and evaluation cause no dispatch", async () => {
  const f = fixture();
  for (const [name, args] of [["camofox_click", { ref: "e1", selector: "#x" }],
    ["camofox_navigate", { url: "https://site.invalid" }], ["camofox_evaluate", { expression: "document.cookie" }],
    ["camofox_create_tab", { url: "https://site.invalid" }], ["camofox_screenshot", {}]]) {
    await assert.rejects(f.upstream(name, { tabId, ...args }), /501:.*capability_unsupported/);
  }
  assert.equal(f.calls.length, 0);
});

test("fresh snapshots never rebind old Camofox refs", async () => {
  const f = fixture({ nodes: 1 });
  await f.upstream("camofox_snapshot", { tabId }); await f.upstream("camofox_snapshot", { tabId });
  await assert.rejects(f.upstream("camofox_click", { tabId, ref: "e1" }), /stale_inspection/);
  assert.equal((await f.upstream("camofox_click", { tabId, ref: "e2" })).payload.ok, true);
});

test("snapshot expiry and revocation reject cached pagination", async () => {
  for (const action of ["expire", "revoke"]) {
    const f = fixture({ largeNames: true });
    const { payload } = await f.upstream("camofox_snapshot", { tabId });
    assert.equal(payload.hasMore, true); f[action]();
    await assert.rejects(f.upstream("camofox_snapshot", { tabId, offset: payload.nextOffset }),
      /stale_inspection|operator_admission_inactive/);
  }
});

test("actual upstream character offsets reconstruct the restricted snapshot", async () => {
  const f = fixture({ largeNames: true }); let offset, full = "", count = 0, total;
  do {
    const { payload } = await f.upstream("camofox_snapshot", { tabId, offset });
    full += payload.snapshot; total = payload.totalChars; offset = payload.nextOffset;
    assert.ok(++count <= 4);
  } while (offset !== null);
  assert.equal(full.length, total); assert.equal((full.match(/\[e\d+\]/g) || []).length, 65);
});

for (const [name, changeSnapshot] of [
  ["changed generation", (v, i) => { if (i === 2) v.document_generation = "c".repeat(32); }],
  ["foreign page", v => { v.page_id = "foreign"; }],
  ["duplicate ref", v => { v.nodes[1].ref = v.nodes[0].ref; }],
  ["wrong cursor", v => { v.next_cursor = `${snapshotId}:0`; }],
  ["truncated tree", v => { v.truncated = true; }],
]) test(`native inspection rejects ${name}`, async () => {
  const f = fixture({ changeSnapshot });
  await assert.rejects(f.client.inspect(), /operator_snapshot_invalid|stale_inspection/);
});

for (const [name, option] of [["lost input response", { inputFailure: true }],
  ["foreign receipt", { changeInput: r => { r.page_id = "foreign"; } }],
  ["uncertain receipt", { changeInput: r => { r.outcome = "uncertain"; r.accepted = false; } }]]) {
  test(`${name} requires reconciliation and prevents blind replay`, async () => {
    const f = fixture(option), event = { document_generation: generation, ref, action: "click" };
    await assert.rejects(f.client.input(event), /operator_reconciliation_required/);
    await assert.rejects(f.client.input(event), /operator_reconciliation_required/);
    const reconciliation = await f.client.reconcile();
    assert.match(reconciliation.request_id, /^[a-f0-9]{32}$/); assert.equal(reconciliation.requires_new_admission, true);
    assert.equal(f.calls.filter(c => c.path.endsWith("/input")).length, 1);
    await f.client.detach();
    assert.ok(f.calls.every(c => !c.path.endsWith("/close")));
  });
}

test("oversized native response cancels its stream", async () => {
  let cancelled = false;
  const f = fixture({ fetch: () => new Response(new ReadableStream({
    pull(c) { c.enqueue(new Uint8Array(32769)); }, cancel() { cancelled = true; },
  })) });
  await assert.rejects(f.client.status(), /operator_response_too_large/); assert.equal(cancelled, true);
});

test("native request timeout aborts only its own fetch", async () => {
  let aborted = false;
  const f = fixture({ timeoutMs: 10, fetch: (_url, init) => new Promise((_, reject) => {
    init.signal.addEventListener("abort", () => { aborted = true; reject(new Error("aborted")); }, { once: true });
  }) });
  await assert.rejects(f.client.status(), /operator_transport_failed/); assert.equal(aborted, true);
});

test("native inspection stops after eight pages", async () => {
  const f = fixture({ nodes: 513 });
  await assert.rejects(f.client.inspect(), /operator_snapshot_incomplete/);
  assert.equal(f.calls.filter(c => c.path.endsWith("/inspect")).length, 8);
});

test("adapter rejects an oversized request while cancelling the body stream", async () => {
  const f = fixture(); let cancelled = false;
  const body = new ReadableStream({ pull(c) { c.enqueue(new Uint8Array(8193)); }, cancel() { cancelled = true; } });
  const response = await f.adapter.handle(new Request(`${adapterOrigin}/tabs/${tabId}/click`, {
    method: "POST", headers: { Authorization: `Bearer ${accessKey}` }, body, duplex: "half" }));
  assert.equal(response.status, 413); assert.equal(cancelled, true); assert.equal(f.calls.length, 0);
});

test("operator cancellation reaches the owned request", async () => {
  const controller = new AbortController(); let started;
  const ready = new Promise(resolve => { started = resolve; });
  const f = fixture({ fetch: (_url, init) => new Promise((_, reject) => {
    init.signal.addEventListener("abort", () => reject(new Error("cancelled")), { once: true }); started();
  }) });
  const pending = f.client.status({ signal: controller.signal }); await ready; controller.abort();
  await assert.rejects(pending, /operator_transport_failed/);
});

test("operator configuration rejects injected owner credentials and untrusted transport", () => {
  const base = { origin, sessionToken: session, pageId, admissionId };
  for (const extra of [{ ownerToken: "owner" }, { origin: "http://remote.invalid" }, { origin: origin + "/other" },
    { pageId: "../other" }, { sessionToken: "bad\ntoken" }]) {
    assert.throws(() => createBrowserOperatorClient({ ...base, ...extra }), /operator_configuration_invalid/);
  }
});
