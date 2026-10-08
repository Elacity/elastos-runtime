import assert from "node:assert/strict";
import test from "node:test";
import { publicJourneyFailure } from "./ci-installed-home-failure.mjs";

test("public browser failure keeps launch authority and operator paths private", () => {
  const message = "frame.waitForURL: timeout at http://localhost:123/apps/marketplace/#home_token=fixture-secret\n/private/operator/home";
  const failure = publicJourneyFailure({ name: "TimeoutError", message, stack: message }, "model_package_admission");
  assert.deepEqual(failure, { failure_stage: "model_package_admission", failure: "timeout" });
  assert(!JSON.stringify(failure).includes("fixture-secret"));
  assert(!JSON.stringify(failure).includes("/private/operator"));
  assert.deepEqual(publicJourneyFailure({ name: "Error", message: message + " net::ERR_BLOCKED_BY_RESPONSE" }, "model_package_admission"),
    { failure_stage: "model_package_admission", failure: "frame_blocked" });
});

test("public failures accept only owned stages and fixed failure codes", () => {
  assert.deepEqual(publicJourneyFailure({ name: "AssertionError", message: "secret" }, "installed_runtime_reply"),
    { failure_stage: "installed_runtime_reply", failure: "assertion_failed" });
  assert.deepEqual(publicJourneyFailure({ name: "secret", message: "secret" }, "secret"),
    { failure_stage: "journey", failure: "journey_failed" });
  assert.deepEqual(publicJourneyFailure(null, null), { failure_stage: "journey", failure: "journey_failed" });
});
