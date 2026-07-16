#!/usr/bin/env node

import { readFileSync } from "node:fs";

const requests = [];
const parentMessages = [];
const eventSources = [];
const resizeObservers = [];
let terminalStartCount = 0;

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
    this.focused = true;
  }

  replaceChildren(...children) {
    this.children = children;
    this.textContent = children.map((child) => child.textContent || "").join("");
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

  emitError() {
    this.onerror?.({ type: "error" });
  }
}

const xtermInstances = [];
class FakeXtermTerminal {
  constructor(options = {}) {
    this.options = options;
    this.cols = options.cols;
    this.rows = options.rows;
    this.writes = [];
    this.text = "";
    this.dataListener = null;
    this.disposed = false;
    xtermInstances.push(this);
  }

  open(node) {
    this.node = node;
    node.dataset.xtermOpen = "true";
  }

  write(data) {
    const text = String(data || "");
    this.writes.push(text);
    this.text += text;
    if (this.node) {
      this.node.textContent += stripAnsi(text);
    }
  }

  writeln(data) {
    this.write(`${String(data || "")}\n`);
  }

  onData(callback) {
    this.dataListener = callback;
    return {
      dispose: () => {
        if (this.dataListener === callback) {
          this.dataListener = null;
        }
      },
    };
  }

  emitData(data) {
    this.dataListener?.(data);
  }

  resize(cols, rows) {
    this.cols = cols;
    this.rows = rows;
  }

  focus() {
    this.focused = true;
  }

  dispose() {
    this.disposed = true;
  }
}

class FakeResizeObserver {
  constructor(callback) {
    this.callback = callback;
    this.node = null;
    this.disconnected = false;
    resizeObservers.push(this);
  }

  observe(node) {
    this.node = node;
  }

  disconnect() {
    this.disconnected = true;
  }

  trigger() {
    this.callback([{ target: this.node }]);
  }
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
output.parentElement = terminalScreen;
xtermMount.parentElement = terminalScreen;
terminalScreen.parentElement = terminalPanel;

function stripAnsi(text) {
  return String(text || "").replace(/\x1b\[[0-9;?]*[ -/]*[@-~]/g, "");
}

function jsonResponse(value) {
  return {
    ok: true,
    status: 200,
    json: async () => value,
    text: async () => JSON.stringify(value),
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
  body,
  createElement: (tag) => new FakeElement(tag),
  querySelector: elementForSelector,
  querySelectorAll: () => [],
  referrer: "http://localhost:61180/apps/home/",
};
globalThis.window = {
  location: {
    href: "http://home-cli.localhost:61180/apps/home-cli/?shell_mode=root&home_token=cli-token",
    origin: "http://home-cli.localhost:61180",
  },
  parent: {
    postMessage(message, origin) {
      parentMessages.push({ message, origin });
    },
  },
};
globalThis.EventSource = FakeEventSource;
globalThis.ResizeObserver = FakeResizeObserver;
globalThis.__ELASTOS_TEST_XTERM__ = FakeXtermTerminal;

globalThis.fetch = async (url, init = {}) => {
  const body = init.body ? JSON.parse(init.body) : null;
  requests.push({ url: String(url), method: init.method || "GET", headers: init.headers || {}, body });
  if (url === "/api/apps/home-cli/terminal/sessions") {
    terminalStartCount += 1;
    assert(init.method === "POST", "home-cli terminal session was not started with POST", init);
    assert(init.headers?.["x-elastos-home-token"] === "cli-token", "home-cli terminal start missed launch token", init);
    assert(body?.schema === "elastos.home-cli.terminal-start/v1", "home-cli terminal start used wrong schema", body);
    assert(body.cols >= 40 && body.rows >= 12, "home-cli terminal start missed viewport dimensions", body);
    assert(
      body.rows === (terminalStartCount === 1 ? 25 : 20),
      "home-cli terminal start must reserve a visible row for the TUI header",
      { body, terminalStartCount },
    );
    return jsonResponse({
      schema: "elastos.home-cli.terminal-session/v1",
      session_id: "term-smoke",
      transport: "runtime_pty_stream",
      pty: true,
      renderer_contract: "capsule-local xterm.js terminal over a Runtime-owned byte-stream contract",
      dimensions: { cols: body.cols, rows: body.rows },
      process: { label: "elastos home", argv: ["elastos", "home"], mode: "tui" },
      stream: {
        schema: "elastos.runtime.stream/v1",
        events_url: "/api/apps/home-cli/terminal/sessions/term-smoke/events?ticket=ticket-smoke",
        input_url: "/api/apps/home-cli/terminal/sessions/term-smoke/input",
        resize_url: "/api/apps/home-cli/terminal/sessions/term-smoke/resize",
        intent_url: "/api/apps/home-cli/terminal/sessions/term-smoke/intent",
        close_url: "/api/apps/home-cli/terminal/sessions/term-smoke/close",
        input_schema: "elastos.home-cli.terminal-input/v1",
        resize_schema: "elastos.home-cli.terminal-resize/v1",
        event_schema: "elastos.home-cli.terminal-event/v1",
        intent_schema: "elastos.home-cli.terminal-intent/v1",
      },
    });
  }
  if (url === "/api/apps/home-cli/terminal/sessions/term-smoke/intent") {
    assert(init.method === "POST", "home-cli terminal host intent was not POSTed", init);
    assert(init.headers?.["x-elastos-home-token"] === "cli-token", "home-cli terminal host intent missed launch token", init);
    assert(body?.schema === "elastos.home.terminal-host-intent/v1", "home-cli terminal host intent used wrong schema", body);
    const isBrowserOpen = body?.action_id === "open-gui:browser" &&
      body?.action === "open-target" &&
      body?.target === "browser";
    const isHomeGuiSwitch = body?.action_id === "shell-switch:home-gui" &&
      body?.action === "active-shell" &&
      body?.target === "home-gui";
    const isSignOut = body?.action_id === "auth-sign-out" &&
      body?.action === "sign-out" &&
      body?.target === "home";
    assert(
      isBrowserOpen || isHomeGuiSwitch || isSignOut,
      "home-cli terminal host intent was not an explicit signed Home action",
      body,
    );
    return jsonResponse({
      schema: "elastos.home-cli.terminal-intent/v1",
      session_id: "term-smoke",
      intent: body,
    });
  }
  if (url === "/api/apps/home-cli/terminal/sessions/term-smoke/input") {
    assert(init.method === "POST", "home-cli terminal input was not POSTed", init);
    assert(init.headers?.["x-elastos-home-token"] === "cli-token", "home-cli terminal input missed launch token", init);
    assert(body?.schema === "elastos.home-cli.terminal-input/v1", "home-cli terminal input used wrong schema", body);
    return jsonResponse({
      schema: "elastos.home-cli.terminal-input/v1",
      session_id: "term-smoke",
      written_bytes: String(body?.data || "").length,
    });
  }
  if (url === "/api/apps/home-cli/terminal/sessions/term-smoke/resize") {
    assert(init.method === "POST", "home-cli terminal resize was not POSTed", init);
    assert(init.headers?.["x-elastos-home-token"] === "cli-token", "home-cli terminal resize missed launch token", init);
    assert(body?.schema === "elastos.home-cli.terminal-resize/v1", "home-cli terminal resize used wrong schema", body);
    assert(body.cols >= 40 && body.rows >= 12, "home-cli terminal resize missed viewport dimensions", body);
    assert(body.rows === 20, "home-cli terminal resize must reserve a visible row for the TUI header", body);
    return jsonResponse({
      schema: "elastos.home-cli.terminal-resize/v1",
      session_id: "term-smoke",
      dimensions: { cols: body.cols, rows: body.rows },
    });
  }
  if (url === "/api/apps/home-cli/terminal/sessions/term-smoke/close") {
    assert(init.method === "POST", "home-cli terminal close was not POSTed", init);
    assert(init.headers?.["x-elastos-home-token"] === "cli-token", "home-cli terminal close missed launch token", init);
    return jsonResponse({
      schema: "elastos.home-cli.terminal-close/v1",
      session_id: "term-smoke",
      status: "closed",
    });
  }
  return jsonResponse({});
};

await import("../capsules/home-cli/browser/home-cli.js?home-cli-smoke");

async function waitFor(predicate, message, details = () => null) {
  for (let attempt = 0; attempt < 40; attempt += 1) {
    const value = predicate();
    if (value) {
      return value;
    }
    await new Promise((resolve) => setTimeout(resolve, 0));
  }
  assert(false, message, details());
}

const productIndex = readFileSync("capsules/home-cli/browser/index.html", "utf8");
assert(productIndex.includes('id="xterm-terminal"'), "home-cli product document does not mount xterm", productIndex);
assert(!productIndex.includes('id="command-input"'), "home-cli product document still renders the old command input", productIndex);
assert(!productIndex.includes('id="terminal-toggle"'), "home-cli product document still renders the old terminal toggle", productIndex);
assert(!productIndex.includes("data-command="), "home-cli product document still renders quick command buttons", productIndex);

await waitFor(
  () => requests.find((request) => request.url === "/api/apps/home-cli/terminal/sessions"),
  "home-cli did not request a Runtime terminal session on boot",
  () => requests,
);
assert(eventSources.length === 1, "home-cli did not autostart the Runtime terminal event stream", eventSources);
assert(xtermInstances.length === 1, "home-cli did not autostart an xterm terminal", xtermInstances);
assert(xtermInstances[0].focused === true, "home-cli did not focus the attached xterm terminal", xtermInstances[0]);
assert(xtermInstances[0].options.convertEol === false, "home-cli must render raw PTY bytes without xterm EOL rewriting", xtermInstances[0].options);
assert(xtermMount.dataset.xtermOpen === "true", "home-cli did not open xterm on the Runtime terminal mount", xtermMount.dataset);
assert(
  body.dataset.runtimeTerminal === "attached" &&
    terminalPanel.dataset.runtimeTerminal === "attached",
  "home-cli did not enter attached Runtime terminal mode",
  { body: body.dataset, panel: terminalPanel.dataset },
);

const activeEventSource = eventSources[0];
const activeXterm = xtermInstances[0];
assert(
  activeEventSource.url === "/api/apps/home-cli/terminal/sessions/term-smoke/events?ticket=ticket-smoke" &&
    !activeEventSource.url.includes("home_token="),
  "home-cli terminal event stream did not use a scoped stream ticket",
  activeEventSource,
);

activeEventSource.emitError();
await new Promise((resolve) => setTimeout(resolve, 0));
assert(eventSources.length === 1, "home-cli replaced its PTY after a transient event-stream error", eventSources);
assert(activeEventSource.closed === false, "home-cli disabled native EventSource reconnection", activeEventSource);
assert(activeXterm.disposed === false, "home-cli disposed xterm after a transient event-stream error", activeXterm);
assert(
  body.dataset.runtimeTerminal === "attached" && terminalPanel.dataset.runtimeTerminal === "attached",
  "home-cli detached its Runtime terminal after a transient event-stream error",
  { body: body.dataset, panel: terminalPanel.dataset },
);

xtermMount.clientWidth = 920;
xtermMount.clientHeight = 420;
resizeObservers[0]?.trigger();
const terminalResizeRequest = await waitFor(
  () => requests.find((request) => (
    request.url === "/api/apps/home-cli/terminal/sessions/term-smoke/resize" &&
      request.body?.schema === "elastos.home-cli.terminal-resize/v1"
  )),
  "home-cli did not send terminal resize to Runtime",
  () => requests,
);
assert(
  terminalResizeRequest.headers["x-elastos-home-token"] === "cli-token",
  "home-cli terminal resize did not carry its launch token",
  terminalResizeRequest,
);

activeEventSource.emit("terminal", {
  schema: "elastos.home-cli.terminal-event/v1",
  session_id: "term-smoke",
  stream: "stdout",
  data: "\u001b[32mstream ready\u001b[0m\n",
});
await waitFor(
  () => xtermMount.textContent.includes("stream ready"),
  "expected xterm output to include stream ready",
  () => xtermMount.textContent,
);
assert(!output.textContent.includes("stream ready"), "home-cli rendered Runtime stream into the fallback log", output.textContent);
assert(
  activeXterm.writes.some((write) => write.includes("\u001b[32mstream ready\u001b[0m")),
  "home-cli did not pass raw terminal bytes to xterm",
  activeXterm.writes,
);

activeEventSource.emit("terminal", {
  schema: "elastos.home-cli.terminal-event/v1",
  session_id: "term-smoke",
  stream: "stdout",
  data: `\u001b]777;elastos-home-intent=${JSON.stringify({
    schema: "elastos.home.terminal-host-intent/v1",
    action: "open-target",
    action_id: "open-gui:browser",
    target: "browser",
  })}\u0007`,
});
await waitFor(
  () => parentMessages.some(({ message }) => (
    message?.type === "home:open-target" &&
      message?.target === "browser" &&
      message?.homeToken === "cli-token"
  )),
  "home-cli terminal host intent did not request a Home-owned app open",
  () => parentMessages,
);
assert(
  requests.some((request) => (
    request.url === "/api/apps/home-cli/terminal/sessions/term-smoke/intent" &&
      request.body?.schema === "elastos.home.terminal-host-intent/v1" &&
      request.body?.action_id === "open-gui:browser"
  )),
  "home-cli did not ask Runtime to authorize the terminal host intent",
  requests,
);
assert(
  !activeXterm.writes.some((write) => write.includes("elastos.home.terminal-host-intent")),
  "home-cli leaked the private host intent control sequence into xterm output",
  activeXterm.writes,
);

activeEventSource.emit("terminal", {
  schema: "elastos.home-cli.terminal-event/v1",
  session_id: "term-smoke",
  stream: "lifecycle",
  message: "exited",
  exit_code: 0,
});
await waitFor(
  () => eventSources.length === 2 && xtermInstances.length === 2,
  "home-cli terminal host intent exit did not reattach the root shell terminal",
  () => ({ eventSources, xtermInstances, output: output.textContent, parentMessages }),
);
assert(
  !parentMessages.some(({ message }) => message?.type === "home:close-self"),
  "home-cli host intent exit switched back to Home GUI",
  parentMessages,
);
assert(activeEventSource.closed === true, "home-cli did not close the terminal event stream after host-intent exit", activeEventSource);
assert(activeXterm.disposed === true, "home-cli did not dispose xterm after host-intent terminal exit", activeXterm);
assert(
  body.dataset.runtimeTerminal === "attached" &&
    terminalPanel.dataset.runtimeTerminal === "attached",
  "home-cli did not re-enter attached Runtime terminal mode after host-intent exit",
  { body: body.dataset, panel: terminalPanel.dataset },
);

const restartedEventSource = eventSources[1];
const restartedXterm = xtermInstances[1];
restartedXterm.emitData("q");
const terminalInputRequest = await waitFor(
  () => requests.find((request) => (
    request.url === "/api/apps/home-cli/terminal/sessions/term-smoke/input" &&
      request.body?.data === "q"
  )),
  "home-cli did not send xterm input to the Runtime terminal",
  () => requests,
);
assert(
  terminalInputRequest.headers["x-elastos-home-token"] === "cli-token",
  "home-cli terminal input did not carry its launch token",
  terminalInputRequest,
);

restartedEventSource.emit("terminal", {
  schema: "elastos.home-cli.terminal-event/v1",
  session_id: "term-smoke",
  stream: "lifecycle",
  message: "exited",
  exit_code: 0,
});
await waitFor(
  () => eventSources.length === 3 && xtermInstances.length === 3,
  "home-cli terminal exit did not reattach Home CLI",
  () => ({ eventSources, xtermInstances, output: output.textContent, parentMessages }),
);
assert(restartedEventSource.closed === true, "home-cli did not close the restarted terminal event stream after lifecycle exit", restartedEventSource);
assert(restartedXterm.disposed === true, "home-cli did not dispose restarted xterm after terminal lifecycle exit", restartedXterm);
assert(
  body.dataset.runtimeTerminal === "attached" &&
    terminalPanel.dataset.runtimeTerminal === "attached",
  "home-cli did not stay attached after terminal lifecycle reattach",
  { body: body.dataset, panel: terminalPanel.dataset },
);
assert(
  !parentMessages.some(({ message }) => message?.type === "home:close-self"),
  "home-cli terminal lifecycle exit switched back to Home GUI",
  parentMessages,
);

const finalEventSource = eventSources[2];
finalEventSource.emit("terminal", {
  schema: "elastos.home-cli.terminal-event/v1",
  session_id: "term-smoke",
  stream: "stdout",
  data: `\u001b]777;elastos-home-intent=${JSON.stringify({
    schema: "elastos.home.terminal-host-intent/v1",
    action: "active-shell",
    action_id: "shell-switch:home-gui",
    target: "home-gui",
  })}\u0007`,
});
await waitFor(
  () => parentMessages.some(({ message }) => (
    message?.type === "home:close-self" &&
      message?.activeShell === "home-gui" &&
      message?.homeToken === "cli-token"
  )),
  "home-cli terminal shell-switch host intent did not request a Home-owned shell switch",
  () => parentMessages,
);
assert(
  requests.some((request) => (
    request.url === "/api/apps/home-cli/terminal/sessions/term-smoke/intent" &&
      request.body?.schema === "elastos.home.terminal-host-intent/v1" &&
      request.body?.action_id === "shell-switch:home-gui"
  )),
  "home-cli did not ask Runtime to authorize the shell-switch host intent",
  requests,
);

finalEventSource.emit("terminal", {
  schema: "elastos.home-cli.terminal-event/v1",
  session_id: "term-smoke",
  stream: "stdout",
  data: `\u001b]777;elastos-home-intent=${JSON.stringify({
    schema: "elastos.home.terminal-host-intent/v1",
    action: "sign-out",
    action_id: "auth-sign-out",
    target: "home",
  })}\u0007`,
});
await waitFor(
  () => parentMessages.some(({ message }) => (
    message?.type === "home:sign-out" && message?.homeToken === "cli-token"
  )),
  "home-cli terminal sign-out intent did not request Home session revocation",
  () => parentMessages,
);
assert(
  requests.some((request) => (
    request.url === "/api/apps/home-cli/terminal/sessions/term-smoke/intent" &&
      request.body?.action === "sign-out" &&
      request.body?.action_id === "auth-sign-out" &&
      request.body?.target === "home"
  )),
  "home-cli did not ask Runtime to authorize the sign-out intent",
  requests,
);
assert(
  requests.some((request) => request.url === "/api/apps/home-cli/terminal/sessions/term-smoke/close"),
  "home-cli did not close its Runtime terminal before signing out",
  requests,
);
assert(
  parentMessages
    .filter(({ message }) => ["home:open-target", "home:close-self", "home:sign-out"].includes(message?.type))
    .every(({ origin }) => origin === "http://localhost:61180"),
  "home-cli sent a host intent to its isolated capsule origin instead of the parent Home origin",
  parentMessages,
);

assert(!requests.some((request) => request.url.startsWith("./commands.json")), "home-cli browser wrapper fetched the old browser command contract", requests);
assert(!requests.some((request) => request.url === "/api/esp/initialize"), "home-cli browser wrapper fetched ESP facts directly", requests);
assert(!requests.some((request) => request.url === "/api/capsules/interfaces"), "home-cli browser wrapper fetched interface facts directly", requests);
assert(!requests.some((request) => request.url === "/api/apps/home/summary"), "home-cli browser wrapper fetched Home summary directly", requests);
assert(!requests.some((request) => request.url.startsWith("/api/provider/")), "home-cli called provider routes directly", requests);
assert(!requests.some((request) => request.url.startsWith("/api/apps/system")), "home-cli called System routes directly", requests);

console.log("[home-cli-browser] PASS");
