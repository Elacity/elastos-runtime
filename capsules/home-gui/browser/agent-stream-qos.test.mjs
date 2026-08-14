import assert from "node:assert/strict";
import test from "node:test";
import {
  createStreamQos,
  makeThinkStormFixture,
  playThinkStorm,
  recoverStalePersistedTurn,
  MAX_DISPATCH_DEPTH,
} from "./agent-stream-qos.js";

function assertStormPass(metrics, { thinkCount, firstAnswerSlice }) {
  assert.equal(metrics.thinkEventsReceived, thinkCount);
  assert.equal(metrics.rawThinkMarkdownRenders, 0);
  assert.equal(metrics.rawThinkKatexRenders, 0);
  assert.ok(metrics.thinkUiCommits <= 2, `thinkUiCommits ${metrics.thinkUiCommits}`);
  assert.ok(metrics.answerEventsReceived > 0);
  assert.ok(metrics.answerUiCommits > 0);
  assert.ok(metrics.maxDispatchDepth <= 2, `maxDispatchDepth ${metrics.maxDispatchDepth}`);
  assert.equal(metrics.dispatchRecursionFailures, 0);
  assert.ok(firstAnswerSlice != null && firstAnswerSlice <= 1, `firstAnswerSlice ${firstAnswerSlice}`);
}

test("558 think-storm: progress coalesced, answer paints first slice, no markdown", () => {
  const events = makeThinkStormFixture({ thinkCount: 558 });
  const { metrics, answer, firstAnswerSlice } = playThinkStorm(events);
  assertStormPass(metrics, { thinkCount: 558, firstAnswerSlice });
  assert.ok(answer.join("").includes("| Culture |"));
  assert.ok(metrics.thinkUiCommits / metrics.thinkEventsReceived <= 2 / 558 + 1e-9);
});

test("5,000 think-storm stays bounded", () => {
  const events = makeThinkStormFixture({ thinkCount: 5_000 });
  const { metrics, firstAnswerSlice } = playThinkStorm(events);
  assertStormPass(metrics, { thinkCount: 5_000, firstAnswerSlice });
});

test("50,000 think-storm stays bounded", () => {
  const events = makeThinkStormFixture({ thinkCount: 50_000 });
  const { metrics, firstAnswerSlice } = playThinkStorm(events);
  assertStormPass(metrics, { thinkCount: 50_000, firstAnswerSlice });
});

test("flush must not recurse when handlers re-enter", () => {
  const qos = createStreamQos();
  qos.ingestEvent({ type: "thinking.delta", delta: "a" });
  qos.ingestEvent({ type: "answer.delta", delta: "hi" });
  let nested = 0;
  const first = qos.flushPresentation({
    applyAnswer: () => {
      nested += 1;
      qos.flushPresentation({ applyAnswer: () => {}, applyProgress: () => {} });
    },
    applyProgress: () => {},
  });
  assert.equal(nested, 1);
  assert.ok(qos.metrics.flushReentryBlocks >= 1);
  assert.equal(qos.metrics.maxDispatchDepth <= MAX_DISPATCH_DEPTH, true);
  assert.equal(first.answerApplied, 1);
});

test("thinking-only storm commits progress once, never per event", () => {
  const qos = createStreamQos();
  for (let i = 0; i < 558; i += 1) {
    qos.ingestEvent({ type: "thinking.delta", delta: `reasoning ${i} ` });
  }
  qos.flushPresentation({ applyAnswer: () => {}, applyProgress: () => {} });
  assert.equal(qos.metrics.thinkEventsReceived, 558);
  assert.equal(qos.metrics.thinkUiCommits, 1);
  assert.equal(qos.metrics.answerUiCommits, 0);
  assert.equal(qos.metrics.maxDispatchDepth, 1);
});

test("thinking events never enqueue answer work", () => {
  const qos = createStreamQos();
  for (let i = 0; i < 558; i += 1) {
    qos.ingestEvent({ type: "thinking.delta", delta: `reasoning ${i} ` });
  }
  assert.equal(qos.queues.answer.length, 0);
  assert.equal(qos.queues.latestProgress?.key, "thinking");
});

test("stale streaming lastTurn becomes interrupted on recover", () => {
  const recovered = recoverStalePersistedTurn({
    id: "s-1",
    lastTurn: { state: "streaming", turnId: "t1", providerRunId: "run:1" },
  });
  assert.equal(recovered.lastTurn.state, "interrupted");
  assert.equal(recovered.lastTurn.error, "client_stream_lost");
  const live = recoverStalePersistedTurn({
    id: "s-2",
    lastTurn: { state: "completed", turnId: "t2" },
  });
  assert.equal(live.lastTurn.state, "completed");
});
