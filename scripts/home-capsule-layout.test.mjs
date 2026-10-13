import assert from "node:assert/strict";
import test from "node:test";

import {
  SHELL_LAYOUT_MESSAGE,
  bindCapsuleLayout,
  capsuleFrameVisible,
  postShellLayout,
} from "../capsules/home-gui/browser/shell-capsule-layout.js";

class FakeFrame {
  constructor() {
    this.messages = [];
    this.isConnected = true;
    this.parentElement = null;
    this.dataset = {};
    this.style = {};
    this.hidden = false;
    this.getBoundingClientRect = () => ({ width: 200, height: 200, top: 0, left: 0, bottom: 200, right: 200 });
    this.contains = (node) => node === this;
    this.contentWindow = {
      postMessage: (message, targetOrigin) => this.messages.push({ message, targetOrigin }),
    };
  }
}

function fakeView({ width = 390, height = 844 } = {}) {
  const listeners = new Map();
  return {
    innerWidth: width,
    innerHeight: height,
    HTMLIFrameElement: FakeFrame,
    getComputedStyle: (node) => ({ display: "block", visibility: "visible", opacity: "1", ...node.style }),
    MutationObserver: class {
      constructor(callback) { this.callback = callback; }
      observe() { listeners.set("mutation", this.callback); }
      disconnect() { listeners.delete("mutation"); }
    },
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
    hidden: false,
    documentElement: {},
    querySelectorAll: (selector) => (selector === "iframe" ? frames : []),
    addEventListener: (type, listener, capture) => listeners.set(`${type}:${capture === true}`, listener),
    removeEventListener: (type, _listener, capture) => listeners.delete(`${type}:${capture === true}`),
    load(target) {
      listeners.get("load:true")?.({ target });
    },
    listeners,
  };
}

test("postShellLayout sends the typed message and survives a torn-down frame", () => {
  const frame = new FakeFrame();
  postShellLayout(frame.contentWindow, { visible: true });
  assert.deepEqual(frame.messages, [
    { message: { type: SHELL_LAYOUT_MESSAGE, layout: { visible: true } }, targetOrigin: "*" },
  ]);
  assert.doesNotThrow(() => postShellLayout({ postMessage: () => { throw new Error("detached"); } }, {}));
  assert.doesNotThrow(() => postShellLayout(null, {}));
});

test("bindCapsuleLayout posts to existing frames, to each frame as it loads, and only on a visibility change", () => {
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
  assert.deepEqual(late.messages.map((entry) => entry.message.layout), [{ visible: true }]);

  doc.load({ tagName: "IMG" });
  view.resize(400, 850);
  assert.equal(existing.messages.length, 1, "a resize with the same visibility posts nothing");

  view.resize(820, 1180);
  assert.deepEqual(existing.messages.at(-1).message.layout, { visible: true });
  assert.equal(late.messages.length, 1, "a resize that keeps a frame visible posts nothing");

  const beforeRequest = existing.messages.length;
  view.listeners.get("message")({ data:{ type:SHELL_LAYOUT_MESSAGE, request:true }, source:{} });
  assert.equal(existing.messages.length, beforeRequest, "foreign frame cannot request a snapshot");
  view.listeners.get("message")({ data:{ type:SHELL_LAYOUT_MESSAGE, request:true }, source:existing.contentWindow });
  assert.equal(existing.messages.length, beforeRequest + 1, "late capsule initialization gets a snapshot");
  unbind();
  assert.equal(doc.listeners.size, 0);
  assert.equal(view.listeners.size, 0);
});


test("Home presentation changes own frame visibility and teardown stops observations", () => {
  const frame = new FakeFrame();
  const parent = { dataset: {}, style: {}, parentElement: null, contains: node => node === frame };
  frame.parentElement = parent;
  const view = fakeView();
  const doc = fakeDocument([frame]);
  const unbind = bindCapsuleLayout(doc, view);
  const visible = () => frame.messages.at(-1).message.layout.visible;
  const mutate = () => view.listeners.get("mutation")([{ target: parent }]);
  assert.equal(visible(), true);
  for (const style of [{ visibility: "hidden" }, { display: "none" }, { opacity: "0" }]) {
    parent.style = style; mutate(); assert.equal(visible(), false);
    parent.style = {}; mutate(); assert.equal(visible(), true);
  }
  parent.dataset.spaceVisible = "false"; mutate(); assert.equal(visible(), false);
  parent.dataset.spaceVisible = "true"; mutate(); assert.equal(visible(), true);
  parent.hidden = true; mutate(); assert.equal(visible(), false);
  parent.hidden = false; mutate(); assert.equal(visible(), true);
  doc.hidden = true; doc.listeners.get("visibilitychange:false")(); assert.equal(visible(), false);
  doc.hidden = false; doc.listeners.get("visibilitychange:false")(); assert.equal(visible(), true);
  const count = frame.messages.length;
  view.listeners.get("mutation")([{ target: { contains: () => false } }]);
  assert.equal(frame.messages.length, count, "unrelated DOM changes post nothing");
  frame.isConnected = false;
  assert.equal(capsuleFrameVisible(frame, doc, view), false);
  unbind();
  assert.equal(view.listeners.size, 0);
  assert.equal(doc.listeners.size, 0);
});
