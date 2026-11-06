import assert from "node:assert/strict";

// Only these bounded observations may leave the private journey log.
const assertionValues = {
  create_http_ok: "boolean", create_token_cap: "number", create_request_id: "boolean",
  create_authority: "boolean", create_status: ["ok", "error"], create_run_id: "boolean",
  events_http_ok: "boolean", stop_create_terminal: "boolean", stop_cancel_http_ok: "boolean",
  stop_terminal_status: ["prepared", "running", "reconciling", "completed", "cancelled", "failed", "settlement_unknown"],
  stop_whole_engine_exit: "boolean", stop_runtime_unchanged: "boolean",
  engine_running_status: ["passed", "failed"], engine_stopped_status: ["passed", "failed"],
};
function safeObservation(check, value) {
  const type = Object.hasOwn(assertionValues, check) ? assertionValues[check] : null;
  if (type === "boolean" && typeof value === "boolean") return value;
  if (type === "number" && Number.isSafeInteger(value) && value >= 0 && value <= 1000000) return value;
  if (Array.isArray(type) && type.includes(value)) return value;
  return "invalid";
}
export function assertJourneyEqual(check, actual, expected) {
  assert(Object.hasOwn(assertionValues, check), "fixed journey assertion required");
  try { assert.equal(actual, expected); }
  catch (error) {
    error.journeyAssertion = check;
    error.journeyActual = safeObservation(check, actual);
    error.journeyExpected = safeObservation(check, expected);
    throw error;
  }
}

// Public proof uses fixed failure codes. Detailed browser errors belong to Home.
export function publicJourneyFailure(error, stage, subcheck) {
  const failure_stage = ["sign_in", "model_package_admission", "engine_absent_refusal", "installed_runtime_reply", "memory_refusal", "finite_resource_lifecycle", "finite_home_recovery", "model_stop_get_different", "model_interrupted_get_retry"].includes(stage) ? stage : "journey";
  const failure = error?.message?.includes("ERR_BLOCKED_BY_RESPONSE") ? "frame_blocked" :
    error?.name === "TimeoutError" ? "timeout" :
    error?.name === "AssertionError" ? "assertion_failed" : "journey_failed";
  const checks = {
    installed_runtime_reply: ["request_cap", "request_authority", "terminal_status", "terminal_output", "reply_display", "reply_bounds"],
    finite_resource_lifecycle: ["owner_configure", "contender_launch", "open_assistant", "contender_configure", "owner_start", "busy", "stop", "stop_recovery", "idle", "idle_recovery", "crash", "crash_recovery"],
    model_stop_get_different: ["stop_real_stream", "get_other_model", "open_assistant", "other_model_reply"],
    model_interrupted_get_retry: ["fresh_get_precondition", "partial_get", "restarted_page_load", "restarted_home_ready", "restarted_signed_in",
      "restarted_desktop", "restarted_marketplace", "restarted_model_detail", "restarted_retry_text", "retry_retained_get"],
  };
  const failure_check = checks[failure_stage]?.includes(subcheck) ? subcheck : null;
  const check = error?.journeyAssertion;
  const detail = failure === "assertion_failed" && Object.hasOwn(assertionValues, check) ? {
    failure_assertion: check,
    observed: safeObservation(check, error.journeyActual),
    expected: safeObservation(check, error.journeyExpected),
  } : {};
  return { failure_stage, failure, ...(failure_check ? { failure_check } : {}), ...detail };
}
