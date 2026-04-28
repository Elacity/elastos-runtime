#!/usr/bin/env node

const CAMOFOX_BASE = process.env.CAMOFOX_BASE || "http://127.0.0.1:9377";
const ELASTOS_BASE_URL = (process.env.ELASTOS_BASE_URL || "http://127.0.0.1:8090").replace(/\/+$/, "");
const HOME_URL = process.env.HOME_URL || `${ELASTOS_BASE_URL}/apps/home/`;
const HOST_ORIGIN = new URL(HOME_URL).origin;
const USER_ID = process.env.CAMOFOX_USER_ID || `system-smoke-${Date.now()}`;

function delay(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

async function waitFor(check, timeoutMs = 15_000, intervalMs = 250) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (await check()) {
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
  await fetch(`${CAMOFOX_BASE}/sessions/${USER_ID}`, { method: "DELETE" }).catch(() => {});
}

async function createTab() {
  await cleanupSession();
  await delay(250);
  const created = await request("/tabs", {
    method: "POST",
    body: JSON.stringify({
      userId: USER_ID,
      sessionKey: `system-smoke-${Date.now()}`,
      url: HOME_URL,
    }),
  });
  const tabId = created.tabId;
  await waitForSelector(tabId, '#desktop-shortcuts .desktop-shortcut[data-target="system"]', 30_000);
  const launched = await launchSystem(tabId);
  const route = new URL(launched.route, HOST_ORIGIN).toString();
  assert(route.includes("home_token="), "System launch did not mint an app token", launched);
  await evaluate(tabId, `(() => { window.location.href = ${JSON.stringify(route)}; return true; })()`);
  await waitForSelector(tabId, ".system-shell", 20_000);
  const rendered = await waitFor(async () => {
    const state = await systemState(tabId);
    return state.fieldLabels.includes("Device identity") && !state.errorText;
  }, 20_000, 300);
  assert(rendered, "System did not render from a Home-issued launch token", await systemState(tabId));
  return tabId;
}

async function closeTab(tabId) {
  await fetch(`${CAMOFOX_BASE}/tabs/${tabId}?userId=${encodeURIComponent(USER_ID)}`, {
    method: "DELETE",
  }).catch(() => {});
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

async function launchSystem(tabId) {
  return browserJson(tabId, "/api/apps/home/launch", {
    method: "POST",
    body: JSON.stringify({ target: "system" }),
  });
}

async function systemState(tabId) {
  return evaluate(
    tabId,
    `(() => ({
      title: document.title,
      shellLabel: document.querySelector(".system-shell")?.getAttribute("aria-label") || "",
      panelLabels: [...document.querySelectorAll(".system-panel h2")].map((node) => node.textContent?.trim() || ""),
      fieldLabels: [...document.querySelectorAll(".system-field dt")].map((node) => node.textContent?.trim() || ""),
      handleValue: document.querySelector("#handle-input")?.value || "",
      runtimeStatus: document.querySelector('[data-field="runtime-status"]')?.textContent?.trim() || "",
      storageStatus: document.querySelector('[data-field="storage-status"]')?.textContent?.trim() || "",
      storageNote: document.querySelector('[data-field="storage-note"]')?.textContent?.trim() || "",
      storageNoteHidden: document.querySelector('[data-field="storage-note"]')?.hidden ?? null,
      runtimeEventsPresent: !!document.querySelector('[data-field="runtime-events"]'),
      handleInputDisabled: document.querySelector('#handle-input')?.disabled ?? null,
      handleSaveDisabled: document.querySelector('#handle-save')?.disabled ?? null,
      errorText: document.querySelector(".system-error:not([hidden])")?.textContent?.trim() || "",
      bodyText: document.body?.textContent || "",
    }))()`,
  );
}

async function main() {
  await cleanupSession();
  let tabId = null;
  try {
    tabId = await createTab();
    const state = await systemState(tabId);
    assert(state.title === "System · ElastOS", "System page title mismatch", state);
    assert(state.shellLabel === "System", "System shell label mismatch", state);
    assert(state.panelLabels.includes("Identity"), "System page is missing the Identity panel", state);
    assert(state.panelLabels.includes("Runtime"), "System page is missing the Runtime panel", state);
    assert(state.panelLabels.includes("Storage"), "System page is missing the Storage panel", state);
    assert(state.fieldLabels.includes("Device identity"), "System page is missing the Device identity field", state);
    assert(state.fieldLabels.includes("Handle"), "System page is missing the Handle field", state);
    assert(state.fieldLabels.includes("Version"), "System page is missing the Version field", state);
    assert(state.fieldLabels.includes("Documents"), "System page is missing the Documents field", state);
    assert(state.errorText.length === 0, "System should not render an access error after Home launch", state);
    assert(state.handleInputDisabled === false && state.handleSaveDisabled === false, "Home-launched System should allow handle editing", state);
    assert(state.runtimeStatus.length > 0, "System runtime version should be present", state);
    assert(state.storageStatus.length > 0, "System storage status should be present", state);
    assert(state.storageNoteHidden === false, "System storage note should stay visible", state);
    assert(state.storageNote.length > 0, "System storage note should explain document storage state", state);
    assert(state.runtimeEventsPresent === false, "System should not render an untrusted runtime activity panel", state);
    assert(!state.bodyText.includes("Last Launch"), "System still renders the old launch block", state);
    assert(!state.bodyText.includes("launch did not produce a capsule id"), "System still renders stale launch-failure copy", state);
    assert(!state.bodyText.includes("Most recent runtime launch attempt"), "System still renders stale launch description wording", state);
    assert(!state.bodyText.includes("Nothing to show yet."), "System still renders placeholder runtime-event copy", state);
    console.log(`PASS system-smoke home=${HOME_URL}`);
  } catch (error) {
    console.error("FAIL system-smoke");
    console.error(error.message);
    if (error.details) {
      console.error(JSON.stringify(error.details, null, 2));
    }
    process.exitCode = 1;
  } finally {
    if (tabId) {
      await closeTab(tabId);
    }
    await cleanupSession();
  }
}

await main();
