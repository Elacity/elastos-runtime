#!/usr/bin/env node

import { createServer } from "node:http";
import { createRequire } from "node:module";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { extname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { assertPhoneDrawer, DRAWER_SETTLE_MS, PHONE_VIEWPORT, setPhoneFormFactor } from "./lib/phone-drawer-assert.mjs";

const repoRoot = resolve(fileURLToPath(new URL("../", import.meta.url)));
const browserRoot = join(repoRoot, "capsules/people/browser");
const brave = process.env.BRAVE_BIN || "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser";
const require = createRequire(new URL("../elastos/tools/browser-playwright-engine/package.json", import.meta.url));
const { chromium } = require("playwright");
const PHONE_SCREENSHOT = "/tmp/people-phone-390x844.png";
const PHONE_DRAWER_SCREENSHOT = "/tmp/people-phone-drawer-390x844.png";

function assert(condition, message, details = undefined) {
  if (!condition) {
    throw new Error(`${message}${details ? `\n${JSON.stringify(details, null, 2)}` : ""}`);
  }
}

function json(response, value, status = 200, headers = {}) {
  const body = Buffer.from(JSON.stringify(value));
  response.writeHead(status, {
    "access-control-allow-origin": "null",
    "cache-control": "no-store",
    "content-length": body.length,
    "content-type": "application/json",
    ...headers,
  });
  response.end(body);
}

async function readJson(request) {
  const chunks = [];
  for await (const chunk of request) {
    chunks.push(Buffer.from(chunk));
  }
  const text = Buffer.concat(chunks).toString("utf8");
  return text ? JSON.parse(text) : {};
}

function discoverySummary({
  configured = false,
  enabled = false,
  status = configured ? (enabled ? "visible" : "off") : "unconfigured",
  statusMessage = configured
    ? (enabled
      ? "Discovery is on. Visibility lasts up to ten minutes, and both people must be visible at the same time."
      : "Discovery is off.")
    : "Discovery is unavailable.",
  discoveredPeers = [],
  pendingRequestCount = 0,
  remainingSeconds = null,
  remoteVisibilityMayRemainUntil = null,
  remoteVisibilityRemainingSeconds = null,
} = {}) {
  return {
    schema: "elastos.people.discovery/v1",
    configured,
    enabled,
    status,
    status_message: statusMessage,
    expires_at: remainingSeconds === null ? null : 1_000 + remainingSeconds,
    remaining_seconds: remainingSeconds,
    remote_visibility_may_remain_until: remoteVisibilityMayRemainUntil,
    remote_visibility_remaining_seconds: remoteVisibilityRemainingSeconds,
    discovered_count: discoveredPeers.length,
    discovered_peers: discoveredPeers,
    request_count: pendingRequestCount,
  };
}

function summary({
  readinessStatus,
  profileName = "",
  setupSuggestion = "",
  contacts = [],
  discovery,
}) {
  return {
    identity: {
      ...(profileName ? { profile: { display_name: profileName } } : {}),
      ...(setupSuggestion ? { profile_setup_display_name: setupSuggestion } : {}),
      profile_readiness: {
        schema: "elastos.profile.readiness/v1",
        status: readinessStatus,
      },
    },
    people: {
      contacts,
    },
    discovery,
  };
}

function acceptedContact() {
  return {
    contact_id: "contact:accepted",
    display_name: "Ari Contact",
    handle: "ari",
    relationship: "connected",
    can_message: true,
    conversation_id: "direct:opaque:ari",
    route: "must-not-render-route",
    remote_presence_device_did: "did:key:zDeviceHidden",
    provider: "must-not-render-provider",
    endpoint: "must-not-render-endpoint",
    peer_id: "must-not-render-peer-id",
  };
}

function visibleDiscoveryPeer() {
  return {
    advertisement_id: "ad-visible-1",
    display_name: "Jordan Visible",
    handle: "jordan",
    peer_id: "must-not-render-peer-id",
    connect_ticket: "must-not-render-ticket",
    route: "must-not-render-route",
    provider: "must-not-render-provider",
    endpoint: "must-not-render-endpoint",
    device_label: "must-not-render-device",
  };
}

function freshScenarioState(scenario) {
  return {
    scenario,
    discoveryEnabled: false,
    discoveryRequestPosts: 0,
    discoveryRefreshPosts: 0,
    discoveryTogglePosts: 0,
    profileAttemptCount: 0,
    createdProfileName: "",
    removalAttempts: 0,
    contactRemoved: false,
  };
}

function scenarioSummary(state) {
  if (state.scenario === "first-run") {
    return summary({
      readinessStatus: "setup_required",
      setupSuggestion: "Suggested Profile Name",
      contacts: [{ contact_id: "", display_name: "Filtered malformed contact" }],
      discovery: discoverySummary(),
    });
  }
  if (state.scenario === "ready-off") {
    return summary({
      readinessStatus: "ready",
      profileName: "Ready Profile",
      setupSuggestion: "Do not use this suggestion",
      contacts: [
        acceptedContact(),
        { contact_id: "", display_name: "Filtered malformed contact" },
      ],
      discovery: state.discoveryEnabled
        ? discoverySummary({
          configured: true,
          enabled: true,
          status: "visible",
          discoveredPeers: [],
          pendingRequestCount: 0,
          remainingSeconds: 600,
        })
        : discoverySummary({
          configured: true,
          enabled: false,
          status: "off",
          statusMessage: "Discovery is off.",
        }),
    });
  }
  if (state.scenario === "remove-confirmation") {
    return summary({
      readinessStatus: "ready",
      profileName: "Ready Profile",
      contacts: [
        { ...acceptedContact(), relationship: state.contactRemoved ? "removed" : "connected", can_message: !state.contactRemoved },
        { ...acceptedContact(), contact_id: "contact:other", display_name: "Other Contact", conversation_id: "direct:opaque:other" },
      ],
      discovery: discoverySummary({ configured: true }),
    });
  }
  if (state.scenario === "visible") {
    return summary({
      readinessStatus: "ready",
      profileName: "Visible Profile",
      contacts: [acceptedContact()],
      discovery: discoverySummary({
        configured: true,
        enabled: true,
        status: "visible",
        statusMessage:
          "Discovery is on. Visibility lasts up to ten minutes. This window has 540 seconds left, and both people must be visible at the same time.",
        discoveredPeers: [visibleDiscoveryPeer()],
        pendingRequestCount: 1,
        remainingSeconds: 540,
      }),
    });
  }
  if (state.scenario === "profile-failure") {
    return state.createdProfileName
      ? summary({
        readinessStatus: "ready",
        profileName: state.createdProfileName,
        discovery: discoverySummary({
          configured: true,
          enabled: false,
          status: "off",
          statusMessage: "Discovery is off.",
        }),
      })
      : summary({
        readinessStatus: "setup_required",
        setupSuggestion: "Suggested Retry Name",
        discovery: discoverySummary(),
      });
  }
  throw new Error(`unknown People smoke scenario: ${state.scenario}`);
}

async function serveFile(response, pathname) {
  const relative = pathname === "/apps/people/" ? "index.html" : pathname.slice("/apps/people/".length);
  const path = join(browserRoot, relative);
  assert(path.startsWith(`${browserRoot}/`) || path === join(browserRoot, "index.html"), "invalid People asset path");
  const body = await readFile(path);
  const contentType = {
    ".css": "text/css",
    ".html": "text/html; charset=utf-8",
    ".js": "text/javascript",
    ".wasm": "application/wasm",
  }[extname(path)] || "application/octet-stream";
  response.writeHead(200, {
    "access-control-allow-origin": "null",
    "cache-control": "no-store",
    "content-length": body.length,
    "content-type": contentType,
  });
  response.end(body);
}

function startServer() {
  const states = new Map();
  const scenarioByToken = new Map();
  const trace = {
    discoveryRefreshes: 0,
    discoveryRequests: 0,
    discoveryToggles: 0,
    profileAttempts: 0,
    removalTargets: [],
  };
  const server = createServer(async (request, response) => {
    try {
      const url = new URL(request.url, "http://localhost");
      if (request.method === "OPTIONS") {
        response.writeHead(204, {
          "access-control-allow-headers": "content-type,x-elastos-home-token",
          "access-control-allow-methods": "GET,POST,OPTIONS",
          "access-control-allow-origin": "null",
        }).end();
        return;
      }
      if (url.pathname === "/fixture") {
        const scenario = url.searchParams.get("scenario") || "first-run";
        const homeToken = `people-test-${scenario}`;
        states.set(scenario, freshScenarioState(scenario));
        scenarioByToken.set(homeToken, scenario);
        const origin = `http://127.0.0.1:${server.address().port}`;
        const iframeSrc =
          `/apps/people/?home_origin=${encodeURIComponent(origin)}` +
          `#home_token=${encodeURIComponent(homeToken)}`;
        const body = Buffer.from(
          "<!doctype html><html><body style=\"margin:0\">"
          + "<script>window.peopleSmokeMessages=[];window.addEventListener('message',(event)=>window.peopleSmokeMessages.push(event.data));</script>"
          + `<iframe title="People" sandbox="allow-forms allow-pointer-lock allow-scripts" style="border:0;height:100vh;width:100vw" src="${iframeSrc}"></iframe>`
          + "</body></html>",
        );
        response.writeHead(200, {
          "content-length": body.length,
          "content-type": "text/html; charset=utf-8",
        });
        response.end(body);
        return;
      }

      if (url.pathname.startsWith("/apps/people/")) {
        await serveFile(response, url.pathname);
        return;
      }

      const homeToken = String(request.headers["x-elastos-home-token"] || "");
      const scenario = scenarioByToken.get(homeToken);
      assert(scenario, "People smoke received an unknown or missing home token", {
        homeToken,
        knownTokens: [...scenarioByToken.keys()],
        path: url.pathname,
      });
      const state = states.get(scenario) || freshScenarioState(scenario);
      states.set(scenario, state);

      if (url.pathname === "/api/apps/people/summary") {
        return json(response, scenarioSummary(state));
      }
      if (url.pathname === "/api/apps/people/discovery" && request.method === "POST") {
        trace.discoveryToggles += 1;
        state.discoveryTogglePosts += 1;
        const body = await readJson(request);
        state.discoveryEnabled = body.enabled === true;
        return json(response, scenarioSummary(state).discovery);
      }
      if (url.pathname === "/api/apps/people/discovery/refresh" && request.method === "POST") {
        trace.discoveryRefreshes += 1;
        state.discoveryRefreshPosts += 1;
        return json(response, scenarioSummary(state).discovery);
      }
      if (url.pathname === "/api/apps/people/discovery/requests" && request.method === "POST") {
        trace.discoveryRequests += 1;
        state.discoveryRequestPosts += 1;
        const body = await readJson(request);
        assert(body.advertisement_id === "ad-visible-1", "People request used the wrong advertisement selector", body);
        return json(response, scenarioSummary(state).discovery);
      }
      if (url.pathname === "/api/apps/people/contacts/remove" && request.method === "POST") {
        const body = await readJson(request);
        assert(scenario === "remove-confirmation" && body.contact_id === "contact:accepted",
          "People removal changed its confirmed target", body);
        trace.removalTargets.push(body.contact_id);
        state.removalAttempts += 1;
        if (state.removalAttempts === 1) {
          return json(response, { error: "unauthorized" }, 401);
        }
        state.contactRemoved = true;
        return json(response, { status: "ok" });
      }
      if (url.pathname === "/api/apps/people/profile" && request.method === "POST") {
        trace.profileAttempts += 1;
        state.profileAttemptCount += 1;
        const body = await readJson(request);
        assert(typeof body.display_name === "string", "People profile POST did not send display_name", body);
        if (scenario === "profile-failure" && state.profileAttemptCount === 1) {
          return json(
            response,
            { message: "provider route failed for did:key:zHidden endpoint peer-id via carrier provider" },
            500,
          );
        }
        state.createdProfileName = body.display_name.trim();
        return json(response, { status: "ok" });
      }
      response.writeHead(404).end();
    } catch (error) {
      response.writeHead(500, { "content-type": "text/plain" }).end(String(error.stack || error));
    }
  });
  return new Promise((resolveServer, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", () => resolveServer({ server, trace }));
  });
}

async function waitFor(check, timeoutMs = 15_000) {
  const deadline = Date.now() + timeoutMs;
  let lastError;
  while (Date.now() < deadline) {
    try {
      const value = await check();
      if (value) {
        return value;
      }
    } catch (error) {
      lastError = error;
    }
    await new Promise((resolveDelay) => setTimeout(resolveDelay, 100));
  }
  throw lastError || new Error("timed out waiting for People UI");
}

async function peopleFrame(page) {
  const handle = await page.waitForSelector('iframe[title="People"]');
  const frame = await handle.contentFrame();
  assert(frame, "opaque People frame is missing");
  await frame.waitForLoadState("domcontentloaded");
  await frame.waitForFunction(() => !document.getElementById("people-shell")?.classList.contains("hidden"));
  return frame;
}

async function openPeople(page, port, scenario, width) {
  await page.setViewportSize({ width, height: 900 });
  await page.goto(`http://127.0.0.1:${port}/fixture?scenario=${encodeURIComponent(scenario)}`, {
    waitUntil: "domcontentloaded",
  });
  return peopleFrame(page);
}

async function openDiscovery(frame) {
  await frame.locator('[data-section-target="discovery"]').click();
  await frame.waitForFunction(() => document.querySelector("#discovery")?.hidden === false);
}

async function assertNoOverflow(frame, label) {
  const state = await frame.evaluate(() => {
    const visible = (node) => {
      if (!node || node.hidden) {
        return false;
      }
      const style = getComputedStyle(node);
      return style.display !== "none" && style.visibility !== "hidden";
    };
    const selectors = [
      ".people-shell",
      ".people-sidebar",
      ".people-main",
      ".people-content",
      ".people-content-inner",
      ".people-section:not([hidden])",
      ".profile-card",
      ".profile-form",
      ".people-list",
      ".discovery-header",
      ".discovery-grid",
    ];
    const panels = [];
    for (const selector of selectors) {
      for (const node of document.querySelectorAll(selector)) {
        if (!visible(node)) {
          continue;
        }
        const overflow = Math.max(0, node.scrollWidth - node.clientWidth);
        if (overflow > 1) {
          panels.push({
            selector,
            overflow,
            scrollWidth: node.scrollWidth,
            clientWidth: node.clientWidth,
          });
        }
      }
    }
    return {
      width: innerWidth,
      documentOverflow: Math.max(0, document.documentElement.scrollWidth - document.documentElement.clientWidth),
      bodyOverflow: Math.max(0, document.body.scrollWidth - document.documentElement.clientWidth),
      panels,
    };
  });
  assert(
    state.documentOverflow <= 1 && state.bodyOverflow <= 1 && state.panels.length === 0,
    `${label} has horizontal overflow`,
    state,
  );
}

async function assertVisible(frame, selectors, label) {
  const results = await frame.evaluate((requestedSelectors) => {
    return requestedSelectors.map((selector) => {
      const node = document.querySelector(selector);
      if (!node) {
        return { selector, found: false };
      }
      node.scrollIntoView({ block: "nearest", inline: "nearest" });
      const style = getComputedStyle(node);
      const rect = node.getBoundingClientRect();
      const pointX = Math.max(0, Math.min(innerWidth - 1, rect.left + Math.min(rect.width / 2, 12)));
      const pointY = Math.max(0, Math.min(innerHeight - 1, rect.top + Math.min(rect.height / 2, 12)));
      const topElement = document.elementFromPoint(pointX, pointY);
      return {
        selector,
        found: true,
        display: style.display,
        visibility: style.visibility,
        opacity: Number(style.opacity || "1"),
        width: Math.round(rect.width),
        height: Math.round(rect.height),
        left: Math.round(rect.left),
        right: Math.round(rect.right),
        top: Math.round(rect.top),
        bottom: Math.round(rect.bottom),
        topMatch: Boolean(topElement && (node === topElement || node.contains(topElement))),
        viewportWidth: innerWidth,
        viewportHeight: innerHeight,
      };
    });
  }, selectors);
  for (const result of results) {
    assert(result.found, `${label} is missing ${result.selector}`);
    assert(
      result.display !== "none"
        && result.visibility !== "hidden"
        && result.opacity > 0
        && result.width > 0
        && result.height > 0
        && result.left >= 0
        && result.right <= result.viewportWidth + 1
        && result.top >= 0
        && result.bottom <= result.viewportHeight + 1
        && result.topMatch,
      `${label} is not fully visible`,
      result,
    );
  }
}

async function assertNoIdentityLeak(frame, label) {
  const pageState = await frame.evaluate(() => ({
    text: document.body.innerText,
    html: document.documentElement.outerHTML,
  }));
  const textChecks = [
    /did:key:/i,
    /\bendpoint\b/i,
    /\broute\b/i,
    /\bprovider\b/i,
    /\bpeer id\b/i,
    /\bticket\b/i,
    /\bUnverified device\b/i,
    /\bUnverified member\b/i,
    /\bElastOS user\b/i,
    /\bElastOS Home\b/i,
    /\bPerson\b/,
  ];
  for (const pattern of textChecks) {
    assert(!pattern.test(pageState.text), `${label} leaked raw or placeholder identity text`, {
      pattern: String(pattern),
      text: pageState.text,
    });
  }
  for (const marker of [
    "must-not-render-route",
    "must-not-render-provider",
    "must-not-render-endpoint",
    "must-not-render-peer-id",
    "must-not-render-ticket",
    "must-not-render-device",
  ]) {
    assert(!pageState.html.includes(marker), `${label} leaked internal fixture identity data`, {
      marker,
      html: pageState.html,
    });
  }
}

async function waitForStatus(frame, expectedText) {
  await frame.waitForFunction(
    (expected) => document.querySelector("#people-status")?.textContent?.trim() === expected,
    expectedText,
  );
}

async function assertFirstRunScenario(frame, width) {
  await waitFor(() => frame.evaluate(() => document.querySelector("#profile-title")?.textContent === "Create your Profile"));
  await assertNoOverflow(frame, `first run at ${width}px`);
  await assertVisible(frame, [
    "#people-count",
    "#profile-title",
    "#profile-description",
    "#profile-name",
    "#profile-submit",
  ], `first run at ${width}px`);
  const state = await frame.evaluate(() => ({
    count: document.querySelector("#people-count")?.textContent?.trim() || "",
    title: document.querySelector("#profile-title")?.textContent?.trim() || "",
    description: document.querySelector("#profile-description")?.textContent?.trim() || "",
    value: document.querySelector("#profile-name")?.value || "",
    placeholder: document.querySelector("#profile-name")?.getAttribute("placeholder") || "",
    submit: document.querySelector("#profile-submit")?.textContent?.trim() || "",
  }));
  assert(state.count === "0 contacts", "first run contact count is wrong", state);
  assert(state.title === "Create your Profile", "first run title is wrong", state);
  assert(
    state.description === "Confirm the name people will see in People and Chat. Discovery stays off until you enable it.",
    "first run explanation is wrong",
    state,
  );
  assert(state.value === "Suggested Profile Name", "first run should prefill the suggested profile name", state);
  assert(state.placeholder === "Your name", "first run should keep a stable editable placeholder", state);
  assert(state.submit === "Create Profile", "first run should offer explicit Profile creation", state);
  await assertNoIdentityLeak(frame, `first run at ${width}px`);
}

async function assertReadyOffScenario(frame, page, width) {
  await waitFor(() => frame.evaluate(() => document.querySelector("#profile-title")?.textContent === "My Profile"));
  await assertNoOverflow(frame, `ready/off at ${width}px`);
  await assertVisible(frame, [
    "#profile-title",
    "#profile-description",
    "#profile-name",
    "#profile-submit",
  ], `ready/off profile at ${width}px`);
  await openDiscovery(frame);
  await assertNoOverflow(frame, `ready/off discovery at ${width}px`);
  await assertVisible(frame, [
    "#discovery-status",
    "#discovery-toggle",
    "#discovery-refresh",
    "#discovery-visible-count",
    "#discovery-requests-count",
  ], `ready/off discovery controls at ${width}px`);
  const offState = await frame.evaluate(() => ({
    profileName: document.querySelector("#profile-name")?.value || "",
    status: document.querySelector("#discovery-status")?.textContent?.trim() || "",
    toggle: document.querySelector("#discovery-toggle")?.textContent?.trim() || "",
    refreshDisabled: document.querySelector("#discovery-refresh")?.disabled === true,
    visibleCount: document.querySelector("#discovery-visible-count")?.textContent?.trim() || "",
    requestCount: document.querySelector("#discovery-requests-count")?.textContent?.trim() || "",
  }));
  assert(offState.profileName === "Ready Profile", "ready/off should show the saved profile name", offState);
  assert(offState.status === "Discovery is off.", "ready/off should show the off discovery state", offState);
  assert(offState.toggle === "Turn On", "ready/off should offer discovery opt-in", offState);
  assert(offState.refreshDisabled === false, "ready/off should keep Refresh usable", offState);
  assert(offState.visibleCount === "0 visible", "ready/off visible count is wrong", offState);
  assert(offState.requestCount === "0 requests", "ready/off request count is wrong", offState);

  await frame.locator("#discovery-refresh").click();
  await waitForStatus(frame, "Refresh requested.");
  await frame.locator("#discovery-toggle").click();
  await waitFor(() => frame.evaluate(() => document.querySelector("#discovery-toggle")?.textContent?.trim() === "Turn Off"));
  const toggled = await frame.evaluate(() => ({
    status: document.querySelector("#discovery-status")?.textContent?.trim() || "",
    toggle: document.querySelector("#discovery-toggle")?.textContent?.trim() || "",
  }));
  assert(
    /Visibility lasts up to ten minutes/i.test(toggled.status) && toggled.toggle === "Turn Off",
    "ready/off controls did not stay usable after opt-in",
    toggled,
  );

  const messages = await page.evaluate(() => window.peopleSmokeMessages || []);
  assert(!messages.some((message) => message?.target === "system"), "ready/off should not leak to System");
  await assertNoIdentityLeak(frame, `ready/off at ${width}px`);
}

async function assertVisibleDiscoveryScenario(frame, page, width) {
  await openDiscovery(frame);
  await waitFor(() => frame.evaluate(() => document.querySelectorAll('#discovery-list [data-action="discovery-request"]').length === 1));
  await assertNoOverflow(frame, `visible discovery at ${width}px`);
  await assertVisible(frame, [
    "#discovery-status",
    "#discovery-toggle",
    "#discovery-refresh",
    "#discovery-visible-count",
    "#discovery-requests-count",
    '#discovery-list [data-action="discovery-request"]',
    '#discovery-requests-list [data-action="open-inbox"]',
  ], `visible discovery controls at ${width}px`);

  const discoveryState = await frame.evaluate(() => ({
    status: document.querySelector("#discovery-status")?.textContent?.trim() || "",
    addContact: document.querySelector('#discovery-list [data-action="discovery-request"]')?.textContent?.trim() || "",
    inbox: document.querySelector('#discovery-requests-list [data-action="open-inbox"]')?.textContent?.trim() || "",
    visibleCount: document.querySelector("#discovery-visible-count")?.textContent?.trim() || "",
    requestCount: document.querySelector("#discovery-requests-count")?.textContent?.trim() || "",
  }));
  assert(
    /540 seconds/.test(discoveryState.status),
    "visible discovery should show the bounded window",
    discoveryState,
  );
  assert(discoveryState.addContact === "Add contact", "visible discovery action is wrong", discoveryState);
  assert(discoveryState.inbox === "Open Inbox", "visible inbox action is wrong", discoveryState);
  assert(discoveryState.visibleCount === "1 visible", "visible discovery count is wrong", discoveryState);
  assert(discoveryState.requestCount === "1 request", "visible request count is wrong", discoveryState);

  await frame.locator('#discovery-list [data-action="discovery-request"]').click();
  await waitForStatus(frame, "Contact request queued.");
  await frame.locator('#discovery-requests-list [data-action="open-inbox"]').click();
  const messages = await page.evaluate(() => window.peopleSmokeMessages || []);
  const inboxMessage = messages.findLast?.((message) => message?.type === "home:open-target")
    || [...messages].reverse().find((message) => message?.type === "home:open-target");
  assert(
    inboxMessage?.target === "inbox",
    "visible discovery did not open Inbox through the Home target bridge",
    { messages },
  );
  await assertNoIdentityLeak(frame, `visible discovery at ${width}px`);
}

async function assertProfileFailureScenario(frame, width) {
  await waitFor(() => frame.evaluate(() => document.querySelector("#profile-title")?.textContent === "Create your Profile"));
  await assertNoOverflow(frame, `profile failure at ${width}px`);
  await frame.locator("#profile-name").fill("Retry Name");
  await frame.locator("#profile-submit").click();
  await waitForStatus(frame, "Could not create your Profile. Try again.");
  const failed = await frame.evaluate(() => ({
    title: document.querySelector("#profile-title")?.textContent?.trim() || "",
    status: document.querySelector("#people-status")?.textContent?.trim() || "",
    inputValue: document.querySelector("#profile-name")?.value || "",
    submitText: document.querySelector("#profile-submit")?.textContent?.trim() || "",
    submitDisabled: document.querySelector("#profile-submit")?.disabled === true,
  }));
  assert(
    failed.title === "Create your Profile"
      && failed.status === "Could not create your Profile. Try again."
      && failed.inputValue === "Retry Name"
      && failed.submitText === "Create Profile"
      && failed.submitDisabled === false,
    "profile creation failure did not stay visible and retryable",
    failed,
  );
  await assertNoIdentityLeak(frame, `profile failure after error at ${width}px`);

  await frame.locator("#profile-submit").click();
  await waitFor(() => frame.evaluate(() => document.querySelector("#profile-title")?.textContent === "My Profile"));
  await waitForStatus(frame, "Profile created.");
  const recovered = await frame.evaluate(() => ({
    title: document.querySelector("#profile-title")?.textContent?.trim() || "",
    inputValue: document.querySelector("#profile-name")?.value || "",
    submitText: document.querySelector("#profile-submit")?.textContent?.trim() || "",
  }));
  assert(
    recovered.title === "My Profile"
      && recovered.inputValue === "Retry Name"
      && recovered.submitText === "Save",
    "profile retry did not recover to the ready state",
    recovered,
  );
  await assertNoOverflow(frame, `profile failure recovered at ${width}px`);
}

// Signed in, the page is short: the sections must not take the spare height.
async function assertPhoneScenario(page, port) {
  const frame = await openPeople(page, port, "ready-off", PHONE_VIEWPORT.width);
  await page.setViewportSize(PHONE_VIEWPORT);
  await setPhoneFormFactor(frame);
  await page.waitForTimeout(DRAWER_SETTLE_MS);
  await waitFor(() => frame.evaluate(() => document.querySelector("#profile-title")?.textContent === "My Profile"));
  const head = await frame.evaluate(() => {
    const rect = (selector) => document.querySelector(selector).getBoundingClientRect();
    const title = rect(".people-page-title");
    const toggle = rect(".el-drawer-toggle");
    return {
      titleTop: Math.round(title.top),
      titleMiddle: Math.round(title.top + title.height / 2),
      toggleMiddle: Math.round(toggle.top + toggle.height / 2),
      toggleLeft: Math.round(toggle.left),
    };
  });
  assert(
    head.titleTop <= 40 && Math.abs(head.titleMiddle - head.toggleMiddle) <= 4 && head.toggleLeft <= 16,
    "People phone: content must start at the top with the drawer toggle leading the page title",
    head,
  );
  await page.screenshot({ path: PHONE_SCREENSHOT });
  await assertPhoneDrawer(page, frame, {
    label: "People",
    drawer: "#people-sidebar",
    room: ".people-main",
    closeTarget: '[data-section-target="people"]',
    screenshot: PHONE_DRAWER_SCREENSHOT,
  });
}

async function assertRemoveConfirmationScenario(frame, page, width, trace) {
  const beforePosts = trace.removalTargets.length;
  const remove = frame.locator('[data-action="remove"][data-contact-id="contact:accepted"]');
  await remove.waitFor({ state: "visible" });
  await remove.click();
  const confirm = frame.locator('[data-action="confirm-remove"][data-contact-id="contact:accepted"]');
  const cancel = frame.locator('[data-action="cancel-remove"][data-contact-id="contact:accepted"]');
  await confirm.waitFor({ state: "visible" });
  assert(await cancel.evaluate(node => node === document.activeElement), "Remove must focus the safe Cancel action");
  assert((await frame.locator('.person-confirm').innerText()).includes("Remove Ari Contact from People?"),
    "Remove confirmation named the wrong contact");
  await assertVisible(frame, ['.person-confirm', '[data-action="cancel-remove"]', '[data-action="confirm-remove"]'],
    `Remove confirmation at ${width}px`);
  await assertNoOverflow(frame, `Remove confirmation at ${width}px`);
  assert(trace.removalTargets.length === beforePosts, "Opening confirmation sent a removal POST");
  await cancel.click();
  await confirm.waitFor({ state: "hidden" });
  assert(trace.removalTargets.length === beforePosts, "Cancel sent a removal POST");
  await remove.click();
  const failed = page.waitForResponse(response => response.request().method() === "POST"
    && new URL(response.url()).pathname === "/api/apps/people/contacts/remove", { timeout: 10000 });
  await confirm.click();
  assert((await failed).status() === 401, "Removal refusal was not exercised");
  await waitForStatus(frame, "Could not complete that action.");
  await waitFor(() => confirm.isEnabled());
  assert(trace.removalTargets.length === beforePosts + 1, "Refused removal dispatched more than once");
  assert((await frame.locator('.person-confirm').innerText()).includes("Remove Ari Contact from People?"),
    "Removal retry changed its contact");
  await confirm.click();
  await waitForStatus(frame, "Removed from People.");
  await confirm.waitFor({ state: "hidden" });
  assert(trace.removalTargets.length === beforePosts + 2
    && trace.removalTargets.slice(beforePosts).every(id => id === "contact:accepted"),
    "Removal retry changed its exact target", trace);
  assert(await frame.locator('[data-action="remove"][data-contact-id="contact:other"]').isVisible(),
    "Removing one contact changed the other contact");
  assert(await frame.locator('[data-action="remove"][data-contact-id="contact:accepted"]').count() === 0,
    "Removed contact still has Remove");
  await assertNoOverflow(frame, `Removed contacts at ${width}px`);
}

async function runScenario(page, port, scenario, width, trace) {
  const frame = await openPeople(page, port, scenario, width);
  if (scenario === "first-run") {
    await assertFirstRunScenario(frame, width);
    return;
  }
  if (scenario === "ready-off") {
    await assertReadyOffScenario(frame, page, width);
    return;
  }
  if (scenario === "visible") {
    await assertVisibleDiscoveryScenario(frame, page, width);
    return;
  }
  if (scenario === "profile-failure") {
    await assertProfileFailureScenario(frame, width);
    return;
  }
  if (scenario === "remove-confirmation") {
    await assertRemoveConfirmationScenario(frame, page, width, trace);
    return;
  }
  throw new Error(`unknown People smoke scenario: ${scenario}`);
}

async function main() {
  const { server, trace } = await startServer();
  const port = server.address().port;
  const profile = await mkdtemp(join(tmpdir(), "elastos-people-layout-"));
  const context = await chromium.launchPersistentContext(profile, {
    executablePath: brave,
    headless: true,
    viewport: { width: 1280, height: 900 },
  });
  const page = context.pages()[0] || await context.newPage();
  try {
    await assertPhoneScenario(page, port);
    for (const width of [375, 1280]) {
      await runScenario(page, port, "first-run", width);
      await runScenario(page, port, "ready-off", width);
      await runScenario(page, port, "visible", width);
      await runScenario(page, port, "profile-failure", width);
      await runScenario(page, port, "remove-confirmation", width, trace);
    }
    assert(trace.discoveryRefreshes === 2, "People smoke changed Discovery refresh count", trace);
    assert(trace.discoveryToggles === 2, "People smoke changed Discovery toggle count", trace);
    assert(trace.discoveryRequests === 2, "People smoke changed discovery request count", trace);
    assert(trace.profileAttempts === 4, "People smoke changed Profile attempt count", trace);
    assert(trace.removalTargets.length === 4, "People smoke changed removal attempt count", trace);
    console.log("PASS People first-run, discovery, retry, exact contact confirmation, and layout");
  } finally {
    await context.close();
    server.close();
    await rm(profile, { recursive: true, force: true });
  }
}

main().catch((error) => {
  console.error(error.stack || error.message);
  process.exit(1);
});
