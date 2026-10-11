#!/usr/bin/env node

const moduleVersion = "home-update-20261003a";
const requests = [];
const localStorageValues = new Map([
  ["elastos.home.active-shell-hint", "home-cli"],
]);
let summaryCalls = 0;
let runtimeEnsureSawGuiSuppressed = false;
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
  "#home-clipboard-prompt",
  "#home-clipboard-title",
  "#home-clipboard-copy",
  "#home-clipboard-allow",
  "#home-clipboard-cancel",
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
  "#desktop",
  "#desktop-context-menu",
  "#home-notification-toast",
  "#launcher-toggle",
  "#toolbar-home",
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
        { name: "home-cli", title: "Home CLI", role: "shell", launchable: false, route: "/apps/home-cli/" },
      ],
    },
    app: { id: "home", route: "/apps/home/" },
    appearance: {},
    browser_state: {
      principal_id: "principal:home-shell-unavailable-shell",
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
  getElementById(id) { return this.querySelector(`#${id}`); },
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
  crypto: { randomUUID: () => "home-shell-unavailable-shell-smoke" },
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
elementForSelector("#home-clipboard-prompt").hidden = true;
elementForSelector("#home-unlock").hidden = true;

let savedShell = "home-cli";

globalThis.fetch = async (url, init = {}) => {
  const body = init.body ? JSON.parse(init.body) : null;
  requests.push({ body, method: init.method || "GET", url: String(url) });
  if (url === "/api/apps/home/summary") {
    return jsonResponse(summaryFor(savedShell));
  }
  if (url === "/api/apps/home/active-shell" && init.method === "POST") {
    savedShell = body?.active;
    return jsonResponse({ active: savedShell });
  }
  if (url === "/api/apps/home/launch") {
    assert(body?.target !== "home-cli", "Home launched a shell that cannot start", body);
    return jsonResponse({ attach_kind: "iframe", route: "/apps/home/?shell_mode=root#home_token=root-token", target: body?.target });
  }
  return jsonResponse({ ok: true });
};

await import(`../capsules/home/browser/home-shell-host.js?v=${moduleVersion}&unavailable-shell`);
for (let attempt = 0; attempt < 50 && savedShell !== "home-gui"; attempt += 1) {
  await new Promise((resolve) => setTimeout(resolve, 0));
}

const resets = requests.filter((request) => request.url === "/api/apps/home/active-shell" && request.method === "POST");
assert(savedShell === "home-gui", "Home kept a saved shell that cannot start", { savedShell, requests });
assert(resets.length === 1 && resets[0].body?.active === "home-gui", "Home did not reset the saved shell to the Desktop once", resets);
assert(!localStorageValues.has("elastos.home.active-shell-hint"), "Home kept the Terminal boot hint, so a reload would not land on the Desktop", [...localStorageValues]);
assert(elementForSelector("#shell-host-recovery").hidden === true, "Home showed the shell recovery screen instead of falling back to the Desktop");
assert(!requests.some((request) => request.url === "/api/apps/home/launch" && request.body?.target === "home-cli"), "Home tried to launch the unavailable shell", requests);

console.log("[home-shell-unavailable-shell-fallback] PASS");
