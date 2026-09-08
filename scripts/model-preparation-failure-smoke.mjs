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
  "window.testFailure = { parseRuntime, operationRuntime, failureText }; window.ElastosModelManagement = {"), context);
const { parseRuntime, operationRuntime, failureText } = context.window.testFailure;
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
console.log("PASS model preparation failure: safe classes, operation/catalog parity, unknown history, strict rejection, shared copies");
