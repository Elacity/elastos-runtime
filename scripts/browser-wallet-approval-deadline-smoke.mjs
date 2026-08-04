#!/usr/bin/env node

import assert from "node:assert/strict";
import {
  cacheWalletApprovalPromise as cacheSelkiesApproval,
  waitForWalletApprovalSignature as waitForSelkiesSignature,
  waitForWalletApprovalStatus as waitForSelkiesStatus,
  waitForWalletApprovalTransaction as waitForSelkiesTransaction,
  walletApprovalDeadlineMs as selkiesDeadlineMs,
  walletRuntimeErrorCodeForStatus,
} from "./browser-selkies-control-service.mjs";
import {
  cacheWalletApprovalPromise as cachePlaywrightApproval,
  waitForWalletApprovalSignature as waitForPlaywrightSignature,
  waitForWalletApprovalStatus as waitForPlaywrightStatus,
  waitForWalletApprovalTransaction as waitForPlaywrightTransaction,
  walletApprovalDeadlineMs as playwrightDeadlineMs,
} from "../elastos/tools/browser-playwright-engine/src/wallet-approval.mjs";

const START_MS = 1_800_000_000_000;
const START_SECONDS = START_MS / 1000;
let assertions = 0;

function check(condition, message) {
  assertions += 1;
  assert.ok(condition, message);
}

function equal(actual, expected, message) {
  assertions += 1;
  assert.equal(actual, expected, message);
}

async function rejects(action, expectedCode, messagePattern) {
  assertions += 1;
  await assert.rejects(action, (error) => {
    assert.equal(error?.code, expectedCode);
    assert.match(String(error?.message || ""), messagePattern);
    return true;
  });
}

function fakeClock(startMs = START_MS) {
  let currentMs = startMs;
  const waits = [];
  return {
    now: () => currentMs,
    advance: (delayMs) => {
      check(
        Number.isFinite(delayMs) && delayMs > 0,
        `fake clock received invalid advance ${delayMs}`,
      );
      currentMs += delayMs;
    },
    wait: async (delayMs) => {
      check(
        Number.isFinite(delayMs) && delayMs > 0,
        `fake clock received invalid delay ${delayMs}`,
      );
      waits.push(delayMs);
      currentMs += delayMs;
    },
    waits,
  };
}

async function verifyCache(name, cacheApproval) {
  for (const kind of ["signature", "transaction"]) {
    const cache = new Map();
    let creates = 0;
    let complete;
    const create = () => {
      creates += 1;
      return new Promise((resolve) => {
        complete = resolve;
      });
    };
    const first = cacheApproval(cache, `${kind}:exact-request`, create);
    const duplicate = cacheApproval(cache, `${kind}:exact-request`, create);
    await Promise.resolve();
    equal(creates, 1, `${name} created a duplicate ${kind} approval`);
    equal(cache.size, 1, `${name} dropped the ${kind} cache while pending`);
    complete(`${kind}:terminal`);
    equal(
      await first,
      `${kind}:terminal`,
      `${name} lost the first ${kind} result`,
    );
    equal(
      await duplicate,
      `${kind}:terminal`,
      `${name} did not share the exact ${kind} result`,
    );
    equal(cache.size, 0, `${name} retained terminal ${kind} cache state`);
  }
}

async function verifyAdapter(name, adapter) {
  const deadlineMs = adapter.deadline(START_SECONDS + 600, START_MS);
  equal(
    deadlineMs,
    START_MS + 600_000,
    `${name} did not use Runtime epoch seconds`,
  );

  let invalidExpiryReads = 0;
  const invalidExpiryOptions = {
    now: () => START_MS,
    wait: async () => {
      throw new Error("invalid expiry path must not wait");
    },
    getStatus: async () => {
      invalidExpiryReads += 1;
      return { status: "pending" };
    },
  };
  for (const expiresAt of [undefined, "1800000600", 1.5, Number.MAX_SAFE_INTEGER]) {
    await rejects(
      () =>
        adapter.waitStatus(
          `${name}:malformed-expiry`,
          expiresAt,
          invalidExpiryOptions,
        ),
      4100,
      /valid expiry/,
    );
  }
  await rejects(
    () =>
      adapter.waitStatus(
        `${name}:expired-at-creation`,
        START_SECONDS,
        invalidExpiryOptions,
      ),
    4001,
    /expired/,
  );
  await rejects(
    () =>
      adapter.waitStatus(
        `${name}:excessive-expiry`,
        START_SECONDS + 1801,
        invalidExpiryOptions,
      ),
    4100,
    /maximum wait/,
  );
  equal(
    invalidExpiryReads,
    0,
    `${name} polled before rejecting an invalid Runtime expiry`,
  );

  const afterFiveMinutes = fakeClock();
  let afterFiveMinuteReads = 0;
  const lateCompletion = await adapter.waitStatus(
    `${name}:after-five-minutes`,
    START_SECONDS + 600,
    {
      now: afterFiveMinutes.now,
      wait: afterFiveMinutes.wait,
      pollIntervalMs: 1000,
      getStatus: async () => {
        afterFiveMinuteReads += 1;
        return afterFiveMinutes.now() >= START_MS + 301_000
          ? { status: "completed", signature: "0xlate" }
          : { status: "pending" };
      },
    },
  );
  equal(
    lateCompletion.signature,
    "0xlate",
    `${name} abandoned approval at the old five-minute boundary`,
  );
  check(
    afterFiveMinuteReads > 300,
    `${name} did not exercise completion after five minutes`,
  );

  const providerExpiryClock = fakeClock();
  let providerExpiryReads = 0;
  await rejects(
    () =>
      adapter.waitSignature(`${name}:provider-expiry`, START_SECONDS + 600, {
        now: providerExpiryClock.now,
        wait: providerExpiryClock.wait,
        getStatus: async () => {
          providerExpiryReads += 1;
          return providerExpiryReads === 1
            ? { status: "pending" }
            : { status: "expired" };
        },
      }),
    4001,
    /expired/,
  );
  equal(providerExpiryReads, 2, `${name} ignored provider expiry`);

  await rejects(
    () =>
      adapter.waitSignature(`${name}:rejected`, START_SECONDS + 600, {
        now: () => START_MS,
        wait: async () => {
          throw new Error("rejection path must not wait");
        },
        getStatus: async () => ({ status: "rejected" }),
      }),
    4001,
    /rejected/,
  );

  const finalRaceClock = fakeClock();
  let finalRaceReads = 0;
  const finalRace = await adapter.waitStatus(
    `${name}:final-status-race`,
    START_SECONDS + 2,
    {
      now: finalRaceClock.now,
      wait: finalRaceClock.wait,
      pollIntervalMs: 1200,
      getStatus: async () => {
        finalRaceReads += 1;
        return finalRaceReads === 3
          ? { status: "completed", signature: "0xfinal" }
          : { status: "pending" };
      },
    },
  );
  equal(finalRace.signature, "0xfinal", `${name} lost the final status race`);
  equal(finalRaceReads, 3, `${name} made an unbounded final status observation`);
  equal(
    finalRaceClock.waits.at(-1),
    800,
    `${name} slept past the Runtime deadline`,
  );

  const signature = await adapter.waitSignature(
    `${name}:signature`,
    START_SECONDS + 600,
    {
      now: () => START_MS,
      wait: async () => {
        throw new Error("completed signature path must not wait");
      },
      getStatus: async () => ({
        status: "completed",
        signature: "0xsigned",
      }),
    },
  );
  equal(signature, "0xsigned", `${name} lost the completed signature`);

  let broadcasts = 0;
  const transactionHash = await adapter.waitTransaction(
    `${name}:transaction`,
    START_SECONDS + 600,
    {
      now: () => START_MS,
      wait: async () => {
        throw new Error("completed transaction path must not wait");
      },
      getStatus: async () => ({
        status: "completed",
        signed_transaction: "0x02f8",
      }),
      broadcastTransaction: async () => {
        broadcasts += 1;
        return { transaction_hash: "0xbroadcast" };
      },
    },
  );
  equal(
    transactionHash,
    "0xbroadcast",
    `${name} lost the transaction broadcast result`,
  );
  equal(broadcasts, 1, `${name} did not broadcast exactly once`);

  broadcasts = 0;
  const existingHash = await adapter.waitTransaction(
    `${name}:already-broadcast`,
    START_SECONDS + 600,
    {
      now: () => START_MS,
      wait: async () => {
        throw new Error("existing hash path must not wait");
      },
      getStatus: async () => ({
        status: "completed",
        signed_transaction: "0x02f8",
        transaction_hash: "0xexisting",
      }),
      broadcastTransaction: async () => {
        broadcasts += 1;
        return { transaction_hash: "0xduplicate" };
      },
    },
  );
  equal(existingHash, "0xexisting", `${name} replaced the terminal hash`);
  equal(broadcasts, 0, `${name} rebroadcast a terminal transaction`);

  await verifyCache(name, adapter.cacheApproval);
}

async function verifyIndeterminateCache(name, adapter, kind) {
  const clock = fakeClock();
  const cache = new Map();
  let creates = 0;
  let statusReads = 0;
  let rejectTransientStatus;
  let signalStatusStarted;
  let signalPollWaitStarted;
  let resumePoll;
  let broadcasts = 0;
  const statusStarted = new Promise((resolve) => {
    signalStatusStarted = resolve;
  });
  const pollWaitStarted = new Promise((resolve) => {
    signalPollWaitStarted = resolve;
  });
  const create = () => {
    creates += 1;
    const options = {
      now: clock.now,
      wait: async (delayMs) => {
        clock.advance(delayMs);
        signalPollWaitStarted();
        await new Promise((resolve) => {
          resumePoll = resolve;
        });
      },
      getStatus: async (_requestId, { timeoutMs }) => {
        check(
          timeoutMs > 0 && timeoutMs <= 3000,
          `${name} ${kind} status I/O was not explicitly bounded`,
        );
        statusReads += 1;
        if (statusReads === 1) {
          signalStatusStarted();
          return await new Promise((_, reject) => {
            rejectTransientStatus = reject;
          });
        }
        return kind === "signature"
          ? { status: "completed", signature: "0xafter-transient" }
          : { status: "completed", signed_transaction: "0x02f8" };
      },
    };
    if (kind === "signature") {
      return adapter.waitSignature(
        `${name}:${kind}:transient`,
        START_SECONDS + 600,
        options,
      );
    }
    return adapter.waitTransaction(
      `${name}:${kind}:transient`,
      START_SECONDS + 600,
      {
        ...options,
        broadcastTransaction: async () => {
          broadcasts += 1;
          return { transaction_hash: "0xafter-transient" };
        },
      },
    );
  };

  const first = adapter.cacheApproval(
    cache,
    `${kind}:transient-exact-request`,
    create,
  );
  await statusStarted;
  rejectTransientStatus(new Error("transient status transport failure"));
  await pollWaitStarted;
  equal(
    cache.size,
    1,
    `${name} released the ${kind} cache after an indeterminate status failure`,
  );
  const duplicate = adapter.cacheApproval(
    cache,
    `${kind}:transient-exact-request`,
    create,
  );
  equal(
    creates,
    1,
    `${name} created a duplicate ${kind} approval after status failure`,
  );
  resumePoll();
  const expected = "0xafter-transient";
  equal(
    await first,
    expected,
    `${name} lost the ${kind} completion after status recovery`,
  );
  equal(
    await duplicate,
    expected,
    `${name} did not share recovered ${kind} completion`,
  );
  equal(cache.size, 0, `${name} retained terminal ${kind} cache state`);
  equal(statusReads, 2, `${name} did not retry ${kind} status observation`);
  equal(
    broadcasts,
    kind === "transaction" ? 1 : 0,
    `${name} ${kind} transaction broadcast count changed`,
  );
}

async function verifyStatusIoRepair(name, adapter) {
  const hangingClock = fakeClock();
  let hangingReads = 0;
  let hangingTimeouts = 0;
  const completionAfterHang = await adapter.waitSignature(
    `${name}:hanging-status`,
    START_SECONDS + 10,
    {
      now: hangingClock.now,
      wait: hangingClock.wait,
      getStatus: async (_requestId, { timeoutMs }) => {
        check(
          timeoutMs > 0 && timeoutMs <= 3000,
          `${name} hanging status call did not receive an I/O bound`,
        );
        hangingReads += 1;
        return hangingReads === 1
          ? new Promise(() => {})
          : { status: "completed", signature: "0xafter-hang" };
      },
      withStatusTimeout: async (promise, timeoutMs) => {
        hangingTimeouts += 1;
        if (hangingTimeouts === 1) {
          hangingClock.advance(timeoutMs);
          throw new Error("status observation timed out");
        }
        return promise;
      },
    },
  );
  equal(
    completionAfterHang,
    "0xafter-hang",
    `${name} did not recover from a bounded hanging status call`,
  );
  equal(hangingReads, 2, `${name} did not retry after a hanging status call`);

  await verifyIndeterminateCache(name, adapter, "signature");
  await verifyIndeterminateCache(name, adapter, "transaction");

  const deadlineClock = fakeClock();
  let deadlineReads = 0;
  const deadlineTimeouts = [];
  await rejects(
    () =>
      adapter.waitStatus(
        `${name}:deadline-final-observation-failure`,
        START_SECONDS + 2,
        {
          now: deadlineClock.now,
          wait: async () => {
            throw new Error("deadline failure path must not poll-sleep");
          },
          getStatus: async (_requestId, { timeoutMs }) => {
            deadlineReads += 1;
            check(
              timeoutMs > 0 && timeoutMs <= 3000,
              `${name} deadline status call did not receive an I/O bound`,
            );
            return new Promise(() => {});
          },
          withStatusTimeout: async (_promise, timeoutMs) => {
            deadlineTimeouts.push(timeoutMs);
            deadlineClock.advance(timeoutMs);
            throw new Error("status observation timed out");
          },
        },
      ),
    4001,
    /timed out/,
  );
  equal(
    deadlineReads,
    2,
    `${name} did not make exactly one bounded final status observation`,
  );
  equal(
    deadlineTimeouts[0],
    2000,
    `${name} initial status observation overran the canonical deadline`,
  );
  equal(
    deadlineTimeouts[1],
    3000,
    `${name} final status observation exceeded its wall-clock bound`,
  );
  equal(
    deadlineClock.now(),
    START_MS + 5000,
    `${name} exceeded the deadline plus one bounded final observation`,
  );
}

await verifyAdapter("selkies", {
  deadline: selkiesDeadlineMs,
  waitStatus: waitForSelkiesStatus,
  waitSignature: waitForSelkiesSignature,
  waitTransaction: waitForSelkiesTransaction,
  cacheApproval: cacheSelkiesApproval,
});
await verifyAdapter("playwright", {
  deadline: playwrightDeadlineMs,
  waitStatus: waitForPlaywrightStatus,
  waitSignature: waitForPlaywrightSignature,
  waitTransaction: waitForPlaywrightTransaction,
  cacheApproval: cachePlaywrightApproval,
});

check(assertions > 0, "wallet approval deadline smoke made no assertions");
const baselineAssertions = assertions;
assert.equal(
  baselineAssertions,
  671,
  "the accepted ESP-06A deterministic suite assertion count changed",
);
assert.equal(walletRuntimeErrorCodeForStatus(400), 4001);
assert.equal(walletRuntimeErrorCodeForStatus(504), -32603);
assert.equal(walletRuntimeErrorCodeForStatus(503), 4100);

await verifyStatusIoRepair("selkies", {
  waitStatus: waitForSelkiesStatus,
  waitSignature: waitForSelkiesSignature,
  waitTransaction: waitForSelkiesTransaction,
  cacheApproval: cacheSelkiesApproval,
});
await verifyStatusIoRepair("playwright", {
  waitStatus: waitForPlaywrightStatus,
  waitSignature: waitForPlaywrightSignature,
  waitTransaction: waitForPlaywrightTransaction,
  cacheApproval: cachePlaywrightApproval,
});

console.log(JSON.stringify({
  ok: true,
  adapters: 2,
  baseline_assertions: baselineAssertions,
  assertions,
  real_sleep_ms: 0,
}));
