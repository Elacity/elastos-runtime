#!/usr/bin/env node

const moduleVersion = "home-20260725a";
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

  cloneNode() {
    return new FakeElement(`${this.selector}:clone`, false);
  }

  addEventListener() {}

  removeEventListener() {}

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

const restoredBrowserLaunches = [];
shellWindows.configureWindowHooks({
  clearIdentitySurface: () => {},
  hideLauncher: () => {},
  refreshLauncherIfVisible: () => {},
  renderDesktop: () => {},
  renderTaskbar: () => {},
  updateTaskbarState: () => {},
  launchTarget: async (target, query) => {
    restoredBrowserLaunches.push({ target, query: { ...query } });
    return {
      target,
      title: "Browser",
      route: `/apps/browser/?browser_instance=${encodeURIComponent(query.browser_instance)}`,
      attach_kind: "iframe",
      launch_status: "launched",
    };
  },
});
shellCore.shellState.currentSummary = summary;
shellCore.shellState.browserContextId = "browser:0123456789abcdef0123456789abcdef";
shellCore.shellState.homeBrowserState.session = {
  browser_context_id: shellCore.shellState.browserContextId,
  root_shell: "home-gui",
  windows: [{
    target: "browser",
    active: true,
    query: {
      browser_instance: "browser:restored-refresh-regression",
      url: "https://ela.city/",
    },
  }],
};
await shellWindows.restoreShellSession();
assert(
  restoredBrowserLaunches.length === 1,
  "one persisted Browser descriptor did not produce exactly one Browser shell launch",
  restoredBrowserLaunches,
);
assert(
  shellCore.shellState.windows.size === 1 &&
    [...shellCore.shellState.windows.values()][0]?.targetId === "browser",
  "one persisted Browser descriptor did not produce exactly one Browser shell",
  [...shellCore.shellState.windows.values()],
);
assert(
  restoredBrowserLaunches[0].query.browser_instance ===
    "browser:restored-refresh-regression",
  "Home refresh changed the persisted Browser window identity",
  restoredBrowserLaunches,
);
shellCore.shellState.windows.clear();
shellCore.shellState.activeWindowId = null;
shellCore.shellState.homeBrowserState.session = null;

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

function sessionWindow(id, targetId, {
  x,
  y,
  width,
  height,
  restoreX,
  restoreY,
  restoreWidth,
  restoreHeight,
  zIndex,
  hidden = false,
  query = {},
}) {
  const node = new FakeElement(`#${id}`);
  node.style.left = `${x}px`;
  node.style.top = `${y}px`;
  node.style.width = `${width}px`;
  node.style.height = `${height}px`;
  node.style.zIndex = String(zIndex);
  node.dataset.restoreLeft = String(restoreX);
  node.dataset.restoreTop = String(restoreY);
  node.dataset.restoreWidth = String(restoreWidth);
  node.dataset.restoreHeight = String(restoreHeight);
  if (hidden) node.classList.add("hidden");
  return {
    id,
    kind: "browser",
    targetId,
    node,
    launchQuery: query,
  };
}

shellCore.shellState.windows.clear();
const lowerWindow = sessionWindow("window-lower", "browser", {
  x: 14,
  y: 26,
  width: 720,
  height: 510,
  restoreX: 10,
  restoreY: 20,
  restoreWidth: 700,
  restoreHeight: 500,
  zIndex: 101,
  query: { url: "https://example.com/lower" },
});
const minimizedWindow = sessionWindow("window-minimized", "system", {
  x: 40,
  y: 52,
  width: 900,
  height: 620,
  restoreX: 36,
  restoreY: 44,
  restoreWidth: 880,
  restoreHeight: 600,
  zIndex: 102,
  hidden: true,
});
const activeWindow = sessionWindow("window-active", "browser", {
  x: 86,
  y: 98,
  width: 1080,
  height: 700,
  restoreX: 80,
  restoreY: 90,
  restoreWidth: 1040,
  restoreHeight: 680,
  zIndex: 103,
  query: { url: "https://example.com/active" },
});
for (const entry of [lowerWindow, minimizedWindow, activeWindow]) {
  shellCore.shellState.windows.set(entry.id, entry);
}
shellCore.shellState.activeWindowId = activeWindow.id;
const exactSnapshot = shellWindows.snapshotBrowserSession();
assert(
  exactSnapshot.root_shell === "home-gui",
  "saved window session lost its root-shell owner",
  exactSnapshot,
);
assert(
  exactSnapshot.windows.map((entry) => entry.query?.url || entry.target).join("|") ===
    "https://example.com/lower|system|https://example.com/active",
  "saved window session lost bottom-to-top z-order",
  exactSnapshot,
);
assert(
  exactSnapshot.windows[0].x === 14 &&
    exactSnapshot.windows[0].y === 26 &&
    exactSnapshot.windows[0].width === 720 &&
    exactSnapshot.windows[0].height === 510 &&
    exactSnapshot.windows[0].restoreX === 10 &&
    exactSnapshot.windows[0].restoreY === 20 &&
    exactSnapshot.windows[0].restoreWidth === 700 &&
    exactSnapshot.windows[0].restoreHeight === 500,
  "saved window session lost exact geometry",
  exactSnapshot.windows[0],
);
assert(
  exactSnapshot.windows[1].hidden === true &&
    exactSnapshot.windows[2].active === true,
  "saved window session lost minimized or active state",
  exactSnapshot,
);
const exactRestored = shellWindows.normalizeRestorableSession(summary, exactSnapshot, {
  rootShell: "home-gui",
});
assert(
  exactRestored.length === 3 &&
    exactRestored[0].x === 14 &&
    exactRestored[0].y === 26 &&
    exactRestored[0].width === 720 &&
    exactRestored[0].height === 510 &&
    exactRestored[1].hidden === true &&
    exactRestored[2].active === true,
  "window session did not round-trip exact geometry, minimization, active state, and z-order",
  exactRestored,
);
shellCore.shellState.windows.delete(minimizedWindow.id);
const afterExplicitClose = shellWindows.snapshotBrowserSession();
assert(
  !afterExplicitClose.windows.some((entry) => entry.target === "system"),
  "explicitly closed window remained in the saved session",
  afterExplicitClose,
);
assert(
  !shellWindows.normalizeRestorableSession(summary, afterExplicitClose, {
    rootShell: "home-gui",
  }).some((entry) => entry.target === "system"),
  "explicitly closed window was restored",
  afterExplicitClose,
);
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
