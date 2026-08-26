#!/usr/bin/env node

import { createServer } from "node:http";
import { createRequire } from "node:module";
import { readFile } from "node:fs/promises";
import { extname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = resolve(fileURLToPath(new URL("../", import.meta.url)));
const browserRoot = join(repoRoot, "capsules/inbox/browser");
const brave = process.env.BRAVE_BIN || "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser";
const require = createRequire(new URL("../elastos/tools/browser-playwright-engine/package.json", import.meta.url));
const { chromium } = require("playwright");

function assert(condition, message, details = undefined) {
  if (!condition) {
    throw new Error(`${message}${details ? `\n${JSON.stringify(details, null, 2)}` : ""}`);
  }
}

function json(response, value, status = 200, headers = {}) {
  const body = Buffer.from(JSON.stringify(value));
  response.writeHead(status, {
    "access-control-allow-origin": "*",
    "cache-control": "no-store",
    "content-length": body.length,
    "content-type": "application/json",
    ...headers,
  });
  response.end(body);
}

function inboxSummary() {
  const now = Math.floor(Date.now() / 1000);
  return {
    notifications: {
      attention_count: 1,
      unread_count: 0,
      entries: [
        {
          id: "request-review",
          kind: "contact_request",
          title: "Contact request",
          body: "Jordan wants to connect.",
          severity: "attention",
          read: true,
          created_at: now - 90,
          source_app: "people",
          action_ref: { action_id: "contact-accept-request:request-review" },
        },
        {
          id: "request-wallet",
          kind: "wallet_approval_request",
          title: "Wallet review",
          body: "Review the exact wallet request.",
          severity: "info",
          read: true,
          created_at: now - 3600,
          source_app: "wallet",
          action_ref: { action_id: "wallet-review-request:wallet-request-1" },
        },
      ],
    },
  };
}

async function serveFile(response, pathname) {
  const relative = pathname === "/apps/inbox/" ? "index.html" : pathname.slice("/apps/inbox/".length);
  const path = join(browserRoot, relative);
  assert(path.startsWith(`${browserRoot}/`) || path === join(browserRoot, "index.html"), "invalid Inbox asset path");
  const body = await readFile(path);
  const contentType = {
    ".css": "text/css",
    ".html": "text/html; charset=utf-8",
    ".js": "text/javascript",
    ".woff2": "font/woff2",
    ".png": "image/png",
  }[extname(path)] || "application/octet-stream";
  response.writeHead(200, {
    "access-control-allow-origin": "*",
    "cache-control": "no-store",
    "content-length": body.length,
    "content-type": contentType,
  });
  response.end(body);
}

function startServer() {
  const server = createServer(async (request, response) => {
    try {
      const url = new URL(request.url, "http://localhost");
      if (url.pathname === "/api/apps/inbox/summary") {
        json(response, inboxSummary());
        return;
      }
      if (url.pathname === "/api/apps/inbox/actions") {
        json(response, {});
        return;
      }
      if (url.pathname.startsWith("/apps/inbox/")) {
        await serveFile(response, url.pathname);
        return;
      }
      response.writeHead(404).end("not found");
    } catch (error) {
      response.writeHead(500).end(String(error.stack || error));
    }
  });
  return new Promise((resolve) => {
    server.listen(0, "127.0.0.1", () => resolve(server));
  });
}

async function launchPage(page, baseUrl, presentation = "window") {
  const query = presentation === "rail"
    ? `?home_origin=${encodeURIComponent(baseUrl)}&presentation=rail`
    : `?home_origin=${encodeURIComponent(baseUrl)}`;
  await page.goto(`${baseUrl}/apps/inbox/${query}#home_token=inbox-layout-token`, {
    waitUntil: "networkidle",
  });
  await page.waitForSelector("#inbox-shell:not(.hidden)");
}

async function noHorizontalOverflow(page, label) {
  const overflow = await page.evaluate(() => ({
    clientWidth: document.documentElement.clientWidth,
    scrollWidth: document.documentElement.scrollWidth,
  }));
  assert(
    overflow.scrollWidth <= overflow.clientWidth,
    `${label}: Inbox overflowed horizontally`,
    overflow,
  );
}

async function run() {
  const server = await startServer();
  const port = server.address().port;
  const baseUrl = `http://127.0.0.1:${port}`;
  const browser = await chromium.launch({
    channel: "chromium",
    executablePath: brave,
    headless: true,
  });

  try {
    const page = await browser.newPage({ viewport: { width: 1280, height: 900 } });
    await launchPage(page, baseUrl);

    const desktop = await page.evaluate(() => {
      const entryRows = document.getElementById("entry-rows");
      const entryDetail = document.getElementById("entry-detail");
      const rowCount = entryRows?.querySelectorAll(".entry-row").length || 0;
      const detailTitle = entryDetail?.querySelector(".entry-title")?.textContent?.trim() || "";
      return {
        rowCount,
        detailTitle,
        splitColumns: getComputedStyle(document.getElementById("entry-split")).gridTemplateColumns,
        detailHidden: getComputedStyle(entryDetail).display === "none",
      };
    });
    assert(desktop.rowCount === 2, "desktop window must render two request rows", desktop);
    assert(desktop.detailTitle === "Contact request", "desktop detail must select the first request", desktop);
    assert(desktop.splitColumns.split(" ").length >= 2, "desktop window must keep a list/detail split", desktop);
    assert(desktop.detailHidden === false, "desktop detail must stay visible in window mode", desktop);
    await noHorizontalOverflow(page, "desktop");

    await page.getByRole("button", { name: "Needs Review" }).click();
    const reviewFilter = await page.evaluate(() => ({
      listTitle: document.getElementById("list-title")?.textContent?.trim() || "",
      rowCount: document.querySelectorAll("#entry-rows .entry-row").length,
      detailTitle: document.querySelector("#entry-detail .entry-title")?.textContent?.trim() || "",
    }));
    assert(reviewFilter.listTitle === "Needs Review", "review filter must update the list title", reviewFilter);
    assert(reviewFilter.rowCount === 1, "review filter must use only current attention entries", reviewFilter);
    assert(reviewFilter.detailTitle === "Contact request", "review filter must keep the matching detail entry", reviewFilter);

    await page.getByRole("button", { name: "Pending" }).click();
    await page.getByRole("option").nth(1).click();
    const secondSelection = await page.evaluate(() => ({
      rowCount: document.querySelectorAll("#entry-rows .entry-row").length,
      detailTitle: document.querySelector("#entry-detail .entry-title")?.textContent?.trim() || "",
    }));
    assert(secondSelection.rowCount === 2, "pending filter must restore all current entries", secondSelection);
    assert(secondSelection.detailTitle === "Wallet review", "row selection must drive the detail panel", secondSelection);

    await page.setViewportSize({ width: 820, height: 760 });
    const compact = await page.evaluate(() => ({
      splitColumns: getComputedStyle(document.getElementById("entry-split")).gridTemplateColumns,
      sidebarHidden: getComputedStyle(document.querySelector(".sidebar")).display === "none",
      detailHidden: getComputedStyle(document.getElementById("entry-detail")).display === "none",
    }));
    assert(compact.sidebarHidden, "compact window must hide the sidebar filter column", compact);
    assert(compact.splitColumns.split(" ").length >= 2, "compact window must keep list/detail visible", compact);
    assert(!compact.detailHidden, "compact window must keep the detail panel visible", compact);
    await noHorizontalOverflow(page, "compact");

    await page.setViewportSize({ width: 520, height: 900 });
    const mobile = await page.evaluate(() => {
      const rows = document.getElementById("entry-rows").getBoundingClientRect();
      const detail = document.getElementById("entry-detail").getBoundingClientRect();
      return {
        splitColumns: getComputedStyle(document.getElementById("entry-split")).gridTemplateColumns,
        rowsTop: rows.top,
        detailTop: detail.top,
      };
    });
    assert(mobile.splitColumns === "520px" || mobile.splitColumns.split(" ").length === 1, "mobile must stack the list/detail split", mobile);
    assert(mobile.detailTop > mobile.rowsTop, "mobile detail must sit below the list", mobile);
    await noHorizontalOverflow(page, "mobile");

    const railPage = await browser.newPage({ viewport: { width: 420, height: 900 } });
    await launchPage(railPage, baseUrl, "rail");
    const rail = await railPage.evaluate(() => ({
      presentation: document.documentElement.dataset.inboxPresentation,
      sidebarHidden: getComputedStyle(document.querySelector(".sidebar")).display === "none",
      toolbarHidden: getComputedStyle(document.querySelector(".toolbar")).display === "none",
      detailHidden: getComputedStyle(document.getElementById("entry-detail")).display === "none",
      railCards: document.querySelectorAll(".entry-rail-card").length,
      rows: document.querySelectorAll(".entry-row").length,
    }));
    assert(rail.presentation === "rail", "rail view must set the presentation dataset", rail);
    assert(rail.sidebarHidden, "rail view must hide capsule sidebar chrome", rail);
    assert(rail.toolbarHidden, "rail view must hide capsule toolbar chrome", rail);
    assert(rail.detailHidden, "rail view must hide the detail panel", rail);
    assert(rail.railCards === 2, "rail view must render inline request cards", rail);
    assert(rail.rows === 0, "rail view must not render window-mode rows", rail);
    await noHorizontalOverflow(railPage, "rail");
  } finally {
    server.close();
    await browser.close();
  }
}

await run();
console.log("inbox-product-layout-smoke: PASS");
