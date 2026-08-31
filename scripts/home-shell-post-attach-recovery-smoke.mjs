#!/usr/bin/env node

const moduleVersion = "home-20260802a";
const requests = [];
const windowListeners = new Map();
const localStorageValues = new Map();
const pendingTimers = new Map();
let nextTimerId = 1;

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
    this.textContent = "";
    this.title = "";
    this.value = "";
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
  addEventListener() {}
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
    principal_id: "principal:home-shell-post-attach-recovery",
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
    { target: "system", title: "System", attach_kind: "iframe", role: "app", target_kind: "app" },
  ],
};

let rootShellLaunchCount = 0;

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
  EventSource: FakeEventSource,
  crypto: { randomUUID: () => "home-shell-post-attach-recovery-smoke" },
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
  setTimeout(callback, delay = 0) {
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
    return jsonResponse(summary);
  }
  if (url === "/api/auth/sessions/refresh") {
    return jsonResponse({ home_token: "host-token" });
  }
  if (url === "/api/apps/home/active-shell") {
    assert(
      init.headers?.["x-elastos-home-token"] === "host-token",
      "post-attach recovery did not use the trusted Home host token",
      init.headers,
    );
    summary.active_shell.active = body?.active || summary.active_shell.active;
    return jsonResponse({ active: summary.active_shell.active });
  }
  if (url === "/api/apps/home/launch") {
    if (body?.target === "home-gui") {
      rootShellLaunchCount += 1;
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
    assert(body?.target === "home-cli", "recovery smoke launched the wrong root shell", body);
    rootShellLaunchCount += 1;
    return jsonResponse({
      attach_kind: "iframe",
      route: "/apps/home-cli/?shell_mode=root&home_origin=http%3A%2F%2Flocalhost%3A61180#home_token=root-token",
      target: "home-cli",
    });
  }
  return jsonResponse({ ok: true });
};

const hostCore = await import(`../capsules/home/browser/shell-core.js?v=${moduleVersion}`);
await import(`../capsules/home/browser/home-shell-host.js?v=${moduleVersion}&post-attach-recovery`);
await waitFor(
  () => Boolean(elementForSelector("#active-shell-frame").dataset.route),
  "setup did not launch the initial root shell",
  () => ({ requests }),
);

const activeShellRoot = elementForSelector("#active-shell-root");
const activeShellFrame = elementForSelector("#active-shell-frame");
const bootMask = elementForSelector("#home-shell-boot-mask");
const recovery = elementForSelector("#shell-host-recovery");
const recoveryTitle = elementForSelector("#shell-host-recovery-title");
const recoveryHome = elementForSelector("#shell-host-recovery-home");
const guiFrameWindow = { postMessage() {} };
activeShellFrame.contentWindow = guiFrameWindow;

dispatchMessage(guiFrameWindow, {
  type: "home:active-shell-applied",
  activeShell: "home-cli",
  homeToken: "gui-token",
});
await waitFor(
  () => activeShellFrame.dataset.route.includes("/apps/home-cli/"),
  "post-attach recovery smoke did not relaunch Home CLI",
  () => ({ requests, dataset: activeShellFrame.dataset }),
);
const cliFrameWindow = { postMessage() {} };
activeShellFrame.contentWindow = cliFrameWindow;

assert(activeShellRoot.dataset.target === "home-cli", "post-attach recovery target drifted", activeShellRoot.dataset);
assert(activeShellFrame.hidden === true, "post-attach recovery revealed the shell before readiness");
assert(bootMask.hidden === false, "post-attach recovery hid the neutral boot mask before readiness");
assert(recovery.hidden === true, "post-attach recovery showed recovery before readiness timed out");

const timeoutCallback = [...pendingTimers.values()][0];
assert(typeof timeoutCallback === "function", "post-attach recovery did not arm the shell-ready timeout", {
  timers: pendingTimers.size,
});
timeoutCallback();
await flush(2);

assert(recovery.hidden === false, "post-attach recovery did not show recovery after the shell-ready timeout");
assert(recovery.dataset.host === "home-shell-host", "post-attach recovery host ownership drifted", recovery.dataset);
assert(recovery.dataset.target === "home-cli", "post-attach recovery target changed", recovery.dataset);
assert(recoveryTitle.textContent.includes("Terminal"), "post-attach recovery did not name the stalled shell", recoveryTitle.textContent);
assert(activeShellFrame.hidden === true, "post-attach recovery left the stalled shell frame visible");
assert(bootMask.hidden === true, "post-attach recovery left the neutral boot mask over recovery");
assert(recoveryHome.disabled === false, "post-attach recovery hid Desktop recovery despite the mounted shell token");

const stalledRoute = activeShellFrame.dataset.route;
const launchCountBeforeRefresh = rootShellLaunchCount;
summary.active_shell.active = "home-cli";
await hostCore.shellState.requestSummaryRefresh();
await flush(2);
assert(recovery.hidden === false, "post-attach recovery hid recovery after summary catch-up");
assert(activeShellFrame.hidden === true, "post-attach recovery revealed the stalled frame after summary catch-up");
assert(bootMask.hidden === true, "post-attach recovery revived the neutral boot mask after summary catch-up");
assert(activeShellFrame.dataset.route === stalledRoute, "post-attach recovery changed the stalled route during summary catch-up", {
  before: stalledRoute,
  after: activeShellFrame.dataset.route,
});
assert(rootShellLaunchCount === launchCountBeforeRefresh, "post-attach recovery relaunched the stalled shell during summary catch-up", {
  before: launchCountBeforeRefresh,
  after: rootShellLaunchCount,
  requests,
});

dispatchMessage(cliFrameWindow, {
  type: "home:shell-ready",
  homeToken: "root-token",
});
await waitFor(
  () => activeShellFrame.hidden === false && recovery.hidden === true && bootMask.hidden === true,
  "post-attach recovery did not settle after a late valid shell-ready message",
  () => ({ requests, route: activeShellFrame.dataset.route }),
);

click(recoveryHome);
await waitFor(
  () => requests.some((request) => request.url === "/api/apps/home/active-shell" && request.body?.active === "home-gui"),
  "post-attach recovery did not drive the explicit Desktop recovery path",
  () => ({ requests }),
);

console.log("[home-shell-post-attach-recovery] PASS");
