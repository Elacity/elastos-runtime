#!/usr/bin/env node

const moduleVersion = "home-20260713a";
const requests = [];
const windowListeners = new Map();
const localStorageValues = new Map();
const windowFrames = [];

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

function assert(condition, message, details = null) {
  if (!condition) {
    const error = new Error(message);
    error.details = details;
    throw error;
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
  clearTimeout() {},
  setInterval: () => 0,
  setTimeout: (callback) => {
    Promise.resolve().then(callback);
    return 0;
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
    return jsonResponse(summary);
  }
  if (url === "/api/apps/home/launch") {
    assert(body?.target === "home-cli", "system switch should launch Home CLI as the root shell", body);
    assert(body?.query?.shell_mode === "root", "system switch did not launch shell root mode", body);
    return jsonResponse({
      attach_kind: "iframe",
      route: "/apps/home-cli/?shell_mode=root&home_token=root-token",
      target: "home-cli",
    });
  }
  return jsonResponse({ ok: true });
};

const hostCore = await import(`../capsules/home/browser/shell-core.js?v=${moduleVersion}`);
await import(`../capsules/home/browser/home-shell-host.js?v=${moduleVersion}`);
for (let attempt = 0; attempt < 20 && document.body.dataset.homeGui !== "mounted"; attempt += 1) {
  await new Promise((resolve) => setTimeout(resolve, 0));
}
const homeGuiCore = await import(`../capsules/home-gui/browser/shell-core.js?v=${moduleVersion}`);

const activeShellRoot = elementForSelector("#active-shell-root");
const activeShellFrame = elementForSelector("#active-shell-frame");
assert(document.body.dataset.homeShell === "desktop", "setup did not mount Home GUI", document.body.dataset);
assert(activeShellFrame.dataset.route === "", "setup launched a root shell before System switched", activeShellFrame.dataset);

const staleLaunchSeq = hostCore.shellState.activeShellRootLaunchSeq;
const systemWindow = new FakeElement("system-window");
systemWindow.dataset.target = "system";
systemWindow.dataset.windowId = "system--1";
const systemFrame = new FakeElement("system-frame");
systemFrame.contentWindow = {};
systemFrame.dataset.route = "/apps/system/?home_token=system-token";
systemFrame.closestNode = systemWindow;
systemWindow.querySelector = (selector) => selector === ".window-frame"
  ? systemFrame
  : new FakeElement(`system-window ${selector}`);
homeGuiCore.shellState.windows.set("system--1", {
  id: "system--1",
  kind: "system",
  node: systemWindow,
  serial: 1,
  targetId: "system",
  title: "System",
});
homeGuiCore.shellState.activeWindowId = "system--1";
summary.active_shell.active = "home-cli";

for (const listener of windowListeners.get("message") || []) {
  listener({
    origin: "http://localhost:61180",
    source: systemFrame.contentWindow,
    data: {
      type: "home:active-shell-applied",
      activeShell: "home-cli",
      homeToken: "system-token",
    },
  });
}

assert(document.body.dataset.homeShell === "alternate", "System shell switch did not retire Home GUI immediately", document.body.dataset);
assert(document.body.dataset.homeGui === "dormant", "System shell switch left Home GUI mounted", document.body.dataset);
assert(activeShellRoot.hidden === false, "System shell switch did not expose the root shell mount");
assert(activeShellRoot.dataset.target === "home-cli", "System shell switch did not preclaim Home CLI root", activeShellRoot.dataset);
assert(activeShellFrame.hidden === true, "System shell switch did not blank the stale shell frame before relaunch");
assert(!activeShellFrame.dataset.route, "System shell switch left the stale shell route visible", activeShellFrame.dataset);
assert(!activeShellFrame.getAttribute("src"), "System shell switch left the stale iframe src visible");
assert(elementForSelector("#home-shell-boot-mask").hidden === false, "System shell switch did not show the neutral boot mask");
assert(systemWindow.removed === true, "System shell switch did not remove stale GUI windows");
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

for (let attempt = 0; attempt < 20 && activeShellFrame.dataset.route === ""; attempt += 1) {
  await new Promise((resolve) => setTimeout(resolve, 0));
}

assert(activeShellFrame.hidden === false, "System shell switch did not restore the active shell frame");
assert(activeShellFrame.dataset.route.includes("/apps/home-cli/"), "System shell switch did not relaunch Home CLI", activeShellFrame.dataset);
assert(
  requests.filter((request) => request.url === "/api/apps/home/launch" && request.body?.target === "home-cli").length >= 1,
  "System shell switch did not relaunch through Runtime after the applied-shell message",
  requests,
);

console.log("[home-shell-system-switch] PASS");
