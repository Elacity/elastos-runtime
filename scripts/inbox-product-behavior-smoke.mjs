#!/usr/bin/env node

import assert from "node:assert/strict";
import fs from "node:fs";
import vm from "node:vm";

class FakeClassList {
  constructor(owner) {
    this.owner = owner;
  }

  add(name) {
    this.toggle(name, true);
  }

  remove(name) {
    this.toggle(name, false);
  }

  toggle(name, enabled) {
    const names = new Set(String(this.owner.className || "").split(/\s+/).filter(Boolean));
    enabled ? names.add(name) : names.delete(name);
    this.owner.className = [...names].join(" ");
  }
}

class FakeElement {
  constructor(tagName) {
    this.tagName = String(tagName).toUpperCase();
    this.children = [];
    this.className = "";
    this.classList = new FakeClassList(this);
    this.dataset = {};
    this.hidden = false;
    this.open = false;
    this.textContent = "";
    this.attributes = new Map();
    this.listeners = new Map();
    this.disabled = false;
    this.type = "";
  }

  append(...children) {
    this.children.push(...children);
  }

  appendChild(child) {
    this.children.push(child);
    return child;
  }

  replaceChildren(...children) {
    this.children = [...children];
  }

  addEventListener(type, callback) {
    const listeners = this.listeners.get(type) || [];
    listeners.push(callback);
    this.listeners.set(type, listeners);
  }

  setAttribute(name, value) {
    this.attributes.set(name, String(value));
  }

  getAttribute(name) {
    return this.attributes.get(name) || null;
  }

  querySelectorAll(selector) {
    const nodes = descendants(this).slice(1);
    if (selector === "button") {
      return nodes.filter((node) => node.tagName === "BUTTON");
    }
    if (selector === ".entry-row") {
      return nodes.filter((node) => String(node.className || "").split(/\s+/).includes("entry-row"));
    }
    return [];
  }

  focus() {}

  scrollIntoView() {}
}

function descendants(node) {
  return [node, ...(node.children || []).flatMap(descendants)];
}

function jsonResponse(payload) {
  return {
    ok: true,
    async json() {
      return payload;
    },
    async text() {
      return JSON.stringify(payload);
    },
  };
}

function plainJson(value) {
  return JSON.parse(JSON.stringify(value));
}

function inboxSummary() {
  const now = Math.floor(Date.now() / 1000);
  return {
    notifications: {
      attention_count: 1,
      unread_count: 0,
      entries: [
        {
          id: "entry-review",
          kind: "contact_request",
          title: "Review request",
          body: "Contact approval needs review",
          severity: "attention",
          read: true,
          created_at: now - 60,
          source_app: "people",
          action_ref: { action_id: "contact-accept-request:entry-review" },
        },
        {
          id: "entry-wallet",
          kind: "wallet_approval_request",
          title: "Wallet request",
          body: "Review wallet request",
          severity: "info",
          read: true,
          created_at: now - 3600,
          source_app: "wallet",
          action_ref: { action_id: "wallet-review-request:wallet-request-1" },
        },
      ],
    },
  };
}

async function settle() {
  for (let attempt = 0; attempt < 8; attempt += 1) {
    await Promise.resolve();
    await new Promise((resolve) => setTimeout(resolve, 0));
  }
}

function extractInboxScript(html) {
  const match = html.match(/<script>([\s\S]*)<\/script>\s*<\/body>/);
  assert(match, "Inbox inline script not found");
  return match[1];
}

async function runInboxHomeChromeSmoke() {
  const inbox = fs.readFileSync(new URL("../capsules/inbox/browser/index.html", import.meta.url), "utf8");
  const inboxScript = extractInboxScript(inbox);
  const topMessages = [];
  const parentMessages = [];
  const fetchCalls = [];
  const filterAll = new FakeElement("button");
  filterAll.className = "summary-card active";
  filterAll.dataset.filter = "all";
  const filterReview = new FakeElement("button");
  filterReview.className = "summary-card";
  filterReview.dataset.filter = "review";
  const nodes = new Map([
    ["locked-shell", new FakeElement("section")],
    ["inbox-shell", new FakeElement("section")],
    ["status-text", new FakeElement("p")],
    ["pending-count", new FakeElement("strong")],
    ["review-count", new FakeElement("strong")],
    ["refresh", new FakeElement("button")],
    ["entry-rows", new FakeElement("section")],
    ["entry-detail", new FakeElement("section")],
    ["entry-split", new FakeElement("div")],
    ["empty-state", new FakeElement("section")],
    ["empty-title", new FakeElement("h2")],
    ["list-title", new FakeElement("h1")],
  ]);
  const windowListeners = new Map();
  const documentListeners = new Map();
  const topFrame = {
    postMessage(message, origin) {
      topMessages.push({ message, origin });
    },
  };
  const parentFrame = {
    postMessage(message, origin) {
      parentMessages.push({ message, origin });
    },
  };
  const documentElement = { dataset: {} };
  const document = {
    hidden: false,
    documentElement,
    getElementById(id) {
      return nodes.get(id) || null;
    },
    querySelectorAll(selector) {
      if (selector === ".summary-card[data-filter]") {
        return [filterAll, filterReview];
      }
      return [];
    },
    addEventListener(type, callback) {
      documentListeners.set(type, callback);
    },
    createElement(tagName) {
      return new FakeElement(tagName);
    },
  };
  const context = {
    console,
    document,
    window: {
      location: {
        search: "?home_origin=http%3A%2F%2Flocalhost%3A61180&presentation=rail",
        hash: "#home_token=inbox-test-token",
      },
      top: topFrame,
      parent: parentFrame,
      addEventListener(type, callback) {
        const listeners = windowListeners.get(type) || [];
        listeners.push(callback);
        windowListeners.set(type, listeners);
      },
      removeEventListener(type, callback) {
        const listeners = windowListeners.get(type) || [];
        windowListeners.set(type, listeners.filter((entry) => entry !== callback));
      },
      setInterval() {
        return 1;
      },
      setTimeout(callback) {
        callback();
        return 1;
      },
      clearTimeout() {},
    },
    URLSearchParams,
    Intl,
    Date,
    Promise,
    setTimeout,
    clearTimeout,
    fetch: async (path) => {
      fetchCalls.push(path);
      return jsonResponse(inboxSummary());
    },
  };

  vm.runInNewContext(inboxScript, context, {
    filename: "capsules/inbox/browser/index.html:inline",
  });
  await settle();

  const menuManifest = topMessages.find((entry) => entry.message.type === "home:menu-manifest");
  assert.deepEqual(
    plainJson(topMessages.find((entry) => entry.message.type === "home:app-ready")),
    {
      message: { type: "home:app-ready", homeToken: "inbox-test-token" },
      origin: "http://localhost:61180",
    },
  );
  assert.deepEqual(plainJson(menuManifest), {
    message: {
      type: "home:menu-manifest",
      homeToken: "inbox-test-token",
      menus: [
        {
          title: "File",
          items: [
            { label: "Refresh", cmd: "refresh" },
            { label: "Close Window", cmd: "__close-window" },
          ],
        },
        {
          title: "View",
          items: [
            { label: "All Pending", cmd: "filter-all" },
            { label: "Needs Review", cmd: "filter-review" },
          ],
        },
      ],
    },
    origin: "http://localhost:61180",
  });
  assert.deepEqual(
    plainJson(parentMessages),
    [
      {
        message: {
          type: "inbox:pending-count",
          count: 0,
        },
        origin: "*",
      },
    ],
  );
  assert.equal(
    topMessages.some((entry) => entry.message.type === "inbox:pending-count"),
    false,
  );
  assert.equal(documentElement.dataset.inboxPresentation, "rail");
  assert.equal(fetchCalls.filter((path) => path === "/api/apps/inbox/summary").length, 1);

  for (const callback of windowListeners.get("message") || []) {
    callback({
      origin: "null",
      source: parentFrame,
      data: { type: "elastos:menu-command", cmd: "filter-review" },
    });
  }
  await settle();
  assert.equal(nodes.get("list-title").textContent, "Needs Review");
  assert.equal(nodes.get("entry-rows").children.length, 1);

  for (const callback of windowListeners.get("message") || []) {
    callback({
      origin: "null",
      source: parentFrame,
      data: { type: "elastos:menu-command", cmd: "refresh" },
    });
  }
  await settle();
  assert.equal(fetchCalls.filter((path) => path === "/api/apps/inbox/summary").length, 2);

  for (const callback of windowListeners.get("message") || []) {
    callback({
      origin: "null",
      source: parentFrame,
      data: { type: "elastos:inbox-chrome-command", cmd: "refresh" },
    });
  }
  await settle();
  assert.equal(fetchCalls.filter((path) => path === "/api/apps/inbox/summary").length, 3);

  for (const callback of windowListeners.get("message") || []) {
    callback({
      origin: "http://localhost:61180",
      source: parentFrame,
      data: { type: "elastos:menu-command", cmd: "refresh" },
    });
    callback({
      origin: "null",
      source: {},
      data: { type: "elastos:inbox-chrome-command", cmd: "refresh" },
    });
  }
  await settle();
  assert.equal(fetchCalls.filter((path) => path === "/api/apps/inbox/summary").length, 3);

  const originalFetch = context.fetch;
  context.fetch = async (path) => {
    fetchCalls.push(path);
    return jsonResponse({
      notifications: {
        attention_count: 0,
        unread_count: 0,
        entries: [
          {
            id: "entry-wallet",
            kind: "wallet_approval_request",
            title: "Wallet request",
            body: "Review wallet request",
            severity: "info",
            read: true,
            created_at: Math.floor(Date.now() / 1000) - 3600,
            source_app: "wallet",
            action_ref: { action_id: "wallet-review-request:wallet-request-1" },
          },
        ],
      },
    });
  };
  for (const callback of windowListeners.get("message") || []) {
    callback({
      origin: "null",
      source: parentFrame,
      data: { type: "elastos:menu-command", cmd: "refresh" },
    });
  }
  await settle();
  for (const callback of windowListeners.get("message") || []) {
    callback({
      origin: "null",
      source: parentFrame,
      data: { type: "elastos:menu-command", cmd: "filter-review" },
    });
  }
  await settle();
  assert.equal(nodes.get("empty-title").textContent, "No requests need review");
  context.fetch = originalFetch;
}

async function runInboxLaunchSelectionSmoke() {
  const inbox = fs.readFileSync(new URL("../capsules/inbox/browser/index.html", import.meta.url), "utf8");
  const inboxScript = extractInboxScript(inbox);
  const nodes = new Map([
    ["locked-shell", new FakeElement("section")],
    ["inbox-shell", new FakeElement("section")],
    ["status-text", new FakeElement("p")],
    ["pending-count", new FakeElement("strong")],
    ["review-count", new FakeElement("strong")],
    ["refresh", new FakeElement("button")],
    ["entry-rows", new FakeElement("section")],
    ["entry-detail", new FakeElement("section")],
    ["entry-split", new FakeElement("div")],
    ["empty-state", new FakeElement("section")],
    ["empty-title", new FakeElement("h2")],
    ["list-title", new FakeElement("h1")],
  ]);
  const filterAll = new FakeElement("button");
  filterAll.className = "summary-card active";
  filterAll.dataset.filter = "all";
  const filterReview = new FakeElement("button");
  filterReview.className = "summary-card";
  filterReview.dataset.filter = "review";
  const document = {
    hidden: false,
    documentElement: { dataset: {} },
    getElementById(id) {
      return nodes.get(id) || null;
    },
    querySelectorAll(selector) {
      if (selector === ".summary-card[data-filter]") {
        return [filterAll, filterReview];
      }
      return [];
    },
    addEventListener() {},
    createElement(tagName) {
      return new FakeElement(tagName);
    },
  };
  const context = {
    console,
    document,
    window: {
      location: {
        search: "?home_origin=http%3A%2F%2Flocalhost%3A61180&notification_id=entry-wallet",
        hash: "#home_token=inbox-test-token",
      },
      top: { postMessage() {} },
      parent: { postMessage() {} },
      addEventListener() {},
      removeEventListener() {},
      setInterval() {
        return 1;
      },
      setTimeout(callback) {
        callback();
        return 1;
      },
      clearTimeout() {},
    },
    URLSearchParams,
    Intl,
    Date,
    Promise,
    setTimeout,
    clearTimeout,
    fetch: async () => jsonResponse(inboxSummary()),
  };

  vm.runInNewContext(inboxScript, context, {
    filename: "capsules/inbox/browser/index.html:inline",
  });
  await settle();

  assert.equal(nodes.get("entry-rows").children.length, 2);
  assert.equal(nodes.get("entry-rows").children[1].getAttribute("aria-selected"), "true");
  const detailText = descendants(nodes.get("entry-detail"))
    .map((node) => node.textContent || "")
    .join(" ");
  assert(detailText.includes("Wallet request"), "Inbox did not keep the requested notification selected", detailText);
}

async function runInboxMissingLaunchSelectionSmoke() {
  const inbox = fs.readFileSync(new URL("../capsules/inbox/browser/index.html", import.meta.url), "utf8");
  const inboxScript = extractInboxScript(inbox);
  const nodes = new Map([
    ["locked-shell", new FakeElement("section")],
    ["inbox-shell", new FakeElement("section")],
    ["status-text", new FakeElement("p")],
    ["pending-count", new FakeElement("strong")],
    ["review-count", new FakeElement("strong")],
    ["refresh", new FakeElement("button")],
    ["entry-rows", new FakeElement("section")],
    ["entry-detail", new FakeElement("section")],
    ["entry-split", new FakeElement("div")],
    ["empty-state", new FakeElement("section")],
    ["empty-title", new FakeElement("h2")],
    ["list-title", new FakeElement("h1")],
  ]);
  const filterAll = new FakeElement("button");
  filterAll.className = "summary-card active";
  filterAll.dataset.filter = "all";
  const filterReview = new FakeElement("button");
  filterReview.className = "summary-card";
  filterReview.dataset.filter = "review";
  const document = {
    hidden: false,
    documentElement: { dataset: {} },
    getElementById(id) {
      return nodes.get(id) || null;
    },
    querySelectorAll(selector) {
      if (selector === ".summary-card[data-filter]") {
        return [filterAll, filterReview];
      }
      return [];
    },
    addEventListener() {},
    createElement(tagName) {
      return new FakeElement(tagName);
    },
  };
  const context = {
    console,
    document,
    window: {
      location: {
        search: "?home_origin=http%3A%2F%2Flocalhost%3A61180&notification_id=missing-entry",
        hash: "#home_token=inbox-test-token",
      },
      top: { postMessage() {} },
      parent: { postMessage() {} },
      addEventListener() {},
      removeEventListener() {},
      setInterval() {
        return 1;
      },
      setTimeout(callback) {
        callback();
        return 1;
      },
      clearTimeout() {},
    },
    URLSearchParams,
    Intl,
    Date,
    Promise,
    setTimeout,
    clearTimeout,
    fetch: async () => jsonResponse(inboxSummary()),
  };

  vm.runInNewContext(inboxScript, context, {
    filename: "capsules/inbox/browser/index.html:inline",
  });
  await settle();

  const selectedRows = nodes.get("entry-rows").children
    .filter((row) => row.getAttribute("aria-selected") === "true");
  assert.equal(selectedRows.length, 0, "Inbox selected a different request after an explicit missing launch id");
  const detailText = descendants(nodes.get("entry-detail"))
    .map((node) => node.textContent || "")
    .join(" ");
  assert(detailText.includes("no longer available"), "Inbox did not show the explicit request as unavailable", detailText);
  assert.equal(
    descendants(nodes.get("entry-detail")).filter((node) => node.tagName === "BUTTON").length,
    0,
    "Inbox exposed actions for a missing explicit launch id",
  );
}

async function runInboxRemovedLaunchSelectionSmoke() {
  const inbox = fs.readFileSync(new URL("../capsules/inbox/browser/index.html", import.meta.url), "utf8");
  const inboxScript = extractInboxScript(inbox);
  const nodes = new Map([
    ["locked-shell", new FakeElement("section")],
    ["inbox-shell", new FakeElement("section")],
    ["status-text", new FakeElement("p")],
    ["pending-count", new FakeElement("strong")],
    ["review-count", new FakeElement("strong")],
    ["refresh", new FakeElement("button")],
    ["entry-rows", new FakeElement("section")],
    ["entry-detail", new FakeElement("section")],
    ["entry-split", new FakeElement("div")],
    ["empty-state", new FakeElement("section")],
    ["empty-title", new FakeElement("h2")],
    ["list-title", new FakeElement("h1")],
  ]);
  const filterAll = new FakeElement("button");
  filterAll.className = "summary-card active";
  filterAll.dataset.filter = "all";
  const filterReview = new FakeElement("button");
  filterReview.className = "summary-card";
  filterReview.dataset.filter = "review";
  let fetchCount = 0;
  const document = {
    hidden: false,
    documentElement: { dataset: {} },
    getElementById(id) {
      return nodes.get(id) || null;
    },
    querySelectorAll(selector) {
      if (selector === ".summary-card[data-filter]") {
        return [filterAll, filterReview];
      }
      return [];
    },
    addEventListener() {},
    createElement(tagName) {
      return new FakeElement(tagName);
    },
  };
  const context = {
    console,
    document,
    window: {
      location: {
        search: "?home_origin=http%3A%2F%2Flocalhost%3A61180&notification_id=entry-wallet",
        hash: "#home_token=inbox-test-token",
      },
      top: { postMessage() {} },
      parent: { postMessage() {} },
      addEventListener() {},
      removeEventListener() {},
      setInterval() {
        return 1;
      },
      setTimeout(callback) {
        callback();
        return 1;
      },
      clearTimeout() {},
    },
    URLSearchParams,
    Intl,
    Date,
    Promise,
    setTimeout,
    clearTimeout,
    fetch: async () => {
      fetchCount += 1;
      if (fetchCount === 1) {
        return jsonResponse(inboxSummary());
      }
      const summary = inboxSummary();
      summary.notifications.entries = summary.notifications.entries.filter((entry) => entry.id !== "entry-wallet");
      return jsonResponse(summary);
    },
  };

  vm.runInNewContext(inboxScript, context, {
    filename: "capsules/inbox/browser/index.html:inline",
  });
  await settle();
  await nodes.get("refresh").listeners.get("click")[0]();
  await settle();

  const selectedRows = nodes.get("entry-rows").children
    .filter((row) => row.getAttribute("aria-selected") === "true");
  assert.equal(selectedRows.length, 0, "Inbox selected a different request after the explicit request disappeared");
  const detailText = descendants(nodes.get("entry-detail"))
    .map((node) => node.textContent || "")
    .join(" ");
  assert(detailText.includes("no longer available"), "Inbox did not keep the vanished explicit request unavailable", detailText);
  assert.equal(
    descendants(nodes.get("entry-detail")).filter((node) => node.tagName === "BUTTON").length,
    0,
    "Inbox exposed actions after the explicit request disappeared on refresh",
  );
}

async function runInboxRequestedSelectionAppearsLaterSmoke() {
  const inbox = fs.readFileSync(new URL("../capsules/inbox/browser/index.html", import.meta.url), "utf8");
  const inboxScript = extractInboxScript(inbox);
  const nodes = new Map([
    ["locked-shell", new FakeElement("section")],
    ["inbox-shell", new FakeElement("section")],
    ["status-text", new FakeElement("p")],
    ["pending-count", new FakeElement("strong")],
    ["review-count", new FakeElement("strong")],
    ["refresh", new FakeElement("button")],
    ["entry-rows", new FakeElement("section")],
    ["entry-detail", new FakeElement("section")],
    ["entry-split", new FakeElement("div")],
    ["empty-state", new FakeElement("section")],
    ["empty-title", new FakeElement("h2")],
    ["list-title", new FakeElement("h1")],
  ]);
  const filterAll = new FakeElement("button");
  filterAll.className = "summary-card active";
  filterAll.dataset.filter = "all";
  const filterReview = new FakeElement("button");
  filterReview.className = "summary-card";
  filterReview.dataset.filter = "review";
  let fetchCount = 0;
  const document = {
    hidden: false,
    documentElement: { dataset: {} },
    getElementById(id) {
      return nodes.get(id) || null;
    },
    querySelectorAll(selector) {
      if (selector === ".summary-card[data-filter]") {
        return [filterAll, filterReview];
      }
      return [];
    },
    addEventListener() {},
    createElement(tagName) {
      return new FakeElement(tagName);
    },
  };
  const context = {
    console,
    document,
    window: {
      location: {
        search: "?home_origin=http%3A%2F%2Flocalhost%3A61180&notification_id=entry-wallet",
        hash: "#home_token=inbox-test-token",
      },
      top: { postMessage() {} },
      parent: { postMessage() {} },
      addEventListener() {},
      removeEventListener() {},
      setInterval() {
        return 1;
      },
      setTimeout(callback) {
        callback();
        return 1;
      },
      clearTimeout() {},
    },
    URLSearchParams,
    Intl,
    Date,
    Promise,
    setTimeout,
    clearTimeout,
    fetch: async () => {
      fetchCount += 1;
      const summary = inboxSummary();
      if (fetchCount === 1) {
        summary.notifications.entries = [summary.notifications.entries[0]];
      }
      return jsonResponse(summary);
    },
  };

  vm.runInNewContext(inboxScript, context, {
    filename: "capsules/inbox/browser/index.html:inline",
  });
  await settle();

  let selectedRows = nodes.get("entry-rows").children
    .filter((row) => row.getAttribute("aria-selected") === "true");
  assert.equal(selectedRows.length, 0, "Inbox selected the wrong request before the explicit request appeared");

  await nodes.get("refresh").listeners.get("click")[0]();
  await settle();

  selectedRows = nodes.get("entry-rows").children
    .filter((row) => row.getAttribute("aria-selected") === "true");
  assert.equal(selectedRows.length, 1);
  assert.equal(selectedRows[0], nodes.get("entry-rows").children[1], "Inbox did not reselect the requested request when it appeared");

  const keydown = nodes.get("entry-rows").listeners.get("keydown")[0];
  keydown({
    key: "ArrowUp",
    preventDefault() {},
  });
  await settle();
  selectedRows = nodes.get("entry-rows").children
    .filter((row) => row.getAttribute("aria-selected") === "true");
  assert.equal(selectedRows[0], nodes.get("entry-rows").children[0], "Keyboard selection did not clear the pinned request");
}

await runInboxHomeChromeSmoke();
await runInboxLaunchSelectionSmoke();
await runInboxMissingLaunchSelectionSmoke();
await runInboxRemovedLaunchSelectionSmoke();
await runInboxRequestedSelectionAppearsLaterSmoke();
console.log("inbox-product-behavior-smoke: PASS");
