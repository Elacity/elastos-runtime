#!/usr/bin/env node

import { existsSync, readFileSync } from "node:fs";

const moduleVersion = "home-20260813a";
const savedStatePatches = [];
const requests = [];
const windowEventListeners = new Map();
const documentEventListeners = new Map();
let randomUuidSerial = 0;

function matchesSelector(node, selector) {
  if (!node || typeof selector !== "string" || !selector) {
    return false;
  }
  if (selector.includes(",")) {
    return selector.split(",").some((part) => matchesSelector(node, part.trim()));
  }
  if (selector.startsWith("#")) {
    return node.id === selector.slice(1);
  }
  if (selector.startsWith(".")) {
    const token = selector.slice(1);
    return node.classList.contains(token)
      || String(node.className || "").split(/\s+/).includes(token);
  }
  if (selector === "[contenteditable='true']") {
    return node.contenteditable === "true" || node.contentEditable === "true";
  }
  return String(node.tagName || "").toLowerCase() === selector.toLowerCase();
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

class FakeElement {
  constructor(selector = "", withTemplateContent = true) {
    this.selector = selector;
    this.children = [];
    this.parentElement = null;
    this.queries = new Map();
    this.listeners = new Map();
    this.dataset = {};
    this.style = {};
    this.hidden = false;
    this.inert = false;
    this.disabled = false;
    this.textContent = "";
    this.innerHTML = "";
    this.className = "";
    this.id = "";
    this.tagName = selector.startsWith("#") ? "DIV" : selector.toUpperCase();
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

  append(...children) {
    for (const child of children) {
      this.appendChild(child);
    }
  }

  cloneNode() {
    return new FakeElement(`${this.selector}:clone`, false);
  }

  addEventListener(type, listener) {
    const listeners = this.listeners.get(type) || [];
    listeners.push(listener);
    this.listeners.set(type, listeners);
  }

  removeEventListener(type, listener) {
    const listeners = this.listeners.get(type) || [];
    this.listeners.set(
      type,
      listeners.filter((candidate) => candidate !== listener),
    );
  }

  dispatch(type, event = {}) {
    const payload = {
      target: this,
      currentTarget: this,
      preventDefault() {
        this.defaultPrevented = true;
      },
      stopPropagation() {
        this.propagationStopped = true;
      },
      ...event,
    };
    for (const listener of this.listeners.get(type) || []) {
      listener(payload);
    }
    return payload;
  }

  replaceChildren(...children) {
    this.children = children;
    for (const child of children) {
      child.parentElement = this;
    }
  }

  querySelector(selector) {
    if (!this.queries.has(selector)) {
      const child = new FakeElement(`${this.selector} ${selector}`);
      if (selector.startsWith("#")) {
        child.id = selector.slice(1);
      }
      if (selector.startsWith(".")) {
        child.className = selector.slice(1);
        child.classList.add(selector.slice(1));
      }
      child.parentElement = this;
      this.queries.set(selector, child);
    }
    return this.queries.get(selector);
  }

  querySelectorAll(selector) {
    const matches = [];
    const stack = [...this.children];
    while (stack.length > 0) {
      const child = stack.shift();
      if (matchesSelector(child, selector)) {
        matches.push(child);
      }
      stack.unshift(...child.children);
    }
    return matches;
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

  closest(selector) {
    let current = this;
    while (current) {
      if (matchesSelector(current, selector)) {
        return current;
      }
      current = current.parentElement || null;
    }
    return null;
  }

  contains(node) {
    let current = node;
    while (current) {
      if (current === this) {
        return true;
      }
      current = current.parentElement || null;
    }
    return false;
  }

  focus() {
    document.activeElement = this;
  }

  scrollIntoView() {}

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
  addEventListener(type, listener) {
    const listeners = documentEventListeners.get(type) || [];
    listeners.push(listener);
    documentEventListeners.set(type, listeners);
  },
  removeEventListener(type, listener) {
    const listeners = documentEventListeners.get(type) || [];
    documentEventListeners.set(
      type,
      listeners.filter((candidate) => candidate !== listener),
    );
  },
  querySelector: elementForSelector,
  createElement: (tag) => new FakeElement(tag),
  getElementById(id) {
    const stack = [...elementCache.values()];
    const seen = new Set();
    while (stack.length > 0) {
      const node = stack.shift();
      if (!node || seen.has(node)) {
        continue;
      }
      seen.add(node);
      if (node.id === id) {
        return node;
      }
      stack.unshift(...node.children, ...node.queries.values());
    }
    return null;
  },
};
globalThis.window = {
  crypto: {
    randomUUID: () => `home-shell-regression-smoke-${++randomUuidSerial}`,
  },
  localStorage: { getItem: () => null, setItem: () => {} },
  location: { href: "http://localhost:61180/apps/home-gui/" },
  performance: { now: () => Date.now() },
  innerWidth: 1280,
  innerHeight: 800,
  addEventListener(type, listener) {
    const listeners = windowEventListeners.get(type) || [];
    listeners.push(listener);
    windowEventListeners.set(type, listeners);
  },
  setTimeout,
  clearTimeout,
  clearInterval: () => {},
};

function sendWindowEvent(type, event) {
  for (const listener of windowEventListeners.get(type) || []) {
    listener(event);
  }
}

function sendDocumentEvent(type, event = {}) {
  const payload = {
    preventDefault() {
      this.defaultPrevented = true;
    },
    stopPropagation() {
      this.propagationStopped = true;
    },
    ...event,
  };
  for (const listener of documentEventListeners.get(type) || []) {
    listener(payload);
  }
  return payload;
}
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

async function withCapturedFrameRevealTimers(run) {
  const nativeSetTimeout = window.setTimeout;
  const nativeClearTimeout = window.clearTimeout;
  let nextTimerId = 1;
  const timers = new Map();
  window.setTimeout = (callback, _delay) => {
    const timerId = nextTimerId++;
    timers.set(timerId, callback);
    return timerId;
  };
  window.clearTimeout = (timerId) => {
    timers.delete(Number(timerId));
  };
  try {
    return await run({
      timerIds: () => [...timers.keys()],
      fire(timerId) {
        const callback = timers.get(timerId);
        assert(callback, "missing captured frame reveal timer", { timerId, timerIds: [...timers.keys()] });
        timers.delete(timerId);
        callback();
      },
    });
  } finally {
    window.setTimeout = nativeSetTimeout;
    window.clearTimeout = nativeClearTimeout;
  }
}

const shellCore = await import(`../capsules/home-gui/browser/shell-core.js?v=${moduleVersion}`);
const shellChrome = await import(`../capsules/home-gui/browser/shell-chrome.js?v=${moduleVersion}`);
const shellControlCentre = await import(`../capsules/home-gui/browser/shell-control-centre.js?v=${moduleVersion}`);
const shellWindows = await import(`../capsules/home-gui/browser/shell-windows.js?v=${moduleVersion}`);
const shellSurface = await import(`../capsules/home-gui/browser/shell-surface.js?v=${moduleVersion}`);
const shellWalletRail = await import(`../capsules/home-gui/browser/shell-wallet-rail.js?v=${moduleVersion}`);
const shellConnectorSheet = await import(`../capsules/home-gui/browser/shell-connector-sheet.js?v=${moduleVersion}`);
const shellSpotlight = await import(`../capsules/home-gui/browser/shell-spotlight.js?v=${moduleVersion}`);
const shellKeyboard = await import(`../capsules/home-gui/browser/shell-keyboard.js?v=${moduleVersion}`);

function sourceBlock(source, needle, label) {
  const start = source.indexOf(needle);
  assert(start >= 0, `${label} missing`, { needle });
  const open = start + needle.lastIndexOf("{");
  assert(open >= 0, `${label} missing body`, { needle });
  let depth = 0;
  for (let index = open; index < source.length; index += 1) {
    const char = source[index];
    if (char === "{") {
      depth += 1;
    } else if (char === "}") {
      depth -= 1;
      if (depth === 0) {
        return source.slice(start, index + 1);
      }
    }
  }
  throw new Error(`${label} block did not terminate`);
}

const trashGlyph = new FakeElement(".taskbar-item-icon");
shellCore.mountGlyph(trashGlyph, "trash");
assert(
  trashGlyph.dataset.tone === "raster" &&
    trashGlyph.dataset.icon === "bin" &&
    trashGlyph.innerHTML === "",
  "trash glyph did not resolve to Home-owned raster bin art",
  trashGlyph,
);
assert(
  existsSync(new URL("../capsules/home-gui/browser/icons/bin/icon-64.png", import.meta.url)),
  "trash bin icon asset is missing",
);

const summary = {
  authority: { signed_in: true },
  targets: [
    { target: "wallet", title: "Wallet", route: "/apps/wallet/" },
    { target: "inbox", title: "Inbox", route: "/apps/inbox/" },
    { target: "people", title: "People", route: "/apps/people/", attach_kind: "iframe", role: "app" },
    { target: "chat-room", title: "Chat", route: "/apps/chat-room/", attach_kind: "iframe", role: "app" },
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
shellCore.shellState.currentSummary = summary;

const layout = shellCore.shellState.shellLayoutState.desktop;
assert(layout.wallet, "wallet desktop position missing", layout);
assert(layout.people, "people desktop position missing", layout);

const firstRunSummary = {
  authority: { signed_in: true },
  targets: [
    { target: "wallet", title: "Wallet", route: "/apps/wallet/" },
    { target: "chat-room", title: "Chat", route: "/apps/chat-room/" },
    { target: "browser", title: "Browser", route: "/apps/browser/" },
    { target: "system", title: "System", route: "/apps/system/" },
    { target: "documents", title: "Documents", route: "/apps/documents/" },
    { target: "object-target", title: "Shared object", route: "/apps/object-target/", target_kind: "object" },
  ],
  browser_state: {
    principal_id: "principal:first-run",
  },
};

shellCore.initializeShellLayout(firstRunSummary);
assert(
  JSON.stringify(shellCore.shellState.shellLayoutState.taskbar) === JSON.stringify([
    "browser",
    "wallet",
    "documents",
    "chat-room",
    "system",
  ]),
  "fresh layout did not seed only the available core dock targets in canonical order",
  shellCore.shellState.shellLayoutState.taskbar,
);
assert(
  JSON.stringify(shellCore.shellState.shellLayoutState.desktopHidden) === JSON.stringify([
    "wallet",
    "chat-room",
    "browser",
    "system",
    "documents",
  ]),
  "fresh layout did not hide all non-object visible targets and keep object targets visible",
  shellCore.shellState.shellLayoutState.desktopHidden,
);
assert(
  shellCore.isTargetOnDesktop("object-target") === true,
  "fresh layout hid an object target from the desktop",
  shellCore.shellState.shellLayoutState.desktopHidden,
);
assert(
  shellCore.isTargetOnDesktop("wallet") === false,
  "fresh layout left an app target on the desktop",
  shellCore.shellState.shellLayoutState.desktopHidden,
);
assert(
  JSON.stringify(shellCore.shellState.shellLayoutState.desktop) === JSON.stringify({
    "object-target": { x: 12, y: 12 },
  }),
  "fresh layout did not seed only the visible desktop entry positions",
  shellCore.shellState.shellLayoutState.desktop,
);

const storedEmptyTaskbarSummary = {
  authority: { signed_in: true },
  targets: firstRunSummary.targets,
  browser_state: {
    principal_id: "principal:stored-empty-taskbar",
    layout: {
      desktop: {
        wallet: { x: 128, y: 64 },
        browser: { x: 240, y: 64 },
      },
      taskbar: [],
      desktopHidden: ["wallet", "browser"],
      desktopIconsVisible: true,
    },
  },
};

shellCore.initializeShellLayout(storedEmptyTaskbarSummary);
assert(
  Array.isArray(shellCore.shellState.shellLayoutState.taskbar) &&
    shellCore.shellState.shellLayoutState.taskbar.length === 0,
  "stored empty taskbar was rewritten by the first-run dock policy",
  shellCore.shellState.shellLayoutState.taskbar,
);
assert(
  JSON.stringify(shellCore.shellState.shellLayoutState.desktopHidden) === JSON.stringify(["wallet", "browser"]),
  "stored desktopHidden changed without required normalization",
  shellCore.shellState.shellLayoutState.desktopHidden,
);
assert(
  JSON.stringify({
    wallet: shellCore.shellState.shellLayoutState.desktop.wallet,
    browser: shellCore.shellState.shellLayoutState.desktop.browser,
  }) === JSON.stringify({
    wallet: { x: 128, y: 64 },
    browser: { x: 240, y: 64 },
  }),
  "stored desktop positions were not preserved for saved hidden targets",
  shellCore.shellState.shellLayoutState.desktop,
);

const peopleStyle = readFileSync(
  new URL("../capsules/people/browser/style.css", import.meta.url),
  "utf8",
);
const chatStyle = readFileSync(
  new URL("../capsules/chat-room/browser/style.css", import.meta.url),
  "utf8",
);
const inboxSource = readFileSync(
  new URL("../capsules/inbox/browser/index.html", import.meta.url),
  "utf8",
);
const collaborationChromeSources = [peopleStyle, chatStyle, inboxSource].join("\n");
const browserStyle = readFileSync(
  new URL("../capsules/browser/browser/style.css", import.meta.url),
  "utf8",
);
const homeGuiScript = readFileSync(
  new URL("../capsules/home-gui/browser/home-gui.js", import.meta.url),
  "utf8",
);
const shellChromeScript = readFileSync(
  new URL("../capsules/home-gui/browser/shell-chrome.js", import.meta.url),
  "utf8",
);
const shellControlCentreScript = readFileSync(
  new URL("../capsules/home-gui/browser/shell-control-centre.js", import.meta.url),
  "utf8",
);
const homeGuiShellScript = readFileSync(
  new URL("../capsules/home-gui/browser/home-gui-shell.js", import.meta.url),
  "utf8",
);
const homeShellHostScript = readFileSync(
  new URL("../capsules/home/browser/home-shell-host.js", import.meta.url),
  "utf8",
);
const controlCentreScript = readFileSync(
  new URL("../capsules/home-gui/browser/shell-control-centre.js", import.meta.url),
  "utf8",
);
const openHomeGuiTargetBlock = sourceBlock(
  homeGuiScript,
  "export function openHomeGuiTarget(target, options = {}) {",
  "Home GUI connector open path",
);
const attachAuthorizedHomeGuiTargetBlock = sourceBlock(
  homeGuiScript,
  "export function attachAuthorizedHomeGuiTarget(launched) {",
  "Home GUI authorized connector attach path",
);
const homeSetupSheetScript = readFileSync(
  new URL("../capsules/home-gui/browser/shell-setup-sheet.js", import.meta.url),
  "utf8",
);
const homeGuiStyle = readFileSync(
  new URL("../capsules/home-gui/browser/style.css", import.meta.url),
  "utf8",
);
const homeGuiTemplate = readFileSync(
  new URL("../capsules/home-gui/browser/home-gui-template.html", import.meta.url),
  "utf8",
);
const launcherDarkDockIcon = new URL(
  "../capsules/home-gui/browser/icons/apps-launcher/dark-dock/icon-64.png",
  import.meta.url,
);
const launcherLightDockIcon = new URL(
  "../capsules/home-gui/browser/icons/apps-launcher/light-dock/icon-64.png",
  import.meta.url,
);
const canonicalWindowHeadMarkup =
  /<div class="window-head">\s*<div class="window-traffic-lights">\s*<button class="window-action-btn" data-action="close"[\s\S]*?<button class="window-action-btn" data-action="minimize"[\s\S]*?<button class="window-action-btn" data-action="maximize"[\s\S]*?<\/div>\s*<div class="window-head-draggable">[\s\S]*?<\/div>\s*<div class="window-head-balance" aria-hidden="true"><\/div>\s*<\/div>/;
const taskbarOpenIndex = homeGuiTemplate.indexOf('<footer class="taskbar"');
const launcherOpenIndex = homeGuiTemplate.indexOf('<aside id="launcher"');
const launcherCloseIndex = homeGuiTemplate.indexOf("</aside>", launcherOpenIndex);
const taskbarPrimaryIndex = homeGuiTemplate.indexOf('<div class="taskbar-primary">', launcherCloseIndex);
const taskbarCloseIndex = homeGuiTemplate.indexOf("</footer>", taskbarPrimaryIndex);
const launcherMarkup = launcherOpenIndex >= 0 && launcherCloseIndex > launcherOpenIndex
  ? homeGuiTemplate.slice(launcherOpenIndex, launcherCloseIndex)
  : "";

assert(
  openHomeGuiTargetBlock.includes("void showConnectorSheet(target, {") &&
    openHomeGuiTargetBlock.includes('query: { ...query, presentation: "sheet" },') &&
    openHomeGuiTargetBlock.includes('console.error("connector sheet open failed", error);') &&
    !openHomeGuiTargetBlock.includes("openTarget(target, options);\n      },"),
  "Home GUI connector sheet open path still falls back to a generic connector launch",
  openHomeGuiTargetBlock,
);
assert(
  /if \(walletRailOpen\(\) && isConnectorSheetTarget\(launched\?\.target\)\) \{\s*return attachAuthorizedConnectorSheet\(launched\);\s*\}\s*return attachAuthorizedTarget\(launched\);/.test(
    attachAuthorizedHomeGuiTargetBlock,
  ),
  "Home GUI authorized connector attach path no longer keeps wallet ceremony on the sheet",
  attachAuthorizedHomeGuiTargetBlock,
);

assert(
  (homeGuiTemplate.match(/id="launcher"/g) || []).length === 1 &&
    taskbarOpenIndex >= 0 &&
    launcherOpenIndex > taskbarOpenIndex &&
    launcherCloseIndex > launcherOpenIndex &&
    taskbarPrimaryIndex > launcherCloseIndex &&
    taskbarCloseIndex > taskbarPrimaryIndex &&
    launcherMarkup.includes('class="launcher-popover" role="dialog" aria-label="Home launcher"') &&
    !launcherMarkup.includes('aria-modal="true"') &&
    !launcherMarkup.includes('id="close-launcher"') &&
    !launcherMarkup.includes('placeholder="Search Home"'),
  "Home must keep exactly one non-modal launcher surface nested inside the Shelf",
  {
    taskbarOpenIndex,
    launcherOpenIndex,
    launcherCloseIndex,
    taskbarPrimaryIndex,
    taskbarCloseIndex,
  },
);
assert(
  homeGuiTemplate.includes('id="control-centre-quick-open"') &&
    homeGuiTemplate.includes('id="control-centre-spotlight"') &&
    homeGuiTemplate.includes('id="control-centre-inbox-detail"') &&
    homeGuiTemplate.includes('id="control-centre-quick-wallet"') &&
    (homeGuiTemplate.match(/id="wallet-rail"/g) || []).length === 1 &&
    (homeGuiTemplate.match(/id="wallet-rail-frame"/g) || []).length === 1 &&
    (homeGuiTemplate.match(/id="wallet-rail-close"/g) || []).length === 1 &&
    (homeGuiTemplate.match(/id="wallet-rail-open-window"/g) || []).length === 1 &&
    (homeGuiTemplate.match(/id="wallet-rail-approvals"/g) || []).length === 1 &&
    (homeGuiTemplate.match(/id="wallet-rail-privacy"/g) || []).length === 1 &&
    (homeGuiTemplate.match(/id="wallet-rail-settings"/g) || []).length === 1 &&
    (homeGuiTemplate.match(/id="wallet-rail-retry"/g) || []).length === 1 &&
    (homeGuiTemplate.match(/id="inbox-rail"/g) || []).length === 1 &&
    (homeGuiTemplate.match(/id="inbox-rail-frame"/g) || []).length === 1 &&
    (homeGuiTemplate.match(/id="inbox-rail-close"/g) || []).length === 1 &&
    (homeGuiTemplate.match(/id="inbox-rail-open-window"/g) || []).length === 1 &&
    (homeGuiTemplate.match(/id="inbox-rail-refresh"/g) || []).length === 1 &&
    (homeGuiTemplate.match(/id="inbox-rail-retry"/g) || []).length === 1 &&
    (homeGuiTemplate.match(/id="spotlight"/g) || []).length === 1 &&
    (homeGuiTemplate.match(/id="spotlight-input"/g) || []).length === 1 &&
    (homeGuiTemplate.match(/id="spotlight-results"/g) || []).length === 1 &&
    (homeGuiTemplate.match(/id="shortcuts-overlay"/g) || []).length === 1 &&
    (homeGuiTemplate.match(/id="shortcuts-close"/g) || []).length === 1 &&
    (homeGuiTemplate.match(/id="about-overlay"/g) || []).length === 1 &&
    (homeGuiTemplate.match(/id="about-close"/g) || []).length === 1 &&
    (homeGuiTemplate.match(/id="about-version"/g) || []).length === 1 &&
    (homeGuiTemplate.match(/id="about-update"/g) || []).length === 1,
  "Home GUI template is missing the Spotlight search surface",
);
assert(
  shellChromeScript.includes("export function summaryDisplayName(summary)") &&
    shellChromeScript.includes("summary?.identity?.profile?.display_name") &&
    shellChromeScript.includes("summary?.identity?.profile?.handle") &&
    !shellChromeScript.includes("profile_setup_display_name"),
  "Home GUI display-name helper must prefer signed Profile names without setup fallbacks",
);
assert(
  shellControlCentreScript.includes('import { summaryDisplayName } from "./shell-chrome.js') &&
    shellControlCentreScript.includes("whoamiDetail.textContent = summaryDisplayName(summary);") &&
    shellControlCentreScript.includes('quickSpotlightRow?.addEventListener("click"') &&
    shellControlCentreScript.includes("showSpotlight();") &&
    shellControlCentreScript.includes('quickInboxRow?.addEventListener("click"') &&
    shellControlCentreScript.includes("showInboxRail();") &&
    shellControlCentreScript.includes('quickWalletRow?.addEventListener("click"') &&
    shellControlCentreScript.includes("showWalletRail();"),
  "Home Control Centre quick-open rows must use the existing Spotlight, Inbox, and Wallet openers",
);
assert(
  homeGuiScript.includes('document.querySelector("#toolbar-spotlight")?.addEventListener("click", () => {') &&
    homeGuiScript.includes("showSpotlight();") &&
    /event\.code === "Space"[\s\S]*toggleSpotlight\(\);/.test(
      readFileSync(new URL("../capsules/home-gui/browser/shell-keyboard.js", import.meta.url), "utf8"),
    ),
  "Home search must stay wired from both the toolbar button and the keyboard shortcut",
);
assert(
  homeGuiStyle.includes(".control-centre-quick-open {\n  display: none;\n}") &&
    homeGuiStyle.includes(".control-centre-row-label {\n  flex: 0 0 auto;\n  white-space: nowrap;\n}") &&
    homeGuiStyle.includes(".control-centre-row-detail {\n  min-width: 0;\n  color: var(--muted);\n") &&
    homeGuiStyle.includes("@media (max-width: 640px)") &&
    homeGuiStyle.includes(".control-centre-quick-open {\n    display: block;\n  }"),
  "Home mobile Control Centre is missing the donor quick-open layout and truncation rules",
);

{
  const previousSummary = shellCore.shellState.currentSummary;
  const launcherSurface = elementForSelector("#launcher");
  const taskbar = elementForSelector(".taskbar");
  shellCore.shellState.currentSummary = summary;
  elementForSelector("#launcher-search").value = "";
  shellSurface.showLauncher();
  assert(
    launcherSurface.hidden === false &&
      launcherSurface.dataset.open === "true" &&
      taskbar.classList.contains("is-launcher-face"),
    "showLauncher did not open the Shelf face with one launcher presentation state",
    {
      launcherHidden: launcherSurface.hidden,
      launcherOpen: launcherSurface.dataset.open,
      taskbarClasses: [...taskbar.classList.values],
    },
  );
  shellSurface.setDockAutoHide(true);
  assert(
    document.body.classList.contains("dock-autohide") &&
      taskbar.classList.contains("is-launcher-face"),
    "open launcher did not preserve the Shelf face while dock auto-hide is enabled",
    {
      bodyClasses: [...document.body.classList.values],
      taskbarClasses: [...taskbar.classList.values],
    },
  );
  shellSurface.hideLauncher();
  shellSurface.setDockAutoHide(false);
  shellCore.shellState.currentSummary = previousSummary;
  assert(
    launcherSurface.dataset.open === "false" &&
      taskbar.classList.contains("is-launcher-face") === false,
    "hideLauncher did not clear the Shelf face presentation state",
    {
      launcherOpen: launcherSurface.dataset.open,
      taskbarClasses: [...taskbar.classList.values],
    },
  );
}

{
  const identitySummary = {
    authority: { signed_in: true },
    identity: {
      profile: {
        display_name: "Verified Profile Name",
        handle: "verified-profile-handle",
      },
      handle: "outer-handle-must-not-render",
      profile_setup_display_name: "Setup Suggestion",
      principal_id: "principal:should-not-render",
    },
    notifications: {
      attention_count: 3,
      entries: [
        { kind: "wallet_approval_request", action_ref: { action_id: "wallet-approve-request:1" } },
        { kind: "generic", action_ref: { action_id: "generic:1" } },
      ],
    },
    targets: [
      { target: "wallet", title: "Wallet", route: "/apps/wallet/" },
      { target: "inbox", title: "Inbox", route: "/apps/inbox/" },
    ],
  };
  const signedHandleFallbackSummary = {
    authority: { signed_in: true },
    identity: {
      profile: {
        display_name: "   ",
        handle: "profile-handle-fallback",
      },
      handle: "outer-fallback-must-not-render",
      profile_setup_display_name: "Setup-only name",
      principal_id: "principal:fallback",
    },
    notifications: { entries: [] },
    targets: [],
  };
  shellControlCentre.bindControlCentre();
  shellCore.shellState.currentSummary = identitySummary;
  assert(
    shellChrome.summaryDisplayName(identitySummary) === "Verified Profile Name" &&
      shellChrome.summaryDisplayName(signedHandleFallbackSummary) === "profile-handle-fallback",
    "Home shared display-name helper did not prefer signed Profile display_name then profile.handle",
    {
      identity: shellChrome.summaryDisplayName(identitySummary),
      fallback: shellChrome.summaryDisplayName(signedHandleFallbackSummary),
    },
  );
  shellControlCentre.syncControlCentre(identitySummary);
  assert(
    elementForSelector("#control-centre-whoami-detail").textContent === "Verified Profile Name" &&
      elementForSelector("#control-centre-inbox").hidden === false &&
      elementForSelector("#control-centre-inbox").disabled === false &&
      elementForSelector("#control-centre-inbox-detail").textContent === "3 pending" &&
      elementForSelector("#control-centre-quick-wallet").hidden === false &&
      elementForSelector("#control-centre-quick-wallet-detail").textContent === "1 pending",
    "Home Control Centre did not project the shared signed identity and bounded quick-open counts",
    {
      whoami: elementForSelector("#control-centre-whoami-detail").textContent,
      inboxHidden: elementForSelector("#control-centre-inbox").hidden,
      inboxDisabled: elementForSelector("#control-centre-inbox").disabled,
      inboxDetail: elementForSelector("#control-centre-inbox-detail").textContent,
      walletHidden: elementForSelector("#control-centre-quick-wallet").hidden,
      walletDetail: elementForSelector("#control-centre-quick-wallet-detail").textContent,
    },
  );
  shellCore.shellState.currentSummary = signedHandleFallbackSummary;
  shellControlCentre.syncControlCentre(signedHandleFallbackSummary);
  assert(
    elementForSelector("#control-centre-inbox").hidden === true &&
      elementForSelector("#control-centre-inbox").disabled === true &&
      elementForSelector("#control-centre-inbox-detail").textContent === "Unavailable" &&
      elementForSelector("#control-centre-quick-wallet").hidden === true &&
      elementForSelector("#control-centre-whoami-detail").textContent === "profile-handle-fallback" &&
      elementForSelector("#control-centre-whoami-detail").textContent !== "Setup-only name" &&
      elementForSelector("#control-centre-whoami-detail").textContent !== "principal:fallback",
    "Home Control Centre did not hide unavailable quick-open rows or keep setup/principal values out of the signed identity label",
    {
      whoami: elementForSelector("#control-centre-whoami-detail").textContent,
      inboxHidden: elementForSelector("#control-centre-inbox").hidden,
      inboxDisabled: elementForSelector("#control-centre-inbox").disabled,
      inboxDetail: elementForSelector("#control-centre-inbox-detail").textContent,
      walletHidden: elementForSelector("#control-centre-quick-wallet").hidden,
    },
  );
  shellCore.shellState.currentSummary = summary;
}

assert(
  peopleStyle.includes("padding: var(--window-chrome-safe-top, 52px) 12px 16px;"),
  "People unified-sidebar must reserve the Home safe-top inset in its sidebar column",
);
assert(
  !chatStyle.includes("window-chrome-safe-top"),
  "Chat still reserves host titlebar space inside the capsule",
);
assert(
  !inboxSource.includes("window-chrome-safe-top"),
  "Inbox still reserves host titlebar space inside the capsule",
);
assert(
  !/window-action-btn|window-traffic-lights|data-action="(?:minimize|maximize|close)"/.test(
    collaborationChromeSources,
  ),
  "People, Chat, or Inbox reintroduced capsule-owned outer window chrome",
);
assert(
  browserStyle.includes("padding: 8px 10px 8px var(--window-chrome-safe-leading, 96px);"),
  "Browser no longer applies the Home-owned toolbar content inset",
);
assert(
  !homeGuiScript.includes("(label || toolbarFullscreenButton).textContent"),
  "Home toolbar fullscreen control still writes visible label text into the menubar",
);
assert(
  /requestHome\("home:ui-preference", \{\s*action: "write",\s*key,\s*value,\s*\}\)/m.test(
    homeGuiShellScript,
  ) &&
    !homeGuiScript.includes('window.addEventListener("elastos:ui-preference-changed"') &&
    homeGuiScript.includes(
      "applyHomeGuiUiPreferences(homeGuiUiPreferencesFromSummary(summary));",
    ),
  "Home GUI must send cosmetic writes through the verified host and apply only Runtime-returned canonical state from summary",
);
assert(
  homeShellHostScript.includes('data.type === "home:ui-preference"') &&
    homeShellHostScript.includes('context.targetId !== HOME_GUI_SHELL_ID') &&
    homeShellHostScript.includes('fetchJson("/api/apps/home/appearance/preferences"') &&
    homeShellHostScript.includes("accent_custom") &&
    !homeShellHostScript.includes("elastos.home.appearance-cache.v1"),
  "Home host must keep one Runtime-backed appearance record, reject non-Home-GUI writers, and avoid a second browser cache store",
);
assert(
  controlCentreScript.includes('window.dispatchEvent(new CustomEvent("elastos:ui-preference-changed"') &&
    !controlCentreScript.includes("window.elastosTheme.set(option.dataset.themeOption)") &&
    !controlCentreScript.includes("setFocusModeEnabled(next)") &&
    !controlCentreScript.includes("setUiSoundsEnabled(next)") &&
    !controlCentreScript.includes("setDockAutoHide(next)"),
  "Control Centre must issue preference requests without treating local mutations as canonical state",
);
assert(
  homeGuiScript.includes("const active = Boolean(fullscreenElement());") &&
    homeGuiScript.includes("document.addEventListener(\"fullscreenchange\", syncFullscreenButton);") &&
    homeGuiScript.includes("document.addEventListener(\"webkitfullscreenchange\", syncFullscreenButton);") &&
    homeGuiScript.includes("await exit.call(document);") &&
    homeGuiScript.includes("await request.call(root);"),
  "Home Control Centre fullscreen action no longer follows the real browser Fullscreen API state",
);
assert(
  !homeGuiScript.includes("toggleActiveFullscreenStage") &&
    !homeGuiScript.includes("Window fullscreen stage (dedicated Space)"),
  "Home Control Centre fullscreen action still routes through the app-window fullscreen Space path",
);
assert(
  homeGuiScript.includes("bindSetupSheet();") &&
    homeGuiScript.includes("syncSetupSheet(previous, summary);") &&
    homeGuiScript.includes("holdHomeSetupAct,") &&
    homeGuiScript.includes('"#setup-sheet"'),
  "Home shell must wire the setup sheet through the existing shell summary and window-hook boundaries",
);
assert(
  homeGuiTemplate.includes('id="setup-sheet"') &&
    homeGuiTemplate.includes("Save a Recovery Kit, then create your Profile.") &&
    homeGuiTemplate.includes('id="setup-sheet-recovery"') &&
    homeGuiTemplate.includes('id="setup-sheet-profile"') &&
    homeGuiTemplate.indexOf('id="setup-sheet-step-recovery"') <
      homeGuiTemplate.indexOf('id="setup-sheet-step-profile"'),
  "Home setup sheet must keep the Recovery-first order and the bounded setup actions",
);
assert(
  homeSetupSheetScript.includes('const PROFILE_READINESS_SCHEMA = "elastos.profile.readiness/v1";') &&
    homeSetupSheetScript.includes('const RECOVERY_READINESS_SCHEMA = "elastos.recovery.readiness/v1";') &&
    homeSetupSheetScript.includes('openTarget("system", { query: { settings: "security" } });') &&
    homeSetupSheetScript.includes('openTarget("people");') &&
    homeSetupSheetScript.includes('const SETUP_HOLD_TARGETS = new Set(["chat-room"]);') &&
    homeSetupSheetScript.includes('return status !== "ready" && status !== "signed_out";') &&
    !homeSetupSheetScript.includes("principal_id") &&
    !homeSetupSheetScript.includes("credential_id") &&
    !homeSetupSheetScript.includes("localStorage") &&
    !homeSetupSheetScript.includes("indexedDB") &&
    !homeSetupSheetScript.includes("sessionStorage") &&
    !homeSetupSheetScript.includes("createInitialProfile") &&
    !homeSetupSheetScript.includes("skip") &&
    !homeSetupSheetScript.includes("fallback"),
  "Home setup must use typed Runtime readiness only, hold Chat only, and open System or People without local fallback state",
);
assert(
  homeSetupSheetScript.includes('recoveryButton.textContent = unavailable') &&
    homeSetupSheetScript.includes('? "Open System"') &&
    homeSetupSheetScript.includes('profileButton.disabled = unavailable || profileReady || !recoveryReady || !targetById(summary, "people");') &&
    homeSetupSheetScript.includes('const next = unavailable || homeRecoveryStatus(shellState.currentSummary) !== "ready"') &&
    homeSetupSheetScript.includes('? recoveryButton') &&
    homeSetupSheetScript.includes(': profileButton;'),
  "Unavailable setup state must route only to System and must not enable Profile",
);
assert(
  homeSetupSheetScript.includes('rememberChromeNotification({') &&
    homeSetupSheetScript.includes('kind: "home_setup"') &&
    homeSetupSheetScript.includes('sheet.setAttribute("aria-hidden", "false");') &&
    homeSetupSheetScript.includes('sheet.setAttribute("aria-hidden", "true");') &&
    homeSetupSheetScript.includes('sheet.setAttribute("aria-modal", "false");') &&
    homeSetupSheetScript.includes('sheet.setAttribute("aria-modal", "true");') &&
    homeSetupSheetScript.includes('if (event.key === "Escape")') &&
    homeSetupSheetScript.includes('window.addEventListener("pointermove", onSetupCardPointerMove);') &&
    homeSetupSheetScript.includes('window.addEventListener("pointerup", onSetupCardPointerUp);'),
  "Home setup must keep the session reminder, accessibility state, Escape close, and yielded drag behavior wired through Home chrome",
);
assert(
  homeGuiStyle.includes(".setup-sheet {") &&
    homeGuiStyle.includes("width: min(92vw, 420px);") &&
    homeGuiStyle.includes("width: min(92vw, 360px);") &&
    homeGuiStyle.includes("@media (prefers-reduced-motion: reduce)") &&
    homeGuiStyle.includes(".setup-sheet-step .el-button") &&
    homeGuiStyle.includes(".setup-sheet-step-body"),
  "Home setup layout must stay bounded on narrow widths and keep reduced-motion behavior",
);
assert(
  canonicalWindowHeadMarkup.test(homeGuiTemplate),
  "Home window template must keep one traffic-light group in close/minimize/maximize order before the draggable title and balance shim",
);
assert(
  (homeGuiTemplate.match(/id="launcher"/g) || []).length === 1 &&
    launcherMarkup.includes('class="launcher-popover" role="dialog" aria-label="Home launcher"') &&
    !launcherMarkup.includes('aria-modal="true"') &&
    !launcherMarkup.includes('id="close-launcher"') &&
    !launcherMarkup.includes('placeholder="Search Home"'),
  "Home launcher template must keep one Shelf face without modal overlay claims",
);
assert(
  existsSync(launcherDarkDockIcon) && existsSync(launcherLightDockIcon),
  "Home launcher dock raster assets must exist for both themes",
);
assert(
  homeGuiStyle.includes("@media (max-width: 1100px)") &&
    homeGuiStyle.includes(".toolbar-active-title") &&
    homeGuiStyle.includes("display: none;"),
  "Home toolbar still lacks the narrow-width title fallback",
);
assert(
  homeGuiTemplate.includes('id="toolbar-system"') &&
    homeGuiTemplate.includes('id="toolbar-active-title"') &&
    homeGuiTemplate.includes('id="toolbar-menubar"') &&
    homeGuiTemplate.includes('id="toolbar-control-centre"') &&
    homeGuiTemplate.includes('id="control-centre-fullscreen"') &&
    homeGuiTemplate.includes('id="identity-menu-sign-out"') &&
    !homeGuiTemplate.includes('id="toolbar-fullscreen"') &&
    !homeGuiTemplate.includes('id="toolbar-sign-out"'),
  "Home template still mixes the obsolete naked top bar with the canonical system bar structure",
);
assert(
  homeGuiStyle.includes("--toolbar-h: 36px;") &&
    homeGuiStyle.includes(".toolbar-status-cluster") === false,
  "Home toolbar geometry contract drifted from the bounded 36px system bar",
);

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
  syncMenubar: () => {},
  launchTarget: async (target, query) => {
    restoredBrowserLaunches.push({ target, query: { ...query } });
    if (restoredBrowserLaunches.length === 2) {
      throw new Error("simulated Browser authority renewal failure");
    }
    const launchToken = restoredBrowserLaunches.length === 1
      ? "browser-window-close-token"
      : `browser-window-renewed-token-${restoredBrowserLaunches.length}`;
    const routeBase = target === "browser" ? "/apps/browser/" : `/apps/${target}/`;
    const title = ({
      people: "People",
      "chat-room": "Chat",
      inbox: "Inbox",
      wallet: "Wallet",
      browser: "Browser",
    })[target] || target;
    return {
      target,
      title,
      route:
        `${routeBase}?browser_instance=${encodeURIComponent(query.browser_instance || "")}` +
        `#home_token=${launchToken}`,
      attach_kind: "iframe",
      launch_status: "launched",
    };
  },
});

{
  const spotlight = elementForSelector("#spotlight");
  const spotlightPanel = spotlight.querySelector(".spotlight-panel");
  const spotlightInput = elementForSelector("#spotlight-input");
  const spotlightResults = elementForSelector("#spotlight-results");
  const spotlightInvoker = elementForSelector("#toolbar-spotlight");
  const previousSummary = shellCore.shellState.currentSummary;
  const previousMounted = shellCore.shellState.homeGuiMounted;
  const previousRecents = shellCore.shellState.recentTargetIds;
  spotlight.hidden = true;
  spotlight.inert = true;
  spotlight.setAttribute("aria-hidden", "true");
  spotlightPanel.hidden = true;
  spotlightPanel.className = "spotlight-panel";
  spotlightPanel.classList.add("spotlight-panel");
  spotlightInput.tagName = "INPUT";
  spotlightResults.hidden = true;
  shellCore.shellState.currentSummary = {
    authority: { signed_in: true },
    targets: [
      { target: "system", title: "System", route: "/apps/system/" },
      { target: "browser", title: "Browser", route: "/apps/browser/" },
    ],
    documents: [],
  };
  shellCore.shellState.recentTargetIds = ["system", "browser"];
  shellCore.shellState.homeGuiMounted = true;
  shellSpotlight.bindSpotlight();
  shellKeyboard.bindShellKeyboard();
  spotlightInvoker.focus();
  const openEvent = sendDocumentEvent("keydown", {
    target: spotlightInvoker,
    currentTarget: document,
    code: "Space",
    key: " ",
    metaKey: true,
    ctrlKey: false,
    altKey: false,
    shiftKey: false,
  });
  assert(openEvent.defaultPrevented === true, "Home keyboard search shortcut did not consume the event");
  assert(
    shellSpotlight.spotlightOpen() === true &&
      spotlight.hidden === false &&
      spotlightInput === document.activeElement &&
      spotlight.getAttribute("aria-hidden") === "false",
    "Home Spotlight did not open and focus the search field",
    {
      hidden: spotlight.hidden,
      activeElement: document.activeElement?.selector || null,
      ariaHidden: spotlight.getAttribute("aria-hidden"),
    },
  );
  spotlightInput.value = "system";
  spotlightInput.dispatch("input");
  assert(
    spotlightResults.children.length >= 2 &&
      spotlightResults.children[1].children[1]?.textContent === "System",
    "Home Spotlight did not show the known app search result",
    spotlightResults.children.map((child) => ({
      className: child.className,
      textContent: child.textContent,
      title: child.children[1]?.textContent || null,
    })),
  );
  const launchesBefore = restoredBrowserLaunches.length;
  spotlight.dispatch("keydown", { key: "Enter" });
  spotlightPanel.dispatch("animationend", { target: spotlightPanel });
  await Promise.resolve();
  await Promise.resolve();
  assert(
    restoredBrowserLaunches.length === launchesBefore + 1 &&
      restoredBrowserLaunches.at(-1)?.target === "system" &&
      shellSpotlight.spotlightOpen() === false,
    "Home Spotlight did not open the selected app and close",
    restoredBrowserLaunches.slice(launchesBefore),
  );
  spotlightInvoker.focus();
  shellSpotlight.showSpotlight();
  spotlightInput.value = "no-match-query";
  spotlightInput.dispatch("input");
  assert(
    spotlightResults.hidden === false &&
      spotlightResults.children.length === 1 &&
      spotlightResults.children[0].className === "spotlight-empty" &&
      spotlightResults.children[0].textContent.includes("No results for"),
    "Home Spotlight did not render the unmatched query state",
    spotlightResults.children.map((child) => ({
      className: child.className,
      textContent: child.textContent,
    })),
  );
  spotlight.dispatch("keydown", { key: "Escape" });
  spotlightPanel.dispatch("animationend", { target: spotlightPanel });
  assert(
    shellSpotlight.spotlightOpen() === false &&
      document.activeElement === spotlightInvoker,
    "Home Spotlight did not close on Escape and restore focus",
    {
      hidden: spotlight.hidden,
      activeElement: document.activeElement?.selector || null,
    },
  );
  shellCore.shellState.windows.clear();
  shellCore.shellState.activeWindowId = null;
  shellCore.shellState.currentSummary = previousSummary;
  shellCore.shellState.homeGuiMounted = previousMounted;
  shellCore.shellState.recentTargetIds = previousRecents;
}
for (const launch of [
  { target: "people", title: "People", route: "/apps/people/", attach_kind: "iframe", launch_status: "launched" },
  { target: "chat", title: "Chat", route: "/apps/chat/", attach_kind: "iframe", launch_status: "launched" },
  { target: "chat-room", title: "Chat", route: "/apps/chat-room/", attach_kind: "iframe", launch_status: "launched" },
  { target: "inbox", title: "Inbox", route: "/apps/inbox/", attach_kind: "iframe", launch_status: "launched" },
  { target: "wallet", title: "Wallet", route: "/apps/wallet/", attach_kind: "iframe", launch_status: "launched" },
  { target: "browser", title: "Browser", route: "/apps/browser/", attach_kind: "iframe", launch_status: "launched" },
]) {
  await shellWindows.attachAuthorizedTarget(launch);
}

const expectedWindowChrome = {
  people: {
    mode: "unified-sidebar",
    className: "window-chrome-unified-sidebar",
  },
  chat: {
    mode: "unified-sidebar",
    className: "window-chrome-unified-sidebar",
  },
  "chat-room": {
    mode: "unified-sidebar",
    className: "window-chrome-unified-sidebar",
  },
  inbox: {
    mode: "unified-sidebar",
    className: "window-chrome-unified-sidebar",
  },
  wallet: {
    mode: "standard",
    className: "window-chrome-continuous",
  },
  browser: {
    mode: "unified-toolbar",
    className: "window-chrome-unified-toolbar",
  },
};

for (const entry of shellCore.shellState.windows.values()) {
  const expected = expectedWindowChrome[entry.targetId];
  assert(
    entry.node.dataset.windowChromeMode === expected.mode,
    "first-party window did not receive the intended Home chrome mode",
    { target: entry.targetId, mode: entry.node.dataset.windowChromeMode, expected },
  );
  if (expected.className) {
    assert(
      entry.node.classList.contains(expected.className),
      "first-party window did not receive the expected Home chrome class",
      { target: entry.targetId, classes: [...entry.node.classList.values] },
    );
  }
}

const browserEntry = [...shellCore.shellState.windows.values()].find((entry) => entry.targetId === "browser");
const browserClose = browserEntry.node.querySelector("[data-action='close']");
browserClose.focus();
shellWindows.minimizeWindow(browserEntry.id);
assert(
  browserEntry.node.classList.contains("hidden") &&
    browserEntry.node.inert === true &&
    browserEntry.node.getAttribute("aria-hidden") === "true" &&
    !browserEntry.node.contains(document.activeElement),
  "hiding a focused window left focus inside aria-hidden or non-inert chrome",
  {
    activeElement: document.activeElement?.selector || null,
    hidden: browserEntry.node.classList.contains("hidden"),
    inert: browserEntry.node.inert,
    ariaHidden: browserEntry.node.getAttribute("aria-hidden"),
  },
);
const focusedVisibleWindow = [...shellCore.shellState.windows.values()].find(
  (entry) =>
    !entry.node.classList.contains("hidden") &&
    entry.node.contains(document.activeElement),
);
assert(
  focusedVisibleWindow ||
    document.activeElement?.selector === "#toolbar-home" ||
    document.activeElement?.selector === "#launcher-toggle" ||
    document.activeElement?.selector === "body",
  "hiding the active window did not move focus to a visible Home-owned window or stable shell control",
  {
    activeElement: document.activeElement?.selector || null,
    focusedVisibleTarget: focusedVisibleWindow?.targetId || null,
  },
);

shellCore.shellState.windows.clear();
shellCore.shellState.activeWindowId = null;
restoredBrowserLaunches.length = 0;
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
const restoredBrowserEntry = [...shellCore.shellState.windows.values()][0];
const restoredBrowserFrame = restoredBrowserEntry.node.querySelector(".window-frame");
const browserCloseMessages = [];
restoredBrowserFrame.contentWindow = {
  postMessage(message, origin) {
    browserCloseMessages.push({ message, origin });
  },
};
const originalBrowserWindow = {
  entry: restoredBrowserEntry,
  frame: restoredBrowserFrame,
  node: restoredBrowserEntry.node,
  route: restoredBrowserFrame.dataset.route,
};
const expiredAuthorityClose = shellWindows.closeWindow(restoredBrowserEntry.id);
const expiredAuthorityCloseRequest = browserCloseMessages.at(-1);
assert(
  expiredAuthorityCloseRequest?.message.homeToken ===
    "browser-window-close-token" &&
    shellCore.shellState.windows.has(restoredBrowserEntry.id),
  "expired Browser authority close did not remain nonterminal before renewal",
  { expiredAuthorityCloseRequest },
);
const failedRenewal = shellWindows.renewBrowserWindowAuthority(
  restoredBrowserEntry.id,
);
const duplicateFailedRenewal = shellWindows.renewBrowserWindowAuthority(
  restoredBrowserEntry.id,
);
const failedRenewals = await Promise.allSettled([
  failedRenewal,
  duplicateFailedRenewal,
]);
assert(
  failedRenewal === duplicateFailedRenewal &&
    failedRenewals.every(
      (result) =>
        result.status === "rejected" &&
        result.reason?.message ===
          "simulated Browser authority renewal failure",
    ) &&
    restoredBrowserLaunches.length === 2 &&
    restoredBrowserFrame.dataset.route === originalBrowserWindow.route &&
    restoredBrowserEntry.browserCloseRequest &&
    shellCore.shellState.windows.has(restoredBrowserEntry.id),
  "failed Browser authority renewal changed the old frame or duplicated launch",
  {
    launches: restoredBrowserLaunches,
    route: restoredBrowserFrame.dataset.route,
    failedRenewals,
  },
);
const successfulRenewal = await shellWindows.renewBrowserWindowAuthority(
  restoredBrowserEntry.id,
);
assert(
  successfulRenewal?.browserInstance ===
    "browser:restored-refresh-regression" &&
    successfulRenewal?.freshHomeToken ===
      "browser-window-renewed-token-3" &&
    await expiredAuthorityClose === false &&
    restoredBrowserLaunches.length === 3 &&
    restoredBrowserLaunches[2].query.browser_instance ===
      "browser:restored-refresh-regression" &&
    shellCore.shellState.windows.get(restoredBrowserEntry.id) ===
      originalBrowserWindow.entry &&
    restoredBrowserEntry.node === originalBrowserWindow.node &&
    restoredBrowserEntry.node.querySelector(".window-frame") ===
      originalBrowserWindow.frame &&
    restoredBrowserFrame.dataset.route.includes(
      "browser_instance=browser%3Arestored-refresh-regression",
    ) &&
    restoredBrowserFrame.dataset.route.endsWith(
      "#home_token=browser-window-renewed-token-3",
    ) &&
    browserCloseMessages.length === 1 &&
    restoredBrowserEntry.node.dataset.browserCloseState === "retry",
  "expired Browser authority close blocked in-place renewal of the active owner",
  {
    launches: restoredBrowserLaunches,
    route: restoredBrowserFrame.dataset.route,
    windows: [...shellCore.shellState.windows.keys()],
  },
);
const renewedBrowserToken = "browser-window-renewed-token-3";
const firstBrowserClose = shellWindows.closeWindow(restoredBrowserEntry.id);
let firstBrowserCloseSettled = false;
firstBrowserClose.finally(() => {
  firstBrowserCloseSettled = true;
});
const firstBrowserCloseRequest = browserCloseMessages.at(-1);
assert(
  firstBrowserCloseRequest?.origin === "*" &&
    Object.keys(firstBrowserCloseRequest.message).sort().join(",") ===
      "browserInstance,homeToken,requestId,type" &&
    firstBrowserCloseRequest.message.type ===
      "elastos.browser.window-close.request/v1" &&
    firstBrowserCloseRequest.message.homeToken === renewedBrowserToken &&
    firstBrowserCloseRequest.message.browserInstance ===
      "browser:restored-refresh-regression" &&
    shellCore.shellState.windows.has(restoredBrowserEntry.id),
  "explicit Browser close did not retain the frame while requesting exact cleanup",
  { firstBrowserCloseRequest, windows: [...shellCore.shellState.windows.keys()] },
);
const pendingResult = {
  type: "elastos.browser.window-close.result/v1",
  requestId: firstBrowserCloseRequest.message.requestId,
  homeToken: renewedBrowserToken,
  browserInstance: "browser:restored-refresh-regression",
  state: "pending",
  pageId: "page-1",
  generation: 1,
  cleanupId: "cleanup-1",
  terminalKind: "",
  reason: "transport_failure",
};
sendWindowEvent("message", {
  origin: "null",
  source: { postMessage() {} },
  data: { ...pendingResult, state: "terminal", terminalKind: "closed", reason: "" },
});
assert(
  shellCore.shellState.windows.has(restoredBrowserEntry.id),
  "a terminal receipt from the wrong iframe source removed Browser",
);
sendWindowEvent("message", {
  origin: "null",
  source: restoredBrowserFrame.contentWindow,
  data: {
    ...pendingResult,
    pageId: "page-substituted-before-binding",
    state: "terminal",
    terminalKind: "already_absent",
    reason: "",
  },
});
assert(
  shellCore.shellState.windows.has(restoredBrowserEntry.id),
  "an unbound immediate terminal receipt removed Browser",
);
sendWindowEvent("message", {
  origin: "null",
  source: restoredBrowserFrame.contentWindow,
  data: pendingResult,
});
await Promise.resolve();
assert(
  firstBrowserCloseSettled === false &&
    shellCore.shellState.windows.has(restoredBrowserEntry.id) &&
    restoredBrowserEntry.node.dataset.browserCloseState === "pending" &&
    restoredBrowserEntry.node
      .querySelector("[data-action='close']")
      .disabled === true &&
    restoredBrowserEntry.node
      .querySelector("[data-action='close']")
      .getAttribute("aria-label") === "Close",
  "nonterminal Browser cleanup ended the close handshake before Runtime settled",
  {
    state: restoredBrowserEntry.node.dataset.browserCloseState,
    windows: [...shellCore.shellState.windows.keys()],
  },
);
assert(
  shellWindows.closeWindow(restoredBrowserEntry.id) === firstBrowserClose &&
    browserCloseMessages.length === 2,
  "a duplicate close replaced the live Home-to-Browser request",
);
for (const substitutedIdentity of [
  { pageId: "page-wrong" },
  { generation: 2 },
  { cleanupId: "cleanup-wrong" },
]) {
  sendWindowEvent("message", {
    origin: "null",
    source: restoredBrowserFrame.contentWindow,
    data: {
      ...pendingResult,
      ...substitutedIdentity,
      state: "terminal",
      terminalKind: "already_absent",
      reason: "",
    },
  });
}
await Promise.resolve();
assert(
  firstBrowserCloseSettled === false &&
    shellCore.shellState.windows.has(restoredBrowserEntry.id),
  "a terminal receipt for a substituted Browser lifecycle removed the window",
);
sendWindowEvent("message", {
  origin: "null",
  source: restoredBrowserFrame.contentWindow,
  data: {
    ...pendingResult,
    state: "terminal",
    terminalKind: "already_absent",
    reason: "",
  },
});
assert(
  await firstBrowserClose === true &&
    !shellCore.shellState.windows.has(restoredBrowserEntry.id),
  "exact delayed already-absent Browser cleanup did not remove the window",
  { firstBrowserCloseRequest, windows: [...shellCore.shellState.windows.keys()] },
);
shellCore.shellState.activeWindowId = null;
shellCore.shellState.homeBrowserState.session = null;

await withCapturedFrameRevealTimers(async ({ timerIds }) => {
  const loadRevealEntry = await shellWindows.attachAuthorizedTarget({
    target: "people",
    title: "People",
    route: "/apps/people/#home_token=deterministic-people-load-reveal",
    attach_kind: "iframe",
    launch_status: "launched",
  });
  const loadRevealFrame = loadRevealEntry.node.querySelector(".window-frame");
  assert(typeof loadRevealFrame.onload === "function", "launched window did not assign a frame onload handler");
  const loadRevealTimerId = Number(loadRevealFrame.dataset.frameRevealTimer || "");
  assert(
    Number.isFinite(loadRevealTimerId) &&
      timerIds().includes(loadRevealTimerId) &&
      loadRevealFrame.dataset.frameVisible !== "true" &&
      loadRevealFrame.dataset.frameVisibleCause === undefined,
    "frame reveal was not deferred behind the existing onload/timer path",
    {
      frameVisible: loadRevealFrame.dataset.frameVisible,
      frameVisibleCause: loadRevealFrame.dataset.frameVisibleCause,
      frameRevealTimerId: loadRevealTimerId,
      timerIds: timerIds(),
    },
  );
  loadRevealFrame.onload();
  assert(
    loadRevealFrame.dataset.frameVisible === "true" &&
      loadRevealFrame.dataset.frameVisibleCause === "load" &&
      !timerIds().includes(loadRevealTimerId),
    "frame onload did not reveal the window with a load cause",
    {
      frameVisible: loadRevealFrame.dataset.frameVisible,
      frameVisibleCause: loadRevealFrame.dataset.frameVisibleCause,
      frameRevealTimerId: loadRevealTimerId,
      timerIds: timerIds(),
    },
  );
  shellWindows.closeWindow(loadRevealEntry.id);
});

await withCapturedFrameRevealTimers(async ({ timerIds, fire }) => {
  const timeoutRevealEntry = await shellWindows.attachAuthorizedTarget({
    target: "inbox",
    title: "Inbox",
    route: "/apps/inbox/#home_token=deterministic-inbox-timeout-reveal",
    attach_kind: "iframe",
    launch_status: "launched",
  });
  const timeoutRevealFrame = timeoutRevealEntry.node.querySelector(".window-frame");
  const timeoutTimerId = Number(timeoutRevealFrame.dataset.frameRevealTimer || "");
  assert(
    Number.isFinite(timeoutTimerId) && timerIds().includes(timeoutTimerId),
    "timeout reveal path did not arm a frame reveal timer",
    { frameRevealTimerId: timeoutTimerId, timerIds: timerIds() },
  );
  fire(timeoutTimerId);
  assert(
    timeoutRevealFrame.dataset.frameVisible === "true" &&
      timeoutRevealFrame.dataset.frameVisibleCause === "timeout",
    "frame timeout reveal did not record a timeout cause",
    {
      frameVisible: timeoutRevealFrame.dataset.frameVisible,
      frameVisibleCause: timeoutRevealFrame.dataset.frameVisibleCause,
      timerIds: timerIds(),
    },
  );
  shellWindows.closeWindow(timeoutRevealEntry.id);
});
shellCore.shellState.windows.clear();
shellCore.shellState.activeWindowId = null;

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

shellWalletRail.bindWalletRail();
shellConnectorSheet.bindConnectorSheet();
const walletRailNode = document.querySelector("#wallet-rail");
walletRailNode.hidden = true;
walletRailNode.inert = true;
const boundWalletRailFrame = shellWalletRail.walletRailFrame();
boundWalletRailFrame.hidden = true;
boundWalletRailFrame.dataset.route =
  "/apps/wallet/?home_origin=http%3A%2F%2Flocalhost%3A61180#home_token=wallet-rail-token";
shellWalletRail.syncWalletRailAvailability(summary);
const connectorSheetNode = document.querySelector("#connector-sheet");
connectorSheetNode.hidden = true;
connectorSheetNode.inert = true;
const windowsBeforeAuthorizedConnectorSheet = shellCore.shellState.windows.size;
const requestsBeforeAuthorizedConnectorSheet = requests.length;
const closedRailConnectorSheet = await shellConnectorSheet.attachAuthorizedConnectorSheet({
  target: "wallet-metamask",
  title: "MetaMask",
  route:
    "/apps/wallet-metamask/?home_origin=http%3A%2F%2Flocalhost%3A61180" +
    "#home_token=authorized-metamask-token",
  attach_kind: "iframe",
  launch_status: "launched",
});
assert(
  closedRailConnectorSheet === false &&
    connectorSheetNode.hidden === true &&
    shellCore.shellState.windows.size === windowsBeforeAuthorizedConnectorSheet &&
    requests.length === requestsBeforeAuthorizedConnectorSheet,
  "direct authorized connector sheet attachment with a closed Wallet rail did not fail closed",
  {
    closedRailConnectorSheet,
    hidden: connectorSheetNode.hidden,
    windows: [...shellCore.shellState.windows.keys()],
    newRequests: requests.slice(requestsBeforeAuthorizedConnectorSheet),
  },
);
shellWalletRail.showWalletRail();
const attachedConnectorSheet = await shellConnectorSheet.attachAuthorizedConnectorSheet({
  target: "wallet-metamask",
  title: "MetaMask",
  route:
    "/apps/wallet-metamask/?home_origin=http%3A%2F%2Flocalhost%3A61180" +
    "#home_token=authorized-metamask-token",
  attach_kind: "iframe",
  launch_status: "launched",
});
assert(
  attachedConnectorSheet === true &&
    shellConnectorSheet.connectorSheetOpen() &&
    shellConnectorSheet.connectorSheetTarget() === "wallet-metamask" &&
    connectorSheetNode.hidden === false,
  "authorized connector descriptor did not open the wallet connector sheet",
  {
    attachedConnectorSheet,
    activeTarget: shellConnectorSheet.connectorSheetTarget(),
    hidden: connectorSheetNode.hidden,
  },
);
const connectorSheetFrame = shellConnectorSheet.connectorSheetFrame();
assert(
  connectorSheetFrame.dataset.route ===
    "/apps/wallet-metamask/?home_origin=http%3A%2F%2Flocalhost%3A61180#home_token=authorized-metamask-token" &&
    connectorSheetFrame.getAttribute("src") ===
      "/apps/wallet-metamask/?home_origin=http%3A%2F%2Flocalhost%3A61180#home_token=authorized-metamask-token",
  "authorized connector sheet did not mount the exact descriptor route byte-for-byte",
  {
    route: connectorSheetFrame.dataset.route,
    src: connectorSheetFrame.getAttribute("src"),
  },
);
assert(
  shellCore.shellState.windows.size === windowsBeforeAuthorizedConnectorSheet &&
    requests.length === requestsBeforeAuthorizedConnectorSheet,
  "authorized connector descriptor created a generic connector surface or a second launch",
  {
    windows: [...shellCore.shellState.windows.keys()],
    newRequests: requests.slice(requestsBeforeAuthorizedConnectorSheet),
  },
);
shellConnectorSheet.hideConnectorSheet();
const failedAuthorizedConnectorSheet = await shellConnectorSheet.attachAuthorizedConnectorSheet({
  target: "wallet-metamask",
  title: "MetaMask",
  route: "",
  attach_kind: "iframe",
  launch_status: "launched",
});
assert(
  failedAuthorizedConnectorSheet === false &&
    connectorSheetNode.hidden === true &&
    !connectorSheetFrame.dataset.route &&
    shellCore.shellState.windows.size === windowsBeforeAuthorizedConnectorSheet &&
    requests.length === requestsBeforeAuthorizedConnectorSheet,
  "failed authorized connector sheet attachment did not fail closed",
  {
    failedAuthorizedConnectorSheet,
    hidden: connectorSheetNode.hidden,
    route: connectorSheetFrame.dataset.route || null,
    windows: [...shellCore.shellState.windows.keys()],
    newRequests: requests.slice(requestsBeforeAuthorizedConnectorSheet),
  },
);

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
