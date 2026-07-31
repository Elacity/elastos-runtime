export const RUNTIME_PAGE_CLOSE_TIMEOUT_MS = 8_000;
export const RUNTIME_PAGE_CLEANUP_RETRY_DELAYS_MS = [1_200, 3_000, 7_000];
export const MAX_TRACKED_RUNTIME_PAGE_CLEANUPS = 4;
export const RUNTIME_OWNED_PAGE_FAILURE_KINDS = Object.freeze([
  "signaling",
  "display_status",
  "malformed_response",
  "timeout",
  "no_first_frame",
]);

function runtimeOwnedPageFailure(failure) {
  if (
    !failure ||
    typeof failure !== "object" ||
    !RUNTIME_OWNED_PAGE_FAILURE_KINDS.includes(failure.kind)
  ) {
    throw new TypeError("a supported Runtime-owned page failure is required");
  }
  return Object.freeze({
    kind: failure.kind,
    message: String(failure.message || "").slice(0, 420),
    retry: failure.retry === true,
  });
}

function pendingOutcome(pageId, generation, reason) {
  return {
    state: "pending",
    page_id: pageId,
    generation,
    reason,
  };
}

function runtimeCleanupHandle(page) {
  const handle = page?.runtime_cleanup;
  if (
    handle?.schema !== "elastos.browser.cleanup-handle/v1" ||
    typeof handle.id !== "string" ||
    handle.id.length < 1 ||
    handle.id.length > 128 ||
    !/^[A-Za-z0-9:_-]+$/.test(handle.id)
  ) {
    return null;
  }
  return Object.freeze({
    schema: handle.schema,
    id: handle.id,
  });
}

function sameRuntimeCleanupHandle(left, right) {
  return Boolean(
    left &&
      right &&
      left.schema === right.schema &&
      left.id === right.id,
  );
}

export function runtimePageOwner(page, generation) {
  const pageId = String(page?.page_id || "");
  const runtimeCleanup = runtimeCleanupHandle(page);
  if (!pageId || !runtimeCleanup) {
    return null;
  }
  return Object.freeze({
    page_id: pageId,
    generation: Number(generation || 0),
    runtime_cleanup: runtimeCleanup,
  });
}

export function sameRuntimePageOwner(left, right) {
  return Boolean(
    left?.page_id &&
      right?.page_id &&
      left.page_id === right.page_id &&
      Number(left.generation || 0) === Number(right.generation || 0) &&
      sameRuntimeCleanupHandle(left.runtime_cleanup, right.runtime_cleanup),
  );
}

export function classifyRuntimePageCloseResponse(owner, response) {
  if (
    !response ||
    typeof response !== "object" ||
    response.schema !== "elastos.browser.close-result/v1" ||
    response.page_id !== owner.page_id ||
    response.cleanup_id !== owner.runtime_cleanup.id
  ) {
    return pendingOutcome(owner.page_id, owner.generation, "malformed_response");
  }
  if (response.closed === true || response.already_closed === true) {
    return {
      state: "terminal",
      page_id: owner.page_id,
      generation: owner.generation,
      terminal_kind:
        response.already_closed === true ? "already_absent" : "closed",
    };
  }
  return pendingOutcome(
    owner.page_id,
    owner.generation,
    response.closed === false || response.already_closed === false
      ? "close_failed"
      : "indeterminate_outcome",
  );
}

export function classifyRuntimePageCloseError(owner, error) {
  return pendingOutcome(
    owner.page_id,
    owner.generation,
    Number(error?.status) >= 400 ? "close_failed" : "transport_failure",
  );
}

export function createRuntimePageCleanupController({
  closePage,
  onPending = () => {},
  onTerminal = () => {},
  closeTimeoutMs = RUNTIME_PAGE_CLOSE_TIMEOUT_MS,
  retryDelaysMs = RUNTIME_PAGE_CLEANUP_RETRY_DELAYS_MS,
  maxTracked = MAX_TRACKED_RUNTIME_PAGE_CLEANUPS,
  setTimeoutFn = globalThis.setTimeout,
  clearTimeoutFn = globalThis.clearTimeout,
  createAbortController = () => new AbortController(),
} = {}) {
  if (typeof closePage !== "function") {
    throw new TypeError("closePage is required");
  }
  if (!Number.isInteger(maxTracked) || maxTracked < 1) {
    throw new TypeError("maxTracked must be a positive integer");
  }
  const records = new Map();
  let sequence = 0;

  function ownerKey(owner) {
    return `${owner.runtime_cleanup.id}:${owner.generation}:${owner.page_id}`;
  }

  function makeRoom() {
    if (records.size < maxTracked) {
      return true;
    }
    const terminal = [...records.entries()]
      .filter(([, record]) => record.terminal)
      .sort((left, right) => left[1].sequence - right[1].sequence)[0];
    if (terminal) {
      records.delete(terminal[0]);
    }
    return records.size < maxTracked;
  }

  function recordFor(owner) {
    const key = ownerKey(owner);
    let record = records.get(key);
    if (record) {
      return record;
    }
    if (!makeRoom()) {
      return null;
    }
    record = {
      attempts: 0,
      failure: null,
      inFlight: null,
      lastOutcome: null,
      owner,
      retryTimer: 0,
      sequence: sequence++,
      terminal: false,
    };
    records.set(key, record);
    return record;
  }

  function scheduleRetry(record) {
    const delay = retryDelaysMs[record.attempts - 1];
    if (!Number.isFinite(delay) || record.retryTimer || record.terminal) {
      return;
    }
    record.retryTimer = setTimeoutFn(() => {
      record.retryTimer = 0;
      void reconcile(record.owner, { force: true });
    }, delay);
  }

  async function attemptClose(record) {
    const abortController = createAbortController();
    let timeoutTimer = 0;
    const request = Promise.resolve()
      .then(() =>
        closePage(record.owner, {
          signal: abortController.signal,
        }),
      )
      .then(
        (response) => classifyRuntimePageCloseResponse(record.owner, response),
        (error) => classifyRuntimePageCloseError(record.owner, error),
      );
    const timeout = new Promise((resolve) => {
      timeoutTimer = setTimeoutFn(() => {
        abortController.abort();
        resolve(
          pendingOutcome(
            record.owner.page_id,
            record.owner.generation,
            "timeout",
          ),
        );
      }, closeTimeoutMs);
    });
    const outcome = await Promise.race([request, timeout]);
    clearTimeoutFn(timeoutTimer);
    return outcome;
  }

  async function reconcile(
    owner,
    {
      failure = null,
      force = false,
      schedule = true,
      newWindow = false,
    } = {},
  ) {
    if (!owner?.page_id) {
      return {
        state: "terminal",
        page_id: "",
        generation: Number(owner?.generation || 0),
        terminal_kind: "no_page",
      };
    }
    const record = recordFor(owner);
    if (!record) {
      return pendingOutcome(owner.page_id, owner.generation, "capacity");
    }
    if (failure && !record.failure) {
      record.failure = runtimeOwnedPageFailure(failure);
    }
    if (record.terminal || record.inFlight) {
      return record.inFlight || record.lastOutcome;
    }
    if (
      newWindow &&
      !record.retryTimer &&
      record.attempts >= retryDelaysMs.length + 1
    ) {
      record.attempts = 0;
      record.lastOutcome = pendingOutcome(
        owner.page_id,
        owner.generation,
        "explicit_retry_window",
      );
    }
    if (
      !force &&
      (record.retryTimer ||
        record.attempts >= retryDelaysMs.length + 1)
    ) {
      return (
        record.lastOutcome ||
        pendingOutcome(owner.page_id, owner.generation, "reconciliation_pending")
      );
    }

    record.attempts += 1;
    record.inFlight = attemptClose(record).then((outcome) => {
      record.inFlight = null;
      record.lastOutcome = outcome;
      if (outcome.state === "terminal") {
        record.terminal = true;
        if (record.retryTimer) {
          clearTimeoutFn(record.retryTimer);
          record.retryTimer = 0;
        }
        if (record.failure) {
          onTerminal(record.owner, outcome, record.failure);
        } else {
          onTerminal(record.owner, outcome);
        }
      } else {
        if (record.failure) {
          onPending(record.owner, outcome, record.failure);
        } else {
          onPending(record.owner, outcome);
        }
        if (schedule) {
          scheduleRetry(record);
        }
      }
      return outcome;
    });
    return record.inFlight;
  }

  function status(owner) {
    const record = owner?.page_id ? records.get(ownerKey(owner)) : null;
    return record
      ? {
          attempts: record.attempts,
          failure: record.failure,
          in_flight: Boolean(record.inFlight),
          pending: !record.terminal,
          retry_scheduled: Boolean(record.retryTimer),
          terminal: record.terminal,
        }
      : null;
  }

  return {
    fail(owner, failure) {
      return reconcile(owner, { failure });
    },
    reconcile,
    retry(owner) {
      return reconcile(owner, { newWindow: true });
    },
    status,
  };
}
