#!/usr/bin/env node

const CAMOFOX_BASE = process.env.CAMOFOX_BASE || "http://localhost:9377";
const ELASTOS_BASE_URL = (process.env.ELASTOS_BASE_URL || "http://localhost:8090").replace(/\/+$/, "");
const HOME_URL = process.env.HOME_URL || `${ELASTOS_BASE_URL}/apps/home/`;
const HOST_ORIGIN = new URL(HOME_URL).origin;
const USER_ID = process.env.CAMOFOX_USER_ID || `home-smoke-${Date.now()}`;
const TEST_DOCUMENT_CID = "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi";
const REQUIRE_DOCUMENT_PUBLISH = process.env.REQUIRE_DOCUMENT_PUBLISH === "1";
const PRESERVE_CAMOFOX_SESSION = process.env.HOME_SMOKE_PRESERVE_SESSION === "1";
const BANNED_PUBLIC_COPY = /\b(runtime mirror|permissioned runtime|projection|schema|derived facts?|runtime facts?|capsules?|providers?|capabilit(?:y|ies)|affordances?|authority boundary|provider boundary|gate preview|runtime-owned|host-loaded|structured home intents?|provider operation|launch token|hostcall|objects?)\b/i;

function delay(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

async function waitFor(check, timeoutMs = 15_000, intervalMs = 250) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const result = await check();
    if (result) {
      return true;
    }
    await delay(intervalMs);
  }
  return false;
}

function assert(condition, message, details = null) {
  if (!condition) {
    const error = new Error(message);
    error.details = details;
    throw error;
  }
}

class SkipCase extends Error {
  constructor(message, details = null) {
    super(message);
    this.details = details;
    this.skip = true;
  }
}

function isDocumentPublishUnavailable(state) {
  return /publishing is unavailable/i.test(String(state?.status || ""));
}

function isPublishedDocumentLinkReady(state) {
  return state?.ok
    && state.status === "Published."
    && String(state.uri || "").startsWith("elastos://")
    && state.text === "Copy link"
    && state.hidden === false
    && state.disabled === false;
}

async function request(path, options = {}) {
  const { timeoutMs = 30_000, ...fetchOptions } = options;
  const controller = new AbortController();
  const timeoutId = setTimeout(() => controller.abort(), timeoutMs);
  let response;
  try {
    response = await fetch(`${CAMOFOX_BASE}${path}`, {
      ...fetchOptions,
      signal: controller.signal,
      headers: {
        "content-type": "application/json",
        ...(fetchOptions.headers || {}),
      },
    });
  } catch (error) {
    if (error.name === "AbortError") {
      throw new Error(`${fetchOptions.method || "GET"} ${path} -> timeout after ${timeoutMs}ms`);
    }
    throw error;
  } finally {
    clearTimeout(timeoutId);
  }
  const text = await response.text();
  let data = {};
  try {
    data = text ? JSON.parse(text) : {};
  } catch {
    data = { raw: text };
  }
  if (!response.ok) {
    const error = new Error(`${fetchOptions.method || "GET"} ${path} -> ${response.status}`);
    error.response = data;
    throw error;
  }
  return data;
}

async function cleanupSession() {
  if (PRESERVE_CAMOFOX_SESSION) {
    return;
  }
  await fetch(`${CAMOFOX_BASE}/sessions/${USER_ID}`, { method: "DELETE" }).catch(() => {});
}

async function createTab() {
  for (let attempt = 0; attempt < 6; attempt += 1) {
    await cleanupSession();
    await delay(1000);
    let tabId = null;
    try {
      const created = await request("/tabs", {
        method: "POST",
        body: JSON.stringify({
          userId: USER_ID,
          sessionKey: `home-smoke-${Date.now()}-${attempt}`,
          url: HOME_URL,
        }),
      });
      tabId = created.tabId;
      const ready = await waitFor(async () => {
        const state = await evaluate(tabId, `(() => ({
          homeStatus: document.body?.dataset?.homeStatus || "",
          hasSystemShortcut: !!document.querySelector('#desktop-shortcuts .desktop-shortcut[data-target="system"]'),
        }))()`);
        return state.homeStatus === "ready" && state.hasSystemShortcut;
      }, 30_000, 400);
      assert(ready, "home smoke did not reach a ready desktop");
      await delay(800);
      return tabId;
    } catch (error) {
      if (tabId) {
        await closeTab(tabId);
      }
      if (attempt === 5) {
        throw error;
      }
      await delay(1500);
    }
  }
  throw new Error("home smoke could not create a stable tab");
}

async function closeTab(tabId) {
  await fetch(`${CAMOFOX_BASE}/tabs/${tabId}?userId=${encodeURIComponent(USER_ID)}`, {
    method: "DELETE",
  }).catch(() => {});
}

async function click(tabId, selector, options = {}) {
  const { noMouseSequence = false } = options;
  await waitForSelector(tabId, selector, 5_000).catch(() => {});
  try {
    return await request(`/tabs/${tabId}/click`, {
      method: "POST",
      body: JSON.stringify({ userId: USER_ID, selector, noMouseSequence }),
    });
  } catch (error) {
    const message = String(error.message || "");
    if (!message.includes("timeout") && !message.includes("-> 500")) {
      throw error;
    }
    await delay(500);
    await waitForSelector(tabId, selector, 5_000).catch(() => {});
    return request(`/tabs/${tabId}/click`, {
      method: "POST",
      body: JSON.stringify({ userId: USER_ID, selector, noMouseSequence }),
    });
  }
}

async function waitForWindowControlReady(tabId, targetId, action) {
  const guardKey = action === "close" ? "closeGuardUntil" : "controlGuardUntil";
  const ready = await waitFor(async () => {
    return evaluate(tabId, `(() => {
      const windowNode = document.querySelector('.window[data-target="${targetId}"]');
      const control = windowNode?.querySelector('[data-action="${action}"]');
      if (!windowNode || !control) return false;
      const guardUntil = Number.parseFloat(windowNode.dataset.${guardKey} || "0");
      return !Number.isFinite(guardUntil) || guardUntil <= performance.now();
    })()`);
  }, 7000, 120);
  assert(ready, `window control was not ready: ${targetId} ${action}`, await shellState(tabId));
}

async function clickWindowControl(tabId, targetId, action) {
  await waitForWindowControlReady(tabId, targetId, action);
  const clicked = await evaluate(tabId, `(() => {
    const control = document.querySelector('.window[data-target="${targetId}"] [data-action="${action}"]');
    if (!control) return false;
    control.click();
    return true;
  })()`);
  assert(clicked, `window control was not clickable: ${targetId} ${action}`, await shellState(tabId));
}

async function activateTaskbarTarget(tabId, targetId) {
  const activated = await evaluate(tabId, `(() => {
    const button = document.querySelector('.taskbar-item[data-target="${targetId}"]');
    if (!button) return false;
    button.click();
    return true;
  })()`);
  assert(activated, `taskbar target was not activatable: ${targetId}`, await shellState(tabId));
}

async function openLauncher(tabId) {
  const activated = await evaluate(tabId, `(() => {
    const button = document.querySelector("#launcher-toggle");
    if (!button) return false;
    button.click();
    return true;
  })()`);
  assert(activated, "launcher button was not available", await shellState(tabId));
  const opened = await waitFor(async () => {
    const state = await shellState(tabId);
    return state.launcherVisible;
  }, 5_000, 200);
  assert(opened, "launcher did not open", await shellState(tabId));
}

async function activate(tabId, selector) {
  const activated = await evaluate(tabId, `(() => {
    const node = document.querySelector(${JSON.stringify(selector)});
    if (!node) return false;
    node.click();
    return true;
  })()`);
  assert(activated, `selector was not available: ${selector}`, await shellState(tabId));
}

async function type(tabId, selector, text, options = {}) {
  return request(`/tabs/${tabId}/type`, {
    method: "POST",
    body: JSON.stringify({
      userId: USER_ID,
      selector,
      text,
      ...options,
    }),
  });
}

async function press(tabId, key) {
  return request(`/tabs/${tabId}/press`, {
    method: "POST",
    body: JSON.stringify({ userId: USER_ID, key }),
  });
}

async function refresh(tabId) {
  return request(`/tabs/${tabId}/refresh`, {
    method: "POST",
    body: JSON.stringify({ userId: USER_ID }),
  });
}

async function browserJson(tabId, path, options = {}) {
  const method = options.method || "GET";
  const body = typeof options.body === "string" ? options.body : "";
  const script = `fetch(${JSON.stringify(path)}, {
    method: ${JSON.stringify(method)},
    headers: { "content-type": "application/json" },
    ${body ? `body: ${JSON.stringify(body)},` : ""}
  }).then(async (response) => {
    const text = await response.text();
    let payload = {};
    try { payload = text ? JSON.parse(text) : {}; } catch (_) { payload = { raw: text }; }
    if (!response.ok) {
      throw new Error(${JSON.stringify(method)} + " " + ${JSON.stringify(path)} + " -> " + response.status + " " + text);
    }
    return payload;
  })`;
  return evaluate(tabId, script);
}

async function launchShellTarget(tabId, target) {
  return browserJson(tabId, "/api/apps/home/launch", {
    method: "POST",
    body: JSON.stringify({ target }),
  });
}

async function inboxAction(tabId, actionId) {
  const launched = await launchShellTarget(tabId, "inbox");
  const token = new URL(launched.route, HOST_ORIGIN).searchParams.get("home_token");
  assert(token, "Inbox launch did not return a home token", launched);
  return evaluate(
    tabId,
    `fetch("/api/apps/inbox/actions", {
      method: "POST",
      headers: {
        "content-type": "application/json",
        "x-elastos-home-token": ${JSON.stringify(token)},
      },
      body: ${JSON.stringify(JSON.stringify({ action_id: actionId }))},
    }).then(async (response) => {
      const text = await response.text();
      if (!response.ok) throw new Error("Inbox action failed: " + response.status + " " + text);
      return text ? JSON.parse(text) : {};
    })`,
  );
}

async function cleanupRoomState(tabId) {
  const summary = await browserJson(tabId, "/api/apps/chat-room/summary");
  if (Array.isArray(summary.pending_requests)) {
    for (const request of summary.pending_requests) {
      await inboxAction(tabId, `room-deny-request:${request.request_id}`);
    }
  }
}

async function waitForSelector(tabId, selector, timeoutMs = 15_000) {
  return request(`/tabs/${tabId}/wait`, {
    method: "POST",
    timeoutMs: timeoutMs + 5_000,
    body: JSON.stringify({ userId: USER_ID, selector, timeoutMs }),
  });
}

async function evaluate(tabId, expression) {
  const result = await request(`/tabs/${tabId}/evaluate`, {
    method: "POST",
    body: JSON.stringify({ userId: USER_ID, expression }),
  });
  return result.result;
}

async function shellState(tabId) {
  return evaluate(tabId, `(() => ({
    pageTitle: document.title,
    homeStatus: document.body?.dataset?.homeStatus || "",
    homeAuthority: document.body?.dataset?.homeAuthority || "",
    unlockVisible: !document.querySelector("#home-unlock")?.hidden,
    unlockTitle: document.querySelector("#home-unlock-title")?.textContent?.trim() || "",
    unlockCopy: document.querySelector("#home-unlock-copy")?.textContent?.trim() || "",
    launcherVisible: !document.querySelector("#launcher").hidden,
    launcherExpanded: {
      taskbar: document.querySelector("#launcher-toggle")?.getAttribute("aria-expanded"),
    },
    desktopShortcuts: [...document.querySelectorAll("#desktop-shortcuts .desktop-shortcut[data-target]")].map((node) => ({
      target: node.dataset.target || "",
      label: node.querySelector(".desktop-shortcut-title")?.textContent?.trim() || "",
    })),
    desktopIconsHidden: !!document.querySelector("#desktop-shortcuts")?.hidden,
    desktopIconsAriaHidden: document.querySelector("#desktop-shortcuts")?.getAttribute("aria-hidden") || "",
    launcherCards: [...document.querySelectorAll(".launcher-card[data-target]")].map((node) => ({
      target: node.dataset.target || "",
      title: node.querySelector(".launcher-card-title")?.textContent?.trim() || "",
      group: node.closest(".launcher-group")?.querySelector(".launcher-group-heading")?.textContent?.trim() || "",
    })),
    launcherHeading: document.querySelector(".launcher-heading")?.textContent?.trim() || "",
    launcherSearchPlaceholder: document.querySelector("#launcher-search")?.getAttribute("placeholder") || "",
    launcherGroupHeadings: [...document.querySelectorAll(".launcher-group-heading")].map((node) => node.textContent?.trim() || ""),
    selectedDesktop: [...document.querySelectorAll(".desktop-shortcut.selected")].map((node) => node.dataset.target),
    selectedDesktopLabel: document.querySelector(".desktop-shortcut.selected .desktop-shortcut-title")?.textContent?.trim() || "",
    desktopActiveDescendant: document.querySelector("#desktop-shortcuts")?.getAttribute("aria-activedescendant"),
    desktopSelectionText: window.getSelection ? window.getSelection().toString() : "",
    contextMenuVisible: !document.querySelector("#desktop-context-menu")?.hidden,
    contextMenuActions: [...document.querySelectorAll("#desktop-context-menu [data-context-action]")].map((node) => node.dataset.contextAction),
    inboxBadge: document.querySelector("#toolbar-inbox-count")?.textContent?.trim() || "",
    inboxButtonTitle: document.querySelector("#toolbar-inbox")?.getAttribute("title") || "",
    inboxButtonLabel: document.querySelector("#toolbar-inbox")?.getAttribute("aria-label") || "",
    renameEditorValue: document.querySelector(".desktop-shortcut-rename")?.value || "",
    windows: [...document.querySelectorAll(".window")].map((node) => ({
      target: node.dataset.target || null,
      hidden: node.classList.contains("hidden"),
      active: node.classList.contains("window-active"),
      maximized: node.dataset.maximized || "",
      title: node.querySelector(".window-head-title")?.textContent?.trim() || "",
    })),
    taskbar: [...document.querySelectorAll("#taskbar-targets .taskbar-item[data-target]")].map((node) => ({
      target: node.dataset.target || "",
      open: node.dataset.open || "",
      active: node.dataset.active || "",
      count: node.dataset.openWindows || "",
      label: node.getAttribute("aria-label") || "",
    })),
    topToolbar: {
      launcherPresent: !!document.querySelector("#toolbar-launcher"),
      fullscreenPresent: !!document.querySelector("#toolbar-fullscreen"),
      searchPresent: !!document.querySelector("#toolbar-search"),
      identityPresent: !!document.querySelector("#identity-button"),
      systemPresent: !!document.querySelector("#toolbar-system"),
      workspacePresent: !!document.querySelector(".workspace-indicator"),
    },
  }))()`);
}

async function dispatchDesktopContextMenu(tabId, targetId) {
  return evaluate(tabId, `(() => {
    const node = document.querySelector('.desktop-shortcut[data-target="${targetId}"]');
    if (!node) return { ok: false, reason: "missing-shortcut" };
    const rect = node.getBoundingClientRect();
    node.dispatchEvent(new MouseEvent("contextmenu", {
      bubbles: true,
      cancelable: true,
      clientX: rect.left + rect.width / 2,
      clientY: rect.top + rect.height / 2,
      button: 2,
    }));
    return { ok: true };
  })()`);
}

async function dispatchLauncherContextMenu(tabId, targetId) {
  return evaluate(tabId, `(() => {
    const node = document.querySelector('.launcher-card[data-target="${targetId}"]');
    if (!node) return { ok: false, reason: "missing-launcher-card" };
    const rect = node.getBoundingClientRect();
    node.dispatchEvent(new MouseEvent("contextmenu", {
      bubbles: true,
      cancelable: true,
      clientX: rect.left + rect.width / 2,
      clientY: rect.top + rect.height / 2,
      button: 2,
    }));
    return { ok: true };
  })()`);
}

async function dispatchDesktopBackgroundContextMenu(tabId) {
  return evaluate(tabId, `(() => {
    const node = document.querySelector("#desktop");
    if (!node) return { ok: false, reason: "missing-desktop" };
    const rect = node.getBoundingClientRect();
    node.dispatchEvent(new MouseEvent("contextmenu", {
      bubbles: true,
      cancelable: true,
      clientX: rect.left + Math.min(48, Math.max(12, rect.width / 2)),
      clientY: rect.top + Math.min(48, Math.max(12, rect.height / 2)),
      button: 2,
    }));
    return { ok: true };
  })()`);
}

async function activateContextAction(tabId, action) {
  const result = await evaluate(tabId, `(() => {
    const node = document.querySelector('#desktop-context-menu [data-context-action="${action}"]');
    if (!node) return { ok: false, reason: "missing-context-action", action: "${action}" };
    node.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true }));
    return { ok: true };
  })()`);
  assert(result.ok, `context menu action failed: ${action}`, result);
}

async function waitForContextAction(tabId, action) {
  const ready = await waitFor(async () => {
    const state = await shellState(tabId);
    return state.contextMenuVisible && state.contextMenuActions.includes(action);
  }, 7000, 200);
  assert(ready, `desktop context menu did not expose action: ${action}`, await shellState(tabId));
}

async function dispatchDesktopDoubleClick(tabId, targetId) {
  return evaluate(tabId, `(() => {
    const node = document.querySelector('.desktop-shortcut[data-target="${targetId}"]');
    if (!node) return { ok: false, reason: "missing-shortcut" };
    node.dispatchEvent(new MouseEvent("dblclick", { bubbles: true, cancelable: true }));
    return { ok: true };
  })()`);
}

async function openShellTarget(tabId, target, query = {}) {
  const message = JSON.stringify({
    type: "home:open-target",
    target,
    query,
  });
  return evaluate(tabId, `(() => {
    window.postMessage(${message}, window.location.origin);
    return { ok: true };
  })()`);
}

async function openElastosUri(tabId, uri, preferredViewer = "documents") {
  const message = JSON.stringify({
    type: "home:open-uri",
    uri,
    preferredViewer,
  });
  return evaluate(tabId, `(() => {
    window.postMessage(${message}, window.location.origin);
    return { ok: true };
  })()`);
}

async function frameState(tabId, targetId, expression) {
  try {
    return await evaluate(tabId, `(() => {
      const frame = document.querySelector('.window[data-target="${targetId}"] .window-frame');
      const win = frame?.contentWindow;
      const doc = win?.document;
      if (!frame || !win || !doc) {
        return { ok: false, reason: "missing-frame" };
      }
      return (${expression});
    })()`);
  } catch (error) {
    return { ok: false, reason: "evaluate-failed", error: String(error.message || error) };
  }
}

async function framePublicCopyState(tabId, targetId) {
  return frameState(tabId, targetId, `(() => {
    const text = doc.body?.innerText?.replace(/\\s+/g, " ").trim() || "";
    const headings = [...doc.querySelectorAll("h1, h2, h3")]
      .filter((node) => node.offsetParent !== null && !node.closest("#technical-details, .technical-details, [data-tab=technical]"))
      .map((node) => node.textContent?.replace(/\\s+/g, " ").trim() || "")
      .filter(Boolean);
    return { ok: true, text, headings };
  })()`);
}

async function directPublicCopyState(tabId, path) {
  const url = new URL(path, HOST_ORIGIN).toString();
  await evaluate(tabId, `(() => { window.location.href = ${JSON.stringify(url)}; return true; })()`);
  const loaded = await waitFor(async () => evaluate(tabId, `(() => (
    document.readyState === "complete" && Boolean(document.body)
  ))()`), 15_000, 250);
  assert(loaded, `direct public-copy page did not load: ${path}`);
  await delay(600);
  return evaluate(tabId, `(() => {
    const headings = [...document.querySelectorAll("h1, h2, h3")]
      .filter((node) => node.offsetParent !== null && !node.closest("#technical-details, .technical-details, [data-tab=technical]"))
      .map((node) => node.textContent?.replace(/\\s+/g, " ").trim() || "")
      .filter(Boolean);
    return {
      url: window.location.href,
      title: document.title,
      text: document.body?.innerText?.replace(/\\s+/g, " ").trim() || "",
      headings,
    };
  })()`);
}

async function clickInFrame(tabId, targetId, selector) {
  return evaluate(tabId, `(() => {
    const frame = document.querySelector('.window[data-target="${targetId}"] .window-frame');
    const doc = frame?.contentWindow?.document;
    const node = doc?.querySelector(${JSON.stringify(selector)});
    if (!node) return { ok: false, reason: "missing-node" };
    node.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true }));
    return { ok: true };
  })()`);
}

async function deleteDocumentWorkingCopy(tabId, docDid) {
  if (!docDid) {
    return { ok: true, skipped: true };
  }
  return frameState(tabId, "documents", `(async () => {
    const homeToken = new URL(win.location.href).searchParams.get("home_token") || "";
    const response = await fetch("/api/provider/documents/delete", {
      method: "POST",
      headers: {
        "content-type": "application/json",
        "x-elastos-home-token": homeToken,
      },
      body: JSON.stringify({ doc_did: ${JSON.stringify(docDid)} }),
    });
    const text = response.ok ? "" : await response.text();
    return { ok: response.ok, status: response.status, text };
  })()`);
}

async function runCase(name, fn) {
  let tabId = null;
  try {
    tabId = await createTab();
    await fn(tabId);
    console.log(`PASS ${name}`);
  } catch (error) {
    if (error.skip) {
      console.log(`SKIP ${name}: ${error.message}`);
      return;
    }
    const state = tabId ? await shellState(tabId).catch(() => null) : null;
    console.error(`FAIL ${name}`);
    console.error(error.message);
    if (error.details) {
      console.error(JSON.stringify(error.details, null, 2));
    }
    if (state) {
      console.error(JSON.stringify(state, null, 2));
    }
    throw error;
  } finally {
    if (tabId) {
      await closeTab(tabId);
    }
    await cleanupSession();
  }
}

async function main() {
  await cleanupSession();
  try {
    await runCase("top-toolbar-keeps-only-working-controls", async (tabId) => {
      await delay(1000);
      const state = await shellState(tabId);
      const publicState = await evaluate(tabId, `(() => ({
        text: document.body?.innerText?.replace(/\\s+/g, " ").trim() || "",
        headings: [...document.querySelectorAll("h1, h2, h3")]
          .filter((node) => node.offsetParent !== null)
          .map((node) => node.textContent?.replace(/\\s+/g, " ").trim() || "")
          .filter(Boolean),
      }))()`);
      assert(!BANNED_PUBLIC_COPY.test(publicState.text), "Home GUI exposes internal narration", publicState);
      const duplicateHomeHeadings = publicState.headings.filter((heading, index, headings) => headings.indexOf(heading) !== index);
      assert(duplicateHomeHeadings.length === 0, "Home GUI renders duplicate visible headings", { duplicateHomeHeadings, publicState });
      assert(state.pageTitle === "Home · ElastOS", "Home page title mismatch", state);
      assert(state.topToolbar.launcherPresent === false, "top toolbar should not show a launcher button", state);
      assert(state.topToolbar.fullscreenPresent === true, "top toolbar should expose fullscreen as a working mobile control", state);
      assert(state.topToolbar.searchPresent === false, "top toolbar should not show a search button", state);
      assert(state.topToolbar.identityPresent === false, "top toolbar should not show a local identity button", state);
      assert(state.topToolbar.systemPresent === false, "top toolbar should not show a System button", state);
      assert(state.topToolbar.workspacePresent === false, "top toolbar should not show a desktop workspace dot", state);
      assert(state.inboxButtonTitle === "Inbox", "empty Inbox toolbar title should stay terse", state);
      assert(state.inboxButtonLabel === "Open Inbox", "empty Inbox toolbar label should stay terse", state);
      assert(
        state.desktopShortcuts.some((shortcut) => shortcut.target === "system" && shortcut.label === "System"),
        "desktop should label system as System",
        state,
      );
      assert(
        state.desktopShortcuts.some((shortcut) => shortcut.target === "library" && shortcut.label === "Library"),
        "desktop should label library as Library",
        state,
      );
    });

    await runCase("taskbar-launcher-opens", async (tabId) => {
      await openLauncher(tabId);
      await delay(1000);
      const state = await shellState(tabId);
      assert(state.launcherVisible, "launcher did not open from taskbar launcher", state);
      assert(state.launcherExpanded.taskbar === "true", "taskbar launcher did not expose expanded state", state);
    });

    await runCase("unsigned-launch-prompts-passkey", async (tabId) => {
      const initial = await shellState(tabId);
      if (initial.homeAuthority === "signed") {
        return;
      }
      await openLauncher(tabId);
      await delay(500);
      await activate(tabId, '.launcher-card[data-target="system"]');
      const prompted = await waitFor(async () => {
        const state = await shellState(tabId);
        return state.unlockVisible && state.unlockTitle === "Sign in";
      }, 5000, 250);
      const state = await shellState(tabId);
      assert(prompted, "unsigned Home launch did not prompt for passkey", state);
      assert(state.windows.length === 0, "unsigned Home launch should not open System before passkey approval", state);
      assert(
        state.unlockCopy === "Use your passkey to unlock your data, apps and desktop.",
        "unsigned Home launch prompt copy drifted",
        state,
      );
    });

    await runCase("direct-empty-and-error-copy-stays-plain", async (tabId) => {
      const paths = [
        "/apps/home-cli/",
        "/apps/system/",
        "/apps/marketplace/",
        "/apps/services/",
        "/apps/inbox/",
        "/apps/library/",
        "/apps/documents/",
        "/apps/wallet/",
        "/apps/browser/",
      ];
      for (const path of paths) {
        const state = await directPublicCopyState(tabId, path);
        assert(state.text.length > 0, `${path} rendered an empty direct state`, state);
        assert(!BANNED_PUBLIC_COPY.test(state.text), `${path} direct state exposes internal narration`, state);
        assert(!/\b(?:400|401|403|500)\b|bad request|request failed|invalid schema|launch token/i.test(state.text), `${path} direct state exposes a raw backend error`, state);
        const duplicates = state.headings.filter((heading, index, headings) => headings.indexOf(heading) !== index);
        assert(duplicates.length === 0, `${path} direct state renders duplicate headings`, { duplicates, state });
      }
    });

    const probeTabId = await createTab();
    const signedProbe = await shellState(probeTabId);
    await closeTab(probeTabId);
    if (signedProbe.homeAuthority !== "signed") {
      console.log("SKIP signed Home app journeys: passkey-backed Camofox session unavailable; set HOME_SMOKE_PRESERVE_SESSION=1 with a signed Camofox profile to run them");
      return;
    }

    await runCase("ordinary-app-copy-stays-plain-and-heading-unique", async (tabId) => {
      for (const target of ["system", "marketplace", "services", "inbox", "library", "documents", "wallet", "browser"]) {
        await openShellTarget(tabId, target);
        const ready = await waitFor(async () => (await framePublicCopyState(tabId, target)).ok, 15_000, 300);
        assert(ready, `${target} did not render for the public-copy visual check`, await shellState(tabId));
        const state = await framePublicCopyState(tabId, target);
        assert(!BANNED_PUBLIC_COPY.test(state.text), `${target} exposes internal narration`, state);
        const duplicates = state.headings.filter((heading, index, headings) => headings.indexOf(heading) !== index);
        assert(duplicates.length === 0, `${target} renders duplicate visible headings`, { duplicates, state });
      }
    });

    await runCase("home-cli-copy-stays-plain", async (tabId) => {
      const launched = await launchShellTarget(tabId, "home-cli");
      const route = new URL(launched.route, HOST_ORIGIN).toString();
      assert(route.includes("home_token="), "Home CLI launch did not mint an app token", launched);
      await evaluate(tabId, `(() => { window.location.href = ${JSON.stringify(route)}; return true; })()`);
      await waitForSelector(tabId, "#xterm-terminal", 20_000);
      await delay(1200);
      const state = await evaluate(tabId, `(() => ({
        text: document.body?.innerText?.replace(/\\s+/g, " ").trim() || "",
        headings: [...document.querySelectorAll("h1, h2, h3")]
          .filter((node) => node.offsetParent !== null)
          .map((node) => node.textContent?.replace(/\\s+/g, " ").trim() || "")
          .filter(Boolean),
      }))()`);
      assert(!BANNED_PUBLIC_COPY.test(state.text), "Home CLI exposes internal narration", state);
      assert(!/\b(?:400|401|403|500)\b|bad request|request failed|invalid schema|launch token/i.test(state.text), "Home CLI exposes a raw backend error", state);
      const duplicates = state.headings.filter((heading, index, headings) => headings.indexOf(heading) !== index);
      assert(duplicates.length === 0, "Home CLI renders duplicate headings", { duplicates, state });
    });

    await runCase("launcher-card-opens-system", async (tabId) => {
      await openLauncher(tabId);
      await delay(1000);
      await activate(tabId, '.launcher-card[data-target="system"]');
      await waitFor(async () => {
        const system = await frameState(tabId, "system", `({
          ok: true,
          runtimeStatus: doc.querySelector('[data-field="runtime-status"]')?.textContent?.trim() || "",
        })`);
        return system.ok && system.runtimeStatus.length > 0;
      }, 12000, 500);
      const state = await shellState(tabId);
      assert(!state.launcherVisible, "launcher stayed open after System launch", state);
      assert(
        state.windows.length === 1 &&
        state.windows[0].target === "system" &&
        state.windows[0].title === "System",
        "System launcher card did not open a System window",
        state,
      );
      const system = await frameState(tabId, "system", `({
        ok: true,
        title: doc.title || "",
        heading: [...doc.querySelectorAll("h1")].find((node) => node.offsetParent !== null)?.textContent?.trim() || "",
        panelLabels: [...doc.querySelectorAll(".pc2-section-title, #catalog-title")].map((node) => node.textContent?.trim() || ""),
        fieldLabels: [...doc.querySelectorAll(".system-fields dt")].map((node) => node.textContent?.trim() || ""),
        aboutFieldLabels: [...doc.querySelectorAll('[data-settings="about"] .system-fields dt')].map((node) => node.textContent?.trim() || ""),
        walletControlsRemoved: !doc.querySelector("#wallet-create") && !doc.querySelector("#wallet-approvals"),
        recoveryPasswordPresent: !!doc.querySelector("#recovery-password"),
        recoveryPasswordPlaceholder: doc.querySelector("#recovery-password")?.getAttribute("placeholder") || "",
        recoveryDownloadLabel: doc.querySelector("#recovery-download")?.textContent?.trim() || "",
        recoveryImportPresent: !!doc.querySelector("#recovery-import"),
        recoveryImportLabel: doc.querySelector('label[for="recovery-import"]')?.textContent?.trim() || "",
        recoveryPendingPresent: !!doc.querySelector("#recovery-pending"),
        recoveryPendingHidden: !!doc.querySelector("#recovery-pending")?.hidden,
        recoveryAttachLabel: doc.querySelector("#recovery-attach")?.textContent?.trim() || "",
        recoveryCancelLabel: doc.querySelector("#recovery-cancel")?.textContent?.trim() || "",
        runtimeStatus: doc.querySelector('[data-field="runtime-status"]')?.textContent?.trim() || "",
        storageSectionPresent: !!doc.querySelector('[data-settings="storage"], #webspace-list'),
        hasAppearancePanel: !!doc.querySelector("#appearance-title"),
        hasBackgroundInput: !!doc.querySelector("#background-input"),
        hasBackgroundPreview: !!doc.querySelector("#background-preview"),
        hasBackgroundOverlay: !!doc.querySelector("#background-overlay"),
        hasBackgroundOverlayRange: !!doc.querySelector("#background-overlay-range"),
        hasBackgroundOverlayOpacity: !!doc.querySelector("#background-overlay-opacity"),
        backgroundInputLabel: doc.querySelector("#background-input")?.getAttribute("aria-label") || "",
        backgroundOverlayLabel: doc.querySelector("#background-overlay")?.getAttribute("aria-label") || "",
        backgroundOverlayOpacityLabel: doc.querySelector("#background-overlay-opacity")?.getAttribute("aria-label") || "",
        backgroundPreviewImage: doc.querySelector("#background-preview")?.style.backgroundImage || "",
        backgroundPreviewEmpty: doc.querySelector("#background-preview")?.dataset.empty || "",
        backgroundOverlayDisabled: !!doc.querySelector("#background-overlay")?.disabled,
        backgroundOverlayRangeHidden: !!doc.querySelector("#background-overlay-range")?.hidden,
        backgroundOverlayOpacityDisabled: !!doc.querySelector("#background-overlay-opacity")?.disabled,
        backgroundOverlayChecked: !!doc.querySelector("#background-overlay")?.checked,
        backgroundOverlayOpacityValue: doc.querySelector("#background-overlay-opacity")?.value || "",
        overlayIsInsideBackgroundField: (() => {
          const preview = doc.querySelector("#background-preview");
          const overlay = doc.querySelector("#background-overlay");
          return !!preview && !!overlay && preview.closest(".pc2-group-row") === overlay.closest(".pc2-group-row");
        })(),
        backgroundActionsSameRow: (() => {
          const choose = doc.querySelector('label[for="background-input"]')?.getBoundingClientRect();
          const reset = doc.querySelector("#background-reset")?.getBoundingClientRect();
          return !!choose && !!reset && Math.abs(choose.top - reset.top) <= 2;
        })(),
      })`);
      assert(system.ok, "System frame was not reachable", system);
      assert(system.title === "System · ElastOS", "System frame title mismatch", system);
      assert(system.heading === "Accounts", "System frame should open on Accounts", system);
      assert(system.panelLabels.includes("Accounts"), "System frame is missing Accounts", system);
      assert(system.panelLabels.includes("Shell"), "System frame is missing Shell", system);
      assert(system.panelLabels.includes("Appearance"), "System frame is missing Appearance", system);
      assert(system.panelLabels.includes("Recovery"), "System frame is missing Recovery", system);
      assert(system.panelLabels.includes("Access"), "System frame is missing Access", system);
      assert(!system.panelLabels.includes("Elastos Webspace"), "System must not present app inventory as a WebSpace", system);
      assert(system.panelLabels.includes("Apps & Services"), "System frame is missing Apps & Services", system);
      assert(system.panelLabels.includes("This Device"), "System frame is missing This Device", system);
      assert(!system.panelLabels.includes("Profile"), "System frame should keep People profile settings out of System", system);
      assert(system.fieldLabels.includes("Device identity"), "System frame is missing the Device identity section", system);
      assert(system.fieldLabels.includes("Version"), "System frame is missing the runtime version", system);
      assert(!system.fieldLabels.includes("Documents"), "System About should not include Documents", system);
      assert(system.aboutFieldLabels.includes("Version"), "System About is missing the runtime version", system);
      assert(!system.aboutFieldLabels.includes("Documents"), "System About should not include Documents", system);
      assert(system.fieldLabels.includes("Accounts"), "System frame is missing account management", system);
      assert(system.fieldLabels.includes("Recovery"), "System frame is missing Recovery Kit controls", system);
      assert(system.fieldLabels.includes("Guest access"), "System frame is missing guest access control", system);
      assert(!system.fieldLabels.includes("Wallet"), "System frame should not duplicate Wallet controls", system);
      assert(system.walletControlsRemoved, "System frame should not include wallet account or approval controls", system);
      assert(system.fieldLabels.includes("Network status"), "System frame is missing network status diagnostics", system);
      assert(!system.fieldLabels.includes("Display name"), "System should keep display-name editing in People", system);
      assert(system.recoveryPasswordPresent, "System frame did not expose Recovery Kit password protection", system);
      assert(system.recoveryPasswordPlaceholder === "Optional password", "System Recovery Kit password copy drifted", system);
      assert(system.recoveryDownloadLabel.toLowerCase().includes("recovery kit"), "System frame did not expose Recovery Kit download", system);
      assert(system.recoveryImportPresent, "System frame did not expose Recovery Kit import", system);
      assert(system.recoveryImportLabel === "Import Recovery Kit", "System Recovery Kit import label drifted", system);
      assert(system.recoveryPendingPresent && system.recoveryPendingHidden, "System Recovery Kit reassignment review should exist but stay hidden until needed", system);
      assert(system.recoveryAttachLabel === "Recover account", "System Recovery Kit recover action drifted", system);
      assert(system.recoveryCancelLabel === "Cancel", "System Recovery Kit cancel action drifted", system);
      assert(system.runtimeStatus.length > 0, "System did not show the managed local runtime version", system);
      assert(!system.storageSectionPresent, "System must not expose the removed Storage section", system);
      assert(system.hasAppearancePanel, "System did not expose Appearance", system);
      assert(system.hasBackgroundInput && system.hasBackgroundPreview, "System Appearance did not expose background controls", system);
      assert(system.backgroundInputLabel === "Choose background image", "System background input lacked a stable label", system);
      assert(system.backgroundOverlayLabel === "Add contrast over background", "System overlay toggle lacked a stable label", system);
      assert(system.backgroundOverlayOpacityLabel === "Overlay strength", "System overlay slider lacked a stable label", system);
      assert(!system.fieldLabels.includes("Overlay"), "System Appearance should not render Overlay as a separate box", system);
      assert(system.overlayIsInsideBackgroundField, "System overlay controls should live inside the Background box", system);
      assert(system.backgroundActionsSameRow, "System background Choose image and Reset actions should stay on one row", system);
      assert(system.backgroundPreviewImage.includes("/apps/home-gui/wallpaper.webp"), "System Appearance did not preview the default wallpaper", system);
      assert(system.backgroundPreviewEmpty === "true", "System Appearance did not label the default wallpaper state", system);
      assert(system.hasBackgroundOverlay && system.backgroundOverlayDisabled === false, "System Appearance did not expose the background overlay setting", system);
      assert(system.hasBackgroundOverlayRange && system.hasBackgroundOverlayOpacity, "System Appearance did not expose the overlay opacity control", system);
      assert(system.backgroundOverlayOpacityValue.length > 0, "System background overlay opacity value was empty", system);
      assert(
        system.backgroundOverlayChecked
          ? !system.backgroundOverlayRangeHidden
          : system.backgroundOverlayRangeHidden,
        "System overlay slider visibility should follow the overlay toggle",
        system,
      );
      const overlayReset = await frameState(tabId, "system", `(async () => {
        const token = new URL(win.location.href).searchParams.get("home_token") || "";
        const response = await fetch("/api/apps/system/appearance/background-overlay", {
          method: "POST",
          headers: {
            "content-type": "application/json",
            "x-elastos-home-token": token,
          },
          body: JSON.stringify({ enabled: false, opacity: 0.55 }),
        });
        const payload = await response.json();
        win.parent.postMessage({
          type: "home:refresh-summary",
          homeToken: token,
        }, win.location.origin);
        return {
          ok: response.ok,
          status: response.status,
          overlayEnabled: payload.background_overlay_enabled,
          overlayOpacity: payload.background_overlay_opacity,
        };
      })()`);
      assert(overlayReset.ok && overlayReset.overlayEnabled === false && overlayReset.overlayOpacity === 0.55, "System overlay reset did not persist off state", overlayReset);
      const overlayResetApplied = await waitFor(async () => {
        const value = await evaluate(tabId, `document.querySelector(".desktop-backdrop")?.dataset.overlay || ""`);
        return value === "false";
      }, 8000, 300);
      assert(overlayResetApplied, "Home did not apply the reset background overlay setting", await shellState(tabId));
      const overlayOn = await frameState(tabId, "system", `(async () => {
        const token = new URL(win.location.href).searchParams.get("home_token") || "";
        const response = await fetch("/api/apps/system/appearance/background-overlay", {
          method: "POST",
          headers: {
            "content-type": "application/json",
            "x-elastos-home-token": token,
          },
          body: JSON.stringify({ enabled: true, opacity: 0.4 }),
        });
        const payload = await response.json();
        win.parent.postMessage({
          type: "home:refresh-summary",
          homeToken: token,
        }, win.location.origin);
        return {
          ok: response.ok,
          status: response.status,
          overlayEnabled: payload.background_overlay_enabled,
          overlayOpacity: payload.background_overlay_opacity,
        };
      })()`);
      assert(overlayOn.ok && overlayOn.overlayEnabled === true && overlayOn.overlayOpacity === 0.4, "System overlay update did not persist on state with opacity", overlayOn);
      const overlayApplied = await waitFor(async () => {
        const value = await evaluate(tabId, `(() => {
          const backdrop = document.querySelector(".desktop-backdrop");
          return {
            overlay: backdrop?.dataset.overlay || "",
            opacity: backdrop?.style.getPropertyValue("--desktop-overlay-opacity") || "",
          };
        })()`);
        return value.overlay === "true" && value.opacity === "0.4";
      }, 8000, 300);
      assert(overlayApplied, "Home did not apply the enabled background overlay opacity setting", await shellState(tabId));
      const overlayOff = await frameState(tabId, "system", `(async () => {
        const token = new URL(win.location.href).searchParams.get("home_token") || "";
        const response = await fetch("/api/apps/system/appearance/background-overlay", {
          method: "POST",
          headers: {
            "content-type": "application/json",
            "x-elastos-home-token": token,
          },
          body: JSON.stringify({ enabled: false, opacity: 0.4 }),
        });
        const payload = await response.json();
        win.parent.postMessage({
          type: "home:refresh-summary",
          homeToken: token,
        }, win.location.origin);
        return {
          ok: response.ok,
          status: response.status,
          overlayEnabled: payload.background_overlay_enabled,
          overlayOpacity: payload.background_overlay_opacity,
        };
      })()`);
      assert(overlayOff.ok && overlayOff.overlayEnabled === false && overlayOff.overlayOpacity === 0.4, "System overlay update did not persist off state", overlayOff);
      const overlayRestored = await waitFor(async () => {
        const value = await evaluate(tabId, `document.querySelector(".desktop-backdrop")?.dataset.overlay || ""`);
        return value === "false";
      }, 8000, 300);
      assert(overlayRestored, "Home did not restore the disabled background overlay setting", await shellState(tabId));
      const oversizedUpload = await frameState(tabId, "system", `(async () => {
        const token = new URL(win.location.href).searchParams.get("home_token") || "";
        const body = new Uint8Array(6 * 1024 * 1024);
        body[0] = 0x89;
        body[1] = 0x50;
        body[2] = 0x4e;
        body[3] = 0x47;
        const response = await fetch("/api/apps/system/appearance/background-image", {
          method: "POST",
          headers: {
            "content-type": "image/png",
            "x-elastos-home-token": token,
          },
          body,
        });
        const text = await response.text();
        return {
          ok: true,
          status: response.status,
          contentType: response.headers.get("content-type") || "",
          text,
        };
      })()`);
      assert(oversizedUpload.ok, "System wallpaper upload smoke could not reach the System frame", oversizedUpload);
      assert(
        oversizedUpload.status === 400 && oversizedUpload.text.includes("larger than 5 MB"),
        "System oversized wallpaper upload should be rejected by the gateway validation, not nginx",
        oversizedUpload,
      );
      assert(!oversizedUpload.text.includes("nginx"), "System wallpaper upload still hit nginx body-size rejection", oversizedUpload);
    });

    await runCase("window-can-move-partially-offscreen", async (tabId) => {
      await openShellTarget(tabId, "system");
      const opened = await waitFor(async () => {
        return evaluate(tabId, `!!document.querySelector('.window[data-target="system"] .window-head')`);
      }, 12_000, 300);
      assert(opened, "System window did not open for geometry proof", await shellState(tabId));

      const geometry = await evaluate(tabId, `(() => {
        const win = document.querySelector('.window[data-target="system"]');
        const head = win?.querySelector('.window-head');
        if (!win || !head) return { ok: false, reason: "missing-window" };
        const rect = head.getBoundingClientRect();
        const startX = rect.left + Math.min(80, rect.width / 2);
        const startY = rect.top + 18;
        const pointerId = 41;
        const fire = (type, x, y, buttons = 1) => head.dispatchEvent(new PointerEvent(type, {
          bubbles: true,
          cancelable: true,
          pointerId,
          pointerType: "mouse",
          button: 0,
          buttons,
          clientX: x,
          clientY: y,
        }));
        try {
          fire("pointerdown", startX, startY);
          fire("pointermove", -180, startY);
          fire("pointerup", -180, startY, 0);
        } catch (error) {
          return { ok: false, reason: String(error && error.message || error) };
        }
        const offscreenLeft = Number.parseFloat(win.style.left);
        const visibleY = Math.max(16, head.getBoundingClientRect().top + 18);
        const clickX = 24;
        fire("pointerdown", clickX, visibleY, pointerId + 1);
        const afterFocusClickLeft = Number.parseFloat(win.style.left);
        fire("pointerup", clickX, visibleY, pointerId + 1, 0);
        fire("pointerdown", clickX, visibleY, pointerId + 2);
        fire("pointermove", clickX + 2, visibleY, pointerId + 2);
        fire("pointerup", clickX + 2, visibleY, pointerId + 2, 0);
        return {
          ok: true,
          left: offscreenLeft,
          afterFocusClickLeft,
          afterTinyMoveLeft: Number.parseFloat(win.style.left),
          snap: win.dataset.snap || "",
          maximized: win.dataset.maximized || "",
        };
      })()`);
      assert(geometry.ok, "partial off-screen drag proof could not run", geometry);
      assert(geometry.left < 0, "window drag still hard-clamped at the desktop boundary", geometry);
      assert(Math.abs(geometry.afterFocusClickLeft - geometry.left) <= 2, "off-screen window jumped on titlebar focus", geometry);
      assert(Math.abs(geometry.afterTinyMoveLeft - geometry.left) <= 4, "off-screen window jumped on tiny titlebar drag", geometry);
      assert(geometry.snap === "" && geometry.maximized !== "true", "off-screen drag should not trigger snap", geometry);
    });

    await runCase("launcher-card-opens-gba", async (tabId) => {
      await openLauncher(tabId);
      await delay(1000);
      await activate(tabId, '.launcher-card[data-target="gba-emulator"]');
      await delay(2200);
      const state = await shellState(tabId);
      assert(state.windows.length === 1 && state.windows[0].target === "gba-emulator", "gba launcher card did not open gba-emulator", state);
      const gba = await frameState(tabId, "gba-emulator", `({
        ok: true,
        chooser: doc.querySelector('#drop-zone-copy')?.textContent?.trim() || "",
        libraryLabel: doc.querySelector('#drop-zone')?.getAttribute('aria-label') || "",
        controls: doc.querySelectorAll('[data-key]').length,
        fastForward: !!doc.querySelector('#btn-ff'),
        stateSlots: doc.querySelectorAll('[id^="btn-save"]').length,
        status: doc.querySelector('#status')?.textContent?.trim() || "",
      })`);
      assert(gba.chooser === "Insert Game", "GBA game chooser was not visible", gba);
      assert(gba.libraryLabel === "Open Library to choose a GBA game", "GBA Library action was not available", gba);
      assert(gba.controls === 10, "GBA controls were incomplete", gba);
      assert(gba.fastForward && gba.stateSlots === 3, "GBA utility controls were incomplete", gba);
      assert(gba.status === "", "GBA chooser showed an unexpected error", gba);
    });

    await runCase("launcher-card-opens-chat-room", async (tabId) => {
      await cleanupRoomState(tabId);
      await openLauncher(tabId);
      await delay(1000);
      await activate(tabId, '.launcher-card[data-target="chat-room"]');
      await waitFor(async () => {
        const room = await frameState(tabId, "chat-room", `({
          ok: true,
          chatCardHidden: doc.querySelector('#chat-card')?.hidden ?? true,
        })`);
        return room.ok && room.chatCardHidden === false;
      }, 4000, 250);
      await delay(250);
      const opening = await frameState(tabId, "chat-room", `({
        ok: true,
        browserAccessStageVisible: (() => {
          const node = doc.querySelector('#browser-access-stage');
          if (!node || node.hidden) return false;
          return win.getComputedStyle(node).display !== 'none';
        })(),
        chatCardHidden: doc.querySelector('#chat-card')?.hidden ?? true,
        messageInputDisabled: doc.querySelector('#message-input')?.disabled ?? true,
      })`);
      assert(opening.ok, "chat-room frame was not reachable during open", opening);
      assert(opening.chatCardHidden === false, "chat-room should open directly into the room surface", opening);
      assert(opening.browserAccessStageVisible === false, "chat-room should not render browser-access entry UI in shell mode", opening);
      await waitFor(async () => {
        const room = await frameState(tabId, "chat-room", `({
          ok: true,
          browserAccessStageVisible: (() => {
            const node = doc.querySelector('#browser-access-stage');
            if (!node || node.hidden) return false;
            return win.getComputedStyle(node).display !== 'none';
          })(),
          messageInputDisabled: doc.querySelector('#message-input')?.disabled ?? true,
          sendButtonDisabled: doc.querySelector('#send-button')?.disabled ?? true,
        })`);
        return room.ok && room.browserAccessStageVisible === false && room.messageInputDisabled === false && room.sendButtonDisabled === false;
      }, 12000, 500);
      await delay(1500);
      const state = await shellState(tabId);
      assert(state.windows.length === 1 && state.windows[0].target === "chat-room", "chat-room launcher card did not open chat room", state);
      const accessClosed = await frameState(tabId, "chat-room", `({
        ok: true,
        toggleHidden: doc.querySelector("#room-access-toggle")?.hidden ?? true,
        accessHidden: doc.querySelector("#room-access-section")?.hidden ?? false,
      })`);
      assert(accessClosed.ok && !accessClosed.toggleHidden && accessClosed.accessHidden, "Chat Room access controls should start collapsed behind settings", accessClosed);
      const toggled = await clickInFrame(tabId, "chat-room", "#room-access-toggle");
      assert(toggled.ok, "Chat Room access settings button was not clickable", toggled);
      await waitFor(async () => {
        const accessOpen = await frameState(tabId, "chat-room", `({
          ok: true,
          expanded: doc.querySelector("#room-access-toggle")?.getAttribute("aria-expanded") || "",
          accessHidden: doc.querySelector("#room-access-section")?.hidden ?? true,
        })`);
        return accessOpen.ok && accessOpen.expanded === "true" && accessOpen.accessHidden === false;
      }, 4000, 250);
      const guardedAttach = await frameState(tabId, "chat-room", `(() => {
        const input = doc.querySelector("#attachment-input");
        if (!input) return { ok: false, reason: "missing-attachment-input" };
        input.__homeSmokeNativeFilePicker = false;
        input.click = () => {
          input.__homeSmokeNativeFilePicker = true;
        };
        return { ok: true };
      })()`);
      assert(guardedAttach.ok, "Chat Room Attach native-picker guard could not be installed", guardedAttach);
      const attachOpenedLibrary = await clickInFrame(tabId, "chat-room", "#attach-button");
      assert(attachOpenedLibrary.ok, "Chat Room Attach was not clickable", attachOpenedLibrary);
      await waitFor(async () => {
        const current = await shellState(tabId);
        return current.windows.some((window) => window.target === "library");
      }, 6000, 300);
      const libraryAttach = await frameState(tabId, "library", `({
        ok: true,
        mode: new URL(win.location.href).searchParams.get("mode") || "",
        returnTarget: new URL(win.location.href).searchParams.get("returnTarget") || "",
        mainTitle: doc.querySelector("#main-title")?.textContent?.trim() || "",
      })`);
      assert(libraryAttach.ok && libraryAttach.mode === "attach" && libraryAttach.returnTarget === "chat-room" && libraryAttach.mainTitle === "Attach to Chat Room", "Chat Room Attach did not open Library in attach mode", libraryAttach);
      const attachContract = await frameState(tabId, "chat-room", `({
        ok: true,
        nativeFilePicker: doc.querySelector("#attachment-input")?.__homeSmokeNativeFilePicker ?? true,
        errorText: doc.querySelector("#error-text")?.textContent?.trim() || "",
      })`);
      assert(attachContract.ok && attachContract.nativeFilePicker === false && attachContract.errorText === "", "Chat Room Attach must open Library without the host file picker", attachContract);
      await clickWindowControl(tabId, "library", "close");
      const room = await frameState(tabId, "chat-room", `({
        ok: true,
        browserAccessStageVisible: (() => {
          const node = doc.querySelector('#browser-access-stage');
          if (!node || node.hidden) return false;
          return win.getComputedStyle(node).display !== 'none';
        })(),
        chatHeaderPresent: !!doc.querySelector('.chat-header'),
        errorText: doc.querySelector('#error-text')?.textContent?.trim() || "",
        chatCardHidden: doc.querySelector('#chat-card')?.hidden ?? true,
        messageInputDisabled: doc.querySelector('#message-input')?.disabled ?? true,
        sendButtonDisabled: doc.querySelector('#send-button')?.disabled ?? true,
        participantCountText: doc.querySelector('#participant-count')?.textContent?.trim() || "",
        messageListText: doc.querySelector('#message-list')?.textContent?.trim() || "",
        participantRows: [...doc.querySelectorAll('.participant')].map((node) => ({
          name: node.querySelector('.participant-name')?.textContent?.trim() || "",
          badge: node.querySelector('.participant-badge')?.textContent?.trim() || "",
          detail: node.querySelector('.participant-detail')?.textContent?.trim() || "",
        })),
        emojiLabels: [...doc.querySelectorAll('.emoji-chip')].map((node) => node.getAttribute('aria-label') || ""),
        innerHeight: win.innerHeight || 0,
        chatCardBottom: doc.querySelector('#chat-card')?.getBoundingClientRect?.().bottom ?? 0,
        conversationBottom: doc.querySelector('.conversation-card')?.getBoundingClientRect?.().bottom ?? 0,
        conversationHeight: doc.querySelector('.conversation-card')?.getBoundingClientRect?.().height ?? 0,
        presenceHeight: doc.querySelector('.presence-card')?.getBoundingClientRect?.().height ?? 0,
      })`);
      assert(room.ok, "chat-room frame was not reachable", room);
      assert(room.browserAccessStageVisible === false, "chat-room should not ship browser-access request UI in shell mode", room);
      assert(room.chatHeaderPresent === false, "chat-room still renders the old room header", room);
      assert(room.chatCardHidden === false, "chat-room hid the room surface after launch", room);
      assert(!room.errorText.includes("Carrier room sync unavailable"), "chat-room exposed raw carrier-sync failure copy", room);
      assert(!room.errorText.includes("Local room history is available"), "chat-room exposed transport-centric shell copy", room);
      assert(!room.messageListText.includes("Opening room"), "chat-room still renders an opening placeholder in the message list", room);
      assert(room.participantCountText.startsWith("People · "), "chat-room roster summary did not use the people ontology", room);
      const localParticipant = room.participantRows.find((participant) => participant.badge === "You");
      assert(localParticipant, "chat-room did not mark the shell participant as You", room);
      assert(!localParticipant.detail.includes("guest browser"), "chat-room mislabeled the shell participant as a guest browser", room);
      assert(room.emojiLabels.includes("Wave") && room.emojiLabels.includes("Heart"), "chat-room emoji actions lacked stable labels", room);
      assert(room.messageInputDisabled === false, "chat-room composer stayed disabled", room);
      assert(room.sendButtonDisabled === false, "chat-room send button stayed disabled", room);
      assert(room.chatCardBottom <= room.innerHeight + 2, "chat-room Home surface overflowed the window height", room);
      assert(Math.abs(room.conversationHeight - room.presenceHeight) <= 2, "chat conversation and people panes are not the same height", room);
    });

    await runCase("inbox-app-approves-browser-access-request", async (tabId) => {
      await cleanupRoomState(tabId);

      await openLauncher(tabId);
      await delay(1000);
      await activate(tabId, '.launcher-card[data-target="chat-room"]');
      await waitFor(async () => {
        const room = await frameState(tabId, "chat-room", `({
          ok: true,
          messageInputDisabled: doc.querySelector('#message-input')?.disabled ?? true,
        })`);
        return room.ok && room.messageInputDisabled === false;
      }, 12000, 500);

      const pair = await browserJson(tabId, "/api/browser/session/request", {
        method: "POST",
        body: JSON.stringify({
          display_name: "Incognito QA",
          device_label: "Incognito Browser",
          capabilities: ["room.access"],
        }),
      });
      assert(pair.request_id, "browser access request did not return a request id", pair);

      await waitFor(async () => {
        const state = await shellState(tabId);
        return state.inboxBadge === "1";
      }, 12000, 500);

      await click(tabId, "#toolbar-inbox");
      await waitFor(async () => {
        const inbox = await frameState(tabId, "inbox", `({
          ok: true,
          title: doc.title || "",
          entries: [...doc.querySelectorAll(".entry")].map((node) => ({
            title: node.querySelector(".entry-title")?.textContent?.trim() || "",
            body: node.querySelector(".entry-body")?.textContent?.trim() || "",
            actions: [...node.querySelectorAll(".entry-action")].map((button) => button.textContent?.trim() || ""),
          })),
          sidebarEyebrow: doc.querySelector(".sidebar .eyebrow")?.textContent?.trim() || "",
          hasSidebarTitle: !!doc.querySelector(".sidebar h1"),
          hasSidebarSubtitle: !!doc.querySelector(".sidebar .subtitle"),
          toolbarText: doc.querySelector(".toolbar")?.textContent || "",
          pendingCount: doc.querySelector("#pending-count")?.textContent?.trim() || "",
          lockedVisible: !!doc.querySelector("#locked-shell") && !doc.querySelector("#locked-shell").classList.contains("hidden"),
        })`);
        return inbox.ok
          && inbox.title === "Inbox · ElastOS"
          && inbox.lockedVisible === false
          && inbox.pendingCount === "1"
          && inbox.entries.some((entry) => entry.title.includes("Incognito QA") && entry.actions.includes("Approve"));
      }, 12000, 500);

      const state = await shellState(tabId);
      assert(state.homeStatus === "ready", "Home did not reach ready state before inbox review", state);
      assert(
        state.windows.some((window) => window.target === "inbox" && window.title === "Inbox"),
        "toolbar inbox did not open the Inbox app",
        state,
      );
      const inboxChrome = await frameState(tabId, "inbox", `({
        ok: true,
        sidebarEyebrow: doc.querySelector(".sidebar .eyebrow")?.textContent?.trim() || "",
        hasSidebarTitle: !!doc.querySelector(".sidebar h1"),
        hasSidebarSubtitle: !!doc.querySelector(".sidebar .subtitle"),
        toolbarText: doc.querySelector(".toolbar")?.textContent || "",
      })`);
      assert(inboxChrome.sidebarEyebrow === "Inbox", "Inbox sidebar eyebrow should be Inbox", inboxChrome);
      assert(!inboxChrome.hasSidebarTitle && !inboxChrome.hasSidebarSubtitle, "Inbox sidebar should not duplicate title copy", inboxChrome);
      assert(!inboxChrome.toolbarText.includes("Queue"), "Inbox toolbar should not expose Queue copy", inboxChrome);

      const approveSelector = `.entry-action[data-action-id="room-approve-request:${pair.request_id}"]`;
      try {
        await clickInFrame(tabId, "inbox", approveSelector);
      } catch (error) {
        await clickInFrame(tabId, "inbox", approveSelector);
      }

      await waitFor(async () => {
        const inbox = await frameState(tabId, "inbox", `({
          ok: true,
          entries: [...doc.querySelectorAll(".entry")].map((node) => node.querySelector(".entry-title")?.textContent?.trim() || ""),
          pendingCount: doc.querySelector("#pending-count")?.textContent?.trim() || "",
        })`);
        return inbox.ok && inbox.pendingCount === "0" && !inbox.entries.some((title) => title.includes("Incognito QA"));
      }, 12000, 500);

      const approved = await browserJson(tabId, `/api/browser/session/request/${pair.request_id}`);
      assert(approved.status === "approved", "browser access request was not approved by Inbox", approved);
    });

    await runCase("launcher-card-opens-documents", async (tabId) => {
      await openLauncher(tabId);
      await delay(1000);
      await activate(tabId, '.launcher-card[data-target="documents"]');
      await waitFor(async () => {
        const viewer = await frameState(tabId, "documents", `({
          ok: true,
          title: doc.title || "",
          shellVisible: !!doc.querySelector("#documents-shell") && !doc.querySelector("#documents-shell").classList.contains("hidden"),
          shareVisible: !!doc.querySelector("#share-shell") && !doc.querySelector("#share-shell").classList.contains("hidden"),
        })`);
        return viewer.ok && viewer.shellVisible && !viewer.shareVisible && viewer.title.includes("Documents");
      }, 12000, 500);
      const viewer = await frameState(tabId, "documents", `({
        ok: true,
        title: doc.title || "",
        shellVisible: !!doc.querySelector("#documents-shell") && !doc.querySelector("#documents-shell").classList.contains("hidden"),
        currentTitle: doc.querySelector("#title-input")?.value || "",
        hasSearch: !!doc.querySelector("#sidebar-search"),
        hasSidebarControls: !!doc.querySelector("#documents-sidebar .sidebar-controls"),
        newButtonText: doc.querySelector("#new-document")?.textContent?.trim() || "",
        newButtonLabel: doc.querySelector("#new-document")?.getAttribute("aria-label") || "",
        sidebarMetaExists: !!doc.querySelector("#documents-sidebar .sidebar-meta"),
        documentsCountExists: !!doc.querySelector("#documents-count"),
        hasWriteMode: !!doc.querySelector("#mode-write"),
        hasSplitMode: !!doc.querySelector("#mode-split"),
        hasReadMode: !!doc.querySelector("#mode-read"),
        hasOutlineTab: !!doc.querySelector("#pane-outline"),
        hasHistoryTab: !!doc.querySelector("#pane-history"),
        workspaceView: doc.querySelector("#documents-shell")?.dataset.workspaceView || "",
        sidebarTitleExists: !!doc.querySelector("#documents-sidebar .sidebar-title"),
        sidebarSubtitleExists: !!doc.querySelector("#documents-sidebar .sidebar-subtitle"),
        actionLabels: [...doc.querySelectorAll(".toolbar-actions button")]
          .filter((button) => !button.classList.contains("hidden") && win.getComputedStyle(button).display !== "none")
          .map((button) => button.getAttribute("aria-label") || ""),
        actionCount: doc.querySelectorAll(".toolbar-actions .action-icon-button").length,
        editorWidth: doc.querySelector(".editor-pane")?.getBoundingClientRect().width || 0,
        previewWidth: doc.querySelector(".preview-pane")?.getBoundingClientRect().width || 0,
        editorHeaderHeight: doc.querySelector(".editor-pane .pane-header")?.getBoundingClientRect().height || 0,
        inspectorHeaderHeight: doc.querySelector(".inspector-pane .pane-header")?.getBoundingClientRect().height || 0,
        historyInside: (() => {
          const history = doc.querySelector("#pane-history")?.getBoundingClientRect();
          const pane = doc.querySelector(".inspector-pane .pane-header")?.getBoundingClientRect();
          return !!history && !!pane && history.right <= pane.right + 1;
        })(),
        hasDocumentPill: !!doc.querySelector("#object-uri-pill"),
        hasPublishedPill: !!doc.querySelector("#published-pill"),
        hasLocalStateChip: !!doc.querySelector("#local-state-chip"),
        hasUpdatedText: !!doc.querySelector("#updated-text"),
        toolbarText: doc.querySelector(".documents-toolbar")?.textContent || "",
        shellText: doc.querySelector("#documents-shell")?.textContent || "",
        titleDisabled: !!doc.querySelector("#title-input")?.disabled,
        editorDisabled: !!doc.querySelector("#editor")?.disabled,
        saveDisabled: !!doc.querySelector("#save-button")?.disabled,
        saveAsDisabled: !!doc.querySelector("#save-as-button")?.disabled,
        unpublishDisabled: !!doc.querySelector("#unpublish-button")?.disabled,
        activeDocDid: doc.querySelector(".document-list-item.active")?.dataset.docDid || "",
      })`);
      assert(viewer.ok, "documents frame was not reachable", viewer);
      assert(viewer.shellVisible, "documents did not open in shell editor mode", viewer);
      assert(viewer.currentTitle === "", "Documents launcher open should not silently select a document", viewer);
      assert(viewer.activeDocDid === "", "Documents launcher open should not leave an active document selected", viewer);
      assert(!viewer.titleDisabled && !viewer.editorDisabled, "Documents launcher open should start an editable unsaved draft", viewer);
      assert(viewer.saveDisabled && viewer.saveAsDisabled && viewer.unpublishDisabled, "Empty Documents draft should not expose persisted-document actions", viewer);
      assert(!viewer.hasDocumentPill && !viewer.hasPublishedPill, "Documents toolbar should not render duplicate document/published pills", viewer);
      assert(!viewer.hasLocalStateChip && !viewer.hasUpdatedText, "Documents title area should not render duplicate draft or last-saved text", viewer);
      assert(!viewer.toolbarText.includes("localhost://Users/self/Documents/"), "documents foregrounded raw working-copy storage", viewer);
      assert(!viewer.shellText.includes("Start writing, then save."), "Documents shell should not show redundant draft instruction copy", viewer);
      assert(viewer.hasSearch, "documents sidebar search is missing", viewer);
      assert(viewer.hasSidebarControls && viewer.newButtonText === "+" && viewer.newButtonLabel === "New document", "Documents sidebar should use compact create control beside search", viewer);
      assert(!viewer.sidebarMetaExists && !viewer.documentsCountExists, "Documents sidebar should not duplicate collection labels or counts", viewer);
      assert(viewer.hasWriteMode && viewer.hasSplitMode && viewer.hasReadMode, "documents workspace mode controls are incomplete", viewer);
      assert(viewer.hasOutlineTab && viewer.hasHistoryTab, "documents inspector tabs are incomplete", viewer);
      assert(viewer.workspaceView === "split", "documents should default to split view on desktop shell", viewer);
      assert(!viewer.sidebarTitleExists && !viewer.sidebarSubtitleExists, "Documents sidebar should not duplicate title copy", viewer);
      assert(viewer.actionLabels.join("|") === "Save|Save as|Publish|Unpublish|Delete|Hide list", "Documents toolbar actions drifted", viewer);
      assert(viewer.actionCount === 6, "Documents toolbar actions should be compact icon buttons", viewer);
      assert(viewer.actionLabels.at(-1) === "Hide list", "Documents Hide list control should stay at the far right of toolbar actions", viewer);
      assert(Math.abs(viewer.editorWidth - viewer.previewWidth) <= 2, "Documents split view should be even", viewer);
      assert(Math.abs(viewer.editorHeaderHeight - viewer.inspectorHeaderHeight) <= 2, "Documents inspector tabs should not make the header taller than Write", viewer);
      assert(viewer.historyInside, "Documents History tab overflowed the inspector header", viewer);
    });

    await runCase("elastos-document-uri-opens-documents-viewer", async (tabId) => {
      const uri = `elastos://${TEST_DOCUMENT_CID}`;
      await openElastosUri(tabId, uri);
      const opened = await waitFor(async () => {
        const routes = await evaluate(tabId, `(() => [...document.querySelectorAll('.window[data-target="documents"] .window-frame')]
          .map((frame) => frame.getAttribute("src") || ""))()`);
        return routes.some((route) => (
          route.startsWith("/apps/documents/?home_token=") &&
          route.includes("cid=" + TEST_DOCUMENT_CID) &&
          route.includes("view=read") &&
          route.includes("uri=elastos%3A%2F%2F" + TEST_DOCUMENT_CID)
        ));
      }, 12000, 500);
      const state = await shellState(tabId);
      assert(opened, "elastos:// document URI did not open a Documents viewer route", state);
      assert(
        state.windows.some((window) => window.target === "documents" && window.title === "Documents"),
        "elastos:// document URI did not open the Documents capsule",
        state,
      );
    });

    await runCase("published-document-link-shared-in-chat-opens-documents", async (tabId) => {
      await cleanupRoomState(tabId);
      await openShellTarget(tabId, "documents");
      await waitFor(async () => {
        const viewer = await frameState(tabId, "documents", `({
          ok: true,
          shellVisible: !!doc.querySelector("#documents-shell") && !doc.querySelector("#documents-shell").classList.contains("hidden"),
          titleDisabled: !!doc.querySelector("#title-input")?.disabled,
          editorDisabled: !!doc.querySelector("#editor")?.disabled,
        })`);
        return viewer.ok && viewer.shellVisible && !viewer.titleDisabled && !viewer.editorDisabled;
      }, 12000, 500);

      const draftTitle = `Chat share ${Date.now()}`;
      const drafted = await frameState(tabId, "documents", `(() => {
        const title = doc.querySelector("#title-input");
        const editor = doc.querySelector("#editor");
        title.value = ${JSON.stringify(draftTitle)};
        editor.value = "# " + title.value + "\\n\\nShared from Documents through Chat Room.";
        title.dispatchEvent(new Event("input", { bubbles: true }));
        editor.dispatchEvent(new Event("input", { bubbles: true }));
        return {
          ok: true,
          saveDisabled: doc.querySelector("#save-button")?.disabled ?? true,
          publishDisabled: doc.querySelector("#publish-button")?.disabled ?? true,
        };
      })()`);
      assert(drafted.ok && !drafted.saveDisabled && !drafted.publishDisabled, "Documents draft was not ready to save and publish", drafted);

      const saved = await clickInFrame(tabId, "documents", "#save-button");
      assert(saved.ok, "Documents Save action was not clickable", saved);
      let savedDocument = null;
      await waitFor(async () => {
        savedDocument = await frameState(tabId, "documents", `({
          ok: true,
          status: doc.querySelector("#status-text")?.textContent?.trim() || "",
          activeDocDid: doc.querySelector(".document-list-item.active")?.dataset.docDid || "",
          saveDisabled: doc.querySelector("#save-button")?.disabled ?? false,
        })`);
        return savedDocument.ok && savedDocument.status === "Saved." && savedDocument.activeDocDid.length > 0 && savedDocument.saveDisabled;
      }, 12000, 500);
      assert(savedDocument?.activeDocDid, "Documents did not expose the saved document DID", savedDocument);

      const published = await clickInFrame(tabId, "documents", "#publish-button");
      assert(published.ok, "Documents Publish action was not clickable", published);
      let publishedLink = null;
      const publishResolved = await waitFor(async () => {
        publishedLink = await frameState(tabId, "documents", `({
          ok: true,
          status: doc.querySelector("#status-text")?.textContent?.trim() || "",
          uri: doc.querySelector("#copy-published-link")?.dataset.copyUri || "",
          text: doc.querySelector("#copy-published-link")?.textContent?.trim() || "",
          hidden: doc.querySelector("#copy-published-link")?.classList.contains("hidden") ?? true,
          disabled: doc.querySelector("#copy-published-link")?.disabled ?? true,
        })`);
        return isPublishedDocumentLinkReady(publishedLink) || isDocumentPublishUnavailable(publishedLink);
      }, 45_000, 1000);
      if (isDocumentPublishUnavailable(publishedLink)) {
        const deletedLocal = await deleteDocumentWorkingCopy(tabId, savedDocument.activeDocDid);
        assert(deletedLocal.ok, "Documents smoke could not remove its local working copy", deletedLocal);
        assert(!REQUIRE_DOCUMENT_PUBLISH, "Documents publishing is unavailable in strict publish mode", publishedLink);
        throw new SkipCase("document publishing is unavailable on this device", publishedLink);
      }
      assert(publishResolved && isPublishedDocumentLinkReady(publishedLink), "Documents did not publish an elastos:// revision", publishedLink);

      const uri = publishedLink.uri;
      const cid = uri.slice("elastos://".length);
      const copied = await frameState(tabId, "documents", `(() => {
        const button = doc.querySelector("#copy-published-link");
        if (!button) return { ok: false, reason: "missing-copy-button" };
        button.click();
        return { ok: true };
      })()`);
      assert(copied.ok, "Documents Copy link action could not be started", copied);
      let copiedState = null;
      const didCopy = await waitFor(async () => {
        copiedState = await frameState(tabId, "documents", `({
          ok: true,
          copiedUri: doc.querySelector("#copy-published-link")?.dataset.copyUri || "",
          status: doc.querySelector("#status-text")?.textContent?.trim() || "",
        })`);
        return copiedState.ok && copiedState.copiedUri === uri && copiedState.status === "Copied link.";
      }, 4000, 250);
      assert(didCopy, "Documents Copy link did not copy the elastos:// URI", copiedState);

      await openShellTarget(tabId, "chat-room");
      await waitFor(async () => {
        const room = await frameState(tabId, "chat-room", `({
          ok: true,
          inputDisabled: doc.querySelector("#message-input")?.disabled ?? true,
          sendDisabled: doc.querySelector("#send-button")?.disabled ?? true,
        })`);
        return room.ok && !room.inputDisabled && !room.sendDisabled;
      }, 12000, 500);

      const attachOpened = await clickInFrame(tabId, "chat-room", "#attach-button");
      assert(attachOpened.ok, "Chat Room Attach was not clickable for document sharing", attachOpened);
      let picker = null;
      const pickerReady = await waitFor(async () => {
        picker = await frameState(tabId, "library", `(() => {
          const attachButton = [...doc.querySelectorAll(".entry-open")]
            .find((button) => button.dataset.attachUri === ${JSON.stringify(uri)});
          return {
            ok: true,
            mode: new URL(win.location.href).searchParams.get("mode") || "",
            returnTarget: new URL(win.location.href).searchParams.get("returnTarget") || "",
            mainTitle: doc.querySelector("#main-title")?.textContent?.trim() || "",
            attachButtonText: attachButton?.textContent?.trim() || "",
            attachButtonDisabled: attachButton?.disabled ?? true,
          };
        })()`);
        return picker.ok
          && picker.mode === "attach"
          && picker.returnTarget === "chat-room"
          && picker.attachButtonText === "Attach"
          && picker.attachButtonDisabled === false;
      }, 12000, 500);
      assert(pickerReady, "Library did not expose the published document as an attachable item", picker);

      const selected = await frameState(tabId, "library", `(() => {
        const attachButton = [...doc.querySelectorAll(".entry-open")]
          .find((button) => button.dataset.attachUri === ${JSON.stringify(uri)});
        if (!attachButton) return { ok: false, reason: "missing-attach-button" };
        attachButton.click();
        return { ok: true };
      })()`);
      assert(selected.ok, "Library could not attach the published document to Chat Room", selected);
      await waitFor(async () => {
        const state = await shellState(tabId);
        return !state.windows.some((window) => window.target === "library");
      }, 6000, 250);

      let linkCard = null;
      const renderedLink = await waitFor(async () => {
        linkCard = await frameState(tabId, "chat-room", `(() => {
          const anchor = [...doc.querySelectorAll("#message-list [data-open-uri]")]
            .find((node) => node.dataset.openUri === ${JSON.stringify(uri)});
          return {
            ok: true,
            found: !!anchor,
            href: anchor?.getAttribute("href") || "",
            title: anchor?.querySelector(".link-title")?.textContent?.trim() || "",
            detail: anchor?.querySelector(".link-detail")?.textContent?.trim() || "",
            target: anchor?.getAttribute("target") || "",
          };
        })()`);
        return linkCard.ok && linkCard.found;
      }, 12000, 500);
      assert(renderedLink, "Chat Room did not render the elastos:// document as an openable link", linkCard);
      assert(linkCard.href === uri && linkCard.detail === "Documents" && linkCard.target === "", "Chat Room document link chrome drifted", linkCard);

      const opened = await clickInFrame(tabId, "chat-room", `#message-list [data-open-uri="${uri}"]`);
      assert(opened.ok, "Chat Room document link was not clickable", opened);
      const openedViewer = await waitFor(async () => {
        const routes = await evaluate(tabId, `(() => [...document.querySelectorAll('.window[data-target="documents"] .window-frame')]
          .map((frame) => frame.getAttribute("src") || ""))()`);
        return routes.some((route) => (
          route.startsWith("/apps/documents/?home_token=") &&
          route.includes("cid=" + cid) &&
          route.includes("view=read") &&
          route.includes("uri=" + encodeURIComponent(uri))
        ));
      }, 12000, 500);
      assert(openedViewer, "Chat Room elastos:// link did not open the Documents capsule viewer", await shellState(tabId));
      const openedLocalDocument = await waitFor(async () => {
        const documents = await evaluate(tabId, `(() => [...document.querySelectorAll('.window[data-target="documents"] .window-frame')].map((frame) => {
          const frameDoc = frame.contentWindow?.document;
          const shell = frameDoc?.querySelector("#documents-shell");
          const share = frameDoc?.querySelector("#share-shell");
          return {
            shellVisible: !!shell && !shell.classList.contains("hidden"),
            shareVisible: !!share && !share.classList.contains("hidden"),
            title: frameDoc?.querySelector("#title-input")?.value || "",
            workspaceView: shell?.dataset.workspaceView || "",
            error: frameDoc?.querySelector(".error-state")?.textContent?.trim() || "",
          };
        }))()`);
        return documents.some((item) => item.shellVisible && !item.shareVisible && item.title === draftTitle && item.workspaceView === "read" && !item.error.includes("Could not load"));
      }, 12000, 500);
      assert(openedLocalDocument, "Chat Room elastos:// link did not resolve the local published document before public CID loading", await shellState(tabId));
      const deletedLocal = await deleteDocumentWorkingCopy(tabId, savedDocument.activeDocDid);
      assert(deletedLocal.ok, "Documents smoke could not remove its local working copy", deletedLocal);
    });

    await runCase("shell-iframe-messages-require-frame-capability", async (tabId) => {
      await openShellTarget(tabId, "documents");
      await waitFor(async () => {
        const viewer = await frameState(tabId, "documents", `({
          ok: true,
          shellVisible: !!doc.querySelector("#documents-shell") && !doc.querySelector("#documents-shell").classList.contains("hidden"),
        })`);
        return viewer.ok && viewer.shellVisible;
      }, 12000, 500);

      let unauthorized = null;
      const unauthorizedSent = await waitFor(async () => {
        unauthorized = await frameState(tabId, "documents", `(() => {
          const sendFromFrame = (message) => {
            win.eval("window.parent.postMessage(" + JSON.stringify(message) + ", window.location.origin)");
          };
          sendFromFrame({
            type: "home:open-target",
            target: "system",
            query: {},
          });
          sendFromFrame({
            type: "home:open-target",
            target: "system",
            query: {},
            homeToken: "not-a-valid-token",
          });
          const homeToken = new URL(win.location.href).searchParams.get("home_token") || "";
          sendFromFrame({
            type: "home:open-target",
            target: "system",
            query: {},
            homeToken,
          });
          return { ok: true, hasHomeToken: homeToken.length > 0 };
        })()`);
        return unauthorized.ok && unauthorized.hasHomeToken;
      }, 7000, 300);
      assert(unauthorizedSent, "Documents frame did not expose its app launch token", unauthorized);
      await delay(800);
      let state = await shellState(tabId);
      assert(
        !state.windows.some((window) => window.target === "system"),
        "Home accepted an unauthorized frame open-target message",
        state,
      );

      const uri = `elastos://${TEST_DOCUMENT_CID}`;
      let authorized = null;
      const authorizedSent = await waitFor(async () => {
        authorized = await frameState(tabId, "documents", `(() => {
          const homeToken = new URL(win.location.href).searchParams.get("home_token") || "";
          const message = {
            type: "home:open-uri",
            uri: ${JSON.stringify(uri)},
            preferredViewer: "documents",
            homeToken,
          };
          win.eval("window.parent.postMessage(" + JSON.stringify(message) + ", window.location.origin)");
          return { ok: true };
        })()`);
        return authorized.ok;
      }, 7000, 300);
      assert(authorizedSent, "Documents frame could not send authorized open-uri message", authorized);
      const opened = await waitFor(async () => {
        const routes = await evaluate(tabId, `(() => [...document.querySelectorAll('.window[data-target="documents"] .window-frame')]
          .map((frame) => frame.getAttribute("src") || ""))()`);
        return routes.some((route) => (
          route.includes("cid=" + TEST_DOCUMENT_CID) &&
          route.includes("uri=elastos%3A%2F%2F" + TEST_DOCUMENT_CID)
        ));
      }, 12000, 500);
      state = await shellState(tabId);
      assert(opened, "Home rejected an authorized Documents open-uri message", state);
    });

    await runCase("documents-delete-removes-current-document", async (tabId) => {
      await openShellTarget(tabId, "documents");
      await waitFor(async () => {
        const viewer = await frameState(tabId, "documents", `({
          ok: true,
          shellVisible: !!doc.querySelector("#documents-shell") && !doc.querySelector("#documents-shell").classList.contains("hidden"),
        })`);
        return viewer.ok && viewer.shellVisible;
      }, 12000, 500);
      const draft = await frameState(tabId, "documents", `(() => {
        const title = doc.querySelector("#title-input");
        const editor = doc.querySelector("#editor");
        if (!title || !editor) return { ok: false, reason: "missing-editor" };
        title.value = "Delete smoke";
        editor.value = "# Delete smoke\\n";
        title.dispatchEvent(new Event("input", { bubbles: true }));
        editor.dispatchEvent(new Event("input", { bubbles: true }));
        return {
          ok: true,
          saveDisabled: !!doc.querySelector("#save-button")?.disabled,
          activeDocDid: doc.querySelector(".document-list-item.active")?.dataset.docDid || "",
        };
      })()`);
      assert(draft.ok && draft.saveDisabled === false && draft.activeDocDid === "", "Documents did not prepare an unsaved deletable draft", draft);
      const saved = await clickInFrame(tabId, "documents", "#save-button");
      assert(saved.ok, "Documents draft save was not clickable", saved);
      await waitFor(async () => {
        const viewer = await frameState(tabId, "documents", `({
          ok: true,
          activeDocDid: doc.querySelector(".document-list-item.active")?.dataset.docDid || "",
          deleteDisabled: !!doc.querySelector("#delete-button")?.disabled,
          saveDisabled: !!doc.querySelector("#save-button")?.disabled,
        })`);
        return viewer.ok && viewer.activeDocDid.length > 0 && viewer.deleteDisabled === false && viewer.saveDisabled === true;
      }, 12000, 500);
      const created = await frameState(tabId, "documents", `({
        ok: true,
        activeDocDid: doc.querySelector(".document-list-item.active")?.dataset.docDid || "",
      })`);
      assert(created.ok && created.activeDocDid.length > 0, "Documents did not create a deletable document", created);
      const openedDelete = await frameState(tabId, "documents", `(() => {
        const button = doc.querySelector("#delete-button");
        if (!button) return { ok: false, reason: "missing-delete-button" };
        let confirmCalled = false;
        const originalConfirm = win.confirm;
        win.confirm = () => {
          confirmCalled = true;
          return false;
        };
        button.click();
        win.confirm = originalConfirm;
        const modal = doc.querySelector("#confirm-modal");
        const action = doc.querySelector("#confirm-action");
        return {
          ok: true,
          modalOpen: modal?.classList.contains("open") || false,
          title: doc.querySelector("#confirm-title")?.textContent?.trim() || "",
          titleHidden: !!doc.querySelector("#confirm-title")?.hidden,
          actionLabel: action?.textContent?.trim() || "",
          confirmCalled,
        };
      })()`);
      assert(openedDelete.ok && openedDelete.modalOpen, "Documents delete did not open the in-capsule confirmation", openedDelete);
      assert(openedDelete.title === "" && openedDelete.titleHidden, "Documents delete confirmation should not repeat a title", openedDelete);
      assert(openedDelete.actionLabel === "Delete", "Documents delete confirmation action drifted", openedDelete);
      assert(openedDelete.confirmCalled === false, "Documents delete still used the browser confirmation API", openedDelete);
      const confirmedDelete = await clickInFrame(tabId, "documents", "#confirm-action");
      assert(confirmedDelete.ok, "Documents delete confirmation was not clickable", confirmedDelete);
      await waitFor(async () => {
        const viewer = await frameState(tabId, "documents", `({
          ok: true,
          status: doc.querySelector("#status-text")?.textContent?.trim() || "",
          activeDocDid: doc.querySelector(".document-list-item.active")?.dataset.docDid || "",
          docDids: [...doc.querySelectorAll(".document-list-item")].map((node) => node.dataset.docDid || ""),
          titleDisabled: !!doc.querySelector("#title-input")?.disabled,
          editorDisabled: !!doc.querySelector("#editor")?.disabled,
          hasDocumentPill: !!doc.querySelector("#object-uri-pill"),
          deleteDisabled: !!doc.querySelector("#delete-button")?.disabled,
        })`);
        return viewer.ok
          && viewer.status === "Deleted."
          && viewer.activeDocDid === ""
          && !viewer.docDids.includes(created.activeDocDid)
          && !viewer.titleDisabled
          && !viewer.editorDisabled
          && !viewer.hasDocumentPill
          && viewer.deleteDisabled;
      }, 12000, 500);
    });

    await runCase("documents-unknown-doc-did-fails-closed", async (tabId) => {
      await openShellTarget(tabId, "documents");
      await waitFor(async () => {
        const viewer = await frameState(tabId, "documents", `({
          ok: true,
          shellVisible: !!doc.querySelector("#documents-shell") && !doc.querySelector("#documents-shell").classList.contains("hidden"),
        })`);
        return viewer.ok && viewer.shellVisible;
      }, 12000, 500);
      const badDocDid = "missing-smoke-doc-did";
      const redirected = await frameState(tabId, "documents", `(() => {
        const url = new URL(win.location.href);
        url.searchParams.set("doc", ${JSON.stringify(badDocDid)});
        win.location.href = url.href;
        return { ok: true, href: url.href };
      })()`);
      assert(redirected.ok, "could not request an unknown document", redirected);
      await waitFor(async () => {
        let viewer;
        try {
          viewer = await frameState(tabId, "documents", `({
            ok: true,
            status: doc.querySelector("#status-text")?.textContent?.trim() || "",
            currentTitle: doc.querySelector("#title-input")?.value || "",
            activeDocDid: doc.querySelector(".document-list-item.active")?.dataset.docDid || "",
          })`);
        } catch {
          return false;
        }
        return viewer.ok && viewer.status.includes("Document not found");
      }, 12000, 500);
      const viewer = await frameState(tabId, "documents", `({
        ok: true,
        status: doc.querySelector("#status-text")?.textContent?.trim() || "",
        currentTitle: doc.querySelector("#title-input")?.value || "",
        titleDisabled: !!doc.querySelector("#title-input")?.disabled,
        editorDisabled: !!doc.querySelector("#editor")?.disabled,
        hasDocumentPill: !!doc.querySelector("#object-uri-pill"),
        activeDocDid: doc.querySelector(".document-list-item.active")?.dataset.docDid || "",
      })`);
      assert(viewer.ok, "documents frame was not reachable after unknown document request", viewer);
      assert(viewer.status.includes("Nothing opened"), "unknown document DID did not surface a fail-closed status", viewer);
      assert(viewer.currentTitle === "", "unknown document DID silently opened another document", viewer);
      assert(viewer.activeDocDid === "", "unknown document DID left a document selected", viewer);
      assert(viewer.titleDisabled && viewer.editorDisabled, "unknown document DID left editing enabled", viewer);
      assert(!viewer.hasDocumentPill, "unknown document DID should not render stale document chrome", viewer);
      assert(viewer.status.includes(`localhost://ElastOS/Documents/${badDocDid}`), "unknown document DID did not preserve the requested address in status", viewer);
    });

    await runCase("library-opens-documents", async (tabId) => {
      await openLauncher(tabId);
      await delay(1000);
      await activate(tabId, '.launcher-card[data-target="library"]');
      await waitFor(async () => {
        const files = await frameState(tabId, "library", `({
          ok: true,
          title: doc.title || "",
          count: doc.querySelectorAll('.entry').length,
          documentsCount: doc.querySelector('#objects-count')?.textContent?.trim() || "",
          draftsCount: doc.querySelector('#drafts-count')?.textContent?.trim() || "",
          firstEntryTitle: doc.querySelector('.entry-title')?.textContent?.trim() || "",
        })`);
        return files.ok && files.title === "Library · ElastOS" && files.count > 0 && files.firstEntryTitle.length > 0;
      }, 12000, 500);
      const files = await frameState(tabId, "library", `({
        ok: true,
        title: doc.title || "",
        count: doc.querySelectorAll('.entry').length,
        hasSearch: !!doc.querySelector('#search'),
        hasNewButton: !!doc.querySelector('#new-document'),
        hasCollections: !!doc.querySelector('[data-filter="all"]') && !!doc.querySelector('[data-filter="published"]'),
        mainEyebrow: doc.querySelector('.main .eyebrow')?.textContent?.trim() || "",
        activeFilter: doc.querySelector('.collection-button[data-active="true"]')?.dataset.filter || "",
        documentsCount: doc.querySelector('#objects-count')?.textContent?.trim() || "",
        draftsCount: doc.querySelector('#drafts-count')?.textContent?.trim() || "",
        currentStatus: doc.querySelector('#status-text')?.textContent?.trim() || "",
        hasDetails: !!doc.querySelector('.entry-details'),
        shellText: doc.querySelector('#library-shell')?.textContent || "",
        firstOpenLabel: doc.querySelector('.entry-open')?.textContent?.trim() || "",
      })`);
      assert(files.ok, "Library frame was not reachable", files);
      assert(files.title === "Library · ElastOS", "Library frame title mismatch", files);
      assert(files.hasSearch, "Library search is missing", files);
      assert(files.hasNewButton, "Library new-document action is missing", files);
      assert(files.hasCollections, "Library collection filters are missing", files);
      assert(files.mainEyebrow === "Documents", "Library main section eyebrow drifted", files);
      assert(files.activeFilter === "all", "Library should open on all documents", files);
      assert(files.documentsCount !== "0", "Library did not load documents", files);
      assert(files.draftsCount !== "", "Library did not expose the drafts summary", files);
      assert(files.currentStatus.length > 0, "Library did not expose a collection status line", files);
      assert(!files.hasDetails, "Library should not expose raw document details in each card", files);
      assert(!files.shellText.includes("localhost://") && !files.shellText.includes("Working copy") && !files.shellText.includes("Published revision"), "Library should not expose raw storage addresses in normal browsing", files);
      assert(files.firstOpenLabel === "Open", "Library open action drifted", files);

      const rowClick = await clickInFrame(tabId, "library", ".entry-title");
      assert(rowClick.ok, "Library title was not clickable for row-click regression", rowClick);
      await delay(600);
      let state = await shellState(tabId);
      assert(!state.windows.some((window) => window.target === "documents"), "Library card text click should not open Documents", state);

      const opened = await clickInFrame(tabId, "library", ".entry-open");
      assert(opened.ok, "Library could not trigger document open", opened);
      await waitFor(async () => {
        const current = await shellState(tabId);
        return current.windows.some((window) => window.target === "documents");
      }, 12000, 500);
      state = await shellState(tabId);
      assert(
        state.windows.some((window) => window.target === "library" && window.title === "Library"),
        "Library window disappeared after opening a document",
        state,
      );
      assert(state.windows.some((window) => window.target === "documents"), "Library did not open a document window", state);
    });

    await runCase("library-new-document-opens-unsaved-draft", async (tabId) => {
      await openLauncher(tabId);
      await delay(1000);
      await activate(tabId, '.launcher-card[data-target="library"]');
      await waitFor(async () => {
        const files = await frameState(tabId, "library", `({
          ok: true,
          count: doc.querySelectorAll('.entry').length,
        })`);
        return files.ok && files.count > 0;
      }, 12000, 500);
      const filesBefore = await frameState(tabId, "library", `({
        ok: true,
        docDids: [...doc.querySelectorAll('.entry')].map((node) => node.dataset.docDid || ""),
      })`);
      assert(filesBefore.ok, "Library frame was not ready before new-document test", filesBefore);
      const opened = await clickInFrame(tabId, "library", "#new-document");
      assert(opened.ok, "Library could not trigger new document creation", opened);
      await waitFor(async () => {
        const viewer = await frameState(tabId, "documents", `({
          ok: true,
          currentTitle: doc.querySelector('#title-input')?.value || "",
          activeDocDid: doc.querySelector('.document-list-item.active')?.dataset.docDid || "",
          hasDocumentPill: !!doc.querySelector("#object-uri-pill"),
          titleDisabled: !!doc.querySelector("#title-input")?.disabled,
          saveDisabled: !!doc.querySelector("#save-button")?.disabled,
        })`);
        return viewer.ok && !viewer.hasDocumentPill && !viewer.titleDisabled;
      }, 12000, 500);
      const viewer = await frameState(tabId, "documents", `({
        ok: true,
        currentTitle: doc.querySelector('#title-input')?.value || "",
        activeDocDid: doc.querySelector('.document-list-item.active')?.dataset.docDid || "",
        hasDocumentPill: !!doc.querySelector("#object-uri-pill"),
        titleDisabled: !!doc.querySelector("#title-input")?.disabled,
        saveDisabled: !!doc.querySelector("#save-button")?.disabled,
      })`);
      assert(viewer.ok, "Documents frame was not reachable after Library new-document handoff", viewer);
      assert(viewer.currentTitle === "" && viewer.activeDocDid === "", "Library new-document should not create a persisted object before Save", { filesBefore, viewer });
      assert(!viewer.hasDocumentPill && !viewer.titleDisabled && viewer.saveDisabled, "Library new-document should open an editable empty draft", viewer);
    });

    await runCase("launcher-includes-chat-room", async (tabId) => {
      await openLauncher(tabId);
      await delay(1000);
      const launcherTargets = await evaluate(tabId, `(() => [...document.querySelectorAll(".launcher-card")].map((node) => node.dataset.target || ""))()`);
      const desktopTargets = await evaluate(tabId, `(() => [...document.querySelectorAll(".desktop-shortcut")].map((node) => node.dataset.target || ""))()`);
      const state = await shellState(tabId);
      assert(launcherTargets.includes("chat-room"), "chat-room should be exposed in the Home launcher", launcherTargets);
      assert(desktopTargets.includes("chat-room"), "chat-room should be exposed as a desktop shortcut", desktopTargets);
      assert(launcherTargets.includes("inbox"), "Inbox should be exposed in the Home launcher", launcherTargets);
      assert(desktopTargets.includes("inbox"), "Inbox should be exposed as a Home desktop shortcut", desktopTargets);
      assert(launcherTargets.includes("library"), "Library should be exposed in the Home launcher", launcherTargets);
      assert(desktopTargets.includes("library"), "Library should be exposed as a Home desktop shortcut", desktopTargets);
      assert(launcherTargets.includes("archive-manager"), "Archive should be exposed in the Home launcher", launcherTargets);
      assert(desktopTargets.includes("archive-manager"), "Archive should be exposed as a Home desktop shortcut", desktopTargets);
      assert(state.launcherHeading === "Open", "Home launcher heading should use plain product wording", state);
      assert(state.launcherSearchPlaceholder === "Search Home", "Home launcher search should use plain product wording", state);
      assert(!state.launcherGroupHeadings.includes("All Apps"), "Home launcher should not describe mixed targets as apps", state);
      assert(state.launcherGroupHeadings.includes("Library"), "Home launcher should group content under Library", state);
      assert(!state.launcherGroupHeadings.includes("Objects"), "Home launcher should not expose raw object jargon", state);
      assert(
        state.launcherCards.every((card) => card.target !== "library" || card.group === "Library"),
        "Home launcher should group Library under Library",
        state,
      );
      assert(
        state.launcherCards.some((card) => card.target === "system" && card.title === "System"),
        "Home launcher did not label system as System",
        state,
      );
      assert(
        state.launcherCards.some((card) => card.target === "library" && card.title === "Library"),
        "Home launcher did not label library as Library",
        state,
      );
      assert(
        state.launcherCards.some((card) => card.target === "archive-manager" && card.title === "Archive"),
        "Home launcher did not label Archive as Archive",
        state,
      );
      assert(
        state.launcherCards.some((card) => card.target === "inbox" && card.title === "Inbox"),
        "Home launcher did not label inbox as Inbox",
        state,
      );
      assert(
        state.desktopShortcuts.some((shortcut) => shortcut.target === "system" && shortcut.label === "System"),
        "Home desktop did not label system as System",
        state,
      );
      assert(
        state.desktopShortcuts.some((shortcut) => shortcut.target === "library" && shortcut.label === "Library"),
        "Home desktop did not label library as Library",
        state,
      );
      assert(
        state.desktopShortcuts.some((shortcut) => shortcut.target === "archive-manager" && shortcut.label === "Archive"),
        "Home desktop did not label Archive as Archive",
        state,
      );
      assert(
        state.desktopShortcuts.some((shortcut) => shortcut.target === "inbox" && shortcut.label === "Inbox"),
        "Home desktop did not label inbox as Inbox",
        state,
      );
    });

    await runCase("archive-launch-routes-to-library", async (tabId) => {
      await openShellTarget(tabId, "archive-manager");
      const archiveReady = await waitFor(async () => {
        const state = await shellState(tabId);
        const archiveWindow = state.windows.find((window) => window.target === "archive-manager");
        if (!archiveWindow || archiveWindow.title !== "Archive") {
          return false;
        }
        const archive = await frameState(tabId, "archive-manager", `({
          ok: true,
          title: doc.title,
          openButton: !!doc.querySelector("#open-existing-archive"),
        })`);
        return archive.ok && archive.title === "Archive - ElastOS" && archive.openButton;
      }, 12000, 500);
      assert(archiveReady, "Archive launcher did not open the Archive window", await shellState(tabId));

      const clicked = await clickInFrame(tabId, "archive-manager", "#open-existing-archive");
      assert(clicked.ok, "Archive open-existing action was not clickable", clicked);
      const openedLibrary = await waitFor(async () => {
        const state = await shellState(tabId);
        return state.windows.some((window) => window.target === "library" && window.title === "Library");
      }, 12000, 500);
      assert(openedLibrary, "Home rejected Archive open-target handoff to Library", await shellState(tabId));
      const pickerMode = await frameState(tabId, "library", `({
        mode: new URL(win.location.href).searchParams.get("mode") || "",
        returnTarget: new URL(win.location.href).searchParams.get("returnTarget") || "",
        pickerButton: doc.querySelector("#picker-action-button")?.textContent?.trim() || "",
      })`);
      assert(
        pickerMode.mode === "archive-open" &&
          pickerMode.returnTarget === "archive-manager" &&
          pickerMode.pickerButton === "Open in Archive",
        "Archive handoff must open Library as an Archive picker, not as a generic Library window",
        pickerMode,
      );
    });

    await runCase("desktop-context-menu-toggles-icons", async (tabId) => {
      let opened = await dispatchDesktopBackgroundContextMenu(tabId);
      assert(opened.ok, "desktop background context menu did not dispatch", opened);
      await waitForContextAction(tabId, "toggle-desktop-icons");
      let state = await shellState(tabId);
      assert(state.contextMenuVisible, "desktop context menu did not open", state);
      assert(state.contextMenuActions.includes("toggle-desktop-icons"), "desktop context menu missing icon visibility toggle", state);
      assert(state.contextMenuActions.includes("auto-arrange"), "desktop context menu missing auto-arrange while icons are visible", state);
      assert(!state.contextMenuActions.includes("home"), "desktop context menu should not include Go Home", state);
      assert(!state.contextMenuActions.includes("launcher"), "desktop context menu should not include Open Launcher", state);
      assert(!state.contextMenuActions.includes("system"), "desktop context menu should not include Open System", state);
      let toggleLabel = await evaluate(tabId, `(() => document.querySelector('[data-context-action="toggle-desktop-icons"]')?.textContent?.trim() || "")()`);
      assert(toggleLabel === "Hide Desktop Icons", "desktop icon toggle should hide icons when visible", { toggleLabel, state });
      await activateContextAction(tabId, "toggle-desktop-icons");
      await delay(400);
      state = await shellState(tabId);
      assert(state.desktopIconsHidden && state.desktopIconsAriaHidden === "true", "desktop icon hide action did not hide icons accessibly", state);
      await refresh(tabId);
      await waitForSelector(tabId, "#desktop-shortcuts");
      await delay(900);
      state = await shellState(tabId);
      assert(state.desktopIconsHidden, "desktop icon visibility did not persist after refresh", state);

      opened = await dispatchDesktopBackgroundContextMenu(tabId);
      assert(opened.ok, "desktop background context menu did not dispatch while icons were hidden", opened);
      await waitForContextAction(tabId, "toggle-desktop-icons");
      state = await shellState(tabId);
      assert(state.contextMenuActions.includes("toggle-desktop-icons"), "hidden-icons context menu missing show action", state);
      assert(!state.contextMenuActions.includes("auto-arrange"), "hidden-icons context menu should not expose auto-arrange", state);
      toggleLabel = await evaluate(tabId, `(() => document.querySelector('[data-context-action="toggle-desktop-icons"]')?.textContent?.trim() || "")()`);
      assert(toggleLabel === "Show Desktop Icons", "desktop icon toggle should show icons when hidden", { toggleLabel, state });
      await activateContextAction(tabId, "toggle-desktop-icons");
      await delay(400);
      state = await shellState(tabId);
      assert(!state.desktopIconsHidden && state.desktopIconsAriaHidden === "false", "desktop icon show action did not restore icons accessibly", state);
    });

    await runCase("desktop-icons-remove-and-add-from-launcher", async (tabId) => {
      let opened = await dispatchDesktopContextMenu(tabId, "library");
      assert(opened.ok, "desktop Library context menu did not dispatch", opened);
      await waitForContextAction(tabId, "remove-desktop-icon");
      let state = await shellState(tabId);
      assert(state.contextMenuActions.includes("remove-desktop-icon"), "desktop icon menu must expose Remove from Desktop", state);
      assert(!state.contextMenuActions.includes("add-desktop-icon"), "desktop icon menu must not offer Add while the icon is already present", state);
      await activateContextAction(tabId, "remove-desktop-icon");
      await delay(500);
      state = await shellState(tabId);
      assert(!state.desktopShortcuts.some((shortcut) => shortcut.target === "library"), "Remove from Desktop did not remove the Library shortcut", state);

      await refresh(tabId);
      await waitForSelector(tabId, "#desktop-shortcuts");
      await delay(900);
      state = await shellState(tabId);
      assert(!state.desktopShortcuts.some((shortcut) => shortcut.target === "library"), "Removed desktop icon did not persist after refresh", state);

      await openLauncher(tabId);
      state = await shellState(tabId);
      assert(state.launcherCards.some((card) => card.target === "library"), "Removing a desktop icon must not remove it from the launcher", state);
      opened = await dispatchLauncherContextMenu(tabId, "library");
      assert(opened.ok, "launcher Library context menu did not dispatch", opened);
      await waitForContextAction(tabId, "add-desktop-icon");
      state = await shellState(tabId);
      assert(state.contextMenuActions.includes("add-desktop-icon"), "launcher menu must expose Add to Desktop for removed icons", state);
      assert(!state.contextMenuActions.includes("remove-desktop-icon"), "launcher menu must not offer Remove from Desktop for a removed icon", state);
      await activateContextAction(tabId, "add-desktop-icon");
      await delay(500);
      state = await shellState(tabId);
      assert(!state.desktopIconsHidden && state.desktopIconsAriaHidden === "false", "Add to Desktop should make desktop icons visible", state);
      assert(state.desktopShortcuts.some((shortcut) => shortcut.target === "library"), "Add to Desktop did not restore the Library shortcut", state);

      await refresh(tabId);
      await waitForSelector(tabId, "#desktop-shortcuts");
      await delay(900);
      state = await shellState(tabId);
      assert(state.desktopShortcuts.some((shortcut) => shortcut.target === "library"), "Added desktop icon did not persist after refresh", state);
    });

    await runCase("desktop-select-and-keyboard-open", async (tabId) => {
      await click(tabId, '.desktop-shortcut[data-target="system"]');
      await delay(800);
      let state = await shellState(tabId);
      assert(state.selectedDesktop.includes("system"), "desktop single click did not select the System icon", state);
      assert(state.selectedDesktopLabel === "System", "desktop selection should expose the System label", state);
      assert(state.windows.length === 0, "desktop single click should not open a window", state);
      assert(state.desktopSelectionText === "", "desktop single click left text selected", state);
      await press(tabId, "Enter");
      await delay(2200);
      state = await shellState(tabId);
      assert(
        state.windows.length === 1 &&
        state.windows[0].target === "system" &&
        state.windows[0].title === "System",
        "Enter did not open the selected System icon",
        state,
      );
    });

    await runCase("desktop-touch-icons-open-and-longpress-move", async (tabId) => {
      const touchProof = await evaluate(tabId, `(async () => {
        const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
        const firePointer = (target, type, x, y, pointerId) => {
          target.dispatchEvent(new PointerEvent(type, {
            bubbles: true,
            cancelable: true,
            pointerId,
            pointerType: "touch",
            button: 0,
            buttons: type === "pointerup" || type === "pointercancel" ? 0 : 1,
            clientX: x,
            clientY: y,
          }));
        };
        const fireClick = (target, x, y) => {
          target.dispatchEvent(new MouseEvent("click", {
            bubbles: true,
            cancelable: true,
            detail: 1,
            clientX: x,
            clientY: y,
          }));
        };

        const system = document.querySelector('.desktop-shortcut[data-target="system"]');
        const library = document.querySelector('.desktop-shortcut[data-target="library"]');
        if (!system || !library) return { ok: false, reason: "missing-shortcuts" };

        const systemRect = system.getBoundingClientRect();
        const longX = systemRect.left + systemRect.width / 2;
        const longY = systemRect.top + systemRect.height / 2;
        firePointer(system, "pointerdown", longX, longY, 201);
        await wait(650);
        const longPressMenu = {
          visible: !document.querySelector("#desktop-context-menu")?.hidden,
          actions: [...document.querySelectorAll("#desktop-context-menu [data-context-action]")].map((node) => node.dataset.contextAction || ""),
          selected: document.querySelector(".desktop-shortcut.selected")?.dataset.target || "",
        };
        firePointer(document, "pointerup", longX, longY, 201);
        fireClick(system, longX, longY);

        const before = {
          left: Number.parseFloat(library.style.left || "0"),
          top: Number.parseFloat(library.style.top || "0"),
        };
        const libRect = library.getBoundingClientRect();
        const startX = libRect.left + libRect.width / 2;
        const startY = libRect.top + libRect.height / 2;
        firePointer(library, "pointerdown", startX, startY, 202);
        await wait(650);
        firePointer(document, "pointermove", startX + 72, startY + 44, 202);
        await wait(80);
        firePointer(document, "pointerup", startX + 72, startY + 44, 202);
        fireClick(library, startX + 72, startY + 44);
        await wait(400);
        const movedLibrary = document.querySelector('.desktop-shortcut[data-target="library"]');
        const after = {
          left: Number.parseFloat(movedLibrary?.style.left || "0"),
          top: Number.parseFloat(movedLibrary?.style.top || "0"),
        };

        const tapRect = system.getBoundingClientRect();
        const tapX = tapRect.left + tapRect.width / 2;
        const tapY = tapRect.top + tapRect.height / 2;
        firePointer(system, "pointerdown", tapX, tapY, 203);
        firePointer(document, "pointerup", tapX, tapY, 203);
        fireClick(system, tapX, tapY);
        return {
          ok: true,
          longPressMenu,
          before,
          after,
          moved: Math.abs(after.left - before.left) > 20 || Math.abs(after.top - before.top) > 20,
        };
      })()`);
      assert(touchProof.ok, "desktop touch proof could not run", touchProof);
      assert(touchProof.longPressMenu.visible, "touch long-press did not open the desktop context menu", touchProof);
      assert(touchProof.longPressMenu.selected === "system", "touch long-press did not select the pressed desktop icon", touchProof);
      assert(touchProof.longPressMenu.actions.includes("open-target"), "touch long-press menu did not target the icon", touchProof);
      assert(touchProof.moved, "touch long-press drag did not move the desktop icon", touchProof);
      await waitFor(async () => {
        const state = await shellState(tabId);
        return state.windows.some((window) => window.target === "system");
      }, 12000, 500);
      const state = await shellState(tabId);
      assert(state.windows.some((window) => window.target === "system"), "touch tap did not open the desktop icon", state);
    });

    await runCase("desktop-double-click-opens", async (tabId) => {
      await dispatchDesktopDoubleClick(tabId, "gba-emulator");
      await delay(2200);
      const state = await shellState(tabId);
      assert(state.windows.length === 1 && state.windows[0].target === "gba-emulator", "desktop double click did not open gba-emulator", state);
    });

    await runCase("desktop-icon-drag-clears-text-selection", async (tabId) => {
      const proof = await evaluate(tabId, `(async () => {
        const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
        const library = document.querySelector('.desktop-shortcut[data-target="library"]');
        if (!library) return { ok: false, reason: "missing-library-shortcut" };
        const probe = document.createElement("span");
        probe.textContent = "selection probe";
        probe.style.cssText = "position:fixed;left:0;top:0;z-index:-1;user-select:text;-webkit-user-select:text;";
        document.body.appendChild(probe);
        const range = document.createRange();
        range.selectNodeContents(probe);
        const selection = window.getSelection();
        selection.removeAllRanges();
        selection.addRange(range);
        const selectedBefore = selection.toString();
        const rect = library.getBoundingClientRect();
        const startX = rect.left + rect.width / 2;
        const startY = rect.top + rect.height / 2;
        const firePointer = (target, type, x, y, pointerId) => {
          target.dispatchEvent(new PointerEvent(type, {
            bubbles: true,
            cancelable: true,
            pointerId,
            pointerType: "mouse",
            button: 0,
            buttons: type === "pointerup" || type === "pointercancel" ? 0 : 1,
            clientX: x,
            clientY: y,
          }));
        };
        firePointer(library, "pointerdown", startX, startY, 706);
        const selectedAfterDown = window.getSelection().toString();
        firePointer(document, "pointermove", startX + 54, startY + 36, 706);
        await wait(80);
        const selectedDuringDrag = window.getSelection().toString();
        const draggingClassDuring = document.body.classList.contains("dragging-target");
        firePointer(document, "pointerup", startX + 54, startY + 36, 706);
        await wait(80);
        probe.remove();
        return {
          ok: true,
          selectedBefore,
          selectedAfterDown,
          selectedDuringDrag,
          draggingClassDuring,
          draggingClassAfter: document.body.classList.contains("dragging-target"),
        };
      })()`);
      assert(proof.ok, "desktop icon drag selection proof could not run", proof);
      assert(proof.selectedBefore.length > 0, "desktop drag selection proof did not create its precondition", proof);
      assert(proof.selectedAfterDown === "" && proof.selectedDuringDrag === "", "desktop icon drag allowed label text selection", proof);
      assert(proof.draggingClassDuring && !proof.draggingClassAfter, "desktop icon drag selection guard did not track drag lifetime", proof);
    });

    await runCase("system-minimize-and-restore", async (tabId) => {
      await openLauncher(tabId);
      await delay(1000);
      await activate(tabId, '.launcher-card[data-target="system"]');
      await delay(2200);
      await clickWindowControl(tabId, "system", "minimize");
      await delay(1200);
      let state = await shellState(tabId);
      assert(state.windows.length === 1 && state.windows[0].hidden, "System minimize did not hide the window", state);
      await activateTaskbarTarget(tabId, "system");
      await delay(1500);
      state = await shellState(tabId);
      assert(state.windows.length === 1 && !state.windows[0].hidden && state.windows[0].active, "taskbar restore did not restore the System window", state);
    });

    await runCase("refresh-restores-shell-session", async (tabId) => {
      await openLauncher(tabId);
      await delay(1000);
      await activate(tabId, '.launcher-card[data-target="system"]');
      await delay(2200);
      await clickWindowControl(tabId, "system", "minimize");
      await delay(1000);
      await openLauncher(tabId);
      await delay(1000);
      await activate(tabId, '.launcher-card[data-target="documents"]');
      await delay(2200);
      await clickWindowControl(tabId, "documents", "maximize");
      await delay(900);
      let state = await shellState(tabId);
      assert(
        state.windows.some((window) => window.target === "system" && window.hidden),
        "System window was not hidden before refresh",
        state,
      );
      assert(
        state.windows.some((window) => window.target === "documents" && !window.hidden && window.maximized === "true" && window.active),
        "documents window was not visible, maximized, and active before refresh",
        state,
      );

      await refresh(tabId);
      await waitFor(async () => {
        const refreshed = await shellState(tabId);
        return (
          refreshed.windows.filter((window) => window.target === "system").length === 1 &&
          refreshed.windows.filter((window) => window.target === "documents").length === 1
        );
      }, 12000, 400);
      await delay(1200);
      state = await shellState(tabId);
      assert(
        state.windows.some((window) => window.target === "system" && window.hidden),
        "refresh did not restore the hidden System window",
        state,
      );
      assert(
        state.windows.some((window) => window.target === "documents" && !window.hidden && window.maximized === "true" && window.active),
        "refresh did not restore the visible maximized documents window",
        state,
      );
      assert(
        state.taskbar.some((item) => item.target === "system" && item.open === "true") &&
        state.taskbar.some((item) => item.target === "documents" && item.open === "true"),
        "refresh did not restore taskbar open state",
        state,
      );
    });

    await runCase("system-maximize-and-close", async (tabId) => {
      await openLauncher(tabId);
      await delay(1000);
      await activate(tabId, '.launcher-card[data-target="system"]');
      await delay(2200);
      await clickWindowControl(tabId, "system", "maximize");
      await delay(1000);
      let state = await shellState(tabId);
      assert(state.windows.length === 1 && state.windows[0].maximized === "true", "System maximize did not stick", state);
      await delay(600);
      await clickWindowControl(tabId, "system", "close");
      await delay(900);
      state = await shellState(tabId);
      assert(state.windows.length === 0, "System close did not remove the window", state);
    });

    await runCase("grouped-taskbar-menu-hides-all", async (tabId) => {
      await openShellTarget(tabId, "system");
      await delay(2200);
      await openShellTarget(tabId, "system");
      await delay(2200);
      let state = await shellState(tabId);
      assert(
        state.windows.filter((window) => window.target === "system").length === 2,
        "expected two System windows before grouped taskbar test",
        state,
      );
      const openedGroupMenu = await evaluate(tabId, `(() => {
        const count = document.querySelector("#taskbar-targets .taskbar-window-count");
        if (!count) return false;
        count.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true }));
        return true;
      })()`);
      assert(openedGroupMenu, "grouped taskbar count was not clickable", state);
      await waitFor(async () => {
        const current = await shellState(tabId);
        return current.contextMenuVisible && current.contextMenuActions.includes("hide-all-windows");
      }, 5000, 200);
      await delay(250);
      state = await shellState(tabId);
      assert(state.contextMenuVisible, "grouped taskbar menu did not open", state);
      assert(state.contextMenuActions.includes("hide-all-windows"), "grouped taskbar menu missing hide-all-windows", state);
      const hidAll = await evaluate(tabId, `(() => {
        const action = document.querySelector('#desktop-context-menu [data-context-action="hide-all-windows"]');
        if (!action) return false;
        action.click();
        return true;
      })()`);
      assert(hidAll, "grouped taskbar hide-all action was not clickable", state);
      await delay(900);
      state = await shellState(tabId);
      const systemWindows = state.windows.filter((window) => window.target === "system");
      assert(systemWindows.length === 2, "grouped taskbar hide-all lost System windows", state);
      assert(systemWindows.every((window) => window.hidden), "grouped taskbar hide-all did not hide both System windows", state);
    });

    await runCase("desktop-rename-persists", async (tabId) => {
      const renamedLabel = "System QA";
      await dispatchDesktopContextMenu(tabId, "system");
      await delay(400);
      let state = await shellState(tabId);
      assert(state.contextMenuVisible, "desktop context menu did not open for rename", state);
      assert(state.contextMenuActions.includes("rename-desktop-icon"), "rename action missing from desktop context menu", state);
      await activateContextAction(tabId, "rename-desktop-icon");
      await delay(300);
      state = await shellState(tabId);
      assert(state.renameEditorValue !== "", "rename editor did not open", state);
      await type(tabId, ".desktop-shortcut-rename", renamedLabel, { mode: "fill" });
      await press(tabId, "Enter");
      await delay(700);
      state = await shellState(tabId);
      assert(state.selectedDesktopLabel === renamedLabel, "desktop rename did not update the label", state);
      await refresh(tabId);
      await waitForSelector(tabId, "#desktop-shortcuts .desktop-shortcut[data-target=\"system\"]");
      await delay(900);
      state = await shellState(tabId);
      assert(state.selectedDesktopLabel === renamedLabel || state.desktopActiveDescendant === null, "desktop rename did not persist after refresh", state);
      const refreshedLabel = await evaluate(tabId, `(() => document.querySelector('.desktop-shortcut[data-target="system"] .desktop-shortcut-title')?.textContent?.trim() || "")()`);
      assert(refreshedLabel === renamedLabel, "desktop rename label was lost after refresh", state);
    });

    console.log(`PASS full-smoke url=${HOME_URL} user=${USER_ID}`);
  } finally {
    await cleanupSession();
  }
}

main().catch((error) => {
  console.error(error.stack || error.message || String(error));
  process.exitCode = 1;
});
