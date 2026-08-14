import assert from "node:assert/strict";
import test from "node:test";
import {
  nextEventCursor,
  nextAppliedCursor,
  selectUnseenRunEvents,
} from "./agent-run-cursor.js";

test("cursor advances by event count when provider reports 0/null/empty", () => {
  assert.equal(nextEventCursor(0, 0, 500), 500);
  assert.equal(nextEventCursor(0, null, 500), 500);
  assert.equal(nextEventCursor(0, "", 500), 500);
  assert.equal(nextEventCursor(0, undefined, 500), 500);
});

test("cursor never goes backwards after applying events", () => {
  const once = nextEventCursor(0, 0, 500);
  assert.equal(nextEventCursor(once, 0, 20), 520);
  assert.equal(nextEventCursor(520, 520, 0), 520);
});

test("provider cursor may skip ahead only when this poll applied nothing", () => {
  assert.equal(nextAppliedCursor(500, 0, 800), 800);
  assert.equal(nextAppliedCursor(0, 256, 800), 256);
  assert.equal(nextEventCursor(500, 500, 10), 510);
});

test("full-log replay is sliced from applied, not re-applied", () => {
  const log = Array.from({ length: 100 }, (_, i) => ({ i }));
  assert.equal(selectUnseenRunEvents(0, log, 100).length, 100);
  const second = selectUnseenRunEvents(100, log, 100);
  assert.equal(second.length, 0);
  const grown = selectUnseenRunEvents(100, [...log, { i: 100 }, { i: 101 }], 102);
  assert.deepEqual(grown.map((e) => e.i), [100, 101]);
});

test("server-sliced batches pass through even when longer than applied", () => {
  const remaining = Array.from({ length: 400 }, (_, i) => ({ i: 256 + i }));
  const unseen = selectUnseenRunEvents(256, remaining, 10000);
  assert.equal(unseen.length, 256);
  assert.equal(unseen[0].i, 256);
  assert.equal(unseen[255].i, 511);
});

test("one poll never applies more than 256 events", () => {
  const log = Array.from({ length: 10_000 }, (_, i) => ({ i }));
  const first = selectUnseenRunEvents(0, log, 10_000);
  assert.equal(first.length, 256);
  assert.equal(first[0].i, 0);
  assert.equal(first[255].i, 255);
  const next = selectUnseenRunEvents(256, log, 10_000);
  assert.equal(next.length, 256);
  assert.equal(next[0].i, 256);
});

test("replay + reported cursor 0 does not grow applied", () => {
  const applied = 100;
  const unseen = selectUnseenRunEvents(
    applied,
    Array.from({ length: 100 }, (_, i) => ({ i })),
    0,
  );
  assert.equal(unseen.length, 0);
  assert.equal(nextAppliedCursor(applied, unseen.length, 0), 100);
});
