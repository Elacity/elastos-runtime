import assert from "node:assert/strict";
import test from "node:test";

import {
  SHELL_LAYOUT_MESSAGE,
  bindCapsuleLayout,
  postShellLayout,
  shellLayout,
} from "../capsules/home-gui/browser/shell-capsule-layout.js";

class FakeFrame {
  constructor() {
    this.messages = [];
    this.contentWindow = {
      postMessage: (message, targetOrigin) => this.messages.push({ message, targetOrigin }),
    };
  }
}

function fakeView({ width = 390, height = 844, coarse = true } = {}) {
  const listeners = new Map();
  return {
    innerWidth: width,
    innerHeight: height,
    HTMLIFrameElement: FakeFrame,
    matchMedia: (query) => ({ matches: coarse && (query === "(pointer: coarse)" || query === "(hover: none)") }),
    addEventListener: (type, listener) => listeners.set(type, listener),
    removeEventListener: (type) => listeners.delete(type),
    resize(nextWidth, nextHeight) {
      this.innerWidth = nextWidth;
      this.innerHeight = nextHeight;
      listeners.get("resize")?.();
    },
    listeners,
  };
}

function fakeDocument(frames) {
  const listeners = new Map();
  return {
    querySelectorAll: (selector) => (selector === "iframe" ? frames : []),
    addEventListener: (type, listener, capture) => listeners.set(`${type}:${capture === true}`, listener),
    removeEventListener: (type, _listener, capture) => listeners.delete(`${type}:${capture === true}`),
    load(target) {
      listeners.get("load:true")?.({ target });
    },
    listeners,
  };
}

test("shellLayout reports the size class and pointer class", () => {
  assert.deepEqual(shellLayout(fakeView()), { formFactor: "phone", pointer: "coarse" });
  assert.deepEqual(shellLayout(fakeView({ width: 1440, height: 900, coarse: false })), { formFactor: "desktop", pointer: "fine" });
});

test("postShellLayout sends the typed message and survives a torn-down frame", () => {
  const frame = new FakeFrame();
  postShellLayout(frame.contentWindow, { formFactor: "phone", pointer: "coarse" });
  assert.deepEqual(frame.messages, [
    { message: { type: SHELL_LAYOUT_MESSAGE, layout: { formFactor: "phone", pointer: "coarse" } }, targetOrigin: "*" },
  ]);
  assert.doesNotThrow(() => postShellLayout({ postMessage: () => { throw new Error("detached"); } }, {}));
  assert.doesNotThrow(() => postShellLayout(null, {}));
});

test("bindCapsuleLayout posts to existing frames, to each frame as it loads, and only on a size-class change", () => {
  const existing = new FakeFrame();
  const frames = [existing];
  const view = fakeView();
  const doc = fakeDocument(frames);
  const unbind = bindCapsuleLayout(doc, view);
  assert.equal(existing.messages.length, 1, "a frame that loaded before binding gets the layout");
  assert.ok(doc.listeners.has("load:true"), "load does not bubble, so the listener must capture");

  const late = new FakeFrame();
  frames.push(late);
  doc.load(late);
  assert.deepEqual(late.messages.map((entry) => entry.message.layout), [{ formFactor: "phone", pointer: "coarse" }]);

  doc.load({ tagName: "IMG" });
  view.resize(400, 850);
  assert.equal(existing.messages.length, 1, "a resize inside the same size class posts nothing");

  view.resize(820, 1180);
  assert.deepEqual(existing.messages.at(-1).message.layout, { formFactor: "tablet", pointer: "coarse" });
  assert.equal(late.messages.length, 2, "a size-class change reaches every frame");

  unbind();
  assert.equal(doc.listeners.size, 0);
  assert.equal(view.listeners.size, 0);
});
