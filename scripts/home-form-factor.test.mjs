import assert from "node:assert/strict";
import test from "node:test";

import {
  FORM_FACTORS,
  PHONE_MAX_WIDTH,
  TABLET_MAX_WIDTH,
  bindFormFactor,
  classifyFormFactor,
  formFactor,
  isCoarsePointer,
  isPhone,
  relayKeyboardInset,
  syncFormFactor,
} from "../capsules/home-gui/browser/shell-form-factor.js";

function fakeView({ width = 1280, height = 800, coarse = false, hover = true, visual = null } = {}) {
  const listeners = new Map();
  const on = (target, type, listener) => {
    listeners.set(`${target}:${type}`, [...(listeners.get(`${target}:${type}`) || []), listener]);
  };
  const off = (target, type, listener) => {
    listeners.set(`${target}:${type}`, (listeners.get(`${target}:${type}`) || []).filter((entry) => entry !== listener));
  };
  const view = {
    innerWidth: width,
    innerHeight: height,
    matchMedia: (query) => ({
      matches: (query === "(pointer: coarse)" && coarse) || (query === "(hover: none)" && !hover),
    }),
    addEventListener: (type, listener) => on("window", type, listener),
    removeEventListener: (type, listener) => off("window", type, listener),
    visualViewport: visual
      ? {
          ...visual,
          addEventListener: (type, listener) => on("visual", type, listener),
          removeEventListener: (type, listener) => off("visual", type, listener),
        }
      : undefined,
    listeners,
    fire(target, type) {
      for (const listener of listeners.get(`${target}:${type}`) || []) listener();
    },
  };
  return view;
}

function fakeDocument() {
  const vars = new Map();
  return {
    body: { dataset: {} },
    documentElement: { style: { setProperty: (name, value) => vars.set(name, value) } },
    vars,
  };
}

test("size classes split at 640 and 1100 inclusive", () => {
  assert.equal(PHONE_MAX_WIDTH, 640);
  assert.equal(TABLET_MAX_WIDTH, 1100);
  const matrix = [
    [320, "phone"],
    [390, "phone"],
    [640, "phone"],
    [641, "tablet"],
    [820, "tablet"],
    [1100, "tablet"],
    [1101, "desktop"],
    [1920, "desktop"],
  ];
  for (const [width, expected] of matrix) {
    assert.equal(classifyFormFactor(width), FORM_FACTORS[expected], `width ${width}`);
  }
  assert.equal(classifyFormFactor(Number.NaN), FORM_FACTORS.desktop, "unknown width fails closed to desktop");
  assert.equal(classifyFormFactor(undefined), FORM_FACTORS.desktop);
});

test("a phone held sideways stays a phone; a short desktop window does not", () => {
  assert.equal(classifyFormFactor(844, 390, true), "phone", "coarse landscape phone");
  assert.equal(classifyFormFactor(844, 390, false), "tablet", "fine pointer, short window");
  assert.equal(classifyFormFactor(1280, 600, false), "desktop", "short laptop window with a mouse");
  assert.equal(classifyFormFactor(1180, 820, true), "desktop", "tablet landscape is not a phone");
  assert.equal(classifyFormFactor(1100, 640, true), "phone", "height at the boundary counts");
  assert.equal(classifyFormFactor(700), "tablet", "height defaults to unknown");
});

test("formFactor and isPhone read width, height and pointer from the view", () => {
  assert.equal(formFactor(fakeView({ width: 390, height: 844 })), "phone");
  assert.equal(formFactor(fakeView({ width: 844, height: 390 })), "tablet", "no coarse pointer: width rules");
  assert.equal(formFactor(fakeView({ width: 844, height: 390, coarse: true, hover: false })), "phone");
  assert.equal(isPhone(fakeView({ width: 640, height: 900 })), true);
  assert.equal(isPhone(fakeView({ width: 641, height: 900 })), false);
});

test("pointer class comes from media features, not width", () => {
  assert.equal(isCoarsePointer(fakeView({ width: 390, coarse: true, hover: false })), true);
  assert.equal(isCoarsePointer(fakeView({ width: 1920, coarse: true, hover: false })), true, "touch monitor is coarse");
  assert.equal(isCoarsePointer(fakeView({ width: 390, coarse: false, hover: true })), false, "narrow window with a mouse is fine");
  assert.equal(isCoarsePointer(fakeView({ width: 820, coarse: false, hover: false })), true, "hover: none alone counts");
  assert.equal(isCoarsePointer({ innerWidth: 390 }), false, "no matchMedia fails closed to fine");
});

test("syncFormFactor mirrors the facts and the host's keyboard inset onto body and :root", () => {
  const doc = fakeDocument();
  const phone = fakeView({ width: 390, height: 844, coarse: true, hover: false });
  assert.deepEqual(relayKeyboardInset(336, doc, phone), { factor: "phone", coarse: true, inset: 336 });
  assert.equal(doc.body.dataset.formFactor, "phone");
  assert.equal(doc.body.dataset.pointer, "coarse");
  assert.equal(doc.vars.get("--keyboard-inset"), "336px");
  assert.equal(relayKeyboardInset(335.6, doc, phone).inset, 336, "rounded to whole px");
  assert.equal(relayKeyboardInset(-40, doc, phone).inset, 0, "clamped at zero");
  assert.equal(relayKeyboardInset("336", doc, phone).inset, 0, "only a finite number counts");
  relayKeyboardInset(0, doc, phone);
});

test("a framed shell ignores its own visual viewport; only the relay moves the inset", () => {
  const doc = fakeDocument();
  syncFormFactor(doc, fakeView({ width: 390, height: 844, coarse: true, hover: false, visual: { height: 508, offsetTop: 0 } }));
  assert.equal(doc.vars.get("--keyboard-inset"), "0px");
});

test("desktop pinch-zoom never becomes a keyboard inset", () => {
  const doc = fakeDocument();
  relayKeyboardInset(450, doc, fakeView({ width: 1440, height: 900, coarse: false }));
  assert.equal(doc.body.dataset.formFactor, "desktop");
  assert.equal(doc.body.dataset.pointer, "fine");
  assert.equal(doc.vars.get("--keyboard-inset"), "0px");
  relayKeyboardInset(0, doc, fakeView({ width: 1440, height: 900, coarse: false }));
});

test("bindFormFactor syncs now and on resize, and unbinds cleanly", () => {
  const doc = fakeDocument();
  const view = fakeView({ width: 390, height: 844, coarse: true, hover: false });
  const unbind = bindFormFactor(doc, view);
  assert.equal(doc.body.dataset.formFactor, "phone");
  assert.equal(doc.vars.get("--keyboard-inset"), "0px");

  view.innerWidth = 844;
  view.innerHeight = 390;
  view.fire("window", "resize");
  assert.equal(doc.body.dataset.formFactor, "phone", "rotation to landscape keeps the phone class");

  view.innerWidth = 1180;
  view.innerHeight = 820;
  view.fire("window", "resize");
  assert.equal(doc.body.dataset.formFactor, "desktop", "a landscape tablet is the desktop class");

  unbind();
  for (const [key, entries] of view.listeners) {
    assert.equal(entries.length, 0, `listener left bound: ${key}`);
  }
});
