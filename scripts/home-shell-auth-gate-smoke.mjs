#!/usr/bin/env node

const moduleVersion = "home-update-20261003a";
const requests = [];
const windowListeners = new Map();
const documentListeners = new Map();
const intervals = new Map();
const eventPollTimers = new Map();
const requestDeadlineTimers = new Map();
const requestDeadlines = [];
const nativeAbortAny = AbortSignal.any;
const nativeAbortTimeout = AbortSignal.timeout;
AbortSignal.any = undefined;
AbortSignal.timeout = undefined;
const eventSources = [];
let nextIntervalId = 1;
let nextEventPollTimerId = 10_000;
let signedSummary = false;
let resolvePresenceHeartbeat = null;
let presenceResponseMode = "pending";
let credentialGetCount = 0;
const credentialRequests = [];
let credentialFailure = null;
let sessionResponseMode = "success";
let resolveSessionRefresh = null;
let eventsResponseMode = "success";
let resolveEventsPoll = null;
let reconnectSummary = false;
let runtimeVersion = "before-restart";

class FakeEventSource {
  constructor(url, options) {
    this.url = url;
    this.options = options;
    this.listeners = new Map();
    this.closed = false;
    eventSources.push(this);
  }

  addEventListener(type, callback) {
    this.listeners.set(type, callback);
  }

  close() {
    this.closed = true;
  }

  emit(type, data = {}) {
    this.listeners.get(type)?.(data);
  }
}

// The guest-enabled variant runs in a child process (see the end of this file)
// because the shell modules bind to this fake DOM once per process.
const guestRegistrationEnabled = process.env.HOME_AUTH_GATE_GUEST_ENABLED === "1";

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
  getElementById: id => elementForSelector(`#${id}`),
  activeElement: null,
  body: elementForSelector("body"),
  documentElement: elementForSelector("html"),
  addEventListener(type, callback) {
    if (!documentListeners.has(type)) documentListeners.set(type, []);
    documentListeners.get(type).push(callback);
  },
  createElement: (tag) => new FakeElement(tag),
  querySelector: elementForSelector,
  querySelectorAll: () => [],
};
Object.defineProperty(globalThis, "navigator", {
  configurable: true,
  value: {
    credentials: {
      async get(request) {
        credentialRequests.push(request);
        credentialGetCount += 1;
        if (credentialFailure) throw credentialFailure;
        return null;
      },
    },
  },
});
globalThis.window = {
  sessionStorage: { getItem: () => null },
  PublicKeyCredential: function PublicKeyCredential() {},
  atob: (value) => Buffer.from(String(value), "base64").toString("binary"),
  btoa: (value) => Buffer.from(String(value), "binary").toString("base64"),
  crypto: { randomUUID: () => "home-shell-auth-gate-smoke" },
  location: { href: "http://localhost:61180/apps/home/", origin: "http://localhost:61180", hostname: "localhost" },
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
    if (eventPollTimers.delete(id)) return;
    if (requestDeadlineTimers.delete(id)) return;
    if (id) clearImmediate(id);
  },
  setInterval(callback, delay) {
    const id = nextIntervalId++;
    intervals.set(id, { callback, delay });
    return id;
  },
  setTimeout(callback, delay) {
    if (delay === 10_000 || delay === 35_000) {
      const id = nextEventPollTimerId++;
      requestDeadlineTimers.set(id, { callback, delay });
      requestDeadlines.push(delay);
      return id;
    }
    if (callback?.name === "pollHomeEvents") {
      const id = nextEventPollTimerId++;
      eventPollTimers.set(id, { callback, delay });
      return id;
    }
    if (typeof callback === "function") {
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
    signal: init.signal,
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
        candidates: reconnectSummary
          ? [{ name: "home-gui", launchable: true, title: "Home GUI" }]
          : [],
      },
      runtime: { version: runtimeVersion },
      targets: reconnectSummary ? [{ target: "system" }] : [],
    });
  }
  if (url === "/api/auth/passkey/status") {
    return jsonResponse({ registered: true, guest_registration_enabled: guestRegistrationEnabled });
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
    if (sessionResponseMode === "network-error") throw new TypeError("Failed to fetch");
    if (sessionResponseMode === "timed-out") throw new DOMException("Request timed out", "TimeoutError");
    if (sessionResponseMode === "unavailable") return failedResponse(503, "Unavailable", "Runtime restarting");
    if (sessionResponseMode === "unauthorized") return failedResponse(401, "Unauthorized", "expired session");
    if (sessionResponseMode === "forbidden") return failedResponse(403, "Forbidden", "revoked session");
    if (sessionResponseMode === "pending") {
      return new Promise((resolve, reject) => {
        resolveSessionRefresh = () => resolve(jsonResponse({ home_token: "trusted-home-host-token" }));
        init.signal?.addEventListener("abort", () => reject(init.signal.reason), { once: true });
      });
    }
    return jsonResponse({ home_token: "trusted-home-host-token" });
  }
  if (url === "/api/apps/home/launch" && reconnectSummary) {
    const target = JSON.parse(init.body).target;
    return jsonResponse({
      target,
      attach_kind: "iframe",
      route: target === "system"
        ? "/apps/system/?home_origin=http%3A%2F%2Flocalhost%3A61180#home_token=surviving-system-token"
        : "/apps/home-gui/#home_token=surviving-shell-token",
    });
  }
  if (String(url).startsWith("/api/apps/home/events?")) {
    if (eventsResponseMode === "network-error") throw new TypeError("Runtime restarting");
    if (eventsResponseMode === "unauthorized") return failedResponse(401, "Unauthorized", "expired session");
    const payload = { schema: "elastos.home.events/v1", cursor: runtimeVersion, events: [], retry_after_ms: 2_000 };
    if (eventsResponseMode === "pending") {
      return new Promise((resolve, reject) => {
        resolveEventsPoll = () => resolve(jsonResponse(payload));
        init.signal?.addEventListener("abort", () => reject(init.signal.reason), { once: true });
      });
    }
    return jsonResponse(payload);
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

const createAccount = elementForSelector("#home-unlock-create");
if (guestRegistrationEnabled) {
  assert(createAccount.hidden === false, "lock face hid Create account while guest registration is on");
  createAccount.click();
  for (let attempt = 0; attempt < 20 && elementForSelector(".home-unlock-card").hidden; attempt += 1) {
    await new Promise((resolve) => setTimeout(resolve, 0));
  }
  assert(
    elementForSelector("#home-unlock-title").textContent === "Create guest account",
    "Create account did not open guest enrollment",
    elementForSelector("#home-unlock-title").textContent,
  );
  assert(elementForSelector(".home-unlock-card").hidden === false, "Create account did not show the enrollment card");
  assert(elementForSelector(".home-unlock-face").hidden === true, "Create account left the lock face over the enrollment card");
  assert(createAccount.hidden === true, "Create account stayed visible during guest enrollment");
  assert(
    requests.filter((request) => request.url === "/api/auth/passkey/authenticate/begin").length === 0,
    "Create account started passkey sign-in instead of enrollment",
    requests,
  );
  console.log("[home-shell-auth-gate] PASS (guest registration on)");
  process.exit(0);
}
assert(createAccount.hidden === true, "lock face offered Create account while guest registration is off");

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

assert(credentialRequests[0].publicKey.allowCredentials.length === 0, "ordinary sign-in changed discovery");
assert(elementForSelector("#home-older-key-action").hidden === false,
  "older key action is missing with guest registration disabled");
const pause = () => new Promise(resolve => setTimeout(resolve, 0));
const hintInput = elementForSelector("#home-passkey-hint");
const hint = { schema: "elastos.passkey.hint/v1", credential_id: "AQID", rp_id: "localhost" };
elementForSelector("#home-older-key-action").click();
assert(elementForSelector(".home-unlock-card").hidden === false, "older key input stays hidden behind lock face");
assert(elementForSelector("#home-passkey-hint-panel").hidden === false, "older key input missing");
const begins = () => requests.filter(request => request.url.endsWith("authenticate/begin")).length;
for (const invalid of ["", "private-invalid-json", JSON.stringify({ ...hint, credential_id: "AB" }),
  JSON.stringify({ ...hint, rp_id: "localhost:61180" }), JSON.stringify({ ...hint, extra: "private-extra" }), "x".repeat(1801)]) {
  hintInput.value = invalid;
  const before = begins();
  elementForSelector("#home-unlock-primary").click();
  await pause();
  assert(hintInput.value === "", "invalid hint survived attempt");
  assert(begins() === before, "invalid hint reached the gateway");
  assert(!elementForSelector("#home-unlock-status").textContent.includes("private-"), "hint reflected in error");
}
hintInput.value = JSON.stringify({ ...hint, rp_id: "other.example" });
elementForSelector("#home-unlock-primary").click();
await pause();
assert(credentialGetCount === 1, "wrong RP reached authenticator");
assert(hintInput.value === "", "wrong RP hint survived attempt");
hintInput.value = JSON.stringify(hint);
credentialFailure = new Error("private-AQID-authenticator-error");
elementForSelector("#home-unlock-primary").click();
await pause();
credentialFailure = null;
assert(credentialGetCount === 2, "valid hint did not reach authenticator");
assert(hintInput.value === "", "valid hint survived attempt");
assert(credentialRequests[1].publicKey.allowCredentials.length === 1, "hint did not select exactly one key");
assert(Buffer.from(credentialRequests[1].publicKey.allowCredentials[0].id).toString("hex") === "010203", "hint selected wrong key");
assert(!elementForSelector("#home-unlock-status").textContent.includes("AQID"), "authenticator reflected ID");
assert(requests.filter(request => request.url.endsWith("authenticate/begin")).every(request => request.body === null), "hint sent in anonymous begin");
hintInput.value = JSON.stringify(hint);
elementForSelector("#home-passkey-hint-cancel").click();
assert(hintInput.value === "", "hint survived cancel");
assert(elementForSelector("#home-passkey-hint-panel").hidden === true, "cancel left recovery panel visible");
elementForSelector("#home-older-key-action").click();
hintInput.value = JSON.stringify(hint);
for (const listener of windowListeners.get("pagehide") || []) listener();
assert(hintInput.value === "", "hint survived pagehide");
for (const listener of windowListeners.get("pageshow") || []) listener();
elementForSelector("#home-passkey-hint-cancel").click();

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
}) === "system", "Home did not direct missing Profile recovery to System");
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

const { refreshHomeSession } = await import(
  `../capsules/home/browser/shell-auth.js?v=${moduleVersion}`
);
const { hasHomeAuthorityToken, shellState } = await import(
  `../capsules/home/browser/shell-core.js?v=${moduleVersion}`
);
for (const mode of ["network-error", "timed-out", "unavailable", "unauthorized", "forbidden"]) {
  sessionResponseMode = "success";
  await refreshHomeSession();
  sessionResponseMode = mode;
  let rejected = false;
  try {
    await refreshHomeSession();
  } catch (_) {
    rejected = true;
  }
  assert(rejected, `session refresh accepted ${mode}`);
  assert(
    hasHomeAuthorityToken() === ["network-error", "timed-out", "unavailable"].includes(mode),
    `session refresh retained the wrong authority after ${mode}`,
  );
}
sessionResponseMode = "success";
await refreshHomeSession();
sessionResponseMode = "pending";
const sessionAbort = new AbortController();
const timedSession = refreshHomeSession({ signal: sessionAbort.signal });
assert(requests.filter(request => request.url === "/api/auth/sessions/refresh").at(-1).signal === sessionAbort.signal,
  "session refresh replaced its caller's owned abort signal");
sessionAbort.abort(new DOMException("Request timed out", "TimeoutError"));
let sessionAborted = false;
try {
  await timedSession;
} catch (error) {
  sessionAborted = error.name === "TimeoutError";
}
assert(sessionAborted && hasHomeAuthorityToken(), "session deadline lost the existing authority");
assert(requestDeadlineTimers.size === 0, "settled session refresh retained its deadline");
const ownedTimedSession = refreshHomeSession();
assert(requestDeadlineTimers.size === 1, "default session refresh lacks one owned deadline");
const [deadlineId, deadline] = requestDeadlineTimers.entries().next().value;
requestDeadlineTimers.delete(deadlineId);
deadline.callback();
let ownedSessionTimedOut = false;
try {
  await ownedTimedSession;
} catch (error) {
  ownedSessionTimedOut = error.name === "TimeoutError";
}
assert(ownedSessionTimedOut && hasHomeAuthorityToken(), "owned session timeout lost its bounded refusal or authority");
assert(requestDeadlineTimers.size === 0, "default session timeout retained its timer");
resolveSessionRefresh = null;
sessionResponseMode = "success";

const waitUntil = async (condition, message) => {
  for (let attempt = 0; attempt < 20 && !condition(); attempt += 1) await pause();
  assert(condition(), message);
};
const runEventPoll = () => {
  assert(eventPollTimers.size === 1, "Home scheduled overlapping event polls", [...eventPollTimers.values()]);
  const [id, timer] = eventPollTimers.entries().next().value;
  eventPollTimers.delete(id);
  return timer.callback();
};
const sessionRequests = () => requests.filter(request => request.url === "/api/auth/sessions/refresh");
const eventRequests = () => requests.filter(request => request.url.startsWith("/api/apps/home/events?"));
const shellMessages = [];
window.EventSource = globalThis.EventSource = FakeEventSource;
reconnectSummary = true;
signedSummary = true;
activeShellFrame.contentWindow = {
  postMessage(message, origin) { shellMessages.push({ message, origin }); },
};
const shellRoute = "/apps/home-gui/#home_token=surviving-shell-token";
shellState.activeShellRootTarget = "home-gui";
shellState.activeShellRootRoute = shellRoute;
activeShellRoot.dataset.target = "home-gui";
activeShellFrame.dataset.route = shellRoute;
activeShellFrame.src = shellRoute;
await shellState.requestSummaryRefresh();
const initialStream = eventSources.at(-1);
assert(initialStream.url === "/api/apps/home/events/stream", "Home changed its event stream route");
assert(initialStream.options.withCredentials === true, "Home event stream lost its session cookie");
const systemMessages = [];
const systemFrame = {
  postMessage(message, origin) { systemMessages.push({ message, origin }); },
};
for (const listener of windowListeners.get("message") || []) listener({
  origin: "null",
  source: activeShellFrame.contentWindow,
  data: { type: "home:launch-target", homeToken: "surviving-shell-token", requestId: "system-reconnect", target: "system" },
});
await waitUntil(() => shellMessages.some(({ message }) => message.requestId === "system-reconnect"),
  "Home did not launch the isolated System fixture");
assert(shellMessages.find(({ message }) => message.requestId === "system-reconnect")?.message.result?.target === "system",
  "Home denied the isolated System fixture", shellMessages);
for (const listener of windowListeners.get("message") || []) listener({
  origin: "null",
  source: systemFrame,
  data: { type: "home:app-ready", homeToken: "surviving-system-token" },
});
runtimeVersion = "after-restart";
const reconnectDeadlineStart = requestDeadlines.length;
sessionResponseMode = "pending";
const refreshCount = sessionRequests().length;
initialStream.emit("open");
initialStream.emit("open");
await waitUntil(() => !!resolveSessionRefresh, "event stream open did not refresh the session");
assert(sessionRequests().length === refreshCount + 1, "Home overlapped reconnect session refreshes");
sessionResponseMode = "success";
resolveSessionRefresh();
await waitUntil(() => shellMessages.some(({ message }) =>
  message.type === "elastos:runtime-events" && message.events[0]?.kind === "home.summary.changed"),
"event reconnect did not notify the open shell and apps");
assert(shellState.currentSummary.runtime.version === "after-restart", "event reconnect kept the old Runtime version");
assert(activeShellFrame.src === shellRoute, "event reconnect replaced the surviving shell frame");
assert(shellMessages.every(({ origin }) => origin === "*"), "event reconnect changed the opaque frame boundary");
assert(systemMessages.some(({ message, origin }) => origin === "*" &&
  message.type === "elastos:runtime-events" && message.schema === "elastos.home.runtime-events/v1" &&
  message.events[0]?.kind === "home.summary.changed" && message.events[0]?.scope === "home"),
"event reconnect did not notify the existing System frame");

initialStream.onerror({});
assert(initialStream.closed, "Home left its failed event stream open");
eventsResponseMode = "pending";
const pollCount = eventRequests().length;
const repeatPoll = eventPollTimers.values().next().value.callback;
const recoveryPoll = runEventPoll();
await repeatPoll();
assert(eventRequests().length === pollCount + 1, "Home did not poll after a stream failure");
assert(eventRequests().at(-1).url.includes("wait_ms=0"), "Home waited for events before checking Runtime recovery");
assert(eventPollTimers.size === 0, "Home scheduled a poll while recovery was in flight", repeatPoll);
eventsResponseMode = "success";
resolveEventsPoll();
await recoveryPoll;
assert(eventSources.length === 2, "Home kept polling after the event stream recovered");
const recoveredStream = eventSources.at(-1);
const refreshedAfterPoll = sessionRequests().length;
recoveredStream.emit("open");
await pause();
assert(sessionRequests().length === refreshedAfterPoll, "Home repeated the completed reconnect refresh");

eventsResponseMode = "network-error";
recoveredStream.onerror({});
const warn = console.warn;
console.warn = () => {};
let reconnectDelayTotal = 0;
try {
  for (let attempt = 0; attempt < 20; attempt += 1) {
    const delay = eventPollTimers.values().next().value?.delay;
    assert(delay > 0 && delay <= 30_000, "Home lost its bounded reconnect backoff", delay);
    reconnectDelayTotal += delay;
    await runEventPoll();
  }
} finally {
  console.warn = warn;
}
assert(reconnectDelayTotal > 30_000, "Home outage did not exceed the former reconnect window");
assert(eventPollTimers.size === 1, "active Home stopped automatic recovery after a long outage");
assert(eventRequests().length === pollCount + 21, "Home lost an owned reconnect probe");
assert(eventPollTimers.values().next().value.delay === 30_000, "Home did not retain its capped reconnect delay");
assert(hasHomeAuthorityToken(), "restart recovery cleared the session");
eventsResponseMode = "success";
await runEventPoll();
assert(eventSources.length === 3, "Home did not reconnect automatically after a long outage");

document.hidden = true;
eventSources.at(-1).onerror({});
const beforeHiddenPoll = eventRequests().length;
await pause();
assert(eventRequests().length === beforeHiddenPoll, "hidden Home polled during restart recovery");
assert(eventPollTimers.size === 0, "hidden Home kept a permanent reconnect timer");
document.hidden = false;
for (const listener of documentListeners.get("visibilitychange") || []) listener();
await pause();
await runEventPoll();
const sourceCountBeforeCancel = eventSources.length;
eventSources.at(-1).onerror({});
eventsResponseMode = "pending";
const cancelledPoll = runEventPoll();
const cancelledSignal = eventRequests().at(-1).signal;
signedSummary = false;
await shellState.requestSummaryRefresh();
await cancelledPoll;
assert(cancelledSignal.aborted, "Home kept its event request after the session became unsigned");
assert(eventPollTimers.size === 0, "a stale event request restarted signed-out recovery");
assert(eventSources.length === sourceCountBeforeCancel, "a stale event request reopened the signed-out stream");
signedSummary = true;
eventsResponseMode = "success";
await shellState.requestSummaryRefresh();
const finalStream = eventSources.at(-1);
finalStream.onerror({});
eventsResponseMode = "unauthorized";
console.warn = () => {};
try {
  await runEventPoll();
} finally {
  console.warn = warn;
}
assert(!hasHomeAuthorityToken(), "Home kept its authority after a definitive event authorization failure");
assert(eventPollTimers.size === 0, "Home retried events after a definitive authorization failure");
assert(elementForSelector("#home-unlock").hidden === false, "event authorization failure lost the sign-in recovery");
AbortSignal.timeout = nativeAbortTimeout;
AbortSignal.any = nativeAbortAny;
const reconnectDeadlines = requestDeadlines.slice(reconnectDeadlineStart);
assert(reconnectDeadlines.length > 0 && reconnectDeadlines.every(delay => delay === 10_000),
  "Home reconnect requests lost their finite deadline", reconnectDeadlines);
assert(requestDeadlineTimers.size === 0, "closed event channel retained request deadlines");

console.log("[home-shell-auth-gate] PASS");

const { spawnSync } = await import("node:child_process");
const guestVariant = spawnSync(process.execPath, [new URL(import.meta.url).pathname], {
  env: { ...process.env, HOME_AUTH_GATE_GUEST_ENABLED: "1" },
  stdio: "inherit",
});
if (guestVariant.status !== 0) process.exit(guestVariant.status ?? 1);
