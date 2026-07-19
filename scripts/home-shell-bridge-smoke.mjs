#!/usr/bin/env node

const moduleVersion = "home-20260719e";
const requests = [];
const windowListeners = new Map();
const localStorageValues = new Map([
  ["elastos.home.active-shell-hint", "home-cli"],
]);

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
    this.queries = new Map();
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
    if (!this.queries.has(selector)) {
      this.queries.set(selector, new FakeElement(`${this.selector} ${selector}`));
    }
    return this.queries.get(selector);
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
    principal_id: "principal:home-shell-bridge",
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
    { target: "inbox", title: "Inbox", attach_kind: "iframe", role: "app", target_kind: "app" },
    { target: "system", title: "System", attach_kind: "iframe", role: "app", target_kind: "app" },
  ],
};
let activeShellName = "home-cli";
let passkeyCompleted = false;

function currentSummary() {
  return {
    ...summary,
    active_shell: {
      ...summary.active_shell,
      active: activeShellName,
    },
  };
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
  value: {
    credentials: {
      get: async () => ({
        id: "credential-id",
        rawId: new Uint8Array([1]).buffer,
        type: "public-key",
        response: {
          authenticatorData: new Uint8Array([2]).buffer,
          clientDataJSON: new Uint8Array([3]).buffer,
          signature: new Uint8Array([4]).buffer,
          userHandle: null,
        },
      }),
    },
  },
});
globalThis.window = {
  PublicKeyCredential: function PublicKeyCredential() {},
  atob: globalThis.atob,
  btoa: globalThis.btoa,
  crypto: { randomUUID: () => "home-shell-bridge-smoke" },
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
    if (requests.filter((request) => request.url === "/api/apps/home/summary").length === 1) {
      assert(
        document.documentElement.dataset.homeShellBoot === "alternate",
        "alternate shell boot hint was not applied before the summary request",
        document.documentElement.dataset,
      );
      assert(
        document.body.dataset.homeShell === "alternate" && document.body.dataset.homeGui === "dormant",
        "alternate shell did not claim the first paint before the summary request",
        document.body.dataset,
      );
      assert(
        elementForSelector("#active-shell-root").hidden === false &&
          elementForSelector("#active-shell-root").dataset.target === "home-cli",
        "alternate shell root was not visible before the summary request",
        elementForSelector("#active-shell-root").dataset,
      );
      assert(
        elementForSelector("#active-shell-frame").hidden === true &&
          !elementForSelector("#active-shell-frame").getAttribute("src"),
        "alternate shell boot hint loaded a frame before Runtime launch authority",
        elementForSelector("#active-shell-frame").dataset,
      );
    }
    return jsonResponse(currentSummary());
  }
  if (url === "/api/auth/sessions/refresh") {
    return jsonResponse({ home_token: "host-token" });
  }
  if (url === "/api/auth/passkey/authenticate/begin") {
    return jsonResponse({
      ceremony_id: "ceremony-id",
      options: { publicKey: { challenge: "AQ", allowCredentials: [] } },
    });
  }
  if (url === "/api/auth/passkey/authenticate/complete") {
    passkeyCompleted = true;
    return jsonResponse({ home_token: "fresh-passkey-token" });
  }
  if (url === "/api/apps/home/active-shell") {
    assert(body?.active === "home-gui", "root shell app-open must switch back to home-gui", body);
    assert(
      init.headers?.["x-elastos-home-token"] === "root-token",
      "home-gui switchback did not use the shell launch token",
      init.headers,
    );
    activeShellName = "home-gui";
    return jsonResponse({ active: "home-gui" });
  }
  if (url === "/api/apps/home/launch") {
    const expectedToken = body?.authority ? "fresh-passkey-token" : "host-token";
    assert(
      init.headers?.["x-elastos-home-token"] === expectedToken,
      "Home launch did not use explicit host-held authority",
      init.headers,
    );
    if (body?.target === "home-cli") {
      assert(body?.query?.shell_mode === "root", "alternate shell must launch in root mode", body);
      return jsonResponse({
        attach_kind: "iframe",
        route: "/apps/home-cli/?shell_mode=root&home_origin=http%3A%2F%2Flocalhost%3A61180#home_token=root-token",
        target: "home-cli",
      });
    }
    if (body?.target === "home-gui") {
      assert(body?.query?.shell_mode === "root", "GUI shell must launch in root mode", body);
      return jsonResponse({
        attach_kind: "iframe",
        route: "/apps/home-gui/?shell_mode=root&home_origin=http%3A%2F%2Flocalhost%3A61180#home_token=gui-token",
        target: "home-gui",
      });
    }
    if (body?.target === "browser") {
      return jsonResponse({
        attach_kind: "iframe",
        route: "/apps/browser/?home_origin=http%3A%2F%2Flocalhost%3A61180#home_token=browser-token",
        target: "browser",
        title: "Browser",
      });
    }
    if (body?.target === "system") {
      const token = body?.authority ? "system-fresh-token" : "system-token";
      return jsonResponse({
        attach_kind: "iframe",
        route: `/apps/system/?home_origin=http%3A%2F%2Flocalhost%3A61180#home_token=${token}`,
        target: "system",
        title: "System",
      });
    }
    throw new Error(`unexpected launch target: ${body?.target || "missing"}`);
  }
  return jsonResponse({ ok: true });
};

const hostCore = await import(`../capsules/home/browser/shell-core.js?v=${moduleVersion}`);

await import(`../capsules/home/browser/home-shell-host.js?v=${moduleVersion}`);
for (let attempt = 0; attempt < 20 && !elementForSelector("#active-shell-frame").dataset.route; attempt += 1) {
  await new Promise((resolve) => setTimeout(resolve, 0));
}

const body = document.body;
const activeShellRoot = elementForSelector("#active-shell-root");
const activeShellFrame = elementForSelector("#active-shell-frame");
const shellHostRecovery = elementForSelector("#shell-host-recovery");
const launchRequest = requests.find((request) => request.url === "/api/apps/home/launch");

assert(body.dataset.homeShell === "alternate", "alternate shell did not take root mode", body.dataset);
assert(body.dataset.homeGui === "dormant", "Home GUI was not marked dormant", body.dataset);
assert(
  document.documentElement.dataset.homeShellHint === "alternate",
  "alternate shell boot hint was not applied before summary",
  document.documentElement.dataset,
);
assert(
  localStorageValues.get("elastos.home.active-shell-hint") === "home-cli",
  "alternate shell hint was not refreshed from Runtime summary",
  Object.fromEntries(localStorageValues),
);
assert(activeShellRoot.hidden === false, "active shell root stayed hidden");
assert(activeShellRoot.dataset.target === "home-cli", "active shell root target drifted", activeShellRoot.dataset);
assert(activeShellFrame.hidden === false, "active shell frame stayed hidden");
assert(activeShellFrame.dataset.route.includes("/apps/home-cli/"), "active shell frame did not load Home CLI", activeShellFrame.dataset);
assert(activeShellFrame.dataset.route.includes("#home_token=root-token"), "active shell launch token was not carried in the frame fragment", activeShellFrame.dataset);
assert(!activeShellFrame.dataset.route.includes("?home_token="), "active shell authority leaked into the query string", activeShellFrame.dataset);
assert(shellHostRecovery.hidden === true, "host recovery panel showed during healthy shell launch");
assert(launchRequest, "alternate shell launch request was not made", requests);
assert(!requests.some((request) => request.url === "/api/apps/home/active-shell"), "bridge smoke should not switch shell using ambient state", requests);

const shellMessages = [];
const shellFrameWindow = {
  postMessage(payload, origin) {
    shellMessages.push({ origin, payload });
  },
};
activeShellFrame.contentWindow = shellFrameWindow;
const launchRequestsBeforeInvalidMessages = requests.filter(
  (request) => request.url === "/api/apps/home/launch",
).length;
for (const data of [
  {
    origin: "http://evil.invalid",
    type: "home:open-target",
    target: "browser",
    homeToken: "root-token",
  },
  {
    origin: "null",
    type: "home:open-target",
    target: "browser",
    homeToken: "wrong-token",
  },
  {
    origin: "null",
    type: "home:open-target",
    target: "home",
    homeToken: "root-token",
  },
  {
    origin: "null",
    type: "home:sign-out",
    homeToken: "wrong-token",
  },
]) {
  for (const listener of windowListeners.get("message") || []) {
    listener({
      origin: data.origin,
      source: shellFrameWindow,
      data,
    });
  }
}
for (let attempt = 0; attempt < 5; attempt += 1) {
  await new Promise((resolve) => setTimeout(resolve, 0));
}
assert(
  requests.filter((request) => request.url === "/api/apps/home/launch").length ===
    launchRequestsBeforeInvalidMessages,
  "unauthorized shell messages launched a target",
  requests,
);
assert(
  !requests.some((request) => request.url === "/api/apps/home/active-shell"),
  "unauthorized shell messages switched active shell",
  requests,
);
assert(
  !requests.some((request) => request.url === "/api/auth/sessions/sign-out"),
  "unauthorized shell message signed out Home",
  requests,
);
for (const listener of windowListeners.get("message") || []) {
  listener({
    origin: "null",
    source: shellFrameWindow,
    data: {
      type: "home:launch-target",
      requestId: "cli-launch-browser",
      target: "browser",
      query: {},
      homeToken: "root-token",
    },
  });
}
for (
  let attempt = 0;
  attempt < 20 && !shellMessages.some((message) => message.payload?.requestId === "cli-launch-browser");
  attempt += 1
) {
  await new Promise((resolve) => setTimeout(resolve, 0));
}
assert(
  shellMessages.some((message) =>
    message.origin === "*" &&
    message.payload?.type === "home:shell-response" &&
    message.payload?.requestId === "cli-launch-browser" &&
    message.payload?.result?.route?.includes("#home_token=browser-token")
  ),
  "Home CLI did not receive the same Runtime-scoped launch result as Home GUI",
  shellMessages,
);
assert(
  !requests.some((request) => request.url === "/api/apps/home/active-shell"),
  "Home CLI launch request switched shells implicitly",
  requests,
);
for (const listener of windowListeners.get("message") || []) {
  listener({
    origin: "null",
    source: shellFrameWindow,
    data: {
      type: "home:open-target",
      target: "browser",
      homeToken: "root-token",
    },
  });
}
await new Promise((resolve) => setTimeout(resolve, 0));
assert(
  !requests.some((request) => request.url === "/api/apps/home/active-shell"),
  "ordinary CLI app intent switched Home GUI implicitly",
  requests,
);
for (const listener of windowListeners.get("message") || []) {
  listener({
    origin: "null",
    source: shellFrameWindow,
    data: {
      type: "home:switch-shell-and-open-target",
      requestId: "explicit-gui-browser",
      target: "browser",
      homeToken: "root-token",
    },
  });
}
for (let attempt = 0; attempt < 20 && activeShellFrame.dataset.route.includes("home-cli"); attempt += 1) {
  await new Promise((resolve) => setTimeout(resolve, 0));
}
assert(body.dataset.homeShell === "desktop", "explicit graphical action did not switch to desktop shell mode", body.dataset);
assert(body.dataset.homeGui === "mounted", "explicit graphical action did not mount Home GUI", body.dataset);
assert(activeShellRoot.hidden === false, "explicit graphical action hid the active shell root");
assert(activeShellRoot.dataset.target === "home-gui", "explicit graphical action did not select isolated Home GUI", activeShellRoot.dataset);
assert(activeShellFrame.dataset.route.includes("/apps/home-gui/"), "explicit graphical action did not launch isolated Home GUI", activeShellFrame.dataset);
assert(activeShellFrame.dataset.route.includes("#home_token=gui-token"), "isolated Home GUI did not receive fragment authority", activeShellFrame.dataset);
assert(
  hostCore.shellState.activeShellRootTarget === "home-gui",
  "explicit graphical action did not replace the alternate root shell",
  hostCore.shellState,
);
assert(
  requests.some((request) => request.url === "/api/apps/home/active-shell"),
  "explicit graphical action did not switch active shell with the shell token",
  requests,
);
assert(
  shellMessages.some((message) => message.payload?.type === "home:gui-command" && message.payload?.target === "browser"),
  "explicit graphical action did not hand Browser intent to isolated Home GUI",
  shellMessages,
);

function sendChildMessage(origin, source, data) {
  for (const listener of windowListeners.get("message") || []) {
    listener({ origin, source, data });
  }
}

sendChildMessage("null", shellFrameWindow, {
  type: "home:shell-ready",
  homeToken: "gui-token",
});
sendChildMessage("null", shellFrameWindow, {
  type: "home:launch-target",
  requestId: "launch-browser",
  target: "browser",
  query: {},
  homeToken: "gui-token",
});
for (let attempt = 0; attempt < 20 && !shellMessages.some((message) => message.payload?.requestId === "launch-browser"); attempt += 1) {
  await new Promise((resolve) => setTimeout(resolve, 0));
}
assert(
  requests.some((request) => request.url === "/api/apps/home/launch" && request.body?.target === "browser"),
  "isolated Home GUI could not ask Home to launch Browser",
  requests,
);
assert(
  shellMessages.some((message) =>
    message.origin === "*" &&
    message.payload?.type === "home:shell-response" &&
    message.payload?.requestId === "launch-browser" &&
    message.payload?.result?.route?.includes("#home_token=browser-token")
  ),
  "Home did not return the isolated Browser route to Home GUI",
  shellMessages,
);

const browserFrameWindow = { postMessage() {} };
sendChildMessage("null", browserFrameWindow, {
  type: "home:app-ready",
  homeToken: "browser-token",
});
const passkeyRequestsBeforeBrowser = requests.filter(
  (request) => request.url === "/api/auth/passkey/authenticate/begin",
).length;
sendChildMessage("null", browserFrameWindow, {
  type: "home:request-passkey-authority",
  requestId: "browser-request",
  homeToken: "browser-token",
  operation: "browser.profile.delete",
  request: { profile: "default" },
});
await new Promise((resolve) => setTimeout(resolve, 0));
assert(
  requests.filter((request) => request.url === "/api/auth/passkey/authenticate/begin").length ===
    passkeyRequestsBeforeBrowser,
  "Browser frame obtained host passkey authority",
  requests,
);

sendChildMessage("null", shellFrameWindow, {
  type: "home:launch-target",
  requestId: "launch-system",
  target: "system",
  query: {},
  homeToken: "gui-token",
});
for (let attempt = 0; attempt < 20 && !shellMessages.some((message) => message.payload?.requestId === "launch-system"); attempt += 1) {
  await new Promise((resolve) => setTimeout(resolve, 0));
}
assert(
  shellMessages.some((message) =>
    message.payload?.requestId === "launch-system" &&
    message.payload?.result?.route?.includes("#home_token=system-token")
  ),
  "Home did not return an isolated System launch",
  shellMessages,
);

let passkeyReply = null;
const systemFrameWindow = {
  postMessage(payload, origin) {
    passkeyReply = { origin, payload };
  },
};
sendChildMessage("null", systemFrameWindow, {
  type: "home:app-ready",
  homeToken: "system-token",
});
sendChildMessage("null", systemFrameWindow, {
  type: "home:request-passkey-authority",
  requestId: "system-request",
  homeToken: "system-token",
  operation: "auth.full-recovery-bundle.export",
  request: { label: "Recovery Kit" },
});
for (let attempt = 0; attempt < 20 && !passkeyReply; attempt += 1) {
  await new Promise((resolve) => setTimeout(resolve, 0));
}
assert(
  passkeyReply?.payload?.homeToken === "system-fresh-token" &&
    passkeyReply?.payload?.requestId === "system-request" &&
    passkeyReply?.origin === "*",
  "validated System frame did not receive capsule-scoped passkey proof",
  passkeyReply,
);

console.log("[home-shell-bridge] PASS");
