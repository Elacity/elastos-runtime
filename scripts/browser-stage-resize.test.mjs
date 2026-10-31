import assert from "node:assert/strict";
import test from "node:test";
import { browserStageSize, observeBrowserStage } from "../capsules/browser/browser/browser-resize.js";

function fixture() {
  let callback, owner = "page:1", visible = true, timerId = 0;
  const timers = new Map(), sent = [], errors = [];
  const stage = { clientWidth: 1001, clientHeight: 701, getClientRects: () => visible ? [{}] : [] };
  const observer = observeBrowserStage({ stage, getOwner: () => owner, initialSequence: 0,
    send(event, owner) { return new Promise((resolve, reject) => sent.push({ event, owner, resolve, reject })); },
    onError: error => errors.push(error),
    ResizeObserverClass: class { constructor(cb) { callback = cb; } observe() {} disconnect() {} },
    setTimer(fn, ms) { assert.equal(ms, 150); timers.set(++timerId, fn); return timerId; },
    clearTimer(id) { timers.delete(id); },
  });
  return { observer, sent, errors, stage,
    resize(width, height) { callback([{ target: stage, contentRect: { width, height } }]); },
    tick() { const callbacks = [...timers.values()]; timers.clear(); callbacks.forEach(fn => fn()); },
    owner(value) { owner = value; }, visible(value) { visible = value; },
    async settle(index, fail = false) { sent[index][fail ? "reject" : "resolve"](fail ? Error("resize failed") : { accepted: true }); await new Promise(resolve => setImmediate(resolve)); },
  };
}

test("CSS sizes round even and obey both dimension and pixel ceilings", () => {
  for (const [width, height, expected] of [[390, 1200, [390, 1200]], [1001, 701, [1002, 702]],
    [1, 1, [320, 240]], [99999, 99999, [3840, 2160]], [844, 338, [844, 338]]]) {
    const size = browserStageSize(width, height);
    assert.deepEqual([size.width, size.height], expected);
    assert.ok(size.width * size.height <= 8_294_400);
  }
  for (const size of [[0, 700], [390, 0], [-2, 700], [Infinity, 700], [NaN, 700]]) assert.equal(browserStageSize(...size), null);
});

test("first size is immediate; debounce and one in-flight request keep the latest size", async () => {
  const f = fixture(); f.observer.opened();
  assert.deepEqual(f.sent[0].event, { type: "resize", width: 1002, height: 702, sequence: 1 });
  f.resize(600, 600); f.resize(390, 1200); f.tick();
  assert.equal(f.sent.length, 1);
  await f.settle(0);
  assert.equal(f.sent.length, 2); assert.equal(f.sent[1].event.width, 390);
  f.resize(844, 338); await f.settle(1);
  assert.equal(f.sent.length, 2, "pending size waits for its settling deadline");
  f.tick(); assert.equal(f.sent[2].event.sequence, 3);
  await f.settle(2); f.resize(844, 338); f.tick();
  assert.equal(f.sent.length, 3, "unchanged committed size is skipped");
});

test("hidden, zero, closed and replaced stages cannot send pending geometry", async () => {
  const f = fixture(); f.visible(false); f.observer.opened(); f.tick(); assert.equal(f.sent.length, 0);
  f.visible(true); f.resize(0, 0); f.tick(); assert.equal(f.sent.length, 0);
  f.resize(390, 1200); f.owner("page:2"); f.tick(); assert.equal(f.sent.length, 0);
  f.observer.opened(); f.resize(844, 338); f.tick(); f.observer.stop(); await f.settle(0);
  assert.equal(f.sent.length, 1);
});

test("failed resize reports recovery and permits another request", async () => {
  const f = fixture(); f.observer.opened(); await f.settle(0, true);
  assert.equal(f.errors.length, 1); f.resize(1001, 701); f.tick();
  assert.equal(f.sent.length, 2); assert.equal(f.sent[1].event.sequence, 2); await f.settle(1);
});
