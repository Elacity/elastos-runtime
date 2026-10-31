import assert from "node:assert/strict";
import test from "node:test";

import {
  bindBrowserInputSurface,
  plainTextFromClipboardEvent,
} from "./browser-input-surface.js";
import { selkiesMessagesForInput } from "./browser-input.js";

function clipboardEvent(types, values = {}) {
  return {
    clipboardData: {
      types,
      getData(type) {
        return values[type] ?? "";
      },
    },
  };
}

test("explicit ClipboardEvent accepts only text/plain", () => {
  assert.equal(
    plainTextFromClipboardEvent(
      clipboardEvent(["text/plain"], { "text/plain": "paste me" }),
    ),
    "paste me",
  );
  assert.equal(
    plainTextFromClipboardEvent(
      clipboardEvent(["text/html"], { "text/html": "<b>no</b>" }),
    ),
    "",
  );
  assert.equal(
    plainTextFromClipboardEvent(
      clipboardEvent(["text"], { text: "alias is not accepted" }),
    ),
    "",
  );
});

test("missing or malformed ClipboardEvent data is rejected", () => {
  assert.equal(plainTextFromClipboardEvent(null), "");
  assert.equal(plainTextFromClipboardEvent({ clipboardData: {} }), "");
  assert.equal(
    plainTextFromClipboardEvent({
      clipboardData: {
        types: ["text/plain"],
        getData() {
          return null;
        },
      },
    }),
    "",
  );
});

function fakeTarget(extra = {}) {
  const listeners = new Map();
  return {
    ...extra,
    addEventListener(type, listener) {
      if (!listeners.has(type)) listeners.set(type, []);
      listeners.get(type).push(listener);
    },
    fire(type, init = {}) {
      const event = {
        type,
        defaultPrevented: false,
        preventDefault() { this.defaultPrevented = true; },
        stopPropagation() {},
        getModifierState: () => false,
        target: this,
        ...init,
      };
      for (const listener of listeners.get(type) || []) listener(event);
      return event;
    },
  };
}

function surface({ datachannel = true } = {}) {
  globalThis.document = fakeTarget({ visibilityState: "visible" });
  globalThis.window = fakeTarget({ setTimeout, clearTimeout });
  const inputs = [];
  const staleChecks = [];
  const frames = [];
  const captured = new Set();
  // Mirrors browser.js: input is attributed to the page current when it is
  // sent, and a release for another page's drag is dropped.
  const pages = { current: { page_id: "page:old" } };
  const sent = [];
  const remoteVideo = fakeTarget({
    hidden: false,
    videoWidth: 1280,
    videoHeight: 720,
    contains: () => false,
    getBoundingClientRect: () => ({ left: 0, top: 0, width: 1280, height: 720 }),
  });
  const renderPanel = fakeTarget({
    focus() {},
    setPointerCapture: (id) => captured.add(id),
    hasPointerCapture: (id) => captured.has(id),
    releasePointerCapture: (id) => captured.delete(id),
  });
  bindBrowserInputSurface({
    copyRemoteClipboardToHost: async () => inputs.push({ type: "copy" }),
    friendlyOpenError: (error) => String(error),
    getCurrentPage: () => pages.current,
    getInputOwner: () => pages.current,
    sameInputOwner: (left, right) => left === right,
    getCurrentView: () => ({ width: 1280, height: 720 }),
    keyboardCapture: null,
    pasteHostClipboardIntoRemote: async () => {},
    readHostClipboardText: async () => "",
    remoteVideo,
    renderPanel,
    releaseDragButton: (event, owner) => {
      if (owner !== pages.current) return;
      inputs.push(event);
      sent.push({ page: pages.current.page_id, ...event });
    },
    sendBrowserInput: async (event, options = {}) => {
      if (options.isStale) staleChecks.push(options.isStale);
      inputs.push(event);
      sent.push({ page: pages.current.page_id, ...event });
    },
    showStatus: () => {},
    streamsPointerDrag: () => datachannel,
    unlockRemoteAudioFromGesture: () => {},
    requestAnimationFrameFn: (callback) => frames.push(callback),
    cancelAnimationFrameFn: () => {},
  });
  const flushFrames = () => frames.splice(0).forEach((callback) => callback());
  const mouse = (type, clientX, clientY, extra = {}) =>
    renderPanel.fire(type, { pointerType: "mouse", pointerId: 1, button: 0, clientX, clientY, ...extra });
  return { inputs, sent, pages, staleChecks, renderPanel, remoteVideo, mouse, flushFrames, captured };
}

test("mouse event encodes as one Selkies held-button or release message", () => {
  assert.deepEqual(selkiesMessagesForInput({ type: "mouse", x: 10.4, y: 20.6, buttons: 1 }), ["m,10,21,1,0"]);
  assert.deepEqual(selkiesMessagesForInput({ type: "mouse", x: 30, y: 40, buttons: 0 }), ["m,30,40,0,0"]);
});

test("a drag stays with its page: a replaced page ends it without sending to the replacement", () => {
  // #482 fixture: down x=100 on old page, move x=120 queued, page replaced
  // before the frame, frame flushed, up x=140.
  const h = surface();
  h.mouse("pointerdown", 100, 100);
  h.mouse("pointermove", 120, 100);
  h.pages.current = { page_id: "page:replacement" };
  h.flushFrames();
  h.mouse("pointerup", 140, 100);
  assert.deepEqual(h.sent, [{ page: "page:old", type: "mouse", x: 100, y: 100, buttons: 1 }]);
  assert.equal(h.captured.size, 0, "the retired drag drops pointer capture");
});

test("a move after page replacement ends the drag, and a fresh gesture on the new page works", () => {
  const h = surface();
  h.mouse("pointerdown", 100, 100);
  h.mouse("pointermove", 120, 100);
  h.flushFrames();
  h.pages.current = { page_id: "page:replacement" };
  h.mouse("pointermove", 160, 100);
  h.flushFrames();
  h.mouse("pointerup", 170, 100);
  h.mouse("pointerdown", 200, 200);
  h.mouse("pointermove", 220, 200);
  h.flushFrames();
  h.mouse("pointerup", 230, 200);
  assert.deepEqual(h.sent, [
    { page: "page:old", type: "mouse", x: 100, y: 100, buttons: 1 },
    { page: "page:old", type: "mouse", x: 120, y: 100, buttons: 1 },
    { page: "page:replacement", type: "mouse", x: 200, y: 200, buttons: 1 },
    { page: "page:replacement", type: "mouse", x: 220, y: 200, buttons: 1 },
    { page: "page:replacement", type: "mouse", x: 230, y: 200, buttons: 0 },
  ]);
});

test("blur after page replacement retires the drag without a replacement-page release", () => {
  const h = surface();
  h.mouse("pointerdown", 100, 100);
  h.mouse("pointermove", 120, 100);
  h.pages.current = { page_id: "page:replacement" };
  globalThis.window.fire("blur");
  h.flushFrames();
  assert.deepEqual(h.sent, [{ page: "page:old", type: "mouse", x: 100, y: 100, buttons: 1 }]);
});

test("datachannel mouse drag sends press, frame-throttled moves, release, and no click", () => {
  const { inputs, renderPanel, mouse, flushFrames, captured } = surface();
  mouse("pointerdown", 100, 100);
  mouse("pointermove", 102, 101);
  assert.deepEqual(inputs, [], "movement under 4 px is not a drag");
  mouse("pointermove", 110, 100);
  mouse("pointermove", 150, 100);
  assert.ok(captured.has(1), "drag captures the pointer");
  flushFrames();
  mouse("pointermove", 200, 120);
  mouse("pointerup", 210, 120);
  flushFrames();
  renderPanel.fire("click", { clientX: 210, clientY: 120 });
  assert.deepEqual(inputs, [
    { type: "mouse", x: 100, y: 100, buttons: 1 },
    { type: "mouse", x: 150, y: 100, buttons: 1 },
    { type: "mouse", x: 210, y: 120, buttons: 0 },
  ]);
  assert.equal(captured.size, 0, "release drops pointer capture");
});

test("a press without movement keeps the Runtime click", () => {
  const { inputs, renderPanel, mouse } = surface();
  mouse("pointerdown", 100, 100);
  mouse("pointermove", 103, 100);
  mouse("pointerup", 103, 100);
  renderPanel.fire("click", { clientX: 103, clientY: 100 });
  assert.deepEqual(inputs, [{ type: "click", x: 103, y: 100 }]);
});

test("pointercancel during a drag sends the release", () => {
  const { inputs, mouse, flushFrames } = surface();
  mouse("pointerdown", 100, 100);
  mouse("pointermove", 120, 100);
  mouse("pointercancel", 0, 0);
  flushFrames();
  assert.deepEqual(inputs, [
    { type: "mouse", x: 100, y: 100, buttons: 1 },
    { type: "mouse", x: 120, y: 100, buttons: 0 },
  ]);
});

for (const [name, end] of [
  ["lostpointercapture", (h) => h.mouse("lostpointercapture", 0, 0)],
  ["window blur", () => globalThis.window.fire("blur")],
  ["pagehide", () => globalThis.window.fire("pagehide")],
  ["hidden tab", () => {
    globalThis.document.visibilityState = "hidden";
    globalThis.document.fire("visibilitychange");
  }],
]) test(`${name} during a drag sends the release`, () => {
  const h = surface();
  h.mouse("pointerdown", 100, 100);
  h.mouse("pointermove", 120, 100);
  end(h);
  h.flushFrames();
  assert.deepEqual(h.inputs, [
    { type: "mouse", x: 100, y: 100, buttons: 1 },
    { type: "mouse", x: 120, y: 100, buttons: 0 },
  ]);
});

test("a visible-tab visibilitychange does not end the drag", () => {
  const h = surface();
  h.mouse("pointerdown", 100, 100);
  h.mouse("pointermove", 120, 100);
  globalThis.document.fire("visibilitychange");
  h.flushFrames();
  assert.deepEqual(h.inputs.map((input) => input.buttons), [1, 1]);
});

test("a drag leaving the video clamps to its edge", () => {
  const h = surface();
  h.mouse("pointerdown", 100, 100);
  h.mouse("pointermove", 1400, -50);
  h.flushFrames();
  h.mouse("pointerup", 1500, 800);
  assert.deepEqual(h.inputs, [
    { type: "mouse", x: 100, y: 100, buttons: 1 },
    { type: "mouse", x: 1280, y: 0, buttons: 1 },
    { type: "mouse", x: 1280, y: 720, buttons: 0 },
  ]);
});

test("queued moves of an ended drag are stale", () => {
  const h = surface();
  h.mouse("pointerdown", 100, 100);
  h.mouse("pointermove", 120, 100);
  assert.ok(h.staleChecks.length > 0 && h.staleChecks.every((isStale) => !isStale()));
  h.mouse("pointerup", 130, 100);
  assert.ok(h.staleChecks.every((isStale) => isStale()));
});

test("Runtime-route input falls back to today's click without drag messages", () => {
  const { inputs, renderPanel, mouse, flushFrames } = surface({ datachannel: false });
  mouse("pointerdown", 100, 100);
  mouse("pointermove", 200, 100);
  mouse("pointerup", 200, 100);
  flushFrames();
  renderPanel.fire("click", { clientX: 200, clientY: 100 });
  assert.deepEqual(inputs, [{ type: "click", x: 200, y: 100 }]);
});

test("touch pan and non-primary buttons do not start a mouse drag", () => {
  const { inputs, mouse, flushFrames } = surface();
  mouse("pointerdown", 100, 100, { button: 2 });
  mouse("pointermove", 200, 100);
  flushFrames();
  assert.deepEqual(inputs, []);
});

test("Cmd+A and Ctrl+A send the guest Ctrl+A select-all", () => {
  const { inputs, renderPanel } = surface();
  const meta = renderPanel.fire("keydown", { key: "a", metaKey: true });
  renderPanel.fire("keydown", { key: "A", ctrlKey: true, shiftKey: true });
  renderPanel.fire("keydown", { key: "a", metaKey: true, altKey: true });
  assert.ok(meta.defaultPrevented);
  assert.deepEqual(inputs, [
    { type: "key_combo", keysyms: [65507, 97] },
    { type: "key_combo", keysyms: [65507, 97] },
  ]);
});
