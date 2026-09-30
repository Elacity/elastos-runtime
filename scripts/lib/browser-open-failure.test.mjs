import assert from "node:assert/strict";
import test from "node:test";
import { browserOpenResponseEvidence } from "./browser-open-failure.mjs";

test("preserves typed settlement without reading its text", async () => {
  const body = { schema: "elastos.browser.open-status/v1", status: "failed" };
  const result = await browserOpenResponseEvidence({ json: async () => body,
    text: () => { throw new Error("unneeded"); } });
  assert.equal(result.body, body);
  assert.equal(result.response_format, "json");
});
test("identifies exact conflict responses and retains no arbitrary body", async () => {
  for (const [text, expected] of [
    ["Browser lifecycle already owns a different open intent", "different_open_intent"],
    ["Browser instance already owns an active or in-flight open", "instance_open_in_flight"],
    ["Browser instance already owns an active or launching lifecycle", "instance_lifecycle_active"],
    ["secret arbitrary server text", "unclassified_non_json"],
  ]) {
    const result = await browserOpenResponseEvidence({ json: async () => { throw new SyntaxError(); }, text: async () => text });
    assert.deepEqual(result, { body: null, response_format: "non_json", admission_reason: expected });
  }
});
test("unavailable response bytes remain unknown", async () => {
  const fail = async () => { throw new Error("response unavailable"); };
  assert.equal((await browserOpenResponseEvidence({ json: fail, text: fail })).admission_reason, "unclassified_non_json");
});
