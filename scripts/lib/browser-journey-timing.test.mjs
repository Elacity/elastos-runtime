import assert from "node:assert/strict";
import test from "node:test";
import vm from "node:vm";
import { browserJourneyFirstFrameProbe } from "./browser-journey-timing.mjs";

test("first-frame clock is armed before video creation and records presentation rather than DOM readiness", () => {
  let attach, frame, video, disconnected = false;
  const window = {};
  const sandbox = { location: { pathname: "/apps/browser/" }, window,
    performance: { timeOrigin: 1000, now: () => 250 },
    document: { querySelector: () => video },
    MutationObserver: class { constructor(fn) { attach = fn; } observe() {} disconnect() { disconnected = true; } } };
  vm.runInNewContext(`(${browserJourneyFirstFrameProbe})()`, sandbox);
  assert.equal(window.__elastosBrowserFirstFrame, undefined);
  video = { requestVideoFrameCallback: fn => { frame = fn; } }; attach();
  assert.equal(disconnected, true);
  assert.equal(window.__elastosBrowserFirstFrame, undefined);
  frame(250, { presentedFrames: 1, width: 1280, height: 720 });
  assert.equal(window.__elastosBrowserFirstFrame.observed_at, 1250);
  assert.equal(window.__elastosBrowserFirstFrame.presented_frames, 1);
});
