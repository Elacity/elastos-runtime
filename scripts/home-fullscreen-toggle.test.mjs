import assert from "node:assert/strict";
import test from "node:test";

import { bindFullscreenToggle } from "../capsules/home-gui/browser/shell-fullscreen.js";

// A fake document whose Fullscreen API behaves like a browser: request/exit
// change `fullscreenElement` and fire `fullscreenchange`.
function harness({ api = true, prefixed = false } = {}) {
  const listeners = new Map();
  const doc = {
    fullscreenElement: null,
    webkitFullscreenElement: null,
    addEventListener(type, listener) {
      listeners.set(type, [...(listeners.get(type) || []), listener]);
    },
  };
  const fire = (type) => {
    for (const listener of listeners.get(type) || []) {
      listener({ type });
    }
  };
  const elementKey = prefixed ? "webkitFullscreenElement" : "fullscreenElement";
  const changeEvent = prefixed ? "webkitfullscreenchange" : "fullscreenchange";
  const root = {};
  if (api) {
    root[prefixed ? "webkitRequestFullscreen" : "requestFullscreen"] = async function request() {
      assert.equal(this, root);
      doc[elementKey] = root;
      fire(changeEvent);
    };
    doc[prefixed ? "webkitExitFullscreen" : "exitFullscreen"] = async function exit() {
      assert.equal(this, doc);
      doc[elementKey] = null;
      fire(changeEvent);
    };
  }
  doc.documentElement = root;
  const attributes = new Map();
  const clicks = [];
  const button = {
    hidden: true,
    title: "",
    setAttribute: (name, value) => attributes.set(name, value),
    getAttribute: (name) => attributes.get(name) ?? null,
    addEventListener(type, listener) {
      assert.equal(type, "click");
      clicks.push(listener);
    },
  };
  const click = () => clicks.forEach((listener) => listener());
  return { doc, root, button, click, fire, elementKey, changeEvent };
}

function assertState(button, active) {
  const text = active ? "Exit full screen" : "Enter full screen";
  assert.equal(button.getAttribute("aria-pressed"), active ? "true" : "false");
  assert.equal(button.getAttribute("aria-label"), text);
  assert.equal(button.title, text);
}

for (const prefixed of [false, true]) {
  test(`the top-bar button enters and exits document full screen (${prefixed ? "webkit" : "standard"} API)`, async () => {
    const h = harness({ prefixed });
    const control = bindFullscreenToggle(h.button, h.doc);
    assert.ok(control);
    assert.equal(h.button.hidden, false);
    assertState(h.button, false);

    await control.toggle();
    assert.equal(h.doc[h.elementKey], h.root);
    assertState(h.button, true);

    await control.toggle();
    assert.equal(h.doc[h.elementKey], null);
    assertState(h.button, false);
  });
}

test("a click on the button requests full screen", async () => {
  const h = harness();
  bindFullscreenToggle(h.button, h.doc);
  h.click();
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(h.doc.fullscreenElement, h.root);
  assertState(h.button, true);
});

test("Esc or a browser exit updates the label and icon through fullscreenchange", () => {
  const h = harness();
  bindFullscreenToggle(h.button, h.doc);
  h.doc.fullscreenElement = h.root;
  h.fire("fullscreenchange");
  assertState(h.button, true);
  h.doc.fullscreenElement = null;
  h.fire("fullscreenchange");
  assertState(h.button, false);
});

test("a refused request leaves the button retryable and in sync", async () => {
  const h = harness();
  h.root.requestFullscreen = async () => {
    throw new Error("denied");
  };
  const control = bindFullscreenToggle(h.button, h.doc);
  await control.toggle();
  assertState(h.button, false);
});

test("without a Fullscreen API (iPhone Safari) the button stays hidden", () => {
  const h = harness({ api: false });
  assert.equal(bindFullscreenToggle(h.button, h.doc), null);
  assert.equal(h.button.hidden, true);
});
