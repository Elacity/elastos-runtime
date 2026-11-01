#!/usr/bin/env node
/* A Light/Dark or accent change in Home reaches the Assistant frame at once,
   like every window and rail, not only on the frame's next load. A change
   made while the frame is still loading is delivered by its ready push. */

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";

const assistantFrameWindow = { received: [], postMessage(message, target) { this.received.push({ message, target }); } };
const assistantFrame = {
  contentWindow: assistantFrameWindow,
  dataset: {},
  classList: { add() {}, remove() {} },
};
const byId = { "#assistant-space-frame": assistantFrame };
const messageListeners = [];

// shell-core and its siblings touch the document at import time; a tiny stub
// stands in for the shell.
globalThis.document = {
  body: { classList: { add() {}, remove() {}, contains: () => false, toggle() {} }, dataset: {} },
  documentElement: { style: { setProperty() {} }, dataset: {} },
  querySelector: (selector) => byId[selector] || null,
  querySelectorAll: () => [],
  addEventListener() {},
};
globalThis.window = {
  innerWidth: 1440,
  innerHeight: 900,
  matchMedia: () => ({ matches: false }),
  addEventListener(type, handler) {
    if (type === "message") messageListeners.push(handler);
  },
  localStorage: { getItem: () => null, setItem() {} },
};

// Same specifiers (including the cache-busting query) as home-gui.js, so the
// test drives the module instances Home GUI uses.
const version = "home-20261101d";
const core = await import(`../capsules/home-gui/browser/shell-core.js?v=${version}`);
const face = await import(`../capsules/home-gui/browser/shell-assistant-face.js?v=${version}`);

function sourceFunction(source, name) {
  const start = source.indexOf(`function ${name}(`);
  assert.ok(start >= 0, name);
  return source.slice(start, source.indexOf("\n}", start) + 2);
}

/* The real applyHomeGuiUiPreferences and its broadcast, with the rest of
   Home GUI stubbed out: no windows or rails are open. */
function homeGuiPreferences() {
  const gui = readFileSync(new URL("../capsules/home-gui/browser/home-gui.js", import.meta.url), "utf8");
  const context = vm.createContext({
    window: { elastosTheme: { set() {}, setAccent() {}, setAccentCustom() {} } },
    shellState: { windows: new Map() },
    walletRailFrame: () => null,
    inboxRailFrame: () => null,
    OPAQUE_FRAME_TARGET: "*",
    rememberSharedUiPreferences: core.rememberSharedUiPreferences,
    postToAssistantFrame: face.postToAssistantFrame,
    setDockAutoHide() {},
    setUiSoundsEnabled() {},
    setFocusModeEnabled() {},
  });
  for (const name of ["applyHomeGuiUiPreferences", "broadcastHomeGuiUiPreferences"]) {
    vm.runInContext(sourceFunction(gui, name), context);
  }
  return context.applyHomeGuiUiPreferences;
}

function frameSays(data) {
  for (const handler of messageListeners) handler({ source: assistantFrameWindow, data });
}

function themesReceived() {
  return assistantFrameWindow.received
    .filter(({ message }) => message.type === "elastos:ui-preference")
    .map(({ message, target }) => ({ theme: message.preferences.theme, target }));
}

// One deps object, bound once: the face keeps it by reference, so a later
// test can change what this Home has.
const faceDeps = {
  targetById: () => null,
  pushUiPreferencesToFrameWindow: core.pushUiPreferencesToFrameWindow,
};

test("a theme change reaches the Assistant frame without a reload", () => {
  face.bindAssistantFace(faceDeps);
  const applyHomeGuiUiPreferences = homeGuiPreferences();
  applyHomeGuiUiPreferences({ theme: "light" });

  // Launched but still loading: nothing is posted to a document that is not ready.
  assistantFrame.dataset.route = "http://localhost/apps/assistant/";
  frameSays({ type: "home-agent:hello", docId: "a".repeat(32) });
  applyHomeGuiUiPreferences({ theme: "dark" });
  assert.deepEqual(themesReceived(), []);

  // Ready: the frame gets the latest preference, including the change made while loading.
  frameSays({ type: "home-agent:ready", docId: "a".repeat(32) });
  assert.deepEqual(themesReceived(), [{ theme: "dark", target: "*" }]);

  // Every later change arrives at once, as it does for windows and rails.
  applyHomeGuiUiPreferences({ theme: "light" });
  assert.deepEqual(themesReceived(), [{ theme: "dark", target: "*" }, { theme: "light", target: "*" }]);
});

test("an Assistant link opens in Browser, or asks Home's link prompt when this Home has no Browser", () => {
  const opens = [];
  faceDeps.openTarget = (target, options) => opens.push({ target, options });
  byId["#assistant-toggle"] = { hidden: true };
  frameSays({ type: "home-agent:hello", docId: "b".repeat(32) });
  frameSays({ type: "home-agent:ready", docId: "b".repeat(32) });
  const linkReplies = () => assistantFrameWindow.received
    .filter(({ message }) => message.type === "home-agent:open-link")
    .map(({ message, target }) => ({ ...message, target }));
  const url = "https://docs.example/guide?q=1";

  faceDeps.targetById = (_summary, id) => (["assistant", "browser"].includes(id) ? { targetId: id } : null);
  face.syncAssistantFaceAvailability({});
  frameSays({ type: "home-agent:open-browser", url });
  assert.deepEqual(opens, [{ target: "browser", options: { query: { url } } }]);
  assert.deepEqual(linkReplies(), [], "with Browser the link never reaches the prompt");

  faceDeps.targetById = (_summary, id) => (id === "assistant" ? { targetId: id } : null);
  face.syncAssistantFaceAvailability({});
  frameSays({ type: "home-agent:open-browser", url });
  assert.equal(opens.length, 1, "no Browser app: nothing opens in Home GUI");
  assert.deepEqual(linkReplies(), [{ type: "home-agent:open-link", url, target: "*" }]);

  // A link the face would not hand to Browser is not handed to the prompt either.
  frameSays({ type: "home-agent:open-browser", url: "javascript:alert(1)" });
  frameSays({ type: "home-agent:open-browser", url, extra: true });
  assert.equal(linkReplies().length, 1);
  delete byId["#assistant-toggle"];
});
