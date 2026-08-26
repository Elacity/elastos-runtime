#!/usr/bin/env node

import { createServer } from "node:http";
import { createRequire } from "node:module";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { extname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = resolve(fileURLToPath(new URL("../", import.meta.url)));
const servicesRoot = join(repoRoot, "capsules/services/browser");
const brave = process.env.BRAVE_BIN || "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser";
const require = createRequire(new URL("../elastos/tools/browser-playwright-engine/package.json", import.meta.url));
const { chromium } = require("playwright");

const TOKEN = "services-token";

function assert(condition, message, details = undefined) {
  if (!condition) {
    throw new Error(`${message}${details ? `\n${JSON.stringify(details, null, 2)}` : ""}`);
  }
}

function json(response, value, status = 200, headers = {}) {
  const body = Buffer.from(JSON.stringify(value));
  response.writeHead(status, {
    "access-control-allow-origin": "null",
    "access-control-allow-methods": "GET,POST,OPTIONS",
    "access-control-allow-headers": "content-type,x-elastos-home-token",
    "cache-control": "no-store",
    "content-length": body.length,
    "content-type": "application/json",
    ...headers,
  });
  response.end(body);
}

function text(response, value, status = 200, headers = {}) {
  const body = Buffer.from(String(value));
  response.writeHead(status, {
    "access-control-allow-origin": "null",
    "access-control-allow-methods": "GET,POST,OPTIONS",
    "access-control-allow-headers": "content-type,x-elastos-home-token",
    "cache-control": "no-store",
    "content-length": body.length,
    "content-type": "text/plain; charset=utf-8",
    ...headers,
  });
  response.end(body);
}

function escapeHtml(value) {
  return String(value).replace(/[&<>"']/g, (char) => ({
    "&": "&amp;",
    "<": "&lt;",
    ">": "&gt;",
    '"': "&quot;",
    "'": "&#39;",
  })[char]);
}

function fixtureDocument(appSrc) {
  return `<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title>Services UIUX smoke</title>
    <style>
      html, body {
        margin: 0;
        width: 100%;
        height: 100%;
        overflow: hidden;
        background: #0f1217;
      }

      iframe {
        display: block;
        width: 100%;
        height: 100%;
        border: 0;
      }
    </style>
  </head>
  <body>
    <iframe
      id="services-frame"
      title="Services"
      sandbox="allow-forms allow-modals allow-pointer-lock allow-scripts"
      src="${escapeHtml(appSrc)}"
    ></iframe>
    <script>
      window.__homeMessages = [];
      window.addEventListener("message", (event) => {
        const frame = document.getElementById("services-frame");
        if (!frame || event.source !== frame.contentWindow) {
          return;
        }
        window.__homeMessages.push({
          origin: event.origin,
          sourceMatchesFrame: event.source === frame.contentWindow,
          data: event.data,
        });
      });
    </script>
  </body>
</html>`;
}

function makeServicesState() {
  return {
    local_offers: [
      {
        offer_id: "mine-browser-engine",
        service_kind: "browser_engine",
        display_name: "My Browser Engine",
        enabled: true,
      },
      {
        offer_id: "hidden-local-offer",
        service_kind: "not-visible",
        display_name: "Hidden local offer",
      },
    ],
    available_local_offers: [
      {
        offer_id: "mine-browser-exit",
        service_kind: "remote_exit",
        display_name: "My Browser Exit service",
      },
    ],
    remote_offers: [
      {
        offer_id: "configured-browser-exit",
        service_kind: "remote_exit",
        display_name: "Configured Browser Exit service",
        source: "configured_remote_exit",
        enabled: true,
        status: "active",
      },
      {
        offer_id: "approved-browser-engine",
        service_kind: "browser_engine",
        display_name: "Ada's Browser Engine",
        grant_required: true,
        status: "approved",
      },
    ],
    available_remote_offers: [
      {
        offer_id: "request-browser-engine",
        service_kind: "browser_engine",
        display_name: "Lin's Browser Engine",
        grant_required: true,
        status: "requestable",
      },
      {
        offer_id: "failing-browser-exit",
        service_kind: "remote_exit",
        display_name: "Kira's Browser Exit service",
      },
      {
        offer_id: "hidden-remote-offer",
        service_kind: "not-visible",
        display_name: "Hidden remote offer",
      },
    ],
  };
}

function cloneState(value) {
  return JSON.parse(JSON.stringify(value));
}

function removeOffer(list, offerId) {
  const index = list.findIndex((offer) => offer?.offer_id === offerId);
  if (index < 0) {
    return null;
  }
  const [offer] = list.splice(index, 1);
  return offer;
}

function upsertOffer(list, offer) {
  const existingIndex = list.findIndex((entry) => entry?.offer_id === offer?.offer_id);
  if (existingIndex >= 0) {
    list.splice(existingIndex, 1, offer);
    return;
  }
  list.push(offer);
}

async function readJsonBody(request) {
  const chunks = [];
  for await (const chunk of request) {
    chunks.push(chunk);
  }
  return JSON.parse(Buffer.concat(chunks).toString("utf8"));
}

async function serveStaticFile(response, pathname) {
  const relativePath = pathname === "/apps/services/"
    ? "index.html"
    : pathname.slice("/apps/services/".length);
  const filePath = join(servicesRoot, relativePath);
  assert(
    filePath === join(servicesRoot, "index.html") || filePath.startsWith(`${servicesRoot}/`),
    "invalid Services asset path",
    { pathname, filePath },
  );
  const body = await readFile(filePath);
  const contentType = {
    ".css": "text/css",
    ".html": "text/html; charset=utf-8",
    ".js": "text/javascript",
    ".woff2": "font/woff2",
  }[extname(filePath)] || "application/octet-stream";
  response.writeHead(200, {
    "access-control-allow-origin": "null",
    "cache-control": "no-store",
    "content-length": body.length,
    "content-type": contentType,
  });
  response.end(body);
}

async function startServer() {
  const state = makeServicesState();
  const trace = {
    summaryRequests: [],
    offerPosts: [],
    requestFailures: [],
  };
  let baseUrl = null;
  const server = createServer(async (request, response) => {
    try {
      const url = new URL(request.url, "http://127.0.0.1");
      if (request.method === "OPTIONS") {
        response.writeHead(204, {
          "access-control-allow-origin": "null",
          "access-control-allow-methods": "GET,POST,OPTIONS",
          "access-control-allow-headers": "content-type,x-elastos-home-token",
        });
        response.end();
        return;
      }
      if (url.pathname === "/favicon.ico") {
        response.writeHead(204, {
          "access-control-allow-origin": "null",
          "cache-control": "no-store",
        });
        response.end();
        return;
      }
      if (url.pathname === "/fixture") {
        const scenario = url.searchParams.get("scenario") || "launched";
        const appSrc = scenario === "locked"
          ? "/apps/services/"
          : `/apps/services/?home_origin=${encodeURIComponent(baseUrl)}#home_token=${TOKEN}`;
        const body = Buffer.from(fixtureDocument(appSrc));
        response.writeHead(200, {
          "content-length": body.length,
          "content-type": "text/html; charset=utf-8",
        });
        response.end(body);
        return;
      }
      if (url.pathname === "/api/apps/services/summary") {
        trace.summaryRequests.push({
          homeToken: request.headers["x-elastos-home-token"] || null,
          method: request.method,
        });
        json(response, cloneState(state));
        return;
      }
      if (url.pathname === "/api/apps/services/offers" && request.method === "POST") {
        const body = await readJsonBody(request);
        trace.offerPosts.push({
          homeToken: request.headers["x-elastos-home-token"] || null,
          body,
        });
        const offerId = typeof body?.offer_id === "string" ? body.offer_id : "";
        const section = typeof body?.section === "string" ? body.section : "";
        const selected = body?.selected === true;
        if (offerId === "failing-browser-exit") {
          text(response, "Try again later.", 500);
          return;
        }
        if (section === "mine" && selected === false) {
          const offer = removeOffer(state.local_offers, offerId);
          if (offer) {
            upsertOffer(state.available_local_offers, {
              ...offer,
              enabled: false,
              status: undefined,
            });
          }
          json(response, cloneState(state));
          return;
        }
        if (section === "others" && selected === true) {
          const offer = removeOffer(state.available_remote_offers, offerId);
          if (offer) {
            upsertOffer(state.remote_offers, {
              ...offer,
              status: offer.grant_required ? "requested" : "active",
              enabled: offer.grant_required ? false : true,
            });
          }
          json(response, cloneState(state));
          return;
        }
        trace.requestFailures.push({
          kind: "unexpected-offer-update",
          body,
        });
        text(response, "Unexpected offer update.", 500);
        return;
      }
      if (url.pathname === "/apps/services/" || url.pathname.startsWith("/apps/services/")) {
        await serveStaticFile(response, url.pathname);
        return;
      }
      if (url.pathname.startsWith("/api/")) {
        trace.requestFailures.push({
          kind: "unexpected-api-route",
          path: url.pathname,
          method: request.method,
        });
        text(response, "Unexpected API route.", 500);
        return;
      }
      response.writeHead(404);
      response.end("not found");
    } catch (error) {
      trace.requestFailures.push({
        kind: "handler-error",
        path: request.url,
        message: error instanceof Error ? error.message : String(error),
      });
      response.writeHead(500, { "content-type": "text/plain; charset=utf-8" });
      response.end(error instanceof Error ? error.message : String(error));
    }
  });
  await new Promise((resolveListen) => server.listen(0, "127.0.0.1", resolveListen));
  const address = server.address();
  assert(address && typeof address === "object", "fixture server missing address");
  baseUrl = `http://127.0.0.1:${address.port}`;
  return {
    baseUrl,
    trace,
    async close() {
      await new Promise((resolveClose, rejectClose) => {
        server.close((error) => (error ? rejectClose(error) : resolveClose()));
      });
    },
  };
}

async function waitForServicesFrame(page) {
  for (let attempt = 0; attempt < 100; attempt += 1) {
    const frame = page.frames().find((candidate) =>
      candidate.url().includes("/apps/services/"),
    );
    if (frame) {
      await frame.waitForSelector("#services-shell, #locked-shell", { state: "attached" });
      return frame;
    }
    await page.waitForTimeout(50);
  }
  throw new Error("Services frame did not load");
}

async function attachPageDiagnostics(page) {
  const diagnostics = {
    consoleErrors: [],
    pageErrors: [],
    requestFailures: [],
  };
  page.on("console", (message) => {
    if (message.type() === "error") {
      diagnostics.consoleErrors.push(message.text());
    }
  });
  page.on("pageerror", (error) => {
    diagnostics.pageErrors.push(error.message);
  });
  page.on("requestfailed", (request) => {
    diagnostics.requestFailures.push({
      failure: request.failure()?.errorText || "unknown",
      method: request.method(),
      url: request.url(),
    });
  });
  return diagnostics;
}

async function assertNoTokenLocked(page, server, diagnostics) {
  await page.goto(`${server.baseUrl}/fixture?scenario=locked`, { waitUntil: "networkidle" });
  const frame = await waitForServicesFrame(page);
  await frame.waitForFunction(() => {
    const locked = document.getElementById("locked-shell");
    const shell = document.getElementById("services-shell");
    return (
      locked instanceof HTMLElement &&
      shell instanceof HTMLElement &&
      !locked.classList.contains("hidden") &&
      shell.classList.contains("hidden")
    );
  });
  assert(server.trace.summaryRequests.length === 0, "locked launch must not call Services summary");
  const homeMessages = await page.evaluate(() => window.__homeMessages);
  assert(homeMessages.length === 0, "locked launch must not announce app-ready", homeMessages);
  assert(diagnostics.consoleErrors.length === 0, "locked launch had console errors", diagnostics.consoleErrors);
  assert(diagnostics.pageErrors.length === 0, "locked launch had page errors", diagnostics.pageErrors);
  assert(diagnostics.requestFailures.length === 0, "locked launch had request failures", diagnostics.requestFailures);
}

async function bootLaunchedPage(page, server) {
  await page.goto(`${server.baseUrl}/fixture?scenario=launched`, { waitUntil: "networkidle" });
  const frame = await waitForServicesFrame(page);
  await page.waitForFunction(
    (token) =>
      Array.isArray(window.__homeMessages) &&
      window.__homeMessages.some(
        (entry) =>
          entry?.data?.type === "home:app-ready" &&
          entry?.data?.homeToken === token,
      ),
    TOKEN,
  );
  await frame.waitForFunction(
    () =>
      document.getElementById("locked-shell")?.classList.contains("hidden") === true &&
      document.getElementById("services-shell")?.classList.contains("hidden") === false &&
      document.getElementById("mine-count")?.textContent === "1" &&
      document.getElementById("others-count")?.textContent === "2",
  );
  return frame;
}

async function assertVerifiedLaunch(page, server, diagnostics) {
  const frame = await bootLaunchedPage(page, server);
  assert(server.trace.summaryRequests.length === 1, "verified launch must fetch one Services summary", server.trace.summaryRequests);
  assert(
    server.trace.summaryRequests[0]?.homeToken === TOKEN,
    "Services summary must use the exact Home token",
    server.trace.summaryRequests,
  );
  const homeMessages = await page.evaluate(() => window.__homeMessages);
  assert(homeMessages.length === 1, "verified launch must send one app-ready message", homeMessages);
  assert(
    homeMessages[0]?.data?.type === "home:app-ready" &&
      homeMessages[0]?.data?.homeToken === TOKEN,
    "verified launch must announce home:app-ready with the exact Home token",
    homeMessages,
  );
  const counts = await frame.evaluate(() => ({
    mine: document.getElementById("mine-count")?.textContent,
    others: document.getElementById("others-count")?.textContent,
    cardCount: document.querySelectorAll(".service-card").length,
    readOnlyBadgeCount: [...document.querySelectorAll(".status-badge")].filter(
      (node) => node.textContent?.trim() === "Managed by config",
    ).length,
  }));
  assert(
    counts.mine === "1" && counts.others === "2" && counts.cardCount === 6 && counts.readOnlyBadgeCount === 1,
    "verified launch must render the expected Services counts and cards",
    counts,
  );
  const readOnlyControlState = await frame.evaluate(() => {
    const cards = [...document.querySelectorAll(".service-card")];
    const card = cards.find((entry) => entry.textContent?.includes("Configured Browser Exit service"));
    if (!(card instanceof HTMLElement)) {
      throw new Error("configured Browser Exit service card missing");
    }
    return {
      hasManagedByConfig: card.textContent?.includes("Managed by config") === true,
      actionButtonCount: card.querySelectorAll("[data-service-offer-id]").length,
    };
  });
  assert(
    readOnlyControlState.hasManagedByConfig && readOnlyControlState.actionButtonCount === 0,
    "configured Browser Exit service must stay read-only",
    readOnlyControlState,
  );
  assert(diagnostics.consoleErrors.length === 0, "verified launch had console errors", diagnostics.consoleErrors);
  assert(diagnostics.pageErrors.length === 0, "verified launch had page errors", diagnostics.pageErrors);
  assert(diagnostics.requestFailures.length === 0, "verified launch had request failures", diagnostics.requestFailures);
}

async function assertSelectAvailableOffer(page, server) {
  const frame = await bootLaunchedPage(page, server);
  await frame.click('[data-service-offer-id="request-browser-engine"]');
  await frame.waitForFunction(
    () =>
      document.getElementById("others-count")?.textContent === "3" &&
      [...document.querySelectorAll(".service-card")].some((card) =>
        card.textContent?.includes("Lin's Browser Engine") &&
        card.textContent?.includes("Requested"),
      ),
  );
  assert(server.trace.offerPosts.length === 1, "selecting an available offer must send one POST", server.trace.offerPosts);
  assert(
    JSON.stringify(server.trace.offerPosts[0]) === JSON.stringify({
      homeToken: TOKEN,
      body: {
        offer_id: "request-browser-engine",
        section: "others",
        selected: true,
      },
    }),
    "available offer selection must send the exact typed update",
    server.trace.offerPosts,
  );
}

async function assertRemoveSelectedOffer(page, server) {
  const frame = await bootLaunchedPage(page, server);
  await frame.click('[data-service-offer-id="mine-browser-engine"]');
  await frame.waitForSelector('[data-confirm-service-action="apply"]');
  assert(server.trace.offerPosts.length === 0, "removing a selected offer must not POST before confirmation", server.trace.offerPosts);
  await frame.click('[data-confirm-service-action="apply"]');
  await frame.waitForFunction(
    () =>
      document.getElementById("mine-count")?.textContent === "0" &&
      !document.querySelector('[data-confirm-service-action="apply"]') &&
      [...document.querySelectorAll(".service-card")].every(
        (card) => !card.textContent?.includes("My Browser Engine is shared"),
      ),
  );
  assert(server.trace.offerPosts.length === 1, "confirmed selected-offer removal must send one POST", server.trace.offerPosts);
  assert(
    JSON.stringify(server.trace.offerPosts[0]) === JSON.stringify({
      homeToken: TOKEN,
      body: {
        offer_id: "mine-browser-engine",
        section: "mine",
        selected: false,
      },
    }),
    "selected offer removal must send the exact typed update",
    server.trace.offerPosts,
  );
}

async function assertFailedUpdate(page, server) {
  const frame = await bootLaunchedPage(page, server);
  await frame.click('[data-section-target="other-services"]');
  await frame.click('[data-service-offer-id="failing-browser-exit"]');
  await frame.waitForFunction(
    () =>
      document.getElementById("services-status")?.dataset?.tone === "error" &&
      document.getElementById("services-status")?.textContent?.trim() === "Services could not be updated.",
  );
  const failureState = await frame.evaluate(() => ({
    othersCount: document.getElementById("others-count")?.textContent,
    requestCardStillAvailable: [...document.querySelectorAll(".service-card")].some((card) =>
      card.textContent?.includes("Kira's Browser Exit service") &&
      card.textContent?.includes("Subscribe"),
    ),
    tone: document.getElementById("services-status")?.dataset?.tone,
    text: document.getElementById("services-status")?.textContent?.trim(),
  }));
  assert(
    failureState.othersCount === "2" &&
      failureState.requestCardStillAvailable &&
      failureState.tone === "error" &&
      failureState.text === "Services could not be updated.",
    "failed update must keep the last selected state and show a clear error",
    failureState,
  );
}

async function assertRectWithinBounds(name, rect, bounds, details) {
  assert(rect.left >= bounds.left - 0.5, `${name} escapes the left bound`, details);
  assert(rect.top >= bounds.top - 0.5, `${name} escapes the top bound`, details);
  assert(rect.right <= bounds.right + 0.5, `${name} escapes the right bound`, details);
  assert(rect.bottom <= bounds.bottom + 0.5, `${name} escapes the bottom bound`, details);
}

async function assertLayoutScenario(width, height, screenshotPath) {
  const server = await startServer();
  const browser = await chromium.launch({
    headless: true,
    executablePath: brave,
  });
  const page = await browser.newPage({ viewport: { width, height } });
  page.setDefaultTimeout(5000);
  page.setDefaultNavigationTimeout(5000);
  const diagnostics = await attachPageDiagnostics(page);
  try {
    const frame = await bootLaunchedPage(page, server);
    await frame.click('[data-section-target="other-services"]');
    await frame.click('[data-section-target="mine-services"]');
    await frame.click('[data-service-offer-id="mine-browser-engine"]');
    await frame.waitForSelector('[data-confirm-service-action="apply"]');
    await frame.waitForFunction(
      (expectedNarrow) => {
        const content = document.querySelector(".settings-content-container");
        const sidebar = document.querySelector(".settings-sidebar");
        if (!(content instanceof HTMLElement) || !(sidebar instanceof HTMLElement)) {
          return false;
        }
        const contentStyle = window.getComputedStyle(content);
        const sidebarStyle = window.getComputedStyle(sidebar);
        const isNarrow = window.innerWidth <= 720;
        return (
          isNarrow === expectedNarrow &&
          contentStyle.overflowY === "auto" &&
          (expectedNarrow ? sidebarStyle.width !== "220px" : sidebarStyle.width === "220px")
        );
      },
      width <= 640,
    );
    const metrics = await frame.evaluate(() => {
      const rectOf = (selector) => {
        const node = document.querySelector(selector);
        if (!(node instanceof HTMLElement)) {
          throw new Error(`missing ${selector}`);
        }
        const rect = node.getBoundingClientRect();
        return {
          left: rect.left,
          top: rect.top,
          right: rect.right,
          bottom: rect.bottom,
          width: rect.width,
          height: rect.height,
        };
      };
      return {
        innerWidth: window.innerWidth,
        innerHeight: window.innerHeight,
        documentScrollWidth: document.scrollingElement?.scrollWidth ?? 0,
        sidebar: rectOf(".settings-sidebar"),
        toolbar: rectOf(".services-toolbar"),
        content: {
          ...rectOf(".settings-content-container"),
          clientWidth: document.querySelector(".settings-content-container")?.clientWidth ?? 0,
          scrollWidth: document.querySelector(".settings-content-container")?.scrollWidth ?? 0,
        },
        confirm: rectOf(".service-confirm"),
        otherSection: rectOf("#other-services"),
        mineSection: rectOf("#mine-services"),
      };
    });
    assert(
      metrics.documentScrollWidth <= metrics.innerWidth,
      "Services document must not scroll horizontally",
      metrics,
    );
    assert(
      metrics.content.scrollWidth <= metrics.content.clientWidth,
      "Services content container must not scroll horizontally",
      metrics,
    );
    await assertRectWithinBounds("sidebar", metrics.sidebar, {
      left: 0,
      top: 0,
      right: metrics.innerWidth,
      bottom: metrics.innerHeight,
    }, metrics);
    await assertRectWithinBounds("toolbar", metrics.toolbar, {
      left: metrics.content.left,
      top: 0,
      right: metrics.innerWidth,
      bottom: metrics.innerHeight,
    }, metrics);
    await assertRectWithinBounds("confirmation", metrics.confirm, {
      left: metrics.content.left,
      top: metrics.content.top,
      right: metrics.content.right,
      bottom: metrics.innerHeight,
    }, metrics);
    await assertRectWithinBounds("mine section", metrics.mineSection, {
      left: metrics.content.left,
      top: metrics.content.top,
      right: metrics.content.right,
      bottom: metrics.innerHeight,
    }, metrics);
    await frame.evaluate(() => {
      document.querySelector(".settings-content-container")?.scrollTo({ top: 0, behavior: "instant" });
    });
    await page.screenshot({ path: screenshotPath, fullPage: false });
    assert(diagnostics.consoleErrors.length === 0, "Services layout had console errors", diagnostics.consoleErrors);
    assert(diagnostics.pageErrors.length === 0, "Services layout had page errors", diagnostics.pageErrors);
    assert(diagnostics.requestFailures.length === 0, "Services layout had request failures", diagnostics.requestFailures);
    assert(server.trace.requestFailures.length === 0, "Services layout hit unexpected fixture routes", server.trace.requestFailures);
  } finally {
    await page.close().catch(() => {});
    await browser.close().catch(() => {});
    await server.close().catch(() => {});
  }
}

async function runScenario(callback) {
  const server = await startServer();
  const browser = await chromium.launch({
    headless: true,
    executablePath: brave,
  });
  try {
    const page = await browser.newPage({ viewport: { width: 1280, height: 900 } });
    page.setDefaultTimeout(5000);
    page.setDefaultNavigationTimeout(5000);
    const diagnostics = await attachPageDiagnostics(page);
    try {
      await callback({ browser, diagnostics, page, server });
    } finally {
      await page.close().catch(() => {});
    }
  } finally {
    await browser.close().catch(() => {});
    await server.close().catch(() => {});
  }
}

async function main() {
  const tmpOutputDir = join(tmpdir(), "services-uiux-smoke");
  await mkdir(tmpOutputDir, { recursive: true });
  const desktopShot = join(tmpOutputDir, "services-uiux-desktop-1280x900.png");
  const narrowShot = join(tmpOutputDir, "services-uiux-narrow-640x900.png");
  await runScenario(({ diagnostics, page, server }) =>
    assertNoTokenLocked(page, server, diagnostics));
  await runScenario(({ diagnostics, page, server }) =>
    assertVerifiedLaunch(page, server, diagnostics));
  await runScenario(({ page, server }) =>
    assertSelectAvailableOffer(page, server));
  await runScenario(({ page, server }) =>
    assertRemoveSelectedOffer(page, server));
  await runScenario(({ page, server }) =>
    assertFailedUpdate(page, server));

  await assertLayoutScenario(1280, 900, desktopShot);
  await assertLayoutScenario(640, 900, narrowShot);
  await writeFile(
    join(tmpOutputDir, "services-uiux-smoke-results.json"),
    JSON.stringify({
      desktopShot,
      narrowShot,
    }, null, 2),
  );
  console.log(`services-uiux-smoke: PASS\nscreenshots:\n${desktopShot}\n${narrowShot}`);
}

main().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
