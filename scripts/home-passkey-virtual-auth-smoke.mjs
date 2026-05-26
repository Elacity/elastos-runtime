#!/usr/bin/env node

import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createRequire } from "node:module";

const require = createRequire(new URL("../elastos/tools/browser-playwright-engine/package.json", import.meta.url));
const { chromium } = require("playwright");

const ELASTOS_BASE_URL = (process.env.ELASTOS_BASE_URL || "http://localhost:8090").replace(/\/+$/, "");
const HOME_URL = process.env.HOME_URL || `${ELASTOS_BASE_URL}/apps/home/`;
const TEST_NAME = process.env.HOME_VIRTUAL_AUTH_NAME || `Agent Smoke ${new Date().toISOString()}`;
const HEADLESS = process.env.HOME_VIRTUAL_AUTH_HEADED !== "1";
const PRESERVE_PROFILE = process.env.HOME_VIRTUAL_AUTH_PRESERVE_PROFILE === "1";
const CLEANUP_PASSKEY = process.env.HOME_VIRTUAL_AUTH_CLEANUP !== "0";
const INCLUDE_BROWSER = process.env.HOME_VIRTUAL_AUTH_BROWSER === "1";
const CHECK_BROWSER_SUMMARY =
  process.env.HOME_VIRTUAL_AUTH_BROWSER_SUMMARY === "1" ||
  process.env.HOME_VIRTUAL_AUTH_BROWSER_OPEN === "1";
const OPEN_BROWSER = process.env.HOME_VIRTUAL_AUTH_BROWSER_OPEN === "1";
const BROWSER_OPEN_CONCURRENT = parseBoundedIntegerEnv(
  "HOME_VIRTUAL_AUTH_BROWSER_OPEN_CONCURRENT",
  1,
  1,
  4,
);
const BROWSER_OPEN_HOLD_MS = parseBoundedIntegerEnv(
  "HOME_VIRTUAL_AUTH_BROWSER_OPEN_HOLD_MS",
  0,
  0,
  300_000,
);
const BROWSER_OPEN_URLS = parseBrowserOpenUrls(process.env.HOME_VIRTUAL_AUTH_BROWSER_OPEN_URLS);
const ALLOW_REMOTE = process.env.HOME_VIRTUAL_AUTH_ALLOW_REMOTE === "1";
const PROFILE_DIR = process.env.HOME_VIRTUAL_AUTH_PROFILE
  || mkdtempSync(join(tmpdir(), "elastos-home-passkey-smoke-"));

function parseBoundedIntegerEnv(name, defaultValue, min, max) {
  const raw = process.env[name];
  if (raw == null || raw === "") {
    return defaultValue;
  }
  const value = Number(raw);
  if (!Number.isInteger(value) || value < min || value > max) {
    throw new Error(`${name} must be an integer between ${min} and ${max}`);
  }
  return value;
}

function parseBrowserOpenUrls(raw) {
  const defaults = [
    "https://example.com/",
    "https://example.org/",
    "https://example.net/",
    "https://example.edu/",
  ];
  if (raw == null || raw.trim() === "") {
    return defaults;
  }
  const urls = raw
    .split(/[\n,]/)
    .map((entry) => entry.trim())
    .filter(Boolean);
  if (urls.length === 0 || urls.length > 4) {
    throw new Error("HOME_VIRTUAL_AUTH_BROWSER_OPEN_URLS must include 1 to 4 http(s) URLs");
  }
  for (const value of urls) {
    const url = new URL(value);
    if (!["http:", "https:"].includes(url.protocol)) {
      throw new Error(`HOME_VIRTUAL_AUTH_BROWSER_OPEN_URLS contains unsupported URL: ${value}`);
    }
  }
  return urls;
}

function assert(condition, message, details = null) {
  if (!condition) {
    const error = new Error(message);
    error.details = details;
    throw error;
  }
}

function isLoopbackUrl(value) {
  const url = new URL(value);
  return url.hostname === "127.0.0.1" || url.hostname === "localhost" || url.hostname === "::1";
}

function isLocalhostWebAuthnUrl(value) {
  return new URL(value).hostname === "localhost";
}

async function waitForHomeReady(page, timeoutMs = 30_000) {
  await page.waitForFunction(
    () => document.body?.dataset?.homeStatus === "ready",
    null,
    { timeout: timeoutMs },
  );
}

async function homeState(page) {
  return page.evaluate(() => ({
    status: document.body?.dataset?.homeStatus || "",
    authority: document.body?.dataset?.homeAuthority || "",
    unlockVisible: !document.querySelector("#home-unlock")?.hidden,
    unlockTitle: document.querySelector("#home-unlock-title")?.textContent?.trim() || "",
    unlockPrimary: document.querySelector("#home-unlock-primary")?.textContent?.trim() || "",
    unlockSecondary: document.querySelector("#home-unlock-secondary")?.textContent?.trim() || "",
    unlockSecondaryHidden: document.querySelector("#home-unlock-secondary")?.hidden ?? true,
    unlockNameVisible: !(document.querySelector("#home-unlock-name")?.hidden ?? true),
    unlockStatus: document.querySelector("#home-unlock-status")?.textContent?.trim() || "",
    systemShortcutPresent: !!document.querySelector('#desktop-shortcuts .desktop-shortcut[data-target="system"]'),
    browserShortcutPresent: !!document.querySelector('#desktop-shortcuts .desktop-shortcut[data-target="browser"]'),
  }));
}

async function waitForSignedHome(page, timeoutMs = 30_000) {
  await page.waitForFunction(
    () => document.body?.dataset?.homeStatus === "ready"
      && document.body?.dataset?.homeAuthority === "signed"
      && !!document.querySelector('#desktop-shortcuts .desktop-shortcut[data-target="system"]'),
    null,
    { timeout: timeoutMs },
  );
}

async function setupVirtualAuthenticator(context, page) {
  const cdp = await context.newCDPSession(page);
  await cdp.send("WebAuthn.enable");
  await cdp.send("WebAuthn.addVirtualAuthenticator", {
    options: {
      protocol: "ctap2",
      transport: "internal",
      hasResidentKey: true,
      hasUserVerification: true,
      isUserVerified: true,
      automaticPresenceSimulation: true,
    },
  });
  return cdp;
}

function captureNextPasskeyToken(page, timeoutMs = 30_000) {
  return page.waitForResponse((response) => {
    const url = response.url();
    return response.request().method() === "POST"
      && (url.endsWith("/api/auth/passkey/register/complete")
        || url.endsWith("/api/auth/passkey/authenticate/complete"));
  }, { timeout: timeoutMs }).then(async (response) => {
    const text = await response.text();
    let body = {};
    try {
      body = text ? JSON.parse(text) : {};
    } catch {
      body = { raw: text };
    }
    assert(response.ok(), "passkey completion response failed", {
      status: response.status(),
      body,
    });
    assert(body.home_token, "passkey completion did not return a Home token", body);
    return body.home_token;
  });
}

function delay(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

async function browserApi(page, token, path, { method = "GET", body = null } = {}) {
  return page.evaluate(async ({ token, path, method, body }) => {
    const headers = { "x-elastos-home-token": token };
    let requestBody;
    if (body != null) {
      headers["content-type"] = "application/json";
      requestBody = JSON.stringify(body);
    }
    const response = await fetch(path, {
      method,
      headers,
      body: requestBody,
    });
    const text = await response.text();
    let payload = {};
    try {
      payload = text ? JSON.parse(text) : {};
    } catch {
      payload = { raw: text };
    }
    return { ok: response.ok, status: response.status, body: payload };
  }, { token, path, method, body });
}

function settleTokenWithin(promise, timeoutMs) {
  return Promise.race([
    promise.catch(() => null),
    delay(timeoutMs).then(() => null),
  ]);
}

async function statusFromServer(page) {
  return page.evaluate(async () => {
    const response = await fetch("/api/auth/passkey/status");
    const text = await response.text();
    let body = {};
    try {
      body = text ? JSON.parse(text) : {};
    } catch {
      body = { raw: text };
    }
    return {
      ok: response.ok,
      status: response.status,
      body,
    };
  });
}

async function createPasskeyFromCurrentUnlock(page, mode) {
  const name = page.locator("#home-unlock-name");
  await name.waitFor({ state: "visible", timeout: 10_000 });
  await name.fill(TEST_NAME);
  const tokenPromise = captureNextPasskeyToken(page);
  await page.locator("#home-unlock-primary").click();
  await waitForSignedHome(page);
  return { mode, homeToken: await tokenPromise };
}

async function ensureSignedWithVirtualPasskey(page) {
  await waitForHomeReady(page);
  let state = await homeState(page);
  if (state.authority === "signed") {
    return { created: false, mode: "existing-session" };
  }

  const status = await statusFromServer(page);
  assert(status.ok, "passkey status endpoint failed", status);
  const registered = status.body.registered === true;
  const guestRegistrationEnabled = status.body.guest_registration_enabled === true;

  if (!registered) {
    const created = await createPasskeyFromCurrentUnlock(page, "admin");
    return { created: true, ...created };
  }

  if (!guestRegistrationEnabled) {
    const skip = new Error("SKIP virtual passkey smoke: existing Home has guest registration disabled");
    skip.skip = true;
    skip.details = { registered, guestRegistrationEnabled, state };
    throw skip;
  }

  const secondary = page.locator("#home-unlock-secondary");
  await secondary.waitFor({ state: "visible", timeout: 15_000 });
  await secondary.click();
  state = await homeState(page);
  assert(
    state.unlockTitle === "Create guest account" && state.unlockNameVisible,
    "Home did not enter guest passkey creation mode",
    state,
  );
  const created = await createPasskeyFromCurrentUnlock(page, "guest");
  return { created: true, ...created };
}

async function currentPasskey(page, homeToken) {
  assert(homeToken, "currentPasskey requires a passkey-issued Home token");
  return page.evaluate(async (token) => {
    const response = await fetch("/api/auth/passkeys", {
      headers: { "x-elastos-home-token": token },
    });
    const text = await response.text();
    let body = {};
    try {
      body = text ? JSON.parse(text) : {};
    } catch {
      body = { raw: text };
    }
    if (!response.ok) {
      throw new Error(`GET /api/auth/passkeys -> ${response.status} ${text}`);
    }
    return (body.passkeys || []).find((passkey) => passkey.current) || null;
  }, homeToken);
}

async function signOut(page) {
  await page.evaluate(async (token) => {
    const response = await fetch("/api/auth/sessions/sign-out", {
      method: "POST",
      credentials: "same-origin",
    });
    if (!response.ok && response.status !== 401 && response.status !== 403) {
      throw new Error(`POST /api/auth/sessions/sign-out -> ${response.status}`);
    }
  });
}

async function signBackIn(page) {
  const tokenPromise = captureNextPasskeyToken(page, 20_000).catch(() => null);
  await page.goto(HOME_URL, { waitUntil: "domcontentloaded" });
  await waitForHomeReady(page);
  let signed = false;
  try {
    await waitForSignedHome(page, 8_000);
    signed = true;
  } catch {
    signed = false;
  }
  if (signed) {
    const token = await settleTokenWithin(tokenPromise, 1_000);
    assert(token, "Home remained signed after sign-out without completing passkey authentication", await homeState(page));
    return token;
  }

  const state = await homeState(page);
  assert(state.unlockVisible, "Home did not show the unlock prompt after sign-out", state);
  const clickTokenPromise = captureNextPasskeyToken(page).catch(() => null);
  await page.locator("#home-unlock-primary").click();
  await waitForSignedHome(page);
  const token = await settleTokenWithin(clickTokenPromise, 1_000)
    || await settleTokenWithin(tokenPromise, 1_000);
  assert(token, "manual virtual passkey sign-in completed without a captured Home token", await homeState(page));
  return token;
}

async function launchSystem(page, homeToken) {
  assert(homeToken, "launchSystem requires a passkey-issued Home token");
  const route = await page.evaluate(async (token) => {
    const response = await fetch("/api/apps/home/launch", {
      method: "POST",
      headers: { "content-type": "application/json", "x-elastos-home-token": token },
      body: JSON.stringify({ target: "system" }),
    });
    const text = await response.text();
    let body = {};
    try {
      body = text ? JSON.parse(text) : {};
    } catch {
      body = { raw: text };
    }
    if (!response.ok) {
      throw new Error(`POST /api/apps/home/launch system -> ${response.status} ${text}`);
    }
    return body.route || "";
  }, homeToken);
  assert(route.includes("home_token="), "System launch did not mint an app-scoped token", { route });
  await page.goto(new URL(route, HOME_URL).toString(), { waitUntil: "domcontentloaded" });
  await page.locator(".system-shell").waitFor({ state: "visible", timeout: 20_000 });
  const system = await page.evaluate(() => ({
    title: document.title,
    panels: [...document.querySelectorAll(".system-panel h2")].map((node) => node.textContent?.trim() || ""),
    fields: [...document.querySelectorAll(".system-field dt")].map((node) => node.textContent?.trim() || ""),
    walletControlsRemoved: !document.querySelector("#wallet-create")
      && !document.querySelector("#wallet-approvals")
      && !document.querySelector("#wallet-accounts"),
    errorText: document.querySelector(".system-error:not([hidden])")?.textContent?.trim() || "",
  }));
  assert(system.title === "System · ElastOS", "System title mismatch after signed launch", system);
  assert(system.panels.includes("Account") && system.panels.includes("Advanced"), "System panels did not render", system);
  assert(system.fields.includes("Accounts") && system.fields.includes("Recovery"), "System signed account fields did not render", system);
  assert(!system.fields.includes("Wallet"), "System should not duplicate Wallet controls", system);
  assert(system.walletControlsRemoved, "System should not include wallet account or approval controls", system);
  assert(!system.errorText, "System rendered an access error after signed launch", system);
  return system;
}

async function checkBrowserLaunchGrant(page, homeToken) {
  assert(homeToken, "checkBrowserLaunchGrant requires a passkey-issued Home token");
  await page.goto(HOME_URL, { waitUntil: "domcontentloaded" });
  await waitForSignedHome(page);
  const launched = await page.evaluate(async (token) => {
    const response = await fetch("/api/apps/home/launch", {
      method: "POST",
      headers: { "content-type": "application/json", "x-elastos-home-token": token },
      body: JSON.stringify({ target: "browser" }),
    });
    const text = await response.text();
    let body = {};
    try {
      body = text ? JSON.parse(text) : {};
    } catch {
      body = { raw: text };
    }
    return {
      ok: response.ok,
      status: response.status,
      body,
    };
  }, homeToken);
  assert(launched.ok, "Browser launch grant failed", launched);
  assert(launched.body?.target === "browser", "Browser launch did not resolve the Browser capsule", launched);
  const route = String(launched.body?.route || "");
  assert(route.includes("home_token="), "Browser launch did not mint an app token", launched);
  if (OPEN_BROWSER) {
    const browserToken = new URL(route, HOME_URL).searchParams.get("home_token") || "";
    assert(browserToken, "Browser launch route did not contain a Browser app token", launched);
    assert(
      BROWSER_OPEN_CONCURRENT <= BROWSER_OPEN_URLS.length,
      "HOME_VIRTUAL_AUTH_BROWSER_OPEN_CONCURRENT exceeds HOME_VIRTUAL_AUTH_BROWSER_OPEN_URLS",
      { concurrent: BROWSER_OPEN_CONCURRENT, urls: BROWSER_OPEN_URLS },
    );
    let summaryBefore = null;
    let baselinePrincipalSessions = 0;
    if (CHECK_BROWSER_SUMMARY) {
      summaryBefore = await browserApi(page, browserToken, "/api/apps/browser/summary");
      assert(summaryBefore.ok, "Browser summary failed before open", summaryBefore);
      assert(
        summaryBefore.body?.sessions?.schema === "elastos.browser.session-capacity/v1",
        "Browser summary did not include the session-capacity receipt",
        summaryBefore,
      );
      baselinePrincipalSessions = Number(summaryBefore.body.sessions.principal_sessions || 0);
      launched.body.browser_summary = {
        sessions: summaryBefore.body.sessions,
        engine_adapter: summaryBefore.body.engine_adapter,
        net: summaryBefore.body.net,
      };
    }
    const urls = BROWSER_OPEN_URLS;
    const pages = [];
    const closeResults = [];
    try {
      const openedPages = await Promise.all(
        Array.from({ length: BROWSER_OPEN_CONCURRENT }, async (_, index) => {
          const opened = await browserApi(page, browserToken, "/api/apps/browser/open", {
            method: "POST",
            body: {
              url: urls[index],
              reason: `virtual passkey Browser open smoke ${index + 1}`,
              viewport: { width: 1280, height: 720 },
              display_mode: "webrtc_remote_display",
            },
          });
          assert(opened.ok, `Browser app token could not open Runtime Browser page ${index + 1}`, opened);
          const pageId = opened.body?.engine_page?.page_id || "";
          assert(opened.body?.schema === "elastos.browser.open-result/v1", "Browser open returned wrong schema", opened);
          assert(opened.body?.engine_page?.schema === "elastos.browser.engine.page/v1", "Browser open returned wrong engine page schema", opened);
          assert(opened.body.engine_page.direct_network === false, "Browser open reported direct network", opened.body.engine_page);
          assert(opened.body.engine_page.engine_control === "page_scoped", "Browser open did not return page-scoped control", opened.body.engine_page);
          assert(opened.body.engine_page.isolated_engine_session === true, "Browser open did not isolate the engine session", opened.body.engine_page);
          assert(opened.body.engine_page.display_session?.mode === "webrtc_remote_display", "Browser open did not return WebRTC display", opened.body.engine_page);
          return {
            page_id: pageId,
            url: urls[index],
            display_backend: opened.body.engine_page.display_session.display_backend,
            display_mode: opened.body.engine_page.display_session.mode,
            engine_control: opened.body.engine_page.engine_control,
            isolated_engine_session: opened.body.engine_page.isolated_engine_session,
            direct_network: opened.body.engine_page.direct_network,
            actual_url: opened.body.engine_page.actual_url,
          };
        }),
      );
      pages.push(...openedPages);
      const uniquePageIds = new Set(pages.map((entry) => entry.page_id));
      assert(uniquePageIds.size === pages.length, "Browser concurrent open returned duplicate page IDs", pages);

      const summaryAfterOpen = await browserApi(page, browserToken, "/api/apps/browser/summary");
      assert(summaryAfterOpen.ok, "Browser summary failed after open", summaryAfterOpen);
      assert(
        Number(summaryAfterOpen.body?.sessions?.principal_sessions || 0)
          >= baselinePrincipalSessions + pages.length,
        "Browser session-capacity receipt did not account for opened pages",
        { before: summaryBefore?.body?.sessions, after: summaryAfterOpen.body?.sessions, pages },
      );

      const heartbeat = async () => {
        await Promise.all(pages.map(async (entry) => {
          const response = await browserApi(
            page,
            browserToken,
            `/api/apps/browser/pages/${encodeURIComponent(entry.page_id)}/heartbeat`,
            { method: "POST" },
          );
          assert(response.ok, `Browser heartbeat failed for ${entry.page_id}`, response);
          assert(response.body?.schema === "elastos.browser.page-heartbeat/v1", "Browser heartbeat returned wrong schema", response);
        }));
      };
      await heartbeat();
      const holdStartedAt = Date.now();
      while (Date.now() - holdStartedAt < BROWSER_OPEN_HOLD_MS) {
        await delay(Math.min(5000, Math.max(250, BROWSER_OPEN_HOLD_MS - (Date.now() - holdStartedAt))));
        await heartbeat();
      }
    } finally {
      await Promise.all(pages.map(async (entry) => {
        const closed = await browserApi(
          page,
          browserToken,
          `/api/apps/browser/pages/${encodeURIComponent(entry.page_id)}/close`,
          { method: "POST", body: {} },
        );
        assert(closed.ok, `Browser open smoke could not close Runtime Browser page ${entry.page_id}`, closed);
        assert(
          closed.body?.schema === "elastos.browser.close-result/v1",
          `Browser close for ${entry.page_id} did not return the close-result receipt`,
          closed,
        );
        assert(
          closed.body?.closed === true,
          `Browser close for ${entry.page_id} did not report closed=true`,
          closed,
        );
        if (entry.isolated_engine_session) {
          assert(
            closed.body?.isolated_session === true,
            `Browser close for ${entry.page_id} did not report isolated_session=true`,
            closed,
          );
          assert(
            closed.body?.shutdown?.ok === true || closed.body?.cleanup?.ok === true,
            `Browser close for ${entry.page_id} did not shutdown or cleanup the isolated session`,
            closed,
          );
        }
        closeResults.push(closed.body);
      }));
    }
    const summaryAfterClose = await browserApi(page, browserToken, "/api/apps/browser/summary");
    assert(summaryAfterClose.ok, "Browser summary failed after close", summaryAfterClose);
    assert(
      Number(summaryAfterClose.body?.sessions?.principal_sessions || 0) <= baselinePrincipalSessions,
      "Browser session-capacity receipt still counted closed smoke pages",
      { before: summaryBefore?.body?.sessions, after: summaryAfterClose.body?.sessions, pages },
    );
    launched.body.browser_open = {
      concurrent_pages: pages.length,
      hold_ms: BROWSER_OPEN_HOLD_MS,
      baseline_principal_sessions: baselinePrincipalSessions,
      final_principal_sessions: Number(summaryAfterClose.body?.sessions?.principal_sessions || 0),
      pages,
      close_results: closeResults,
    };
  } else if (CHECK_BROWSER_SUMMARY) {
    const browserToken = new URL(route, HOME_URL).searchParams.get("home_token") || "";
    assert(browserToken, "Browser launch route did not contain a Browser app token", launched);
    const summary = await browserApi(page, browserToken, "/api/apps/browser/summary");
    assert(summary.ok, "Browser summary failed", summary);
    assert(
      summary.body?.sessions?.schema === "elastos.browser.session-capacity/v1",
      "Browser summary did not include the session-capacity receipt",
      summary,
    );
    launched.body.browser_summary = {
      sessions: summary.body.sessions,
      engine_adapter: summary.body.engine_adapter,
      net: summary.body.net,
    };
  }
  return launched.body;
}

async function revokeCurrentPasskey(page, proofBindingId, homeToken) {
  if (!proofBindingId) {
    return { skipped: true, reason: "missing proof binding" };
  }
  assert(homeToken, "revokeCurrentPasskey requires a passkey-issued Home token");
  return page.evaluate(async ({ id, token }) => {
    const response = await fetch(`/api/auth/passkeys/${encodeURIComponent(id)}/revoke`, {
      method: "POST",
      headers: { "x-elastos-home-token": token },
    });
    const text = await response.text();
    let body = {};
    try {
      body = text ? JSON.parse(text) : {};
    } catch {
      body = { raw: text };
    }
    return {
      ok: response.ok,
      status: response.status,
      body,
    };
  }, { id: proofBindingId, token: homeToken });
}

async function main() {
  if (!ALLOW_REMOTE) {
    assert(
      isLoopbackUrl(HOME_URL),
      "Refusing to create a virtual passkey on a non-loopback Home URL without HOME_VIRTUAL_AUTH_ALLOW_REMOTE=1",
      { HOME_URL },
    );
    assert(
      isLocalhostWebAuthnUrl(HOME_URL),
      "WebAuthn virtual passkey smoke must use http://localhost, not a loopback IP, because browsers reject IP addresses as relying-party IDs",
      { HOME_URL },
    );
  }

  const context = await chromium.launchPersistentContext(PROFILE_DIR, {
    headless: HEADLESS,
    ignoreHTTPSErrors: true,
    viewport: { width: 1280, height: 900 },
  });
  let page = context.pages()[0] || await context.newPage();
  let created = null;
  let passkey = null;
  let cleanupResult = null;
  let homeToken = "";
  let cleanupAttempted = false;
  async function cleanupCreatedPasskey() {
    if (
      cleanupAttempted
      || !created?.created
      || !CLEANUP_PASSKEY
      || !passkey?.proof_binding_id
      || !homeToken
    ) {
      return cleanupResult || { skipped: !created?.created || !CLEANUP_PASSKEY };
    }
    cleanupAttempted = true;
    cleanupResult = await revokeCurrentPasskey(page, passkey.proof_binding_id, homeToken);
    return cleanupResult;
  }
  try {
    await setupVirtualAuthenticator(context, page);
    await page.goto(HOME_URL, { waitUntil: "domcontentloaded" });
    created = await ensureSignedWithVirtualPasskey(page);
    homeToken = created.homeToken;
    passkey = await currentPasskey(page, homeToken);
    assert(passkey?.proof_binding_id, "signed virtual passkey was not visible through the passkey list", passkey);

    await signOut(page, homeToken);
    homeToken = await signBackIn(page);
    const afterSignIn = await currentPasskey(page, homeToken);
    assert(
      afterSignIn?.proof_binding_id === passkey.proof_binding_id,
      "virtual passkey sign-in did not restore the same proof binding",
      { before: passkey, after: afterSignIn },
    );

    const system = await launchSystem(page, homeToken);
    const browserLaunch = INCLUDE_BROWSER ? await checkBrowserLaunchGrant(page, homeToken) : null;

    if (created.created && CLEANUP_PASSKEY) {
      cleanupResult = await cleanupCreatedPasskey();
      assert(cleanupResult.ok, "virtual test passkey cleanup failed", cleanupResult);
    }

    console.log(JSON.stringify({
      schema: "elastos.home.passkey-virtual-auth-smoke/v1",
      ok: true,
      home_url: HOME_URL,
      profile_dir: PROFILE_DIR,
      created_mode: created.mode,
      proof_binding_id: passkey.proof_binding_id,
      principal_id: passkey.principal_id,
      role: passkey.role,
      system_fields: system.fields,
      browser_launch_checked: Boolean(browserLaunch),
      browser_open_checked: Boolean(browserLaunch?.browser_open),
      browser_open: browserLaunch?.browser_open || null,
      cleanup: cleanupResult || { skipped: !created.created || !CLEANUP_PASSKEY },
    }, null, 2));
  } catch (error) {
    if (error.skip) {
      console.log(error.message);
      if (error.details) {
        console.log(JSON.stringify(error.details, null, 2));
      }
      return;
    }
    try {
      const cleanup = await cleanupCreatedPasskey();
      if (cleanup && cleanup.ok === false) {
        console.error("virtual test passkey cleanup failed after smoke error");
        console.error(JSON.stringify(cleanup, null, 2));
      }
    } catch (cleanupError) {
      console.error("virtual test passkey cleanup threw after smoke error");
      console.error(cleanupError.message || cleanupError);
    }
    console.error("FAIL home-passkey-virtual-auth-smoke");
    console.error(error.message || error);
    if (error.details) {
      console.error(JSON.stringify(error.details, null, 2));
    } else {
      const state = page ? await homeState(page).catch(() => null) : null;
      if (state) {
        console.error(JSON.stringify(state, null, 2));
      }
    }
    process.exitCode = 1;
  } finally {
    await context.close().catch(() => {});
    if (!PRESERVE_PROFILE && !process.env.HOME_VIRTUAL_AUTH_PROFILE) {
      rmSync(PROFILE_DIR, { recursive: true, force: true });
    }
  }
}

await main();
