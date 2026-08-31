#!/usr/bin/env node

import { readFileSync } from "node:fs";

const requests = [];
const parentMessages = [];
const eventSources = [];
const webSockets = [];
const timers = [];
let terminalStartCount = 0;
let activeShellRequestCount = 0;
let resolveRetriedTerminalStart = null;

class FakeElement {
  constructor(selector = "") {
    this.selector = selector;
    this.children = [];
    this.dataset = {};
    this.hidden = false;
    this.listeners = new Map();
    this.parentElement = null;
    this.textContent = "";
    this.clientWidth = 1000;
    this.clientHeight = 520;
  }

  addEventListener(type, callback) {
    if (!this.listeners.has(type)) {
      this.listeners.set(type, []);
    }
    this.listeners.get(type).push(callback);
  }

  focus() {
    globalThis.document.activeElement = this;
  }

  replaceChildren(...children) {
    this.children = children;
  }

  getBoundingClientRect() {
    return { width: this.clientWidth, height: this.clientHeight };
  }
}

class FakeEventSource {
  constructor(url) {
    this.url = String(url || "");
    this.closed = false;
    this.listeners = new Map();
    eventSources.push(this);
  }

  addEventListener(type, callback) {
    if (!this.listeners.has(type)) {
      this.listeners.set(type, []);
    }
    this.listeners.get(type).push(callback);
  }

  close() {
    this.closed = true;
  }

  emit(type, payload) {
    for (const listener of this.listeners.get(type) || []) {
      listener({ data: JSON.stringify(payload) });
    }
  }
}

class FakeWebSocket {
  static CONNECTING = 0;
  static OPEN = 1;
  static CLOSED = 3;

  constructor(url) {
    this.url = String(url || "");
    this.readyState = FakeWebSocket.CONNECTING;
    webSockets.push(this);
    queueMicrotask(() => {
      if (this.readyState !== FakeWebSocket.CONNECTING) {
        return;
      }
      this.readyState = FakeWebSocket.OPEN;
      this.onopen?.({ type: "open" });
    });
  }

  close() {
    this.readyState = FakeWebSocket.CLOSED;
    this.onclose?.({ type: "close" });
  }
}

class FakeXtermTerminal {
  constructor(options = {}) {
    this.options = options;
  }

  open(node) {
    this.node = node;
  }

  resize() {}

  onData() {
    return { dispose() {} };
  }

  focus() {
    this.focused = true;
  }

  dispose() {}
}

const elements = new Map();
function elementForSelector(selector) {
  if (!elements.has(selector)) {
    elements.set(selector, new FakeElement(selector));
  }
  return elements.get(selector);
}

const body = elementForSelector("body");
const output = elementForSelector("#terminal-output");
const xtermMount = elementForSelector("#xterm-terminal");
const terminalPanel = elementForSelector("#terminal-panel");
const terminalScreen = elementForSelector(".terminal-screen");
const fallbackActions = elementForSelector("#terminal-fallback-actions");
const fallbackRefresh = elementForSelector("#terminal-fallback-refresh");
const fallbackDesktop = elementForSelector("#terminal-fallback-desktop");
output.parentElement = terminalScreen;
xtermMount.parentElement = terminalScreen;
fallbackActions.parentElement = terminalScreen;
fallbackRefresh.parentElement = fallbackActions;
fallbackDesktop.parentElement = fallbackActions;
terminalScreen.parentElement = terminalPanel;

function jsonResponse(value) {
  return {
    ok: true,
    status: 200,
    json: async () => value,
    text: async () => JSON.stringify(value),
  };
}

function failedResponse(status, text) {
  return {
    ok: false,
    status,
    json: async () => ({ error: text }),
    text: async () => text,
  };
}

function assert(condition, message, details = null) {
  if (!condition) {
    const error = new Error(message);
    error.details = details;
    throw error;
  }
}

globalThis.document = {
  activeElement: null,
  body,
  createElement: (tag) => new FakeElement(tag),
  querySelector: elementForSelector,
  querySelectorAll: () => [],
  referrer: "http://localhost:61180/apps/home/",
};
globalThis.window = {
  location: {
    href: "http://localhost:61180/apps/home-cli/?shell_mode=root&home_origin=http%3A%2F%2Flocalhost%3A61180#home_token=cli-token",
    hash: "#home_token=cli-token",
    origin: "null",
  },
  parent: {
    postMessage(message, origin) {
      parentMessages.push({ message, origin });
    },
  },
};
globalThis.EventSource = FakeEventSource;
globalThis.WebSocket = FakeWebSocket;
globalThis.ResizeObserver = class {
  constructor() {}
  observe() {}
  disconnect() {}
};
globalThis.__ELASTOS_TEST_XTERM__ = FakeXtermTerminal;
globalThis.setTimeout = (callback) => {
  timers.push(callback);
  return timers.length;
};

globalThis.fetch = async (url, init = {}) => {
  const body = init.body ? JSON.parse(init.body) : null;
  requests.push({ url: String(url), method: init.method || "GET", headers: init.headers || {}, body });
  if (url === "/api/apps/home-cli/terminal/sessions") {
    terminalStartCount += 1;
    if (terminalStartCount === 1 || terminalStartCount === 3) {
      return failedResponse(503, "simulated terminal start failure");
    }
    if (terminalStartCount === 2) {
      return new Promise((resolve) => {
        resolveRetriedTerminalStart = () => resolve(jsonResponse({
          schema: "elastos.home-cli.terminal-session/v1",
          session_id: "term-smoke",
          stream: {
            events_url: "/api/apps/home-cli/terminal/sessions/term-smoke/events?ticket=ticket-smoke",
            input_socket_url: "/api/apps/home-cli/terminal/sessions/term-smoke/input?ticket=input-ticket-smoke",
            resize_url: "/api/apps/home-cli/terminal/sessions/term-smoke/resize",
            intent_url: "/api/apps/home-cli/terminal/sessions/term-smoke/intent",
            close_url: "/api/apps/home-cli/terminal/sessions/term-smoke/close",
          },
        }));
      });
    }
    return jsonResponse({
      schema: "elastos.home-cli.terminal-session/v1",
      session_id: "term-smoke",
      stream: {
        events_url: "/api/apps/home-cli/terminal/sessions/term-smoke/events?ticket=ticket-smoke",
        input_socket_url: "/api/apps/home-cli/terminal/sessions/term-smoke/input?ticket=input-ticket-smoke",
        resize_url: "/api/apps/home-cli/terminal/sessions/term-smoke/resize",
        intent_url: "/api/apps/home-cli/terminal/sessions/term-smoke/intent",
        close_url: "/api/apps/home-cli/terminal/sessions/term-smoke/close",
      },
    });
  }
  if (url === "/api/apps/home/active-shell") {
    activeShellRequestCount += 1;
    assert(init.method === "POST", "desktop recovery did not POST the active-shell update", init);
    assert(init.headers?.["x-elastos-home-token"] === "cli-token", "desktop recovery lost the shell launch token", init.headers);
    assert(body?.active === "home-gui", "desktop recovery selected the wrong shell", body);
    if (activeShellRequestCount === 1) {
      return failedResponse(500, "simulated shell switch failure");
    }
    return jsonResponse({
      schema: "elastos.home.active-shell/v1",
      active: "home-gui",
      candidates: [],
    });
  }
  if (url === "/api/apps/home-cli/terminal/sessions/term-smoke/close") {
    return jsonResponse({ schema: "elastos.home-cli.terminal-close/v1", status: "closed" });
  }
  if (url === "/api/apps/home-cli/terminal/sessions/term-smoke/resize") {
    return jsonResponse({ schema: "elastos.home-cli.terminal-resize/v1" });
  }
  if (url === "/api/apps/home-cli/terminal/sessions/term-smoke/intent") {
    return jsonResponse({ schema: "elastos.home-cli.terminal-intent/v1", session_id: "term-smoke", intent: body });
  }
  return jsonResponse({});
};

await import("../capsules/home-cli/browser/home-cli.js?home-cli-fallback-recovery");

async function waitFor(predicate, message, details = () => null) {
  for (let attempt = 0; attempt < 40; attempt += 1) {
    const value = predicate();
    if (value) {
      return value;
    }
    while (timers.length) {
      const timer = timers.shift();
      timer();
    }
    await new Promise((resolve) => queueMicrotask(resolve));
  }
  assert(false, message, details());
}

const productIndex = readFileSync("capsules/home-cli/browser/index.html", "utf8");
const productStyle = readFileSync("capsules/home-cli/browser/style.css", "utf8");
assert(productIndex.includes('id="terminal-fallback-desktop"'), "home-cli recovery action is missing from the document", productIndex);
assert(productIndex.includes('id="terminal-fallback-refresh"'), "home-cli refresh action is missing from the document", productIndex);
assert(productStyle.includes(".terminal-fallback-actions[hidden]"), "home-cli fallback actions can override native hidden handling", productStyle);
assert(productStyle.includes(".terminal-fallback-button[hidden]"), "home-cli fallback buttons can override native hidden handling", productStyle);
assert(productStyle.includes(".terminal-screen {\n  display: flex;\n  flex-direction: column;"), "home-cli fallback layout does not keep actions in the first viewport", productStyle);
assert(productStyle.includes('body[data-runtime-terminal="idle"] #terminal-output {\n  min-height: 0;'), "home-cli fallback layout still lets terminal output push recovery actions below the viewport", productStyle);

await waitFor(
  () => output.textContent.includes("Home CLI could not start."),
  "home-cli did not show the initial start failure fallback",
  () => ({ output: output.textContent, requests }),
);

assert(fallbackActions.hidden === false, "home-cli hid recovery actions after initial start failure");
assert(fallbackRefresh.hidden === false, "home-cli hid refresh after initial start failure");
assert(fallbackDesktop.hidden === false, "home-cli hid Desktop recovery after initial start failure");
assert(document.activeElement === fallbackDesktop, "home-cli did not focus Desktop recovery after initial start failure");

for (const listener of fallbackRefresh.listeners.get("click") || []) {
  listener();
}
for (const listener of fallbackRefresh.listeners.get("click") || []) {
  listener();
}

assert(fallbackActions.hidden === true, "home-cli left recovery actions visible while an explicit refresh was pending");
assert(
  requests.filter((request) => request.url === "/api/apps/home-cli/terminal/sessions").length === 2,
  "home-cli started more than one terminal session for a double refresh click",
  requests,
);
assert(resolveRetriedTerminalStart, "home-cli did not keep the retried terminal start pending for the double-click guard test");
resolveRetriedTerminalStart();

await waitFor(
  () => eventSources.length === 1 && webSockets.length === 1,
  "home-cli did not start after explicit refresh",
  () => ({ eventSources, webSockets, requests, output: output.textContent }),
);

assert(fallbackActions.hidden === true, "home-cli kept fallback actions visible during a healthy retried session");

eventSources[0].emit("terminal", {
  schema: "elastos.home-cli.terminal-event/v1",
  session_id: "term-smoke",
  stream: "stdout",
  data: "\u001b[31mfatal configuration mismatch\u001b[0m\n",
});
eventSources[0].emit("terminal", {
  schema: "elastos.home-cli.terminal-event/v1",
  session_id: "term-smoke",
  stream: "error",
  message: "private transport detail",
});

eventSources[0].emit("terminal", {
  schema: "elastos.home-cli.terminal-event/v1",
  session_id: "term-smoke",
  stream: "lifecycle",
  message: "exited",
  exit_code: 0,
});

await waitFor(
  () => output.textContent.includes("Home CLI could not reconnect."),
  "home-cli did not show its bounded fallback after a failed reconnect",
  () => ({ output: output.textContent, requests }),
);

assert(fallbackActions.hidden === false, "home-cli hid the fallback action row after reconnect failure");
assert(fallbackRefresh.hidden === false, "home-cli hid the refresh action after reconnect failure");
assert(fallbackDesktop.hidden === false, "home-cli hid the Desktop recovery action after reconnect failure");
assert(document.activeElement === fallbackDesktop, "home-cli did not focus Desktop recovery after reconnect failure");
assert(
  output.textContent.includes("Return to the Desktop"),
  "home-cli fallback copy did not expose Desktop recovery",
  output.textContent,
);
assert(
  output.textContent.includes("fatal configuration mismatch"),
  "home-cli fallback did not retain the last visible terminal diagnostic",
  output.textContent,
);
assert(
  output.textContent.match(/fatal configuration mismatch/g)?.length === 1,
  "home-cli fallback duplicated the retained terminal diagnostic",
  output.textContent,
);
assert(
  !output.textContent.includes("private transport detail"),
  "home-cli surfaced a private transport-side error instead of the terminal diagnostic",
  output.textContent,
);

for (const listener of fallbackDesktop.listeners.get("click") || []) {
  await listener();
}

await waitFor(
  () => output.textContent.includes("Desktop did not open."),
  "home-cli did not preserve visible recovery after a denied Desktop switch",
  () => ({ output: output.textContent, requests, parentMessages }),
);

assert(fallbackActions.hidden === false, "home-cli hid recovery actions after a denied Desktop switch");
assert(fallbackRefresh.hidden === false, "home-cli hid refresh after a denied Desktop switch");
assert(fallbackDesktop.hidden === false, "home-cli hid Desktop recovery after a denied Desktop switch");
assert(document.activeElement === fallbackDesktop, "home-cli lost Desktop recovery focus after a denied switch");
assert(
  !parentMessages.some(({ message }) => (
    message?.type === "home:active-shell-applied" ||
      message?.type === "home:close-self"
  )),
  "home-cli reported a shell change after a denied Desktop switch",
  parentMessages,
);

for (const listener of fallbackDesktop.listeners.get("click") || []) {
  await listener();
}

await waitFor(
  () => parentMessages.some(({ message }) => (
    message?.type === "home:active-shell-applied" &&
      message?.activeShell === "home-gui" &&
      message?.homeToken === "cli-token"
  )),
  "home-cli fallback Desktop recovery did not report the Runtime-owned shell change",
  () => parentMessages,
);

assert(
  requests.some((request) => (
    request.url === "/api/apps/home/active-shell" &&
      request.method === "POST" &&
      request.body?.active === "home-gui"
  )),
  "home-cli fallback Desktop recovery did not use the Runtime-owned active-shell route",
  requests,
);
assert(
  requests.filter((request) => request.url === "/api/apps/home/active-shell").length === 2,
  "home-cli fallback Desktop recovery did not preserve retry after a denied switch",
  requests,
);
assert(
  parentMessages.every(({ origin }) => origin === "http://localhost:61180"),
  "home-cli fallback sent recovery to the wrong parent origin",
  parentMessages,
);

console.log("[home-cli-fallback-recovery] PASS");
