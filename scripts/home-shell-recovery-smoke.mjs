#!/usr/bin/env node

import { readFileSync } from "node:fs";
import vm from "node:vm";
import { getEventListeners } from "node:events";

const moduleVersion = "home-update-20261003a";
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
      { name: "home-gui", title: "Home GUI", role: "shell", launchable: true, route: "/home/" },
      { name: "home-cli", title: "Home CLI", role: "shell", launchable: true, route: "/apps/home-cli/" },
    ],
  },
  app: { id: "home", route: "/home/" },
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
  getElementById: id => elementForSelector(`#${id}`),
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
  sessionStorage: { getItem: () => null },
  crypto: { randomUUID: () => "home-shell-recovery-smoke" },
  location: {
    href: "http://localhost:61180/home/",
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
  if (url === "/api/auth/sessions/refresh") {
    return jsonResponse({ home_token: "host-token" });
  }
  if (url === "/api/apps/home/launch") {
    assert(body?.target === "home-cli", "alternate shell launch target drifted", body);
    assert(body?.query?.shell_mode === "root", "alternate shell must launch in root mode", body);
    return failedResponse(500, "Internal Server Error", "simulated root shell launch failure");
  }
  if (url === "/api/apps/home/active-shell") {
    assert(body?.active === "home-gui", "failed launch recovery requested the wrong active shell", body);
    assert(
      init.headers?.["x-elastos-home-token"] === "host-token",
      "failed launch recovery did not use the trusted Home host token",
      init.headers,
    );
    return jsonResponse({
      schema: "elastos.home.active-shell/v1",
      active: "home-gui",
      candidates: [],
    });
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
assert(/reload/i.test(recoveryCopy.textContent), "recovery copy did not expose reload path", recoveryCopy.textContent);
assert(recoveryDetail.textContent === "A Home service failed while loading.", "recovery detail exposed an internal launch error", recoveryDetail.textContent);
assert(recoveryHome.disabled === false, "Desktop recovery should stay available with trusted Home host authority");
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

for (const listener of recoveryHome.listeners.get("click") || []) {
  listener();
}
for (let attempt = 0; attempt < 20; attempt += 1) {
  if (requests.some((request) => request.url === "/api/apps/home/active-shell")) {
    break;
  }
  await new Promise((resolve) => setTimeout(resolve, 0));
}
assert(
  requests.some(
    (request) =>
      request.url === "/api/apps/home/active-shell"
      && request.headers["x-elastos-home-token"] === "host-token"
      && request.body?.active === "home-gui",
  ),
  "failed launch recovery did not drive Desktop activation with trusted Home host authority",
  requests,
);

delete window.location.reload;
for (const listener of recoveryReload.listeners.get("click") || []) {
  listener();
}
assert(window.location.href === "/home/", "recovery reload fallback did not return to Home", window.location.href);

// Exercise the real event-channel functions with an isolated clock and transport.
const hostSource = readFileSync(new URL("../capsules/home/browser/home-shell-host.js", import.meta.url), "utf8");
const eventFunctions = hostSource.slice(hostSource.indexOf("function bindHomeEventLifecycle() {"));
const eventConstants = hostSource.split("\n").filter(line => /^const HOME_EVENTS_/.test(line)).join("\n");
const settleEventTasks = () => new Promise(resolve => setImmediate(resolve));

function eventFixture() {
  const timers = new Map(), listeners = new Map(), streams = [], reads = [], broadcasts = [], deadlines = [];
  let timerId = 0, elapsed = 0, clears = 0, prompts = 0, sessions = 0, summaries = 0;
  class EventSourceFixture {
    constructor(url, options) { this.url = url; this.options = options; this.listeners = new Map(); this.closed = false; streams.push(this); }
    addEventListener(type, handler) { this.listeners.set(type, handler); }
    close() { this.closed = true; }
  }
  const signed = { authority: { signed_in: true } };
  const state = { currentSummary: signed, homeEventsCursor: "", homeEventsInFlight: false,
    homeEventsSource: null, homeEventsStreamFailed: false, homeEventsTimer: null };
  const context = vm.createContext({
    AbortController, URLSearchParams, EventSource: EventSourceFixture,
    AbortSignal: {}, // Older Safari supplies AbortController without the static signal helpers.
    document: { hidden: false },
    window: {
      EventSource: EventSourceFixture,
      addEventListener: (type, handler) => listeners.set(type, handler),
      setTimeout: (handler, delay) => {
        if (delay === 10_000 || delay === 35_000) deadlines.push(delay);
        timers.set(++timerId, { handler, delay }); return timerId;
      },
      clearTimeout: id => timers.delete(id),
    },
    shellState: state, launchedAppContexts: new Map(), OPAQUE_FRAME_TARGET: "*",
    homeEventsReconnectAttempts: 0, homeEventsReconnectInFlight: null, homeEventsReconnectController: null,
    homeEventsChannelActive: true, homeEventsChannelGeneration: 0, homeEventsPollController: null,
    homeSummarySignedIn: value => value?.authority?.signed_in === true,
    homeSummaryHasProofBoundSession: () => true,
    refreshHomeSession: async () => { sessions++; },
    refreshShellSummary: async () => { summaries++; return state.currentSummary; },
    fetchJson: async (url, init) => { reads.push({ url, ...init }); return { schema: "elastos.home.events/v1", cursor: "same-session", events: [] }; },
    isHomeAuthError: error => error.status === 401 || error.status === 403,
    clearHomeAuthorityToken: () => { clears++; state.currentSummary = null; },
    showHostAuthGate: async () => { prompts++; },
    postToActiveShell: message => broadcasts.push(message),
    requestShellSummaryRefresh: () => {}, console: { warn() {}, error() {} },
  });
  vm.runInContext(`${eventConstants}\n${eventFunctions}`, context);
  const fireNext = async () => {
    const next = [...timers].sort((a, b) => a[1].delay - b[1].delay)[0];
    assert(next, "event recovery lost its next owned timer");
    timers.delete(next[0]);
    elapsed += next[1].delay;
    await next[1].handler();
    return next[1].delay;
  };
  return { context, state, timers, listeners, streams, reads, broadcasts, deadlines, fireNext,
    counts: () => ({ elapsed, clears, prompts, sessions, summaries }) };
}

{
  const f = eventFixture();
  f.context.ensureHomeEventChannel();
  const initial = f.streams[0];
  initial.onerror(new TypeError("Home is offline"));
  assert(initial.closed, "the failed event stream stayed open");
  f.context.fetchJson = async (url, init) => { f.reads.push({ url, ...init }); throw new TypeError("Home is offline"); };
  const delays = [];
  for (let attempt = 0; attempt < 17; attempt++) delays.push(await f.fireNext());
  assert(f.counts().elapsed > 30_000, "outage did not exceed the former reconnect window");
  assert(delays.every(delay => delay > 0 && delay <= 30_000), "event recovery exceeded its backoff cap", delays);
  assert(delays.slice(-3).every(delay => delay === 30_000), "event recovery did not keep its capped backoff", delays);
  assert(f.counts().clears === 0, "transient outage cleared the signed Home session");
  assert(f.timers.size === 1, "event recovery stopped after more than fifteen failures");
  f.context.fetchJson = async (url, init) => { f.reads.push({ url, ...init }); return { schema: "elastos.home.events/v1", cursor: "same-session", events: [] }; };
  await f.fireNext();
  assert(f.counts().sessions === 1 && f.counts().summaries === 1, "recovery did not refresh the same signed session and summary");
  assert(f.state.homeEventsCursor === "same-session", "recovery lost the event cursor");
  assert(f.streams.length === 2 && !f.streams[1].closed, "recovery did not restore the event stream");
  assert(f.streams[1].options.withCredentials === true, "recovery lost signed stream credentials");
  assert(f.deadlines.every(ms => ms === 10_000), "outage probes or recovery refreshes lost their request deadline", f.deadlines);
  assert(f.reads.every(read => getEventListeners(read.signal, "abort").length === 0),
    "settled event requests retained their owned abort listeners");
  assert(f.broadcasts.some(message => message.events.some(event => event.kind === "home.summary.changed")), "recovery did not notify the active shell");
  f.listeners.get("pagehide")();
  assert(f.streams[1].closed && f.timers.size === 0, "recovered event channel outlived its document");
}

{
  const f = eventFixture();
  f.context.window.EventSource = null;
  let finish;
  f.context.fetchJson = (url, init) => { f.reads.push({ url, ...init }); return new Promise(resolve => { finish = resolve; }); };
  const polling = f.context.pollHomeEvents();
  const generation = f.context.homeEventsChannelGeneration;
  assert(f.deadlines[0] === 35_000, "normal long poll lost its bounded deadline");
  f.listeners.get("pagehide")();
  assert(f.reads[0].signal.aborted, "page close did not abort the owned long poll");
  assert(f.timers.size === 0 && getEventListeners(f.reads[0].signal, "abort").length === 0,
    "page close kept the request deadline or its abort listener");
  assert(f.context.homeEventsChannelGeneration > generation, "page close did not retire the event epoch");
  f.context.ensureHomeEventChannel();
  assert(f.timers.size === 0, "closed document armed another event timer");
  finish({ schema: "elastos.home.events/v1", cursor: "stale", events: [{ kind: "home.summary.changed" }] });
  await polling;
  assert(f.state.homeEventsCursor === "" && f.broadcasts.length === 0, "late event response crossed the retired document epoch");
  f.listeners.get("pageshow")();
  assert(f.timers.size === 1, "restored Home document did not resume its channel");
  f.listeners.get("pagehide")();
  assert(f.timers.size === 0, "restored document kept an owned timer after close");
}

for (const reconnecting of [false, true]) {
  const f = eventFixture();
  f.context.window.EventSource = null;
  f.state.homeEventsStreamFailed = reconnecting;
  f.context.fetchJson = (url, init) => {
    f.reads.push({ url, ...init });
    return new Promise((resolve, reject) => init.signal.addEventListener("abort", () => reject(init.signal.reason), { once: true }));
  };
  const polling = f.context.pollHomeEvents();
  const deadline = [...f.timers.values()][0];
  assert(deadline.delay === (reconnecting ? 10_000 : 35_000), "event request lost its exact deadline");
  deadline.handler();
  await polling;
  assert(f.reads[0].signal.aborted && f.reads[0].signal.reason.name === "TimeoutError",
    "owned event deadline failed to abort the pending transport");
  assert(getEventListeners(f.reads[0].signal, "abort").length === 0,
    "timed-out event request retained an abort listener");
  assert(f.timers.size === 1 && f.counts().clears === 0, "request timeout lost session authority or retry ownership");
  f.listeners.get("pagehide")();
  assert(f.timers.size === 0, "document close kept the timeout retry");
}

{
  const f = eventFixture();
  const pendingSessions = [];
  f.context.refreshHomeSession = ({ signal }) => new Promise(resolve => { pendingSessions.push({ signal, resolve }); });
  const oldRefresh = f.context.refreshHomeAfterEventReconnect();
  assert(oldRefresh === f.context.refreshHomeAfterEventReconnect(), "reconnect refreshes did not share one request");
  await settleEventTasks();
  f.listeners.get("pagehide")();
  assert(pendingSessions[0].signal.aborted, "page close did not abort session recovery");
  f.listeners.get("pageshow")();
  const newRefresh = f.context.refreshHomeAfterEventReconnect();
  await settleEventTasks();
  pendingSessions[0].resolve();
  await oldRefresh;
  assert(f.context.homeEventsReconnectInFlight === newRefresh, "old refresh completion cleared the new document's request");
  assert(f.counts().summaries === 0 && f.broadcasts.length === 0, "old recovery refreshed or broadcast into the new epoch");
  pendingSessions[1].resolve();
  await newRefresh;
  assert(f.counts().summaries === 1 && f.broadcasts.length === 1, "current recovery did not refresh and notify once");
  f.listeners.get("pagehide")();
  assert(f.timers.size === 0 && f.streams.every(stream => stream.closed), "session recovery kept a stream or timer after close");
}

{
  const f = eventFixture();
  f.context.ensureHomeEventChannel();
  const error = new Error("Home access changed"); error.status = 403;
  f.streams[0].onerror(error);
  await settleEventTasks();
  assert(f.counts().clears === 1 && f.counts().prompts === 1, "definite auth refusal did not request Home sign-in");
  f.context.ensureHomeEventChannel();
  assert(f.timers.size === 0 && f.streams.every(stream => stream.closed), "auth refusal restarted the signed event channel");
}

console.log("[home-shell-recovery] PASS (6 event recovery cases)");
