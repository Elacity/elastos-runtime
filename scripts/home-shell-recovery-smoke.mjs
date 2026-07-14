#!/usr/bin/env node

const moduleVersion = "home-20260712b";
const requests = [];
const originalConsoleError = console.error;
console.error = (...args) => {
  if (String(args[0] || "").includes("active shell root launch failed")) {
    return;
  }
  originalConsoleError(...args);
};

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
    this.src = "";
    this.attributes = new Map();
    this.listeners = new Map();
    this.classList = new FakeClassList();
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
    this.children.push(child);
    return child;
  }

  cloneNode() {
    return new FakeElement(`${this.selector}:clone`);
  }

  closest() {
    return null;
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

const summary = {
  authority: { signed_in: true },
  active_shell: {
    active: "home-cli",
    candidates: [
      { name: "home-gui", title: "Home GUI", role: "shell", launchable: true, route: "/apps/home/" },
      { name: "home-cli", title: "Home CLI", role: "shell", launchable: true, route: "/apps/home-cli/" },
    ],
  },
  app: { id: "home", route: "/apps/home/" },
  appearance: {},
  browser_state: {
    principal_id: "principal:home-shell-recovery",
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
    { target: "inbox", title: "Inbox", attach_kind: "iframe", role: "app", target_kind: "app" },
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
  querySelectorAll: () => [],
};
Object.defineProperty(globalThis, "navigator", {
  configurable: true,
  value: {},
});
globalThis.window = {
  crypto: { randomUUID: () => "home-shell-recovery-smoke" },
  location: {
    href: "http://localhost:61180/apps/home/",
    origin: "http://localhost:61180",
    reload() {},
  },
  localStorage: { getItem: () => null, setItem: () => {}, removeItem: () => {} },
  performance: { now: () => Date.now() },
  innerWidth: 1280,
  addEventListener() {},
  clearInterval() {},
  clearTimeout() {},
  setInterval: () => 0,
  setTimeout: () => 0,
};

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
    assert(body?.target === "home-cli", "alternate shell launch target drifted", body);
    assert(body?.query?.shell_mode === "root", "alternate shell must launch in root mode", body);
    return failedResponse(500, "Internal Server Error", "simulated root shell launch failure");
  }
  return jsonResponse({ ok: true });
};

await import(`../capsules/home/browser/home-shell-host.js?v=${moduleVersion}`);
for (let attempt = 0; attempt < 20 && elementForSelector("#shell-host-recovery").hidden; attempt += 1) {
  await new Promise((resolve) => setTimeout(resolve, 0));
}

const body = document.body;
const activeShellRoot = elementForSelector("#active-shell-root");
const activeShellFrame = elementForSelector("#active-shell-frame");
const recovery = elementForSelector("#shell-host-recovery");
const recoveryTitle = elementForSelector("#shell-host-recovery-title");
const recoveryCopy = elementForSelector("#shell-host-recovery-copy");
const recoveryDetail = elementForSelector("#shell-host-recovery-detail");
const recoveryHome = elementForSelector("#shell-host-recovery-home");
const recoveryReload = elementForSelector("#shell-host-recovery-reload");
const recoverySignOut = elementForSelector("#shell-host-recovery-sign-out");
const workspace = elementForSelector(".desktop-workspace");
const toolbarHome = elementForSelector("#toolbar-home");
const toolbarInbox = elementForSelector("#toolbar-inbox");
const launcherToggle = elementForSelector("#launcher-toggle");
const launcherSearch = elementForSelector("#launcher-search");
const desktop = elementForSelector("#desktop");

assert(body.dataset.homeShell === "alternate", "failed shell launch did not take alternate host mode", body.dataset);
assert(body.dataset.homeGui === "dormant", "Home GUI was not marked dormant after failed shell launch", body.dataset);
assert(activeShellRoot.hidden === false, "active shell root stayed hidden after failed shell launch");
assert(activeShellRoot.dataset.target === "home-cli", "recovery target drifted", activeShellRoot.dataset);
assert(activeShellFrame.hidden === true, "active shell frame stayed visible after failed shell launch");
assert(!activeShellFrame.dataset.route, "failed shell launch kept a stale frame route", activeShellFrame.dataset);
assert(recovery.hidden === false, "host recovery panel did not show after failed shell launch");
assert(recovery.dataset.host === "home-shell-host", "host recovery did not advertise host ownership", recovery.dataset);
assert(recovery.dataset.target === "home-cli", "host recovery target drifted", recovery.dataset);
assert(recoveryTitle.textContent.includes("Terminal"), "recovery title did not name failed shell", recoveryTitle.textContent);
assert(recoveryCopy.textContent.includes("Reload"), "recovery copy did not expose reload path", recoveryCopy.textContent);
assert(recoveryDetail.textContent === "A Home service failed while loading.", "recovery detail exposed an internal launch error", recoveryDetail.textContent);
assert(recoveryHome.disabled === true, "switchback button must fail closed without a launch token");
assert((recoveryHome.listeners.get("click") || []).length === 1, "home-gui recovery control was not wired");
assert((recoveryReload.listeners.get("click") || []).length === 1, "reload recovery control was not wired");
assert((recoverySignOut.listeners.get("click") || []).length === 1, "sign-out recovery control was not wired");
assert((toolbarHome.listeners.get("click") || []).length === 0, "Home GUI toolbar was bound before failed alternate shell settled");
assert((toolbarInbox.listeners.get("click") || []).length === 0, "Home GUI inbox control was bound before failed alternate shell settled");
assert((launcherToggle.listeners.get("click") || []).length === 0, "Home GUI launcher was bound before failed alternate shell settled");
assert((launcherSearch.listeners.get("input") || []).length === 0, "Home GUI launcher search was bound before failed alternate shell settled");
assert((workspace.listeners.get("contextmenu") || []).length === 0, "Home GUI desktop context menu was bound before failed alternate shell settled");
assert((desktop.listeners.get("pointerdown") || []).length === 0, "Home GUI desktop input was bound before failed alternate shell settled");
assert(!requests.some((request) => request.url === "/api/apps/home/active-shell"), "failed launch recovery must not switch shell using ambient state", requests);

console.log("[home-shell-recovery] PASS");
