#!/usr/bin/env node

const moduleVersion = "home-20260802a";
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
        { name: "home-cli", title: "Home CLI", role: "shell", launchable: true, route: "/apps/home-cli/" },
      ],
    },
    app: { id: "home", route: "/apps/home/" },
    appearance: {},
    browser_state: {
      principal_id: "principal:home-shell-stale-hint",
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
  crypto: { randomUUID: () => "home-shell-stale-hint-boot-smoke" },
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

function assertGuiSuppressed(label) {
  assert(document.body.dataset.homeShell !== "desktop", `${label}: Home GUI shell became active`, document.body.dataset);
  assert(document.body.dataset.homeGui === "dormant", `${label}: Home GUI was not dormant`, document.body.dataset);
  for (const selector of homeGuiSelectors) {
    assert(querySelector(selector) === null, `${label}: ${selector} was inserted during alternate shell boot`);
  }
  assert(
    elementForSelector("#home-shell-boot-mask").hidden === false,
    `${label}: neutral boot mask was not kept over the stale GUI summary`,
  );
}

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
      assert(
        document.documentElement.dataset.homeShellHint === "alternate" &&
          document.documentElement.dataset.homeShellBoot === "alternate",
        "stale-hint boot did not apply the alternate shell paint hint before summary",
        document.documentElement.dataset,
      );
      return jsonResponse(summaryFor("home-gui"));
    }
    return jsonResponse(summaryFor("home-cli"));
  }
  if (url === "/api/apps/home/runtime/ensure") {
    assertGuiSuppressed("runtime ensure");
    runtimeEnsureSawGuiSuppressed = true;
    return jsonResponse({ ok: true });
  }
  if (url === "/api/apps/home/launch") {
    assert(body?.target === "home-cli", "stale-hint boot launched the wrong active shell", body);
    assert(body?.query?.shell_mode === "root", "stale-hint boot did not launch root shell mode", body);
    return jsonResponse({
      attach_kind: "iframe",
      route: "/apps/home-cli/?shell_mode=root&home_origin=http%3A%2F%2Flocalhost%3A61180#home_token=root-token",
      target: "home-cli",
    });
  }
  return jsonResponse({ ok: true });
};

await import(`../capsules/home/browser/home-shell-host.js?v=${moduleVersion}&stale-hint-boot`);
for (let attempt = 0; attempt < 30 && !elementForSelector("#active-shell-frame").dataset.route; attempt += 1) {
  await new Promise((resolve) => setTimeout(resolve, 0));
}

const activeShellRoot = elementForSelector("#active-shell-root");
const activeShellFrame = elementForSelector("#active-shell-frame");

assert(summaryCalls >= 2, "stale-hint boot did not refresh summary after runtime ensure", { summaryCalls, requests });
assert(runtimeEnsureSawGuiSuppressed, "stale-hint boot did not prove GUI suppression during runtime ensure");
assert(document.body.dataset.homeShell === "alternate", "stale-hint boot did not end in alternate shell mode", document.body.dataset);
assert(document.body.dataset.homeGui === "dormant", "stale-hint boot left Home GUI mounted", document.body.dataset);
assert(activeShellRoot.hidden === false, "stale-hint boot did not show active shell root after Runtime settled");
assert(activeShellRoot.dataset.target === "home-cli", "stale-hint boot active shell target drifted", activeShellRoot.dataset);
assert(activeShellFrame.hidden === false, "stale-hint boot did not show active shell frame after Runtime settled");
assert(activeShellFrame.dataset.route.includes("/apps/home-cli/"), "stale-hint boot did not load Home CLI", activeShellFrame.dataset);
assert(elementForSelector("#home-shell-boot-mask").hidden === true, "stale-hint boot left the neutral mask over Home CLI");
assert(!requests.some((request) => request.url === "/api/apps/home/active-shell"), "stale-hint boot used the hint to switch shells", requests);

console.log("[home-shell-stale-hint-boot] PASS");
