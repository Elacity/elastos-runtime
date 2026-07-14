#!/usr/bin/env node

const CAMOFOX_BASE = process.env.CAMOFOX_BASE || "http://localhost:9377";
const ELASTOS_BASE_URL = (process.env.ELASTOS_BASE_URL || "http://localhost:8090").replace(/\/+$/, "");
const HOME_URL = process.env.HOME_URL || `${ELASTOS_BASE_URL}/apps/home/`;
const SYSTEM_URL = process.env.SYSTEM_URL || "";
const HOST_ORIGIN = new URL(HOME_URL).origin;
const USER_ID = process.env.CAMOFOX_USER_ID || `system-smoke-${Date.now()}`;
const BANNED_PUBLIC_COPY = /\b(runtime mirror|permissioned runtime|projection|schema|derived facts?|runtime facts?|capsules?|providers?|capabilit(?:y|ies)|affordances?|authority boundary|provider boundary|gate preview|runtime-owned|host-loaded|structured home intents?|provider operation|launch token|hostcall|objects?)\b/i;

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

class SkipCase extends Error {
  constructor(message, details = null) {
    super(message);
    this.details = details;
    this.skip = true;
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
      url: SYSTEM_URL || HOME_URL,
    }),
  });
  const tabId = created.tabId;
  if (SYSTEM_URL) {
    await waitForSelector(tabId, ".settings-container", 20_000);
    const rendered = await waitFor(async () => {
      const state = await systemState(tabId);
      return state.fieldLabels.includes("Device identity") && !state.errorText;
    }, 20_000, 300);
    assert(rendered, "System did not render from SYSTEM_URL", await systemState(tabId));
    return tabId;
  }
  await waitForDesktopTarget(tabId, "system", 30_000);
  const authority = await evaluate(tabId, `(() => ({
    homeAuthority: document.body?.dataset?.homeAuthority || "",
    homeStatus: document.body?.dataset?.homeStatus || ""
  }))()`);
  if (authority.homeAuthority !== "signed") {
    throw new SkipCase("System Camofox smoke requires a signed Home session or SYSTEM_URL with a System launch token", authority);
  }
  await assertSystemWindowLayout(tabId);
  const launched = await launchSystem(tabId);
  const route = new URL(launched.route, HOST_ORIGIN).toString();
  assert(route.includes("home_token="), "System launch did not mint an app token", launched);
  await evaluate(tabId, `(() => { window.location.href = ${JSON.stringify(route)}; return true; })()`);
  await waitForSelector(tabId, ".settings-container", 20_000);
  const rendered = await waitFor(async () => {
    const state = await systemState(tabId);
    return state.fieldLabels.includes("Device identity") && !state.errorText;
  }, 20_000, 300);
  assert(rendered, "System did not render from a Home-issued launch token", await systemState(tabId));
  return tabId;
}

async function assertSystemWindowLayout(tabId) {
  await evaluate(tabId, `(() => {
    const node = document.querySelector('#desktop-shortcut-system');
    node?.dispatchEvent(new MouseEvent('dblclick', { bubbles: true, cancelable: true }));
    return Boolean(node);
  })()`);
  await waitForSelector(tabId, '.window[data-target="system"] iframe.window-frame', 20_000);
  await delay(1_500);
  const state = await systemWindowState(tabId);
  assert(state.panelLabels.includes("Accounts"), "System window is missing Accounts", state);
  assert(state.panelLabels.includes("Shell"), "System window is missing Shell", state);
  assert(state.panelLabels.includes("Appearance"), "System window is missing Appearance", state);
  assert(state.panelLabels.includes("Recovery"), "System window is missing Recovery", state);
  assert(state.panelLabels.includes("Apps & Services"), "System window is missing Apps & Services", state);
  assert(state.panelLabels.includes("This Device"), "System window is missing About device details", state);
  assert(!state.panelLabels.includes("Profile"), "System window still duplicates People profile settings", state);
  assert(!state.panelLabels.includes("Local state"), "System window still has the old Local state card", state);
  assert(!state.panelLabels.includes("Networks"), "System window still has the old Networks card", state);
  assert(!state.panelLabels.includes("Runtime"), "System window still has the old Runtime card", state);
  assert(state.overflowing.length === 0, "System window has horizontally overflowing controls", state);
  assert(state.bodyScrollHeight <= state.rootClientHeight + 16, "System window needs avoidable internal vertical scroll", state);
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

async function waitForDesktopTarget(tabId, target, timeoutMs = 15_000) {
  const ready = await waitFor(async () => {
    const state = await evaluate(
      tabId,
      `(() => ({
        ready: document.body?.dataset.homeStatus === "ready",
        present: !!document.querySelector('#desktop-shortcuts .desktop-shortcut[data-target="${target}"]'),
        locked: document.body?.dataset.homeStatus === "locked",
        unlock: document.querySelector('#home-unlock-status')?.textContent?.trim() || ""
      }))()`,
    );
    if (state.locked && state.unlock) {
      throw new Error(`Home locked during smoke: ${state.unlock}`);
    }
    return state.ready && state.present;
  }, timeoutMs, 300);
  assert(ready, `Home did not render the ${target} shortcut`, { target });
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
      shellLabel: document.querySelector(".settings-container")?.getAttribute("aria-label") || "",
      panelLabels: [...document.querySelectorAll(".pc2-section-title")].map((node) => node.textContent?.trim() || ""),
      fieldLabels: [...document.querySelectorAll(".system-fields dt")].map((node) => node.textContent?.trim() || ""),
      walletControlsRemoved: !document.querySelector("#wallet-create") && !document.querySelector("#wallet-approvals"),
      runtimeStatus: document.querySelector('[data-field="runtime-status"]')?.textContent?.trim() || "",
      storageSectionPresent: !!document.querySelector('[data-settings="storage"], #webspace-list'),
      accountListPresent: !!document.querySelector('#account-list'),
      recoveryPasswordPresent: !!document.querySelector('#recovery-password'),
      recoveryPasswordPlaceholder: document.querySelector('#recovery-password')?.getAttribute('placeholder') || '',
      recoveryDownloadLabel: document.querySelector('#recovery-download')?.textContent?.trim() || '',
      recoveryImportPresent: !!document.querySelector('#recovery-import'),
      recoveryImportLabel: document.querySelector('label[for="recovery-import"]')?.textContent?.trim() || '',
      catalogPresent: !!document.querySelector('#capsule-catalog'),
      catalogGroupLabels: [...document.querySelectorAll('.catalog-group-title')].map((node) => node.textContent?.trim() || ''),
      technicalDetailsPresent: !!document.querySelector('#technical-details'),
      technicalDetailsOpen: document.querySelector('#technical-details')?.open ?? null,
      technicalDetailsLabels: [...document.querySelectorAll('.technical-section-title')].map((node) => node.textContent?.trim() || ''),
      selectedTechnicalId: document.querySelector('.technical-inspect-row.active')?.dataset.technicalInspectId || '',
      technicalOperationCount: document.querySelectorAll('.technical-operation option:not([value=""])').length,
      legacyInspectorPresent: !!document.querySelector('#inspect-list') || !!document.querySelector('#inspect-detail'),
      runtimeEventsPresent: !!document.querySelector('[data-field="runtime-events"]'),
      errorText: document.querySelector(".system-error:not([hidden])")?.textContent?.trim() || "",
      bodyText: document.body?.textContent || "",
      ordinaryText: (() => {
        const clone = document.body?.cloneNode(true);
        clone?.querySelector('#technical-details')?.remove();
        clone?.querySelectorAll('script, style').forEach((node) => node.remove());
        return clone?.textContent?.replace(/\\s+/g, ' ').trim() || '';
      })(),
      ordinaryHeadings: [...document.querySelectorAll('h1, h2')]
        .filter((node) => !node.closest('#technical-details'))
        .map((node) => node.textContent?.replace(/\\s+/g, ' ').trim() || '')
        .filter(Boolean),
    }))()`,
  );
}

async function systemWindowState(tabId) {
  return evaluate(
    tabId,
    `(() => {
      const win = document.querySelector('.window[data-target="system"]');
      const frame = win?.querySelector('iframe.window-frame');
      const doc = frame?.contentDocument;
      if (!win || !frame || !doc) {
        return { missing: true, panelLabels: [], overflowing: [] };
      }
      const root = doc.documentElement;
      const body = doc.body;
      const overflowing = [...doc.querySelectorAll('.pc2-group, .pc2-card-value, .pc2-input, .system-code, .account-table, .account-table td, .account-name-wrap, .capsule-catalog, .catalog-row, .catalog-facts, .technical-inspect-grid, .technical-inspect-row, .technical-section')]
        .filter((node) => node.scrollWidth > node.clientWidth + 2)
        .map((node) => ({
          tag: node.tagName.toLowerCase(),
          className: node.className || '',
          text: (node.textContent || '').trim().slice(0, 80),
          scrollWidth: node.scrollWidth,
          clientWidth: node.clientWidth,
        }));
      return {
        panelLabels: [...doc.querySelectorAll('.pc2-section-title, #catalog-title')].map((node) => node.textContent?.trim() || ''),
        bodyScrollHeight: body?.scrollHeight || 0,
        rootClientHeight: root?.clientHeight || 0,
        overflowing,
      };
    })()`,
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
    assert(state.panelLabels.includes("Accounts"), "System page is missing Accounts", state);
    assert(state.panelLabels.includes("Shell"), "System page is missing Shell", state);
    assert(state.panelLabels.includes("Appearance"), "System page is missing the Appearance panel", state);
    assert(state.panelLabels.includes("Recovery"), "System page is missing the Recovery panel", state);
    assert(state.panelLabels.includes("Access"), "System page is missing Access", state);
    assert(!state.panelLabels.includes("Elastos Webspace"), "System must not present app inventory as a WebSpace", state);
    assert(state.panelLabels.includes("This Device"), "System page is missing This Device", state);
    assert(state.fieldLabels.includes("Device identity"), "System page is missing the Device identity field", state);
    assert(!state.fieldLabels.includes("Display name"), "System must keep People profile settings out of System", state);
    assert(state.fieldLabels.includes("Version"), "System page is missing the Version field", state);
    assert(!state.fieldLabels.includes("Documents"), "System About must not duplicate Documents", state);
    assert(state.fieldLabels.includes("Accounts"), "System page is missing the Accounts field", state);
    assert(state.fieldLabels.includes("Recovery"), "System page is missing the Recovery field", state);
    assert(state.fieldLabels.includes("Guest access"), "System page is missing the Guest access field", state);
    assert(!state.fieldLabels.includes("Wallet"), "System page should not duplicate Wallet controls", state);
    assert(state.walletControlsRemoved, "System page should not include wallet account or approval controls", state);
    assert(state.fieldLabels.includes("Network status"), "System page is missing the Network status field", state);
    assert(!state.fieldLabels.includes("Runtime mirror"), "System page still exposes the old Runtime mirror field", state);
    assert(state.errorText.length === 0, "System should not render an access error after Home launch", state);
    assert(state.runtimeStatus.length > 0, "System runtime version should be present", state);
    assert(state.storageSectionPresent === false, "System must not expose the removed Storage section", state);
    assert(state.accountListPresent === true, "System page must expose account management", state);
    assert(state.recoveryPasswordPresent === true, "System page must expose Recovery Kit password protection", state);
    assert(state.recoveryPasswordPlaceholder === "Optional password", "Recovery Kit password input should stay concise", state);
    assert(state.recoveryDownloadLabel.toLowerCase().includes("recovery kit"), "System page must expose Recovery Kit download", state);
    assert(state.recoveryImportPresent === true, "System page must expose Recovery Kit import", state);
    assert(state.recoveryImportLabel === "Import Recovery Kit", "Recovery Kit import label drifted", state);
    assert(state.catalogPresent === true, "System page must expose Apps & Services", state);
    assert(state.technicalDetailsPresent === true, "System Security must expose Technical Details", state);
    assert(state.technicalDetailsOpen === false, "System Technical Details must be closed by default", state);
    assert(state.legacyInspectorPresent === false, "System page still exposes the old Capsule Inspector", state);
    assert(state.runtimeEventsPresent === false, "System should not render an untrusted runtime activity panel", state);
    assert(!BANNED_PUBLIC_COPY.test(state.ordinaryText), "System ordinary views expose internal narration", state);
    const duplicateHeadings = state.ordinaryHeadings.filter((heading, index, headings) => headings.indexOf(heading) !== index);
    assert(duplicateHeadings.length === 0, "System ordinary views contain duplicate headings", { duplicateHeadings, state });
    assert(!state.bodyText.includes("Last Launch"), "System still renders the old launch block", state);
    assert(!state.bodyText.includes("launch did not produce a capsule id"), "System still renders stale launch-failure copy", state);
    assert(!state.bodyText.includes("Most recent runtime launch attempt"), "System still renders stale launch description wording", state);
    assert(!state.bodyText.includes("Nothing to show yet."), "System still renders placeholder runtime-event copy", state);
    await evaluate(tabId, `(() => {
      const details = document.querySelector('#technical-details');
      if (details) details.open = true;
      return Boolean(details);
    })()`);
    const technicalLoaded = await waitFor(async () => {
      const current = await systemState(tabId);
      return current.technicalDetailsLabels.includes("Identity");
    }, 20_000, 300);
    assert(technicalLoaded, "System Technical Details did not load", await systemState(tabId));
    const technicalState = await systemState(tabId);
    assert(technicalState.technicalDetailsLabels.includes("Verification"), "System Technical Details is missing Verification status", technicalState);
    assert(!technicalState.bodyText.includes("not stamped"), "System Technical Details rendered an absent CID placeholder", technicalState);
    assert(!technicalState.bodyText.includes("not present"), "System Technical Details rendered an absent signature placeholder", technicalState);
    assert(!technicalState.bodyText.includes("No gate metadata declared"), "System Technical Details rendered absent approval metadata", technicalState);
    await evaluate(tabId, `(() => {
      document.querySelector('[data-technical-inspect-id="capsule:exit-provider"]')?.click();
      return true;
    })()`);
    const providerLoaded = await waitFor(async () => {
      const current = await systemState(tabId);
      return current.selectedTechnicalId === "capsule:exit-provider" && current.technicalDetailsLabels.includes("Approval");
    }, 20_000, 300);
    assert(providerLoaded, "Registered Exit provider did not expose Approval details", await systemState(tabId));
    assert((await systemState(tabId)).technicalOperationCount > 0, "Registered Exit provider has no previewable operations", await systemState(tabId));
    await evaluate(tabId, `(() => {
      document.querySelector('[data-technical-inspect-id="capsule:browser"]')?.click();
      return true;
    })()`);
    const appLoaded = await waitFor(async () => {
      const current = await systemState(tabId);
      return current.selectedTechnicalId === "capsule:browser" && !current.technicalDetailsLabels.includes("Approval");
    }, 20_000, 300);
    assert(appLoaded, "Ordinary Browser component incorrectly exposed Approval details", await systemState(tabId));
    console.log(`PASS system-smoke home=${HOME_URL}`);
  } catch (error) {
    if (error.skip) {
      console.log(`SKIP system-smoke ${error.message}`);
      if (error.details) {
        console.log(JSON.stringify(error.details, null, 2));
      }
      return;
    }
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
