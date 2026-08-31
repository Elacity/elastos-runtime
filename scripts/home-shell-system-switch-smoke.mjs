#!/usr/bin/env node

const moduleVersion = "home-20260802a";
const requests = [];
const windowListeners = new Map();
const localStorageValues = new Map();
const windowFrames = [];
const pendingTimers = new Map();
let nextTimerId = 1;
const originalConsoleError = console.error;
console.error = (...args) => {
  if (String(args[0] || "").includes("active shell root launch failed")) {
    return;
  }
  originalConsoleError(...args);
};

function addWindowEventListener(type, callback) {
  if (!windowListeners.has(type)) {
    windowListeners.set(type, []);
  }
  windowListeners.get(type).push(callback);
}

class FakeClassList {
  constructor() {
    this.values = new Set();
  }

  add(...tokens) {
    for (const token of tokens) this.values.add(token);
  }

  remove(...tokens) {
    for (const token of tokens) this.values.delete(token);
  }

  contains(token) {
    return this.values.has(token);
  }

  toggle(token, force) {
    const next = force === undefined ? !this.values.has(token) : Boolean(force);
    if (next) this.values.add(token);
    else this.values.delete(token);
    return next;
  }
}

class FakeStyle {
  constructor() {
    this.values = new Map();
  }

  removeProperty(name) {
    this.values.delete(name);
    delete this[name];
  }

  setProperty(name, value) {
    this.values.set(name, String(value));
    this[name] = String(value);
  }
}

class FakeElement {
  constructor(selector = "", withTemplateContent = true) {
    this.selector = selector;
    this.children = [];
    this.dataset = {};
    this.style = new FakeStyle();
    this.hidden = false;
    this.disabled = false;
    this.inert = false;
    this.removed = false;
    this.textContent = "";
    this.title = "";
    this.value = "";
    this.src = "";
    this.attributes = new Map();
    this.listeners = new Map();
    this.classList = new FakeClassList();
    this.closestNode = null;
    this.content = withTemplateContent
      ? {
          firstElementChild: new FakeElement(`:template-child`, false),
          cloneNode: () => new FakeElement(`:template-fragment`, false),
        }
      : { firstElementChild: null };
  }

  addEventListener(type, callback) {
    if (!this.listeners.has(type)) {
      this.listeners.set(type, []);
    }
    this.listeners.get(type).push(callback);
  }

  appendChild(child) {
    child.parentElement = this;
    this.children.push(child);
    return child;
  }

  insertBefore(child, before) {
    child.parentElement = this;
    const index = this.children.indexOf(before);
    if (index >= 0) this.children.splice(index, 0, child);
    else this.children.push(child);
    return child;
  }

  cloneNode() {
    return new FakeElement(`${this.selector}:clone`);
  }

  closest() {
    return this.closestNode;
  }

  focus() {}

  getAttribute(name) {
    return this.attributes.get(name) || "";
  }

  getBoundingClientRect() {
    return { left: 0, top: 0, width: 1024, height: 768, right: 1024, bottom: 768 };
  }

  querySelector(selector) {
    return new FakeElement(`${this.selector} ${selector}`);
  }

  querySelectorAll() {
    return [];
  }

  remove() {
    this.removed = true;
    if (this.parentElement) {
      this.parentElement.children = this.parentElement.children.filter((child) => child !== this);
      this.parentElement = null;
    }
  }

  removeAttribute(name) {
    this.attributes.delete(name);
    delete this[name];
  }

  replaceChildren(...children) {
    this.children = children;
  }

  setAttribute(name, value) {
    this.attributes.set(name, String(value));
    this[name] = String(value);
  }
}

const elementCache = new Map();
function elementForSelector(selector) {
  if (!elementCache.has(selector)) {
    elementCache.set(selector, new FakeElement(selector));
  }
  return elementCache.get(selector);
}

function jsonResponse(value) {
  return {
    ok: true,
    status: 200,
    statusText: "OK",
    json: async () => value,
    text: async () => JSON.stringify(value),
  };
}

function failedResponse(status, statusText, detail) {
  return {
    ok: false,
    status,
    statusText,
    json: async () => ({ error: detail }),
    text: async () => detail,
  };
}

function assert(condition, message, details = null) {
  if (!condition) {
    const error = new Error(message);
    error.details = details;
    throw error;
  }
}

async function flush(turns = 1) {
  for (let attempt = 0; attempt < turns; attempt += 1) {
    await new Promise((resolve) => setTimeout(resolve, 0));
  }
}

async function waitFor(predicate, message, details) {
  for (let attempt = 0; attempt < 40; attempt += 1) {
    if (predicate()) {
      return;
    }
    await flush();
  }
  assert(false, message, details?.());
}

function dispatchMessage(source, data) {
  for (const listener of windowListeners.get("message") || []) {
    listener({
      origin: "null",
      source,
      data,
    });
  }
}

function click(element) {
  for (const listener of element.listeners.get("click") || []) {
    listener({ preventDefault() {} });
  }
}

class FakeEventSource {
  constructor() {
    this.listeners = new Map();
  }

  addEventListener(type, callback) {
    this.listeners.set(type, callback);
  }

  close() {}
}

const summary = {
  authority: { signed_in: true },
  active_shell: {
    active: "home-gui",
    candidates: [
      { name: "home-gui", title: "Home GUI", role: "shell", launchable: true, route: "/apps/home/" },
      { name: "home-cli", title: "Home CLI", role: "shell", launchable: true, route: "/apps/home-cli/" },
    ],
  },
  app: { id: "home", route: "/apps/home/" },
  appearance: {},
  browser_state: {
    principal_id: "principal:home-shell-system-switch",
    layout: { desktop: {}, taskbar: [], desktopHidden: [], desktopIconsVisible: true },
    recent_targets: [],
    session: { windows: [] },
  },
  desktop_objects: { objects: [] },
  identity: {},
  notifications: {},
  people: {},
  runtime: { running: true },
  services: {},
  site: {},
  targets: [
    { target: "browser", title: "Browser", attach_kind: "iframe", role: "app", target_kind: "app" },
    { target: "system", title: "System", attach_kind: "iframe", role: "app", target_kind: "app" },
  ],
};

let deferredRootShellLaunch = null;
let failNextRootShellLaunchTarget = "";
let nextSummaryFailure = null;
const rootShellLaunchCounts = {
  "home-gui": 0,
  "home-cli": 0,
};

globalThis.HTMLElement = FakeElement;
globalThis.document = {
  activeElement: null,
  body: elementForSelector("body"),
  documentElement: elementForSelector("html"),
  addEventListener() {},
  createElement: (tag) => new FakeElement(tag),
  querySelector: elementForSelector,
  querySelectorAll: (selector) => {
    if (selector === ".window[data-target] .window-frame") {
      return windowFrames;
    }
    return [];
  },
};
Object.defineProperty(globalThis, "navigator", {
  configurable: true,
  value: {},
});
globalThis.window = {
  EventSource: FakeEventSource,
  crypto: { randomUUID: () => "home-shell-system-switch-smoke" },
  location: { href: "http://localhost:61180/apps/home/", origin: "http://localhost:61180" },
  localStorage: {
    getItem: (key) => localStorageValues.get(key) || null,
    removeItem: (key) => localStorageValues.delete(key),
    setItem: (key, value) => localStorageValues.set(key, String(value)),
  },
  performance: { now: () => Date.now() },
  innerWidth: 1280,
  addEventListener: addWindowEventListener,
  clearInterval() {},
  clearTimeout(timerId) {
    pendingTimers.delete(Number(timerId));
  },
  setInterval: () => 0,
  setTimeout: (callback, delay = 0) => {
    if (Number(delay) <= 0) {
      Promise.resolve().then(callback);
      return 0;
    }
    const timerId = nextTimerId++;
    pendingTimers.set(timerId, callback);
    return timerId;
  },
};
globalThis.EventSource = FakeEventSource;

elementForSelector("#active-shell-root").hidden = true;
elementForSelector("#active-shell-frame").hidden = true;
elementForSelector("#shell-host-recovery").hidden = true;
elementForSelector("#shell-host-recovery-detail").hidden = true;
elementForSelector("#desktop-context-menu").hidden = true;
elementForSelector("#home-notification-toast").hidden = true;
elementForSelector("#home-unlock").hidden = true;
elementForSelector("#launcher").hidden = true;

globalThis.fetch = async (url, init = {}) => {
  const body = init.body ? JSON.parse(init.body) : null;
  requests.push({
    body,
    headers: init.headers || {},
    method: init.method || "GET",
    url: String(url),
  });
  if (url === "/api/apps/home/summary") {
    if (nextSummaryFailure) {
      const failure = nextSummaryFailure;
      nextSummaryFailure = null;
      return failure;
    }
    return jsonResponse(summary);
  }
  if (url === "/api/auth/sessions/refresh") {
    return jsonResponse({ home_token: "host-token" });
  }
  if (url === "/api/apps/home/active-shell") {
    assert(
      init.headers?.["x-elastos-home-token"] === "host-token",
      "Desktop recovery did not use the trusted Home host token",
      init.headers,
    );
    summary.active_shell.active = body?.active || summary.active_shell.active;
    return jsonResponse({ active: summary.active_shell.active });
  }
  if (url === "/api/apps/home/launch") {
    if (body?.target === "home-gui") {
      assert(body?.query?.shell_mode === "root", "setup did not launch Home GUI in root mode", body);
      rootShellLaunchCounts["home-gui"] += 1;
      if (deferredRootShellLaunch?.target === "home-gui") {
        return new Promise((resolve, reject) => {
          deferredRootShellLaunch.resolve = resolve;
          deferredRootShellLaunch.reject = reject;
        });
      }
      if (failNextRootShellLaunchTarget === "home-gui") {
        failNextRootShellLaunchTarget = "";
        throw new Error("simulated root shell launch failure");
      }
      return jsonResponse({
        attach_kind: "iframe",
        route: "/apps/home-gui/?shell_mode=root&home_origin=http%3A%2F%2Flocalhost%3A61180#home_token=gui-token",
        target: "home-gui",
      });
    }
    if (body?.target === "system") {
      return jsonResponse({
        attach_kind: "iframe",
        route: "/apps/system/?home_origin=http%3A%2F%2Flocalhost%3A61180#home_token=system-token",
        target: "system",
      });
    }
    assert(body?.target === "home-cli", "system switch should launch Home CLI as the root shell", body);
    assert(body?.query?.shell_mode === "root", "system switch did not launch shell root mode", body);
    rootShellLaunchCounts["home-cli"] += 1;
    if (deferredRootShellLaunch?.target === "home-cli") {
      return new Promise((resolve, reject) => {
        deferredRootShellLaunch.resolve = resolve;
        deferredRootShellLaunch.reject = reject;
      });
    }
    if (failNextRootShellLaunchTarget === "home-cli") {
      failNextRootShellLaunchTarget = "";
      throw new Error("simulated root shell launch failure");
    }
    return jsonResponse({
      attach_kind: "iframe",
      route: "/apps/home-cli/?shell_mode=root&home_origin=http%3A%2F%2Flocalhost%3A61180#home_token=root-token",
      target: "home-cli",
    });
  }
  return jsonResponse({ ok: true });
};

const hostCore = await import(`../capsules/home/browser/shell-core.js?v=${moduleVersion}`);
await import(`../capsules/home/browser/home-shell-host.js?v=${moduleVersion}`);
await waitFor(
  () => Boolean(elementForSelector("#active-shell-frame").dataset.route),
  "setup did not launch the initial root shell",
  () => ({ requests }),
);

const activeShellRoot = elementForSelector("#active-shell-root");
const activeShellFrame = elementForSelector("#active-shell-frame");
const shellHostRecovery = elementForSelector("#shell-host-recovery");
const shellHostRecoveryHomeButton = elementForSelector("#shell-host-recovery-home");
assert(document.body.dataset.homeShell === "desktop", "setup did not mount Home GUI", document.body.dataset);
assert(activeShellFrame.dataset.route.includes("/apps/home-gui/"), "setup did not launch isolated Home GUI", activeShellFrame.dataset);

const guiMessages = [];
const guiFrameWindow = {
  postMessage(payload, origin) {
    guiMessages.push({ payload, origin });
  },
};
activeShellFrame.contentWindow = guiFrameWindow;
dispatchMessage(guiFrameWindow, {
  type: "home:launch-target",
  requestId: "launch-system",
  target: "system",
  query: {},
  homeToken: "gui-token",
});
await waitFor(
  () => guiMessages.some((message) => message.payload?.requestId === "launch-system"),
  "home GUI did not launch System",
  () => ({ guiMessages, requests }),
);

const staleLaunchSeq = hostCore.shellState.activeShellRootLaunchSeq;
const systemFrameWindow = { postMessage() {} };
dispatchMessage(systemFrameWindow, { type: "home:app-ready", homeToken: "system-token" });
dispatchMessage(systemFrameWindow, {
  type: "home:active-shell-applied",
  activeShell: "home-cli",
  homeToken: "system-token",
});

assert(document.body.dataset.homeShell === "alternate", "System shell switch did not retire Home GUI immediately", document.body.dataset);
assert(document.body.dataset.homeGui === "dormant", "System shell switch left Home GUI mounted", document.body.dataset);
assert(activeShellRoot.hidden === false, "System shell switch did not expose the root shell mount");
assert(activeShellRoot.dataset.target === "home-cli", "System shell switch did not preclaim Home CLI root", activeShellRoot.dataset);
assert(activeShellFrame.hidden === true, "System shell switch did not blank the stale shell frame before relaunch");
assert(!activeShellFrame.dataset.route, "System shell switch left the stale shell route visible", activeShellFrame.dataset);
assert(activeShellFrame.src === "about:blank", "System shell switch did not unload the stale shell frame before relaunch", activeShellFrame.src);
assert(elementForSelector("#home-shell-boot-mask").hidden === false, "System shell switch did not show the neutral boot mask");
assert(
  hostCore.shellState.activeShellRootLaunchSeq > staleLaunchSeq,
  "System shell switch did not cancel stale root-shell launches",
  hostCore.shellState,
);
assert(
  localStorageValues.get("elastos.home.active-shell-hint") === "home-cli",
  "System shell switch did not store the alternate-shell paint hint",
  Object.fromEntries(localStorageValues),
);
assert(
  hostCore.shellState.pendingAppliedShellTarget === "home-cli",
  "System shell switch did not retain the trusted pending shell target while summary was stale",
  hostCore.shellState,
);
await waitFor(
  () => activeShellFrame.dataset.route.includes("/apps/home-cli/"),
  "System shell switch did not relaunch Home CLI while the first summary still pointed at Desktop",
  () => ({ requests, shellState: hostCore.shellState, summary: summary.active_shell }),
);

const cliFrameWindow = { postMessage() {} };
activeShellFrame.contentWindow = cliFrameWindow;
assert(activeShellFrame.hidden === true, "System shell switch revealed Home CLI before the shell-ready handshake");
assert(activeShellFrame.dataset.route.includes("/apps/home-cli/"), "System shell switch did not relaunch Home CLI", activeShellFrame.dataset);
assert(elementForSelector("#home-shell-boot-mask").hidden === false, "System shell switch hid the neutral host mask before Home CLI was ready");
dispatchMessage(cliFrameWindow, {
  type: "home:shell-ready",
  homeToken: "root-token",
});
await waitFor(
  () => activeShellFrame.hidden === false && elementForSelector("#home-shell-boot-mask").hidden === true,
  "System shell switch did not settle after the Home CLI ready handshake",
  () => ({ shellState: hostCore.shellState, activeShellFrame, body: document.body.dataset }),
);
assert(shellHostRecovery.hidden === true, "System shell switch surfaced recovery after a valid Home CLI ready handshake");
assert(
  requests.filter((request) => request.url === "/api/apps/home/launch" && request.body?.target === "home-cli").length >= 1,
  "System shell switch did not relaunch through Runtime after the applied-shell message",
  requests,
);

summary.active_shell.active = "home-cli";
await hostCore.shellState.requestSummaryRefresh();
assert(
  hostCore.shellState.pendingAppliedShellTarget === "",
  "System shell switch did not clear the trusted pending shell target after summary catch-up",
  hostCore.shellState,
);

const cliRootRoute = activeShellFrame.dataset.route;
deferredRootShellLaunch = { target: "home-gui" };
dispatchMessage(cliFrameWindow, {
  type: "home:active-shell-applied",
  activeShell: "home-gui",
  homeToken: "root-token",
});
await waitFor(
  () => typeof deferredRootShellLaunch?.reject === "function",
  "Desktop relaunch did not begin after Home CLI requested it",
  () => ({ requests, shellState: hostCore.shellState }),
);
summary.active_shell.active = "home-cli";
hostCore.shellState.activeShellRootLaunchSeq += 1;
hostCore.shellState.pendingAppliedShellTarget = "";
hostCore.shellState.activeShellRootTarget = "home-cli";
hostCore.shellState.activeShellRootRoute = cliRootRoute;
activeShellFrame.hidden = false;
activeShellFrame.src = cliRootRoute;
activeShellFrame.dataset.route = cliRootRoute;
deferredRootShellLaunch.reject(new Error("simulated stale Desktop launch failure"));
deferredRootShellLaunch = null;
await flush(2);
assert(shellHostRecovery.hidden === true, "A stale Desktop launch failure surfaced recovery after Home CLI superseded it", shellHostRecovery.dataset);
assert(
  hostCore.shellState.activeShellRootTarget === "home-cli",
  "A stale Desktop launch failure revived the wrong active shell target",
  hostCore.shellState,
);
summary.active_shell.active = "home-cli";
dispatchMessage(cliFrameWindow, {
  type: "home:active-shell-applied",
  activeShell: "home-gui",
  homeToken: "root-token",
});
activeShellFrame.contentWindow = guiFrameWindow;
await waitFor(
  () => activeShellFrame.dataset.route.includes("/apps/home-gui/"),
  "Desktop relaunch setup did not restore Home GUI",
  () => ({ requests, shellState: hostCore.shellState }),
);
assert(activeShellFrame.hidden === true, "Desktop relaunch revealed Home GUI before the shell-ready handshake");
assert(elementForSelector("#home-shell-boot-mask").hidden === false, "Desktop relaunch hid the neutral host mask before Home GUI was ready");
dispatchMessage(guiFrameWindow, {
  type: "home:shell-ready",
  homeToken: "gui-token",
});
await waitFor(
  () => activeShellFrame.hidden === false && elementForSelector("#home-shell-boot-mask").hidden === true,
  "Desktop relaunch did not settle after the Home GUI ready handshake",
  () => ({ shellState: hostCore.shellState, activeShellFrame, body: document.body.dataset }),
);
summary.active_shell.active = "home-gui";
await hostCore.shellState.requestSummaryRefresh();

failNextRootShellLaunchTarget = "home-cli";
dispatchMessage(guiFrameWindow, {
  type: "home:active-shell-applied",
  activeShell: "home-cli",
  homeToken: "gui-token",
});
await waitFor(
  () => shellHostRecovery.hidden === false && shellHostRecovery.dataset.target === "home-cli",
  "Terminal launch failure did not surface host recovery while summary still pointed at Desktop",
  () => ({ requests, shellState: hostCore.shellState, summary: summary.active_shell }),
);
assert(
  hostCore.shellState.pendingAppliedShellTarget === "home-cli",
  "Terminal launch failure did not retain the pending Terminal target until recovery",
  hostCore.shellState,
);
click(shellHostRecoveryHomeButton);
await waitFor(
  () => requests.some((request) => request.url === "/api/apps/home/active-shell" && request.body?.active === "home-gui"),
  "Desktop recovery did not force an active-shell change while summary was stale",
  () => ({ requests, shellState: hostCore.shellState }),
);
await waitFor(
  () => activeShellFrame.dataset.route.includes("/apps/home-gui/"),
  "Desktop recovery did not relaunch Home GUI after a stale-summary Terminal failure",
  () => ({ requests, shellState: hostCore.shellState, summary: summary.active_shell }),
);
summary.active_shell.active = "home-gui";
await hostCore.shellState.requestSummaryRefresh();

nextSummaryFailure = failedResponse(401, "Unauthorized", "expired");
dispatchMessage(guiFrameWindow, {
  type: "home:active-shell-applied",
  activeShell: "home-cli",
  homeToken: "gui-token",
});
await waitFor(
  () => hostCore.shellState.pendingAppliedShellTarget === "" && activeShellRoot.hidden === true,
  "Auth gate did not clear the pending shell target and root shell mount",
  () => ({ shellState: hostCore.shellState, body: document.body.dataset }),
);

console.log("[home-shell-system-switch] PASS");
