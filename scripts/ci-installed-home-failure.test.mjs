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
  assert.deepEqual(publicJourneyFailure({ name: "AssertionError", message: "secret" }, "engine_absent_refusal"),
    { failure_stage: "engine_absent_refusal", failure: "assertion_failed" });
  for (const stage of ["memory_refusal", "finite_resource_lifecycle", "finite_home_recovery"]) {
    assert.deepEqual(publicJourneyFailure({ name: "AssertionError", message: "secret" }, stage, "reply_display"),
      { failure_stage: stage, failure: "assertion_failed" });
  }
  assert.deepEqual(publicJourneyFailure({ name: "secret", message: "secret" }, "secret"),
    { failure_stage: "journey", failure: "journey_failed" });
  assert.deepEqual(publicJourneyFailure(null, null), { failure_stage: "journey", failure: "journey_failed" });
});

test("reply subchecks expose only fixed names while error details stay private", () => {
  const secret = "home_token=fixture-secret /private/operator/home";
  for (const subcheck of ["request_cap", "request_authority", "terminal_status", "terminal_output", "reply_display", "reply_bounds"]) {
    for (const name of ["AssertionError", "TimeoutError", "Error"]) {
      const failure = publicJourneyFailure({ name, message: secret, code: secret, stack: secret }, "installed_runtime_reply", subcheck);
      assert.equal(failure.failure_check, subcheck);
      assert.equal(failure.failure, { AssertionError: "assertion_failed", TimeoutError: "timeout", Error: "journey_failed" }[name]);
      assert(!JSON.stringify(failure).includes(secret));
      assert(!JSON.stringify(failure).includes("fixture-secret"));
      assert(!JSON.stringify(failure).includes("/private/operator"));
    }
  }
  assert.deepEqual(publicJourneyFailure({ name: "AssertionError", message: secret, code: "reply_display" }, "installed_runtime_reply", secret),
    { failure_stage: "installed_runtime_reply", failure: "assertion_failed" });
  assert.deepEqual(publicJourneyFailure({ name: "Error", message: secret }, secret, "reply_display"),
    { failure_stage: "journey", failure: "journey_failed" });
  assert.deepEqual(publicJourneyFailure({ name: "AssertionError", message: secret }, "engine_absent_refusal", "reply_display"),
    { failure_stage: "engine_absent_refusal", failure: "assertion_failed" });
});

test("lifecycle subchecks locate the blocked UI phase without disclosing launch authority", () => {
  const secret = "home_token=fixture-secret /private/operator/home";
  for (const subcheck of ["owner_configure", "contender_launch", "contender_configure", "owner_start", "busy", "stop", "stop_recovery", "idle", "idle_recovery", "crash", "crash_recovery"]) {
    assert.deepEqual(publicJourneyFailure({ name: "TimeoutError", message: secret }, "finite_resource_lifecycle", subcheck),
      { failure_stage: "finite_resource_lifecycle", failure: "timeout", failure_check: subcheck });
  }
  for (const subcheck of [secret, "reply_display", "constructor", "toString", "__proto__"]) {
    assert.deepEqual(publicJourneyFailure({ name: "TimeoutError", message: secret }, "finite_resource_lifecycle", subcheck),
      { failure_stage: "finite_resource_lifecycle", failure: "timeout" });
  }
});

test("model repair failures retain only the fixed journey phase", () => {
  const secret = "home_token=fixture-secret /private/operator/home";
  const stages = {
    model_stop_get_different: ["stop_real_stream", "get_other_model", "other_model_reply"],
    model_interrupted_get_retry: ["fresh_get_precondition", "partial_get", "restarted_home", "retry_retained_get"],
  };
  for (const [stage, checks] of Object.entries(stages)) {
    for (const check of checks) {
      assert.deepEqual(publicJourneyFailure({ name: "TimeoutError", message: secret }, stage, check),
        { failure_stage: stage, failure: "timeout", failure_check: check });
    }
    for (const check of [secret, "reply_display", "constructor", "__proto__"]) {
      assert.deepEqual(publicJourneyFailure({ name: "Error", message: secret }, stage, check),
        { failure_stage: stage, failure: "journey_failed" });
    }
  }
});
