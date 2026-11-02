import assert from "node:assert/strict";
import { test } from "node:test";

// Drives home-cli.js with the host intent Home emits for People "Message" and
// checks that the Desktop is asked to open Chat at the direct conversation.

const parentMessages = [];
const eventSources = [];
const intentRequests = [];

class FakeElement {
  constructor() {
    this.children = [];
    this.dataset = {};
    this.hidden = false;
    this.textContent = "";
    this.clientWidth = 1000;
    this.clientHeight = 520;
    this.parentElement = null;
  }
  addEventListener() {}
  focus() {}
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
    this.listeners = new Map();
    eventSources.push(this);
  }
  addEventListener(type, callback) {
    this.listeners.set(type, [...(this.listeners.get(type) || []), callback]);
  }
  close() {}
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
  constructor() {
    this.readyState = FakeWebSocket.OPEN;
  }
  send() {}
  close() {}
}

class FakeXtermTerminal {
  open() {}
  resize() {}
  write() {}
  onData() {
    return { dispose() {} };
  }
  focus() {}
  dispose() {}
}

const elements = new Map();
const element = (selector) => {
  if (!elements.has(selector)) elements.set(selector, new FakeElement());
  return elements.get(selector);
};

const json = (value) => ({ ok: true, status: 200, json: async () => value, text: async () => JSON.stringify(value) });
const SESSION = "/api/apps/home-cli/terminal/sessions/term-people";

globalThis.document = {
  activeElement: null,
  body: element("body"),
  createElement: () => new FakeElement(),
  querySelector: element,
  querySelectorAll: () => [],
  referrer: "http://localhost:61180/apps/home/",
};
globalThis.window = {
  location: {
    href: "http://localhost:61180/apps/home-cli/?shell_mode=root&home_origin=http%3A%2F%2Flocalhost%3A61180#home_token=cli-token",
    hash: "#home_token=cli-token",
    origin: "null",
  },
  parent: { postMessage: (message, origin) => parentMessages.push({ message, origin }) },
};
globalThis.EventSource = FakeEventSource;
globalThis.WebSocket = FakeWebSocket;
globalThis.ResizeObserver = class {
  observe() {}
  disconnect() {}
};
globalThis.__ELASTOS_TEST_XTERM__ = FakeXtermTerminal;
globalThis.fetch = async (url, init = {}) => {
  const body = init.body ? JSON.parse(init.body) : null;
  if (url === "/api/apps/home-cli/terminal/sessions") {
    return json({
      schema: "elastos.home-cli.terminal-session/v1",
      session_id: "term-people",
      stream: {
        events_url: `${SESSION}/events?ticket=t`,
        input_socket_url: `${SESSION}/input?ticket=t`,
        resize_url: `${SESSION}/resize`,
        intent_url: `${SESSION}/intent`,
        close_url: `${SESSION}/close`,
      },
    });
  }
  if (url === `${SESSION}/intent`) {
    intentRequests.push(body);
    // The gateway returns the authorized intent it accepted.
    return json({ schema: "elastos.home-cli.terminal-intent/v1", session_id: "term-people", intent: body });
  }
  return json({});
};

const settle = async () => {
  for (let i = 0; i < 20; i += 1) await new Promise((resolve) => setTimeout(resolve, 0));
};

test("terminal People Message opens Chat on the Desktop at the direct conversation", async () => {
  await import("./home-cli.js?people-message-test");
  await settle();
  assert.equal(eventSources.length, 1, "home-cli did not attach the terminal");

  const intent = {
    schema: "elastos.home.terminal-host-intent/v1",
    action: "switch-shell-open-target",
    action_id: "people-message:contact-alice",
    target: "chat-room",
    source: "people-contact",
    contact_id: "contact-alice",
    query: { conversation_id: "direct:sha256:abc" },
  };
  eventSources[0].emit("terminal", {
    schema: "elastos.home-cli.terminal-event/v1",
    session_id: "term-people",
    stream: "stdout",
    data: `\u001b]777;elastos-home-intent=${JSON.stringify(intent)}\u0007`,
  });
  await settle();

  assert.deepEqual(intentRequests, [intent]);
  const launches = parentMessages
    .map(({ message }) => message)
    .filter((message) => message.type === "home:switch-shell-and-open-target");
  assert.equal(launches.length, 1, JSON.stringify(parentMessages));
  assert.equal(launches[0].target, "chat-room");
  assert.deepEqual(launches[0].query, { conversation_id: "direct:sha256:abc" });
  assert.equal(launches[0].homeToken, "cli-token");
});
