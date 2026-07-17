#!/usr/bin/env node

const moduleVersion = "home-20260717b";
const requests = [];
const localStorageValues = new Map([
  ["elastos.home.active-shell-hint", "home-gui"],
]);
let summaryCalls = 0;
let runtimeEnsureSawGuiMounted = false;
const hostStaticSelectors = new Set([
  ".home-host-shell",
  ".home-unlock-card",
  "#active-shell-frame",
  "#active-shell-root",
  "#home-shell-boot-mask",
  "#home-unlock",
  "#home-unlock-copy",
  "#home-unlock-name",
  "#home-unlock-primary",
  "#home-unlock-secondary",
  "#home-unlock-status",
  "#home-unlock-title",
  "#shell-host-recovery",
  "#shell-host-recovery-copy",
  "#shell-host-recovery-detail",
  "#shell-host-recovery-home",
  "#shell-host-recovery-reload",
  "#shell-host-recovery-sign-out",
  "#shell-host-recovery-title",
  "#window-snap-preview",
]);
const homeGuiSelectors = [
  ".desktop-backdrop",
  ".toolbar",
  ".desktop-workspace",
  ".taskbar",
  ".launcher",
  "#launcher",
  "#desktop",
  "#desktop-shortcuts",
  "#desktop-context-menu",
  "#home-notification-toast",
  "#launcher-grid",
  "#launcher-empty-state",
  "#launcher-search",
  "#launcher-toggle",
  "#close-launcher",
  "#toolbar-home",
  "#toolbar-inbox",
  "#toolbar-inbox-count",
  "#toolbar-fullscreen",
  "#toolbar-sign-out",
  "#clock",
  "#taskbar-targets",
  "#launcher-item-template",
  "#shortcut-template",
  "#taskbar-item-template",
  "#window-error-template",
  "#window-template",
];

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
    this.value = "";
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

function querySelector(selector) {
  if (elementCache.has(selector)) {
    return elementCache.get(selector);
  }
  if (hostStaticSelectors.has(selector)) {
    return elementForSelector(selector);
  }
  if (homeGuiSelectors.includes(selector)) {
    return elementForSelector(selector);
  }
  return null;
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

function summaryFor(activeShell) {
  return {
    authority: { signed_in: true },
    active_shell: {
      active: activeShell,
      candidates: [
        { name: "home-gui", title: "Home GUI", role: "shell", launchable: true, route: "/apps/home/" },
        { name: "home-cli", title: "Home CLI", role: "shell", launchable: true, route: "/apps/home-cli/" },
      ],
    },
    app: { id: "home", route: "/apps/home/" },
    appearance: {},
    browser_state: {
      principal_id: "principal:home-shell-no-hint-boot",
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
    ],
  };
}

globalThis.HTMLElement = FakeElement;
globalThis.document = {
  activeElement: null,
  body: elementForSelector("body"),
  documentElement: elementForSelector("html"),
  addEventListener() {},
  createElement: (tag) => new FakeElement(tag),
  querySelector,
  querySelectorAll: () => [],
};
Object.defineProperty(globalThis, "navigator", {
  configurable: true,
  value: {},
});
globalThis.window = {
  crypto: { randomUUID: () => "home-shell-no-hint-boot-smoke" },
  location: { href: "http://localhost:61180/apps/home/", origin: "http://localhost:61180" },
  localStorage: {
    getItem: (key) => localStorageValues.get(key) || null,
    removeItem: (key) => localStorageValues.delete(key),
    setItem: (key, value) => localStorageValues.set(key, String(value)),
  },
  performance: { now: () => Date.now() },
  innerWidth: 1280,
  addEventListener() {},
  clearInterval() {},
  clearTimeout() {},
  setInterval: () => 0,
  setTimeout: () => 0,
};

document.body.dataset.homeStatus = "booting";
document.body.dataset.homeShell = "resolving";
document.body.dataset.homeGui = "resolving";
elementForSelector("#active-shell-root").hidden = true;
elementForSelector("#active-shell-frame").hidden = true;
elementForSelector("#shell-host-recovery").hidden = true;
elementForSelector("#shell-host-recovery-detail").hidden = true;
elementForSelector("#home-unlock").hidden = true;

globalThis.fetch = async (url, init = {}) => {
  const body = init.body ? JSON.parse(init.body) : null;
  requests.push({
    body,
    headers: init.headers || {},
    method: init.method || "GET",
    url: String(url),
  });
  if (url === "/api/apps/home/summary") {
    summaryCalls += 1;
    if (summaryCalls === 1) {
      assert(!document.documentElement.dataset.homeShellHint, "no-hint boot started with an alternate shell hint", document.documentElement.dataset);
      assert(!document.documentElement.dataset.homeShellBoot, "no-hint boot used a remembered shell boot mode", document.documentElement.dataset);
      assert(
        document.body.dataset.homeShell === "resolving",
        "no-hint boot did not stay in resolving mode until Runtime summary",
        document.body.dataset,
      );
      assert(
        elementForSelector("#home-shell-boot-mask").hidden === false,
        "no-hint boot did not keep the neutral host mask visible before Runtime summary",
      );
      assert(elementForSelector("#active-shell-root").hidden === true, "no-hint boot showed root shell before Runtime summary");
      assert(elementForSelector("#active-shell-frame").hidden === true, "no-hint boot showed shell frame before Runtime launch");
      return jsonResponse(summaryFor("home-gui"));
    }
    return jsonResponse(summaryFor("home-gui"));
  }
  if (url === "/api/apps/home/runtime/ensure") {
    assert(
      document.body.dataset.homeShell === "desktop",
      "no-hint runtime settle did not keep the Home GUI active",
      document.body.dataset,
    );
    assert(
      elementForSelector("#home-shell-boot-mask").hidden === true,
      "no-hint runtime settle left the neutral host mask over Home GUI",
    );
    assert(elementForSelector("#active-shell-root").hidden === true, "no-hint runtime settle showed a root shell");
    runtimeEnsureSawGuiMounted = true;
    return jsonResponse({ ok: true });
  }
  if (url === "/api/apps/home/launch") {
    throw new Error(`no-hint boot should not launch an alternate shell: ${JSON.stringify(body)}`);
  }
  return jsonResponse({ ok: true });
};

await import(`../capsules/home/browser/home-shell-host.js?v=${moduleVersion}&no-hint-boot`);
for (let attempt = 0; attempt < 20 && summaryCalls < 2; attempt += 1) {
  await new Promise((resolve) => setTimeout(resolve, 0));
}

const activeShellRoot = elementForSelector("#active-shell-root");
const activeShellFrame = elementForSelector("#active-shell-frame");

assert(summaryCalls >= 2, "no-hint boot did not refresh summary after Runtime settle", { summaryCalls, requests });
assert(runtimeEnsureSawGuiMounted, "no-hint boot did not prove GUI stayed mounted during Runtime settle");
assert(document.body.dataset.homeShell === "desktop", "no-hint boot did not keep Home GUI active", document.body.dataset);
assert(document.body.dataset.homeGui === "mounted", "no-hint boot left Home GUI dormant", document.body.dataset);
assert(activeShellRoot.hidden === true, "no-hint boot showed active shell root");
assert(activeShellRoot.dataset.target === "", "no-hint boot kept a stale active shell target", activeShellRoot.dataset);
assert(activeShellFrame.hidden === true, "no-hint boot showed active shell frame");
assert(!activeShellFrame.dataset.route, "no-hint boot kept a stale shell frame route", activeShellFrame.dataset);
assert(
  elementForSelector("#home-shell-boot-mask").hidden === true,
  "no-hint boot left the neutral host mask over Home GUI",
);
assert(
  !localStorageValues.has("elastos.home.active-shell-hint"),
  "no-hint boot kept a stale Home GUI shell hint",
  Object.fromEntries(localStorageValues),
);
assert(!requests.some((request) => request.url === "/api/apps/home/active-shell"), "no-hint boot switched shell using ambient state", requests);
assert(!requests.some((request) => request.url === "/api/apps/home/launch"), "no-hint boot launched an alternate shell", requests);

console.log("[home-shell-no-hint-boot] PASS");
