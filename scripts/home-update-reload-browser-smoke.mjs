#!/usr/bin/env node
// After a Home update only System reloads itself; the Home page, the shell and
// open windows keep the code they loaded. This runs the real Home page and
// Desktop shell: when the Runtime comes back on another version, one bar asks
// for a reload and its Reload button reloads the page. The bar stays away on
// the first load, after a restart on the same version, while a Home sheet is
// shown and when the person is signed out. It then runs the real System: the
// update finishing reloads System, which says to reload Home.
// Screenshots: HOME_UPDATE_RELOAD_SCREENSHOTS=<dir> writes reload-bar-desktop.png
// and reload-bar-phone.png there.
import assert from "node:assert/strict";
import { createServer } from "node:http";
import { once } from "node:events";
import { readFile } from "node:fs/promises";
import { extname, join, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { chromium, jsonResponse, makeAppearanceRecord, makeSystemSummary, startSystemFixtureServer }
  from "./system-uiux-fixture.mjs";

const root = resolve(fileURLToPath(new URL("../", import.meta.url)));
const screenshots = process.env.HOME_UPDATE_RELOAD_SCREENSHOTS || "";
const BAR_TEXT = "Home was updated. Reload to finish.";
const principal = "did:elastos:fixture-update-reload";

// What the Runtime serves now; a "restart" drops every open event stream.
const runtime = { version: "0.8.0", signedIn: true };
const counts = { page: 0, summary: 0 };
const streams = new Set();
const errors = [];

const summary = () => ({
  home: { route: "/apps/home/", attach_kind: "iframe" },
  app: { id: "home", route: "/apps/home/" },
  identity: {
    device_did: null,
    profile_readiness: { schema: "elastos.profile.readiness/v1", status: "ready" },
    recovery_readiness: { schema: "elastos.recovery.readiness/v1", status: "ready" },
  },
  authority: runtime.signedIn
    ? { signed_in: true, principal_id: principal, session_id: "fixture-session", proof_binding_id: "fixture-proof", wallet_connected: false }
    : { signed_in: false },
  browser_state: {
    schema: "elastos.home.browser-state/v1", principal_id: principal, localhost_root: "localhost://fixture",
    layout: null, recent_targets: [], session: { browser_context_id: "", root_shell: "home-gui", windows: [] },
  },
  active_shell: {
    schema: "elastos.home.active-shell/v1", active: "home-gui",
    candidates: [{ name: "home-gui", title: "Desktop", description: "Fixture Desktop", route: "/apps/home-gui/",
      role: "shell", trust_state: "trusted", launchable: true }],
  },
  appearance: makeAppearanceRecord({ revision: 0, accent: "blue", dock_auto_hide: false }),
  runtime: { running: true, kind: "local", version: runtime.version, api_url: null, pid: null, running_capsules: [], note: null },
  site: { staged: false, root_uri: "", path: "", active_release: null, active_channel: null, active_bundle_cid: null, release_count: 0 },
  room: {
    room_slug: "fixture", title: "", member_count: 0, active_member_count: 0, pending_count: 0, active_session_count: 0,
    latest_request_name: null, latest_request_device: null, local_runtime_did: null, local_runtime_role: null,
    canonical_hosted_guest_url: null, ephemeral_hosted_guest_url: null,
    browser_access_allowed: true, browser_access_block_reason: null, pending_requests: [], active_sessions: [],
  },
  people: { schema: "elastos.people.contacts/v1", contact_count: 0, contacts: [], service_offer_count: 0, service_offers: [] },
  services: {
    schema: "elastos.runtime.services/v1", local_offer_count: 0, remote_offer_count: 0,
    available_local_offer_count: 0, available_remote_offer_count: 0, local_offers: [], remote_offers: [],
    available_local_offers: [], available_remote_offers: [], grant_model: "principal_scoped_provider_grant",
    carrier_contract: "", capsule_contract: "",
  },
  notifications: { unread_count: 0, attention_count: 0, entries: [] },
  desktop_objects: { schema: "elastos.home.desktop-objects/v1", uri: "localhost://fixture/Desktop", objects: [], stale: false },
  capsule_catalog: {
    schema: "elastos.capsules.catalog/v1", capsules: [],
    counts: { total: 0, installed: 0, launchable: 0, interfaces: 0, methods: 0, apps: 0, viewers: 0, providers: 0, content: 0, shell: 0 },
    policy: { install_state: "", install_note: "", payment_state: "", payment_note: "", drm_state: "", drm_note: "" },
  },
  capsule_interfaces: {
    schema: "elastos.capsules.interfaces/v1", interfaces: [],
    counts: { capsules: 0, interfaces: 0, methods: 0, executable_methods: 0 },
    policy: { descriptor_state: "", descriptor_note: "", invocation_state: "", invocation_note: "" },
  },
  targets: [["documents", "Documents"], ["chat-room", "Chat"], ["library", "Library"], ["system", "System"]]
    .map(([target, title]) => ({ target, title, description: title, role: "app", target_kind: "app", attach_kind: "iframe" })),
});

// The Desktop shell is an opaque-sandboxed frame, so its module loads are CORS requests.
const HEADERS = { "access-control-allow-origin": "*", "cache-control": "no-store" };
const json = (res, value, status = 200) => {
  res.writeHead(status, { ...HEADERS, "content-type": "application/json" });
  res.end(JSON.stringify(value));
};

async function api(req, res, url) {
  const route = `${req.method} ${url.pathname}`;
  if (req.method === "OPTIONS") {
    res.writeHead(204, { ...HEADERS, "access-control-allow-headers": "content-type,x-elastos-home-token",
      "access-control-allow-methods": "GET,POST,OPTIONS" });
    res.end();
    return;
  }
  if (route === "POST /api/auth/sessions/refresh") return json(res, { home_token: "fixture-home-token" });
  if (route === "GET /api/apps/home/summary") { counts.summary += 1; return json(res, summary()); }
  if (route === "POST /api/apps/home/runtime/ensure") return json(res, { running: true });
  if (route === "GET /api/apps/home/events") {
    return json(res, { schema: "elastos.home.events/v1", cursor: "fixture-cursor", events: [], retry_after_ms: 60_000 });
  }
  if (route === "GET /api/apps/home/events/stream") {
    res.writeHead(200, { "content-type": "text/event-stream", "cache-control": "no-store" });
    res.write(": open\n\n");
    streams.add(res);
    res.on("close", () => streams.delete(res));
    return;
  }
  if (route === "POST /api/apps/home/launch") {
    const input = JSON.parse(await body(req) || "{}");
    assert.equal(input.target, "home-gui", "only the Desktop shell launches here");
    const query = new URLSearchParams({ ...input.query, home_origin: String(input.query?.home_origin || "") });
    return json(res, { target: "home-gui", title: "Desktop", attach_kind: "iframe", launch_status: "launched",
      route: `/apps/home-gui/?${query}#home_token=fixture-shell-token` });
  }
  if (route === "POST /api/apps/home/state") { await body(req); return json(res, { saved: true }); }
  throw new Error(`Unexpected API request: ${route}`);
}

async function body(req) {
  const chunks = [];
  for await (const chunk of req) chunks.push(chunk);
  return Buffer.concat(chunks).toString("utf8");
}

const TYPES = { ".html": "text/html", ".js": "text/javascript", ".mjs": "text/javascript", ".css": "text/css",
  ".svg": "image/svg+xml", ".png": "image/png", ".webp": "image/webp", ".woff2": "font/woff2", ".json": "application/json",
  ".webmanifest": "application/manifest+json" };

const server = createServer(async (req, res) => {
  const url = new URL(req.url, "http://fixture");
  try {
    if (url.pathname.startsWith("/api/")) { await api(req, res, url); return; }
    if (url.pathname === "/favicon.ico") { res.writeHead(204); res.end(); return; }
    const match = url.pathname.match(/^\/apps\/(home|home-gui)\/(.*)$/);
    assert.ok(match, `Unexpected asset: ${url.pathname}`);
    const base = join(root, "capsules", match[1], "browser");
    const path = resolve(base, decodeURIComponent(match[2]) || "index.html");
    assert.ok(path.startsWith(`${base}${sep}`), `Asset outside its capsule: ${url.pathname}`);
    if (match[1] === "home" && !match[2]) counts.page += 1;
    res.writeHead(200, { ...HEADERS, "content-type": TYPES[extname(path)] || "application/octet-stream" });
    res.end(await readFile(path));
  } catch (error) {
    errors.push(String(error?.message || error));
    if (!res.headersSent) json(res, { error: "fixture failure" }, 500);
    else res.end();
  }
});

// The Runtime restarts (as after an update) on `version`: open event streams
// drop, and Home reconnects and reads the summary again.
async function restart(page, version, { signedIn = true } = {}) {
  Object.assign(runtime, { version, signedIn });
  const before = counts.summary;
  for (const stream of streams) stream.destroy();
  await waitUntil(() => counts.summary > before, `Home read the summary after the restart on ${version}`);
  // Let the host apply the summary it just read.
  await page.waitForTimeout(300);
}

async function waitUntil(check, label, timeout = 15_000) {
  const deadline = Date.now() + timeout;
  while (!(await check())) {
    assert.deepEqual(errors, [], label);
    if (Date.now() > deadline) throw new Error(`Timed out: ${label}`);
    await new Promise((done) => setTimeout(done, 50));
  }
}

async function openHome(browser, origin, options) {
  const context = await browser.newContext(options);
  const page = await context.newPage();
  page.on("pageerror", (error) => errors.push(`page: ${error}`));
  await page.goto(`${origin}/apps/home/`);
  await page.waitForFunction(() => document.body?.dataset.homeStatus === "ready"
    && document.querySelector("#active-shell-frame")?.hidden === false, null, { timeout: 15_000 });
  const shell = () => page.frames().find((frame) => frame.url().includes("/apps/home-gui/"));
  await waitUntil(() => shell(), "Home mounted the Desktop shell");
  await shell().waitForFunction(() => document.body?.dataset.homeStatus === "ready", null, { timeout: 15_000 });
  await waitUntil(() => streams.size > 0, "Home opened its event stream");
  return page;
}

const bar = (page) => page.locator("#home-update-bar");

async function assertNoBar(page, label) {
  assert.equal(await bar(page).isVisible(), false, label);
}

async function assertOneBar(page, label) {
  assert.equal(await page.locator("#home-update-bar").count(), 1, `${label}: one bar`);
  assert.equal(await bar(page).isVisible(), true, `${label}: the bar shows`);
  assert.equal((await bar(page).locator("p").textContent()).trim(), BAR_TEXT, label);
  assert.deepEqual(await bar(page).getByRole("button").allTextContents(), ["Reload"], `${label}: one Reload button`);
  const box = await bar(page).boundingBox();
  const viewport = page.viewportSize();
  assert.ok(box.x >= 0 && box.y >= 0 && box.x + box.width <= viewport.width && box.y + box.height <= viewport.height,
    `${label}: the bar fits the screen ${JSON.stringify({ box, viewport })}`);
  // The shell frame fills the page, so its boxes share the page's coordinates.
  const shell = page.frameLocator("#active-shell-frame");
  for (const chrome of ["header.toolbar", "footer.taskbar"]) {
    const other = await shell.locator(chrome).boundingBox();
    if (!other) continue;
    const apart = box.y + box.height <= other.y || other.y + other.height <= box.y
      || box.x + box.width <= other.x || other.x + other.width <= box.x;
    assert.ok(apart, `${label}: the bar leaves ${chrome} uncovered ${JSON.stringify({ box, other })}`);
  }
}

let browser, stage = "launch";
let systemFixture = null;
try {
  server.listen(0, "127.0.0.1");
  await once(server, "listening");
  const origin = `http://127.0.0.1:${server.address().port}`;
  browser = await chromium.launch({ headless: true, ...(process.env.BRAVE_BIN ? { executablePath: process.env.BRAVE_BIN } : {}) });

  stage = "first load";
  const page = await openHome(browser, origin, { viewport: { width: 1280, height: 800 } });
  await assertNoBar(page, "no bar on the first load");

  stage = "restart on the same version";
  await restart(page, "0.8.0");
  await assertNoBar(page, "no bar after a restart on the same version");

  stage = "restart on a new version";
  await restart(page, "0.8.1");
  await assertOneBar(page, "after the update");
  if (screenshots) await page.screenshot({ path: join(screenshots, "reload-bar-desktop.png") });
  await restart(page, "0.8.1");
  await assertOneBar(page, "after another reconnect");

  stage = "a Home sheet on screen";
  for (const sheet of ["#home-link-prompt", "#home-clipboard-prompt"]) {
    await page.locator(sheet).evaluate((node) => { node.hidden = false; });
    await assertNoBar(page, `the bar gives way to ${sheet}`);
    await page.locator(sheet).evaluate((node) => { node.hidden = true; });
    await assertOneBar(page, `after ${sheet} closes`);
  }

  stage = "Reload";
  const pagesBefore = counts.page;
  await page.evaluate(() => { window.__beforeReload = true; });
  await bar(page).getByRole("button", { name: "Reload" }).click();
  await page.waitForFunction(() => window.__beforeReload !== true && document.body?.dataset.homeStatus === "ready",
    null, { timeout: 15_000 });
  assert.equal(counts.page, pagesBefore + 1, "Reload loads the Home page again");
  await page.waitForFunction(() => document.querySelector("#active-shell-frame")?.hidden === false, null, { timeout: 15_000 });
  await assertNoBar(page, "no bar once the page runs the updated code");

  stage = "signed out";
  await restart(page, "0.8.2", { signedIn: false });
  await assertNoBar(page, "no bar while signed out");
  await page.context().close();

  stage = "phone";
  Object.assign(runtime, { version: "0.8.0", signedIn: true });
  const phone = await openHome(browser, origin,
    { viewport: { width: 390, height: 844 }, deviceScaleFactor: 3, isMobile: true, hasTouch: true });
  await assertNoBar(phone, "no bar on the first phone load");
  await restart(phone, "0.8.1");
  await assertOneBar(phone, "phone after the update");
  if (screenshots) await phone.screenshot({ path: join(screenshots, "reload-bar-phone.png") });
  await phone.context().close();

  stage = "System after its update";
  const controller = { id: "a".repeat(32), current_version: "0.8.0", new_version: "0.8.1" };
  let updatePhase = "restarting";
  systemFixture = await startSystemFixtureServer({
    title: "System update reload fixture",
    async onApiRequest({ response, url }) {
      if (url.pathname !== "/api/apps/system/summary") return false;
      const done = updatePhase === "updated";
      jsonResponse(response, {
        ...makeSystemSummary(makeAppearanceRecord()),
        runtime_update: {
          configured: true, available: false, can_apply: false, current_version: done ? "0.8.1" : "0.8.0",
          controller: { ...controller, phase: updatePhase, message: done ? "Home is up to date." : "Restarting." },
        },
      });
      return true;
    },
  });
  const systemPage = await (await browser.newContext({ viewport: { width: 1100, height: 760 } })).newPage();
  systemPage.on("pageerror", (error) => errors.push(`System page: ${error}`));
  await systemPage.goto(`${systemFixture.baseUrl}/fixture?settings=about`);
  const system = systemPage.frameLocator("#system-frame");
  const status = system.locator("#runtime-update-status");
  await status.filter({ hasText: "Restarting to finish the update." }).waitFor({ timeout: 10_000 });
  // The restarted Home answers on the new version: System reloads itself.
  updatePhase = "updated";
  await status.filter({ hasText: "Updated to 0.8.1. Reload Home to finish." }).waitFor({ timeout: 15_000 });
  // Opened again (a new navigation), System just reports the result.
  await systemPage.reload();
  await status.filter({ hasText: "Home is up to date." }).waitFor({ timeout: 10_000 });

  assert.deepEqual(errors, []);
  assert.deepEqual(systemFixture.requestFailures, []);
  console.log("PASS Home update reload: one Reload bar after a version change (not on first load, same version, under a sheet or signed out), Reload reloads Home; System reloaded by its update says to reload Home");
} catch (error) {
  console.error(`Home update reload smoke failed at ${stage}`);
  console.error(JSON.stringify({ errors, failures: systemFixture?.requestFailures || [] }, null, 1));
  throw error;
} finally {
  await browser?.close();
  await systemFixture?.close();
  for (const stream of streams) stream.destroy();
  server.close();
}
