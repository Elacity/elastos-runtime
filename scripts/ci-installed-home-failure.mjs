// Public proof uses fixed failure codes. Detailed browser errors belong to Home.
export function publicJourneyFailure(error, stage) {
  const failure_stage = ["sign_in", "model_package_admission", "engine_absent_refusal", "installed_runtime_reply"].includes(stage) ? stage : "journey";
  const failure = error?.message?.includes("ERR_BLOCKED_BY_RESPONSE") ? "frame_blocked" :
    error?.name === "TimeoutError" ? "timeout" :
    error?.name === "AssertionError" ? "assertion_failed" : "journey_failed";
  return { failure_stage, failure };
}
