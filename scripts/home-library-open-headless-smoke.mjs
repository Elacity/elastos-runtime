#!/usr/bin/env node
// Library opens files through the real Home shell host and Home GUI in Chromium:
// a ZIP opens in Archive (which is not in the Home summary) with its title, only
// for the open Library asked for; Open With still opens Documents and the GBA
// emulator; the "No app" offer and a shared file's Download file save the
// bytes, and the Home tab never closes. The fixture gateway sends the real
// gateway's COOP/COEP/CORP and opaque-origin CORS headers.

import { createServer } from "node:http";
import { existsSync, readFileSync } from "node:fs";
import { readFile } from "node:fs/promises";
import { extname, isAbsolute, join, relative, resolve, sep } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const repoRoot = fileURLToPath(new URL("../", import.meta.url));
// Run against another checkout's capsules (for example the pre-fix commit).
const capsuleRoot = resolve(process.env.HOME_LIBRARY_OPEN_CAPSULE_ROOT || join(repoRoot, "capsules"));
const principalId = "did:elastos:fixture-library-open";
const principalRoot = "localhost://Users/fixture";
const documentsUri = `${principalRoot}/Documents`;
const sharedCid = "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3w2pz3hftb5xg3mh5g4xq7y5u";
const sharedBytes = Buffer.from("%PDF-1.4\n% shared Chat file fixture\n");
const pdfBytes = Buffer.from("%PDF-1.4\n% local Library PDF fixture\n");
const STUB_TARGETS = new Set(["archive-manager", "documents", "gba-emulator"]);
const TITLES = { "home-gui": "Desktop", library: "Library", documents: "Documents", "gba-emulator": "GBA Emulator", "archive-manager": "Archive" };

const state = { launches: [], unknown: [], errors: [], downloads: [] };
const pageErrors = [];
const failedRequests = [];
const contentNavigations = [];
let launchSerial = 0;

function assert(condition, message, details = undefined) {
  if (condition) return;
  throw new Error(`${message}${details === undefined ? "" : `\n${JSON.stringify(details, null, 2)}`}`);
}

function isolationHeaders(req, extra = {}) {
  const headers = {
    "cache-control": "no-store",
    "cross-origin-opener-policy": "same-origin",
    "cross-origin-embedder-policy": "require-corp",
    ...extra,
  };
  // The gateway front door answers opaque app frames with CORS for public
  // reads and launch-token API calls.
  if (req.headers.origin === "null") {
    headers["access-control-allow-origin"] = "null";
    headers["access-control-expose-headers"] = "etag,content-disposition,x-elastos-request-id,x-elastos-transfer-receipt";
    headers.vary = "Origin";
  }
  return headers;
}

function send(req, res, status, body, headers) {
  res.writeHead(status, isolationHeaders(req, { "content-length": body.length, ...headers }));
  res.end(body);
}

function json(req, res, status, value) {
  send(req, res, status, Buffer.from(JSON.stringify(value)), { "content-type": "application/json; charset=utf-8" });
}

function contentType(path) {
  return {
    ".css": "text/css; charset=utf-8",
    ".html": "text/html; charset=utf-8",
    ".js": "text/javascript; charset=utf-8",
    ".mjs": "text/javascript; charset=utf-8",
    ".json": "application/json; charset=utf-8",
    ".png": "image/png",
    ".svg": "image/svg+xml",
    ".woff2": "font/woff2",
  }[extname(path)] || "application/octet-stream";
}

function staticPath(pathname) {
  for (const [prefix, root] of [
    ["/apps/home/", join(capsuleRoot, "home/browser")],
    ["/apps/home-gui/", join(capsuleRoot, "home-gui/browser")],
    ["/apps/library/", join(capsuleRoot, "library/browser")],
  ]) {
    if (!pathname.startsWith(prefix)) continue;
    const candidate = resolve(root, decodeURIComponent(pathname.slice(prefix.length)) || "index.html");
    const escaped = relative(root, candidate);
    if (escaped.startsWith(`..${sep}`) || escaped === ".." || isAbsolute(escaped)) return null;
    return candidate;
  }
  return null;
}

async function readBody(req) {
  const chunks = [];
  for await (const chunk of req) chunks.push(chunk);
  const text = Buffer.concat(chunks).toString("utf8");
  return text ? JSON.parse(text) : null;
}

function libraryObject(name, mime, viewers, extra = {}) {
  return {
    schema: "elastos.library.object/v1",
    uri: `${documentsUri}/${name}`,
    name,
    kind: "file",
    mime,
    size: extra.size || 64,
    created_at: 1780000000,
    modified_at: 1780000000,
    revision: `rev:${name}`,
    viewer: null,
    viewers,
    thumbnail_uri: null,
    availability: "local-only",
    blocked_reason: null,
    content_cid: null,
    published_cid: null,
    metadata: extra.metadata || { schema: "elastos.library.object-metadata/v1" },
    published: false,
    shared: false,
    capabilities: ["download", "rename", "move", "copy", "trash", "properties", ...(extra.capabilities || [])],
  };
}

const objects = [
  libraryObject("Bundle.zip", "application/zip", [{ id: "archive-manager", label: "Archive" }], {
    capabilities: ["extract_archive"],
    metadata: {
      schema: "elastos.library.object-metadata/v1",
      archive_support: { schema: "elastos.library.archive-support/v1", family: "zip", status: "extractable" },
    },
  }),
  libraryObject("Other.zip", "application/zip", [{ id: "archive-manager", label: "Archive" }]),
  libraryObject("Notes.txt", "text/plain", [{ id: "documents", label: "Documents" }]),
  libraryObject("Game.gba", "application/x-gba-rom", [{ id: "gba-emulator", label: "GBA Emulator", default: true }]),
  libraryObject("Letter.pdf", "application/pdf", [], { size: pdfBytes.length }),
];
const documentsFolder = {
  ...libraryObject("Documents", "inode/directory", []),
  uri: documentsUri,
  kind: "directory",
  capabilities: ["open", "list", "properties"],
};

function homeSummary() {
  return {
    home: { route: "/apps/home/", attach_kind: "iframe" },
    app: { id: "home", route: "/apps/home/" },
    identity: {
      device_did: null,
      profile_readiness: { schema: "elastos.profile.readiness/v1", status: "ready" },
      recovery_readiness: { schema: "elastos.recovery.readiness/v1", status: "ready" },
    },
    authority: { signed_in: true, principal_id: principalId, session_id: "fixture-session", proof_binding_id: "fixture-proof-binding", wallet_connected: false },
    browser_state: {
      schema: "elastos.home.browser-state/v1", principal_id: principalId, localhost_root: principalRoot, layout: null, recent_targets: [],
      session: { browser_context_id: "browser:00112233445566778899aabbccddeeff", root_shell: "home-gui", windows: [] },
    },
    active_shell: {
      schema: "elastos.home.active-shell/v1", active: "home-gui",
      candidates: [{ name: "home-gui", title: "Desktop", description: "Fixture Desktop", route: "/apps/home-gui/", role: "shell", trust_state: "trusted", launchable: true }],
    },
    appearance: {
      schema: "elastos.home.appearance/v1", revision: 0, theme: "dark", accent: "blue", accent_custom: "#4f7fff", dock_auto_hide: false, sounds: false,
      focus_mode: false, background_image_url: null, background_overlay_enabled: true, background_overlay_opacity: 0.55,
    },
    runtime: { running: true, kind: "local", version: null, api_url: null, pid: null, running_capsules: [], note: null },
    site: { staged: false, root_uri: "", path: "", active_release: null, active_channel: null, active_bundle_cid: null, release_count: 0 },
    room: {
      room_slug: "fixture", title: "", member_count: 0, active_member_count: 0, pending_count: 0, active_session_count: 0, latest_request_name: null,
      latest_request_device: null, local_runtime_did: null, local_runtime_role: null, canonical_hosted_guest_url: null, ephemeral_hosted_guest_url: null,
      browser_access_allowed: true, browser_access_block_reason: null, pending_requests: [], active_sessions: [],
    },
    people: { schema: "elastos.people.contacts/v1", contact_count: 0, contacts: [], service_offer_count: 0, service_offers: [] },
    services: {
      schema: "elastos.runtime.services/v1", local_offer_count: 0, remote_offer_count: 0, available_local_offer_count: 0, available_remote_offer_count: 0,
      local_offers: [], remote_offers: [], available_local_offers: [], available_remote_offers: [], grant_model: "principal_scoped_provider_grant",
      carrier_contract: "", capsule_contract: "",
    },
    notifications: { unread_count: 0, attention_count: 0, entries: [] },
    desktop_objects: { schema: "elastos.home.desktop-objects/v1", uri: `${principalRoot}/Desktop`, objects: [], stale: false },
    capsule_catalog: {
      schema: "elastos.capsules.catalog/v1", capsules: [],
      counts: { total: 0, installed: 0, launchable: 0, interfaces: 0, methods: 0, apps: 0, viewers: 0, providers: 0, content: 0, shell: 0 },
      policy: { install_state: "signed-app-install-pending", install_note: "", payment_state: "provider-rail-required", payment_note: "", drm_state: "provider-rail-required", drm_note: "" },
    },
    capsule_interfaces: {
      schema: "elastos.capsules.interfaces/v1", interfaces: [], counts: { capsules: 0, interfaces: 0, methods: 0, executable_methods: 0 },
      policy: { descriptor_state: "manifest-declared", descriptor_note: "", invocation_state: "runtime-gated", invocation_note: "" },
    },
    // As the gateway lists them: Archive is launchable but not a Home target.
    targets: ["library", "documents", "gba-emulator"].map((target) => ({
      target, title: TITLES[target], description: "", role: "app", target_kind: "app", attach_kind: "iframe",
    })),
  };
}

function launchResponse(origin, target, query) {
  const params = new URLSearchParams();
  for (const [key, value] of Object.entries(query || {})) params.set(key, String(value));
  params.set("home_origin", origin);
  launchSerial += 1;
  state.launches.push({ target, query });
  return {
    target,
    title: TITLES[target],
    attach_kind: "iframe",
    launch_status: "launched",
    route: `/apps/${target}/?${params}#home_token=${encodeURIComponent(`${target}-launch-${launchSerial}`)}`,
  };
}

async function handleApi(req, res, url) {
  if (req.method === "OPTIONS" && url.pathname.startsWith("/api/")) {
    res.writeHead(204, isolationHeaders(req, {
      "access-control-allow-methods": "GET, POST, PUT, DELETE, HEAD",
      "access-control-allow-headers": "content-type,x-elastos-home-token,last-event-id,if-match,if-none-match,range",
    }));
    res.end();
    return true;
  }
  if (url.pathname === "/api/auth/sessions/refresh") return json(req, res, 200, { home_token: "fixture-home-authority" }), true;
  if (url.pathname === "/api/apps/home/summary") return json(req, res, 200, homeSummary()), true;
  if (url.pathname === "/api/apps/home/runtime/ensure") return json(req, res, 200, { ready: true }), true;
  if (url.pathname === "/api/apps/home/events") return json(req, res, 200, { events: [], cursor: "" }), true;
  if (url.pathname === "/api/apps/home/events/stream" || url.pathname === "/api/provider/object/events/stream") {
    res.writeHead(200, isolationHeaders(req, { "content-type": "text/event-stream" }));
    res.write(": fixture\n\n");
    return true;
  }
  if (url.pathname === "/api/apps/home/state") return json(req, res, 200, { saved: true }), true;
  if (url.pathname === "/api/apps/home/launch" && req.method === "POST") {
    const input = await readBody(req);
    const target = String(input?.target || "");
    if (!TITLES[target]) return json(req, res, 404, { error: "Home target not found" }), true;
    return json(req, res, 200, launchResponse(String(input?.query?.home_origin || ""), target, input?.query || {})), true;
  }
  const provider = url.pathname.match(/^\/api\/provider\/object\/([^/]+)$/);
  if (provider && req.method === "POST") {
    const payload = (await readBody(req)) || {};
    if (provider[1] === "roots") {
      return json(req, res, 200, { status: "ok", data: { roots: [
        { schema: "elastos.library.root/v1", id: "documents", label: "Documents", uri: documentsUri, kind: "directory" },
      ] } }), true;
    }
    if (provider[1] === "list") {
      return json(req, res, 200, { status: "ok", data: { uri: payload.uri, objects, object: documentsFolder } }), true;
    }
    state.unknown.push(`provider:${provider[1]}`);
    return json(req, res, 200, { status: "ok", data: {} }), true;
  }
  if (url.pathname === "/api/provider/object/download/raw") {
    const uri = url.searchParams.get("uri");
    state.downloads.push({ uri, token: req.headers["x-elastos-home-token"] || "" });
    if (uri !== `${documentsUri}/Letter.pdf`) return json(req, res, 404, { error: "not found" }), true;
    send(req, res, 200, pdfBytes, { "content-type": "application/pdf", "content-disposition": 'attachment; filename="Letter.pdf"' });
    return true;
  }
  if (url.pathname.startsWith("/api/")) {
    state.unknown.push(`${req.method} ${url.pathname}`);
    return json(req, res, 404, { error: "fixture route not found" }), true;
  }
  return false;
}

const server = createServer(async (req, res) => {
  try {
    const url = new URL(req.url || "/", "http://127.0.0.1");
    if (await handleApi(req, res, url)) return;
    if (url.pathname === `/content/${sharedCid}`) {
      // As the gateway serves user content: no CORP, a sandbox CSP.
      send(req, res, 200, sharedBytes, {
        "content-type": "application/octet-stream",
        "content-security-policy": "sandbox allow-scripts allow-forms allow-popups; connect-src 'none'",
      });
      return;
    }
    const stub = url.pathname.match(/^\/apps\/([a-z-]+)\/$/);
    if (stub && STUB_TARGETS.has(stub[1])) {
      send(req, res, 200, Buffer.from(`<!doctype html><meta charset="utf-8"><title>${stub[1]}</title><body data-fixture-app="${stub[1]}">${stub[1]}</body>`), {
        "content-type": "text/html; charset=utf-8",
        "cross-origin-resource-policy": "cross-origin",
      });
      return;
    }
    const path = staticPath(url.pathname);
    if (!path || !existsSync(path)) return json(req, res, 404, { error: "fixture route not found", path: url.pathname });
    send(req, res, 200, readFileSync(path), { "content-type": contentType(path), "cross-origin-resource-policy": "cross-origin" });
  } catch (error) {
    state.errors.push(String(error?.stack || error));
    if (!res.headersSent) json(req, res, 500, { error: String(error?.message || error) });
    else res.end();
  }
});

function playwrightSpecifier() {
  const configured = process.env.ELASTOS_PLAYWRIGHT_MODULE || "";
  if (configured) return configured.startsWith("file:") ? configured : pathToFileURL(resolve(configured)).href;
  return pathToFileURL(join(repoRoot, "elastos/tools/browser-playwright-engine/node_modules/playwright/index.js")).href;
}

async function waitFor(check, timeoutMs, label) {
  const startedAt = Date.now();
  while (Date.now() - startedAt <= timeoutMs) {
    assert(state.errors.length === 0, `fixture failed while waiting for ${label}`, state.errors);
    if (await check()) return;
    await new Promise((resolveWait) => setTimeout(resolveWait, 50));
  }
  throw new Error(`timed out waiting for ${label}: ${JSON.stringify({ pageErrors, failedRequests, launches: state.launches, unknown: state.unknown })}`);
}

await new Promise((resolveListen) => server.listen(0, "127.0.0.1", resolveListen));
const origin = `http://127.0.0.1:${server.address().port}`;
let browser = null;
try {
  const imported = await import(playwrightSpecifier());
  const { chromium } = imported.default || imported;
  browser = await chromium.launch({
    ...(process.env.ELASTOS_BROWSER_EXECUTABLE ? { executablePath: process.env.ELASTOS_BROWSER_EXECUTABLE } : {}),
    headless: true,
    args: ["--no-first-run", "--disable-extensions", "--host-resolver-rules=MAP * ~NOTFOUND, EXCLUDE 127.0.0.1"],
  });
  const context = await browser.newContext({ acceptDownloads: true, viewport: { width: 1440, height: 900 } });
  const page = await context.newPage();
  let homeClosed = false;
  page.on("close", () => { homeClosed = true; });
  page.on("pageerror", (error) => pageErrors.push(String(error?.stack || error).slice(0, 600)));
  page.on("console", (message) => { if (message.type() === "error") pageErrors.push(`console: ${message.text()}`.slice(0, 600)); });
  page.on("requestfailed", (request) => failedRequests.push(`${new URL(request.url()).pathname}: ${request.failure()?.errorText}`));
  // The 0.8.0-alpha.27 candidate's Download file navigated Library's frame to
  // /content, which the gateway's response blocked (net::ERR_BLOCKED_BY_RESPONSE).
  page.on("request", (request) => {
    if (request.isNavigationRequest() && new URL(request.url()).pathname.startsWith("/content/")) contentNavigations.push(request.url());
  });

  const gui = () => page.frames().find((frame) => frame.url().includes("/apps/home-gui/")) || null;
  const appFrames = (target) => page.frames().filter((frame) => frame.url().includes(`/apps/${target}/`));
  const windowNode = (target) => gui().locator(`.window[data-target="${target}"]`);
  const openFromHome = (target, query = {}) => page.evaluate(([openTarget, openQuery]) => {
    window.postMessage({ type: "home:open-target", target: openTarget, query: openQuery }, location.origin);
  }, [target, query]);
  const libraryFrame = async (count) => {
    await waitFor(() => appFrames("library").length >= count, 15_000, `Library frame ${count}`);
    const frame = appFrames("library")[count - 1];
    await frame.locator("#content .item").first().waitFor({ timeout: 15_000 });
    return frame;
  };
  const expectWindow = async (target, title, label) => {
    await waitFor(async () => (await windowNode(target).count()) > 0, 15_000, label);
    const node = windowNode(target).last();
    await node.waitFor({ state: "visible", timeout: 15_000 });
    const titles = await node.evaluate((el) => [el.querySelector(".window-head-title")?.textContent?.trim() || "", el.querySelector("iframe")?.title || ""]);
    assert(titles.every((text) => text === title), `${label}: the window must be titled ${title}`, titles);
    await waitFor(() => appFrames(target).some((frame) => frame.url().includes("objectUri=")), 15_000, `${label} frame`);
  };
  // The GUI drops its launch token from its URL, so read it from Home's frame.
  const shellLaunch = async (target, query) => {
    const route = await (await gui().frameElement()).evaluate((frame) => frame.dataset.route || frame.getAttribute("src") || "");
    const homeToken = new URLSearchParams(route.split("#")[1] || "").get("home_token");
    assert(homeToken, "the Home GUI frame has a launch token", route);
    return gui().evaluate(([launchTarget, launchQuery, token]) => new Promise((resolveReply) => {
      const requestId = `fixture-${Math.random().toString(16).slice(2)}`;
      const timer = setTimeout(() => resolveReply({ error: "no reply", target: "" }), 5000);
      const receive = (event) => {
        if (event.data?.type !== "home:shell-response" || event.data.requestId !== requestId) return;
        clearTimeout(timer);
        window.removeEventListener("message", receive);
        resolveReply({ error: event.data.error || "", target: event.data.result?.target || "" });
      };
      window.addEventListener("message", receive);
      window.parent.postMessage({ type: "home:launch-target", requestId, target: launchTarget, query: launchQuery, homeToken: token }, "*");
    }), [target, query, homeToken]);
  };

  await page.goto(`${origin}/apps/home/`, { waitUntil: "domcontentloaded" });
  await waitFor(() => page.evaluate(() => document.body.dataset.homeStatus === "ready"), 15_000, "Home bootstrap");
  await waitFor(() => gui(), 15_000, "Home GUI frame");
  await gui().locator("body").waitFor({ timeout: 15_000 });

  // The Home GUI alone may not launch Archive: it is not a Home target.
  const unasked = await shellLaunch("archive-manager", {});
  assert(/denied/.test(unasked.error), "Home must refuse an Archive launch Library did not ask for", unasked);

  // Every opened app covers Library, so each step opens a fresh Library window.
  let libraryWindows = 0;
  const freshLibrary = async (query = {}) => {
    libraryWindows += 1;
    await openFromHome("library", query);
    return libraryFrame(libraryWindows);
  };
  let library = await freshLibrary();

  // A file no app opens is offered as a download, and Download saves it.
  await library.locator("#content .item", { hasText: "Letter.pdf" }).dblclick();
  const offer = library.locator("#dialog:not(.hidden)", { hasText: "No app in Home opens this kind of file" });
  await offer.waitFor({ timeout: 15_000 });
  const [pdfDownload] = await Promise.all([
    page.waitForEvent("download", { timeout: 15_000 }),
    offer.locator("[data-dialog-confirm]").click(),
  ]);
  assert(pdfDownload.suggestedFilename() === "Letter.pdf", "the offered download keeps the file name", pdfDownload.suggestedFilename());
  assert((await readFile(await pdfDownload.path())).equals(pdfBytes), "the offered download saves the PDF bytes");
  assert(state.downloads.some((entry) => entry.uri === `${documentsUri}/Letter.pdf` && entry.token.startsWith("library-launch-")),
    "the offered download uses Library's authenticated download", state.downloads);

  // A ZIP double-clicked in Library opens in Archive, titled Archive.
  await library.locator("#content .item", { hasText: "Bundle.zip" }).dblclick();
  await expectWindow("archive-manager", "Archive", "ZIP opens in Archive");
  const archiveLaunch = state.launches.find((launch) => launch.target === "archive-manager");
  assert(archiveLaunch?.query?.objectUri === `${documentsUri}/Bundle.zip`, "Archive must open the chosen ZIP", archiveLaunch);
  // The grant is single-use: replaying the same launch is refused.
  const { home_origin: _ignored, ...replayQuery } = archiveLaunch.query;
  const replay = await shellLaunch("archive-manager", replayQuery);
  assert(/denied/.test(replay.error), "Home must refuse a replayed Archive launch", replay);

  // Open With still launches Archive, Documents and the GBA emulator.
  const openWith = async (name, viewer) => {
    library = await freshLibrary();
    await library.locator("#content .item", { hasText: name }).click({ button: "right" });
    const parent = library.locator("#context-menu > .menu-entry > .menu-item", { hasText: "Open With" }).first();
    await parent.click();
    await parent.locator("xpath=..").locator(".menu-submenu .menu-item", { hasText: viewer }).first().click();
  };
  await openWith("Other.zip", "Archive");
  await waitFor(() => state.launches.some((launch) => launch.target === "archive-manager" && launch.query.name === "Other.zip"), 15_000, "Open With Archive");
  await expectWindow("archive-manager", "Archive", "Open With Archive");
  await openWith("Notes.txt", "Documents");
  await expectWindow("documents", "Documents", "Open With Documents");
  library = await freshLibrary();
  await library.locator("#content .item", { hasText: "Game.gba" }).dblclick();
  await expectWindow("gba-emulator", "GBA Emulator", "Game opens in the GBA emulator");

  // A shared Chat file: Library's "File is ready" Download file saves it and Home stays open.
  library = await freshLibrary({ cid: sharedCid });
  const ready = library.locator("#dialog:not(.hidden)", { hasText: "File is ready" });
  await ready.waitFor({ timeout: 15_000 });
  const sharedDownload = page.waitForEvent("download", { timeout: 15_000 }).catch((error) => error);
  await ready.locator("text=Download file").click();
  const shared = await sharedDownload;
  await new Promise((resolveWait) => setTimeout(resolveWait, 1500));
  assert(!homeClosed && !page.isClosed(), "Download file must never close the Home tab", { failedRequests });
  assert(!(shared instanceof Error), "Download file must save the shared file", { error: String(shared), failedRequests, pageErrors });
  assert(shared.suggestedFilename() === sharedCid, "the shared download is named by its CID", shared.suggestedFilename());
  assert((await readFile(await shared.path())).equals(sharedBytes), "the shared download saves the CID's bytes");
  assert(contentNavigations.length === 0, "Download file must not navigate a frame to /content", contentNavigations);
  assert(!failedRequests.some((entry) => entry.includes("/content/")), "no failed /content request", failedRequests);
  const launchErrors = pageErrors.filter((entry) => /failed to (open|launch)|denied the shell launch|home open-target failed/.test(entry));
  assert(launchErrors.length === 0, "no launch errors in the Home console", launchErrors);

  console.log(JSON.stringify({
    ok: true,
    launches: state.launches.map((launch) => launch.target),
    downloads: [pdfDownload.suggestedFilename(), shared.suggestedFilename()],
  }));
} finally {
  await browser?.close();
  server.closeAllConnections();
  await new Promise((resolveClose) => server.close(resolveClose));
}
