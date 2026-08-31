#!/usr/bin/env node

const moduleVersion = "home-20260802a";
const requests = [];
const windowListeners = new Map();
const intervals = new Map();
let nextIntervalId = 1;
let signedSummary = false;
let resolvePresenceHeartbeat = null;
let presenceResponseMode = "pending";
let credentialGetCount = 0;

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

  click() {
    for (const callback of this.listeners.get("click") || []) {
      callback({ currentTarget: this, preventDefault() {}, stopPropagation() {} });
    }
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
  value: {
    credentials: {
      async get() {
        credentialGetCount += 1;
        return null;
      },
    },
  },
});
globalThis.window = {
  PublicKeyCredential: function PublicKeyCredential() {},
  atob: (value) => Buffer.from(String(value), "base64").toString("binary"),
  btoa: (value) => Buffer.from(String(value), "binary").toString("base64"),
  crypto: { randomUUID: () => "home-shell-auth-gate-smoke" },
  location: { href: "http://localhost:61180/apps/home/", origin: "http://localhost:61180" },
  localStorage: { getItem: () => null, removeItem() {}, setItem() {} },
  matchMedia: () => ({ matches: false }),
  performance: { now: () => Date.now() },
  innerWidth: 1280,
  addEventListener(type, callback) {
    if (!windowListeners.has(type)) {
      windowListeners.set(type, []);
    }
    windowListeners.get(type).push(callback);
  },
  clearInterval(id) {
    intervals.delete(id);
  },
  clearTimeout(id) {
    if (id) clearImmediate(id);
  },
  setInterval(callback, delay) {
    const id = nextIntervalId++;
    intervals.set(id, { callback, delay });
    return id;
  },
  setTimeout(callback) {
    if (typeof callback === "function" && callback.name !== "pollHomeEvents") {
      return setImmediate(callback);
    }
    return 0;
  },
};
globalThis.window.navigator = globalThis.navigator;

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
      authority: signedSummary
        ? { signed_in: true, proof_binding_id: "proof:passkey:host" }
        : { signed_in: false },
      identity: signedSummary
        ? {
            profile: {
              display_name: "Verified Person",
            },
          }
        : undefined,
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
  if (url === "/api/auth/passkey/authenticate/begin") {
    return jsonResponse({
      ceremony_id: "auth-gate-passkey",
      options: {
        publicKey: {
          challenge: "AQ",
          timeout: 60000,
          rpId: "localhost",
          allowCredentials: [],
          userVerification: "preferred",
        },
      },
    });
  }
  if (url === "/api/auth/sessions/refresh") {
    return jsonResponse({ home_token: "trusted-home-host-token" });
  }
  if (url === "/api/apps/home/collaboration/presence") {
    if (presenceResponseMode === "auth-failure") {
      return failedResponse(401, "Unauthorized", "expired Home authority");
    }
    if (presenceResponseMode === "success") {
      return jsonResponse({
        configured: true,
        queued: true,
        next_heartbeat_after_ms: 15_000,
      });
    }
    return new Promise((resolve) => {
      resolvePresenceHeartbeat = () => resolve(jsonResponse({
        configured: true,
        queued: true,
        next_heartbeat_after_ms: 15_000,
      }));
    });
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
assert(unlock.dataset.surface === "lock-face", "auth gate did not show the lock face", unlock.dataset);
assert(
  elementForSelector("#home-shell-boot-mask").hidden === true,
  "auth gate left the neutral host mask over the passkey prompt",
);
assert(
  elementForSelector(".home-unlock-face").hidden === false,
  "auth gate did not show the lock face content",
);
assert(
  elementForSelector(".home-unlock-card").hidden === true,
  "auth gate left the neutral card visible for a registered lock face",
);
assert(
  elementForSelector("#home-unlock-person-name").textContent === "",
  "unsigned Home leaked a profile name into the lock face",
  elementForSelector("#home-unlock-person-name").textContent,
);
assert(
  requests.filter((request) => request.url === "/api/auth/passkey/authenticate/begin").length === 0,
  "auth gate started passkey sign-in without an explicit click",
  requests,
);
assert(document.body.dataset.homeStatus === "ready", "auth gate prompt did not leave Home ready for passkey input", document.body.dataset);
assert(document.body.dataset.homeShell === "resolving", "auth gate left a root shell visible", document.body.dataset);
assert(document.body.dataset.homeGui === "dormant", "auth gate left Home GUI mounted", document.body.dataset);
assert(activeShellRoot.hidden === true, "auth gate left the active shell root visible");
assert(activeShellRoot.dataset.target === "", "auth gate kept a stale active shell target", activeShellRoot.dataset);
assert(activeShellFrame.hidden === true, "auth gate left the active shell frame visible");
assert(!activeShellFrame.dataset.route, "auth gate kept a stale active shell route", activeShellFrame.dataset);
assert(activeShellFrame.src === "about:blank", "auth gate did not unload the stale shell iframe", activeShellFrame.src);
assert(!requests.some((request) => request.url === "/api/apps/home/active-shell"), "auth gate tried to switch shells without a token", requests);
assert(!requests.some((request) => request.url === "/api/apps/home/launch"), "auth gate tried to launch a shell while locked", requests);

elementForSelector("#home-unlock-person").click();
for (
  let attempt = 0;
  attempt < 20 && credentialGetCount < 1;
  attempt += 1
) {
  await new Promise((resolve) => setTimeout(resolve, 0));
}
assert(
  requests.filter((request) => request.url === "/api/auth/passkey/authenticate/begin").length === 1,
  "auth gate did not start passkey sign-in after the explicit lock-face click",
  requests,
);
assert(
  credentialGetCount === 1,
  "auth gate did not reach navigator.credentials.get after the explicit lock-face click",
);

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

assert(
  !requests.some((request) => request.url === "/api/apps/home/collaboration/presence"),
  "unsigned Home published collaboration presence",
  requests,
);
signedSummary = true;
for (const listener of windowListeners.get("message") || []) {
  listener({
    origin: "http://localhost:61180",
    source: window,
    data: { type: "home:refresh-summary" },
  });
}
for (
  let attempt = 0;
  attempt < 20 && !requests.some((request) => request.url === "/api/apps/home/collaboration/presence");
  attempt += 1
) {
  await new Promise((resolve) => setTimeout(resolve, 0));
}
const heartbeatRequests = () => requests.filter(
  (request) => request.url === "/api/apps/home/collaboration/presence",
);
assert(heartbeatRequests().length === 1, "proof-bound Home did not emit one presence heartbeat", requests);
assert(
  elementForSelector("#home-unlock-person-name").textContent === "",
  "summary refresh leaked a signed profile name into an unsigned lock gate",
  elementForSelector("#home-unlock-person-name").textContent,
);
assert(
  JSON.stringify(heartbeatRequests()[0].body) === "{}",
  "Home presence heartbeat forwarded authority fields",
  heartbeatRequests()[0],
);
assert(
  heartbeatRequests()[0].headers["x-elastos-home-token"] === "trusted-home-host-token",
  "Home presence heartbeat did not carry the trusted host token",
  heartbeatRequests()[0],
);
const presenceIntervals = [...intervals.values()].filter(({ delay }) => delay === 15_000);
assert(presenceIntervals.length === 1, "Home created more than one presence timer", presenceIntervals);
presenceIntervals[0].callback();
presenceIntervals[0].callback();
assert(heartbeatRequests().length === 1, "in-flight presence heartbeat was not coalesced", requests);
resolvePresenceHeartbeat();
await new Promise((resolve) => setTimeout(resolve, 0));

presenceResponseMode = "auth-failure";
presenceIntervals[0].callback();
await new Promise((resolve) => setTimeout(resolve, 0));
assert(heartbeatRequests().length === 2, "Home did not make the next normal heartbeat", requests);
assert(
  ![...intervals.values()].some(({ delay }) => delay === 15_000),
  "Home kept retrying presence after an authorization failure",
  [...intervals.values()],
);

presenceResponseMode = "success";
const summaryCountBeforeRestart = requests.filter(
  (request) => request.url === "/api/apps/home/summary",
).length;
for (const listener of windowListeners.get("message") || []) {
  listener({
    origin: "http://localhost:61180",
    source: window,
    data: { type: "home:refresh-summary" },
  });
}
for (
  let attempt = 0;
  attempt < 20 && heartbeatRequests().length < 3;
  attempt += 1
) {
  await new Promise((resolve) => setTimeout(resolve, 0));
}
assert(heartbeatRequests().length === 3, "later proof-bound summary did not restart presence", requests);
assert(
  [...intervals.values()].filter(({ delay }) => delay === 15_000).length === 1,
  "proof-bound restart did not retain exactly one presence timer",
  [...intervals.values()],
);

signedSummary = false;
for (const listener of windowListeners.get("message") || []) {
  listener({
    origin: "http://localhost:61180",
    source: window,
    data: { type: "home:refresh-summary" },
  });
}
for (
  let attempt = 0;
  attempt < 20 && requests.filter((request) => request.url === "/api/apps/home/summary").length < summaryCountBeforeRestart + 2;
  attempt += 1
) {
  await new Promise((resolve) => setTimeout(resolve, 0));
}
assert(
  ![...intervals.values()].some(({ delay }) => delay === 15_000),
  "Home kept the presence timer after becoming unsigned",
  [...intervals.values()],
);
presenceIntervals[0].callback();
assert(heartbeatRequests().length === 3, "unsigned Home emitted from a stale presence timer", requests);

const { profileReadinessActionTarget } = await import(
  `../capsules/home/browser/shell-auth.js?v=${moduleVersion}`
);
const { showHomeUnlock } = await import(
  `../capsules/home/browser/shell-auth.js?v=${moduleVersion}`
);
await showHomeUnlock(() => {}, {
  presentation: "prompt",
  personName: "Verified Person",
});
assert(
  elementForSelector("#home-unlock-person-name").textContent === "Verified Person",
  "lock face did not render an explicit signed-session profile label",
  elementForSelector("#home-unlock-person-name").textContent,
);
assert(profileReadinessActionTarget({
  profile_readiness: {
    schema: "elastos.profile.readiness/v1",
    status: "setup_required",
  },
}) === "people", "Home did not direct explicit Profile setup to People");
assert(profileReadinessActionTarget({
  profile_readiness: {
    schema: "elastos.profile.readiness/v1",
    status: "ready",
  },
}) === "", "Home treated a ready Profile as action-required");
assert(profileReadinessActionTarget({
  profile_readiness: {
    schema: "elastos.profile.readiness/v1",
    status: "unavailable",
  },
}) === "system", "Home did not route invalid Profile authority to System Recovery");
assert(profileReadinessActionTarget({}) === "system", "Home silently accepted missing Profile readiness");
assert(profileReadinessActionTarget({
  profile_readiness: {
    schema: "elastos.profile.readiness/unknown",
    status: "ready",
  },
}) === "system", "Home silently accepted an unknown Profile readiness schema");
assert(profileReadinessActionTarget({
  profile_readiness: {
    schema: "elastos.profile.readiness/v1",
    status: "unknown",
  },
}) === "system", "Home silently accepted an unknown Profile readiness status");

console.log("[home-shell-auth-gate] PASS");
