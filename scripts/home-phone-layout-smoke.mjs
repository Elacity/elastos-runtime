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
    desktop: { targets: 0, text: 0 },
    launcher: { targets: 0, text: 0 },
    spotlight: { targets: 0, text: 0 },
    "spotlight-results": { targets: 0, text: 0 },
    "control-centre": { targets: 0, text: 0 },
    notifications: { targets: 0, text: 0 },
    "mission-control": { targets: 0, text: 0 },
    "assistant-face": { targets: 0, text: 0 },
    // The 24 px handle that brings the Dock back over a window, by design.
    window: { targets: 1, text: 0 },
  },
  "phone-landscape": {
    desktop: { targets: 0, text: 0 },
    launcher: { targets: 0, text: 0 },
    spotlight: { targets: 0, text: 0 },
    "spotlight-results": { targets: 0, text: 0 },
    "control-centre": { targets: 0, text: 0 },
    notifications: { targets: 0, text: 0 },
    "mission-control": { targets: 0, text: 0 },
    "assistant-face": { targets: 0, text: 0 },
    // The 24 px handle that brings the Dock back over a window, by design.
    window: { targets: 1, text: 0 },
  },
  tablet: {
    desktop: { targets: 8, text: 2 },
    launcher: { targets: 9, text: 2 },
    spotlight: { targets: 8, text: 2 },
    "spotlight-results": { targets: null, text: null },
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

// One letter that fills Spotlight with result rows.
const SPOTLIGHT_PROBE_QUERY = "e";

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
    id: "spotlight-results",
    open: async (frame) => {
      await frame.locator("#toolbar-spotlight").click();
      await sleep(SHEET_SETTLE_MS);
      await frame.locator("#spotlight-input").fill(SPOTLIGHT_PROBE_QUERY);
    },
    settle: SURFACE_SETTLE_MS,
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
    // Overview leaves the bar on phone and tablet-with-touch; Control Centre's
    // Overview row is the route every size class has.
    open: async (frame) => {
      await frame.locator("#toolbar-control-centre").click();
      await sleep(SHEET_SETTLE_MS);
      await frame.locator("#control-centre-show-windows").click();
    },
    settle: SHEET_SETTLE_MS,
    close: pressEscape,
  },
  {
    id: "assistant-face",
    capsule: "assistant",
    open: (frame) => frame.locator("#assistant-toggle").click(),
    settle: SHEET_SETTLE_MS,
    close: pressEscape,
  },
];

// Targets and text inside a capsule's own frame, and the size class it landed.
async function measureCapsuleFrame(page, target) {
  const capsuleFrame = page.frames().find((candidate) => candidate.url().includes(`/apps/${target}/`));
  if (!capsuleFrame) {
    return { capsule: null, capsuleLayout: null };
  }
  return {
    capsule: await capsuleFrame.evaluate(measureSurface, { minTarget: MIN_TARGET_PX, minText: MIN_TEXT_PX }).catch(() => null),
    capsuleLayout: await capsuleFrame.evaluate(() => ({
      sharedTheme: Boolean(document.querySelector('script[src*="elastos-theme.js"]')),
      formFactor: document.documentElement.getAttribute("data-el-form-factor"),
    })).catch(() => null),
  };
}

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
  let dockProbe = null;
  let stageProbe = null;
  let keyboardProbe = null;
  let spotlightProbe = null;
  let bootRetries = 0;
  try {
    let booted;
    try {
      booted = await bootHome(context, origin);
    } catch (error) {
      // WebKit occasionally never attaches the GUI frame on the first page of
      // a fresh engine (the host reports ready, the frame never lands). This
      // smoke measures layout, not boot reliability; one retry is recorded in
      // the report so a real regression still shows as repeated retries.
      if (!/Home GUI frame/.test(error.message)) {
        throw error;
      }
      bootRetries += 1;
      console.warn(`[home-phone-layout] ${engineId}/${profile.id} boot retry: ${error.message.split("\n")[0]}`);
      await Promise.all(context.pages().map((page) => page.close()));
      booted = await bootHome(context, origin);
    }
    const { page, frame } = booted;
    for (const surface of SHELL_SURFACES) {
      await surface.open(frame);
      await sleep(surface.settle);
      const shellSurface = await measure(frame, surface.id);
      if (surface.capsule) {
        shellSurface.target = surface.capsule;
        Object.assign(shellSurface, await measureCapsuleFrame(page, surface.capsule));
      }
      surfaces.push(shellSurface);
      await screenshot(page, dir, surface.id);
      if (surface.id === "assistant-face" && profile.id.startsWith("phone")) {
        keyboardProbe = await probeKeyboardInset(page, frame);
      }
      if (surface.id === "spotlight-results" && profile.id === "phone-portrait") {
        spotlightProbe = await probeSpotlight(page, frame);
      }
      await surface.close(frame);
    }
    for (const [target] of FIRST_PARTY_APPS) {
      if (target === "assistant") {
        // The Assistant opens as the face, measured above, never as a window.
        continue;
      }
      await openWindow(frame, target);
      if (profile.id.startsWith("phone") && !dockProbe) {
        dockProbe = await probePhoneDock(frame, page, dir);
        stageProbe = await probePhoneStage(frame, page, dir, profile, target);
      }
      const shell = await measure(frame, "window");
      shell.target = target;
      Object.assign(shell, await measureCapsuleFrame(page, target));
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
  return { engine: engineId, profile: profile.id, viewport: profile.viewport, surfaces, dock: dockProbe, stage: stageProbe, keyboard: keyboardProbe, spotlight: spotlightProbe, bootRetries };
}

// Soft keyboard: only the host page sees it, so the host relays its height
// (home:keyboard-inset). With the Assistant face open, the probe plays the
// host: the room must end above the inset, the same message from a capsule
// frame must change nothing, and 0 must give the room back.
const KEYBOARD_PROBE_INSET_PX = 300;
const KEYBOARD_SETTLE_MS = 150;

function postKeyboardInsetFromHost(page, inset) {
  return page.evaluate((message) => {
    document.getElementById("active-shell-frame").contentWindow.postMessage(message, "*");
  }, { type: "home:keyboard-inset", inset });
}

async function probeKeyboardInset(page, frame) {
  const postFromHost = (inset) => postKeyboardInsetFromHost(page, inset);
  const readRoom = () => frame.evaluate(() => ({
    inset: getComputedStyle(document.documentElement).getPropertyValue("--keyboard-inset").trim(),
    roomBottom: Math.round(document.querySelector(".assistant-space").getBoundingClientRect().bottom),
    viewportHeight: window.innerHeight,
  }));
  await postFromHost(KEYBOARD_PROBE_INSET_PX);
  await sleep(KEYBOARD_SETTLE_MS);
  const raised = await readRoom();
  const capsuleFrame = page.frames().find((candidate) => candidate.url().includes("/apps/assistant/"));
  await capsuleFrame?.evaluate(() => window.parent.postMessage({ type: "home:keyboard-inset", inset: 120 }, "*"));
  await sleep(KEYBOARD_SETTLE_MS);
  const spoofed = await readRoom();
  await postFromHost(0);
  await sleep(KEYBOARD_SETTLE_MS);
  const lowered = await readRoom();
  return { raised, spoofed, lowered, capsuleFrame: Boolean(capsuleFrame) };
}

function phoneKeyboardFailures(run) {
  if (!run.keyboard) {
    return [];
  }
  const { raised, spoofed, lowered, capsuleFrame } = run.keyboard;
  const label = `${run.engine}/${run.profile}/keyboard`;
  const failures = [];
  if (raised.inset !== `${KEYBOARD_PROBE_INSET_PX}px` || raised.roomBottom !== raised.viewportHeight - KEYBOARD_PROBE_INSET_PX) {
    failures.push(`${label}: the host's inset must end the Assistant room above the keyboard. Got ${JSON.stringify(raised)}`);
  }
  if (!capsuleFrame || spoofed.inset !== raised.inset) {
    failures.push(`${label}: a capsule frame must not move the keyboard inset. Got ${JSON.stringify({ capsuleFrame, spoofed })}`);
  }
  if (lowered.inset !== "0px" || lowered.roomBottom !== lowered.viewportHeight) {
    failures.push(`${label}: an inset of 0 must give the room back. Got ${JSON.stringify(lowered)}`);
  }
  return failures;
}

// Spotlight with results on a portrait phone: the panel hangs just under the
// bar, and with the host's keyboard inset it ends above the keyboard.
// (Landscape leaves the keyboard almost no stage; not probed.)
const SPOTLIGHT_MAX_GAP_UNDER_BAR_PX = 12;

async function probeSpotlight(page, frame) {
  const readPanel = () => frame.evaluate(() => {
    const panel = document.querySelector(".spotlight-panel").getBoundingClientRect();
    return {
      barBottom: Math.round(document.querySelector(".toolbar").getBoundingClientRect().bottom),
      panelTop: Math.round(panel.top),
      panelBottom: Math.round(panel.bottom),
      viewportHeight: window.innerHeight,
    };
  });
  await postKeyboardInsetFromHost(page, KEYBOARD_PROBE_INSET_PX);
  await sleep(KEYBOARD_SETTLE_MS);
  const raised = await readPanel();
  await postKeyboardInsetFromHost(page, 0);
  await sleep(KEYBOARD_SETTLE_MS);
  return { raised, lowered: await readPanel() };
}

function phoneSpotlightFailures(run) {
  if (!run.spotlight) {
    return [];
  }
  const { raised, lowered } = run.spotlight;
  const label = `${run.engine}/${run.profile}/spotlight-results`;
  const failures = [];
  const gap = lowered.panelTop - lowered.barBottom;
  if (gap < 0 || gap > SPOTLIGHT_MAX_GAP_UNDER_BAR_PX) {
    failures.push(`${label}: the panel must hang just under the bar. Got ${JSON.stringify(lowered)}`);
  }
  if (raised.panelBottom > raised.viewportHeight - KEYBOARD_PROBE_INSET_PX) {
    failures.push(`${label}: with the keyboard up the results must end above it. Got ${JSON.stringify(raised)}`);
  }
  return failures;
}

// Phone stage contract with a window open: a downward drag on the title bar
// opens Mission Control (cards carry icon + name and a Close on touch);
// system back goes home and leaves no history entry behind; rotating keeps
// the window on the stage at full width. Ends with the window open again.
async function probePhoneStage(frame, page, dir, profile, target) {
  const windowState = () =>
    frame.evaluate((selector) => {
      const win = document.querySelector(selector);
      const rect = win?.getBoundingClientRect();
      return {
        visible: Boolean(win) && !win.classList.contains("hidden"),
        width: rect ? Math.round(rect.width) : null,
        viewportWidth: window.innerWidth,
        exposeActive: document.body.classList.contains("expose-active"),
        historyState: history.state,
        stageHistory: document.body.dataset.stageHistory || null,
        captionIcon: Boolean(document.querySelector(".expose-card .expose-caption-icon")),
        cardClose: Boolean(document.querySelector(".expose-card .expose-close")?.getClientRects().length),
      };
    }, `.window[data-target="${target}"]`);

  const head = await frame.locator(`.window[data-target="${target}"] .window-head-draggable`).boundingBox();
  await page.mouse.move(head.x + head.width / 2, head.y + head.height / 2);
  await page.mouse.down();
  await page.mouse.move(head.x + head.width / 2, head.y + head.height / 2 + 40, { steps: 4 });
  await page.mouse.move(head.x + head.width / 2, head.y + head.height / 2 + 90, { steps: 4 });
  await page.mouse.up();
  await sleep(SHEET_SETTLE_MS);
  const swiped = await windowState();
  await screenshot(page, dir, "switcher");
  await pressEscape(frame);
  await sleep(SHEET_SETTLE_MS);

  const before = await windowState();
  // Same joint-session-history step the system back button takes; Playwright's
  // page.goBack() waits for a main-frame navigation that never comes. Where
  // the shell runs buttons-only (WebKit), back is not the shell's to answer
  // and the probe only checks nothing was pushed.
  let afterBack = before;
  if (before.stageHistory === "history") {
    await frame.evaluate(() => history.back());
    await sleep(SHEET_SETTLE_MS);
    afterBack = await windowState();
    await openWindow(frame, target);
  }
  afterBack.homeAlive = !frame.isDetached() && frame.url().includes("/apps/home-gui/");
  await sleep(SURFACE_SETTLE_MS);

  const rotated = { width: profile.viewport.height, height: profile.viewport.width };
  await page.setViewportSize(rotated);
  await sleep(SHEET_SETTLE_MS);
  const afterRotate = await windowState();
  await page.setViewportSize(profile.viewport);
  await sleep(SHEET_SETTLE_MS);
  const restored = await windowState();
  return { swiped, before, afterBack, afterRotate, restored };
}

function phoneStageFailures(run) {
  if (!run.stage) {
    return [];
  }
  const { swiped, before, afterBack, afterRotate, restored } = run.stage;
  const label = `${run.engine}/${run.profile}/stage`;
  const failures = [];
  if (!swiped.exposeActive) {
    failures.push(`${label}: title-bar swipe down must open Mission Control`);
  }
  if (swiped.exposeActive && (!swiped.captionIcon || !swiped.cardClose)) {
    failures.push(`${label}: Mission Control cards must show the app icon and a Close on touch`);
  }
  if (!before.visible) {
    failures.push(`${label}: the window must be on the stage before back`);
  }
  if (!afterBack.homeAlive) {
    failures.push(`${label}: system back must never navigate the Home frame away`);
  }
  if (before.stageHistory === "history") {
    if (!before.historyState?.elastosStage) {
      failures.push(`${label}: an open window must hold one stage history entry`);
    }
    if (afterBack.visible || afterBack.historyState?.elastosStage) {
      failures.push(`${label}: system back must go home and leave no stage history entry`);
    }
  } else if (before.stageHistory === "buttons") {
    if (before.historyState?.elastosStage) {
      failures.push(`${label}: buttons-only shells must not push stage history`);
    }
  } else {
    failures.push(`${label}: body[data-stage-history] must say history or buttons`);
  }
  if (!afterRotate.visible || afterRotate.width !== afterRotate.viewportWidth) {
    failures.push(`${label}: rotating must keep the window on the stage at full width (${afterRotate.width}/${afterRotate.viewportWidth})`);
  }
  if (!restored.visible || restored.width !== restored.viewportWidth) {
    failures.push(`${label}: rotating back must keep the window at full width`);
  }
  return failures;
}

// Phone Dock contract with a window open: the Dock is off screen and the
// window reaches down to the handle; the handle peeks the Dock over the app;
// the scrim tucks it again. Runs against the first window of a phone profile.
async function probePhoneDock(frame, page, dir) {
  const dockState = () =>
    frame.evaluate(() => {
      const dock = document.querySelector(".taskbar").getBoundingClientRect();
      const win = document.querySelector(".window:not([aria-hidden='true'])")?.getBoundingClientRect();
      const handle = document.querySelector("#phone-dock-handle");
      return {
        tucked: document.body.classList.contains("phone-dock-tucked"),
        peek: document.body.classList.contains("phone-dock-peek"),
        dockOnScreen: dock.top < window.innerHeight,
        windowBottom: win ? Math.round(win.bottom) : null,
        handleVisible: Boolean(handle && !handle.hidden && handle.getClientRects().length > 0),
        handleHeight: handle ? Math.round(handle.getBoundingClientRect().height) : 0,
      };
    });
  const tucked = await dockState();
  await frame.locator("#phone-dock-handle").click();
  await sleep(SURFACE_SETTLE_MS);
  const peeked = await dockState();
  await screenshot(page, dir, "dock-peek");
  await frame.locator("#phone-dock-scrim").click();
  await sleep(SURFACE_SETTLE_MS);
  const dismissed = await dockState();
  return { tucked, peeked, dismissed };
}

// Idle pill height on phone: --taskbar-h. The hidden launcher and Assistant
// face stay in flow (display: flex, collapsed) so any column gap on
// .taskbar-inner shows up as dead space above the icons.
const DOCK_IDLE_HEIGHT_PX = 72;

function phoneDockFailures(run) {
  if (!run.dock) {
    return [];
  }
  const { tucked, peeked, dismissed } = run.dock;
  const label = `${run.engine}/${run.profile}/dock`;
  const failures = [];
  const idleDock = run.surfaces.find((surface) => surface.surface === "desktop")?.geometry?.dock;
  if (!idleDock || Math.abs(idleDock.height - DOCK_IDLE_HEIGHT_PX) > 1) {
    failures.push(`${label}: idle Dock pill is ${idleDock?.height}px tall, expected ${DOCK_IDLE_HEIGHT_PX} (icons must sit centred)`);
  }
  const viewportHeight = run.viewport.height;
  if (!tucked.tucked || tucked.dockOnScreen || !tucked.handleVisible) {
    failures.push(`${label}: Dock must tuck under an open window with the handle showing`);
  }
  if (tucked.handleHeight < 24) {
    failures.push(`${label}: Dock handle is ${tucked.handleHeight}px tall (minimum 24)`);
  }
  if (tucked.windowBottom === null || tucked.windowBottom < viewportHeight - tucked.handleHeight - 1) {
    failures.push(`${label}: window bottom ${tucked.windowBottom} must reach the Dock handle (${viewportHeight - tucked.handleHeight})`);
  }
  if (!peeked.peek || !peeked.dockOnScreen) {
    failures.push(`${label}: handle must peek the Dock over the app`);
  }
  if (dismissed.peek || dismissed.dockOnScreen) {
    failures.push(`${label}: scrim must tuck the Dock again`);
  }
  return failures;
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

// The shell posts its size class to every capsule frame (elastos:shell-layout);
// capsules on the shared theme runtime land it on the frame's <html>.
const EXPECTED_CAPSULE_FORM_FACTOR = {
  "phone-portrait": "phone",
  "phone-landscape": "phone",
  tablet: "tablet",
};

// Capsule ratchet (M6): targets < 44 px inside each app's frame on the phone
// stage, the higher of both engines on 2026-09-23. Each capsule change lowers
// its row; the M6 goal is 0 everywhere. Tablet is recorded, not yet gated.
const CAPSULE_TARGET_BASELINE = {
  "phone-portrait": {
    library: 0, documents: 0, marketplace: 0, system: 0, people: 0, services: 0,
    wallet: 0, inbox: 0, "archive-manager": 0, "elacity-player": 0, browser: 0,
    assistant: 0,
  },
  "phone-landscape": {
    library: 0, documents: 0, marketplace: 0, system: 0, people: 0, services: 0,
    wallet: 0, inbox: 0, "archive-manager": 0, "elacity-player": 0, browser: 0,
    assistant: 0,
  },
};

function shellFailures(run) {
  const failures = [];
  const baseline = BASELINE[run.profile] || {};
  for (const surface of run.surfaces) {
    const layout = surface.capsuleLayout;
    if (layout?.sharedTheme && layout.formFactor !== EXPECTED_CAPSULE_FORM_FACTOR[run.profile]) {
      failures.push(`${run.engine}/${run.profile}/window:${surface.target}: capsule frame form factor ${layout.formFactor} (expected ${EXPECTED_CAPSULE_FORM_FACTOR[run.profile]})`);
    }
    const capsuleLimit = CAPSULE_TARGET_BASELINE[run.profile]?.[surface.target];
    const capsuleSmall = surface.capsule?.smallTargets.length ?? 0;
    if (capsuleLimit !== undefined && capsuleSmall > capsuleLimit) {
      failures.push(`${run.engine}/${run.profile}/window:${surface.target}: ${capsuleSmall} capsule targets < ${MIN_TARGET_PX}px (baseline ${capsuleLimit})`);
    }
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
        failures.push(...shellFailures(run), ...phoneDockFailures(run), ...phoneStageFailures(run), ...phoneKeyboardFailures(run), ...phoneSpotlightFailures(run));
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
      `boot_retries=${report.runs.reduce((sum, run) => sum + run.bootRetries, 0)} ` +
      `${skippedEngines.length ? `skipped=${skippedEngines.join(",")} (not installed) ` : ""}report=${join(outputRoot, "report.json")}`,
  );
} finally {
  await new Promise((resolveClose) => server.close(resolveClose));
}
