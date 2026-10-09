#!/usr/bin/env node

import { createServer } from "node:http";
import { createRequire } from "node:module";
import { readFile } from "node:fs/promises";
import { extname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = resolve(fileURLToPath(new URL("../", import.meta.url)));
const browserRoot = join(repoRoot, "capsules/marketplace/browser");
const brave = process.env.BRAVE_BIN || "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser";
const require = createRequire(new URL("../elastos/tools/browser-playwright-engine/package.json", import.meta.url));
const { chromium } = require("playwright");

const normalToken = "marketplace-layout-token";
const errorToken = "marketplace-error-token";
const mediaErrorToken = "marketplace-media-error-token";
const mediaMalformedToken = "marketplace-media-malformed-token";
const mediaEmptyToken = "marketplace-media-empty-token";
const mediaCreatorMint = "a".repeat(64);
const mediaPurchasedMint = "b".repeat(64);
const mediaAvailableMint = "c".repeat(64);
const mediaPayToken = "0x1111111111111111111111111111111111111111";
const mediaSellerAddress = "0x2222222222222222222222222222222222222222";
const mediaTokenId = "0x7";
const mediaAvailability = {
  schema: "elastos.library.runtime-custody-availability-summary/v1",
  status: "last_verified_receipt",
  checked_at: 1_756_295_696,
  required_replicas: 3,
  observed_replicas: 3,
  recheck_before_buy: true,
  recheck_before_open: true,
};

function mediaListing({ mintId, displayName, accessState, quantity, price, codecs = "avc1.640028" }) {
  return {
    schema: "elastos.library.runtime-custody-listing/v1",
    mint_id: mintId,
    display_name: displayName,
    mime_type: "video/mp4",
    codecs,
    quantity,
    price,
    pay_token: mediaPayToken,
    seller_address: mediaSellerAddress,
    token_id: mediaTokenId,
    published_at: 1_756_293_600,
    availability: mediaAvailability,
    access_state: accessState,
  };
}

function mediaListResponse(listings, truncated = false) {
  return {
    schema: "elastos.library.runtime-custody-listings/v1",
    truncated,
    listings,
  };
}

const catalogCapsules = [
  {
    name: "people",
    title: "People",
    author: "Elastos",
    description: "Find people and manage contacts.",
    category: "apps",
    role: "app",
    installed: true,
    launchable: true,
    launch_target: "people",
    type: "wasm",
    trust_state: "local-manifest-signature",
    signature_state: "manifest-signature-declared",
    icon: [
      { size: 32, route: "/apps/people/icons/icon-32.png" },
      { size: 128, route: "/apps/people/icons/icon-128.png" },
      { size: 256, route: "/apps/people/icons/icon-256.png" },
    ],
  },
  {
    name: "documents",
    title: "Documents",
    author: "Elastos",
    description: "Write and share Markdown documents.",
    category: "apps",
    role: "app",
    installed: true,
    launchable: true,
    launch_target: "documents",
    type: "wasm",
    trust_state: "cid-with-manifest-signature",
    signature_state: "manifest-signature-declared",
    accepted_content: [{ name: "markdown", title: "Markdown" }],
    requires: [{ name: "people" }],
    viewer_title: "Documents",
  },
  {
    name: "object-provider",
    title: "Object Provider",
    author: "Elastos",
    description: "Storage service for this Home.",
    category: "providers",
    role: "provider",
    installed: true,
    launchable: false,
    type: "native-provider",
    trust_state: "local-manifest-signature",
    signature_state: "manifest-signature-declared",
  },
  {
    name: "bad-icons",
    title: "Bad Icons",
    author: "Elastos",
    description: "App with invalid icon metadata.",
    category: "apps",
    role: "app",
    installed: false,
    launchable: false,
    type: "wasm",
    icon: [
      { size: 128, route: "/apps/not-allowed/icons/icon-128.png" },
      { size: "oops", route: "/apps/bad-icons/icons/icon-32.png" },
    ],
  },
  {
    name: "zip-viewer",
    title: "ZIP Viewer",
    author: "Elastos",
    description: "Open compatible files and content.",
    category: "viewers",
    role: "viewer",
    installed: false,
    launchable: false,
    type: "wasm",
    accepted_content: [{ name: "zip", title: "ZIP archives" }],
  },
];

const interfaceEntries = [
  {
    capsule: "people",
    interface: {
      methods: [
        {
          input_schema: { accepts: [] },
        },
      ],
    },
    bindings: [
      { method: "capsule.open", executable: true },
      { method: "people.refresh", executable: true },
    ],
  },
  {
    capsule: "documents",
    interface: {
      methods: [
        {
          input_schema: {
            accepts: [
              {
                extensions: [".md"],
              },
            ],
          },
        },
      ],
    },
    bindings: [
      { method: "capsule.open", executable: true },
      { method: "documents.share", executable: true },
    ],
  },
  {
    capsule: "zip-viewer",
    interface: {
      methods: [
        {
          input_schema: {
            accepts: [
              {
                extensions: [".zip"],
              },
            ],
          },
        },
      ],
    },
    bindings: [],
  },
];

function assert(condition, message, details = undefined) {
  if (!condition) {
    throw new Error(`${message}${details ? `\n${JSON.stringify(details, null, 2)}` : ""}`);
  }
}

function createBarrier() {
  let release = () => {};
  const promise = new Promise((resolve) => {
    release = resolve;
  });
  return { promise, release };
}

async function readJsonBody(request) {
  const chunks = [];
  for await (const chunk of request) {
    chunks.push(Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk));
  }
  const body = Buffer.concat(chunks).toString("utf8");
  return body ? JSON.parse(body) : null;
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

async function serveFile(response, pathname) {
  const relative = pathname === "/apps/marketplace/" ? "index.html" : pathname.slice("/apps/marketplace/".length);
  const path = join(browserRoot, relative);
  assert(path.startsWith(`${browserRoot}/`) || path === join(browserRoot, "index.html"), "invalid Marketplace asset path");
  const body = await readFile(path);
  const contentType = {
    ".css": "text/css",
    ".html": "text/html; charset=utf-8",
    ".js": "text/javascript",
    ".png": "image/png",
    ".woff2": "font/woff2",
  }[extname(path)] || "application/octet-stream";
  response.writeHead(200, {
    "access-control-allow-origin": "null",
    "content-length": body.length,
    "content-type": contentType,
  });
  response.end(body);
}

async function serveCapsuleIcon(response, pathname) {
  const match = pathname.match(/^\/apps\/([A-Za-z0-9_.-]+)\/icons\/(icon-(?:32|64|128|256)\.png)$/);
  assert(match, "invalid capsule icon path", { pathname });
  const path = join(repoRoot, "capsules", match[1], "browser", "icons", match[2]);
  const body = await readFile(path);
  response.writeHead(200, {
    "access-control-allow-origin": "null",
    "content-length": body.length,
    "content-type": "image/png",
  });
  response.end(body);
}

function buildShellDocument(appSrc) {
  return `<!doctype html>
<html>
<body style="margin:0">
  <iframe id="app" title="Apps" sandbox="allow-forms allow-modals allow-pointer-lock allow-scripts" src="${appSrc}" style="border:0;width:100%;height:100vh"></iframe>
  <script>
    window.addEventListener("message", (event) => {
      if (event.data?.type !== "fixture:post-to-app") return;
      document.getElementById("app").contentWindow.postMessage(event.data.payload, "*");
    });
  <\/script>
</body>
</html>`;
}

function buildFixtureHtml(shellSrc) {
  return `<!doctype html>
<html>
<body style="margin:0">
  <iframe id="shell" title="Home shell" sandbox="allow-forms allow-modals allow-pointer-lock allow-scripts" src="${shellSrc}" style="border:0;width:100%;height:100vh"></iframe>
  <script>
    window.homeMessages = [];
    window.addEventListener("message", (event) => {
      window.homeMessages.push({ origin: event.origin, message: event.data });
    });
    window.postToMarketplace = (payload) => {
      document.getElementById("shell").contentWindow.postMessage({ type: "fixture:post-to-app", payload }, "*");
    };
    window.postToMarketplaceDirect = (payload) => {
      const shell = document.getElementById("shell");
      const child = shell.contentWindow?.frames?.[0];
      if (!child) {
        throw new Error("Marketplace child frame is not ready");
      }
      child.postMessage(payload, "*");
    };
  <\/script>
</body>
</html>`;
}

function startServer() {
  const requestLog = [];
  const notFoundPaths = [];
  const state = {
    [errorToken]: { catalogFailuresRemaining: 1 },
    [normalToken]: {
      catalogFailuresRemaining: 0,
      initialCatalogBarrier: createBarrier(),
      initialInterfacesBarrier: createBarrier(),
      initialMediaBarrier: createBarrier(),
      initialCatalogHeld: false,
      initialInterfacesHeld: false,
      initialMediaHeld: false,
      mediaListings: [
        mediaListing({
          mintId: mediaCreatorMint,
          displayName: "Creator Video",
          accessState: "creator",
          quantity: "0x2",
          price: "0x5",
        }),
        mediaListing({
          mintId: mediaPurchasedMint,
          displayName: "Owned Video",
          accessState: "purchased",
          quantity: "0x3",
          price: "0x6",
        }),
        mediaListing({
          mintId: mediaAvailableMint,
          displayName: "Store Video",
          accessState: "available",
          quantity: "0x4",
          price: "0x8",
        }),
      ],
    },
    [mediaErrorToken]: { catalogFailuresRemaining: 0 },
    [mediaMalformedToken]: { catalogFailuresRemaining: 0 },
    [mediaEmptyToken]: { catalogFailuresRemaining: 0 },
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
      if (url.pathname === "/favicon.ico") {
        response.writeHead(204).end();
        return;
      }
      if (url.pathname === "/fixture-normal") {
        const topOrigin = `http://${request.headers.host}`;
        const appSrc = `/apps/marketplace/?home_origin=${encodeURIComponent(topOrigin)}#home_token=${encodeURIComponent(normalToken)}`;
        const shellSrc = `/fixture-shell?app_src=${encodeURIComponent(appSrc)}`;
        const body = Buffer.from(buildFixtureHtml(shellSrc));
        response.writeHead(200, { "content-length": body.length, "content-type": "text/html; charset=utf-8" });
        response.end(body);
        return;
      }
      if (url.pathname === "/fixture-error") {
        const topOrigin = `http://${request.headers.host}`;
        const appSrc = `/apps/marketplace/?home_origin=${encodeURIComponent(topOrigin)}#home_token=${encodeURIComponent(errorToken)}`;
        const shellSrc = `/fixture-shell?app_src=${encodeURIComponent(appSrc)}`;
        const body = Buffer.from(buildFixtureHtml(shellSrc));
        response.writeHead(200, { "content-length": body.length, "content-type": "text/html; charset=utf-8" });
        response.end(body);
        return;
      }
      if (url.pathname === "/fixture-media-error") {
        const topOrigin = `http://${request.headers.host}`;
        const appSrc = `/apps/marketplace/?home_origin=${encodeURIComponent(topOrigin)}#home_token=${encodeURIComponent(mediaErrorToken)}`;
        const shellSrc = `/fixture-shell?app_src=${encodeURIComponent(appSrc)}`;
        const body = Buffer.from(buildFixtureHtml(shellSrc));
        response.writeHead(200, { "content-length": body.length, "content-type": "text/html; charset=utf-8" });
        response.end(body);
        return;
      }
      if (url.pathname === "/fixture-media-malformed") {
        const topOrigin = `http://${request.headers.host}`;
        const appSrc = `/apps/marketplace/?home_origin=${encodeURIComponent(topOrigin)}#home_token=${encodeURIComponent(mediaMalformedToken)}`;
        const shellSrc = `/fixture-shell?app_src=${encodeURIComponent(appSrc)}`;
        const body = Buffer.from(buildFixtureHtml(shellSrc));
        response.writeHead(200, { "content-length": body.length, "content-type": "text/html; charset=utf-8" });
        response.end(body);
        return;
      }
      if (url.pathname === "/fixture-media-empty") {
        const topOrigin = `http://${request.headers.host}`;
        const appSrc = `/apps/marketplace/?home_origin=${encodeURIComponent(topOrigin)}#home_token=${encodeURIComponent(mediaEmptyToken)}`;
        const shellSrc = `/fixture-shell?app_src=${encodeURIComponent(appSrc)}`;
        const body = Buffer.from(buildFixtureHtml(shellSrc));
        response.writeHead(200, { "content-length": body.length, "content-type": "text/html; charset=utf-8" });
        response.end(body);
        return;
      }
      if (url.pathname === "/fixture-shell") {
        const appSrc = url.searchParams.get("app_src") || "";
        const body = Buffer.from(buildShellDocument(appSrc));
        response.writeHead(200, { "content-length": body.length, "content-type": "text/html; charset=utf-8" });
        response.end(body);
        return;
      }
      if (url.pathname.startsWith("/apps/marketplace/")) {
        await serveFile(response, url.pathname);
        return;
      }
      if (/^\/apps\/[A-Za-z0-9_.-]+\/icons\/icon-(?:32|64|128|256)\.png$/.test(url.pathname)) {
        await serveCapsuleIcon(response, url.pathname);
        return;
      }
      if (url.pathname === "/api/capsules/catalog") {
        const token = String(request.headers["x-elastos-home-token"] || "");
        requestLog.push({ path: url.pathname, token });
        if (token === normalToken && !state[normalToken].initialCatalogHeld) {
          state[normalToken].initialCatalogHeld = true;
          await state[normalToken].initialCatalogBarrier.promise;
        }
        if (state[token]?.catalogFailuresRemaining > 0) {
          state[token].catalogFailuresRemaining -= 1;
          json(response, { message: "provider launch failed" }, 500);
          return;
        }
        json(response, { capsules: catalogCapsules });
        return;
      }
      if (url.pathname === "/api/capsules/interfaces") {
        const token = String(request.headers["x-elastos-home-token"] || "");
        requestLog.push({ path: url.pathname, token });
        if (token === normalToken && !state[normalToken].initialInterfacesHeld) {
          state[normalToken].initialInterfacesHeld = true;
          await state[normalToken].initialInterfacesBarrier.promise;
        }
        json(response, { interfaces: interfaceEntries });
        return;
      }
      if (url.pathname === "/api/provider/object/list_runtime_custody") {
        const token = String(request.headers["x-elastos-home-token"] || "");
        const body = await readJsonBody(request);
        requestLog.push({ path: url.pathname, token, method: request.method, body });
        if (token === normalToken && !state[normalToken].initialMediaHeld) {
          state[normalToken].initialMediaHeld = true;
          await state[normalToken].initialMediaBarrier.promise;
        }
        if (token === mediaErrorToken) {
          json(response, { message: "runtime service unavailable" }, 500);
          return;
        }
        if (token === mediaMalformedToken) {
          json(response, {
            status: "ok",
            data: mediaListResponse([
              {
                ...mediaListing({
                  mintId: mediaCreatorMint,
                  displayName: "Broken Video",
                  accessState: "creator",
                  quantity: "0x1",
                  price: "0x2",
                }),
                access_state: "unknown",
              },
            ]),
          });
          return;
        }
        if (token === mediaEmptyToken) {
          json(response, { status: "ok", data: mediaListResponse([]) });
          return;
        }
        json(response, { status: "ok", data: mediaListResponse(state[token]?.mediaListings || []) });
        return;
      }
      if (url.pathname === "/api/provider/object/buy") {
        const token = String(request.headers["x-elastos-home-token"] || "");
        const body = await readJsonBody(request);
        requestLog.push({ path: url.pathname, token, method: request.method, body });
        if (token === normalToken && body?.mint_id === mediaAvailableMint) {
          state[normalToken].mediaListings = state[normalToken].mediaListings.map((listing) =>
            listing.mint_id === mediaAvailableMint
              ? { ...listing, access_state: "purchased" }
              : listing,
          );
          json(response, { status: "ok", data: { status: "accepted" } });
          return;
        }
        json(response, { message: "runtime purchase unavailable" }, 500);
        return;
      }
      notFoundPaths.push(url.pathname);
      response.writeHead(404).end("not found");
    } catch (error) {
      response.writeHead(500, { "content-type": "text/plain; charset=utf-8" });
      response.end(error.stack || String(error));
    }
  });
  return { notFoundPaths, requestLog, server, state };
}

async function waitForMarketplaceFrame(page) {
  for (let attempt = 0; attempt < 50; attempt += 1) {
    const frame = page.frames().find((entry) => entry.url().includes("/apps/marketplace/"));
    if (frame) {
      return frame;
    }
    await page.waitForTimeout(100);
  }
  throw new Error(`Marketplace iframe did not appear\n${JSON.stringify(page.frames().map((entry) => entry.url()), null, 2)}`);
}

async function readHomeMessages(page) {
  return page.evaluate(() =>
    window.homeMessages.map((entry) => ({
      origin: entry.origin,
      type: entry.message?.type || "",
      target: entry.message?.target || "",
      query: entry.message?.query || null,
      menus: entry.message?.menus || null,
      homeToken: entry.message?.homeToken || "",
      keys: Object.keys(entry.message || {}).sort(),
    })),
  );
}

async function waitForFrameWidth(frame, expectedWidth) {
  await frame.waitForFunction((width) => window.innerWidth === width, expectedWidth);
}

async function waitForRequestCount(requestLog, token, path, count) {
  const deadline = Date.now() + 5_000;
  while (Date.now() < deadline) {
    const current = requestLog.filter((entry) => entry.token === token && entry.path === path).length;
    if (current >= count) {
      return;
    }
    await new Promise((resolveDelay) => setTimeout(resolveDelay, 10));
  }
  const current = requestLog.filter((entry) => entry.token === token && entry.path === path).length;
  throw new Error(`Timed out waiting for ${path}`, { cause: { token, path, expected: count, current } });
}

async function assertNoHorizontalOverflow(frame, label) {
  const overflow = await frame.evaluate(() => ({
    doc: document.documentElement.scrollWidth - document.documentElement.clientWidth,
    main: (() => {
      const node = document.getElementById("store-main");
      return node ? node.scrollWidth - node.clientWidth : 0;
    })(),
    shell: (() => {
      const node = document.querySelector(".store-shell");
      return node ? node.scrollWidth - node.clientWidth : 0;
    })(),
  }));
  assert(overflow.doc <= 1, `${label}: document overflowed horizontally`, overflow);
  assert(overflow.main <= 1, `${label}: main overflowed horizontally`, overflow);
  assert(overflow.shell <= 1, `${label}: shell overflowed horizontally`, overflow);
}

async function run() {
  const { notFoundPaths, requestLog, server, state } = startServer();
  const browser = await chromium.launch({ executablePath: brave, headless: true });
  const page = await browser.newPage({ viewport: { width: 1280, height: 900 } });
  const pageErrors = [];
  const consoleErrors = [];
  const nonOkResponses = [];
  const requestFailures = [];
  try {
    await new Promise((resolveListen, rejectListen) => {
      server.once("error", rejectListen);
      server.listen(0, "127.0.0.1", () => resolveListen());
    });
    const address = server.address();
    assert(address && typeof address === "object", "fixture server did not bind");
    const port = address.port;

    page.on("pageerror", (error) => {
      pageErrors.push(error?.stack || String(error));
    });
    page.on("console", (message) => {
      if (message.type() === "error") {
        consoleErrors.push(message.text());
      }
    });
    page.on("response", async (response) => {
      if (response.status() < 400) {
        return;
      }
      const request = response.request();
      const headers = request.headers();
      nonOkResponses.push({
        url: response.url(),
        status: response.status(),
        token: headers["x-elastos-home-token"] || "",
        method: request.method(),
      });
    });
    page.on("requestfailed", (request) => {
      requestFailures.push({
        url: request.url(),
        method: request.method(),
        error: request.failure()?.errorText || "unknown",
      });
    });

    await page.goto(`http://127.0.0.1:${port}/fixture-normal`);
    const frame = await waitForMarketplaceFrame(page);
    await frame.locator(".store-row-skeleton").first().waitFor();
    await waitForRequestCount(requestLog, normalToken, "/api/capsules/catalog", 1);
    await waitForRequestCount(requestLog, normalToken, "/api/capsules/interfaces", 1);
    await waitForRequestCount(requestLog, normalToken, "/api/provider/object/list_runtime_custody", 1);
    state[normalToken].initialCatalogBarrier.release();
    state[normalToken].initialInterfacesBarrier.release();
    await frame.locator(".store-row").filter({ hasText: "People" }).first().waitFor();
    const discoverTextWhileMediaPending = await frame.locator("#store-main").textContent();
    assert(
      /People/.test(discoverTextWhileMediaPending || ""),
      "Marketplace must render Discover when catalog requests finish even while Media is still pending",
      { discoverTextWhileMediaPending },
    );
    await frame.locator('[data-destination="media"]').click();
    await frame.locator(".store-row-skeleton").first().waitFor();
    const mediaRowsWhilePending = await frame.locator(".store-row-media").count();
    assert(mediaRowsWhilePending === 0, "Marketplace Media must keep its own loading state while listings are pending", { mediaRowsWhilePending });
    state[normalToken].initialMediaBarrier.release();
    await frame.locator('.store-row-media').first().waitFor();
    await frame.locator('[data-destination="discover"]').click();
    await frame.locator('.store-row[data-app="people"]').first().waitFor();

    const initialMessages = await readHomeMessages(page);
    assert(initialMessages[0]?.type === "home:app-ready", "Marketplace must announce Home readiness first", initialMessages);
    assert(initialMessages[1]?.type === "home:menu-manifest", "Marketplace must publish its menu after readiness", initialMessages);
    assert(initialMessages[1]?.menus?.[0]?.title === "File", "Marketplace menu must use the canonical title/items shape", initialMessages[1]);
    assert(initialMessages[1]?.menus?.[1]?.title === "View", "Marketplace must expose the View menu", initialMessages[1]);

    const normalCatalogRequests = requestLog.filter((entry) => entry.token === normalToken && entry.path === "/api/capsules/catalog").length;
    const normalInterfaceRequests = requestLog.filter((entry) => entry.token === normalToken && entry.path === "/api/capsules/interfaces").length;
    const initialMediaRequest = requestLog.find((entry) => entry.token === normalToken && entry.path === "/api/provider/object/list_runtime_custody");
    assert(
      normalCatalogRequests === 1 && normalInterfaceRequests === 1,
      "Marketplace must read the canonical catalog routes once on first load",
      requestLog,
    );
    assert(
      initialMediaRequest?.method === "POST" && JSON.stringify(initialMediaRequest.body) === "{}",
      "Marketplace must load protected media through the typed list_runtime_custody request",
      initialMediaRequest,
    );

    const peopleIconVisible = await frame.locator('.store-row[data-app="people"] .app-icon-img').first().getAttribute("src");
    assert(peopleIconVisible === "/apps/people/icons/icon-128.png", "Marketplace must prefer the declared 128px icon route", { peopleIconVisible });

    const badIconHasImage = await frame.locator('.store-row[data-app="bad-icons"] .app-icon-img').first().count();
    assert(badIconHasImage === 0, "Marketplace must reject malformed or cross-capsule icon routes");
    assert(
      !requestLog.some((entry) => entry.path === "/apps/not-allowed/icons/icon-128.png"),
      "Marketplace must not request rejected icon routes",
      requestLog,
    );

    const installedBadge = await frame.locator("#installed-badge").textContent();
    assert(installedBadge?.trim() === "3", "Marketplace installed count must come from catalog installed fields", { installedBadge });

    const discoverTitles = await frame.locator(".store-section-title").evaluateAll((nodes) => nodes.map((node) => node.textContent?.trim() || ""));
    assert(discoverTitles.includes("Installed"), "Marketplace Discover must include the Installed section", discoverTitles);
    assert(discoverTitles.includes("Apps"), "Marketplace Discover must include app category sections", discoverTitles);
    assert(discoverTitles.includes("Services"), "Marketplace Discover must include service category sections", discoverTitles);

    await frame.locator('.store-see-all[data-destination="providers"]').click();
    await frame.locator("#store-title").waitFor({ state: "visible" });
    const providerTitle = await frame.locator("#store-title").textContent();
    assert(providerTitle?.trim() === "Services", "Marketplace See All must route through the category destination", { providerTitle });

    await frame.locator('[data-destination="discover"]').click();
    await frame.locator("#search-input").fill("documents");
    const visibleRows = await frame.locator(".store-row").evaluateAll((nodes) =>
      nodes
        .filter((node) => node.offsetParent !== null)
        .map((node) => node.textContent?.trim() || "")
        .filter(Boolean),
    );
    assert(
      visibleRows.length >= 1 && visibleRows.every((row) => /Documents/.test(row)),
      "Marketplace search must filter the current catalog rows",
      visibleRows,
    );
    await frame.locator("#search-input").fill("");

    await frame.locator('[data-destination="media"]').click();
    await frame.locator('.store-row-media').first().waitFor();
    const mediaTitle = await frame.locator("#store-title").textContent();
    assert(mediaTitle?.trim() === "Media", "Marketplace must expose the Media destination", { mediaTitle });
    const mediaRows = await frame.locator(".store-row-media").evaluateAll((nodes) =>
      nodes.map((node) => node.textContent?.trim() || ""),
    );
    assert(
      mediaRows.some((row) => /Creator Video/.test(row) && /Quantity 2/.test(row) && /Price 5 base units/.test(row))
        && mediaRows.every((row) => !/Quantity 0x|Price 0x/.test(row)),
      "Marketplace media rows must present canonical uint256 listing values in decimal",
      mediaRows,
    );
    await frame.locator("#search-input").fill("price 5");
    const decimalSearchRows = await frame.locator(".store-row-media").evaluateAll((nodes) =>
      nodes.map((node) => node.textContent?.trim() || ""),
    );
    assert(
      decimalSearchRows.length === 1 && /Creator Video/.test(decimalSearchRows[0]),
      "Marketplace media search must include the visible decimal price",
      decimalSearchRows,
    );
    await frame.locator("#search-input").fill("");
    const mediaPageText = await frame.locator("#store-main").textContent();
    assert(
      !mediaPageText?.includes(mediaCreatorMint)
        && !mediaPageText?.includes(mediaPurchasedMint)
        && !mediaPageText?.includes(mediaAvailableMint),
      "Marketplace must keep mint IDs internal to media actions",
      { mediaPageText },
    );

    const mediaOpenCountBefore = (await readHomeMessages(page)).filter((entry) => entry.type === "home:open-target").length;
    await frame.locator(`.store-row-media[data-mint="${mediaCreatorMint}"] [data-action="open-media"]`).click();
    const mediaOpenMessagesAfterCreator = await readHomeMessages(page);
    const creatorOpen = mediaOpenMessagesAfterCreator.filter((entry) => entry.type === "home:open-target").at(-1);
    assert(
      mediaOpenMessagesAfterCreator.filter((entry) => entry.type === "home:open-target").length === mediaOpenCountBefore + 1,
      "Marketplace creator media rows must open through Home exactly once",
      mediaOpenMessagesAfterCreator,
    );
    assert(
      creatorOpen?.target === "elacity-player"
        && creatorOpen.homeToken === normalToken
        && JSON.stringify(creatorOpen.query) === JSON.stringify({ mint_id: mediaCreatorMint })
        && JSON.stringify(creatorOpen.keys) === JSON.stringify(["homeToken", "query", "target", "type"]),
      "Marketplace creator media rows must send the exact authorized elacity-player launch command",
      creatorOpen,
    );

    const listCountBeforeBuy = requestLog.filter((entry) => entry.token === normalToken && entry.path === "/api/provider/object/list_runtime_custody").length;
    const pendingBuyState = await frame.locator(`.store-row-media[data-mint="${mediaAvailableMint}"] [data-action="buy-media"]`).evaluate((button) => {
      button.click();
      button.click();
      const activeButton = document.querySelector(`.store-row-media[data-mint="${button.dataset.mint}"] [data-action="buy-media"]`);
      return {
        disabled: activeButton?.disabled === true,
        label: activeButton?.textContent?.trim() || "",
      };
    });
    assert(
      pendingBuyState.disabled && pendingBuyState.label === "Buying...",
      "Marketplace must mark an in-memory media buy as pending before the request settles",
      pendingBuyState,
    );
    await waitForRequestCount(requestLog, normalToken, "/api/provider/object/buy", 1);
    await waitForRequestCount(requestLog, normalToken, "/api/provider/object/list_runtime_custody", listCountBeforeBuy + 1);
    const buyRequests = requestLog.filter((entry) => entry.token === normalToken && entry.path === "/api/provider/object/buy");
    const buyRequest = buyRequests[0];
    assert(
      buyRequests.length === 1
        && buyRequest?.method === "POST"
        && JSON.stringify(buyRequest.body) === JSON.stringify({ mint_id: mediaAvailableMint }),
      "Marketplace rapid media Buy activation must submit one exact typed request",
      buyRequests,
    );
    await frame.locator(`.store-row-media[data-mint="${mediaAvailableMint}"] [data-action="open-media"]`).waitFor();
    const boughtRowText = await frame.locator(`.store-row-media[data-mint="${mediaAvailableMint}"]`).textContent();
    assert(/Owned/.test(boughtRowText || ""), "Marketplace must reload the media list after buy", { boughtRowText });

    const purchasedOpenCountBefore = (await readHomeMessages(page)).filter((entry) => entry.type === "home:open-target").length;
    await frame.locator(`.store-row-media[data-mint="${mediaPurchasedMint}"] [data-action="open-media"]`).click();
    const mediaOpenMessagesAfterPurchased = await readHomeMessages(page);
    const purchasedOpen = mediaOpenMessagesAfterPurchased.filter((entry) => entry.type === "home:open-target").at(-1);
    assert(
      mediaOpenMessagesAfterPurchased.filter((entry) => entry.type === "home:open-target").length === purchasedOpenCountBefore + 1,
      "Marketplace purchased media rows must open through Home exactly once",
      mediaOpenMessagesAfterPurchased,
    );
    assert(
      purchasedOpen?.target === "elacity-player"
        && purchasedOpen.homeToken === normalToken
        && JSON.stringify(purchasedOpen.query) === JSON.stringify({ mint_id: mediaPurchasedMint })
        && JSON.stringify(purchasedOpen.keys) === JSON.stringify(["homeToken", "query", "target", "type"]),
      "Marketplace purchased media rows must send the exact authorized elacity-player launch command",
      purchasedOpen,
    );

    await frame.locator('[data-destination="discover"]').click();
    await frame.locator('.store-row[data-app="documents"]').first().waitFor();

    await frame.locator('.store-row[data-app="documents"]').first().focus();
    await frame.locator('.store-row[data-app="documents"]').first().click();
    await frame.locator("#detail-modal.active").waitFor();
    const modalText = await frame.locator("#detail-content").textContent();
    assert(/Status/.test(modalText || ""), "Marketplace detail modal must expose trust and status", { modalText });
    assert(/Works with/.test(modalText || ""), "Marketplace detail modal must expose accepted content and dependencies", { modalText });
    assert(/Available actions/.test(modalText || ""), "Marketplace detail modal must expose executable actions", { modalText });

    await page.keyboard.press("Tab");
    await page.keyboard.press("Tab");
    const trappedInsideModal = await frame.evaluate(() => {
      const modal = document.getElementById("detail-content");
      return Boolean(modal && modal.contains(document.activeElement));
    });
    assert(trappedInsideModal, "Marketplace detail focus must stay trapped in the modal");
    await page.keyboard.press("Escape");
    const modalActiveAfterEscape = await frame.locator("#detail-modal").evaluate((node) => node.classList.contains("active"));
    assert(!modalActiveAfterEscape, "Marketplace detail modal must close on Escape");
    const restoredFocus = await frame.evaluate(() => document.activeElement?.getAttribute("data-app") || "");
    assert(restoredFocus === "documents", "Marketplace must restore focus to the invoking row after closing the detail modal", { restoredFocus });

    const openMessagesBefore = (await readHomeMessages(page)).filter((entry) => entry.type === "home:open-target").length;
    await frame.locator('.store-row[data-app="people"] .store-pill').first().click();
    const openMessagesAfter = await readHomeMessages(page);
    const latestOpen = openMessagesAfter.filter((entry) => entry.type === "home:open-target").at(-1);
    assert((openMessagesAfter.filter((entry) => entry.type === "home:open-target").length) === openMessagesBefore + 1, "Marketplace must launch only through Home open-target", openMessagesAfter);
    assert(latestOpen?.target === "people", "Marketplace must use the catalog launch_target", latestOpen);

    const openCountBeforeBad = openMessagesAfter.filter((entry) => entry.type === "home:open-target").length;
    await frame.locator('.store-row[data-app="bad-icons"]').first().click();
    await frame.locator("#detail-modal.active").waitFor();
    const badOpenButtonCount = await frame.locator('#detail-content [data-action="open"]').count();
    assert(badOpenButtonCount === 0, "Marketplace must not offer Open for non-launchable entries");
    await frame.locator('#detail-content [data-action="close-detail"]').last().click();
    const openCountAfterBad = (await readHomeMessages(page)).filter((entry) => entry.type === "home:open-target").length;
    assert(openCountAfterBad === openCountBeforeBad, "Marketplace non-launchable entries must not send launch messages");

    await frame.locator('[data-destination="discover"]').focus();
    await page.keyboard.press("ArrowDown");
    let currentDestination = await frame.locator('[aria-current="page"]').textContent();
    assert(/Installed/.test(currentDestination || ""), "Marketplace sidebar ArrowDown must move among destinations", { currentDestination });
    await frame.locator('[data-destination="discover"]').click();
    await frame.locator("#search-input").focus();
    await page.keyboard.press("ArrowDown");
    currentDestination = await frame.locator('[aria-current="page"]').textContent();
    assert(/Discover/.test(currentDestination || ""), "Marketplace sidebar keys must not steal ArrowDown from inputs", { currentDestination });

    const countsBeforeInvalidRefresh = {
      catalog: requestLog.filter((entry) => entry.token === normalToken && entry.path === "/api/capsules/catalog").length,
      interfaces: requestLog.filter((entry) => entry.token === normalToken && entry.path === "/api/capsules/interfaces").length,
    };
    const refreshCatalogResponse = page.waitForResponse((response) => {
      const headers = response.request().headers();
      return response.url() === `http://127.0.0.1:${port}/api/capsules/catalog`
        && headers["x-elastos-home-token"] === normalToken;
    });
    const refreshInterfacesResponse = page.waitForResponse((response) => {
      const headers = response.request().headers();
      return response.url() === `http://127.0.0.1:${port}/api/capsules/interfaces`
        && headers["x-elastos-home-token"] === normalToken;
    });
    await page.evaluate(() => {
      window.postToMarketplaceDirect({ type: "elastos:menu-command", cmd: "refresh" });
      window.postToMarketplace({ type: "elastos:menu-command", cmd: "refresh" });
    });
    await Promise.all([refreshCatalogResponse, refreshInterfacesResponse]);
    const countsAfterRefresh = {
      catalog: requestLog.filter((entry) => entry.token === normalToken && entry.path === "/api/capsules/catalog").length,
      interfaces: requestLog.filter((entry) => entry.token === normalToken && entry.path === "/api/capsules/interfaces").length,
    };
    assert(
      countsAfterRefresh.catalog === countsBeforeInvalidRefresh.catalog + 1
        && countsAfterRefresh.interfaces === countsBeforeInvalidRefresh.interfaces + 1,
      "Marketplace must reject non-parent menu commands and reuse the same refresh path for trusted parent commands",
      { countsBeforeInvalidRefresh, countsAfterRefresh },
    );

    await page.setViewportSize({ width: 1280, height: 900 });
    await waitForFrameWidth(frame, 1280);
    await frame.locator('[data-destination="media"]').click();
    await frame.locator('.store-row-media').first().waitFor();
    await assertNoHorizontalOverflow(frame, "wide Marketplace layout");

    await page.setViewportSize({ width: 640, height: 900 });
    await waitForFrameWidth(frame, 640);
    await assertNoHorizontalOverflow(frame, "narrow Marketplace layout");

    await page.goto(`http://127.0.0.1:${port}/fixture-media-error`);
    const mediaErrorFrame = await waitForMarketplaceFrame(page);
    await mediaErrorFrame.locator('.store-row[data-app="people"]').first().waitFor();
    await mediaErrorFrame.locator('[data-destination="media"]').click();
    await mediaErrorFrame.locator(".store-error-card").waitFor();
    const mediaErrorText = await mediaErrorFrame.locator("#load-error").textContent();
    assert(/Couldn’t load media/.test(mediaErrorText || ""), "Marketplace must show a bounded public media error", { mediaErrorText });
    assert(!/runtime service unavailable/.test(mediaErrorText || ""), "Marketplace must keep raw Runtime errors out of visible media text", { mediaErrorText });
    await mediaErrorFrame.locator('[data-destination="discover"]').click();
    await mediaErrorFrame.locator('.store-row[data-app="people"]').first().waitFor();

    await page.goto(`http://127.0.0.1:${port}/fixture-media-empty`);
    const mediaEmptyFrame = await waitForMarketplaceFrame(page);
    await mediaEmptyFrame.locator('[data-destination="media"]').click();
    await mediaEmptyFrame.locator(".empty-state").waitFor();
    const mediaEmptyText = await mediaEmptyFrame.locator("#store-main").textContent();
    assert(/No protected media available/.test(mediaEmptyText || ""), "Marketplace must keep a clear empty media state", { mediaEmptyText });

    await page.goto(`http://127.0.0.1:${port}/fixture-media-malformed`);
    const mediaMalformedFrame = await waitForMarketplaceFrame(page);
    await mediaMalformedFrame.locator('[data-destination="media"]').click();
    await mediaMalformedFrame.locator(".store-error-card").waitFor();
    const mediaMalformedText = await mediaMalformedFrame.locator("#load-error").textContent();
    assert(/Couldn’t load media/.test(mediaMalformedText || ""), "Marketplace must fail closed on malformed media listings", { mediaMalformedText });

    await page.goto(`http://127.0.0.1:${port}/fixture-error`);
    const errorFrame = await waitForMarketplaceFrame(page);
    await errorFrame.locator(".store-error-card").waitFor();
    const errorText = await errorFrame.locator("#load-error").textContent();
    assert(/Couldn’t load apps/.test(errorText || ""), "Marketplace must show a bounded public error state", { errorText });
    assert(!/provider launch failed/.test(errorText || ""), "Marketplace must not expose raw internal load errors", { errorText });
    const errorMessagesBeforeRetry = (await readHomeMessages(page)).filter((entry) => entry.type === "home:menu-manifest").length;
    await errorFrame.locator('[data-action="retry"]').click();
    await errorFrame.locator('.store-row[data-app="people"]').first().waitFor();
    const errorMessagesAfterRetry = (await readHomeMessages(page)).filter((entry) => entry.type === "home:menu-manifest").length;
    assert(errorMessagesAfterRetry === errorMessagesBeforeRetry, "Marketplace must deduplicate unchanged menu manifests across retry");

    assert(notFoundPaths.length === 0, "Marketplace layout smoke hit unexpected fixture paths", notFoundPaths);
    assert(
      nonOkResponses.length === 2
        && nonOkResponses.some((entry) =>
          entry.url === `http://127.0.0.1:${port}/api/capsules/catalog`
            && entry.status === 500
            && entry.token === errorToken
            && entry.method === "GET",
        )
        && nonOkResponses.some((entry) =>
          entry.url === `http://127.0.0.1:${port}/api/provider/object/list_runtime_custody`
            && entry.status === 500
            && entry.token === mediaErrorToken
            && entry.method === "POST",
        ),
      "Marketplace layout smoke must see only the deliberate catalog and media fixture failures",
      nonOkResponses,
    );
    assert(pageErrors.length === 0, "Marketplace layout smoke saw page errors", pageErrors);
    const expectedConsoleError = "Failed to load resource: the server responded with a status of 500 (Internal Server Error)";
    const unexpectedConsoleErrors = consoleErrors.filter((entry) => entry !== expectedConsoleError);
    assert(
      consoleErrors.length === 2,
      "Marketplace layout smoke must see only the deliberate catalog and media fixture console errors",
      consoleErrors,
    );
    assert(unexpectedConsoleErrors.length === 0, "Marketplace layout smoke saw console errors", unexpectedConsoleErrors);
    assert(requestFailures.length === 0, "Marketplace layout smoke saw failed browser requests", requestFailures);
  } finally {
    server.closeAllConnections?.();
    await new Promise((resolveClose) => server.close(() => resolveClose()));
    await browser.close();
  }
}

run()
  .then(() => {
    console.log("marketplace-product-layout-smoke: OK");
  })
  .catch((error) => {
    console.error(error.stack || String(error));
    process.exitCode = 1;
  });
