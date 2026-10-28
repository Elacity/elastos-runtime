#!/usr/bin/env node

import { createServer } from "node:http";
import { createRequire } from "node:module";
import { mkdir, mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { extname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { PHONE_VIEWPORT, setPhoneFormFactor } from "./lib/phone-drawer-assert.mjs";

const repoRoot = resolve(fileURLToPath(new URL("../", import.meta.url)));
const browserRoot = join(repoRoot, "capsules/people/browser");
const brave = process.env.BRAVE_BIN || "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser";
const require = createRequire(new URL("../elastos/tools/browser-playwright-engine/package.json", import.meta.url));
const { chromium } = require("playwright");
const PHONE_SCREENSHOT = "/tmp/people-phone-390x844.png";
const SHOTS_DIR = process.env.PEOPLE_SHOTS_DIR || "/tmp/people-pages-shots";

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
      ? "Public is on for up to ten minutes."
      : "Public is off. Turn it on for ten minutes to see who else is public.")
    : "Public isn’t available on this Home.",
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
    profile_id: readinessStatus === "ready" ? "did:key:z6MkLocalProfileFixture111111111111111111111111111" : null,
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
    status: "visible",
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
          statusMessage: "Public is off. Turn it on for ten minutes to see who else is public.",
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
      contacts: [
        { ...acceptedContact(), reachable: true },
        { contact_id: "contact:waiting", display_name: "", relationship: "requested", can_message: false },
        { contact_id: "contact:stuck", display_name: "Sam Waiting", relationship: "requested", can_message: false,
          reachable: true, delivery_detail: "They may be on an older version. Ask them to update, or turn on Public in People on both Homes." },
      ],
      discovery: discoverySummary({
        configured: true,
        enabled: true,
        status: "visible",
        statusMessage: "Public is on for up to ten minutes.",
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
          statusMessage: "Public is off. Turn it on for ten minutes to see who else is public.",
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

      if (["/apps/home/home-clipboard-client.js", "/apps/home/home-clipboard-protocol.js"].includes(url.pathname)) {
        const body = await readFile(join(repoRoot, "capsules/home/browser", url.pathname.split("/").at(-1)));
        response.writeHead(200, { "access-control-allow-origin": "null", "content-type": "text/javascript" });
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
      if (url.pathname === "/api/apps/people/contacts/request" && request.method === "POST") {
        const { profile_id } = await readJson(request);
        if (profile_id === "invalid") {
          response.writeHead(400, { "access-control-allow-origin": "null", "content-type": "text/plain" });
          response.end("That isn't a DID. A DID starts with did:key:z.");
          return;
        }
        assert(profile_id === "did:key:zRemote", "DID request body differs");
        return json(response, {
          status: "requested",
          message: "Request sent.",
        });
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
  await page.setViewportSize({ width, height: width === 375 ? 1400 : 900 });
  await page.goto(`http://127.0.0.1:${port}/fixture?scenario=${encodeURIComponent(scenario)}`, {
    waitUntil: "domcontentloaded",
  });
  return peopleFrame(page);
}

// People, Profile and Public are three pages; `discovery` is Public's id.
async function openSection(frame, id) {
  await frame.locator(`[data-section-target="${id}"]`).click();
  await frame.waitForFunction((section) => document.getElementById(section)?.hidden === false, id);
}

function shownSection(frame) {
  return frame.evaluate(() => [...document.querySelectorAll(".people-section")]
    .filter((section) => !section.hidden).map((section) => section.id).join(","));
}

// Evidence for a person: each page at this width, in the dark and the light theme.
async function screenshotPages(page, frame, width) {
  await mkdir(SHOTS_DIR, { recursive: true });
  for (const theme of ["dark", "light"]) {
    await frame.evaluate((next) => document.documentElement.setAttribute("data-el-theme", next), theme);
    for (const [id, name] of [["people", "people"], ["profile", "profile"], ["discovery", "public"]]) {
      await openSection(frame, id);
      await assertNoOverflow(frame, `${name} page (${theme}) at ${width}px`);
      await page.screenshot({ path: join(SHOTS_DIR, `${name}-${width}-${theme}.png`), fullPage: false });
    }
  }
  await frame.evaluate(() => document.documentElement.setAttribute("data-el-theme", "dark"));
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
      ".people-form",
      ".people-field-row",
      ".people-requests",
      ".people-list",
      ".person-card",
      ".public-head",
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
  const pageState = await frame.evaluate(() => {
    const body = document.body.cloneNode(true);
    body.querySelector("#profile-id-value")?.remove();
    body.querySelector("#add-contact-id")?.remove();
    return { text: body.textContent, html: body.innerHTML };
  });
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

const FIRST_RUN_COPY = "Choose the name people see. Public stays off until you turn it on.";

async function assertFirstRunScenario(frame, width) {
  // The static page has no description. Wait for the text only people.js writes,
  // so the checks below read the rendered first-run state, not the HTML before the script runs.
  await waitFor(() => frame.evaluate((copy) => document.querySelector("#profile-description")?.textContent?.trim()
    === copy, FIRST_RUN_COPY));
  assert(await shownSection(frame) === "profile", "first run must open on Profile", { shown: await shownSection(frame) });
  await assertNoOverflow(frame, `first run at ${width}px`);
  await assertVisible(frame, [
    "#profile-title",
    "#profile-description",
    "#profile-name",
    "#profile-submit",
  ], `first run at ${width}px`);
  const state = await frame.evaluate(() => ({
    title: document.querySelector("#profile-title")?.textContent?.trim() || "",
    value: document.querySelector("#profile-name")?.value || "",
    placeholder: document.querySelector("#profile-name")?.getAttribute("placeholder") || "",
    submit: document.querySelector("#profile-submit")?.textContent?.trim() || "",
  }));
  assert(state.title === "Create your Profile", "first run title is wrong", state);
  assert(state.value === "Suggested Profile Name", "first run should prefill the suggested profile name", state);
  assert(state.placeholder === "Your name", "first run should keep a stable editable placeholder", state);
  assert(state.submit === "Create Profile", "first run should offer explicit Profile creation", state);
  assert(await frame.locator("#profile-id").isHidden(), "Your DID appeared before Profile creation");
  await openSection(frame, "people");
  assert(await frame.locator("#add-contact-form").isHidden(), "Add appeared before Profile creation");
  assert(await frame.locator("#people-count").innerText() === "0", "first run contact count is wrong");
  assert(await frame.locator("#people-list").innerText() === "No contacts yet.", "first run empty list is wrong");
  await assertNoIdentityLeak(frame, `first run at ${width}px`);
}

async function assertReadyOffScenario(frame, page, width) {
  await waitFor(() => frame.evaluate(() => document.querySelector("#profile-form")?.dataset.profileState === "saved"));
  assert(await shownSection(frame) === "people", "a saved Profile must open on People");
  await openSection(frame, "profile");
  await assertNoOverflow(frame, `ready/off profile at ${width}px`);
  await assertVisible(frame, ["#profile-name", "#profile-submit", "#profile-id-value", "#profile-id-copy"],
    `ready/off profile at ${width}px`);
  const profile = await frame.evaluate(() => ({
    text: document.querySelector("#profile").innerText.replace(/\s+/g, " ").trim(),
    titleHidden: document.querySelector("#profile-title").hidden,
    descriptionHidden: document.querySelector("#profile-description").hidden,
    submit: document.querySelector("#profile-submit")?.textContent?.trim() || "",
  }));
  assert(profile.titleHidden && profile.descriptionHidden && profile.submit === "Save"
    && profile.text === "Display name Save Your DID did:key:z6MkLocalProfileFixture111111111111111111111111111 Copy",
  "a saved Profile shows only the name, Save, the DID and Copy", profile);
  await page.evaluate(() => {
    const child = document.querySelector("iframe").contentWindow;
    const homeToken = new URLSearchParams(new URL(document.querySelector("iframe").src).hash.slice(1)).get("home_token");
    child.postMessage({ type: "home:clipboard-ready", schema: "elastos.home.clipboard.ready/v1",
      targetId: "people", homeToken, parentOrigin: location.origin, generation: "fixture:1" }, "*");
    window.peopleCopies = [];
    window.addEventListener("message", (event) => {
      const data = event.data;
      if (event.source !== child || data?.type !== "home:clipboard-request") return;
      window.peopleCopies.push(data);
      child.postMessage({ type: "home:clipboard-result", schema: "elastos.home.clipboard.result/v1",
        requestId: data.requestId, targetId: "people", homeToken, parentOrigin: location.origin,
        generation: "fixture:1", operation: "write", purpose: "profile.id", ok: true }, "*");
    });
  });
  await frame.locator("#profile-id-copy").click();
  await waitForStatus(frame, "DID copied.");
  const copies = await page.evaluate(() => window.peopleCopies);
  assert(copies.length === 1 && copies[0].purpose === "profile.id"
    && copies[0].text === await frame.locator("#profile-id-value").innerText(), "Copy lost the Profile DID", copies);

  await openSection(frame, "people");
  assert(await frame.locator("#people-status").isHidden(), "a status line followed its page to another page");
  await assertVisible(frame, ["#add-contact-id", "#add-contact-submit"], `Add by DID at ${width}px`);
  await frame.locator("#add-contact-id").fill("invalid");
  await frame.locator("#add-contact-submit").click();
  await frame.waitForFunction(() => document.querySelector("#add-contact-result").textContent === "That isn't a DID. A DID starts with did:key:z.");
  assert(await frame.locator("#add-contact-id").inputValue() === "invalid", "Refusal cleared the input");
  await frame.locator("#add-contact-id").fill("did:key:zRemote");
  await frame.locator("#add-contact-id").press("Enter");
  await frame.waitForFunction(() => document.querySelector("#add-contact-result").textContent === "Request sent.");
  assert(await frame.locator("#add-contact-id").inputValue() === "", "Sent request kept the input");
  await assertNoOverflow(frame, `DID request at ${width}px`);

  await openSection(frame, "discovery");
  await assertNoOverflow(frame, `ready/off Public at ${width}px`);
  await assertVisible(frame, ["#discovery-status", "#discovery-toggle", "#discovery-refresh"],
    `ready/off Public controls at ${width}px`);
  const offState = await frame.evaluate(() => ({
    status: document.querySelector("#discovery-status")?.textContent?.trim() || "",
    toggle: document.querySelector("#discovery-toggle")?.textContent?.trim() || "",
    refreshDisabled: document.querySelector("#discovery-refresh")?.disabled === true,
    listHidden: document.querySelector("#discovery-people")?.hidden === true,
  }));
  assert(offState.status === "Public is off. Turn it on for ten minutes to see who else is public.",
    "ready/off should show the off Public state", offState);
  assert(offState.toggle === "Turn On", "ready/off should offer Public opt-in", offState);
  assert(offState.refreshDisabled === false, "ready/off should keep Refresh usable", offState);
  assert(offState.listHidden, "an empty list must not show while Public is off", offState);

  await frame.locator("#discovery-refresh").click();
  await frame.locator("#discovery-toggle").click();
  await waitFor(() => frame.evaluate(() => document.querySelector("#discovery-toggle")?.textContent?.trim() === "Turn Off"));
  const toggled = await frame.evaluate(() => ({
    status: document.querySelector("#discovery-status")?.textContent?.trim() || "",
    toggle: document.querySelector("#discovery-toggle")?.textContent?.trim() || "",
    list: document.querySelector("#discovery-list")?.textContent?.trim() || "",
    count: document.querySelector("#discovery-visible-count")?.textContent?.trim() || "",
    flashed: document.querySelector("#people-status")?.hidden === false,
  }));
  assert(
    toggled.status === "Public is on. 10 minutes left." && toggled.toggle === "Turn Off"
      && toggled.list === "No one else is public right now." && toggled.count === "0" && !toggled.flashed,
    "ready/off controls did not stay usable after opt-in",
    toggled,
  );

  const messages = await page.evaluate(() => window.peopleSmokeMessages || []);
  assert(!messages.some((message) => message?.target === "system"), "ready/off should not leak to System");
  await assertNoIdentityLeak(frame, `ready/off at ${width}px`);
}

async function assertVisibleDiscoveryScenario(frame, page, width, trace) {
  await waitFor(() => frame.evaluate(() => document.querySelector("#people-requests")?.hidden === false));
  await assertNoOverflow(frame, `People with a request at ${width}px`);
  const peopleState = await frame.evaluate(() => ({
    requests: document.querySelector("#people-requests-count")?.textContent?.trim() || "",
    rows: [...document.querySelectorAll("#people-list .person-card")].map((card) => [
      card.querySelector("h4")?.textContent || "",
      ...[...card.querySelectorAll(".person-details span, .person-actions button")].map((node) => node.textContent),
    ].join("|")),
  }));
  assert(peopleState.requests === "1 contact request waiting.", "People must point to the waiting request", peopleState);
  assert(peopleState.rows.length === 3 && peopleState.rows[0] === "Ari Contact|Online now|Message|Remove"
    && peopleState.rows[1] === "Request sent" && peopleState.rows[2] === "Sam Waiting|Not delivered",
  "each row is a name and one short status", peopleState);
  await assertVisible(frame, ['#people-requests [data-action="open-inbox"]'], `People requests at ${width}px`);
  // Undelivered rows say only Not delivered; one note above the list says what to do.
  await assertVisible(frame, ["#people-undelivered"], `undelivered note at ${width}px`);
  const note = await frame.evaluate(() => ({
    text: document.querySelector("#people-undelivered").textContent,
    rows: document.querySelector("#people-list").textContent,
  }));
  assert(note.text === "Some requests can't be delivered yet. Ask them to update, or turn on Public on both Homes."
    && !/older version|Why\?/.test(note.rows), "one note, not a reason on each row", note);
  await screenshotPages(page, frame, width);

  await openSection(frame, "discovery");
  await waitFor(() => frame.evaluate(() => document.querySelectorAll('#discovery-list [data-action="discovery-request"]').length === 1));
  await assertNoOverflow(frame, `visible Public at ${width}px`);
  await assertVisible(frame, [
    "#discovery-status",
    "#discovery-toggle",
    "#discovery-refresh",
    "#discovery-visible-count",
    '#discovery-list [data-action="discovery-request"]',
  ], `visible Public controls at ${width}px`);

  const discoveryState = await frame.evaluate(() => ({
    status: document.querySelector("#discovery-status")?.textContent?.trim() || "",
    addContact: document.querySelector('#discovery-list [data-action="discovery-request"]')?.textContent?.trim() || "",
    visibleCount: document.querySelector("#discovery-visible-count")?.textContent?.trim() || "",
  }));
  assert(discoveryState.status === "Public is on. 9 minutes left.", "visible Public should show the bounded window", discoveryState);
  assert(discoveryState.addContact === "Add contact", "visible Public action is wrong", discoveryState);
  assert(discoveryState.visibleCount === "1", "visible Public count is wrong", discoveryState);

  const requestsBefore = trace.discoveryRequests;
  await frame.locator('#discovery-list [data-action="discovery-request"]').click();
  await waitFor(() => trace.discoveryRequests === requestsBefore + 1);
  await openSection(frame, "people");
  await frame.locator('#people-requests [data-action="open-inbox"]').click();
  await waitFor(async () => (await page.evaluate(() => window.peopleSmokeMessages || []))
    .some((message) => message?.type === "home:open-target" && message.target === "inbox"));
  await assertNoIdentityLeak(frame, `visible Public at ${width}px`);
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
  await waitFor(() => frame.evaluate(() => document.querySelector("#profile-form")?.dataset.profileState === "saved"));
  await waitForStatus(frame, "Profile created.");
  const recovered = await frame.evaluate(() => ({
    titleHidden: document.querySelector("#profile-title")?.hidden === true,
    inputValue: document.querySelector("#profile-name")?.value || "",
    submitText: document.querySelector("#profile-submit")?.textContent?.trim() || "",
  }));
  assert(
    recovered.titleHidden
      && recovered.inputValue === "Retry Name"
      && recovered.submitText === "Save",
    "profile retry did not recover to the ready state",
    recovered,
  );
  await assertNoOverflow(frame, `profile failure recovered at ${width}px`);
}

// Phone: the three pages are one segmented row at the top, every segment a
// 44 px target, and the content follows with no page title above it.
async function assertPhoneScenario(page, port) {
  const frame = await openPeople(page, port, "ready-off", PHONE_VIEWPORT.width);
  await page.setViewportSize(PHONE_VIEWPORT);
  await setPhoneFormFactor(frame);
  await waitFor(() => frame.evaluate(() => document.querySelector("#profile-form")?.dataset.profileState === "saved"));
  const readRow = () => frame.evaluate(() => {
    const items = [...document.querySelectorAll("[data-section-target]")].map((item) => {
      const rect = item.getBoundingClientRect();
      return { label: item.innerText.trim(), top: Math.round(rect.top), left: Math.round(rect.left),
        right: Math.round(rect.right), height: Math.round(rect.height), active: item.classList.contains("active") };
    });
    const content = document.querySelector(".people-content").getBoundingClientRect();
    const title = getComputedStyle(document.querySelector(".people-page-title")).display;
    return { items, contentTop: Math.round(content.top), title, width: innerWidth };
  });
  const row = await readRow();
  assert(
    row.items.map((item) => item.label).join(",") === "People,Profile,Public"
      && row.items.every((item) => item.top === row.items[0].top && item.height >= 44 && item.left >= 0 && item.right <= row.width)
      && row.items[0].top <= 16 && row.contentTop >= row.items[0].top + 44 && row.title === "none"
      && row.items[0].active,
    "People phone: the pages must be one row of 44 px segments above the content",
    row,
  );
  await assertNoOverflow(frame, "People phone");
  await page.screenshot({ path: PHONE_SCREENSHOT });
  await openSection(frame, "discovery");
  assert((await readRow()).items[2].active, "People phone: Public did not become the active segment");
  await assertNoOverflow(frame, "People phone Public");
}

async function assertRemoveConfirmationScenario(frame, page, width, trace) {
  const beforePosts = trace.removalTargets.length;
  const remove = frame.locator('[data-action="remove"][data-contact-id="contact:accepted"]');
  await remove.waitFor({ state: "visible" });
  await remove.evaluate(node => node.scrollIntoView({ block: "center" }));
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
  await remove.evaluate(node => node.scrollIntoView({ block: "center" }));
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
    await assertVisibleDiscoveryScenario(frame, page, width, trace);
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
      await runScenario(page, port, "visible", width, trace);
      await runScenario(page, port, "profile-failure", width);
      await runScenario(page, port, "remove-confirmation", width, trace);
    }
    assert(trace.discoveryRefreshes === 2, "People smoke changed Public refresh count", trace);
    assert(trace.discoveryToggles === 2, "People smoke changed Public toggle count", trace);
    assert(trace.discoveryRequests === 2, "People smoke changed Public request count", trace);
    assert(trace.profileAttempts === 4, "People smoke changed Profile attempt count", trace);
    assert(trace.removalTargets.length === 4, "People smoke changed removal attempt count", trace);
    console.log(`PASS People, Profile and Public pages: first run, Public, retry, exact contact confirmation, layout; screenshots in ${SHOTS_DIR}`);
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
