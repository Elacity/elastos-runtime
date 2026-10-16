import assert from "node:assert/strict";
import test from "node:test";

// shell-core queries the document at import time; a tiny DOM stub stands in
// for the shell so the Dock decision logic can run under node:test.
function fakeClassList() {
  const names = new Set();
  return {
    add: (...values) => values.forEach((value) => names.add(value)),
    remove: (...values) => values.forEach((value) => names.delete(value)),
    contains: (value) => names.has(value),
    toggle: (value, force) => {
      const on = force === undefined ? !names.has(value) : Boolean(force);
      if (on) {
        names.add(value);
      } else {
        names.delete(value);
      }
      return on;
    },
    names,
  };
}

const launcherNode = { hidden: true };
const handleNode = { hidden: true };
const dockRow = { scrollLeft: 0, scrollWidth: 400, clientWidth: 400, style: { vars: {}, setProperty(name, value) { this.vars[name] = value; } } };
const byId = { "#launcher": launcherNode, "#taskbar-targets": dockRow, "#phone-dock-handle": handleNode };

const doc = {
  body: { classList: fakeClassList(), dataset: {} },
  documentElement: { style: { setProperty() {} } },
  querySelector: (selector) => byId[selector] || null,
  querySelectorAll: () => [],
  addEventListener() {},
};
globalThis.document = doc;
globalThis.window = {
  innerWidth: 390,
  innerHeight: 844,
  matchMedia: () => ({ matches: false }),
  addEventListener() {},
  localStorage: { getItem: () => null, setItem() {} },
};

// Same specifier (including the cache-busting query) as shell-phone-dock.js,
// otherwise Node instantiates a second shell-core with its own shellState.
const { shellState } = await import("../capsules/home-gui/browser/shell-core.js?v=home-20260813a");
const {
  DOCK_EDGE_FADE_PX,
  PHONE_DOCK_PEEK_CLASS,
  PHONE_DOCK_TUCKED_CLASS,
  hasVisibleWindow,
  hidePhoneDockPeek,
  peekPhoneDock,
  syncDockEdgeFades,
  syncPhoneDock,
} = await import("../capsules/home-gui/browser/shell-phone-dock.js");

const phoneView = { innerWidth: 390, innerHeight: 844, matchMedia: () => ({ matches: true }) };
const desktopView = { innerWidth: 1440, innerHeight: 900, matchMedia: () => ({ matches: false }) };

function windowEntry(id, { hidden = false, spaceVisible = true } = {}) {
  return {
    id,
    node: {
      classList: { contains: (name) => name === "hidden" && hidden },
      dataset: spaceVisible ? {} : { spaceVisible: "false" },
    },
  };
}

function reset() {
  shellState.windows.clear();
  shellState.activeWindowId = null;
  launcherNode.hidden = true;
  doc.body.classList.names.clear();
}

test("hasVisibleWindow ignores hidden and other-Space windows", () => {
  const windows = new Map([
    ["a", windowEntry("a", { hidden: true })],
    ["b", windowEntry("b", { spaceVisible: false })],
  ]);
  assert.equal(hasVisibleWindow(windows), false);
  windows.set("c", windowEntry("c"));
  assert.equal(hasVisibleWindow(windows), true);
});

test("the Dock tucks only on a phone with a visible window and the launcher closed", () => {
  reset();
  syncPhoneDock(doc, phoneView);
  assert.equal(doc.body.classList.contains(PHONE_DOCK_TUCKED_CLASS), false);
  assert.equal(handleNode.hidden, true);

  shellState.windows.set("a", windowEntry("a"));
  syncPhoneDock(doc, phoneView);
  assert.equal(doc.body.classList.contains(PHONE_DOCK_TUCKED_CLASS), true);
  assert.equal(handleNode.hidden, false);

  launcherNode.hidden = false;
  syncPhoneDock(doc, phoneView);
  assert.equal(doc.body.classList.contains(PHONE_DOCK_TUCKED_CLASS), false);

  launcherNode.hidden = true;
  syncPhoneDock(doc, desktopView);
  assert.equal(doc.body.classList.contains(PHONE_DOCK_TUCKED_CLASS), false, "desktop never tucks");
});

test("a peek survives summary refreshes but ends when focus moves or the window closes", () => {
  reset();
  shellState.windows.set("a", windowEntry("a"));
  shellState.activeWindowId = "a";
  syncPhoneDock(doc, phoneView);
  peekPhoneDock(doc);
  assert.equal(doc.body.classList.contains(PHONE_DOCK_PEEK_CLASS), true);

  syncPhoneDock(doc, phoneView);
  assert.equal(doc.body.classList.contains(PHONE_DOCK_PEEK_CLASS), true, "unrelated refresh keeps the peek");

  shellState.windows.set("b", windowEntry("b"));
  shellState.activeWindowId = "b";
  syncPhoneDock(doc, phoneView);
  assert.equal(doc.body.classList.contains(PHONE_DOCK_PEEK_CLASS), false, "picking a window tucks the Dock");

  peekPhoneDock(doc);
  shellState.windows.clear();
  syncPhoneDock(doc, phoneView);
  assert.equal(doc.body.classList.contains(PHONE_DOCK_PEEK_CLASS), false);
  assert.equal(doc.body.classList.contains(PHONE_DOCK_TUCKED_CLASS), false);
});

test("peek is a no-op unless the Dock is tucked; hide always clears it", () => {
  reset();
  peekPhoneDock(doc);
  assert.equal(doc.body.classList.contains(PHONE_DOCK_PEEK_CLASS), false);
  doc.body.classList.add(PHONE_DOCK_PEEK_CLASS);
  hidePhoneDockPeek(doc);
  assert.equal(doc.body.classList.contains(PHONE_DOCK_PEEK_CLASS), false);
});

test("edge fades appear only on the side with hidden icons", () => {
  syncDockEdgeFades(dockRow);
  assert.deepEqual(dockRow.style.vars, { "--dock-fade-start": "0px", "--dock-fade-end": "0px" });

  dockRow.scrollWidth = 600;
  syncDockEdgeFades(dockRow);
  assert.equal(dockRow.style.vars["--dock-fade-start"], "0px");
  assert.equal(dockRow.style.vars["--dock-fade-end"], `${DOCK_EDGE_FADE_PX}px`);

  dockRow.scrollLeft = 200;
  syncDockEdgeFades(dockRow);
  assert.equal(dockRow.style.vars["--dock-fade-start"], `${DOCK_EDGE_FADE_PX}px`);
  assert.equal(dockRow.style.vars["--dock-fade-end"], "0px");
});
