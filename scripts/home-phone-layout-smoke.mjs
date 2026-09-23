#!/usr/bin/env node
// Phone and tablet layout smoke for the Home GUI shell.
//
// Loads the real Home host and Home GUI source against a fixture host (the
// same pattern as home-browser-restored-lifecycle-headless-smoke.mjs), then
// measures every shell surface at three coarse-pointer profiles: iPhone 14
// portrait, iPhone 14 landscape and a 820 px tablet. For each surface it
// records interactive targets under 44 px, visible text under 12 px and
// horizontal overflow, writes a JSON report plus screenshots, and asserts a
// ratchet: no surface may get worse than the baseline pinned below. Each
// mobile PR lowers its baseline rows; nothing here ever raises one.
//
// Source truths that do not need a browser (viewport-fit, bare 100vh) are
// asserted here too so the report is one document.

import { createServer } from "node:http";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { extname, isAbsolute, join, relative, resolve, sep } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const repoRoot = fileURLToPath(new URL("../", import.meta.url));
const outputRoot = process.env.HOME_PHONE_SMOKE_OUT
  ? resolve(process.env.HOME_PHONE_SMOKE_OUT)
  : join(tmpdir(), `home-phone-layout-smoke-${process.pid}`);

// Apple HIG and WCAG 2.5.5 minimum target; the shell's phone contract.
const MIN_TARGET_PX = 44;
// Smallest text a phone user should have to read in shell chrome.
const MIN_TEXT_PX = 12;
const BOOT_TIMEOUT_MS = 20_000;
const SURFACE_SETTLE_MS = 400;
// Launcher and sheets animate in; measure after the motion finishes.
const SHEET_SETTLE_MS = 900;

const homeAuthorityToken = "fixture-home-authority-token";
const homeGuiToken = "fixture-home-gui-launch-token";
const principalId = "did:elastos:fixture-phone-layout";

// Apps the default `elastos setup` home profile installs and a person can
// open from the launcher. The fixture serves each capsule's browser
// directory so windows load real capsule chrome; their APIs answer 404 so
// capsules render their own empty or error states, which is enough to
// measure the shell around them.
const FIRST_PARTY_APPS = [
  ["library", "Library", "Browse files and folders"],
  ["documents", "Documents", "Write and publish markdown"],
  ["marketplace", "Marketplace", "Find and install apps"],
  ["system", "System", "Runtime, updates and devices"],
  ["people", "People", "Contacts and trusted peers"],
  ["services", "Services", "Offers this Home can use"],
  ["wallet", "Wallet", "Keys, chains and approvals"],
  ["inbox", "Inbox", "Requests that need a look"],
  ["archive-manager", "Archive", "Open and create ZIP archives"],
  ["elacity-player", "Elacity Player", "Protected video viewer"],
  ["assistant", "Assistant", "Typed model access"],
  ["browser", "Browser", "Private Runtime Browser"],
];

const PROFILES = [
  {
    id: "phone-portrait",
    viewport: { width: 390, height: 844 },
    deviceScaleFactor: 3,
    isMobile: true,
    hasTouch: true,
  },
  {
    id: "phone-landscape",
    viewport: { width: 844, height: 390 },
    deviceScaleFactor: 3,
    isMobile: true,
    hasTouch: true,
  },
  {
    id: "tablet",
    viewport: { width: 820, height: 1180 },
    deviceScaleFactor: 2,
    isMobile: true,
    hasTouch: true,
  },
];

// Ratchet baseline: measured on 2026-09-23 against the shell before any phone
// work, rounded up by one to absorb font rounding across engines. A PR that
// improves a surface lowers its row; a PR may never raise one. `null` means
// the surface is recorded but not yet gated on that profile.
const BASELINE = {
  "phone-portrait": {
    desktop: { targets: 8, text: 2 },
    launcher: { targets: 9, text: 2 },
    spotlight: { targets: 8, text: 2 },
    "control-centre": { targets: 33, text: 9 },
    notifications: { targets: 9, text: 42 },
    "mission-control": { targets: 1, text: 1 },
    "assistant-face": { targets: 1, text: 1 },
    window: { targets: 13, text: 2 },
  },
  "phone-landscape": {
    desktop: { targets: 8, text: 2 },
    launcher: { targets: 9, text: 2 },
    spotlight: { targets: 8, text: 2 },
    "control-centre": { targets: 25, text: 7 },
    notifications: { targets: 9, text: 5 },
    "mission-control": { targets: 1, text: 1 },
    "assistant-face": { targets: 1, text: 1 },
    window: { targets: 13, text: 2 },
  },
  tablet: {
    desktop: { targets: 8, text: 2 },
    launcher: { targets: 9, text: 2 },
    spotlight: { targets: 8, text: 2 },
    "control-centre": { targets: 30, text: 8 },
    notifications: { targets: 9, text: 42 },
    "mission-control": { targets: 1, text: 1 },
    "assistant-face": { targets: 1, text: 1 },
    window: { targets: 13, text: 2 },
  },
};

const state = { errors: [] };
const pageErrors = [];
const capsuleErrors = []; // capsule-side and engine-noise errors, reported not asserted
const consoleErrors = [];

function assert(condition, message, details = undefined) {
  if (condition) {
    return;
  }
  const suffix = details === undefined ? "" : `\n${JSON.stringify(details, null, 2)}`;
  throw new Error(`${message}${suffix}`);
}

function json(res, status, value) {
  const body = JSON.stringify(value);
  res.writeHead(status, {
    "access-control-allow-origin": "*",
    "cache-control": "no-store",
    "content-length": Buffer.byteLength(body),
    "content-type": "application/json; charset=utf-8",
  });
  res.end(body);
}

function empty(res, status = 204) {
  res.writeHead(status, {
    "access-control-allow-origin": "*",
    "cache-control": "no-store",
  });
  res.end();
}

function contentType(path) {
  return {
    ".css": "text/css; charset=utf-8",
    ".html": "text/html; charset=utf-8",
    ".js": "text/javascript; charset=utf-8",
    ".json": "application/json; charset=utf-8",
    ".mjs": "text/javascript; charset=utf-8",
    ".png": "image/png",
    ".svg": "image/svg+xml",
    ".webmanifest": "application/manifest+json",
    ".webp": "image/webp",
    ".woff2": "font/woff2",
  }[extname(path)] || "application/octet-stream";
}

const STATIC_ROOTS = [
  ["/apps/home/", join(repoRoot, "capsules/home/browser")],
  ["/apps/home-gui/", join(repoRoot, "capsules/home-gui/browser")],
  ...FIRST_PARTY_APPS.map(([id]) => [`/apps/${id}/`, join(repoRoot, `capsules/${id}/browser`)]),
];

function staticPath(pathname) {
  for (const [prefix, root] of STATIC_ROOTS) {
    if (!pathname.startsWith(prefix)) {
      continue;
    }
    const suffix = decodeURIComponent(pathname.slice(prefix.length)) || "index.html";
    const candidate = resolve(root, suffix);
    const escaped = relative(root, candidate);
    if (escaped.startsWith(`..${sep}`) || escaped === ".." || isAbsolute(escaped)) {
      return null;
    }
    return candidate;
  }
  return null;
}

async function readBody(req) {
  const chunks = [];
  let bytes = 0;
  for await (const chunk of req) {
    bytes += chunk.length;
    if (bytes > 1_048_576) {
      throw new Error("fixture request body exceeds 1 MiB");
    }
    chunks.push(chunk);
  }
  const text = Buffer.concat(chunks).toString("utf8");
  return text ? JSON.parse(text) : null;
}

function iconVariants(id) {
  const root = join(repoRoot, `capsules/${id}/browser/icons`);
  return [32, 64, 128, 256]
    .filter((size) => existsSync(join(root, `icon-${size}.png`)))
    .map((size) => ({ size, route: `/apps/${id}/icons/icon-${size}.png` }));
}

function appTarget([id, title, description]) {
  return {
    target: id,
    title,
    description,
    route: `/apps/${id}/`,
    attach_kind: "iframe",
    role: "app",
    target_kind: "app",
    icon: iconVariants(id),
  };
}

function homeSummary() {
  return {
    home: { route: "/apps/home/", attach_kind: "iframe" },
    app: { id: "home", route: "/apps/home/" },
    identity: {
      device_did: null,
      profile_readiness: { schema: "elastos.profile.readiness/v1", status: "ready" },
      recovery_readiness: { schema: "elastos.recovery.readiness/v1", status: "ready" },
    },
    authority: {
      signed_in: true,
      principal_id: principalId,
      session_id: "fixture-session",
      proof_binding_id: "fixture-proof-binding",
      wallet_connected: false,
    },
    browser_state: {
      schema: "elastos.home.browser-state/v1",
      principal_id: principalId,
      localhost_root: "localhost://fixture",
      layout: null,
      recent_targets: [],
      session: { browser_context_id: null, root_shell: "home-gui", windows: [] },
    },
    active_shell: {
      schema: "elastos.home.active-shell/v1",
      active: "home-gui",
      candidates: [
        {
          name: "home-gui",
          title: "Desktop",
          description: "Fixture Desktop",
          route: "/apps/home-gui/",
          role: "shell",
          trust_state: "trusted",
          launchable: true,
        },
      ],
    },
    appearance: {
      schema: "elastos.home.appearance/v1",
      revision: 0,
      theme: "dark",
      accent: "blue",
      accent_custom: "#4f7fff",
      dock_auto_hide: false,
      sounds: false,
      focus_mode: false,
      background_image_url: null,
      background_overlay_enabled: true,
      background_overlay_opacity: 0.55,
    },
    runtime: {
      running: true,
      kind: "local",
      version: null,
      api_url: null,
      pid: null,
      running_capsules: [],
      note: null,
    },
    site: {
      staged: false,
      root_uri: "",
      path: "",
      active_release: null,
      active_channel: null,
      active_bundle_cid: null,
      release_count: 0,
    },
    room: {
      room_slug: "fixture",
      title: "",
      member_count: 0,
      active_member_count: 0,
      pending_count: 0,
      active_session_count: 0,
      latest_request_name: null,
      latest_request_device: null,
      local_runtime_did: null,
      local_runtime_role: null,
      canonical_hosted_guest_url: null,
      ephemeral_hosted_guest_url: null,
      browser_access_allowed: true,
      browser_access_block_reason: null,
      pending_requests: [],
      active_sessions: [],
    },
    people: {
      schema: "elastos.people.contacts/v1",
      contact_count: 0,
      contacts: [],
      service_offer_count: 0,
      service_offers: [],
    },
    services: {
      schema: "elastos.runtime.services/v1",
      local_offer_count: 0,
      remote_offer_count: 0,
      available_local_offer_count: 0,
      available_remote_offer_count: 0,
      local_offers: [],
      remote_offers: [],
      available_local_offers: [],
      available_remote_offers: [],
      grant_model: "principal_scoped_provider_grant",
      carrier_contract: "People discovers trusted offers; Carrier carries signed offer envelopes; providers enforce grants.",
      capsule_contract: "capsule -> runtime capability -> provider grant -> service",
    },
    notifications: {
      unread_count: 1,
      attention_count: 1,
      entries: [
        {
          id: "fixture-request",
          kind: "contact_request",
          title: "Contact request",
          body: "Jordan wants to connect.",
          severity: "attention",
          read: false,
          created_at: Math.floor(Date.now() / 1000) - 90,
          source_app: "people",
          action_ref: { action_id: "contact-accept-request:fixture-request" },
        },
      ],
    },
    desktop_objects: {
      schema: "elastos.home.desktop-objects/v1",
      uri: "localhost://fixture/Desktop",
      objects: [],
      stale: false,
    },
    capsule_catalog: {
      schema: "elastos.capsules.catalog/v1",
      capsules: [],
      counts: {
        total: 0,
        installed: 0,
        launchable: 0,
        interfaces: 0,
        methods: 0,
        apps: 0,
        viewers: 0,
        providers: 0,
        content: 0,
        shell: 0,
      },
      policy: {
        install_state: "signed-app-install-pending",
        install_note: "Fixture inventory",
        payment_state: "provider-rail-required",
        payment_note: "",
        drm_state: "provider-rail-required",
        drm_note: "",
      },
    },
    capsule_interfaces: {
      schema: "elastos.capsules.interfaces/v1",
      interfaces: [],
      counts: { capsules: 0, interfaces: 0, methods: 0, executable_methods: 0 },
      policy: {
        descriptor_state: "manifest-declared",
        descriptor_note: "Fixture inventory",
        invocation_state: "runtime-gated",
        invocation_note: "",
      },
    },
    targets: FIRST_PARTY_APPS.map(appTarget),
  };
}

function launchRoute(origin, target, query) {
  const params = new URLSearchParams();
  for (const [key, value] of Object.entries(query || {})) {
    params.set(key, String(value));
  }
  params.set("home_origin", origin);
  const token = target === "home-gui" ? homeGuiToken : `fixture-${target}-launch-token`;
  return `/apps/${target}/?${params.toString()}#home_token=${encodeURIComponent(token)}`;
}

function requireToken(req, expected, res) {
  if (req.headers["x-elastos-home-token"] === expected) {
    return true;
  }
  json(res, 401, { error: "fixture launch owner rejected" });
  return false;
}

async function handleApi(req, res, url) {
  if (req.method === "OPTIONS") {
    res.writeHead(204, {
      "access-control-allow-headers": "content-type,x-elastos-home-token",
      "access-control-allow-methods": "GET,POST,OPTIONS",
      "access-control-allow-origin": "*",
      "access-control-max-age": "60",
    });
    res.end();
    return true;
  }
  if (url.pathname === "/api/auth/sessions/refresh" && req.method === "POST") {
    json(res, 200, { home_token: homeAuthorityToken });
    return true;
  }
  if (url.pathname === "/api/apps/home/summary" && req.method === "GET") {
    json(res, 200, homeSummary());
    return true;
  }
  if (url.pathname === "/api/apps/home/collaboration/presence" && req.method === "POST") {
    if (!requireToken(req, homeAuthorityToken, res)) return true;
    await readBody(req);
    json(res, 200, {
      schema: "elastos.people.discovery/v1",
      configured: false,
      enabled: false,
      status: "unconfigured",
      status_message: "Discovery isn’t available on this Home.",
      discovered_count: 0,
      discovered_peers: [],
      request_count: 0,
    });
    return true;
  }
  if (url.pathname === "/api/apps/home/runtime/ensure" && req.method === "POST") {
    json(res, 200, { ready: true });
    return true;
  }
  if (url.pathname === "/api/apps/home/events/stream" && req.method === "GET") {
    empty(res);
    return true;
  }
  if (url.pathname === "/api/apps/home/events" && req.method === "GET") {
    json(res, 200, { events: [], cursor: "" });
    return true;
  }
  if (url.pathname === "/api/apps/home/launch" && req.method === "POST") {
    if (!requireToken(req, homeAuthorityToken, res)) return true;
    const input = await readBody(req);
    const target = typeof input?.target === "string" ? input.target : "";
    const known = target === "home-gui" || FIRST_PARTY_APPS.some(([id]) => id === target);
    if (!known) {
      json(res, 404, { error: "fixture target not found", target });
      return true;
    }
    const origin = String(input?.query?.home_origin || "");
    const title = target === "home-gui"
      ? "Desktop"
      : FIRST_PARTY_APPS.find(([id]) => id === target)[1];
    json(res, 200, {
      target,
      title,
      attach_kind: "iframe",
      launch_status: "launched",
      route: launchRoute(origin, target, input?.query || {}),
    });
    return true;
  }
  if (url.pathname === "/api/apps/home/state" && req.method === "POST") {
    if (!requireToken(req, homeGuiToken, res)) return true;
    await readBody(req);
    json(res, 200, { saved: true });
    return true;
  }
  if (url.pathname.startsWith("/api/")) {
    // Capsule APIs are out of scope: the capsule shows its own empty/error
    // state and the shell around it is what this smoke measures.
    json(res, 404, { error: "fixture api route not stubbed", path: url.pathname });
    return true;
  }
  return false;
}

const server = createServer(async (req, res) => {
  try {
    const url = new URL(req.url || "/", "http://127.0.0.1");
    if (await handleApi(req, res, url)) {
      return;
    }
    const path = staticPath(url.pathname);
    if (!path || !existsSync(path)) {
      json(res, 404, { error: "fixture route not found", path: url.pathname });
      return;
    }
    const body = readFileSync(path);
    res.writeHead(200, {
      "access-control-allow-origin": "*",
      "cache-control": "no-store",
      "content-length": body.length,
      "content-type": contentType(path),
    });
    res.end(body);
  } catch (error) {
    state.errors.push(String(error?.stack || error));
    if (!res.headersSent) {
      json(res, 500, { error: String(error?.message || error) });
    } else {
      res.end();
    }
  }
});

async function listen() {
  await new Promise((resolveListen, rejectListen) => {
    server.once("error", rejectListen);
    server.listen(0, "127.0.0.1", resolveListen);
  });
  const address = server.address();
  assert(address && typeof address === "object", "fixture server did not bind");
  return `http://127.0.0.1:${address.port}`;
}

function playwrightSpecifier() {
  const configured = process.env.ELASTOS_PLAYWRIGHT_MODULE || "";
  if (configured) {
    return configured.startsWith("file:") ? configured : pathToFileURL(resolve(configured)).href;
  }
  return pathToFileURL(
    join(repoRoot, "elastos/tools/browser-playwright-engine/node_modules/playwright/index.js"),
  ).href;
}

function sleep(ms) {
  return new Promise((resolveSleep) => setTimeout(resolveSleep, ms));
}

async function waitFor(check, timeoutMs, label) {
  const startedAt = Date.now();
  while (Date.now() - startedAt <= timeoutMs) {
    assert(
      pageErrors.length === 0 && state.errors.length === 0,
      `fixture failed while waiting for ${label}`,
      { pageErrors, serverErrors: state.errors },
    );
    if (await check()) {
      return;
    }
    await sleep(50);
  }
  throw new Error(`timed out waiting for ${label}: ${JSON.stringify({ pageErrors, serverErrors: state.errors })}`);
}

function homeGuiFrame(page) {
  return page.frames().find((frame) => frame.url().includes("/apps/home-gui/")) || null;
}

// Runs inside the GUI frame. Counts visible interactive targets whose smaller
// side is under the minimum and visible text nodes rendered under the
// minimum font size, both clipped to the frame viewport.
function measureSurface({ minTarget, minText }) {
  const viewportWidth = window.innerWidth;
  const viewportHeight = window.innerHeight;

  function isVisible(element) {
    if (!(element instanceof Element)) return false;
    if (element.closest("[hidden], [aria-hidden='true']")) return false;
    if (typeof element.checkVisibility === "function" && !element.checkVisibility({ checkOpacity: true })) {
      return false;
    }
    const style = getComputedStyle(element);
    if (style.display === "none" || style.visibility === "hidden" || Number(style.opacity) === 0) {
      return false;
    }
    const rect = element.getBoundingClientRect();
    if (rect.width <= 0 || rect.height <= 0) return false;
    return rect.bottom > 0 && rect.right > 0 && rect.top < viewportHeight && rect.left < viewportWidth;
  }

  const targetSelector = "button, a[href], [role='button'], [role='tab'], [role='option'], [role='menuitem'], input:not([type='hidden']), select, textarea, [tabindex]:not([tabindex='-1'])";
  const smallTargets = [];
  for (const element of document.querySelectorAll(targetSelector)) {
    if (!isVisible(element)) continue;
    const rect = element.getBoundingClientRect();
    const side = Math.min(rect.width, rect.height);
    if (side < minTarget) {
      smallTargets.push({
        selector: `${element.tagName.toLowerCase()}${element.id ? `#${element.id}` : ""}${element.className && typeof element.className === "string" ? `.${element.className.trim().split(/\s+/).slice(0, 2).join(".")}` : ""}`,
        width: Math.round(rect.width),
        height: Math.round(rect.height),
      });
    }
  }

  const smallText = [];
  const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
  const seen = new Set();
  let node = walker.nextNode();
  while (node) {
    const text = node.textContent.trim();
    const parent = node.parentElement;
    if (text && parent && !seen.has(parent) && isVisible(parent)) {
      seen.add(parent);
      const size = parseFloat(getComputedStyle(parent).fontSize);
      if (size < minText) {
        smallText.push({
          selector: `${parent.tagName.toLowerCase()}${parent.id ? `#${parent.id}` : ""}${parent.className && typeof parent.className === "string" ? `.${parent.className.trim().split(/\s+/).slice(0, 2).join(".")}` : ""}`,
          fontSize: Math.round(size * 10) / 10,
          sample: text.slice(0, 40),
        });
      }
    }
    node = walker.nextNode();
  }

  return {
    viewport: { width: viewportWidth, height: viewportHeight },
    overflow: document.documentElement.scrollWidth > document.documentElement.clientWidth,
    targetCount: document.querySelectorAll(targetSelector).length,
    smallTargets,
    smallText,
  };
}

function geometry() {
  const rect = (selector) => {
    const element = document.querySelector(selector);
    if (!element) return null;
    const box = element.getBoundingClientRect();
    return { x: Math.round(box.x), y: Math.round(box.y), width: Math.round(box.width), height: Math.round(box.height) };
  };
  return {
    toolbar: rect(".toolbar"),
    dock: rect(".taskbar"),
    window: rect(".window:not([aria-hidden='true'])"),
    launcher: rect(".launcher-popover:not([hidden])"),
    bodyClasses: Array.from(document.body.classList),
    formFactor: document.body.dataset.formFactor || null,
  };
}

async function measure(frame, label) {
  const metrics = await frame.evaluate(measureSurface, { minTarget: MIN_TARGET_PX, minText: MIN_TEXT_PX });
  const shape = await frame.evaluate(geometry);
  return { surface: label, ...metrics, geometry: shape };
}

async function pressEscape(frame) {
  await frame.page().keyboard.press("Escape");
  await sleep(SURFACE_SETTLE_MS);
}

// Each surface: how to open it, how long it takes to settle, how to close it.
const SHELL_SURFACES = [
  {
    id: "desktop",
    open: async () => {},
    settle: SURFACE_SETTLE_MS,
    close: async () => {},
  },
  {
    id: "launcher",
    open: (frame) => frame.locator("#launcher-toggle").click(),
    settle: SHEET_SETTLE_MS,
    close: pressEscape,
  },
  {
    id: "spotlight",
    open: (frame) => frame.locator("#toolbar-spotlight").click(),
    settle: SHEET_SETTLE_MS,
    close: pressEscape,
  },
  {
    id: "control-centre",
    open: (frame) => frame.locator("#toolbar-control-centre").click(),
    settle: SHEET_SETTLE_MS,
    close: pressEscape,
  },
  {
    id: "notifications",
    open: (frame) => frame.locator("#clock").click(),
    settle: SHEET_SETTLE_MS,
    close: pressEscape,
  },
  {
    id: "mission-control",
    open: (frame) => frame.locator("#toolbar-mission-control").click(),
    settle: SHEET_SETTLE_MS,
    close: pressEscape,
  },
  {
    id: "assistant-face",
    open: (frame) => frame.locator("#assistant-toggle").click(),
    settle: SHEET_SETTLE_MS,
    close: pressEscape,
  },
];

async function openWindow(frame, target) {
  const dockButton = frame.locator(`.taskbar-item[data-target="${target}"]`);
  if (await dockButton.count()) {
    await dockButton.first().click();
  } else {
    await frame.locator("#launcher-toggle").click();
    await sleep(SHEET_SETTLE_MS);
    await frame.locator(`.launcher-card[data-target="${target}"]`).first().click();
  }
  await waitFor(
    () => frame.locator(`.window[data-target="${target}"]`).count().then((count) => count > 0),
    BOOT_TIMEOUT_MS,
    `${target} window`,
  );
  await sleep(SHEET_SETTLE_MS);
}

async function closeWindow(frame, target) {
  const closeButton = frame.locator(`.window[data-target="${target}"] [data-action="close"]`);
  if (await closeButton.count()) {
    await closeButton.first().click();
  }
  await waitFor(
    () => frame.locator(`.window[data-target="${target}"]`).count().then((count) => count === 0),
    BOOT_TIMEOUT_MS,
    `${target} window closed`,
  );
  await sleep(SURFACE_SETTLE_MS);
}

async function screenshot(page, dir, name) {
  await page.screenshot({ path: join(dir, `${name}.png`), fullPage: false });
}

async function bootHome(context, origin) {
  const page = await context.newPage();
  page.on("pageerror", (error) => {
    const text = String(error?.stack || error).slice(0, 2000);
    // Capsules fail their own API calls under the fixture (no Runtime); that
    // is their concern, not the shell's. Only Home host and GUI errors count.
    const fromCapsule = /\/apps\/(?!home\/|home-gui\/)[a-z-]+\//.test(text);
    // WebKit surfaces the caught cross-frame probe in installFrameAutoFit
    // (shell-windows.js, wrapped in try/catch) as a page error; Chromium
    // returns null silently. The shell keeps working, so it is recorded as
    // engine noise rather than a failure.
    const webkitSandboxProbe = /Sandbox access violation/.test(text) && /shell-windows\.js/.test(text);
    if (fromCapsule || webkitSandboxProbe) {
      capsuleErrors.push(text);
    } else if (pageErrors.length < 20) {
      pageErrors.push(text);
    }
  });
  page.on("console", (message) => {
    // Console errors are recorded, not asserted: capsule API 404s are expected
    // under the fixture and WebKit logs the caught cross-frame probe in
    // installFrameAutoFit as a console error. Uncaught shell errors still fail.
    if (message.type() === "error" && consoleErrors.length < 40) {
      consoleErrors.push(message.text().slice(0, 400));
    }
  });
  await page.goto(`${origin}/apps/home/`, { waitUntil: "domcontentloaded" });
  await waitFor(() => page.evaluate(() => document.body.dataset.homeStatus === "ready"), BOOT_TIMEOUT_MS, "Home bootstrap");
  try {
    await waitFor(() => homeGuiFrame(page), BOOT_TIMEOUT_MS, "Home GUI frame");
  } catch (error) {
    const hostState = await page.evaluate(() => ({
      dataset: { ...document.body.dataset },
      frames: Array.from(document.querySelectorAll("iframe")).map((frame) => ({
        id: frame.id,
        src: frame.getAttribute("src"),
        hidden: frame.hidden,
      })),
      recovery: document.querySelector("#shell-host-recovery:not([hidden])")?.textContent?.trim() || null,
    })).catch(() => null);
    throw new Error(`${error.message}\nhost state: ${JSON.stringify(hostState, null, 2)}\nconsole: ${JSON.stringify(consoleErrors.slice(-5), null, 2)}`);
  }
  const frame = homeGuiFrame(page);
  await waitFor(() => frame.locator("#launcher-toggle").isVisible(), BOOT_TIMEOUT_MS, "Home GUI dock");
  await sleep(SHEET_SETTLE_MS);
  return { page, frame };
}

async function runProfile(browser, engineId, profile, origin) {
  const dir = join(outputRoot, engineId, profile.id);
  mkdirSync(dir, { recursive: true });
  const context = await browser.newContext({
    viewport: profile.viewport,
    deviceScaleFactor: profile.deviceScaleFactor,
    isMobile: profile.isMobile,
    hasTouch: profile.hasTouch,
    reducedMotion: "reduce",
  });
  const surfaces = [];
  try {
    const { page, frame } = await bootHome(context, origin);
    for (const surface of SHELL_SURFACES) {
      await surface.open(frame);
      await sleep(surface.settle);
      surfaces.push(await measure(frame, surface.id));
      await screenshot(page, dir, surface.id);
      await surface.close(frame);
    }
    for (const [target] of FIRST_PARTY_APPS) {
      if (target === "assistant") {
        // The Assistant opens as the face, measured above, never as a window.
        continue;
      }
      await openWindow(frame, target);
      const shell = await measure(frame, "window");
      shell.target = target;
      const capsuleFrame = page.frames().find((candidate) => candidate.url().includes(`/apps/${target}/`));
      shell.capsule = capsuleFrame
        ? await capsuleFrame.evaluate(measureSurface, { minTarget: MIN_TARGET_PX, minText: MIN_TEXT_PX }).catch(() => null)
        : null;
      surfaces.push(shell);
      await screenshot(page, dir, `window-${target}`);
      if (target === "browser") {
        // Browser close is a Runtime handshake (page close result) the fixture
        // does not answer; it is the last window, the context closes it.
        continue;
      }
      await closeWindow(frame, target);
    }
  } finally {
    await context.close();
  }
  return { engine: engineId, profile: profile.id, viewport: profile.viewport, surfaces };
}

function sourceTruths() {
  const homeIndex = readFileSync(join(repoRoot, "capsules/home/browser/index.html"), "utf8");
  const guiIndex = readFileSync(join(repoRoot, "capsules/home-gui/browser/index.html"), "utf8");
  const guiStyle = readFileSync(join(repoRoot, "capsules/home-gui/browser/style.css"), "utf8");
  const viewportFit = (html) => /<meta\s+name="viewport"\s+content="[^"]*viewport-fit=cover[^"]*"/.test(html);
  // A 100vh line is a fallback when the next line repeats it with 100dvh;
  // any other 100vh is bare and wrong on phones.
  const styleLines = guiStyle.split("\n");
  const bareViewportHeightUnits = styleLines.filter((line, index) =>
    line.includes("100vh") && !(styleLines[index + 1] || "").includes(line.replace("100vh", "100dvh").trim())).length;
  return {
    viewportFitCover: { home: viewportFit(homeIndex), gui: viewportFit(guiIndex) },
    bareViewportHeightUnits,
    viewportHeightFallbacks: (guiStyle.match(/\b100vh\b/g) || []).length - bareViewportHeightUnits,
    dynamicViewportHeightUnits: (guiStyle.match(/\b100dvh\b/g) || []).length,
    backdropFilters: (guiStyle.match(/backdrop-filter\s*:/g) || []).length,
  };
}

function shellFailures(run) {
  const failures = [];
  const baseline = BASELINE[run.profile] || {};
  for (const surface of run.surfaces) {
    const limits = baseline[surface.surface];
    if (!limits) continue;
    const label = `${run.engine}/${run.profile}/${surface.surface}${surface.target ? `:${surface.target}` : ""}`;
    if (surface.overflow) {
      failures.push(`${label}: horizontal overflow`);
    }
    if (limits.targets !== null && surface.smallTargets.length > limits.targets) {
      failures.push(`${label}: ${surface.smallTargets.length} targets < ${MIN_TARGET_PX}px (baseline ${limits.targets})`);
    }
    if (limits.text !== null && surface.smallText.length > limits.text) {
      failures.push(`${label}: ${surface.smallText.length} texts < ${MIN_TEXT_PX}px (baseline ${limits.text})`);
    }
  }
  return failures;
}

function summarize(run) {
  return run.surfaces
    .map((surface) => `${surface.surface}${surface.target ? `:${surface.target}` : ""}=${surface.smallTargets.length}/${surface.smallText.length}${surface.overflow ? "!" : ""}`)
    .join(" ");
}

async function launchEngine(playwright, engineId) {
  const engine = playwright[engineId];
  const executable = engineId === "chromium" && process.env.ELASTOS_BROWSER_EXECUTABLE
    ? process.env.ELASTOS_BROWSER_EXECUTABLE
    : engine.executablePath();
  if (!existsSync(executable)) {
    return null;
  }
  const args = engineId === "chromium"
    ? [
        "--disable-background-networking",
        "--disable-breakpad",
        "--disable-component-update",
        "--disable-domain-reliability",
        "--disable-extensions",
        "--disable-sync",
        "--host-resolver-rules=MAP * ~NOTFOUND, EXCLUDE 127.0.0.1, EXCLUDE localhost",
        "--no-first-run",
        "--no-proxy-server",
      ]
    : [];
  return engine.launch({ executablePath: executable, headless: true, args });
}

const requestedEngines = (process.env.HOME_PHONE_SMOKE_ENGINES || "chromium,webkit")
  .split(",")
  .map((value) => value.trim())
  .filter(Boolean);

const origin = await listen();
mkdirSync(outputRoot, { recursive: true });
const report = { generatedAt: new Date().toISOString(), origin, source: sourceTruths(), runs: [] };
const failures = [];
const skippedEngines = [];
try {
  const imported = await import(playwrightSpecifier());
  const playwright = imported.default || imported;
  for (const engineId of requestedEngines) {
    assert(engineId === "chromium" || engineId === "webkit", `unsupported engine ${engineId}`);
    const browser = await launchEngine(playwright, engineId);
    if (!browser) {
      skippedEngines.push(engineId);
      continue;
    }
    try {
      for (const profile of PROFILES) {
        const run = await runProfile(browser, engineId, profile, origin);
        report.runs.push(run);
        failures.push(...shellFailures(run));
        console.log(`[home-phone-layout] ${engineId}/${profile.id} ${summarize(run)}`);
      }
    } finally {
      await browser.close().catch(() => {});
    }
  }
  assert(
    report.runs.length > 0,
    `home-phone-layout-smoke ran no engine; requested ${requestedEngines.join(",")} but none is installed under elastos/tools/browser-playwright-engine`,
  );
  assert(
    !requestedEngines.includes("chromium") || report.runs.some((run) => run.engine === "chromium"),
    "home-phone-layout-smoke needs the chromium engine from elastos/tools/browser-playwright-engine",
  );
  report.capsuleErrors = capsuleErrors;
  report.consoleErrors = consoleErrors;
  writeFileSync(join(outputRoot, "report.json"), JSON.stringify(report, null, 2));
  assert(pageErrors.length === 0, "Home/GUI source logged errors under the phone fixture", pageErrors);
  assert(state.errors.length === 0, "fixture server recorded errors", state.errors);
  assert(failures.length === 0, "phone layout regressed past its baseline", failures);
  const source = report.source;
  console.log(
    `home-phone-layout-smoke: PASS engines=${report.runs.map((run) => run.engine).filter((value, index, all) => all.indexOf(value) === index).join("+")} ` +
      `viewport_fit=${source.viewportFitCover.home && source.viewportFitCover.gui ? "cover" : "none"} ` +
      `bare_100vh=${source.bareViewportHeightUnits} backdrop_filters=${source.backdropFilters} ` +
      `${skippedEngines.length ? `skipped=${skippedEngines.join(",")} (not installed) ` : ""}report=${join(outputRoot, "report.json")}`,
  );
} finally {
  await new Promise((resolveClose) => server.close(resolveClose));
}
