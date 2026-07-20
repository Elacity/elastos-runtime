#!/usr/bin/env node

const moduleVersion = "home-20260719y";
const requests = [];
const windowListeners = new Map();

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
  PublicKeyCredential: function PublicKeyCredential() {},
  crypto: { randomUUID: () => "home-shell-auth-gate-smoke" },
  location: { href: "http://localhost:61180/apps/home/", origin: "http://localhost:61180" },
  localStorage: { getItem: () => null, removeItem() {}, setItem() {} },
  performance: { now: () => Date.now() },
  innerWidth: 1280,
  addEventListener(type, callback) {
    if (!windowListeners.has(type)) {
      windowListeners.set(type, []);
    }
    windowListeners.get(type).push(callback);
  },
  clearInterval() {},
  clearTimeout() {},
  setInterval: () => 0,
  setTimeout(callback) {
    if (typeof callback === "function") {
      callback();
    }
    return 0;
  },
};

elementForSelector("#home-unlock").hidden = true;
elementForSelector("#launcher").hidden = true;
elementForSelector("#shell-host-recovery").hidden = true;
elementForSelector("#shell-host-recovery-detail").hidden = true;
elementForSelector("#desktop-context-menu").hidden = true;
elementForSelector("#home-notification-toast").hidden = true;

globalThis.fetch = async (url, init = {}) => {
  requests.push({
    body: init.body ? JSON.parse(init.body) : null,
    headers: init.headers || {},
    method: init.method || "GET",
    url: String(url),
  });
  if (url === "/api/apps/home/summary") {
    return jsonResponse({
      app: { id: "home", route: "/apps/home/" },
      authority: { signed_in: false },
      active_shell: {
        schema: "elastos.home.active-shell/v1",
        active: "home-gui",
        candidates: [],
      },
      targets: [],
    });
  }
  if (url === "/api/auth/passkey/status") {
    return jsonResponse({ registered: true, guest_registration_enabled: false });
  }
  return jsonResponse({ ok: true });
};

document.body.dataset.homeStatus = "ready";
document.body.dataset.homeShell = "desktop";
document.body.dataset.homeGui = "mounted";
const activeShellRoot = elementForSelector("#active-shell-root");
const activeShellFrame = elementForSelector("#active-shell-frame");
activeShellRoot.hidden = false;
activeShellRoot.dataset.target = "home-cli";
activeShellFrame.hidden = false;
activeShellFrame.dataset.route = "/apps/home-cli/?shell_mode=root#home_token=stale-token";
activeShellFrame.setAttribute("src", activeShellFrame.dataset.route);

await import(`../capsules/home/browser/home-shell-host.js?v=${moduleVersion}`);
for (let attempt = 0; attempt < 20 && elementForSelector("#home-unlock").hidden; attempt += 1) {
  await new Promise((resolve) => setTimeout(resolve, 0));
}

const unlock = elementForSelector("#home-unlock");
assert(unlock.hidden === false, "auth gate did not show the passkey prompt");
assert(unlock.dataset.surface === "neutral", "auth gate did not use a neutral non-GUI unlock surface", unlock.dataset);
assert(
  elementForSelector("#home-shell-boot-mask").hidden === true,
  "auth gate left the neutral host mask over the passkey prompt",
);
assert(
  document.body.dataset.homeStatus === "locked",
  "unsigned auth gate must leave Home locked for the account picker",
  document.body.dataset,
);
assert(document.body.dataset.homeShell === "resolving", "auth gate left a root shell visible", document.body.dataset);
assert(document.body.dataset.homeGui === "dormant", "auth gate left Home GUI mounted", document.body.dataset);
assert(activeShellRoot.hidden === true, "auth gate left the active shell root visible");
assert(activeShellRoot.dataset.target === "", "auth gate kept a stale active shell target", activeShellRoot.dataset);
assert(activeShellFrame.hidden === true, "auth gate left the active shell frame visible");
assert(!activeShellFrame.dataset.route, "auth gate kept a stale active shell route", activeShellFrame.dataset);
assert(!activeShellFrame.getAttribute("src"), "auth gate kept a stale shell iframe src", activeShellFrame.getAttribute("src"));
assert(!requests.some((request) => request.url === "/api/apps/home/active-shell"), "auth gate tried to switch shells without a token", requests);
assert(!requests.some((request) => request.url === "/api/apps/home/launch"), "auth gate tried to launch a shell while locked", requests);

for (const listener of windowListeners.get("message") || []) {
  listener({
    origin: "http://localhost:61180",
    source: window,
    data: { type: "home:refresh-summary" },
  });
}
for (
  let attempt = 0;
  attempt < 20 && requests.filter((request) => request.url === "/api/apps/home/summary").length < 2;
  attempt += 1
) {
  await new Promise((resolve) => setTimeout(resolve, 0));
}
assert(
  document.body.dataset.homeShell === "resolving",
  "locked summary refresh exposed a root shell",
  document.body.dataset,
);
assert(activeShellRoot.hidden === true, "locked summary refresh showed the active shell root");
assert(activeShellFrame.hidden === true, "locked summary refresh showed the active shell frame");

console.log("[home-shell-auth-gate] PASS");
