import assert from "node:assert/strict";
import test from "node:test";

import {
  bindTouchLongPress,
  LONG_PRESS_CLICK_SWALLOW_MS,
  LONG_PRESS_DRIFT_PX,
  LONG_PRESS_MS,
} from "../capsules/home-gui/browser/shell-touch.js";

// A fake clock and timer queue, a root that runs capture listeners, and
// targets whose `contextmenu` listener opens (cancels) or ignores the event.
function harness({ opensMenu = true, skipped = false } = {}) {
  let time = 0;
  let nextTimer = 1;
  const timers = new Map();
  const listeners = new Map();
  const root = {
    addEventListener(type, listener, capture) {
      assert.equal(capture, true, `${type} must bind in the capture phase`);
      listeners.set(type, [...(listeners.get(type) || []), listener]);
    },
  };
  const received = [];
  const target = {
    closest: (selector) => (skipped && selector === ".desktop-shortcut" ? target : null),
    dispatchEvent(event) {
      received.push(event);
      if (opensMenu) {
        event.defaultPrevented = true;
      }
      return !event.defaultPrevented;
    },
  };
  const api = bindTouchLongPress(root, {
    skip: ".desktop-shortcut",
    setTimer(callback, ms) {
      const id = nextTimer++;
      timers.set(id, { callback, at: time + ms });
      return id;
    },
    clearTimer: (id) => timers.delete(id),
    now: () => time,
    makeContextMenuEvent: ({ clientX, clientY }) => ({ type: "contextmenu", clientX, clientY, defaultPrevented: false }),
  });
  const dispatch = (type, init = {}) => {
    const event = {
      target,
      pointerId: 1,
      pointerType: "touch",
      clientX: 100,
      clientY: 200,
      isTrusted: true,
      defaultPrevented: false,
      stopped: false,
      preventDefault() { this.defaultPrevented = true; },
      stopImmediatePropagation() { this.stopped = true; },
      ...init,
    };
    for (const listener of listeners.get(type) || []) {
      listener(event);
    }
    return event;
  };
  const advance = (ms) => {
    time += ms;
    for (const [id, timer] of [...timers]) {
      if (timer.at <= time) {
        timers.delete(id);
        timer.callback();
      }
    }
  };
  return { api, dispatch, advance, received, pendingTimers: () => timers.size };
}

test("holding a finger still dispatches one contextmenu at the finger", () => {
  const { dispatch, advance, received } = harness();
  dispatch("pointerdown");
  advance(LONG_PRESS_MS - 1);
  assert.equal(received.length, 0, "fires no earlier than the hold time");
  advance(1);
  assert.equal(received.length, 1);
  assert.deepEqual([received[0].clientX, received[0].clientY], [100, 200]);
  advance(LONG_PRESS_MS * 4);
  assert.equal(received.length, 1, "one hold, one menu");
});

test("the release after a long-press that opened a menu does not click", () => {
  const { dispatch, advance } = harness();
  dispatch("pointerdown");
  advance(LONG_PRESS_MS);
  dispatch("pointerup");
  const click = dispatch("click");
  assert.equal(click.defaultPrevented, true);
  assert.equal(click.stopped, true);
  assert.equal(dispatch("click").stopped, false, "only the one click is swallowed");
});

test("a slow tap on something without a menu still clicks", () => {
  const { dispatch, advance, received } = harness({ opensMenu: false });
  dispatch("pointerdown");
  advance(LONG_PRESS_MS);
  assert.equal(received.length, 1);
  dispatch("pointerup");
  assert.equal(dispatch("click").stopped, false);
});

test("a quick tap neither opens a menu nor loses its click", () => {
  const { dispatch, advance, received, pendingTimers } = harness();
  dispatch("pointerdown");
  advance(120);
  dispatch("pointerup");
  assert.equal(pendingTimers(), 0);
  assert.equal(dispatch("click").stopped, false);
  advance(LONG_PRESS_MS);
  assert.equal(received.length, 0);
});

test("drifting the drift distance cancels; a smaller wobble does not", () => {
  const drifted = harness();
  drifted.dispatch("pointerdown");
  drifted.dispatch("pointermove", { clientX: 100 + LONG_PRESS_DRIFT_PX, clientY: 200 });
  drifted.advance(LONG_PRESS_MS);
  assert.equal(drifted.received.length, 0);

  const wobbled = harness();
  wobbled.dispatch("pointerdown");
  wobbled.dispatch("pointermove", { clientX: 106, clientY: 206 });
  wobbled.advance(LONG_PRESS_MS);
  assert.equal(wobbled.received.length, 1);
});

test("pointercancel (the browser took the gesture to scroll) cancels", () => {
  const { dispatch, advance, received } = harness();
  dispatch("pointerdown");
  dispatch("pointercancel");
  advance(LONG_PRESS_MS);
  assert.equal(received.length, 0);
});

test("a mouse is left to the browser's own right-click", () => {
  const { dispatch, advance, received, pendingTimers } = harness();
  dispatch("pointerdown", { pointerType: "mouse" });
  assert.equal(pendingTimers(), 0);
  advance(LONG_PRESS_MS);
  assert.equal(received.length, 0);
});

test("pen presses long-press like touch", () => {
  const { dispatch, advance, received } = harness();
  dispatch("pointerdown", { pointerType: "pen" });
  advance(LONG_PRESS_MS);
  assert.equal(received.length, 1);
});

test("desktop icons keep their own long-press", () => {
  const { dispatch, advance, received, pendingTimers } = harness({ skipped: true });
  dispatch("pointerdown");
  assert.equal(pendingTimers(), 0);
  advance(LONG_PRESS_MS);
  assert.equal(received.length, 0);
});

test("a browser contextmenu after ours is dropped, so the menu opens once", () => {
  const { dispatch, advance } = harness();
  dispatch("pointerdown");
  advance(LONG_PRESS_MS);
  const native = dispatch("contextmenu");
  assert.equal(native.stopped, true);
  assert.equal(native.defaultPrevented, true);
});

test("a browser contextmenu before ours (Android) wins and ours stands down", () => {
  const { dispatch, advance, received, pendingTimers } = harness();
  dispatch("pointerdown");
  advance(LONG_PRESS_MS - 50);
  const native = dispatch("contextmenu");
  assert.equal(native.stopped, false, "the browser's own event reaches the menu binding");
  assert.equal(pendingTimers(), 0);
  advance(LONG_PRESS_MS);
  assert.equal(received.length, 0);
  dispatch("pointerup");
  assert.equal(dispatch("click").stopped, true, "the release still does not click");
});

test("a right-click with no press in flight is untouched", () => {
  const { dispatch } = harness();
  const native = dispatch("contextmenu");
  assert.equal(native.stopped, false);
  assert.equal(native.defaultPrevented, false);
});

test("swallowNextClick eats one click inside the window, not a later one", () => {
  const { api, dispatch, advance } = harness();
  api.swallowNextClick();
  assert.equal(dispatch("click").stopped, true);
  api.swallowNextClick();
  advance(LONG_PRESS_CLICK_SWALLOW_MS);
  assert.equal(dispatch("click").stopped, false);
});

test("another finger's pointer events do not disturb the press", () => {
  const { dispatch, advance, received } = harness();
  dispatch("pointerdown");
  dispatch("pointermove", { pointerId: 2, clientX: 300, clientY: 300 });
  dispatch("pointerup", { pointerId: 2 });
  advance(LONG_PRESS_MS);
  assert.equal(received.length, 1);
});
