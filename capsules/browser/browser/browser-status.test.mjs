import assert from "node:assert/strict";
import test from "node:test";

import {
  friendlyOpenError,
  runtimeOpenOutcome,
} from "./browser-status.js";

function openError(state, effects) {
  const error = new Error("browser-vz-engine-supervisor exited");
  error.status = 503;
  error.payload = {
    schema: "elastos.browser.open-error/v1",
    outcome: {
      schema: "elastos.browser.open-outcome/v1",
      state,
      effects: {
        page_acquired: false,
        vm_acquired: false,
        stream_acquired: false,
        ...effects,
      },
    },
  };
  return error;
}

test("pre-effect open failure never claims a missing terminal close", () => {
  const error = openError("terminal_pre_effect_failure");

  assert.equal(runtimeOpenOutcome(error)?.state, "terminal_pre_effect_failure");
  assert.equal(
    friendlyOpenError(error),
    "Browser Engine failed to start cleanly. No Browser page or VM was acquired.",
  );
  assert.doesNotMatch(friendlyOpenError(error), /terminal close/i);
});

test("cleanup-pending open failure is driven by structured acquired effects", () => {
  const error = openError("cleanup_pending", {
    page_acquired: true,
    vm_acquired: true,
  });

  assert.equal(
    friendlyOpenError(error),
    "Browser Engine failed to start cleanly. Runtime cleanup is pending for the acquired Browser session.",
  );
});

test("indeterminate dispatched launch reports retained reconciliation ownership", () => {
  const error = openError("cleanup_pending", {
    page_acquired: null,
    vm_acquired: null,
    stream_acquired: true,
  });
  error.payload.outcome.ownership = "launch_reconciliation_pending";

  assert.equal(runtimeOpenOutcome(error)?.effects.page_acquired, null);
  assert.equal(
    friendlyOpenError(error),
    "Browser Engine returned no safe launch result. Runtime retained ownership and is reconciling before another Browser session can start.",
  );
  assert.doesNotMatch(friendlyOpenError(error), /no Browser page|effects were closed/i);
});

test("indeterminate effects require an exact retained-ownership marker", () => {
  const error = openError("cleanup_pending", {
    page_acquired: null,
    vm_acquired: null,
    stream_acquired: true,
  });

  assert.equal(runtimeOpenOutcome(error), null);
});

test("launcher wording alone cannot synthesize a cleanup outcome", () => {
  const error = new Error(
    "browser-vz-engine-supervisor exited before readiness",
  );
  error.status = 503;

  assert.equal(runtimeOpenOutcome(error), null);
  assert.equal(
    friendlyOpenError(error),
    "Browser is temporarily unavailable. Refresh Browser or choose another Browser Engine.",
  );
});
