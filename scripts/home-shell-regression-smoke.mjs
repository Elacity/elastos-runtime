#!/usr/bin/env node

const moduleVersion = "home-20260724as";
const savedStatePatches = [];
const requests = [];

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

class FakeElement {
  constructor(selector = "", withTemplateContent = true) {
    this.selector = selector;
    this.children = [];
    this.dataset = {};
    this.style = {};
    this.hidden = false;
    this.classList = new FakeClassList();
    this.content = withTemplateContent
      ? {
          firstElementChild: new FakeElement(`:template-child`, false),
          cloneNode: () => new FakeElement(`:template-fragment`, false),
        }
      : { firstElementChild: null };
  }

  append(...children) {
    for (const child of children) this.appendChild(child);
  }

  appendChild(child) {
    this.children.push(child);
    return child;
  }

  replaceChildren(...children) {
    this.children = children;
  }

  querySelector(selector) {
    return new FakeElement(`${this.selector} ${selector}`);
  }

  querySelectorAll() {
    return [];
  }

  setAttribute(name, value) {
    this[name] = String(value);
  }

  removeAttribute(name) {
    delete this[name];
  }

  closest() {
    return null;
  }

  focus() {}

  getBoundingClientRect() {
    if (this.selector === "#desktop") {
      return { left: 0, top: 0, width: 1024, height: 768, right: 1024, bottom: 768 };
    }
    return { left: 0, top: 0, width: 640, height: 480, right: 640, bottom: 480 };
  }
}

const elementCache = new Map();
function elementForSelector(selector) {
  if (!elementCache.has(selector)) {
    elementCache.set(selector, new FakeElement(selector));
  }
  return elementCache.get(selector);
}

globalThis.HTMLElement = FakeElement;
globalThis.document = {
  activeElement: null,
  body: elementForSelector("body"),
  querySelector: elementForSelector,
  createElement: (tag) => new FakeElement(tag),
};
globalThis.window = {
  crypto: { randomUUID: () => "home-shell-regression-smoke" },
  localStorage: { getItem: () => null, setItem: () => {} },
  performance: { now: () => Date.now() },
  innerWidth: 1280,
  clearInterval: () => {},
};
globalThis.fetch = async (_url, init = {}) => {
  requests.push({
    url: String(_url),
    body: init.body ? JSON.parse(init.body) : null,
  });
  if (init.body) {
    savedStatePatches.push(JSON.parse(init.body));
  }
  return {
    ok: true,
    status: 200,
    statusText: "OK",
    json: async () => ({ ok: true }),
    text: async () => "{}",
  };
};

function assert(condition, message, details = null) {
  if (!condition) {
    const error = new Error(message);
    error.details = details;
    throw error;
  }
}

function positionsOverlap(left, right) {
  return Math.abs(left.x - right.x) < 92 && Math.abs(left.y - right.y) < 98;
}

const shellCore = await import(`../capsules/home-gui/browser/shell-core.js?v=${moduleVersion}`);
const shellWindows = await import(`../capsules/home-gui/browser/shell-windows.js?v=${moduleVersion}`);
const shellSurface = await import(`../capsules/home-gui/browser/shell-surface.js?v=${moduleVersion}`);

const summary = {
  authority: { signed_in: true },
  targets: [
    { target: "wallet", title: "Wallet", route: "/apps/wallet/" },
    { target: "inbox", title: "Inbox", route: "/apps/inbox/" },
    { target: "people", title: "People", route: "/apps/people/", attach_kind: "iframe", role: "app" },
    { target: "browser", title: "Browser", route: "/apps/browser/" },
    { target: "system", title: "System", route: "/apps/system/" },
  ],
  browser_state: {
    principal_id: "principal:home-shell-regression",
    layout: {
      desktop: {
        wallet: { x: 12, y: 12 },
        people: { x: 12, y: 12 },
        inbox: { x: 12, y: 12 },
        browser: { x: 12, y: 12 },
      },
      taskbar: [],
      desktopApps: ["wallet", "people", "inbox", "browser"],
      desktopIconsVisible: true,
    },
  },
};

shellCore.initializeShellLayout(summary);
document.body.dataset.homeShell = "desktop";

const layout = shellCore.shellState.shellLayoutState.desktop;
assert(layout.wallet, "wallet desktop position missing", layout);
assert(layout.people, "people desktop position missing", layout);
assert(!positionsOverlap(layout.wallet, layout.people), "People and Wallet desktop positions still overlap", layout);
assert(!positionsOverlap(layout.inbox, layout.people), "People and Inbox desktop positions still overlap", layout);
assert(
  savedStatePatches.some((patch) => patch.layout && patch.layout.desktop && patch.layout.desktop.people),
  "de-collided desktop layout was not saved",
  savedStatePatches,
);

const restored = shellWindows.normalizeRestorableSession(summary, {
  root_shell: "home-gui",
  windows: [
    { target: "people", active: true, x: 10, y: 10 },
    { target: "people", active: false, x: 20, y: 20 },
    { target: "inbox", x: 30, y: 30 },
    { target: "inbox", x: 40, y: 40 },
    { target: "wallet", x: 50, y: 50 },
    { target: "wallet", x: 60, y: 60 },
    { target: "browser", query: { url: "https://example.com/a" } },
    { target: "browser", query: { url: "https://example.com/b" } },
    { target: "missing-target" },
  ],
});

const rootlessSessionRestoredIntoGui = shellWindows.normalizeRestorableSession(summary, {
  windows: [
    { target: "browser", query: { url: "https://example.com/rootless" } },
  ],
}, { rootShell: "home-gui" });
const cliOwnedRestoredIntoGui = shellWindows.normalizeRestorableSession(summary, {
  root_shell: "home-cli",
  windows: [
    { target: "browser", query: { url: "https://example.com/cli" } },
  ],
}, { rootShell: "home-gui" });

const counts = restored.reduce((next, entry) => {
  next[entry.target] = (next[entry.target] || 0) + 1;
  return next;
}, {});

assert(counts.people === 1, "People restored more than once", restored);
assert(counts.inbox === 1, "Inbox restored more than once", restored);
assert(counts.wallet === 1, "Wallet restored more than once", restored);
assert(counts.browser === 2, "Browser should allow multiple restored windows", restored);
assert(!counts["missing-target"], "unknown saved session target was restored", restored);
assert(
  rootlessSessionRestoredIntoGui.length === 0,
  "rootless GUI session restored without explicit shell ownership",
  rootlessSessionRestoredIntoGui,
);
assert(
  cliOwnedRestoredIntoGui.length === 0,
  "CLI-owned overlay session restored into Home GUI",
  cliOwnedRestoredIntoGui,
);
assert(
  restored
    .filter((entry) => entry.target === "browser")
    .every((entry) => typeof entry.query.browser_instance === "string" && entry.query.browser_instance !== ""),
  "restored Browser windows must receive distinct browser_instance query values",
  restored,
);

const objectWithoutCapabilities = {
  uri: "localhost://Users/self/Desktop/Mystery.txt",
  name: "Mystery.txt",
  kind: "file",
  mime: "text/plain",
};
shellCore.shellState.currentSummary = {
  ...summary,
  desktop_objects: { objects: [objectWithoutCapabilities] },
};
shellCore.shellState.selectedDesktopTargetId = `object:${objectWithoutCapabilities.uri}`;
shellCore.shellState.contextMenuTarget = {
  kind: "desktop-object",
  entryId: `object:${objectWithoutCapabilities.uri}`,
};
const objectActionRequestCount = requests.length;
assert(
  shellSurface.openSelectedDesktopEntry() === false,
  "desktop object without capability metadata opened instead of failing closed",
);
shellSurface.handleContextAction("open-desktop-object");
shellSurface.handleContextAction("download-desktop-object");
shellSurface.handleContextAction("properties-desktop-object");
shellSurface.handleContextAction("empty-trash");
assert(
  requests.length === objectActionRequestCount,
  "desktop object actions without capability metadata reached Runtime",
  requests.slice(objectActionRequestCount),
);

console.log("[home-shell-regression] PASS");
