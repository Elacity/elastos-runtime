#!/usr/bin/env node

const moduleVersion = "home-20260715a";
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
    this.parentElement = null;
    this.queries = new Map();
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

  appendChild(child) {
    this.children.push(child);
    child.parentElement = this;
    return child;
  }

  replaceChildren(...children) {
    this.children = children;
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

  setAttribute(name, value) {
    this[name] = String(value);
  }

  getAttribute(name) {
    return Object.hasOwn(this, name) ? this[name] : null;
  }

  removeAttribute(name) {
    delete this[name];
  }

  remove() {
    if (this.parentElement) {
      this.parentElement.children = this.parentElement.children.filter(
        (child) => child !== this,
      );
    }
    this.parentElement = null;
  }

  closest() {
    return null;
  }

  focus() {}

  getBoundingClientRect() {
    if (this.selector === "#desktop") {
      return { left: 0, top: 0, width: 1024, height: 768, right: 1024, bottom: 768 };
    }
    const left = Number.parseFloat(this.style.left) || 0;
    const top = Number.parseFloat(this.style.top) || 0;
    const width = Number.parseFloat(this.style.width) || 640;
    const height = Number.parseFloat(this.style.height) || 480;
    return {
      left,
      top,
      x: left,
      y: top,
      width,
      height,
      right: left + width,
      bottom: top + height,
    };
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
  innerHeight: 800,
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
      desktopHidden: [],
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

function windowModel(entry) {
  const frame = entry.node.querySelector(".window-frame");
  const rect = entry.node.getBoundingClientRect();
  return {
    id: entry.id,
    target: entry.targetId,
    node: entry.node,
    frame,
    route: frame.dataset.route || frame.getAttribute("src") || "",
    hidden: entry.node.classList.contains("hidden"),
    active: entry.node.classList.contains("window-active"),
    rect,
  };
}

function modelIntersectionRatio(subject, overlay) {
  const width = Math.max(
    0,
    Math.min(subject.right, overlay.right) - Math.max(subject.left, overlay.left),
  );
  const height = Math.max(
    0,
    Math.min(subject.bottom, overlay.bottom) - Math.max(subject.top, overlay.top),
  );
  return (width * height) / Math.max(1, subject.width * subject.height);
}

const continuityWalletEntry = await shellWindows.attachAuthorizedTarget({
  target: "wallet",
  title: "Wallet",
  route:
    "/apps/wallet/?home_origin=http%3A%2F%2Flocalhost%3A61180" +
    "#home_token=deterministic-wallet-token",
  attach_kind: "iframe",
  launch_status: "launched",
});
const continuityWallet = windowModel(continuityWalletEntry);
for (const target of ["wallet-metamask", "wallet-unisat"]) {
  const connectorEntry = await shellWindows.attachAuthorizedTarget({
    target,
    title: target === "wallet-metamask" ? "MetaMask" : "UniSat",
    route:
      `/apps/${target}/?home_origin=http%3A%2F%2Flocalhost%3A61180` +
      `#home_token=deterministic-${target}-token`,
    attach_kind: "iframe",
    launch_status: "launched",
  });
  const walletDuringConnector = windowModel(continuityWalletEntry);
  const connector = windowModel(connectorEntry);
  assert(
    connector.id !== continuityWallet.id &&
      connector.node !== continuityWallet.node &&
      connector.frame !== continuityWallet.frame,
    `${target} did not receive a distinct deterministic window and frame`,
    { continuityWallet, connector },
  );
  assert(
    walletDuringConnector.id === continuityWallet.id &&
      walletDuringConnector.node === continuityWallet.node &&
      walletDuringConnector.frame === continuityWallet.frame &&
      walletDuringConnector.route === continuityWallet.route &&
      walletDuringConnector.hidden === false &&
      walletDuringConnector.rect.left === continuityWallet.rect.left &&
      walletDuringConnector.rect.top === continuityWallet.rect.top &&
      walletDuringConnector.rect.width === continuityWallet.rect.width &&
      walletDuringConnector.rect.height === continuityWallet.rect.height,
    `${target} changed the deterministic Wallet window`,
    { continuityWallet, walletDuringConnector },
  );
  assert(
    connector.hidden === false &&
      connector.active === true &&
      connector.rect.width >= 320 &&
      connector.rect.width <= 520 &&
      connector.rect.height >= 220 &&
      connector.rect.height <= 620 &&
      modelIntersectionRatio(walletDuringConnector.rect, connector.rect) < 0.55,
    `${target} did not use bounded non-covering connector geometry`,
    { walletDuringConnector, connector },
  );
  const connectorSnapshot = shellWindows.snapshotBrowserSession();
  assert(
    connectorSnapshot.windows.some((entry) => entry.target === "wallet") &&
      !connectorSnapshot.windows.some((entry) => entry.target === target),
    `${target} leaked into deterministic Home session persistence`,
    connectorSnapshot,
  );
  shellWindows.closeWindow(connector.id);
  const walletAfterConnector = windowModel(continuityWalletEntry);
  assert(
    !shellCore.shellState.windows.has(connector.id) &&
      shellCore.shellState.activeWindowId === continuityWallet.id &&
      walletAfterConnector.active === true &&
      walletAfterConnector.hidden === false &&
      walletAfterConnector.node === continuityWallet.node &&
      walletAfterConnector.frame === continuityWallet.frame &&
      walletAfterConnector.route === continuityWallet.route,
    `${target} close did not restore deterministic Wallet focus`,
    {
      activeWindowId: shellCore.shellState.activeWindowId,
      continuityWallet,
      walletAfterConnector,
    },
  );
}
const backgroundConnector = await shellWindows.attachAuthorizedTarget({
  target: "wallet-metamask",
  title: "MetaMask",
  route:
    "/apps/wallet-metamask/?home_origin=http%3A%2F%2Flocalhost%3A61180" +
    "#home_token=deterministic-background-metamask",
  attach_kind: "iframe",
  launch_status: "launched",
});
const activeConnector = await shellWindows.attachAuthorizedTarget({
  target: "wallet-unisat",
  title: "UniSat",
  route:
    "/apps/wallet-unisat/?home_origin=http%3A%2F%2Flocalhost%3A61180" +
    "#home_token=deterministic-active-unisat",
  attach_kind: "iframe",
  launch_status: "launched",
});
shellWindows.closeWindow(activeConnector.id);
assert(
  shellCore.shellState.activeWindowId === continuityWallet.id &&
    continuityWalletEntry.node.classList.contains("window-active") &&
    shellCore.shellState.windows.has(backgroundConnector.id),
  "active connector close did not return directly to Wallet focus",
  {
    activeWindowId: shellCore.shellState.activeWindowId,
    backgroundConnector: backgroundConnector.id,
    wallet: continuityWallet.id,
  },
);
shellWindows.closeWindow(backgroundConnector.id);
shellWindows.closeWindow(continuityWallet.id);
shellCore.shellState.windows.clear();
shellCore.shellState.activeWindowId = null;

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
