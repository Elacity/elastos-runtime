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
// Source truths that do not need a browser (the base viewport meta, bare
// 100vh) are asserted here too so the report is one document. The base meta
// stays `width=device-width, initial-scale=1`; on a phone the host rewrites it
// to add viewport-fit=cover, which each profile checks in the browser.

import { createServer } from "node:http";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { extname, isAbsolute, join, relative, resolve, sep } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

import { LONG_PRESS_MS } from "../capsules/home-gui/browser/shell-touch.js";

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
// The viewport meta every page ships, and what the host adds on a phone only.
const BASE_VIEWPORT_CONTENT = "width=device-width, initial-scale=1";
const PHONE_VIEWPORT_FIT = "viewport-fit=cover";

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
    "context-menu": { targets: 0, text: 0 },
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
    "context-menu": { targets: 0, text: 0 },
    spotlight: { targets: 0, text: 0 },
    "spotlight-results": { targets: 0, text: 0 },
    "control-centre": { targets: 0, text: 0 },
    notifications: { targets: 0, text: 0 },
    "mission-control": { targets: 0, text: 0 },
    "assistant-face": { targets: 0, text: 0 },
    // The 24 px handle that brings the Dock back over a window, by design.
    window: { targets: 1, text: 0 },
  },
  // A tablet keeps the desktop shell byte-for-byte (phone-only touch
  // layout), so its targets are the desktop's: recorded, not gated. Text
  // stays gated.
  tablet: {
    desktop: { targets: null, text: 2 },
    launcher: { targets: null, text: 2 },
    "context-menu": { targets: null, text: null },
    spotlight: { targets: null, text: 2 },
    "spotlight-results": { targets: null, text: null },
    "control-centre": { targets: null, text: 8 },
    notifications: { targets: null, text: 42 },
    "mission-control": { targets: null, text: 1 },
    "assistant-face": { targets: null, text: 1 },
    window: { targets: null, text: 2 },
  },
};

const state = { errors: [] };
// The healthy fixture holds the event stream open. The closed-stream run ends
// it at once and holds summary replies, so a reconnect's summary refresh can
// land while a finger holds a Dock tile.
const fixture = { closeEventStream: false, holdSummaries: false, heldSummaries: [], summaryReplies: 0, libraryRoots: false };
const LIBRARY_FIXTURE_ROOTS = Object.freeze([
  { id: "documents", uri: "localhost://Users/fixture/Documents", name: "Documents", label: "Documents" },
  { id: "downloads", uri: "localhost://Users/fixture/Downloads", name: "Downloads", label: "Downloads" },
]);

function releaseHeldSummaries() {
  fixture.holdSummaries = false;
  for (const reply of fixture.heldSummaries.splice(0)) {
    reply();
  }
}
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
    const reply = () => {
      fixture.summaryReplies += 1;
      json(res, 200, homeSummary());
    };
    if (fixture.holdSummaries) {
      fixture.heldSummaries.push(reply);
    } else {
      reply();
    }
    return true;
  }
  if (url.pathname === "/api/apps/home/runtime/ensure" && req.method === "POST") {
    json(res, 200, { ready: true });
    return true;
  }
  if (url.pathname === "/api/apps/home/events/stream" && req.method === "GET") {
    // A Home holds its event stream open. A stream that ends at once sends the
    // host into its poll-and-reopen recovery, and every summary it refreshes
    // rebuilds the Dock under a held finger.
    res.writeHead(200, {
      "access-control-allow-origin": "*",
      "cache-control": "no-store",
      "content-type": "text/event-stream",
    });
    if (fixture.closeEventStream) {
      res.end();
    } else {
      res.write(": open\n\n");
    }
    return true;
  }
  if (url.pathname === "/api/apps/home/events" && req.method === "GET") {
    json(res, 200, { schema: "elastos.home.events/v1", events: [], cursor: "" });
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
  // Only for the Back probes: with roots, Library reaches the point where it
  // would add history entries of its own; a second root is a second folder
  // to move to.
  if (fixture.libraryRoots && url.pathname === "/api/provider/object/roots" && req.method === "POST") {
    await readBody(req);
    json(res, 200, { status: "ok", data: { roots: LIBRARY_FIXTURE_ROOTS } });
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

// Longer than shell-touch.js LONG_PRESS_MS (500).
const LONG_PRESS_HOLD_MS = 700;

// Playwright cannot hold a finger down, so this plays the browser: a touch
// pointerdown on the first visible match, then (on release) the pointerup and
// the click a release produces. A touch pointer is captured by the pressed
// tile, so the release goes to that tile, or to the element under the finger
// when a redraw removed it. Records when the pressed tile left the page.
// Returns the app the icon opens.
async function pressApp(frame, selector) {
  return frame.evaluate((selector) => {
    const tile = Array.from(document.querySelectorAll(selector)).find((node) => {
      if (node.id === "launcher-toggle" || node.id === "assistant-toggle") return false;
      const box = node.getBoundingClientRect();
      return box.width > 0 && box.left >= 0 && box.right <= window.innerWidth;
    });
    const box = tile.getBoundingClientRect();
    const init = {
      bubbles: true,
      cancelable: true,
      composed: true,
      pointerId: 41,
      pointerType: "touch",
      isPrimary: true,
      clientX: box.x + box.width / 2,
      clientY: box.y + box.height / 2,
    };
    const pressedAt = performance.now();
    window.__smokeTileRemovedAfterMs = null;
    const removal = new MutationObserver(() => {
      if (!tile.isConnected && window.__smokeTileRemovedAfterMs === null) {
        window.__smokeTileRemovedAfterMs = Math.round(performance.now() - pressedAt);
      }
    });
    removal.observe(document.body, { childList: true, subtree: true });
    tile.dispatchEvent(new PointerEvent("pointerdown", init));
    window.__smokeReleaseLongPress = () => {
      removal.disconnect();
      const releasedOn = tile.isConnected ? tile : document.elementFromPoint(init.clientX, init.clientY);
      releasedOn?.dispatchEvent(new PointerEvent("pointerup", init));
      releasedOn?.dispatchEvent(new MouseEvent("click", init));
    };
    return tile.dataset.target;
  }, selector);
}

async function longPressApp(frame, selector) {
  const target = await pressApp(frame, selector);
  await sleep(LONG_PRESS_HOLD_MS);
  await frame.evaluate(() => window.__smokeReleaseLongPress());
  return target;
}

const longPressDockApp = (frame) => longPressApp(frame, ".taskbar-item[data-target]");

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
    // The phone Home is the app grid; the Apps sheet exists off the phone.
    phone: false,
    open: (frame) => frame.locator("#launcher-toggle").click(),
    settle: SHEET_SETTLE_MS,
    close: pressEscape,
  },
  {
    id: "context-menu",
    open: longPressDockApp,
    settle: SURFACE_SETTLE_MS,
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

// The Dock tile, else the phone Home grid icon, else the launcher card.
async function openWindow(frame, target) {
  const dockButton = frame.locator(`.taskbar-item[data-target="${target}"]:visible`);
  const homeButton = frame.locator(`.phone-home-app[data-target="${target}"]:visible`);
  if (await dockButton.count()) {
    await dockButton.first().click();
  } else if (await homeButton.count()) {
    await homeButton.first().click();
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

// A real tap on an app, then a real tap on Close while the open ghost-close
// guard is still armed: a fresh touch elsewhere is not a ghost click, so the
// window must close at once (a touch tap carries no pointermove).
async function probePromptTouchClose(frame, target = "library") {
  await frame.locator(`.taskbar-item[data-target="${target}"]:visible, .phone-home-app[data-target="${target}"]:visible`).first().tap();
  await waitFor(
    () => frame.locator(`.window[data-target="${target}"] [data-action="close"]`).count().then((count) => count > 0),
    BOOT_TIMEOUT_MS,
    `${target} window`,
  );
  // Sample the guard at the Close press itself, not before the tap is issued.
  await frame.evaluate((target) => {
    window.__promptCloseGuardMs = null;
    const node = document.querySelector(`.window[data-target="${target}"]`);
    node?.querySelector('[data-action="close"]')?.addEventListener("pointerdown", () => {
      window.__promptCloseGuardMs = Math.round(Number(node.dataset.closeGuardUntil) - performance.now());
    }, { once: true, capture: true });
  }, target);
  await frame.locator(`.window[data-target="${target}"] [data-action="close"]`).first().tap();
  const guardLeftMs = await frame.evaluate(() => window.__promptCloseGuardMs ?? 0);
  let closed = true;
  try {
    await waitFor(
      () => frame.locator(`.window[data-target="${target}"]`).count().then((count) => count === 0),
      SHEET_SETTLE_MS,
      `${target} closed by a prompt tap`,
    );
  } catch (_error) {
    closed = false;
    await closeWindow(frame, target);
  }
  await sleep(SURFACE_SETTLE_MS);
  return { target, guardLeftMs, closed };
}

function promptCloseFailures(run) {
  const probe = run.promptClose;
  if (!probe) {
    return [];
  }
  const label = `${run.engine}/${run.profile}/prompt-close`;
  if (probe.guardLeftMs <= 0) {
    return [`${label}: the Close tap must land while the open guard is armed to test it. Got ${JSON.stringify(probe)}`];
  }
  return probe.closed ? [] : [`${label}: a fresh tap on Close right after a tap opened ${probe.target} must close it. Got ${JSON.stringify(probe)}`];
}

// One Back as the system button takes it: a joint-session-history step.
function historyBack(frame) {
  return frame.evaluate(() => history.back());
}

// An app window as Back sees it, and the stage around it.
function stageWindowState(frame, target) {
  return frame.evaluate(async (target) => {
    const stage = await import(new URL("./shell-phone-stage.js?v=home-20260813a", location.href).href);
    const node = document.querySelector(`.window[data-target="${target}"]`);
    return {
      home: location.href.includes("/apps/home-gui/"),
      formFactor: document.body.dataset.formFactor || null,
      present: Boolean(node),
      onStage: Boolean(node && !node.classList.contains("hidden") && node.dataset.spaceVisible !== "false"),
      fullscreenWindows: document.querySelectorAll('.window[data-fullscreen-stage="true"]').length,
      openLayers: stage.openStageLayers(document).map((entry) => entry.id),
      layerCount: typeof stage.stageLayerCount === "function" ? stage.stageLayerCount(document) : null,
    };
  }, target);
}

// The top-level page is still Home and its shell frame is still attached.
function homeState(page, frame) {
  return { url: page.url(), shellFrame: !frame.isDetached() && homeGuiFrame(page) === frame };
}

// Closes every window of an app through the shell, wherever it is.
function discardWindows(frame, target) {
  return frame.evaluate(async (target) => {
    const core = await import(new URL("./shell-core.js?v=home-20260813a", location.href).href);
    const windows = await import(new URL("./shell-windows.js?v=home-20260813a", location.href).href);
    for (const entry of [...core.shellState.windows.values()]) {
      if (entry.targetId === target) {
        await windows.closeWindow(entry.id);
      }
    }
  }, target);
}

// Back races on the history-driven stage (Chromium; WebKit uses buttons).
// libraryOwnHistory: an app frame adds no history entry of its own on a phone,
//   so one Back sends Library home instead of stepping Library's history.
// reopenAfterClose: an app closed by hand then reopened; one Back sends it home
//   and Home stays.
// reopenedLayers: Control Centre and Spotlight, whose closes animate (Spotlight
//   animates its inner panel, not the registered #spotlight), reopened over an
//   app in the same task as the Back that started closing them (before the
//   stage's settlement poll) are new layers: each stays open, the next Back
//   closes it and the app stays on the stage.
const REOPEN_RACES = Object.freeze([
  { id: "control-centre", module: "shell-control-centre.js", open: "showControlCentre", leaving: "#control-centre", leavingClass: "menubar-card-leaving" },
  { id: "spotlight", module: "shell-spotlight.js", open: "showSpotlight", leaving: "#spotlight .spotlight-panel", leavingClass: "launcher-leaving" },
]);

async function probeBackRaces(page, frame) {
  const mode = await frame.evaluate(() => document.body.dataset.stageHistory || "");
  if (mode !== "history") {
    return null;
  }

  fixture.libraryRoots = true;
  let libraryOwnHistory;
  try {
    await openTarget(frame, "library");
    await waitFor(
      () => Promise.resolve(page.frames().find((f) => f.url().includes("/apps/library/")))
        .then((f) => f?.evaluate(() => document.documentElement.dataset.elFormFactor === "phone" && !document.querySelector(".library-shell.hidden, #library-shell.hidden")).catch(() => false)),
      BOOT_TIMEOUT_MS,
      "Library on the phone stage",
    );
    await sleep(SHEET_SETTLE_MS);
    const libraryFrame = page.frames().find((f) => f.url().includes("/apps/library/"));
    const libraryState = await libraryFrame.evaluate(() => history.state?.schema || null);
    await historyBack(frame);
    await sleep(SHEET_SETTLE_MS);
    libraryOwnHistory = { libraryState, after: await stageWindowState(frame, "library") };
  } finally {
    fixture.libraryRoots = false;
  }
  await discardWindows(frame, "library");
  await sleep(SURFACE_SETTLE_MS);

  await openTarget(frame, "system");
  await closeWindow(frame, "system");
  await openTarget(frame, "system");
  await historyBack(frame);
  await sleep(SHEET_SETTLE_MS);
  const reopenAfterClose = await stageWindowState(frame, "system");
  await discardWindows(frame, "system");
  await sleep(SURFACE_SETTLE_MS);

  // Reduced motion makes every close synchronous (shell-motion.js); the race
  // needs the leave animations, so motion is on for this step.
  const homeUrl = page.url();
  await page.emulateMedia({ reducedMotion: "no-preference" });
  const reopenedLayers = [];
  try {
    await openTarget(frame, "system");
    for (const spec of REOPEN_RACES) {
      await LAYER_OPENERS[spec.id].open(frame);
      await sleep(SHEET_SETTLE_MS);
      const race = await frame.evaluate(async ({ spec, timeoutMs }) => {
        const owner = await import(new URL(`./${spec.module}?v=home-20260813a`, location.href).href);
        const leaving = document.querySelector(spec.leaving);
        return new Promise((resolve) => {
          // Added after the shell's own popstate listener, so the shell's Back
          // has started the close when this runs; reopen in that same task.
          const reopen = () => {
            clearTimeout(timer);
            const closing = Boolean(leaving?.classList.contains(spec.leavingClass));
            owner[spec.open]();
            resolve({ popstate: true, closing, reopened: Boolean(leaving && !leaving.classList.contains(spec.leavingClass)) });
          };
          const timer = setTimeout(() => {
            window.removeEventListener("popstate", reopen);
            resolve({ popstate: false });
          }, timeoutMs);
          window.addEventListener("popstate", reopen, { once: true });
          history.back();
        });
      }, { spec, timeoutMs: LAYER_CLOSE_WAIT_MS });
      // Past the leave animation's own timer: a cancelled close must not land.
      await sleep(SHEET_SETTLE_MS);
      const before = await readLayer(frame, spec.id);
      await historyBack(frame);
      const after = await waitLayerClosed(frame, spec.id);
      const app = await stageWindowState(frame, "system");
      reopenedLayers.push({ id: spec.id, race, before, after, app, homeUrl, home: homeState(page, frame) });
    }
  } finally {
    await page.emulateMedia({ reducedMotion: "reduce" });
  }
  await discardWindows(frame, "system");
  await sleep(SURFACE_SETTLE_MS);

  return { libraryOwnHistory, reopenAfterClose, reopenedLayers };
}

function backRaceFailures(run) {
  const probe = run.backRaces;
  if (!probe) {
    return [];
  }
  const label = `${run.engine}/${run.profile}/back-races`;
  const failures = [];
  const { libraryOwnHistory, reopenAfterClose, reopenedLayers } = probe;
  if (libraryOwnHistory.libraryState || !libraryOwnHistory.after.home || libraryOwnHistory.after.onStage) {
    failures.push(`${label}: Library must add no history entry on a phone, and one Back must send it home. Got ${JSON.stringify(libraryOwnHistory)}`);
  }
  if (!reopenAfterClose.home || reopenAfterClose.onStage) {
    failures.push(`${label}: an app closed by hand and reopened must go home on one Back, Home staying. Got ${JSON.stringify(reopenAfterClose)}`);
  }
  for (const reopened of reopenedLayers) {
    if (!reopened.race.popstate || !reopened.race.closing) {
      failures.push(`${label}: ${reopened.id} must be reopened while Back's animated close is still in flight to test it. Got ${JSON.stringify(reopened)}`);
    } else if (!reopened.race.reopened || reopened.before.state !== "open" || !reopened.before.openLayers.includes(reopened.id)) {
      failures.push(`${label}: ${reopened.id} reopened during Back's close must stay open as a new layer. Got ${JSON.stringify(reopened)}`);
    } else if (reopened.after.open || !reopened.app.onStage || !reopened.after.home || reopened.home.url !== reopened.homeUrl || !reopened.home.shellFrame) {
      failures.push(`${label}: ${reopened.id} reopened during Back's close must close on the next Back, the app beneath staying on the stage and Home staying. Got ${JSON.stringify(reopened)}`);
    }
  }
  return failures;
}

// Form-factor change: the shell decides phone, tablet or desktop at load, and
// a resize or rotation across the phone boundary runs one clean stage reset
// (body[data-stage-reset="running"] in the Home GUI frame) before new input.
// Played on a fine-pointer window, as a desktop browser resized narrow.
// fullscreenToPhone: a fullscreen Space entered with the green control leaves
//   fullscreen when the window becomes a phone; the app stays on the stage and
//   one Back sends it home, Home staying.
// libraryAcrossStages: Library opened on a phone moves to another folder on the
//   desktop (its own history entries there); back on the phone with Control
//   Centre over it, each Back closes exactly the top layer, Library keeps its
//   folder and Home stays.
// The host page's viewport meta gains viewport-fit=cover on the phone and
// returns to the base content off it.
const STAGE_RESET_DESKTOP = Object.freeze({ width: 1280, height: 800 });
const STAGE_RESET_PHONE = PROFILES.find((profile) => profile.id === "phone-portrait").viewport;
// Each open layer's close and each history traversal in a reset is bounded by
// the shell (shell-phone-stage.js LAYER_CLOSE_TIMEOUT_MS, RESET_HISTORY_TIMEOUT_MS).
const STAGE_RESET_TIMEOUT_MS = 10_000;
const LIBRARY_HISTORY_SCHEMA = "elastos.library.history/v1";

// The reset has finished and committed `expectedFactor`.
async function waitStageReset(frame, expectedFactor) {
  const startedAt = Date.now();
  let seen = null;
  while (Date.now() - startedAt <= STAGE_RESET_TIMEOUT_MS) {
    seen = await frame.evaluate(() => ({
      stageReset: document.body.dataset.stageReset ?? null,
      formFactor: document.body.dataset.formFactor || null,
    }));
    if (seen.stageReset === null && seen.formFactor === expectedFactor) {
      return seen;
    }
    await sleep(50);
  }
  throw new Error(`timed out after ${STAGE_RESET_TIMEOUT_MS} ms waiting for the stage reset to commit form factor ${expectedFactor}: ${JSON.stringify({ seen, pageErrors })}`);
}

// Resizes across the phone boundary and waits the reset out. `runs` counts how
// often data-stage-reset was set, synchronous resets included.
async function resizeAcrossBoundary(page, frame, viewport, expectedFactor) {
  const from = await frame.evaluate(() => {
    window.__smokeStageResetObserver?.disconnect();
    window.__smokeStageResetRuns = 0;
    window.__smokeStageResetObserver = new MutationObserver((records) => {
      for (const record of records) {
        if (record.oldValue === null) {
          window.__smokeStageResetRuns += 1;
        }
      }
    });
    window.__smokeStageResetObserver.observe(document.body, {
      attributes: true,
      attributeFilter: ["data-stage-reset"],
      attributeOldValue: true,
    });
    return document.body.dataset.formFactor || null;
  });
  const startedAt = Date.now();
  await page.setViewportSize(viewport);
  await waitStageReset(frame, expectedFactor);
  const durationMs = Date.now() - startedAt;
  await sleep(SURFACE_SETTLE_MS);
  const runs = await frame.evaluate(() => {
    window.__smokeStageResetObserver.disconnect();
    return window.__smokeStageResetRuns;
  });
  return { from, to: expectedFactor, runs, durationMs };
}

function libraryFrameOf(page) {
  return page.frames().find((candidate) => candidate.url().includes("/apps/library/")) || null;
}

// Library's stage, its own history entry and the folder its sidebar marks.
async function libraryStageState(page) {
  const library = libraryFrameOf(page);
  if (!library) {
    return null;
  }
  return library.evaluate(() => ({
    formFactor: document.documentElement.dataset.elFormFactor || null,
    ownEntry: history.state?.schema || null,
    folder: document.querySelector('.place[data-active="true"]')?.dataset.uri || null,
  })).catch(() => null);
}

async function probeFullscreenToPhone(page, frame) {
  const target = "system";
  const homeUrl = page.url();
  const desktopMeta = await settledHostViewportMeta(page, false);
  await openWindow(frame, target);
  await waitWindowGuard(frame, target, "controlGuardUntil");
  await frame.locator(`.window[data-target="${target}"] [data-action="maximize"]`).first().click();
  const fullscreenEntered = await waitFor(
    () => frame.locator(`.window[data-target="${target}"][data-fullscreen-stage="true"]`).count().then((count) => count > 0),
    SHEET_SETTLE_MS * 2,
    `${target} in a fullscreen Space`,
  ).then(() => true, () => false);
  await sleep(SURFACE_SETTLE_MS);
  const before = await stageWindowState(frame, target);
  const reset = await resizeAcrossBoundary(page, frame, STAGE_RESET_PHONE, "phone");
  const afterReset = await stageWindowState(frame, target);
  const phoneMeta = await settledHostViewportMeta(page, true);
  const stageHistory = await frame.evaluate(() => document.body.dataset.stageHistory || "");
  let afterBack = null;
  if (stageHistory === "history") {
    await historyBack(frame);
    await sleep(SHEET_SETTLE_MS);
    afterBack = { ...(await stageWindowState(frame, target)), ...homeState(page, frame) };
  }
  await discardWindows(frame, target);
  await sleep(SURFACE_SETTLE_MS);
  return { target, homeUrl, fullscreenEntered, before, reset, afterReset, desktopMeta, phoneMeta, stageHistory, afterBack };
}

// Starts on the phone stage (where probeFullscreenToPhone ends).
async function probeLibraryAcrossStages(page, frame) {
  const stageHistory = await frame.evaluate(() => document.body.dataset.stageHistory || "");
  if (stageHistory !== "history") {
    return null;
  }
  const homeUrl = page.url();
  const [startFolder, otherFolder] = LIBRARY_FIXTURE_ROOTS.map((root) => root.uri);
  const snapshot = async () => ({
    ...(await stageWindowState(frame, "library")),
    centreOpen: (await readLayer(frame, "control-centre")).open,
    library: await libraryStageState(page),
    ...homeState(page, frame),
  });
  fixture.libraryRoots = true;
  try {
    await openTarget(frame, "library");
    await waitFor(
      () => libraryStageState(page).then((library) => library?.formFactor === "phone" && library.folder === startFolder),
      BOOT_TIMEOUT_MS,
      "Library in its first folder on the phone stage",
    );
    const opened = await snapshot();

    const toDesktop = await resizeAcrossBoundary(page, frame, STAGE_RESET_DESKTOP, "desktop");
    const desktopMeta = await settledHostViewportMeta(page, false);
    const ownEntryInstalled = await waitFor(
      () => libraryStageState(page).then((library) => library?.formFactor === "desktop" && library.ownEntry === LIBRARY_HISTORY_SCHEMA),
      SHEET_SETTLE_MS * 2,
      "Library's own history entry on the desktop",
    ).then(() => true, () => false);
    await libraryFrameOf(page).evaluate((uri) => document.querySelector(`.place[data-uri="${uri}"]`)?.click(), otherFolder);
    await waitFor(
      () => libraryStageState(page).then((library) => library?.folder === otherFolder),
      SHEET_SETTLE_MS * 2,
      "Library in its second folder",
    ).catch(() => {});
    const desktop = { ownEntryInstalled, ...(await snapshot()) };

    const toPhone = await resizeAcrossBoundary(page, frame, STAGE_RESET_PHONE, "phone");
    await waitFor(
      () => libraryStageState(page).then((library) => library?.formFactor === "phone"),
      SHEET_SETTLE_MS * 2,
      "Library back on the phone stage",
    ).catch(() => {});
    const phoneMeta = await settledHostViewportMeta(page, true);
    await LAYER_OPENERS["control-centre"].open(frame);
    await sleep(SHEET_SETTLE_MS);
    const twoLayers = await snapshot();

    await historyBack(frame);
    await waitLayerClosed(frame, "control-centre");
    await sleep(SURFACE_SETTLE_MS);
    const firstBack = await snapshot();
    await historyBack(frame);
    await sleep(SHEET_SETTLE_MS);
    const secondBack = await snapshot();
    await sleep(SHEET_SETTLE_MS);
    const settled = await snapshot();
    await discardWindows(frame, "library");
    await sleep(SURFACE_SETTLE_MS);
    return {
      homeUrl, startFolder, otherFolder, opened, toDesktop, desktopMeta, desktop, toPhone, phoneMeta,
      twoLayers, firstBack, secondBack, settled,
    };
  } finally {
    fixture.libraryRoots = false;
  }
}

// The Search button's popup semantics, which only the phone has.
function searchButtonAria(frame) {
  return frame.evaluate(() => {
    const button = document.querySelector("#toolbar-spotlight");
    return Object.fromEntries(["aria-haspopup", "aria-controls", "aria-expanded"].map((name) => [name, button?.getAttribute(name) ?? null]));
  });
}

function shellOwnsEntry(frame) {
  return frame.evaluate(() => history.state?.elastosStage === true);
}

// Starts on the phone stage (where probeLibraryAcrossStages ends). Each step
// of a reset is reported by its owner, never inferred from a timeout:
// toDesktop: a title-bar press on a phone app, held across the resize to the
//   desktop and released while the reset runs, is cancelled by the reset: a
//   later move opens no Mission Control. On the history stage the shell's own
//   back() is held, so the reset waits its full history timeout (no early
//   success) and recovers: the entry is no longer the shell's and a Back on
//   the desktop keeps Home. Search loses its popup semantics off the phone.
// toPhone (history stage): Library holds its own entries on the desktop and
//   its drop traversal is held, so it cannot acknowledge. The reset asks it
//   again, waits every attempt, and leaves the shell unarmed while Library
//   holds; once the traversal lands Library acknowledges, the shell arms, and
//   one Back sends Library home in its folder, Home staying.
async function probeResetOwnerReports(page, frame) {
  const homeUrl = page.url();
  const stageHistory = await frame.evaluate(() => document.body.dataset.stageHistory || "");
  const limits = await frame.evaluate(async () => {
    const stage = await import(new URL("./shell-phone-stage.js?v=home-20260813a", location.href).href);
    const popovers = await import(new URL("./shell-popovers.js?v=home-20260813a", location.href).href);
    return {
      timeoutMs: stage.RESET_HISTORY_TIMEOUT_MS,
      attempts: stage.RESET_HISTORY_ATTEMPTS,
      gestures: popovers.registeredStageGestures(),
    };
  });
  const phoneAria = await searchButtonAria(frame);

  await openTarget(frame, "system");
  const armed = stageHistory === "history" ? await shellOwnsEntry(frame) : null;
  const head = await frame.locator('.window[data-target="system"] .window-head').first().boundingBox();
  if (stageHistory === "history") {
    await frame.evaluate(() => {
      window.__smokeHeldBacks = 0;
      history.back = () => {
        window.__smokeHeldBacks += 1;
      };
    });
  }
  const x = head.x + head.width / 2;
  const y = head.y + head.height / 2;
  await page.mouse.move(x, y);
  await page.mouse.down();
  const resetStarted = Date.now();
  const resizing = page.setViewportSize(STAGE_RESET_DESKTOP);
  await waitFor(
    () => frame.evaluate(() => document.body.dataset.stageReset === "running").catch(() => false),
    SHEET_SETTLE_MS,
    "the reset to the desktop running",
  ).catch(() => {});
  const releasedDuringReset = await frame.evaluate(() => document.body.dataset.stageReset === "running");
  await page.mouse.up();
  await resizing;
  await waitStageReset(frame, "desktop");
  const toDesktopMs = Date.now() - resetStarted;
  await sleep(SURFACE_SETTLE_MS);
  await page.mouse.move(x, y + 200, { steps: 8 });
  await sleep(SHEET_SETTLE_MS);
  const toDesktop = {
    armed,
    releasedDuringReset,
    durationMs: toDesktopMs,
    missionControl: await frame.evaluate(() => document.body.classList.contains("expose-active")),
    aria: await searchButtonAria(frame),
    heldBacks: stageHistory === "history" ? await frame.evaluate(() => window.__smokeHeldBacks) : null,
    shellEntry: stageHistory === "history" ? await shellOwnsEntry(frame) : null,
  };
  if (stageHistory === "history") {
    await frame.evaluate(() => {
      delete history.back;
    });
    await historyBack(frame);
    await sleep(SHEET_SETTLE_MS);
    toDesktop.afterBack = homeState(page, frame);
  }
  await discardWindows(frame, "system");
  await sleep(SURFACE_SETTLE_MS);
  if (stageHistory !== "history") {
    return { stageHistory, homeUrl, limits, phoneAria, toDesktop, toPhone: null };
  }

  const [, otherFolder] = LIBRARY_FIXTURE_ROOTS.map((root) => root.uri);
  fixture.libraryRoots = true;
  try {
    await openWindow(frame, "library");
    await waitFor(
      () => libraryStageState(page).then((library) => library?.formFactor === "desktop" && library.ownEntry === LIBRARY_HISTORY_SCHEMA),
      BOOT_TIMEOUT_MS,
      "Library holding its own history entry on the desktop",
    );
    await libraryFrameOf(page).evaluate((uri) => document.querySelector(`.place[data-uri="${uri}"]`)?.click(), otherFolder);
    await waitFor(
      () => libraryStageState(page).then((library) => library?.folder === otherFolder),
      SHEET_SETTLE_MS * 2,
      "Library in its second folder",
    );
    await libraryFrameOf(page).evaluate(() => {
      window.__smokeHeldGo = [];
      history.go = (delta) => window.__smokeHeldGo.push(delta);
    });
    const reset = await resizeAcrossBoundary(page, frame, STAGE_RESET_PHONE, "phone");
    const held = {
      reset,
      drops: await libraryFrameOf(page).evaluate(() => window.__smokeHeldGo.length),
      library: await libraryStageState(page),
      window: await stageWindowState(frame, "library"),
      shellEntry: await shellOwnsEntry(frame),
    };
    await libraryFrameOf(page).evaluate(() => {
      delete history.go;
      for (const delta of window.__smokeHeldGo.splice(0)) {
        history.go(delta);
      }
    });
    const armedAfterAck = await waitFor(() => shellOwnsEntry(frame), SHEET_SETTLE_MS * 2, "the shell arming once Library acknowledged")
      .then(() => true, () => false);
    const acknowledged = { armed: armedAfterAck, library: await libraryStageState(page) };
    await historyBack(frame);
    await sleep(SHEET_SETTLE_MS);
    const afterBack = { ...(await stageWindowState(frame, "library")), library: await libraryStageState(page), ...homeState(page, frame) };

    // The drop lands but its popstate never reaches Library: only the shell's
    // drop request ({ drop: true }), answered from history.state, can get the
    // acknowledgement. Without it the reset would wait every attempt and the
    // shell stay unarmed.
    await resizeAcrossBoundary(page, frame, STAGE_RESET_DESKTOP, "desktop");
    await openWindow(frame, "library");
    await waitFor(
      () => libraryStageState(page).then((library) => library?.formFactor === "desktop" && library.ownEntry === LIBRARY_HISTORY_SCHEMA),
      SHEET_SETTLE_MS * 2,
      "Library holding its own entry again on the desktop",
    );
    await libraryFrameOf(page).evaluate(() => {
      window.__smokeSwallowedPops = 0;
      window.addEventListener("popstate", (event) => {
        if (window.__smokeSwallowedPops === 0) {
          window.__smokeSwallowedPops += 1;
          event.stopImmediatePropagation();
        }
      }, { capture: true });
    });
    const silentReset = await resizeAcrossBoundary(page, frame, STAGE_RESET_PHONE, "phone");
    const silent = {
      reset: silentReset,
      swallowed: await libraryFrameOf(page).evaluate(() => window.__smokeSwallowedPops),
      library: await libraryStageState(page),
      shellEntry: await shellOwnsEntry(frame),
    };
    await discardWindows(frame, "library");
    await sleep(SURFACE_SETTLE_MS);
    return { stageHistory, homeUrl, limits, phoneAria, toDesktop, toPhone: { otherFolder, held, acknowledged, afterBack, silent } };
  } finally {
    fixture.libraryRoots = false;
  }
}

// Every phone gesture registers the cancel a stage reset calls.
const RESET_CANCELLED_GESTURES = Object.freeze([
  "title-swipe",
  "long-press",
  "dock-handle",
  "home-edit-drag",
  "sheet-handle:control-centre-handle",
  "sheet-handle:notification-center-handle",
  "sheet-handle:spotlight-handle",
]);

function resetOwnerReportFailures(label, probe) {
  const failures = [];
  const { limits, phoneAria, toDesktop, toPhone } = probe;
  const unregistered = RESET_CANCELLED_GESTURES.filter((id) => !limits.gestures.includes(id));
  if (unregistered.length > 0) {
    failures.push(`${label}/gesture-cancel: every phone gesture must register its cancel for the stage reset. Missing ${JSON.stringify(unregistered)} of ${JSON.stringify(limits.gestures)}`);
  }
  if (phoneAria["aria-haspopup"] !== "dialog" || phoneAria["aria-controls"] !== "spotlight" || phoneAria["aria-expanded"] !== "false") {
    failures.push(`${label}/search-aria: on the phone Search must name Spotlight as its dialog popup, collapsed. Got ${JSON.stringify(phoneAria)}`);
  }
  if (Object.values(toDesktop.aria).some((value) => value !== null)) {
    failures.push(`${label}/search-aria: off the phone Search must keep the base button markup (no popup semantics). Got ${JSON.stringify(toDesktop.aria)}`);
  }
  // Only the history stage holds the reset long enough (its held back()) to
  // release inside it for certain; the button stage still checks the move.
  if (!toDesktop.releasedDuringReset && probe.stageHistory === "history") {
    failures.push(`${label}/gesture-cancel: the title-bar press must be released while the reset runs to test it. Got ${JSON.stringify(toDesktop)}`);
  } else if (toDesktop.missionControl) {
    failures.push(`${label}/gesture-cancel: a title-bar press cancelled by the reset must not open Mission Control on a later move. Got ${JSON.stringify(toDesktop)}`);
  }
  if (probe.stageHistory !== "history") {
    return failures;
  }
  if (!toDesktop.armed || toDesktop.heldBacks !== 1) {
    failures.push(`${label}/own-entry: the phone app must arm the shell's entry and the reset must call back() once to test it. Got ${JSON.stringify(toDesktop)}`);
  } else if (toDesktop.durationMs < limits.timeoutMs || toDesktop.shellEntry || toDesktop.afterBack.url !== probe.homeUrl || !toDesktop.afterBack.shellFrame) {
    failures.push(`${label}/own-entry: a back() that never lands must hold the reset for its full ${limits.timeoutMs} ms, then recover: the entry is no longer the shell's and a Back keeps Home. Got ${JSON.stringify({ homeUrl: probe.homeUrl, limits, toDesktop })}`);
  }
  const { held, acknowledged, afterBack, otherFolder, silent } = toPhone;
  const waitedMs = limits.timeoutMs * limits.attempts;
  if (held.drops !== 1 || held.library?.ownEntry !== LIBRARY_HISTORY_SCHEMA) {
    failures.push(`${label}/app-ack: Library must start exactly one drop and still hold its entry while its traversal is held. Got ${JSON.stringify(held)}`);
  } else if (held.reset.runs !== 1 || held.reset.durationMs < waitedMs - SURFACE_SETTLE_MS) {
    failures.push(`${label}/app-ack: a frame that has not acknowledged must hold the reset for every attempt (${waitedMs} ms), never count as success. Got ${JSON.stringify(held.reset)}`);
  } else if (held.shellEntry || !held.window.onStage) {
    failures.push(`${label}/app-ack: while Library holds its entries the shell must stay unarmed, Library on the stage. Got ${JSON.stringify(held)}`);
  } else if (!acknowledged.armed || acknowledged.library?.ownEntry === LIBRARY_HISTORY_SCHEMA) {
    failures.push(`${label}/app-ack: once Library's traversal lands (back on its load-time guard entry) it must acknowledge and the shell arm. Got ${JSON.stringify(acknowledged)}`);
  } else if (afterBack.onStage || !afterBack.home || afterBack.library?.folder !== otherFolder || afterBack.url !== probe.homeUrl || !afterBack.shellFrame) {
    failures.push(`${label}/app-ack: one Back must then send Library home in its folder, Home staying. Got ${JSON.stringify(afterBack)}`);
  }
  if (silent.swallowed !== 1) {
    failures.push(`${label}/app-request: Library's drop popstate must be withheld once to test the drop request. Got ${JSON.stringify(silent)}`);
  } else if (silent.reset.runs !== 1 || silent.reset.durationMs >= waitedMs
    || silent.library?.ownEntry === LIBRARY_HISTORY_SCHEMA || !silent.shellEntry) {
    failures.push(`${label}/app-request: a drop that landed without its popstate must be acknowledged on the shell's drop request (before ${waitedMs} ms) and the shell arm. Got ${JSON.stringify(silent)}`);
  }
  return failures;
}

// A fine-pointer desktop window that a person narrows to a phone and widens
// again: its own context, so no profile's stage is disturbed.
async function runStageReset(browser, engineId, origin) {
  const context = await browser.newContext({
    viewport: STAGE_RESET_DESKTOP,
    deviceScaleFactor: 1,
    isMobile: false,
    hasTouch: false,
    reducedMotion: "reduce",
  });
  try {
    const { page, frame } = await bootHome(context, origin);
    const bootFactor = await frame.evaluate(() => document.body.dataset.formFactor || null);
    const fullscreenToPhone = await probeFullscreenToPhone(page, frame);
    const libraryAcrossStages = await probeLibraryAcrossStages(page, frame);
    const ownerReports = await probeResetOwnerReports(page, frame);
    return { engine: engineId, bootFactor, fullscreenToPhone, libraryAcrossStages, ownerReports };
  } finally {
    await context.close();
  }
}

function stageResetFailures(run) {
  const label = `${run.engine}/stage-reset`;
  const failures = [];
  const resetFailures = (step, reset) => (reset.runs === 1
    ? []
    : [`${label}/${step}: a resize across the phone boundary (${reset.from} -> ${reset.to}) must run exactly one stage reset. Got ${JSON.stringify(reset)}`]);
  const metaFailures = (step, content, phone) => (viewportMetaFits(content, phone)
    ? []
    : [`${label}/${step}: the host page's viewport meta must ${phone ? `add ${PHONE_VIEWPORT_FIT} on a phone` : `be "${BASE_VIEWPORT_CONTENT}" off a phone`}. Got ${JSON.stringify(content)}`]);
  if (run.bootFactor !== "desktop") {
    failures.push(`${label}: a ${STAGE_RESET_DESKTOP.width}x${STAGE_RESET_DESKTOP.height} fine-pointer window must load as the desktop. Got ${JSON.stringify(run.bootFactor)}`);
  }

  const fullscreen = run.fullscreenToPhone;
  const fullscreenLabel = `${label}/fullscreen-to-phone`;
  failures.push(...metaFailures("fullscreen-to-phone/desktop", fullscreen.desktopMeta, false));
  if (!fullscreen.fullscreenEntered || fullscreen.before.fullscreenWindows === 0) {
    failures.push(`${fullscreenLabel}: the green control must put ${fullscreen.target} in a fullscreen Space on the desktop before the resize. Got ${JSON.stringify(fullscreen.before)}`);
  } else {
    failures.push(...resetFailures("fullscreen-to-phone", fullscreen.reset));
    if (fullscreen.afterReset.fullscreenWindows !== 0 || !fullscreen.afterReset.present || !fullscreen.afterReset.onStage) {
      failures.push(`${fullscreenLabel}: becoming a phone must leave every fullscreen Space and keep ${fullscreen.target} open on the stage. Got ${JSON.stringify(fullscreen.afterReset)}`);
    }
    failures.push(...metaFailures("fullscreen-to-phone/phone", fullscreen.phoneMeta, true));
    const back = fullscreen.afterBack;
    if (back && (!back.home || back.onStage || back.url !== fullscreen.homeUrl || !back.shellFrame)) {
      failures.push(`${fullscreenLabel}: one Back after the reset must send ${fullscreen.target} home, Home staying. Got ${JSON.stringify({ homeUrl: fullscreen.homeUrl, afterBack: back })}`);
    }
  }

  failures.push(...resetOwnerReportFailures(`${label}/owner-reports`, run.ownerReports));

  const library = run.libraryAcrossStages;
  if (!library) {
    return failures;
  }
  const libraryLabel = `${label}/library-across-stages`;
  const inFolder = (state) => state.library?.folder === library.otherFolder;
  failures.push(...resetFailures("library-across-stages/to-desktop", library.toDesktop));
  failures.push(...metaFailures("library-across-stages/desktop", library.desktopMeta, false));
  if (!library.desktop.present || !library.desktop.onStage) {
    failures.push(`${libraryLabel}: Library must stay open across the reset to the desktop. Got ${JSON.stringify(library.desktop)}`);
  }
  if (!library.desktop.ownEntryInstalled || !inFolder(library.desktop) || library.desktop.library?.ownEntry !== LIBRARY_HISTORY_SCHEMA) {
    failures.push(`${libraryLabel}: on the desktop Library must hold its own history entry and move to ${library.otherFolder} to test it. Got ${JSON.stringify(library.desktop)}`);
    return failures;
  }
  failures.push(...resetFailures("library-across-stages/to-phone", library.toPhone));
  failures.push(...metaFailures("library-across-stages/phone", library.phoneMeta, true));
  const { twoLayers, firstBack, secondBack, settled } = library;
  if (!twoLayers.onStage || !twoLayers.centreOpen || twoLayers.openLayers.join() !== "control-centre"
    || (twoLayers.layerCount !== null && twoLayers.layerCount !== 2) || !inFolder(twoLayers)) {
    failures.push(`${libraryLabel}: back on the phone, Library in ${library.otherFolder} with Control Centre over it must be exactly two layers. Got ${JSON.stringify(twoLayers)}`);
    return failures;
  }
  if (firstBack.centreOpen || !firstBack.onStage || firstBack.openLayers.length !== 0 || !inFolder(firstBack)
    || firstBack.url !== library.homeUrl || !firstBack.shellFrame) {
    failures.push(`${libraryLabel}: the first Back must close Control Centre alone and leave Library in its folder. Got ${JSON.stringify(firstBack)}`);
  }
  if (secondBack.onStage || !secondBack.home || secondBack.openLayers.length !== 0 || !inFolder(secondBack)
    || secondBack.url !== library.homeUrl || !secondBack.shellFrame) {
    failures.push(`${libraryLabel}: the second Back must send Library home without stepping its folders, Home staying. Got ${JSON.stringify(secondBack)}`);
  }
  if (settled.onStage !== secondBack.onStage || settled.openLayers.join() !== secondBack.openLayers.join()
    || settled.layerCount !== secondBack.layerCount || settled.library?.folder !== secondBack.library?.folder
    || settled.url !== library.homeUrl || !settled.shellFrame) {
    failures.push(`${libraryLabel}: nothing may change after the second Back settles. Got ${JSON.stringify({ secondBack, settled })}`);
  }
  return failures;
}

// A window control pressed too soon after the window opened is ignored
// (ghost-click guard, shell-windows.js `closeGuardUntil`/`controlGuardUntil`).
async function waitWindowGuard(frame, target, key) {
  const guardMs = await frame.evaluate(({ target, key }) => {
    const until = Number(document.querySelector(`.window[data-target="${target}"]`)?.dataset[key]);
    return Number.isFinite(until) ? Math.max(0, Math.ceil(until - performance.now())) : 0;
  }, { target, key });
  await sleep(guardMs);
}

// The shell ignores a press on Close at the spot that opened the window for a
// moment after it opens (ghost-click guard, shell-windows.js). This smoke
// opens windows with synthetic clicks, so that spot is wherever the last real
// click landed — often the previous window's close button, the very pixel
// tapped next. Wait the guard out instead of tapping into it.
async function closeWindow(frame, target) {
  const closeButton = frame.locator(`.window[data-target="${target}"] [data-action="close"]`);
  if (await closeButton.count()) {
    await waitWindowGuard(frame, target, "closeGuardUntil");
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
  const path = join(dir, `${name}.png`);
  if (page.context().browser()?.browserType().name() === "chromium") {
    // Playwright captures beyond the viewport when an open sheet overflows it.
    // Under mobile emulation Chromium then delivers mouse input at the requested
    // point divided by the device scale factor, on Linux CI and macOS alike.
    const session = await page.context().newCDPSession(page);
    const { data } = await session.send("Page.captureScreenshot", { format: "png", captureBeyondViewport: false });
    await session.detach();
    writeFileSync(path, Buffer.from(data, "base64"));
    return;
  }
  await page.screenshot({ path, fullPage: false });
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
  await waitFor(() => frame.locator(".taskbar").isVisible(), BOOT_TIMEOUT_MS, "Home GUI dock");
  await sleep(SHEET_SETTLE_MS);
  return { page, frame };
}

// Where the bar's controls sat when a surface did not open, so a failure on an
// engine or platform this machine cannot run still says what was under the tap.
async function openFailureState(page, frame) {
  const host = await page.evaluate(() => {
    const shell = document.getElementById("active-shell-frame")?.getBoundingClientRect();
    return {
      innerWidth,
      innerHeight,
      scale: window.visualViewport?.scale ?? null,
      scroll: [scrollX, scrollY],
      visualViewport: window.visualViewport
        ? [visualViewport.offsetLeft, visualViewport.offsetTop, visualViewport.width, visualViewport.height]
        : null,
      scrollWidth: document.documentElement.scrollWidth,
      shellFrame: shell ? [Math.round(shell.x), Math.round(shell.y), Math.round(shell.width), Math.round(shell.height)] : null,
    };
  }).catch((error) => String(error));
  const shell = await frame.evaluate(() => {
    const box = (node) => {
      const rect = node.getBoundingClientRect();
      return [Math.round(rect.left), Math.round(rect.top), Math.round(rect.width), Math.round(rect.height)];
    };
    const bar = document.querySelector(".toolbar");
    const search = document.getElementById("toolbar-spotlight")?.getBoundingClientRect();
    const hit = search ? document.elementFromPoint(search.left + search.width / 2, search.top + search.height / 2) : null;
    const brandImage = document.querySelector("#toolbar-home img");
    return {
      innerWidth,
      innerHeight,
      formFactor: document.body.dataset.formFactor || null,
      bodyClasses: [...document.body.classList],
      scroll: [scrollX, scrollY],
      visualViewport: window.visualViewport
        ? [visualViewport.offsetLeft, visualViewport.offsetTop, visualViewport.width, visualViewport.height]
        : null,
      activeElement: document.activeElement ? `${document.activeElement.tagName}#${document.activeElement.id}` : null,
      pointerEvents: window.phoneSmokePointerEvents || [],
      bar: bar ? box(bar) : null,
      barScrollWidth: bar?.scrollWidth ?? null,
      items: bar
        ? [...bar.querySelectorAll("button, .toolbar-clock")]
          .filter((node) => node.getBoundingClientRect().width > 0)
          .map((node) => `${node.id || String(node.className).slice(0, 24)}:${box(node).join(",")}`)
        : [],
      hitAtSearch: hit ? `${hit.tagName}#${hit.id}.${String(hit.className).slice(0, 40)}` : null,
      brandImage: brandImage ? { box: box(brandImage), natural: [brandImage.naturalWidth, brandImage.naturalHeight], complete: brandImage.complete } : null,
    };
  }).catch((error) => String(error));
  const playwrightSearchBox = await frame.locator("#toolbar-spotlight").boundingBox().catch((error) => String(error));
  return { host, shell, playwrightSearchBox };
}

// Registered before Playwright's own hit-target listeners, so it sees the
// pointer events Playwright rejects as intercepted.
async function recordPointerEvents(frame) {
  await frame.evaluate(() => {
    window.phoneSmokePointerEvents = [];
    for (const type of ["pointerdown", "mousedown", "click"]) {
      window.addEventListener(type, (event) => {
        const target = event.target;
        window.phoneSmokePointerEvents.push(`${type}@${Math.round(event.clientX)},${Math.round(event.clientY)}:${target?.id || target?.tagName}`);
        window.phoneSmokePointerEvents.splice(0, window.phoneSmokePointerEvents.length - 8);
      }, true);
    }
  });
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
  let sheetHandleProbe = null;
  let linkProbe = null;
  let touchMenuProbe = null;
  let inboxOpenProbe = null;
  let homeProbe = null;
  let homeEditProbe = null;
  let backProbe = null;
  let promptCloseProbe = null;
  let backRacesProbe = null;
  let viewportMeta = null;
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
    viewportMeta = await settledHostViewportMeta(page, profile.id.startsWith("phone"));
    await recordPointerEvents(frame);
    for (const surface of SHELL_SURFACES) {
      if (surface.phone === false && profile.id.startsWith("phone")) {
        continue;
      }
      try {
        await surface.open(frame);
      } catch (error) {
        console.error(`[home-phone-layout] ${engineId}/${profile.id}/${surface.id} did not open: ${JSON.stringify(await openFailureState(page, frame))}`);
        throw error;
      }
      await sleep(surface.settle);
      const shellSurface = await measure(frame, surface.id);
      if (surface.capsule) {
        shellSurface.target = surface.capsule;
        Object.assign(shellSurface, await measureCapsuleFrame(page, surface.capsule));
      }
      surfaces.push(shellSurface);
      await screenshot(page, dir, surface.id);
      if (surface.id === "desktop" && profile.id.startsWith("phone")) {
        homeProbe = await probePhoneHome(frame);
      }
      if (surface.id === "assistant-face" && profile.id.startsWith("phone")) {
        keyboardProbe = await probeKeyboardInset(page, frame);
      }
      if (surface.id === "spotlight-results" && profile.id === "phone-portrait") {
        spotlightProbe = await probeSpotlight(page, frame);
      }
      if (surface.id === "notifications" && profile.id === "phone-portrait") {
        linkProbe = await probeHomeLink(page, frame, dir);
      }
      await surface.close(frame);
    }
    if (profile.id === "phone-portrait") {
      sheetHandleProbe = await probeSheetHandles(page, frame);
      touchMenuProbe = await probeTouchMenu(page, frame, dir);
      inboxOpenProbe = await probeInboxOpenClosesRail(page, frame);
    }
    if (profile.id.startsWith("phone")) {
      homeEditProbe = await probePhoneHomeEdit(page, frame, dir);
      backProbe = await probePhoneBack(frame);
      promptCloseProbe = await probePromptTouchClose(frame);
      backRacesProbe = await probeBackRaces(page, frame);
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
  return { engine: engineId, profile: profile.id, viewport: profile.viewport, viewportMeta, surfaces, dock: dockProbe, stage: stageProbe, keyboard: keyboardProbe, spotlight: spotlightProbe, sheetHandles: sheetHandleProbe, link: linkProbe, touchMenu: touchMenuProbe, inboxOpen: inboxOpenProbe, home: homeProbe, homeEdit: homeEditProbe, back: backProbe, promptClose: promptCloseProbe, backRaces: backRacesProbe, bootRetries };
}

// Open on an Inbox rail entry reaches Home as the host's open-target command
// (#92): the rail must close and the target window must be the one on show.
async function probeInboxOpenClosesRail(page, frame) {
  const target = "system";
  await callShellModule(frame, "shell-inbox-rail.js", "showInboxRail");
  await waitFor(
    () => frame.evaluate(() => document.querySelector("#inbox-rail")?.hidden === false),
    SURFACE_SETTLE_MS * 10,
    "inbox rail open",
  );
  await page.evaluate((message) => {
    document.getElementById("active-shell-frame").contentWindow.postMessage(message, "*");
  }, { type: "home:gui-command", command: "open-target", target, query: {} });
  const read = () => frame.evaluate((target) => {
    const win = document.querySelector(`.window[data-target="${target}"]`);
    return {
      railHidden: document.querySelector("#inbox-rail").hidden === true,
      windowVisible: Boolean(win) && !win.classList.contains("hidden") && win.getClientRects().length > 0,
    };
  }, target);
  await waitFor(
    () => read().then((state) => state.railHidden && state.windowVisible),
    SURFACE_SETTLE_MS * 10,
    "inbox Open closes the rail and shows the target",
  );
  const state = await read();
  await closeWindow(frame, target);
  return { target, ...state };
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
  const drawer = await probeAssistantDrawer(capsuleFrame);
  return { raised, spoofed, lowered, capsuleFrame: Boolean(capsuleFrame), drawer };
}

// The context runs with reduced motion: the phone drawer must still sit off
// screen when closed (clear of the conversation) and on screen when open.
async function probeAssistantDrawer(capsuleFrame) {
  if (!capsuleFrame) return null;
  const read = () => capsuleFrame.evaluate(() => {
    const box = (el) => {
      const r = el.getBoundingClientRect();
      return { left: Math.round(r.left), right: Math.round(r.right), width: Math.round(r.width) };
    };
    return {
      sidebar: box(document.querySelector(".agent-harness-sidebar")),
      main: box(document.querySelector(".agent-harness-main")),
      open: document.body.classList.contains("agent-harness-drawer-open"),
    };
  });
  const toggle = () => capsuleFrame.evaluate(() => document.querySelector("#agent-harness-drawer-open").click());
  const closed = await read();
  await toggle();
  await sleep(KEYBOARD_SETTLE_MS);
  const opened = await read();
  await toggle();
  await sleep(KEYBOARD_SETTLE_MS);
  return { closed, opened, reclosed: await read() };
}

function phoneKeyboardFailures(run) {
  if (!run.keyboard) {
    return [];
  }
  const { raised, spoofed, lowered, capsuleFrame, drawer } = run.keyboard;
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
  const clear = (state) => state && !state.open && state.sidebar.right <= state.main.left;
  if (!drawer || !clear(drawer.closed) || !clear(drawer.reclosed)
    || !drawer.opened.open || drawer.opened.sidebar.left !== 0 || drawer.opened.sidebar.width <= 0) {
    failures.push(`${label}/drawer: with reduced motion the closed Assistant drawer must sit clear of the conversation and the open drawer on screen. Got ${JSON.stringify(drawer)}`);
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

// Host says the gateway is gone (`home:link-status`). The bar shows
// "Reconnecting…" and Notification Centre opens with the explanation;
// a reachable message clears both. Only the host page may post it.
const LINK_DOT_MIN_GAP_PX = 8;

async function probeHomeLink(page, frame, dir) {
  const post = (reachable) => page.evaluate((message) => {
    document.getElementById("active-shell-frame").contentWindow.postMessage(message, "*");
  }, { type: "home:link-status", reachable });
  const read = () => frame.evaluate(() => {
    const banner = document.querySelector(".notification-center-link");
    const dot = document.querySelector(".toolbar-link-status-dot").getBoundingClientRect();
    return {
      link: document.body.dataset.homeLink || "",
      bar: document.querySelector("#toolbar-link-status-text")?.textContent || "",
      banner: banner ? getComputedStyle(banner).display : "missing",
      dotGap: Math.round(dot.left - document.querySelector("#toolbar-home").getBoundingClientRect().right),
      overflow: document.documentElement.scrollWidth > document.documentElement.clientWidth,
    };
  });
  await frame.evaluate(() => {
    window.parent.postMessage({ type: "home:link-status", reachable: false }, "*");
  });
  await sleep(KEYBOARD_SETTLE_MS);
  const spoofed = await read();
  await post(false);
  await sleep(KEYBOARD_SETTLE_MS);
  const down = await read();
  await screenshot(page, dir, "notifications-reconnecting");
  await post(true);
  await sleep(KEYBOARD_SETTLE_MS);
  return { spoofed, down, up: await read() };
}

function phoneLinkFailures(run) {
  if (!run.link) {
    return [];
  }
  const { spoofed, down, up } = run.link;
  const label = `${run.engine}/${run.profile}/link`;
  const failures = [];
  if (spoofed.link !== "" || spoofed.bar !== "") {
    failures.push(`${label}: a capsule frame must not move the Home link. Got ${JSON.stringify(spoofed)}`);
  }
  if (down.link !== "reconnecting" || down.bar !== "Reconnecting…" || down.banner === "none" || down.overflow) {
    failures.push(`${label}: the host's unreachable message must show Reconnecting on the bar and in Notification Centre without overflowing. Got ${JSON.stringify(down)}`);
  }
  if (down.dotGap < LINK_DOT_MIN_GAP_PX) {
    failures.push(`${label}: the bar's Reconnecting dot must stand clear of the brand. Got ${JSON.stringify(down)}`);
  }
  if (up.link !== "" || up.bar !== "" || up.banner !== "none") {
    failures.push(`${label}: a reachable message must clear the bar and the explanation. Got ${JSON.stringify(up)}`);
  }
  return failures;
}

// Phone Home screen: the resting screen is the app grid. Every app is either
// in the Dock (at most PHONE_DOCK_SLOTS Shelf pins, beside the Assistant) or
// on the grid, never both; the Apps button, desktop files and the Dock's Bin
// and running apps stand down; the grid sits between the bar and the Dock.
const PHONE_DOCK_SLOTS = 4;

async function probePhoneHome(frame) {
  return frame.evaluate(() => {
    const shown = (node) => Boolean(node) && node.getClientRects().length > 0;
    const grid = document.querySelector("#phone-home");
    const dock = document.querySelector(".taskbar").getBoundingClientRect();
    const toolbar = document.querySelector(".toolbar").getBoundingClientRect();
    const gridApps = Array.from(document.querySelectorAll(".phone-home-app")).filter(shown);
    const lastApp = gridApps.at(-1)?.getBoundingClientRect();
    return {
      gridShown: shown(grid),
      gridApps: gridApps.map((node) => node.dataset.target),
      dockApps: Array.from(document.querySelectorAll(".taskbar-sortable .taskbar-item[data-target]"))
        .filter(shown)
        .map((node) => node.dataset.target),
      assistantTile: shown(document.querySelector("#assistant-toggle")),
      otherDockTiles: Array.from(document.querySelectorAll(".taskbar-sortable .taskbar-entry"))
        .filter((node) => shown(node) && !node.querySelector(".taskbar-item[data-target]")).length,
      launcherToggleShown: shown(document.querySelector("#launcher-toggle")),
      desktopShortcutsShown: Array.from(document.querySelectorAll(".desktop-shortcut")).some(shown),
      columns: getComputedStyle(grid.querySelector(".phone-home-page[data-page]")).gridTemplateColumns.split(" ").length,
      gridTop: Math.round(grid.getBoundingClientRect().top),
      toolbarBottom: Math.round(toolbar.bottom),
      lastAppBottom: lastApp ? Math.round(lastApp.bottom) : null,
      dockTop: Math.round(dock.top),
    };
  });
}

function phoneHomeFailures(run) {
  if (!run.home) {
    return [];
  }
  const home = run.home;
  const label = `${run.engine}/${run.profile}/home`;
  const failures = [];
  const expected = FIRST_PARTY_APPS.map(([target]) => target);
  const inDock = [...home.dockApps, ...(home.assistantTile ? ["assistant"] : [])];
  const placed = [...home.gridApps, ...inDock];
  if (!home.gridShown || expected.some((target) => !placed.includes(target))) {
    failures.push(`${label}: every app must be on the Home grid or in the Dock. Got ${JSON.stringify(home)}`);
  }
  if (home.gridApps.some((target) => inDock.includes(target))) {
    failures.push(`${label}: an app in the Dock must not also be on the Home grid. Got ${JSON.stringify(home)}`);
  }
  if (home.dockApps.length > PHONE_DOCK_SLOTS || home.otherDockTiles > 0 || home.launcherToggleShown) {
    failures.push(`${label}: the phone Dock holds at most ${PHONE_DOCK_SLOTS} apps beside the Assistant, with no Apps button, Bin or running apps. Got ${JSON.stringify(home)}`);
  }
  if (home.desktopShortcutsShown) {
    failures.push(`${label}: desktop files must not show on the phone Home. Got ${JSON.stringify(home)}`);
  }
  if (home.gridTop < home.toolbarBottom || home.lastAppBottom === null || home.lastAppBottom > home.dockTop) {
    failures.push(`${label}: the Home grid must sit between the bar and the Dock. Got ${JSON.stringify(home)}`);
  }
  const wantColumns = run.profile === "phone-landscape" ? 6 : 4;
  if (home.columns !== wantColumns) {
    failures.push(`${label}: the Home grid must be ${wantColumns} columns. Got ${JSON.stringify(home)}`);
  }
  return failures;
}

// Home pages and edit mode, played as a thumb would: hold a grid app until
// its menu opens, move it (edit mode takes over), push it against the right
// edge until the page turns, drop it there; move a Dock app onto the grid and
// back; Done; turn back with the dots; swipe right past page 1 into the
// Assistant, which hands back page 1 when it closes.
const EDGE_TURN_WAIT_MS = 2500;
const PAGE_SETTLE_WAIT_MS = 400;
const ASSISTANT_OPEN_WAIT_MS = 1800;

async function probePhoneHomeEdit(page, frame, dir) {
  const read = () => frame.evaluate(() => {
    const pages = document.querySelector("#phone-home-pages");
    const gridPages = Array.from(pages.querySelectorAll(":scope > .phone-home-page[data-page]"));
    const hasAssistantPage = Boolean(pages.querySelector(":scope > .phone-home-assistant-page"));
    const current = Math.round(pages.scrollLeft / pages.clientWidth) - (hasAssistantPage ? 1 : 0);
    const dots = document.querySelector("#phone-home-dots");
    return {
      pages: gridPages.map((node) => Array.from(node.querySelectorAll(".phone-home-app")).map((tile) => tile.dataset.target)),
      dock: Array.from(document.querySelectorAll('.taskbar-entry[data-phone-dock="slot"] > .taskbar-item')).map((node) => node.dataset.target),
      hasAssistantPage,
      current,
      dots: dots.children.length,
      dotsShown: dots.dataset.single === "false",
      editing: document.body.classList.contains("phone-home-editing"),
      menuOpen: !document.querySelector("#desktop-context-menu").hidden,
      ghost: Boolean(document.querySelector(".phone-home-ghost")),
      doneShown: document.querySelector("#phone-home-done").getClientRects().length > 0,
      assistantOpen: document.body.classList.contains("assistant-space-active"),
      windows: document.querySelectorAll(".window").length,
    };
  });
  // One synthetic touch pointer, dispatched at the pressed tile the way a
  // browser targets a captured touch.
  const pointer = (type, selector, at) => frame.evaluate(({ type, selector, at }) => {
    if (selector) {
      window.__smokeTile = document.querySelector(selector);
    }
    const tile = window.__smokeTile;
    const box = tile.getBoundingClientRect();
    const x = at === "centre" ? box.x + box.width / 2 : at.x;
    const y = at === "centre" ? box.y + box.height / 2 : at.y;
    tile.dispatchEvent(new PointerEvent(type, {
      bubbles: true, cancelable: true, composed: true,
      pointerId: 43, pointerType: "touch", isPrimary: true, clientX: x, clientY: y,
    }));
    return { x, y };
  }, { type, selector, at });
  const spot = (selector, dx = 0, dy = 0) => frame.evaluate(({ selector, dx, dy }) => {
    const box = document.querySelector(selector).getBoundingClientRect();
    return { x: box.x + box.width / 2 + dx, y: box.y + box.height / 2 + dy };
  }, { selector, dx, dy });
  const pageMiddle = () => spot("#phone-home-pages", 0, 0);

  const before = await read();
  const firstPage = before.pages[before.current] || [];
  const dragged = firstPage[0];
  const tileSelector = (target) => `.phone-home-app[data-target="${target}"]`;

  // Hold, then move: the menu gives way to edit mode.
  const start = await pointer("pointerdown", tileSelector(dragged), "centre");
  await sleep(LONG_PRESS_HOLD_MS);
  const held = await read();
  await pointer("pointermove", null, { x: start.x + 24, y: start.y });
  const lifted = await read();

  // Against the right edge until the page turns, then drop on that page.
  const edge = await frame.evaluate(() => window.innerWidth - 6);
  await pointer("pointermove", null, { x: edge, y: start.y });
  const turnedFrom = lifted.current;
  let turned = lifted;
  for (let waited = 0; waited < EDGE_TURN_WAIT_MS && turned.current === turnedFrom; waited += 100) {
    await sleep(100);
    turned = await read();
  }
  await pointer("pointermove", null, await pageMiddle());
  await sleep(PAGE_SETTLE_WAIT_MS);
  await pointer("pointerup", null, await pageMiddle());
  await sleep(PAGE_SETTLE_WAIT_MS);
  const dropped = await read();
  await screenshot(page, dir, "home-edit");

  // A tap in edit mode opens nothing.
  await frame.evaluate((selector) => document.querySelector(selector).click(), tileSelector(dragged));
  await sleep(SURFACE_SETTLE_MS);
  const tapped = await read();

  // A Dock app onto the grid, then back into the Dock.
  const dockApp = dropped.dock[0];
  const dockSelector = `.taskbar-entry[data-phone-dock="slot"] > .taskbar-item[data-target="${dockApp}"]`;
  const dockStart = await pointer("pointerdown", dockSelector, "centre");
  await pointer("pointermove", null, { x: dockStart.x, y: dockStart.y - 30 });
  await pointer("pointermove", null, await pageMiddle());
  await pointer("pointerup", null, await pageMiddle());
  await sleep(PAGE_SETTLE_WAIT_MS);
  const dockOut = await read();
  const backStart = await pointer("pointerdown", tileSelector(dockApp), "centre");
  await pointer("pointermove", null, { x: backStart.x, y: backStart.y + 20 });
  const dockSpot = await spot(".taskbar-sortable");
  await pointer("pointermove", null, dockSpot);
  await pointer("pointerup", null, dockSpot);
  await sleep(PAGE_SETTLE_WAIT_MS);
  const dockIn = await read();

  await frame.locator("#phone-home-done").click();
  await sleep(SURFACE_SETTLE_MS);
  const done = await read();

  // The dots turn back to page 1.
  const firstDot = await spot("#phone-home-dots .phone-home-dot");
  await frame.evaluate(({ x, y }) => {
    document.querySelector("#phone-home-dots").dispatchEvent(new MouseEvent("click", { bubbles: true, clientX: x, clientY: y }));
  }, firstDot);
  await sleep(PAGE_SETTLE_WAIT_MS);
  const dotted = await read();

  // Swipe right past page 1: the Assistant opens; page 1 is back behind it.
  await frame.evaluate(() => document.querySelector("#phone-home-pages").scrollTo({ left: 0 }));
  await sleep(ASSISTANT_OPEN_WAIT_MS);
  const assistant = await read();
  await screenshot(page, dir, "home-assistant");
  await pressEscape(frame);
  await sleep(SHEET_SETTLE_MS);
  const closed = await read();
  return { dragged, dockApp, before, held, lifted, turned, dropped, tapped, dockOut, dockIn, done, dotted, assistant, closed };
}

function phoneHomeEditFailures(run) {
  if (!run.homeEdit) {
    return [];
  }
  const { dragged, dockApp, before, held, lifted, turned, dropped, tapped, dockOut, dockIn, done, dotted, assistant, closed } = run.homeEdit;
  const label = `${run.engine}/${run.profile}/home-edit`;
  const failures = [];
  const pageOf = (state, target) => state.pages.findIndex((page) => page.includes(target));
  const everyAppOnce = (state) => {
    const placed = [...state.pages.flat(), ...state.dock];
    return new Set(placed).size === placed.length;
  };
  if (!before.hasAssistantPage || before.current !== 0) {
    failures.push(`${label}: the Home must open on page 1 with the Assistant page to its left. Got ${JSON.stringify(before)}`);
  }
  if (!held.menuOpen || held.editing) {
    failures.push(`${label}: holding a grid app must open its menu first. Got ${JSON.stringify(held)}`);
  }
  if (lifted.menuOpen || !lifted.editing || !lifted.ghost || !lifted.doneShown) {
    failures.push(`${label}: moving a held app must close the menu and pick the app up in edit mode, with Done showing. Got ${JSON.stringify(lifted)}`);
  }
  if (turned.current !== lifted.current + 1) {
    failures.push(`${label}: holding an app against the right edge must turn the page. Got ${JSON.stringify(turned)}`);
  }
  if (pageOf(dropped, dragged) !== turned.current || !dropped.editing || dropped.ghost || !everyAppOnce(dropped)) {
    failures.push(`${label}: the app must land on the page it was dropped on, still in edit mode, every app placed once. Got ${JSON.stringify(dropped)}`);
  }
  if (dropped.pages.length < 2 || !dropped.dotsShown || dropped.dots !== dropped.pages.length) {
    failures.push(`${label}: two pages must show one dot each. Got ${JSON.stringify(dropped)}`);
  }
  if (tapped.windows !== dropped.windows) {
    failures.push(`${label}: a tap in edit mode must not open an app. Got ${JSON.stringify(tapped)}`);
  }
  if (dockOut.dock.includes(dockApp) || pageOf(dockOut, dockApp) < 0 || !everyAppOnce(dockOut)) {
    failures.push(`${label}: a Dock app dragged onto the grid must leave the Dock for the grid. Got ${JSON.stringify(dockOut)}`);
  }
  if (!dockIn.dock.includes(dockApp) || pageOf(dockIn, dockApp) >= 0 || !everyAppOnce(dockIn)) {
    failures.push(`${label}: a grid app dragged onto the Dock must leave the grid for the Dock. Got ${JSON.stringify(dockIn)}`);
  }
  if (done.editing || done.doneShown || pageOf(done, dragged) !== pageOf(dropped, dragged)) {
    failures.push(`${label}: Done must end edit mode and keep the arrangement. Got ${JSON.stringify(done)}`);
  }
  if (dotted.current !== 0) {
    failures.push(`${label}: the first dot must turn back to page 1. Got ${JSON.stringify(dotted)}`);
  }
  if (!assistant.assistantOpen || assistant.current !== 0) {
    failures.push(`${label}: swiping right past page 1 must open the Assistant and leave page 1 behind it. Got ${JSON.stringify(assistant)}`);
  }
  if (closed.assistantOpen || closed.current !== 0) {
    failures.push(`${label}: closing the Assistant must land on page 1. Got ${JSON.stringify(closed)}`);
  }
  return failures;
}

// Touch menus on a portrait phone: a long-press on a Dock app opens its menu
// as a full-width bottom sheet with thumb-size rows and does not launch the
// app; a tap beside the sheet only dismisses it (the click-driven Control
// Centre button under that tap stays shut).
const MENU_SHEET_EDGE_PX = 16;
const MENU_SHEET_ROW_PX = 48;
const MENU_SHEET_FONT_PX = 16;

async function probeTouchMenu(page, frame, dir) {
  const windowsBefore = await frame.locator(".window").count();
  const target = await longPressDockApp(frame);
  await sleep(SURFACE_SETTLE_MS);
  const opened = await frame.evaluate(() => {
    const menu = document.querySelector("#desktop-context-menu");
    const box = menu.getBoundingClientRect();
    const rows = Array.from(menu.querySelectorAll(".context-menu-item")).map((row) => ({
      height: row.getBoundingClientRect().height,
      font: parseFloat(getComputedStyle(row).fontSize),
    }));
    return {
      open: !menu.hidden,
      sheet: menu.classList.contains("context-menu-sheet"),
      title: menu.querySelector(".context-menu-title")?.textContent || "",
      label: menu.getAttribute("aria-label"),
      left: Math.round(box.left),
      rightGap: Math.round(window.innerWidth - box.right),
      bottomGap: Math.round(window.innerHeight - box.bottom),
      rows: rows.length,
      minRow: Math.round(Math.min(...rows.map((row) => row.height))),
      minFont: Math.min(...rows.map((row) => row.font)),
    };
  });
  await screenshot(page, dir, "context-menu-sheet");
  const windowsAfterPress = await frame.locator(".window").count();
  const beside = await frame.locator("#toolbar-control-centre").boundingBox();
  await page.touchscreen.tap(beside.x + beside.width / 2, beside.y + beside.height / 2);
  await sleep(SHEET_SETTLE_MS);
  const dismissed = await frame.evaluate(() => ({
    menuOpen: !document.querySelector("#desktop-context-menu").hidden,
    controlCentreOpen: !document.querySelector("#control-centre").hidden,
  }));
  if (dismissed.menuOpen || dismissed.controlCentreOpen) {
    await pressEscape(frame);
  }
  // The same hold on a Home grid icon opens that app's menu and launches nothing.
  const homeTarget = await longPressApp(frame, ".phone-home-app");
  await sleep(SURFACE_SETTLE_MS);
  const home = await frame.evaluate(() => {
    const menu = document.querySelector("#desktop-context-menu");
    return {
      open: !menu.hidden,
      sheet: menu.classList.contains("context-menu-sheet"),
      title: menu.querySelector(".context-menu-title")?.textContent || "",
      items: Array.from(menu.querySelectorAll(".context-menu-item")).map((row) => row.textContent),
      windows: document.querySelectorAll(".window").length,
    };
  });
  await pressEscape(frame);
  return { target, windowsBefore, windowsAfterPress, opened, dismissed, homeTarget, home };
}

function phoneTouchMenuFailures(run) {
  if (!run.touchMenu) {
    return [];
  }
  const { windowsBefore, windowsAfterPress, opened, dismissed } = run.touchMenu;
  const label = `${run.engine}/${run.profile}/touch-menu`;
  const failures = [];
  if (!opened.open || opened.rows === 0) {
    failures.push(`${label}: a long-press on a Dock app must open its menu. Got ${JSON.stringify(run.touchMenu)}`);
    return failures;
  }
  if (windowsAfterPress !== windowsBefore) {
    failures.push(`${label}: the release after a long-press must not launch the app. Got ${JSON.stringify(run.touchMenu)}`);
  }
  if (!opened.sheet || opened.left > MENU_SHEET_EDGE_PX || opened.rightGap > MENU_SHEET_EDGE_PX || opened.bottomGap > MENU_SHEET_EDGE_PX) {
    failures.push(`${label}: the menu must be a full-width sheet at the bottom of the phone. Got ${JSON.stringify(opened)}`);
  }
  if (!opened.title || opened.label !== `${opened.title} actions`) {
    failures.push(`${label}: the menu sheet must name the app it acts on. Got ${JSON.stringify(opened)}`);
  }
  if (opened.minRow < MENU_SHEET_ROW_PX || opened.minFont < MENU_SHEET_FONT_PX) {
    failures.push(`${label}: menu sheet rows must be ${MENU_SHEET_ROW_PX} px at ${MENU_SHEET_FONT_PX} px text. Got ${JSON.stringify(opened)}`);
  }
  if (dismissed.menuOpen || dismissed.controlCentreOpen) {
    failures.push(`${label}: a tap beside the menu sheet must only dismiss it. Got ${JSON.stringify(dismissed)}`);
  }
  const { home } = run.touchMenu;
  if (!home.open || !home.sheet || !home.title || home.items[0] !== `Open ${home.title}` || home.windows !== windowsBefore) {
    failures.push(`${label}: a long-press on a Home grid app must open its named menu sheet and launch nothing. Got ${JSON.stringify(run.touchMenu)}`);
  }
  return failures;
}

// Closed event stream on a portrait phone: the host keeps reconnecting and
// refreshing its summary, and each refresh rebuilds the Dock. A finger holds a
// Dock tile while the next refresh lands; the redraw must come before the
// long-press fires, and the release must open that app's menu and launch
// nothing.
const CLOSED_STREAM_REFRESH_TIMEOUT_MS = 10_000;
const CLOSED_STREAM_PROFILE = PROFILES.find((profile) => profile.id === "phone-portrait");

async function runClosedStream(browser, engineId, origin) {
  const dir = join(outputRoot, engineId, "phone-portrait-closed-stream");
  mkdirSync(dir, { recursive: true });
  const context = await browser.newContext({
    viewport: CLOSED_STREAM_PROFILE.viewport,
    deviceScaleFactor: CLOSED_STREAM_PROFILE.deviceScaleFactor,
    isMobile: CLOSED_STREAM_PROFILE.isMobile,
    hasTouch: CLOSED_STREAM_PROFILE.hasTouch,
    reducedMotion: "reduce",
  });
  fixture.closeEventStream = true;
  try {
    const { page, frame } = await bootHome(context, origin);
    const windowsBefore = await frame.locator(".window").count();
    const repliesAtBoot = fixture.summaryReplies;
    await waitFor(() => fixture.summaryReplies > repliesAtBoot, CLOSED_STREAM_REFRESH_TIMEOUT_MS,
      "a summary refresh from the closed event stream's recovery");
    fixture.holdSummaries = true;
    await waitFor(() => fixture.heldSummaries.length > 0, CLOSED_STREAM_REFRESH_TIMEOUT_MS,
      "the next reconnect's summary refresh");
    const startedAt = Date.now();
    const target = await pressApp(frame, ".taskbar-item[data-target]");
    releaseHeldSummaries();
    await sleep(Math.max(0, LONG_PRESS_HOLD_MS - (Date.now() - startedAt)));
    const tileRemovedAfterMs = await frame.evaluate(() => window.__smokeTileRemovedAfterMs);
    await frame.evaluate(() => window.__smokeReleaseLongPress());
    await sleep(SURFACE_SETTLE_MS);
    const menu = await frame.evaluate(() => {
      const node = document.querySelector("#desktop-context-menu");
      return {
        open: !node.hidden,
        sheet: node.classList.contains("context-menu-sheet"),
        title: node.querySelector(".context-menu-title")?.textContent || "",
        items: Array.from(node.querySelectorAll(".context-menu-item")).map((row) => row.textContent),
      };
    });
    const expectedTitle = FIRST_PARTY_APPS.find(([id]) => id === target)?.[1] || "";
    await screenshot(page, dir, "closed-stream-dock-hold");
    const windowsAfter = await frame.locator(".window").count();
    await pressEscape(frame);
    return { engine: engineId, target, expectedTitle, tileRemovedAfterMs, menu, windowsBefore, windowsAfter };
  } finally {
    fixture.closeEventStream = false;
    releaseHeldSummaries();
    await context.close();
  }
}

function closedStreamFailures(run) {
  const label = `${run.engine}/phone-portrait/closed-stream`;
  if (run.tileRemovedAfterMs === null || run.tileRemovedAfterMs >= LONG_PRESS_MS) {
    return [`${label}: the reconnect's summary refresh must redraw the Dock under the held tile before the long-press fires (${LONG_PRESS_MS} ms). Got ${JSON.stringify(run)}`];
  }
  const failures = [];
  if (!run.menu.open || !run.menu.sheet || run.menu.title !== run.expectedTitle) {
    failures.push(`${label}: a long-press held through a Dock redraw must open the held app's menu. Got ${JSON.stringify(run)}`);
  }
  if (run.windowsAfter !== run.windowsBefore) {
    failures.push(`${label}: the release after a long-press held through a Dock redraw must not launch the app. Got ${JSON.stringify(run)}`);
  }
  return failures;
}

// Sheet grab handles on a portrait phone: each bar sheet's handle is a 44 px
// Close (a tap closes), a short drag snaps the sheet back, and a long drag
// toward its origin (up) closes it.
const SHEET_HANDLE_SHORT_DRAG_PX = 20;
const SHEET_HANDLE_LONG_DRAG_PX = 90;
const SHEET_HANDLES = [
  { id: "control-centre", opener: "#toolbar-control-centre", sheet: "#control-centre", handle: "#control-centre-handle", dragSign: -1, tapCloses: true },
  { id: "notifications", opener: "#clock", sheet: "#notification-center", handle: "#notification-center-handle", dragSign: -1, tapCloses: true },
  { id: "spotlight", opener: "#toolbar-spotlight", sheet: "#spotlight", handle: "#spotlight-handle", dragSign: -1, tapCloses: true },
];

async function probeSheetHandles(page, frame) {
  const results = [];
  for (const spec of SHEET_HANDLES) {
    const isOpen = () => frame.evaluate((selector) => !document.querySelector(selector).hidden, spec.sheet);
    const drag = async (distance) => {
      const box = await frame.locator(spec.handle).boundingBox();
      const x = box.x + box.width / 2;
      const y = box.y + box.height / 2;
      await page.mouse.move(x, y);
      await page.mouse.down();
      await page.mouse.move(x, y + (spec.dragSign * distance) / 2, { steps: 4 });
      await page.mouse.move(x, y + spec.dragSign * distance, { steps: 4 });
      await page.mouse.up();
      await sleep(SURFACE_SETTLE_MS);
    };
    const open = async () => {
      await frame.locator(spec.opener).click();
      await sleep(SHEET_SETTLE_MS);
    };
    await open();
    const opened = await isOpen();
    const handle = await frame.evaluate((selector) => {
      const node = document.querySelector(selector);
      const rect = node.getBoundingClientRect();
      return { height: Math.round(rect.height), label: node.getAttribute("aria-label"), tag: node.tagName.toLowerCase() };
    }, spec.handle);
    let tapClosed = null;
    if (spec.tapCloses) {
      await frame.locator(spec.handle).click();
      await sleep(SURFACE_SETTLE_MS);
      tapClosed = !(await isOpen());
      await open();
    }
    await drag(SHEET_HANDLE_SHORT_DRAG_PX);
    const shortDragStayed = await isOpen();
    const translateCleared = await frame.evaluate(
      (selector) => !Array.from(document.querySelectorAll(`${selector}, ${selector} *`)).some((node) => node.style?.translate),
      spec.sheet,
    );
    await drag(SHEET_HANDLE_LONG_DRAG_PX);
    const dragClosed = !(await isOpen());
    if (!dragClosed) {
      await pressEscape(frame);
    }
    results.push({ id: spec.id, opened, handle, tapClosed, shortDragStayed, translateCleared, dragClosed });
  }
  return results;
}

function phoneSheetHandleFailures(run) {
  if (!run.sheetHandles) {
    return [];
  }
  const failures = [];
  for (const result of run.sheetHandles) {
    const spec = SHEET_HANDLES.find((candidate) => candidate.id === result.id);
    const label = `${run.engine}/${run.profile}/${result.id}-handle`;
    if (!result.opened) {
      failures.push(`${label}: the sheet did not open from ${spec.opener}. Got ${JSON.stringify(result)}`);
      continue;
    }
    if (spec.tapCloses && (result.handle.tag !== "button" || result.handle.label !== "Close" || result.handle.height < MIN_TARGET_PX || result.tapClosed !== true)) {
      failures.push(`${label}: the handle must be a 44 px Close button that closes on tap. Got ${JSON.stringify(result)}`);
    }
    if (!result.shortDragStayed || !result.translateCleared) {
      failures.push(`${label}: a short drag must snap the sheet back. Got ${JSON.stringify(result)}`);
    }
    if (!result.dragClosed) {
      failures.push(`${label}: dragging the handle toward the sheet's origin must close it. Got ${JSON.stringify(result)}`);
    }
  }
  return failures;
}

// System back on a phone with nothing else on the stage. The long-press menu,
// the welcome sheet over the Assistant room, and then the room each have to
// close in that order, and Home has to stay. Then an off-Space window that is
// still the active one must not be the window Back minimises.
async function openTarget(frame, target) {
  await frame.evaluate((target) => {
    const button = document.querySelector(`.taskbar-item[data-target="${target}"]`)
      || document.querySelector(`.phone-home-app[data-target="${target}"]`);
    button?.click();
  }, target);
  await waitFor(
    () => frame.locator(`.window[data-target="${target}"]`).count().then((count) => count > 0),
    BOOT_TIMEOUT_MS,
    `${target} window`,
  );
  await sleep(SHEET_SETTLE_MS);
}

async function probePhoneBack(frame) {
  const read = () => frame.evaluate(() => {
    const menu = document.querySelector("#desktop-context-menu");
    const space = document.querySelector("#assistant-space");
    const windows = Array.from(document.querySelectorAll(".window")).map((node) => ({
      target: node.dataset.target,
      hidden: node.classList.contains("hidden"),
      spaceVisible: node.dataset.spaceVisible || "",
      active: node.classList.contains("window-active"),
    }));
    const welcome = document.querySelector("#setup-sheet");
    return {
      home: location.href.includes("/apps/home-gui/"),
      stageHistory: document.body.dataset.stageHistory || "",
      menuOpen: Boolean(menu && !menu.hidden),
      assistantOpen: Boolean(space && !space.hidden),
      welcomeOpen: Boolean(welcome && !welcome.hidden),
      windows,
    };
  });
  const systemBack = async () => {
    const mode = await frame.evaluate(() => document.body.dataset.stageHistory || "");
    if (mode === "history") {
      await frame.evaluate(() => history.back());
      return;
    }
    await frame.evaluate(async () => {
      const stage = await import(new URL("./shell-phone-stage.js?v=home-20260813a", location.href).href);
      stage.closeTopStageLayer(document);
    });
  };

  await longPressDockApp(frame);
  await sleep(SURFACE_SETTLE_MS);
  const menuBefore = await read();
  await systemBack();
  await sleep(SHEET_SETTLE_MS);
  const menuAfter = await read();
  if (menuAfter.menuOpen) {
    await pressEscape(frame);
  }

  const assistantReady = await read();
  if (!assistantReady.assistantOpen) {
    await frame.evaluate(() => document.querySelector("#assistant-toggle")?.click());
    await sleep(ASSISTANT_OPEN_WAIT_MS);
  }
  // The welcome sheet in front of the room: Back takes the sheet first.
  await frame.evaluate(() => {
    const welcome = document.querySelector("#setup-sheet");
    if (welcome) {
      welcome.classList.remove("is-yielded");
      welcome.hidden = false;
    }
  });
  await sleep(SHEET_SETTLE_MS);
  const welcomeBefore = await read();
  await systemBack();
  await sleep(SHEET_SETTLE_MS);
  const welcomeAfter = await read();

  const assistantBefore = await read();
  await systemBack();
  await sleep(ASSISTANT_OPEN_WAIT_MS);
  const assistantAfter = await read();
  if (assistantAfter.assistantOpen) {
    await frame.evaluate(() => document.querySelector("#assistant-toggle")?.click());
    await sleep(ASSISTANT_OPEN_WAIT_MS);
  }

  // Every registered Back layer: open it alone from bare Home, Back once, and
  // exactly that layer must close (none open before or after but it) with Home
  // still here. Every Home host surface must be a registered layer, and every
  // registered layer needs an opener below, so Back cannot silently miss one.
  const registry = await frame.evaluate(async () => {
    const popovers = await import(new URL("./shell-popovers.js?v=home-20260813a", location.href).href);
    const stage = await import(new URL("./shell-phone-stage.js?v=home-20260813a", location.href).href);
    const layers = popovers.registeredStageLayers().map(({ id, selector }) => ({ id, selector }));
    const registered = new Set(layers.map((layer) => document.querySelector(layer.selector)).filter(Boolean));
    const unregistered = stage.LAYER_HOST_SELECTORS
      .filter((selector) => document.querySelector(selector) && !registered.has(document.querySelector(selector)));
    return { layers, unregistered };
  });
  const layers = [];
  for (const { id, selector } of registry.layers) {
    const presence = await frame.evaluate((selector) => {
      const base = selector.replace(/:not\(\[hidden\]\)$/, "");
      const menubar = document.querySelector("#toolbar-menubar");
      return {
        present: Boolean(document.querySelector(base)),
        // App menus live in the bar's menubar, which a phone never shows.
        phoneHidden: base === ".toolbar-menu-popover" && Boolean(menubar) && menubar.getClientRects().length === 0,
      };
    }, selector);
    if (presence.phoneHidden) {
      layers.push({ id, selector, present: true, phoneHidden: true });
      continue;
    }
    if (!presence.present && !LAYER_OPENERS[id]?.creates) {
      layers.push({ id, selector, present: false });
      continue;
    }
    const opener = LAYER_OPENERS[id];
    if (!opener) {
      layers.push({ id, selector, present: true, opener: false });
      continue;
    }
    await opener.open(frame);
    await sleep(opener.wait ?? SHEET_SETTLE_MS);
    const before = await readLayer(frame, id);
    await systemBack();
    const after = await waitLayerClosed(frame, id);
    await sleep(SHEET_SETTLE_MS);
    const settled = await readLayer(frame, id);
    await opener.cleanup?.(frame);
    layers.push({ id, selector, present: true, opener: true, before, after: { ...after, windows: settled.windows } });
  }

  // Two layers at once: the yielded welcome card under Mission Control. Back
  // closes Mission Control, the one on top, and leaves the card alone.
  await openTarget(frame, "library");
  await setWelcome(frame, { open: true, yielded: true });
  await callShellModule(frame, "shell-expose.js", "openExpose");
  await sleep(SHEET_SETTLE_MS);
  const overviewBefore = { overview: await readLayer(frame, "mission-control"), welcome: await readLayer(frame, "setup-sheet") };
  await systemBack();
  await waitLayerClosed(frame, "mission-control");
  const overviewAfter = { overview: await readLayer(frame, "mission-control"), welcome: await readLayer(frame, "setup-sheet") };
  await setWelcome(frame, { open: false });
  await sleep(SHEET_SETTLE_MS);

  // A fullscreen Space with the bar's identity menu open: Back closes the
  // menu and nothing else; the Space stays fullscreen.
  const fullscreenSetup = await frame.evaluate(async () => {
    const core = await import(new URL("./shell-core.js?v=home-20260813a", location.href).href);
    const stages = await import(new URL("./shell-stages.js?v=home-20260813a", location.href).href);
    const id = core.shellState.activeWindowId;
    if (!id) {
      return null;
    }
    stages.enterFullscreenStage(id);
    return { id };
  });
  await sleep(SHEET_SETTLE_MS);
  const readFullscreen = () => frame.evaluate(async (id) => {
    const stages = await import(new URL("./shell-stages.js?v=home-20260813a", location.href).href);
    const active = stages.getActiveStageId();
    return { activeStage: active, fullscreen: Boolean(id) && !stages.isDesktopSpace(active) };
  }, fullscreenSetup?.id || null);
  await LAYER_OPENERS["identity-menu"].open(frame);
  await sleep(SHEET_SETTLE_MS);
  const fullscreenBefore = { ...(await readFullscreen()), menu: await readLayer(frame, "identity-menu") };
  await systemBack();
  await waitLayerClosed(frame, "identity-menu");
  await sleep(SHEET_SETTLE_MS);
  const fullscreenAfter = { ...(await readFullscreen()), menu: await readLayer(frame, "identity-menu") };
  if (fullscreenSetup) {
    await frame.evaluate(async (id) => {
      const stages = await import(new URL("./shell-stages.js?v=home-20260813a", location.href).href);
      stages.exitFullscreenStage(id);
    }, fullscreenSetup.id);
    await sleep(SHEET_SETTLE_MS);
  }

  await openTarget(frame, "library");
  await openTarget(frame, "documents");
  const spaceSetup = await frame.evaluate(() => {
    const active = document.querySelector(".window.window-active");
    const other = Array.from(document.querySelectorAll(".window")).find((node) => node !== active);
    if (!active || !other) {
      return null;
    }
    active.dataset.spaceVisible = "false";
    other.classList.remove("hidden");
    other.dataset.spaceVisible = "true";
    return { active: active.dataset.target, other: other.dataset.target };
  });
  await systemBack();
  await sleep(SHEET_SETTLE_MS);
  const spaceAfter = await read();
  await frame.evaluate(() => {
    for (const node of document.querySelectorAll(".window")) {
      node.classList.remove("hidden");
      node.dataset.spaceVisible = "true";
    }
  });
  for (const target of ["documents", "library"]) {
    if (await frame.locator(`.window[data-target="${target}"]`).count()) {
      await closeWindow(frame, target);
    }
  }
  return {
    menuBefore, menuAfter, welcomeBefore, welcomeAfter, assistantBefore, assistantAfter,
    unregistered: registry.unregistered, layers,
    overviewBefore, overviewAfter, fullscreenSetup, fullscreenBefore, fullscreenAfter,
    spaceSetup, spaceAfter,
  };
}

// One registered layer as Back sees it: its own reported state (open while it
// is on screen at all, leaving included), and the layers Back would count.
function readLayer(frame, id) {
  return frame.evaluate(async (id) => {
    const popovers = await import(new URL("./shell-popovers.js?v=home-20260813a", location.href).href);
    const stage = await import(new URL("./shell-phone-stage.js?v=home-20260813a", location.href).href);
    const layer = popovers.registeredStageLayers().find((entry) => entry.id === id);
    const state = layer ? stage.stageLayerState(layer, document) : "closed";
    return {
      home: location.href.includes("/apps/home-gui/"),
      state,
      open: state !== "closed",
      openLayers: stage.openStageLayers(document).map((entry) => entry.id),
      windows: Array.from(document.querySelectorAll(".window")).filter((node) => !node.classList.contains("hidden")).length,
    };
  }, id);
}

// Back settles when the layer's hide has really finished.
async function waitLayerClosed(frame, id) {
  const startedAt = Date.now();
  let state = await readLayer(frame, id);
  while (state.open && Date.now() - startedAt < LAYER_CLOSE_WAIT_MS) {
    await sleep(100);
    state = await readLayer(frame, id);
  }
  return state;
}
const LAYER_CLOSE_WAIT_MS = 2500;

function setWelcome(frame, { open, yielded = false }) {
  return frame.evaluate(({ open, yielded }) => {
    const sheet = document.querySelector("#setup-sheet");
    if (!sheet) {
      return;
    }
    sheet.classList.toggle("is-yielded", Boolean(open && yielded));
    sheet.hidden = !open;
  }, { open, yielded });
}

async function callShellModule(frame, file, call) {
  await frame.evaluate(async ({ file, call }) => {
    const module = await import(new URL(`./${file}?v=home-20260813a`, location.href).href);
    module[call]();
  }, { file, call });
}

// How a person opens each registered Back layer, keyed by its layer id.
// `creates`: the surface is built on open, so it is absent until then.
const LAYER_OPENERS = Object.freeze({
  launcher: { open: (frame) => callShellModule(frame, "shell-surface.js", "showLauncher") },
  "context-menu": { open: (frame) => longPressDockApp(frame) },
  "notification-center": { open: (frame) => callShellModule(frame, "shell-notifications.js", "toggleNotificationCenter") },
  "control-centre": { open: (frame) => callShellModule(frame, "shell-control-centre.js", "showControlCentre") },
  "wallet-rail": { open: (frame) => callShellModule(frame, "shell-wallet-rail.js", "showWalletRail") },
  "inbox-rail": { open: (frame) => callShellModule(frame, "shell-inbox-rail.js", "showInboxRail") },
  "setup-sheet": { open: (frame) => setWelcome(frame, { open: true }) },
  spotlight: { open: (frame) => callShellModule(frame, "shell-spotlight.js", "showSpotlight") },
  shortcuts: { open: (frame) => callShellModule(frame, "shell-keyboard.js", "toggleShortcutsOverlay") },
  about: { open: (frame) => frame.evaluate(() => document.querySelector("#identity-menu-about")?.click()) },
  "identity-menu": { open: (frame) => frame.evaluate(() => document.querySelector("#toolbar-home")?.click()) },
  "mission-control": { creates: true, open: (frame) => callShellModule(frame, "shell-expose.js", "openExpose") },
  // Peeking the Dock needs an app on the stage (the Dock is tucked under it);
  // Back must take the peek and leave the app where it is.
  "dock-peek": {
    open: async (frame) => {
      await openTarget(frame, "system");
      await callShellModule(frame, "shell-phone-dock.js", "peekPhoneDock");
    },
    cleanup: (frame) => closeWindow(frame, "system"),
  },
  "home-edit": { open: (frame) => callShellModule(frame, "shell-phone-home-edit.js", "enterPhoneHomeEdit") },
  assistant: {
    wait: ASSISTANT_OPEN_WAIT_MS,
    open: (frame) => frame.evaluate(() => document.querySelector("#assistant-toggle")?.click()),
  },
});

function phoneBackFailures(run) {
  if (!run.back) {
    return [];
  }
  const {
    menuBefore, menuAfter, welcomeBefore, welcomeAfter, assistantBefore, assistantAfter,
    unregistered, layers, overviewBefore, overviewAfter, fullscreenSetup, fullscreenBefore, fullscreenAfter,
    spaceSetup, spaceAfter,
  } = run.back;
  const label = `${run.engine}/${run.profile}/back`;
  const failures = [];
  if (!menuBefore.menuOpen) {
    failures.push(`${label}: a long-press must open the menu before Back. Got ${JSON.stringify(menuBefore)}`);
  }
  if (!menuAfter.home || menuAfter.menuOpen) {
    failures.push(`${label}: Back must close the long-press menu and leave Home. Got ${JSON.stringify(menuAfter)}`);
  }
  if (!welcomeBefore.welcomeOpen || !welcomeBefore.assistantOpen) {
    failures.push(`${label}: the welcome sheet must be open over the Assistant before Back. Got ${JSON.stringify(welcomeBefore)}`);
  }
  if (!welcomeAfter.home || welcomeAfter.welcomeOpen || !welcomeAfter.assistantOpen) {
    failures.push(`${label}: Back must close the welcome sheet first and keep the Assistant. Got ${JSON.stringify(welcomeAfter)}`);
  }
  if (!assistantBefore.assistantOpen) {
    failures.push(`${label}: Assistant must be open before Back. Got ${JSON.stringify(assistantBefore)}`);
  }
  if (!assistantAfter.home || assistantAfter.assistantOpen) {
    failures.push(`${label}: Back must close Assistant and leave Home. Got ${JSON.stringify(assistantAfter)}`);
  }
  for (const selector of unregistered) {
    failures.push(`${label}: Home surface ${selector} is not a registered Back layer (registerStageLayer)`);
  }
  for (const layer of layers) {
    if (!layer.present || layer.phoneHidden) {
      continue;
    }
    if (!layer.opener) {
      failures.push(`${label}: Back layer ${layer.id} has no opener in LAYER_OPENERS`);
      continue;
    }
    if (!layer.before.open || layer.before.openLayers.join() !== layer.id) {
      failures.push(`${label}: ${layer.id} must be the only open layer before Back. Got ${JSON.stringify(layer.before)}`);
    }
    if (!layer.after.home || layer.after.open || layer.after.openLayers.length !== 0
      || layer.after.windows !== layer.before.windows) {
      failures.push(`${label}: one Back must close ${layer.id} alone and leave Home. Got ${JSON.stringify({ before: layer.before, after: layer.after })}`);
    }
  }
  if (!overviewBefore.overview.open || !overviewBefore.welcome.open) {
    failures.push(`${label}: Mission Control must be open over the yielded welcome card before Back. Got ${JSON.stringify(overviewBefore)}`);
  }
  if (!overviewAfter.overview.home || overviewAfter.overview.open || !overviewAfter.welcome.open) {
    failures.push(`${label}: Back must close Mission Control and keep the welcome card. Got ${JSON.stringify(overviewAfter)}`);
  }
  if (!fullscreenSetup || !fullscreenBefore.fullscreen || !fullscreenBefore.menu.open) {
    failures.push(`${label}: a fullscreen Space with the identity menu open is required before Back. Got ${JSON.stringify({ fullscreenSetup, fullscreenBefore })}`);
  } else if (!fullscreenAfter.menu.home || fullscreenAfter.menu.open || !fullscreenAfter.fullscreen
    || fullscreenAfter.activeStage !== fullscreenBefore.activeStage) {
    failures.push(`${label}: Back must close the identity menu and keep the fullscreen Space. Got ${JSON.stringify({ fullscreenBefore, fullscreenAfter })}`);
  }
  if (!spaceSetup) {
    failures.push(`${label}: two windows are required to check an off-Space Back`);
    return failures;
  }
  const active = spaceAfter.windows.find((entry) => entry.target === spaceSetup.active);
  const other = spaceAfter.windows.find((entry) => entry.target === spaceSetup.other);
  if (!spaceAfter.home || !active || active.hidden || !other || !other.hidden) {
    failures.push(`${label}: Back must minimise the window on this Space, not the hidden one. Got ${JSON.stringify({ spaceSetup, spaceAfter })}`);
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

// The host page's viewport meta content as the browser has it now.
function hostViewportMeta(page) {
  return page.evaluate(() => document.querySelector('meta[name="viewport"]')?.getAttribute("content") ?? null);
}

function viewportMetaFits(content, phone) {
  return phone ? Boolean(content?.includes(PHONE_VIEWPORT_FIT)) : content === BASE_VIEWPORT_CONTENT;
}

// The host rewrites the meta when its own form factor changes; give it a
// moment, then report what is there.
async function settledHostViewportMeta(page, phone) {
  const startedAt = Date.now();
  let content = await hostViewportMeta(page);
  while (!viewportMetaFits(content, phone) && Date.now() - startedAt < SURFACE_SETTLE_MS) {
    await sleep(50);
    content = await hostViewportMeta(page);
  }
  return content;
}

function viewportMetaFailures(run) {
  const phone = run.profile.startsWith("phone");
  if (viewportMetaFits(run.viewportMeta, phone)) {
    return [];
  }
  return [`${run.engine}/${run.profile}/viewport-meta: the host page's viewport meta must ${phone ? `add ${PHONE_VIEWPORT_FIT} on a phone` : `be "${BASE_VIEWPORT_CONTENT}" off a phone`}. Got ${JSON.stringify(run.viewportMeta)}`];
}

function sourceTruths() {
  const homeIndexPath = "capsules/home/browser/index.html";
  const guiIndexPath = "capsules/home-gui/browser/index.html";
  const guiTemplatePath = "capsules/home-gui/browser/home-gui-template.html";
  const read = (path) => readFileSync(join(repoRoot, path), "utf8");
  const guiStyle = read("capsules/home-gui/browser/style.css");
  const viewportMetas = (html) => [...html.matchAll(/<meta\s+name="viewport"\s+content="([^"]*)"/g)].map((match) => match[1]);
  // Each page ships the base content byte for byte; the template is a body
  // fragment and may carry no meta, but never another viewport.
  const baseViewport = (html, required) => {
    const metas = viewportMetas(html);
    return (required ? metas.length === 1 : metas.length <= 1) && metas.every((content) => content === BASE_VIEWPORT_CONTENT);
  };
  // A 100vh line is a fallback when the same declaration also exists with
  // 100dvh: on the next line, or in a phone-scoped rule (desktop keeps its
  // original 100vh untouched). Any other 100vh is bare and wrong on phones.
  const styleLines = guiStyle.split("\n");
  const bareViewportHeightUnits = styleLines.filter((line) =>
    line.includes("100vh") && !guiStyle.includes(line.replace("100vh", "100dvh").trim())).length;
  return {
    baseViewportMeta: {
      [homeIndexPath]: baseViewport(read(homeIndexPath), true),
      [guiIndexPath]: baseViewport(read(guiIndexPath), true),
      [guiTemplatePath]: baseViewport(read(guiTemplatePath), false),
    },
    bareViewportHeightUnits,
    viewportHeightFallbacks: (guiStyle.match(/\b100vh\b/g) || []).length - bareViewportHeightUnits,
    dynamicViewportHeightUnits: (guiStyle.match(/\b100dvh\b/g) || []).length,
    backdropFilters: (guiStyle.match(/backdrop-filter\s*:/g) || []).length,
  };
}

function sourceFailures(source) {
  return Object.entries(source.baseViewportMeta)
    .filter(([, base]) => !base)
    .map(([path]) => `source/${path}: the viewport meta must stay exactly content="${BASE_VIEWPORT_CONTENT}"; the host adds ${PHONE_VIEWPORT_FIT} at runtime on a phone only`);
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

const requestedProfiles = (process.env.HOME_PHONE_SMOKE_PROFILES || PROFILES.map((profile) => profile.id).join(","))
  .split(",")
  .map((value) => value.trim())
  .filter(Boolean);

const origin = await listen();
mkdirSync(outputRoot, { recursive: true });
const report = { generatedAt: new Date().toISOString(), origin, source: sourceTruths(), runs: [], closedStreamRuns: [], stageResetRuns: [] };
const failures = sourceFailures(report.source);
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
        if (!requestedProfiles.includes(profile.id)) {
          continue;
        }
        const run = await runProfile(browser, engineId, profile, origin);
        report.runs.push(run);
        failures.push(...shellFailures(run), ...viewportMetaFailures(run), ...phoneDockFailures(run), ...phoneStageFailures(run), ...phoneKeyboardFailures(run), ...phoneSpotlightFailures(run), ...phoneSheetHandleFailures(run), ...phoneLinkFailures(run), ...phoneTouchMenuFailures(run), ...phoneHomeFailures(run), ...phoneHomeEditFailures(run), ...phoneBackFailures(run), ...promptCloseFailures(run), ...backRaceFailures(run));
        console.log(`[home-phone-layout] ${engineId}/${profile.id} ${summarize(run)}`);
      }
      const closedStream = await runClosedStream(browser, engineId, origin);
      report.closedStreamRuns.push(closedStream);
      failures.push(...closedStreamFailures(closedStream));
      console.log(`[home-phone-layout] ${engineId}/phone-portrait/closed-stream tile_redrawn_after_ms=${closedStream.tileRemovedAfterMs} menu=${closedStream.menu.open ? closedStream.menu.title : "shut"} windows=${closedStream.windowsBefore}->${closedStream.windowsAfter}`);
      const stageReset = await runStageReset(browser, engineId, origin);
      report.stageResetRuns.push(stageReset);
      failures.push(...stageResetFailures(stageReset));
      console.log(`[home-phone-layout] ${engineId}/stage-reset boot=${stageReset.bootFactor} fullscreen_to_phone=${stageReset.fullscreenToPhone.reset.runs}x/${stageReset.fullscreenToPhone.reset.durationMs}ms back=${stageReset.fullscreenToPhone.afterBack ? "history" : "skipped"} library_across_stages=${stageReset.libraryAcrossStages ? "probed" : "skipped (buttons)"}`);
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
      `viewport_meta=${Object.values(source.baseViewportMeta).every(Boolean) ? "base" : "changed"} ` +
      `bare_100vh=${source.bareViewportHeightUnits} backdrop_filters=${source.backdropFilters} ` +
      `boot_retries=${report.runs.reduce((sum, run) => sum + run.bootRetries, 0)} ` +
      `${skippedEngines.length ? `skipped=${skippedEngines.join(",")} (not installed) ` : ""}report=${join(outputRoot, "report.json")}`,
  );
} finally {
  server.closeAllConnections();
  await new Promise((resolveClose) => server.close(resolveClose));
}
