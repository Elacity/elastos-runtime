#!/usr/bin/env node
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import vm from "node:vm";

const source = readFileSync(new URL("../capsules/_shared/model-management.js", import.meta.url), "utf8");
for (const capsule of ["system", "marketplace"]) {
  assert.equal(readFileSync(new URL(`../capsules/${capsule}/browser/model-management.js`, import.meta.url), "utf8"), source);
}
const context = { window: {} };
vm.runInNewContext(source.replace("window.ElastosModelManagement = {",
  "window.testFailure = { parseRuntime, operationRuntime, failureText, catalogEntries, selectCatalogEntry, phaseOf, PHASE_TEXT, COMPACT_PHASE_TEXT }; window.ElastosModelManagement = {"), context);
const { parseRuntime, operationRuntime, failureText, catalogEntries, selectCatalogEntry, phaseOf, PHASE_TEXT, COMPACT_PHASE_TEXT } = context.window.testFailure;
const cid = `bafybei${"a".repeat(52)}`;
const preparation = { operation_id: "fixture", cid, state: "failed", total_bytes: 1024,
  completed_bytes: 512, cancel_requested: false, admitted: false, activation_pending: false };
const runtime = { admitted: false, kept: false, dispatch_ready: false, offer_id: null, preparation };
for (const [code, message] of Object.entries({
  authorization_unavailable: "Preparation stopped. Open Models again to check access.",
  policy_unavailable: "Preparation stopped. Model policy is unavailable.",
  capacity_unavailable: "Preparation stopped. Storage capacity is unavailable.",
  content_unavailable: "Preparation failed. Model content could not be read.",
  local_storage_unavailable: "Preparation failed. Local storage could not be written.",
  verification_failed: "Preparation failed. Model verification did not pass.",
  preparation_unavailable: "Preparation failed.",
})) {
  preparation.failure_class = code;
  assert.equal(failureText(parseRuntime(runtime, cid).preparation), message);
  const output = { ...runtime, ...preparation }; delete output.preparation;
  assert.equal(failureText(operationRuntime(output, cid).preparation), message);
}
for (const unknown of [null, undefined]) {
  preparation.failure_class = unknown;
  assert.equal(failureText(parseRuntime(runtime, cid).preparation), "Preparation failed.");
}
for (const invalid of ["/private/provider?credential=secret", "__proto__", "constructor", ["verification_failed"], { toString: () => "verification_failed" }, {}, 7]) {
  preparation.failure_class = invalid;
  assert.throws(() => parseRuntime(runtime, cid), /Invalid model response/);
}
const otherCid = `bafybei${"c".repeat(51)}e`;
const catalogRow = (id, title) => ({
  name: title, title, role: "content", source: "signed-model-catalog", installed: false, launchable: false,
  cid: id, publisher_did: "did:key:zFixturePublisher", content_size_bytes: 1024,
  signature_state: "catalog-signature-verified", model_runtime: {
    admitted: false, kept: false, dispatch_ready: false, offer_id: null, preparation: null,
  },
});
const two = catalogEntries({
  schema: "elastos.capsules.catalog/v1", model_catalog_state: "verified",
  capsules: [catalogRow(cid, "Qwen"), catalogRow(otherCid, "SmolLM2")],
});
assert.equal(two.length, 2);
assert.equal(selectCatalogEntry(two, null), null);
assert.equal(selectCatalogEntry(two, cid).cid, cid);
assert.equal(selectCatalogEntry(two, otherCid).cid, otherCid);
assert.equal(selectCatalogEntry(two, `bafybei${"d".repeat(52)}`), null);
assert.equal(selectCatalogEntry(two.slice(0, 1), null).cid, cid);
// One phase per projection. The full view keeps its established copy; the
// compact Marketplace detail changes only the device-centred lines.
const projection = (state, extra = {}) => ({ admitted: false, dispatch_ready: false, offer_id: null,
  preparation: state === null ? null : { ...preparation, state, cancel_requested: false, failure_class: undefined, ...extra } });
const phases = {
  absent: [projection(null), null], ready: [{ ...projection("admitted"), admitted: true, dispatch_ready: true, offer_id: `model:${"b".repeat(64)}` }, null],
  service_unavailable: [{ ...projection("admitted"), admitted: true }, null],
  cancelling: [projection("preparing", { cancel_requested: true }), null], capacity: [projection("capacity_pending"), null],
  settling: [projection("uncertain"), null], verifying: [projection("verifying"), null], preparing: [projection("preparing"), null],
  failed: [projection("failed", { failure_class: "verification_failed" }), null], reclaimed: [projection("reclaimed"), null],
  cancelled: [projection("cancelled"), null], expired: [projection("expired"), null],
  unavailable: [projection("preparing"), "Status is unavailable. Refresh to check before trying again."],
};
for (const [expected, [r, message]] of Object.entries(phases)) {
  assert.equal(phaseOf(r, r.preparation, message), expected);
}
for (const reservedLike of ["reserved", "admission_pending"]) {
  assert.equal(phaseOf(projection(reservedLike), projection(reservedLike).preparation, ""), "preparing", `${reservedLike} is an active preparation`);
}
// Table values cross the vm realm; compare their entries, not their prototype.
const plain = table => Object.fromEntries(Object.entries(table));
assert.deepEqual(plain(PHASE_TEXT), {
  unavailable: "Current status unavailable", ready: "Available on this device",
  service_unavailable: "Available on this device. Model service unavailable.",
  cancelling: "Cancelling preparation…", capacity: "Waiting for local capacity…", settling: "Waiting for preparation to settle…",
  verifying: "Checking model files…", preparing: "Preparing local files…", absent: "Ready to prepare",
  reclaimed: "Model removed from local cache.", cancelled: "Preparation cancelled.", expired: "Preparation expired.",
});
assert.deepEqual(plain(COMPACT_PHASE_TEXT), { ...plain(PHASE_TEXT), absent: "Not on this device yet", preparing: "Getting the model…",
  reclaimed: "Removed from this device", cancelled: "Stopped before finishing", expired: "Not finished in time" });
assert.deepEqual(Object.keys(PHASE_TEXT).sort(), Object.keys(phases).filter(name => name !== "failed").sort(), "every phase except failed reads its copy from the table");
console.log("PASS model preparation failure: safe classes, operation/catalog parity, unknown history, strict rejection, shared copies, explicit two-entry selection, phase copy tables");

// Advance a pending operation beyond the former three-minute polling ceiling.
// The bounded clock exercises the controller without downloading model bytes.
class FixtureNode {
  constructor(tag) { this.tagName = tag; this.children = []; this.dataset = {}; this.classList = { add() {} }; }
  append(...nodes) { this.children.push(...nodes); }
  replaceChildren(...nodes) { this.children = nodes; }
  setAttribute() {}
  addEventListener() {}
  contains() { return false; }
  focus() {}
}
let nextTimer = 0, statusCalls = 0, settled = false;
const timers = new Map();
const fixtureRoot = new FixtureNode("section");
const fixtureRuntime = () => ({
  admitted: settled, kept: false, dispatch_ready: settled,
  offer_id: settled ? `model:${"b".repeat(64)}` : null,
  preparation: { operation_id: "long-preparation", cid, state: settled ? "admitted" : "preparing",
    total_bytes: 1024, completed_bytes: settled ? 1024 : statusCalls,
    cancel_requested: false, admitted: settled, activation_pending: false },
});
const fixtureContext = {
  window: { addEventListener() {}, removeEventListener() {} },
  document: { hidden: false, activeElement: null, createElement: tag => new FixtureNode(tag),
    createTextNode: text => ({ textContent: text }), addEventListener() {}, removeEventListener() {} },
  AbortController, crypto: { randomUUID: () => `poll-${statusCalls}` },
  setTimeout: (callback, delay) => { const id = ++nextTimer; timers.set(id, { callback, delay }); return id; },
  clearTimeout: id => timers.delete(id),
  fetch: async (path, options) => {
    let result;
    if (path === "/api/capsules/catalog") result = { schema: "elastos.capsules.catalog/v1", model_catalog_state: "verified",
      capsules: [{ ...catalogRow(cid, "Small fixture"), model_runtime: fixtureRuntime() }] };
    else if (path === "/api/capsules/interfaces") result = { interfaces: [{ capsule: "marketplace",
      interface: { id: "elastos.marketplace.catalog", methods: ["use", "status", "cancel", "retention", "reclaim"].map(operation => ({
        id: `content.${operation}`, operation, resource: "elastos://capsules/*", approval: "runtime_policy", risk: operation === "status" ? "read" : "write",
      })) }, bindings: ["use", "status", "cancel", "retention", "reclaim"].map(operation => ({ method: `content.${operation}`, executable: true })) }] };
    else {
      const request = JSON.parse(options.body);
      assert.equal(request.method, "content.status");
      assert.equal(request.input.operation_id, "long-preparation");
      statusCalls++;
      assert.ok(statusCalls <= 132, "fixture bounds the status requests");
      const runtime = fixtureRuntime();
      result = { ...request, schema: "elastos.capsules.invoke-result/v1", status: "ok",
        output: { ...runtime, ...runtime.preparation, cid } };
    }
    return { ok: true, text: async () => JSON.stringify(result) };
  },
};
vm.runInNewContext(source, fixtureContext);
const control = fixtureContext.window.ElastosModelManagement.create({ root: fixtureRoot, capsule: "marketplace", token: "fixture", cid, compact: true });
const pendingPolls = () => [...timers].filter(([, timer]) => timer.delay === 1500);
const tickPoll = async () => {
  const pending = pendingPolls();
  assert.equal(pending.length, 1, "visible pending operation keeps exactly one status poll");
  const [id, timer] = pending[0]; timers.delete(id); await timer.callback();
};
control.setVisible(true);
await new Promise(setImmediate);
for (let index = 0; index < 130; index++) await tickPoll();
assert.equal(statusCalls, 130);
settled = true;
await tickPoll();
assert.equal(pendingPolls().length, 0, "terminal admission stops polling");
const visibleText = node => [node.textContent || "", ...(node.children || []).map(visibleText)].join(" ");
assert.match(visibleText(fixtureRoot), /Available on this device/);
settled = false;
await control.refresh();
assert.equal(pendingPolls().length, 1);
control.setVisible(false);
assert.equal(timers.size, 0, "hidden controller releases timers");
control.setVisible(true);
await new Promise(setImmediate);
assert.equal(pendingPolls().length, 1);
control.destroy();
assert.equal(timers.size, 0, "destroyed controller releases timers");
console.log("PASS long model preparation: 131 bounded status reads, terminal render, hide and destroy cleanup");
