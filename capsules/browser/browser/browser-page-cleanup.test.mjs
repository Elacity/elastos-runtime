import assert from "node:assert/strict";
import test from "node:test";

import {
  RUNTIME_OWNED_PAGE_FAILURE_KINDS,
  classifyRuntimePageCloseError,
  createRuntimePageCleanupController,
  runtimePageOwner,
  sameRuntimePageOwner,
} from "./browser-page-cleanup.js";

function deferred() {
  let reject;
  let resolve;
  const promise = new Promise((resolvePromise, rejectPromise) => {
    reject = rejectPromise;
    resolve = resolvePromise;
  });
  return { promise, reject, resolve };
}

function fakeTimers() {
  let nextId = 1;
  const timers = new Map();
  return {
    clearTimeoutFn(id) {
      timers.delete(id);
    },
    fireDelay(delay) {
      const timer = [...timers.entries()].find(([, entry]) => entry.delay === delay);
      assert.ok(timer, `expected a ${delay}ms timer`);
      timers.delete(timer[0]);
      timer[1].callback();
    },
    pendingDelays() {
      return [...timers.values()].map(({ delay }) => delay);
    },
    setTimeoutFn(callback, delay) {
      const id = nextId++;
      timers.set(id, { callback, delay });
      return id;
    },
  };
}

function owner(pageId = "page:one", generation = 1) {
  return boundOwner(pageId, generation);
}

function runtimeCleanupHandle(id = "browser-cleanup:test") {
  return {
    schema: "elastos.browser.cleanup-handle/v1",
    id,
  };
}

function boundOwner(
  pageId = "page:one",
  generation = 1,
  cleanupId = "browser-cleanup:test",
) {
  return runtimePageOwner(
    {
      page_id: pageId,
      runtime_cleanup: runtimeCleanupHandle(cleanupId),
    },
    generation,
  );
}

function closeReceipt(pageId, fields = {}) {
  return {
    schema: "elastos.browser.close-result/v1",
    page_id: pageId,
    closed: true,
    cleanup_id: "browser-cleanup:test",
    ...fields,
  };
}

async function flushTasks() {
  await new Promise((resolve) => setImmediate(resolve));
}

test("successful close is terminal and calls the terminal hook once", async () => {
  const terminal = [];
  const pageOwner = owner();
  const controller = createRuntimePageCleanupController({
    closePage: async () => closeReceipt(pageOwner.page_id),
    onTerminal: (...args) => terminal.push(args),
  });

  const outcome = await controller.reconcile(pageOwner);

  assert.equal(outcome.state, "terminal");
  assert.equal(outcome.terminal_kind, "closed");
  assert.deepEqual(terminal, [[pageOwner, outcome]]);
  assert.equal(controller.status(pageOwner).terminal, true);
});

test("Runtime-proven already-absent receipt is terminal", async () => {
  const pageOwner = owner();
  const receiptController = createRuntimePageCleanupController({
    closePage: async () =>
      closeReceipt(pageOwner.page_id, {
        closed: false,
        already_closed: true,
      }),
  });
  const receiptOutcome = await receiptController.reconcile(pageOwner);
  assert.equal(receiptOutcome.state, "terminal");
  assert.equal(receiptOutcome.terminal_kind, "already_absent");
});

test("a Runtime 404 does not prove terminal engine cleanup", () => {
  const pageOwner = owner();
  const error = new Error("browser session is not active");
  error.status = 404;
  error.payload = "browser session is not active";

  assert.deepEqual(classifyRuntimePageCloseError(pageOwner, error), {
    state: "pending",
    page_id: pageOwner.page_id,
    generation: pageOwner.generation,
    reason: "close_failed",
  });
});

test("timeout aborts the attempt and retains cleanup as pending", async () => {
  const timers = fakeTimers();
  const pending = [];
  let aborts = 0;
  const controller = createRuntimePageCleanupController({
    closePage: async () => new Promise(() => {}),
    closeTimeoutMs: 25,
    retryDelaysMs: [100],
    setTimeoutFn: timers.setTimeoutFn,
    clearTimeoutFn: timers.clearTimeoutFn,
    createAbortController: () => ({
      signal: {},
      abort() {
        aborts += 1;
      },
    }),
    onPending: (...args) => pending.push(args),
  });

  const close = controller.reconcile(owner());
  await Promise.resolve();
  timers.fireDelay(25);
  const outcome = await close;

  assert.equal(outcome.state, "pending");
  assert.equal(outcome.reason, "timeout");
  assert.equal(aborts, 1);
  assert.equal(pending.length, 1);
  assert.deepEqual(timers.pendingDelays(), [100]);
});

test("failed close and transport failure remain pending", async () => {
  const failedOwner = owner("page:failed");
  const failed = createRuntimePageCleanupController({
    closePage: async () =>
      closeReceipt(failedOwner.page_id, {
        closed: false,
      }),
    retryDelaysMs: [],
  });
  const failedOutcome = await failed.reconcile(failedOwner);
  assert.equal(failedOutcome.state, "pending");
  assert.equal(failedOutcome.reason, "close_failed");

  const transportOwner = owner("page:transport");
  const transport = createRuntimePageCleanupController({
    closePage: async () => {
      throw new TypeError("failed to fetch");
    },
    retryDelaysMs: [],
  });
  const transportOutcome = await transport.reconcile(transportOwner);
  assert.equal(transportOutcome.state, "pending");
  assert.equal(transportOutcome.reason, "transport_failure");
});

test("indeterminate close response remains pending", async () => {
  const pageOwner = owner();
  const controller = createRuntimePageCleanupController({
    closePage: async () => ({
      schema: "elastos.browser.close-result/v1",
      page_id: pageOwner.page_id,
      cleanup_id: pageOwner.runtime_cleanup.id,
    }),
    retryDelaysMs: [],
  });

  const outcome = await controller.reconcile(pageOwner);

  assert.equal(outcome.state, "pending");
  assert.equal(outcome.reason, "indeterminate_outcome");
});

test("malformed response remains pending", async () => {
  const pageOwner = owner();
  const controller = createRuntimePageCleanupController({
    closePage: async () => ({
      schema: "elastos.browser.close-result/v1",
      page_id: "page:substituted",
      closed: true,
    }),
    retryDelaysMs: [],
  });

  const outcome = await controller.reconcile(pageOwner);

  assert.equal(outcome.state, "pending");
  assert.equal(outcome.reason, "malformed_response");
});

test("terminal close requires the exact opaque Runtime cleanup handle", async () => {
  const pageOwner = boundOwner();
  const substitutions = [
    { cleanup_id: "browser-cleanup:foreign" },
    { page_id: "page:foreign" },
  ];

  for (const substitution of substitutions) {
    const controller = createRuntimePageCleanupController({
      closePage: async () =>
        closeReceipt(pageOwner.page_id, {
          ...substitution,
        }),
      retryDelaysMs: [],
    });
    const outcome = await controller.reconcile(pageOwner);
    assert.equal(outcome.state, "pending");
    assert.equal(outcome.reason, "malformed_response");
  }

  const controller = createRuntimePageCleanupController({
    closePage: async () => closeReceipt(pageOwner.page_id),
    retryDelaysMs: [],
  });
  assert.equal((await controller.reconcile(pageOwner)).state, "terminal");
});

test("missing or malformed cleanup handles never create Browser ownership", () => {
  for (const runtimeCleanup of [
    null,
    {},
    runtimeCleanupHandle(""),
    runtimeCleanupHandle("contains space"),
    runtimeCleanupHandle("x".repeat(129)),
    { schema: "elastos.browser.cleanup-handle/v2", id: "browser-cleanup:test" },
  ]) {
    assert.equal(
      runtimePageOwner(
        {
          page_id: "page:one",
          runtime_cleanup: runtimeCleanup,
        },
        1,
      ),
      null,
    );
  }

  assert.equal(runtimePageOwner({ page_id: "page:legacy" }, 0), null);
});

test("cleanup records distinguish exact opaque Runtime handles", async () => {
  const first = boundOwner("page:stable-id", 0, "browser-cleanup:first");
  const replacement = boundOwner(
    "page:stable-id",
    0,
    "browser-cleanup:replacement",
  );
  const closed = [];
  const controller = createRuntimePageCleanupController({
    closePage: async (pageOwner) => {
      closed.push(pageOwner.runtime_cleanup.id);
      return closeReceipt(pageOwner.page_id, {
        cleanup_id: pageOwner.runtime_cleanup.id,
      });
    },
  });

  assert.equal((await controller.reconcile(first)).state, "terminal");
  assert.equal((await controller.reconcile(replacement)).state, "terminal");
  assert.deepEqual(closed, [
    "browser-cleanup:first",
    "browser-cleanup:replacement",
  ]);
});

test("repeated reconciliation deduplicates one close effect", async () => {
  const pageOwner = owner();
  const response = deferred();
  let closeCalls = 0;
  let terminalCalls = 0;
  const controller = createRuntimePageCleanupController({
    closePage: async () => {
      closeCalls += 1;
      return response.promise;
    },
    onTerminal: () => {
      terminalCalls += 1;
    },
  });

  const first = controller.reconcile(pageOwner);
  const repeated = controller.reconcile(pageOwner);
  await Promise.resolve();
  assert.equal(closeCalls, 1);
  response.resolve(closeReceipt(pageOwner.page_id));

  assert.equal((await first).state, "terminal");
  assert.equal((await repeated).state, "terminal");
  assert.equal((await controller.reconcile(pageOwner)).state, "terminal");
  assert.equal(closeCalls, 1);
  assert.equal(terminalCalls, 1);
});

test("automatic reconciliation is bounded", async () => {
  const timers = fakeTimers();
  const pageOwner = owner();
  let closeCalls = 0;
  const controller = createRuntimePageCleanupController({
    closePage: async () => {
      closeCalls += 1;
      return closeReceipt(pageOwner.page_id, { closed: false });
    },
    retryDelaysMs: [10, 20],
    setTimeoutFn: timers.setTimeoutFn,
    clearTimeoutFn: timers.clearTimeoutFn,
  });

  await controller.reconcile(pageOwner);
  timers.fireDelay(10);
  await flushTasks();
  timers.fireDelay(20);
  await flushTasks();

  assert.equal(closeCalls, 3);
  assert.equal(controller.status(pageOwner).pending, true);
  assert.equal(controller.status(pageOwner).retry_scheduled, false);
  await controller.reconcile(pageOwner);
  assert.equal(closeCalls, 3);
});

test("an explicit later action opens one new deduplicated bounded retry window", async () => {
  const timers = fakeTimers();
  const pageOwner = owner();
  let closeCalls = 0;
  const controller = createRuntimePageCleanupController({
    closePage: async () => {
      closeCalls += 1;
      return closeReceipt(pageOwner.page_id, { closed: false });
    },
    retryDelaysMs: [10],
    setTimeoutFn: timers.setTimeoutFn,
    clearTimeoutFn: timers.clearTimeoutFn,
  });

  await controller.reconcile(pageOwner);
  timers.fireDelay(10);
  await flushTasks();
  assert.equal(closeCalls, 2);

  const first = controller.retry(pageOwner);
  const duplicate = controller.retry(pageOwner);
  await Promise.all([first, duplicate]);
  assert.equal(closeCalls, 3);
  timers.fireDelay(10);
  await flushTasks();
  assert.equal(closeCalls, 4);
  await controller.reconcile(pageOwner);
  assert.equal(closeCalls, 4);
});

test("terminal completion for an older generation cannot clear its replacement", async () => {
  const firstOwner = boundOwner(
    "page:stable-id",
    1,
    "browser-cleanup:first",
  );
  const replacementOwner = boundOwner(
    "page:stable-id",
    2,
    "browser-cleanup:replacement",
  );
  const response = deferred();
  let currentOwner = firstOwner;
  const controller = createRuntimePageCleanupController({
    closePage: async () => response.promise,
    onTerminal: (closedOwner) => {
      if (sameRuntimePageOwner(currentOwner, closedOwner)) {
        currentOwner = null;
      }
    },
  });

  const close = controller.reconcile(firstOwner);
  currentOwner = replacementOwner;
  response.resolve(
    closeReceipt(firstOwner.page_id, {
      cleanup_id: firstOwner.runtime_cleanup.id,
    }),
  );
  await close;

  assert.deepEqual(currentOwner, replacementOwner);
  assert.equal(sameRuntimePageOwner(firstOwner, replacementOwner), false);
});

test("every post-ownership failure retains exact ownership through pending retry and terminal cleanup", async () => {
  for (const failureKind of RUNTIME_OWNED_PAGE_FAILURE_KINDS) {
    const timers = fakeTimers();
    const pageOwner = boundOwner(
      `page:${failureKind}`,
      7,
      `browser-cleanup:${failureKind}`,
    );
    let currentOwner = pageOwner;
    let closeCalls = 0;
    const pending = [];
    const terminal = [];
    const failure = {
      kind: failureKind,
      message: `${failureKind} failed`,
      retry: failureKind === "signaling",
    };
    const controller = createRuntimePageCleanupController({
      closePage: async () => {
        closeCalls += 1;
        return closeReceipt(pageOwner.page_id, {
          cleanup_id: pageOwner.runtime_cleanup.id,
          closed: closeCalls > 1,
        });
      },
      retryDelaysMs: [10],
      setTimeoutFn: timers.setTimeoutFn,
      clearTimeoutFn: timers.clearTimeoutFn,
      onPending: (pendingOwner, outcome, pendingFailure) => {
        pending.push([pendingOwner, outcome, pendingFailure]);
      },
      onTerminal: (closedOwner, outcome, terminalFailure) => {
        terminal.push([closedOwner, outcome, terminalFailure]);
        if (sameRuntimePageOwner(currentOwner, closedOwner)) {
          currentOwner = null;
        }
      },
    });

    const first = await controller.fail(pageOwner, failure);

    assert.equal(first.state, "pending");
    assert.deepEqual(currentOwner, pageOwner);
    assert.equal(closeCalls, 1);
    assert.deepEqual(controller.status(pageOwner).failure, failure);
    assert.equal(pending.length, 1);
    assert.equal(terminal.length, 0);

    timers.fireDelay(10);
    await flushTasks();

    assert.equal(closeCalls, 2);
    assert.equal(currentOwner, null);
    assert.equal(controller.status(pageOwner).terminal, true);
    assert.equal(terminal.length, 1);
    assert.deepEqual(terminal[0], [
      pageOwner,
      {
        state: "terminal",
        page_id: pageOwner.page_id,
        generation: pageOwner.generation,
        terminal_kind: "closed",
      },
      failure,
    ]);
  }
});

test("unsupported post-ownership failure cannot enter cleanup", async () => {
  const pageOwner = owner();
  let closeCalls = 0;
  const controller = createRuntimePageCleanupController({
    closePage: async () => {
      closeCalls += 1;
      return closeReceipt(pageOwner.page_id);
    },
  });

  await assert.rejects(
    controller.fail(pageOwner, {
      kind: "arbitrary_failure",
      message: "must not broaden failure vocabulary",
    }),
    /supported Runtime-owned page failure/,
  );
  assert.equal(closeCalls, 0);
});
