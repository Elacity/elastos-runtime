#!/usr/bin/env node

const moduleVersion = "home-20260719c";
const requests = [];
const localStorageValues = new Map([
  ["elastos.home.active-shell-hint", "home-cli"],
]);
const originalConsoleError = console.error;
console.error = (...args) => {
  if (String(args[0] || "").includes("home-gui recovery failed")) {
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

  append(...children) {
    for (const child of children) this.appendChild(child);
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
    principal_id: "principal:home-shell-switchback-recovery",
    layout: { desktop: {}, taskbar: [], desktopApps: [], desktopIconsVisible: true },
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
  crypto: { randomUUID: () => "home-shell-switchback-recovery-smoke" },
  location: {
    href: "http://localhost:61180/apps/home/",
    origin: "http://localhost:61180",
    reload() {},
  },
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
    assert(body?.target === "home-cli", "switchback recovery launched the wrong active shell", body);
    assert(body?.query?.shell_mode === "root", "switchback recovery did not launch root shell mode", body);
    return jsonResponse({
      attach_kind: "iframe",
      route: "/apps/home-cli/?shell_mode=root&home_origin=http%3A%2F%2Flocalhost%3A61180#home_token=root-token",
      target: "home-cli",
    });
  }
  if (url === "/api/apps/home/active-shell") {
    assert(body?.active === "home-gui", "switchback recovery requested the wrong active shell", body);
    assert(
      init.headers?.["x-elastos-home-token"] === "root-token",
      "switchback recovery did not use the mounted shell launch token",
      init.headers,
    );
    return failedResponse(500, "Internal Server Error", "simulated active shell switch failure");
  }
  return jsonResponse({ ok: true });
};

await import(`../capsules/home/browser/home-shell-host.js?v=${moduleVersion}&switchback-recovery`);
for (let attempt = 0; attempt < 20 && !elementForSelector("#active-shell-frame").dataset.route; attempt += 1) {
  await new Promise((resolve) => setTimeout(resolve, 0));
}

const activeShellRoot = elementForSelector("#active-shell-root");
const activeShellFrame = elementForSelector("#active-shell-frame");
const recovery = elementForSelector("#shell-host-recovery");
const recoveryTitle = elementForSelector("#shell-host-recovery-title");
const recoveryCopy = elementForSelector("#shell-host-recovery-copy");
const recoveryDetail = elementForSelector("#shell-host-recovery-detail");
const recoveryHome = elementForSelector("#shell-host-recovery-home");
const toolbarHome = elementForSelector("#toolbar-home");
const launcherToggle = elementForSelector("#launcher-toggle");
const desktop = elementForSelector("#desktop");

assert(document.body.dataset.homeShell === "alternate", "switchback setup did not enter alternate shell mode", document.body.dataset);
assert(document.body.dataset.homeGui === "dormant", "switchback setup left Home GUI mounted", document.body.dataset);
assert(activeShellRoot.hidden === false, "switchback setup did not show active shell root");
assert(activeShellRoot.dataset.target === "home-cli", "switchback setup active shell target drifted", activeShellRoot.dataset);
assert(activeShellFrame.hidden === false, "switchback setup did not show active shell frame");
assert(activeShellFrame.dataset.route.includes("#home_token=root-token"), "switchback setup did not carry root launch token", activeShellFrame.dataset);
assert(
  elementForSelector("#home-shell-boot-mask").hidden === true,
  "switchback setup left the neutral host mask over the launched shell",
);
assert((recoveryHome.listeners.get("click") || []).length === 1, "switchback recovery control was not wired");

for (const listener of recoveryHome.listeners.get("click") || []) {
  listener();
}
for (let attempt = 0; attempt < 20 && recovery.hidden; attempt += 1) {
  await new Promise((resolve) => setTimeout(resolve, 0));
}

const activeShellRequests = requests.filter((request) => request.url === "/api/apps/home/active-shell");
assert(activeShellRequests.length === 1, "switchback recovery should make exactly one active-shell request", requests);
assert(activeShellRequests[0].headers["x-elastos-home-token"] === "root-token", "switchback recovery lost the shell launch token", activeShellRequests[0]);
assert(recovery.hidden === false, "switchback failure did not show host recovery");
assert(recovery.dataset.host === "home-shell-host", "switchback recovery did not advertise host ownership", recovery.dataset);
assert(recovery.dataset.target === "home-cli", "switchback recovery target drifted", recovery.dataset);
assert(recoveryTitle.textContent.includes("Desktop"), "switchback recovery title did not name Desktop", recoveryTitle.textContent);
assert(recoveryCopy.textContent.includes("Reload"), "switchback recovery copy did not expose reload path", recoveryCopy.textContent);
assert(recoveryDetail.textContent === "A Home service failed while loading.", "switchback recovery exposed an internal shell error", recoveryDetail.textContent);
assert(
  elementForSelector("#home-shell-boot-mask").hidden === true,
  "switchback failure left the neutral host mask over recovery",
);
assert(activeShellFrame.hidden === true, "switchback failure left the shell frame visible");
assert(document.body.dataset.homeShell === "alternate", "switchback failure mounted Home GUI", document.body.dataset);
assert(document.body.dataset.homeGui === "dormant", "switchback failure left Home GUI mounted", document.body.dataset);
assert((toolbarHome.listeners.get("click") || []).length === 0, "switchback failure bound Home GUI toolbar interactions");
assert((launcherToggle.listeners.get("click") || []).length === 0, "switchback failure bound Home GUI launcher interactions");
assert((desktop.listeners.get("pointerdown") || []).length === 0, "switchback failure bound Home GUI desktop input");
assert(
  !requests.some((request) => request.url === "/api/apps/home/active-shell" && !request.headers["x-elastos-home-token"]),
  "switchback recovery used ambient active-shell authority",
  requests,
);

console.log("[home-shell-switchback-recovery] PASS");
