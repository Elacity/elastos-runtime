import assert from "node:assert/strict";
import test from "node:test";
import { browserStageSize, observeBrowserStage, createBrowserFrameFence, RESIZE_RETRY_MS } from "../capsules/browser/browser/browser-resize.js";

function fixture() {
  let callback, owner = "page:1", visible = true, timerId = 0;
  const timers = new Map(), delays = [], sent = [], errors = [];
  const stage = { clientWidth: 1001, clientHeight: 701, getClientRects: () => visible ? [{}] : [] };
  const observer = observeBrowserStage({ stage, getOwner: () => owner, initialSequence: 0,
    send(event, owner) { return new Promise((resolve, reject) => sent.push({ event, owner, resolve, reject })); },
    onError: error => errors.push(error),
    ResizeObserverClass: class { constructor(cb) { callback = cb; } observe() {} disconnect() {} },
    setTimer(fn, ms) { assert.ok(ms === 150 || ms === RESIZE_RETRY_MS); delays.push(ms); timers.set(++timerId, fn); return timerId; },
    clearTimer(id) { timers.delete(id); },
  });
  return { observer, sent, errors, stage, delays,
    resize(width, height) { callback([{ target: stage, contentRect: { width, height } }]); },
    tick() { const callbacks = [...timers.values()]; timers.clear(); callbacks.forEach(fn => fn()); },
    owner(value) { owner = value; }, visible(value) { visible = value; },
    async settle(index, fail = false) { sent[index][fail ? "reject" : "resolve"](fail ? Error("resize failed") : { accepted: true }); await new Promise(resolve => setImmediate(resolve)); },
  };
}

test("CSS sizes round even and obey both dimension and pixel ceilings", () => {
  for (const [width, height, expected] of [[390, 1200, [390, 1200]], [1001, 701, [1002, 702]],
    [1, 1, [320, 240]], [99999, 99999, [1920, 1080]], [844, 338, [844, 338]]]) {
    const size = browserStageSize(width, height);
    assert.deepEqual([size.width, size.height], expected);
    assert.ok(size.width * size.height <= 2_073_600);
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

test("failed resize retries once, then reports and lets the same size be sent again", async () => {
  const f = fixture(); f.observer.opened(); await f.settle(0);
  f.resize(390, 1200); f.tick(); await f.settle(1, true);
  assert.equal(f.errors.length, 0, "the first failure is retried before it is reported");
  assert.equal(f.delays.at(-1), RESIZE_RETRY_MS);
  f.tick();
  assert.deepEqual(f.sent[2].event, { type: "resize", width: 390, height: 1200, sequence: 3 });
  await f.settle(2, true);
  assert.equal(f.errors.length, 1);
  f.tick(); assert.equal(f.sent.length, 3, "a failed retry is not retried again");
  f.resize(1001, 701); f.tick();
  assert.equal(f.sent.length, 4, "an unknown guest geometry cannot skip the earlier size");
  assert.equal(f.sent[3].event.sequence, 4); await f.settle(3);
});

test("a resize retry succeeds silently; a newer size or a replaced page supersedes it", async () => {
  const f = fixture(); f.observer.opened(); await f.settle(0, true);
  f.tick(); assert.equal(f.sent.length, 2); await f.settle(1);
  assert.equal(f.errors.length, 0);
  f.resize(1001, 701); f.tick(); assert.equal(f.sent.length, 2, "the retried size is now applied");
  f.resize(390, 1200); f.tick(); await f.settle(2, true);
  f.resize(844, 338); f.tick();
  assert.deepEqual([f.sent[3].event.width, f.sent[3].event.height], [844, 338]);
  await f.settle(3, true); f.owner("page:2"); f.tick();
  assert.equal(f.sent.length, 4, "a retry for a replaced page is dropped");
  assert.equal(f.errors.length, 0);
});

const turn = async (count = 4) => { for (let i = 0; i < count; i++) await new Promise(resolve => setImmediate(resolve)); };
function fenceFixture(video, onWait = () => {}) {
  const clock = { now: 0 };
  const fence = createBrowserFrameFence(video, { now: () => clock.now, timeout: 100,
    wait: async ms => { clock.now += ms; onWait(fence); await turn(1); } });
  return { fence, clock };
}
const ack = (width, height) => async () => ({ accepted: true, width, height });

test("frame fence clears when the decoded frame shows the ACKed geometry", async () => {
  const video = { videoWidth: 1000, videoHeight: 700 };
  let ticks = 0;
  const { fence } = fenceFixture(video, fence => { assert.equal(fence.blocked(), true);
    if (++ticks === 3) Object.assign(video, { videoWidth: 390, videoHeight: 1200 }); });
  await fence.run(ack(390, 1200));
  assert.equal(ticks, 3); assert.equal(fence.blocked(), false);
});

test("a frame later than the deadline clears the fence; input stays blocked until then", async () => {
  const video = { videoWidth: 1000, videoHeight: 700 };
  const { fence, clock } = fenceFixture(video);
  await fence.run(ack(390, 1200));
  assert.ok(clock.now >= 100); assert.equal(fence.blocked(), true);
  Object.assign(video, { videoWidth: 390, videoHeight: 1200 }); await turn();
  assert.equal(fence.blocked(), false);
});

test("no frame at the ACKed size keeps input blocked until the page is reset", async () => {
  const video = { videoWidth: 1000, videoHeight: 700 };
  const { fence } = fenceFixture(video);
  await fence.run(ack(844, 338)); await turn(8);
  assert.equal(fence.blocked(), true);
  fence.reset(); assert.equal(fence.blocked(), false);
  Object.assign(video, { videoWidth: 844, videoHeight: 338 }); await turn();
  assert.equal(fence.blocked(), false);
});

test("a replaced page ends the old wait; old geometry cannot clear the new page", async () => {
  const video = { videoWidth: 1000, videoHeight: 700 };
  const { fence } = fenceFixture(video);
  await fence.run(ack(390, 1200));
  assert.equal(fence.blocked(), true);
  fence.reset(); assert.equal(fence.blocked(), false);
  let release;
  const replaced = fence.run(() => new Promise(resolve => { release = resolve; }));
  fence.reset(); release({ accepted: true, width: 390, height: 1200 }); await replaced;
  assert.equal(fence.blocked(), false, "an ACK for a replaced page has no effect");
  const next = fence.run(ack(844, 338));
  Object.assign(video, { videoWidth: 390, videoHeight: 1200 }); await next; await turn();
  assert.equal(fence.blocked(), true);
  Object.assign(video, { videoWidth: 844, videoHeight: 338 }); await turn();
  assert.equal(fence.blocked(), false);
});

test("an unknown result keeps input blocked; a refusal keeps the earlier geometry", async () => {
  const video = { videoWidth: 1000, videoHeight: 700 };
  const { fence } = fenceFixture(video);
  const refusal = async () => ({ accepted: false, resize_error: { code: "resize_dialog_open" } });
  await assert.rejects(fence.run(async () => { throw Error("network"); }));
  assert.equal(fence.blocked(), true);
  await fence.run(refusal); assert.equal(fence.blocked(), true, "a refusal after an unknown result proves nothing");
  await fence.run(async () => undefined); assert.equal(fence.blocked(), true);
  await fence.run(ack(1000, 700)); assert.equal(fence.blocked(), false);
  await fence.run(refusal); assert.equal(fence.blocked(), false);
  await fence.run(ack(844, 338)); await fence.run(refusal);
  assert.equal(fence.blocked(), true, "a refusal keeps the ACKed size that has not decoded yet");
  Object.assign(video, { videoWidth: 844, videoHeight: 338 }); await turn();
  assert.equal(fence.blocked(), false);
});

test("a resize that fails twice returns pointer input to the geometry the guest still shows", async () => {
  const video = { videoWidth: 1000, videoHeight: 700 };
  const { fence } = fenceFixture(video);
  const timers = new Map(), requests = [], errors = [];
  let timerId = 0;
  const observer = observeBrowserStage({ stage: { clientWidth: 390, clientHeight: 1200, getClientRects: () => [{}] },
    getOwner: () => "page:1", initialSequence: 0,
    send: event => fence.run(() => new Promise((resolve, reject) => requests.push({ event, resolve, reject })), event),
    onError: error => errors.push({ error, kept: fence.recover() }),
    ResizeObserverClass: class { observe() {} disconnect() {} },
    setTimer(fn) { timers.set(++timerId, fn); return timerId; }, clearTimer(id) { timers.delete(id); } });
  observer.opened();
  requests[0].reject(Error("Browser Engine returned an invalid resize receipt")); await turn();
  assert.equal(fence.blocked(), true, "pointer input waits for the pending retry");
  assert.equal(errors.length, 0);
  [...timers.values()].forEach(fn => fn()); timers.clear();
  assert.deepEqual(requests[1].event, { type: "resize", width: 390, height: 1200, sequence: 2 });
  requests[1].reject(Error("Browser Engine returned an invalid resize receipt")); await turn();
  assert.deepEqual(errors.map(entry => entry.kept), [{ width: 1000, height: 700 }]);
  assert.equal(fence.blocked(), false, "the decoded frame already shows the kept geometry");
  assert.equal(timers.size, 0, "no further automatic retry");
  observer.stop();
});

test("recover waits for a frame of the earlier or the requested geometry", async () => {
  const video = { videoWidth: 1000, videoHeight: 700 };
  const { fence } = fenceFixture(video);
  const fail = async () => { throw Error("network"); };
  await assert.rejects(fence.run(fail, { width: 390, height: 1200 }));
  Object.assign(video, { videoWidth: 640, videoHeight: 480 });
  await assert.rejects(fence.run(fail, { width: 390, height: 1200 }));
  assert.deepEqual(fence.recover(), { width: 1000, height: 700 }, "the retry keeps the geometry from before the first failure");
  await turn(); assert.equal(fence.blocked(), true);
  Object.assign(video, { videoWidth: 390, videoHeight: 1200 }); await turn();
  assert.equal(fence.blocked(), false, "a guest that applied the request also releases input");
  assert.equal(fence.recover(), undefined);
});

test("recover with no decoded geometry releases input on the first decoded frame", async () => {
  const video = { videoWidth: 0, videoHeight: 0 };
  const { fence } = fenceFixture(video);
  await assert.rejects(fence.run(async () => { throw Error("network"); }, { width: 390, height: 1200 }));
  assert.equal(fence.recover(), null); await turn();
  assert.equal(fence.blocked(), true);
  Object.assign(video, { videoWidth: 1280, videoHeight: 720 }); await turn();
  assert.equal(fence.blocked(), false);
});

test("recover does nothing after a refusal or an accepted resize", async () => {
  const video = { videoWidth: 1000, videoHeight: 700 };
  const { fence } = fenceFixture(video);
  await fence.run(async () => ({ accepted: false, resize_error: { code: "resize_dialog_open" } }));
  assert.equal(fence.recover(), undefined); assert.equal(fence.blocked(), false);
  await assert.rejects(fence.run(async () => { throw Error("network"); }, { width: 390, height: 1200 }));
  await fence.run(ack(1000, 700));
  assert.equal(fence.recover(), undefined); assert.equal(fence.blocked(), false);
});

test("display setup has no resize owner; first ready frame sends the stage size", () => {
  const f = fixture(); f.owner(null); f.observer.opened(); f.resize(1000, 700); f.tick();
  assert.equal(f.sent.length, 0);
  f.owner("ready-page"); f.observer.opened(); assert.equal(f.sent.length, 1);
});
