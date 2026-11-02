import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import {
  FORM_FACTORS,
  PHONE_LANDSCAPE_MAX_HEIGHT,
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
  assert.equal(classifyFormFactor(1100, 500, true), "phone", "height at the boundary counts");
  assert.equal(classifyFormFactor(1024, 600, true), "tablet", "a 7-inch tablet or short touch-laptop window is not a phone");
  assert.equal(classifyFormFactor(932, 501, true), "tablet", "just over a landscape phone's height");
  assert.equal(classifyFormFactor(700), "tablet", "height defaults to unknown");
  assert.equal(classifyFormFactor(1440, 600, true), "desktop", "short window on a wide touch laptop");
  assert.equal(classifyFormFactor(1101, 500, true), "desktop", "coarse landscape stops at tablet width");
});

test("the CSS phone media list mirrors classifyFormFactor", () => {
  const css = readFileSync(new URL("../capsules/home-gui/browser/style.css", import.meta.url), "utf8");
  const landscape = `(max-height: ${PHONE_LANDSCAPE_MAX_HEIGHT}px) and (max-width: ${TABLET_MAX_WIDTH}px)`;
  const list = `@media (max-width: ${PHONE_MAX_WIDTH}px), (pointer: coarse) and ${landscape}, (hover: none) and ${landscape} {`;
  assert.ok(css.includes(list), `style.css keeps the phone media list ${list}`);
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

// phones-r1 #3-#5: tablet and desktop keep the desktop shell byte-for-byte,
// whatever the pointer. Every touch or narrow change keys on the phone class.
test("a coarse tablet takes no keyboard inset; the phone does", () => {
  const doc = fakeDocument();
  const tablet = fakeView({ width: 820, height: 1180, coarse: true, hover: false });
  assert.deepEqual(relayKeyboardInset(336, doc, tablet), { factor: "tablet", coarse: true, inset: 0 });
  assert.equal(doc.vars.get("--keyboard-inset"), "0px");
  relayKeyboardInset(0, doc, tablet);
});

test("touch and narrow shell changes key on the phone class, never pointer or width alone", () => {
  const read = (path) => readFileSync(new URL(`../capsules/${path}`, import.meta.url), "utf8");
  const expose = read("home-gui/browser/shell-expose.js");
  assert.doesNotMatch(expose, /data(set)?\.pointer|coarse/i, "Mission Control reads isPhone(), not the pointer");
  const caption = expose.slice(expose.indexOf("function mountExposeCaption"), expose.indexOf("function clearExposeCaption"));
  assert.match(caption, /if \(!isPhone\(\)\) \{\s*caption\.textContent = text;\s*entry\.node\.appendChild\(caption\);\s*return;/,
    "off the phone the caption is the desktop's plain text and no card Close");
  assert.match(expose, /if \(isPhone\(\)\) \{\s*node\.style\.setProperty\("--expose-card-scale"/);

  const spotlight = read("home-gui/browser/shell-spotlight.js");
  assert.match(spotlight, /openOnClick = event\.pointerType === "touch" && isPhone\(\);/,
    "only a phone defers Spotlight touch activation to click");

  const css = read("home-gui/browser/style.css");
  const phoneList = css.indexOf("@media (max-width: 640px), (pointer: coarse)");
  const outside = css.slice(0, phoneList);
  assert.doesNotMatch(outside, /data-pointer|\(pointer: coarse\)|\(hover: none\)/, "no pointer-keyed shell CSS outside the phone list");
  for (const declaration of ["overscroll-behavior: none", "touch-action: manipulation", "-webkit-touch-callout: none", "translateX(-50%) scale(calc(1 / var(--expose-card-scale"]) {
    for (let at = outside.indexOf(declaration); at !== -1; at = outside.indexOf(declaration, at + 1)) {
      const rule = outside.slice(outside.lastIndexOf("}", at) + 1, at);
      assert.match(rule, /body\[data-form-factor="phone"\]/, `${declaration} outside a phone-scoped rule`);
    }
  }
  assert.match(css, /\.expose-caption \{[^}]*transform: translateX\(-50%\);/, "desktop caption keeps its plain transform");

  const people = read("people/browser/style.css");
  const peopleNarrow = people.slice(people.indexOf("@media (max-width: 760px)"));
  assert.doesNotMatch(peopleNarrow.slice(0, peopleNarrow.indexOf(".people-sidebar")), /grid-template-rows/,
    "a narrow desktop People window keeps the desktop sidebar row");
  const inbox = read("inbox/browser/index.html");
  assert.match(inbox, /\n\s*\.entry-split \{\s*grid-template-columns: 1fr;\s*grid-template-rows: minmax\(160px, 42%\) minmax\(0, 1fr\);/);
  assert.match(inbox, /html\[data-el-form-factor="phone"\] \.entry-rows \{\s*max-height: 40dvh;/);
  assert.doesNotMatch(inbox.replace(/html\[data-el-form-factor="phone"\] \.entry-rows \{[^}]*\}/, ""), /40dvh/);
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

test("crossing the phone boundary is one stage reset; the form factor changes only through its commit", async () => {
  const doc = fakeDocument();
  const view = fakeView({ width: 390, height: 844 });
  const resets = [];
  const unbind = bindFormFactor(doc, view, { reset: (commit) => new Promise((resolve) => resets.push({ commit, resolve })) });
  assert.equal(formFactor(view), "phone");

  view.innerWidth = 1280;
  view.innerHeight = 800;
  view.fire("window", "resize");
  await Promise.resolve();
  assert.equal(resets.length, 1, "one reset per crossing");
  assert.equal(isPhone(view), true, "the old form factor holds until the reset commits");
  assert.equal(doc.body.dataset.formFactor, "phone");
  relayKeyboardInset(300, doc, view);
  assert.equal(isPhone(view), true, "a keyboard relay never commits a crossing");
  relayKeyboardInset(0, doc, view);
  view.fire("window", "resize");
  await Promise.resolve();
  assert.equal(resets.length, 1, "resizes during a reset wait for it");

  resets[0].commit();
  assert.equal(formFactor(view), "desktop");
  assert.equal(doc.body.dataset.formFactor, "desktop");
  resets[0].resolve();
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.equal(resets.length, 1, "a settled viewport starts no second reset");

  view.innerWidth = 1000;
  view.fire("window", "resize");
  await Promise.resolve();
  assert.equal(resets.length, 1, "tablet and desktop are one side of the boundary");
  assert.equal(doc.body.dataset.formFactor, "tablet");
  unbind();
});

test("Notification Centre and Control Centre keep their base roles off a phone", () => {
  const node = () => {
    const attrs = new Map();
    return {
      attrs,
      setAttribute: (name, value) => attrs.set(name, value),
      removeAttribute: (name) => attrs.delete(name),
    };
  };
  const nodes = { "#notification-center": node(), "#control-centre": node(), "#clock": node() };
  const doc = { ...fakeDocument(), querySelector: (selector) => nodes[selector] || null };
  const view = fakeView({ width: 390, height: 844 });
  syncFormFactor(doc, view);
  assert.equal(nodes["#notification-center"].attrs.get("role"), "dialog");
  assert.equal(nodes["#control-centre"].attrs.get("aria-modal"), "false");
  assert.equal(nodes["#clock"].attrs.get("aria-haspopup"), "dialog");
  view.innerWidth = 1280;
  syncFormFactor(doc, view);
  for (const selector of ["#notification-center", "#control-centre"]) {
    assert.equal(nodes[selector].attrs.get("role"), "group", selector);
    assert.equal(nodes[selector].attrs.has("aria-modal"), false, selector);
  }
  assert.equal(nodes["#clock"].attrs.get("aria-haspopup"), "true");
  const template = readFileSync(new URL("../capsules/home-gui/browser/home-gui-template.html", import.meta.url), "utf8");
  for (const id of ["notification-center", "control-centre"]) {
    assert.match(template, new RegExp(`id="${id}"\\s+class="${id}"\\s+role="group"\\s+aria-label=`), `${id} ships its base role`);
  }
  const search = template.slice(template.indexOf('id="toolbar-spotlight"'), template.indexOf("</button>", template.indexOf('id="toolbar-spotlight"')));
  assert.doesNotMatch(search, /aria-(haspopup|expanded|controls)/, "Search ships its base markup; only the phone adds popup semantics (shell-spotlight.js)");
});
