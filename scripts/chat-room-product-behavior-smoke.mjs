#!/usr/bin/env node

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import vm from "node:vm";

const repoRoot = resolve(new URL("..", import.meta.url).pathname);
const chatIndexPath = resolve(repoRoot, "capsules/chat-room/browser/index.html");
const source = readFileSync(chatIndexPath, "utf8");

function extractScript(needle) {
  const scripts = [...source.matchAll(/<script src="\.\/([^"?]+)(?:\?[^"]*)?"><\/script>/g)]
    .map((entry) => readFileSync(resolve(repoRoot, "capsules/chat-room/browser", entry[1]), "utf8"));
  const script = scripts.find((entry) => entry.includes(needle));
  assert(script, `missing script for ${needle}`);
  return script;
}

class FakeElement {
  constructor(id = "") {
    this.id = id;
    this.hidden = false;
    this.disabled = false;
    this.dataset = {};
    this.style = {};
    this.listeners = new Map();
    this.children = [];
    this.clickCount = 0;
    this._query = new Map();
  }

  addEventListener(type, callback) {
    this.listeners.set(type, callback);
  }

  dispatchEvent(event) {
    const callback = this.listeners.get(event.type);
    if (callback) {
      callback(event);
    }
  }

  click() {
    this.clickCount += 1;
  }

  querySelector(selector) {
    return this._query.get(selector) || null;
  }

  querySelectorAll(selector) {
    if (selector === "[data-conversation-choice]") {
      return this.children;
    }
    return [];
  }
}

class FakeTextAreaElement extends FakeElement {
  constructor(id = "") {
    super(id);
    this.value = "";
  }

  get scrollHeight() {
    return this.value.includes("\n") ? 88 : 34;
  }
}

function createEnvironment() {
  const topMessages = [];
  const parentMessages = [];
  const windowListeners = new Map();
  const observers = [];
  const participantToggle = new FakeElement("participant-toggle");
  const roomAccessToggle = new FakeElement("room-access-toggle");
  roomAccessToggle.hidden = true;
  const conversationSelector = new FakeElement("conversation-selector");
  conversationSelector.children = [
    { dataset: { conversationChoice: "shared" } },
    { dataset: { conversationChoice: "direct:sha256:fixture-conversation" } },
  ];
  const conversationJoinSection = new FakeElement("conversation-join-section");
  conversationJoinSection.hidden = true;
  const composerForm = new FakeElement("composer-form");
  const composerField = new FakeElement("composer-field");
  const messageInput = new FakeTextAreaElement("message-input");
  composerForm._query.set(".composer-field", composerField);
  const body = {
    dataset: {},
    setAttribute(name, value) {
      if (name.startsWith("data-")) {
        const key = name
          .slice(5)
          .replace(/-([a-z])/g, (_, letter) => letter.toUpperCase());
        this.dataset[key] = String(value);
      }
    },
  };
  const topFrame = {
    postMessage(message, origin) {
      topMessages.push({ message, origin });
    },
  };
  const parentFrame = {
    postMessage(message, origin) {
      parentMessages.push({ message, origin });
    },
  };
  const nodes = new Map([
    ["participant-toggle", participantToggle],
    ["room-access-toggle", roomAccessToggle],
    ["conversation-selector", conversationSelector],
    ["conversation-join-section", conversationJoinSection],
    ["composer-form", composerForm],
    ["message-input", messageInput],
  ]);
  class FakeMutationObserver {
    constructor(callback) {
      this.callback = callback;
    }

    observe(target, options) {
      observers.push({ callback: this.callback, target, options });
    }
  }
  const context = {
    URLSearchParams,
    HTMLElement: FakeElement,
    HTMLTextAreaElement: FakeTextAreaElement,
    MutationObserver: FakeMutationObserver,
    document: {
      body,
      getElementById(id) {
        return nodes.get(id) || null;
      },
    },
    window: {
      location: {
        search: "?home_origin=http%3A%2F%2Fhome.example",
        hash: "#home_token=chat-test-token",
      },
      top: topFrame,
      parent: parentFrame,
      addEventListener(type, callback) {
        windowListeners.set(type, callback);
      },
    },
    globalThis: null,
    getComputedStyle(node) {
      if (node === messageInput) {
        return { lineHeight: "20px" };
      }
      return { lineHeight: "16px" };
    },
    console,
  };
  context.globalThis = context;
  vm.createContext(context);
  return {
    body,
    composerField,
    context,
    conversationJoinSection,
    conversationSelector,
    messageInput,
    notify(target) {
      for (const observer of observers) {
        if (observer.target === target) {
          observer.callback([{ target }]);
        }
      }
    },
    parentFrame,
    parentMessages,
    participantToggle,
    roomAccessToggle,
    topFrame,
    topMessages,
    windowListeners,
  };
}

function manifestCommands(manifest) {
  return manifest.menus.flatMap((menu) => menu.items.map((item) => item.cmd));
}

function plainJson(value) {
  return JSON.parse(JSON.stringify(value));
}

function checkChatStartup() {
  const startup = readFileSync(resolve(repoRoot, "capsules/chat-room/browser/chat-room.js"), "utf8")
    .replace(/^[ \t]*import\b[\s\S]*?;/gm, "")
    .replaceAll("import.meta.url", JSON.stringify("http://home.example/apps/chat-room/chat-room.js"));
  for (const token of ["", "chat-test-token"]) {
    const env = createEnvironment();
    const calls = [];
    env.context.window.location.hash = token ? `#home_token=${token}` : "";
    Object.assign(env.context, {
      URL,
      createHomeNavigationClient(options) {
        calls.push({ type: "navigation", options });
        return { setQuery(query) { calls.push({ type: "query", query }); } };
      },
      createHomeClipboardClient(options) {
        calls.push({ type: "clipboard", options });
        return {
          start() { calls.push({ type: "clipboard-start" }); },
          writeText(text, options) { calls.push({ type: "copy", text, options }); },
        };
      },
      init(options) { calls.push({ type: "wasm", url: options.module_or_path.href }); },
    });
    vm.runInContext(startup, env.context);
    if (!token) {
      assert.deepEqual(calls, [], "Unsigned Chat must keep bridge and WASM startup inactive");
      continue;
    }
    assert.deepEqual(calls.map(call => call.type), ["navigation", "clipboard", "clipboard-start", "wasm"]);
    assert.equal(calls[0].options.homeToken, token);
    assert.equal(calls[1].options.homeToken, token);
    assert.equal(calls[1].options.targetId, "chat-room");
    assert.equal(calls[3].url, "http://home.example/apps/chat-room/chat_room_ui_bg.wasm?v=chat-room-ui-20260804a");
    env.context.elastosChatNavigation({ conversation_id: "fixture" });
    env.context.elastosChatCopyInvite("fixture-invite");
    assert.deepEqual(plainJson(calls.slice(4)), [
      { type: "query", query: { conversation_id: "fixture" } },
      { type: "copy", text: "fixture-invite", options: { purpose: "conversation.invite" } },
    ]);
  }
  assert.match(source, /id="chat-open-home" href="\/home\/" target="_self"/);
  assert(!source.includes('id="browser-access-form"'), "Direct Chat must replace its unavailable guest form with Home recovery");
}

function main() {
  checkChatStartup();
  const accessModeScript = extractScript("data-room-access-mode");
  const behaviorScript = extractScript("function announceHomeChrome()");
  const env = createEnvironment();

  vm.runInContext(accessModeScript, env.context);
  vm.runInContext(behaviorScript, env.context);

  assert.notEqual(env.topFrame, env.parentFrame, "top and parent must be distinct destinations");
  assert.equal(env.body.dataset.roomAccessMode, "shell", "Chat must mark shell access mode from the Home launch token");
  assert.equal(env.topMessages.length, 2, "Chat must announce ready and its menu exactly once at startup");
  assert.equal(env.parentMessages.length, 0, "Chat must not send app-ready or menus to the parent rail frame");
  assert.deepEqual(plainJson(env.topMessages[0]), {
    message: { type: "home:app-ready", homeToken: "chat-test-token" },
    origin: "http://home.example",
  });
  assert.equal(env.topMessages[1].message.type, "home:menu-manifest");
  assert.deepEqual(
    plainJson(manifestCommands(env.topMessages[1].message)),
    ["__close-window", "view-people"],
    "Access Settings must stay out of the menu while the current control is unavailable",
  );

  env.roomAccessToggle.hidden = false;
  env.notify(env.roomAccessToggle);
  assert.equal(env.topMessages.length, 3, "Access Settings availability must refresh the Home menu");
  assert.deepEqual(
    plainJson(manifestCommands(env.topMessages[2].message)),
    ["__close-window", "view-people", "view-access-settings"],
    "Access Settings must appear only when the current control becomes available",
  );

  const onMessage = env.windowListeners.get("message");
  assert(onMessage, "Chat must listen for Home menu commands");
  onMessage({
    origin: "https://wrong.example",
    source: env.parentFrame,
    data: { type: "elastos:menu-command", cmd: "view-people" },
  });
  onMessage({
    origin: "null",
    source: env.topFrame,
    data: { type: "elastos:menu-command", cmd: "view-people" },
  });
  assert.equal(env.participantToggle.clickCount, 0, "Chat must reject menu commands from the wrong source or origin");

  onMessage({
    origin: "null",
    source: env.parentFrame,
    data: { type: "elastos:menu-command", cmd: "view-people" },
  });
  onMessage({
    origin: "null",
    source: env.parentFrame,
    data: { type: "elastos:menu-command", cmd: "view-access-settings" },
  });
  assert.equal(env.participantToggle.clickCount, 1, "People must route through the existing toggle button");
  assert.equal(env.roomAccessToggle.clickCount, 1, "Access Settings must route through the existing toggle button");

  env.roomAccessToggle.disabled = true;
  env.notify(env.roomAccessToggle);
  assert.equal(env.topMessages.length, 4, "Disabling Access Settings must refresh the Home menu");
  assert.deepEqual(
    plainJson(manifestCommands(env.topMessages[3].message)),
    ["__close-window", "view-people"],
    "Disabled Access Settings must disappear from the Home menu",
  );
  onMessage({
    origin: "null",
    source: env.parentFrame,
    data: { type: "elastos:menu-command", cmd: "view-access-settings" },
  });
  assert.equal(env.roomAccessToggle.clickCount, 1, "Disabled Access Settings must not create a second action path");

  assert.equal(env.body.dataset.roomCompactRail, "visible", "Two conversations must keep the compact rail available");
  env.conversationSelector.children = [{ dataset: { conversationChoice: "shared" } }];
  env.notify(env.conversationSelector);
  assert.equal(env.body.dataset.roomCompactRail, "hidden", "One conversation may hide the compact rail when no required join control is visible");
  env.conversationJoinSection.hidden = false;
  env.notify(env.conversationJoinSection);
  assert.equal(env.body.dataset.roomCompactRail, "visible", "Visible join controls must keep the compact rail available");

  env.messageInput.value = "line one\nline two\nline three";
  env.messageInput.dispatchEvent({ type: "input" });
  assert.equal(env.messageInput.style.height, "88px", "Chat must expand the current textarea for multiline input");
  assert.equal(env.composerField.dataset.multiline, "true", "Chat must expose multiline presentation state from the current textarea");
  env.messageInput.value = "line one";
  env.messageInput.dispatchEvent({ type: "input" });
  assert.equal(env.messageInput.style.height, "34px", "Chat must shrink the current textarea after multiline input clears");
  assert.equal(env.composerField.dataset.multiline, "false", "Chat must clear multiline presentation state after input shrinks");

  const accessSectionIndex = source.indexOf('id="room-access-section"');
  const inviteIndex = source.indexOf('id="conversation-invite-create"');
  const accessSectionClose = source.indexOf("</section>", accessSectionIndex);
  assert(accessSectionIndex >= 0 && inviteIndex > accessSectionIndex && inviteIndex < accessSectionClose, "Invite creation must stay inside the current access section");

  console.log("PASS Chat Home recovery, signed startup, Home menu, compact rail, and multiline composer behavior");
}

main();
