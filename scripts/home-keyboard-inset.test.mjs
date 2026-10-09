import assert from "node:assert/strict";
import test from "node:test";

import {
  KEYBOARD_INSET_MESSAGE,
  bindKeyboardInsetRelay,
  keyboardInset,
} from "../capsules/home/browser/home-keyboard-inset.js";

function listenerTarget() {
  const listeners = new Map();
  return {
    listeners,
    addEventListener: (type, listener) => listeners.set(type, [...(listeners.get(type) || []), listener]),
    removeEventListener: (type, listener) => listeners.set(type, (listeners.get(type) || []).filter((entry) => entry !== listener)),
    fire(type) {
      for (const listener of listeners.get(type) || []) listener();
    },
  };
}

function fakeView({ height = 844, visual = null } = {}) {
  return {
    ...listenerTarget(),
    innerHeight: height,
    visualViewport: visual ? { ...visual, ...listenerTarget() } : undefined,
  };
}

function recordingPost(accepts = () => true) {
  const sent = [];
  const post = (message) => {
    if (!accepts()) {
      return false;
    }
    sent.push(message);
    return true;
  };
  return { sent, post };
}

test("keyboard inset is the visual-viewport shortfall, never negative", () => {
  assert.equal(keyboardInset(fakeView({ height: 844 })), 0, "no visualViewport");
  assert.equal(keyboardInset(fakeView({ height: 844, visual: { height: 844, offsetTop: 0 } })), 0);
  assert.equal(keyboardInset(fakeView({ height: 844, visual: { height: 508, offsetTop: 0 } })), 336, "iOS keyboard");
  assert.equal(keyboardInset(fakeView({ height: 844, visual: { height: 508, offsetTop: 100 } })), 236, "scrolled visual viewport");
  assert.equal(keyboardInset(fakeView({ height: 844, visual: { height: 900, offsetTop: 0 } })), 0, "clamped at zero");
  assert.equal(keyboardInset(fakeView({ height: 844, visual: { height: 507.6, offsetTop: 0 } })), 336, "rounded to whole px");
});

test("the relay posts the inset now and on each change, once per value", () => {
  const view = fakeView({ height: 844, visual: { height: 844, offsetTop: 0 } });
  const { sent, post } = recordingPost();
  const unbind = bindKeyboardInsetRelay({ post, view });
  assert.deepEqual(sent, [{ type: KEYBOARD_INSET_MESSAGE, inset: 0 }]);

  view.visualViewport.height = 508;
  view.visualViewport.fire("resize");
  view.visualViewport.fire("scroll");
  assert.deepEqual(sent.map((message) => message.inset), [0, 336], "an unchanged inset is not re-sent");

  view.visualViewport.height = 844;
  view.fire("resize");
  assert.deepEqual(sent.map((message) => message.inset), [0, 336, 0]);
  unbind();
});

test("the relay retries while no shell takes it and re-sends when the shell reloads", () => {
  const view = fakeView({ height: 844, visual: { height: 508, offsetTop: 0 } });
  const frame = listenerTarget();
  let shellReady = false;
  const { sent, post } = recordingPost(() => shellReady);
  const unbind = bindKeyboardInsetRelay({ post, frame, view });
  assert.equal(sent.length, 0, "no shell yet");

  shellReady = true;
  view.visualViewport.fire("scroll");
  assert.deepEqual(sent.map((message) => message.inset), [336], "delivered once a shell takes it");

  frame.fire("load");
  assert.deepEqual(sent.map((message) => message.inset), [336, 336], "a reloaded shell starts from 0, so it hears the inset again");
  unbind();
});

test("the relay unbinds every listener", () => {
  const view = fakeView({ height: 844, visual: { height: 844, offsetTop: 0 } });
  const frame = listenerTarget();
  const unbind = bindKeyboardInsetRelay({ post: () => true, frame, view });
  unbind();
  for (const target of [view, view.visualViewport, frame]) {
    for (const [type, entries] of target.listeners) {
      assert.equal(entries.length, 0, `listener left bound: ${type}`);
    }
  }
});
