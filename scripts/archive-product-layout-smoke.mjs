#!/usr/bin/env node

import { createServer } from "node:http";
import { createRequire } from "node:module";
import { readFile } from "node:fs/promises";
import { extname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = resolve(fileURLToPath(new URL("../", import.meta.url)));
const browserRoot = join(repoRoot, "capsules/archive-manager/browser");
const brave = process.env.BRAVE_BIN || "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser";
const require = createRequire(new URL("../elastos/tools/browser-playwright-engine/package.json", import.meta.url));
const { chromium } = require("playwright");

const token = "archive-layout-test-token";
const archiveUri = "localhost://Users/archive/Documents/Portable.zip";
const previewText = "Portable archive preview text.";
const entries = [
  {
    path: "docs/guide.md",
    kind: "file",
    size: 128,
    modified_at: 1_780_000_000,
    mime: "text/markdown",
    safety: { status: "safe" },
  },
  {
    path: "images/logo.txt",
    kind: "file",
    size: 64,
    modified_at: 1_780_000_100,
    mime: "text/plain",
    safety: { status: "safe" },
  },
  {
    path: "blocked/device.bin",
    kind: "file",
    size: 32,
    modified_at: 1_780_000_200,
    mime: "application/octet-stream",
    safety: { status: "blocked" },
  },
];
const roots = [
  { id: "desktop", label: "Desktop", uri: "localhost://Users/archive/Desktop", kind: "directory" },
  { id: "documents", label: "Documents", uri: "localhost://Users/archive/Documents", kind: "directory" },
  { id: "trash", label: "Trash", uri: "localhost://Users/archive/.Trash", kind: "directory" },
];
const archiveSupport = {
  schema: "elastos.library.archive-support/v1",
  family: "zip",
  status: "extractable",
};

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

async function serveFile(response, pathname) {
  const relative = pathname === "/apps/archive-manager/" ? "index.html" : pathname.slice("/apps/archive-manager/".length);
  const path = join(browserRoot, relative);
  assert(path.startsWith(`${browserRoot}/`) || path === join(browserRoot, "index.html"), "invalid Archive asset path");
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

function archiveObject() {
  return {
    uri: archiveUri,
    name: "Portable.zip",
    mime: "application/zip",
    content_cid: "bafyarchivefixture",
    metadata: { archive_support: archiveSupport },
  };
}

function buildShellDocument(appSrc) {
  return `<!doctype html>
<html>
<body style="margin:0">
  <iframe id="app" title="Archive" sandbox="allow-forms allow-modals allow-pointer-lock allow-scripts" src="${appSrc}" style="border:0;width:100%;height:100vh"></iframe>
  <script>
    window.addEventListener("message", (event) => {
      if (event.data?.type !== "fixture:post-to-app") return;
      document.getElementById("app").contentWindow.postMessage(event.data.payload, "*");
    });
  </script>
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
    window.postToArchive = (payload) => {
      const shell = document.getElementById("shell");
      shell.contentWindow.postMessage({ type: "fixture:post-to-app", payload }, "*");
    };
    window.postToArchiveDirect = (payload) => {
      const shell = document.getElementById("shell");
      const archiveFrame = shell.contentWindow?.frames?.[0];
      if (!archiveFrame) {
        throw new Error("Archive child frame is not ready");
      }
      archiveFrame.postMessage(payload, "*");
    };
  </script>
</body>
</html>`;
}

function startServer() {
  const requestLog = [];
  const extractBodies = [];
  const notFoundPaths = [];
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
      if (url.pathname === "/fixture-empty") {
        const topOrigin = `http://${request.headers.host}`;
        const appSrc = `/apps/archive-manager/?home_origin=${encodeURIComponent(topOrigin)}#home_token=${encodeURIComponent(token)}`;
        const shellSrc = `/fixture-shell?app_src=${encodeURIComponent(appSrc)}`;
        const body = Buffer.from(buildFixtureHtml(shellSrc));
        response.writeHead(200, { "content-length": body.length, "content-type": "text/html; charset=utf-8" });
        response.end(body);
        return;
      }
      if (url.pathname === "/fixture-loaded") {
        const topOrigin = `http://${request.headers.host}`;
        const appSrc = `/apps/archive-manager/?uri=${encodeURIComponent(archiveUri)}&name=Portable.zip&mime=application%2Fzip&home_origin=${encodeURIComponent(topOrigin)}#home_token=${encodeURIComponent(token)}`;
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
      if (url.pathname.startsWith("/apps/archive-manager/")) {
        await serveFile(response, url.pathname);
        return;
      }
      if (url.pathname === "/api/viewers/archive-manager/library-roots") {
        requestLog.push({ method: request.method, path: url.pathname, token: request.headers["x-elastos-home-token"] || null });
        json(response, { data: { roots } });
        return;
      }
      if (url.pathname === "/api/viewers/archive-manager/library-object") {
        const envelope = {
          method: request.method,
          path: url.pathname,
          query: Object.fromEntries(url.searchParams.entries()),
          token: request.headers["x-elastos-home-token"] || null,
        };
        requestLog.push(envelope);
        if (request.method === "GET" && url.searchParams.get("stat_only") === "true") {
          json(response, { data: { object: archiveObject() } });
          return;
        }
        if (request.method === "GET" && url.searchParams.get("entries") === "true") {
          json(response, {
            data: {
              object: archiveObject(),
              entries,
              limits: { returned_entries: entries.length, truncated: false },
            },
          });
          return;
        }
        if (request.method === "GET" && url.searchParams.has("preview_entry")) {
          const path = url.searchParams.get("preview_entry");
          const entry = entries.find((item) => item.path === path) || null;
          json(response, {
            data: {
              entry,
              preview: entry
                ? { text: previewText, truncated: false }
                : null,
            },
          });
          return;
        }
        if (request.method === "POST") {
          let bodyText = "";
          for await (const chunk of request) {
            bodyText += Buffer.from(chunk).toString("utf8");
          }
          const body = bodyText ? JSON.parse(bodyText) : {};
          extractBodies.push(body);
          json(response, {
            data: {
              receipt: {
                status: "completed",
                progress: {
                  written_entries: Array.isArray(body.entries) ? body.entries.length : 0,
                  skipped_entries: 0,
                  blocked_entries: 0,
                },
              },
            },
          });
          return;
        }
      }
      notFoundPaths.push(url.pathname);
      response.writeHead(404).end("not found");
    } catch (error) {
      response.writeHead(500, { "content-type": "text/plain; charset=utf-8" });
      response.end(error.stack || String(error));
    }
  });
  return { extractBodies, notFoundPaths, requestLog, server };
}

async function waitForArchiveFrame(page) {
  for (let attempt = 0; attempt < 50; attempt += 1) {
    const frame = page.frames().find((entry) => entry.url().includes("/apps/archive-manager/"));
    if (frame) {
      return frame;
    }
    await page.waitForTimeout(100);
  }
  throw new Error(`Archive iframe did not appear\n${JSON.stringify(page.frames().map((entry) => entry.url()), null, 2)}`);
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
    })),
  );
}

async function postToArchive(page, payload) {
  await page.evaluate((message) => {
    window.postToArchive(message);
  }, payload);
}

async function postToArchiveDirect(page, payload) {
  await page.evaluate((message) => {
    window.postToArchiveDirect(message);
  }, payload);
}

async function waitForSemanticWidth(frame, expectedWidth) {
  await frame.waitForFunction((width) => window.innerWidth === width, expectedWidth);
}

async function run() {
  const { extractBodies, notFoundPaths, requestLog, server } = startServer();
  const browser = await chromium.launch({
    executablePath: brave,
    headless: true,
  });
  const page = await browser.newPage();
  const pageErrors = [];
  const consoleErrors = [];
  const requestFailures = [];
  const responseFailures = [];
  try {
    const port = await new Promise((resolvePort, reject) => {
      server.once("error", reject);
      server.listen(0, "127.0.0.1", () => {
        const address = server.address();
        if (!address || typeof address === "string") {
          reject(new Error("missing Archive smoke address"));
          return;
        }
        resolvePort(address.port);
      });
    });
    page.on("pageerror", (error) => pageErrors.push(String(error)));
    page.on("console", (message) => {
      if (message.type() === "error") {
        consoleErrors.push(message.text());
      }
    });
    page.on("requestfailed", (request) => {
      requestFailures.push({ url: request.url(), error: request.failure()?.errorText || "request failed" });
    });
    page.on("response", (response) => {
      if (response.status() >= 400) {
        responseFailures.push({ url: response.url(), status: response.status() });
      }
    });

    await page.goto(`http://127.0.0.1:${port}/fixture-empty`);
    const emptyFrame = await waitForArchiveFrame(page);
    await page.waitForFunction(() => window.homeMessages.length >= 2);
    const emptyMessages = await readHomeMessages(page);
    assert(
      emptyMessages[0]?.type === "home:app-ready" && emptyMessages[1]?.type === "home:menu-manifest",
      "Archive must announce Home ready before its menu manifest.",
      emptyMessages,
    );
    assert(
      emptyMessages[0]?.homeToken === token && emptyMessages[1]?.homeToken === token,
      "Archive Home messages must keep the launch token bound.",
      emptyMessages,
    );
    await emptyFrame.waitForSelector("#empty-state:not([hidden])");
    await emptyFrame.locator("#open-existing-archive").click();
    await page.waitForFunction(() => window.homeMessages.some((entry) => entry.message?.type === "home:open-target" && entry.message?.query?.mode === "archive-open"));
    await emptyFrame.locator("#make-new-archive").click();
    await page.waitForFunction(() => window.homeMessages.some((entry) => entry.message?.type === "home:open-target" && entry.message?.query?.mode === "archive-create"));
    await postToArchive(page, { type: "elastos:menu-command", cmd: "open-archive" });
    await page.waitForFunction(() =>
      window.homeMessages.filter((entry) => entry.message?.type === "home:open-target" && entry.message?.query?.mode === "archive-open").length >= 2,
    );
    await postToArchive(page, { type: "elastos:menu-command", cmd: "new-archive" });
    await page.waitForFunction(() =>
      window.homeMessages.filter((entry) => entry.message?.type === "home:open-target" && entry.message?.query?.mode === "archive-create").length >= 2,
    );
    const countsBeforeReject = await page.evaluate(() => ({
      openCount: window.homeMessages.filter((entry) => entry.message?.type === "home:open-target" && entry.message?.query?.mode === "archive-open").length,
      createCount: window.homeMessages.filter((entry) => entry.message?.type === "home:open-target" && entry.message?.query?.mode === "archive-create").length,
    }));
    await postToArchiveDirect(page, { type: "elastos:menu-command", cmd: "open-archive" });
    await postToArchive(page, { type: "elastos:menu-command", cmd: "new-archive" });
    await page.waitForFunction(
      (beforeCreateCount) =>
        window.homeMessages.filter((entry) => entry.message?.type === "home:open-target" && entry.message?.query?.mode === "archive-create").length === beforeCreateCount + 1,
      countsBeforeReject.createCount,
    );
    const countsAfterReject = await page.evaluate(() => ({
      openCount: window.homeMessages.filter((entry) => entry.message?.type === "home:open-target" && entry.message?.query?.mode === "archive-open").length,
      createCount: window.homeMessages.filter((entry) => entry.message?.type === "home:open-target" && entry.message?.query?.mode === "archive-create").length,
    }));
    assert(
      countsAfterReject.openCount === countsBeforeReject.openCount &&
        countsAfterReject.createCount === countsBeforeReject.createCount + 1,
      "Archive must reject menu commands from a non-parent source while still accepting the next trusted parent command.",
      { countsBeforeReject, countsAfterReject },
    );

    await page.goto(`http://127.0.0.1:${port}/fixture-loaded`);
    const frame = await waitForArchiveFrame(page);
    await frame.waitForSelector(".entry-row");
    assert(
      requestLog.some((entry) => entry.path === "/api/viewers/archive-manager/library-roots") &&
        requestLog.some((entry) => entry.path === "/api/viewers/archive-manager/library-object" && entry.query?.stat_only === "true") &&
        requestLog.some((entry) => entry.path === "/api/viewers/archive-manager/library-object" && entry.query?.entries === "true"),
      "Archive must load roots, stat-only metadata, and entries through the existing viewer routes.",
      requestLog,
    );

    const initialState = await frame.evaluate(() => ({
      destinationUri: document.querySelector("#destination-uri")?.value || "",
      title: document.querySelector("#title")?.textContent || "",
      buttonLabels: [
        document.querySelector("#open-archive-button")?.textContent || "",
        document.querySelector("#new-archive-button")?.textContent || "",
      ],
    }));
    assert(
      initialState.destinationUri === "localhost://Users/archive/Documents" &&
        initialState.title === "Portable.zip" &&
        initialState.buttonLabels[0] === "Open" &&
        initialState.buttonLabels[1] === "New ZIP",
      "Archive loaded state must keep the current destination, title, and open or new controls.",
      initialState,
    );

    const viewports = [640, 1280];
    for (const width of viewports) {
      await page.setViewportSize({ width, height: 900 });
      await waitForSemanticWidth(frame, width);
      const layout = await frame.evaluate(() => {
        const list = document.querySelector(".list-panel");
        const side = document.querySelector(".side-panel");
        const listRect = list?.getBoundingClientRect();
        const sideRect = side?.getBoundingClientRect();
        return {
          width: window.innerWidth,
          overflow: Math.max(document.documentElement.scrollWidth, document.body.scrollWidth) - document.documentElement.clientWidth,
          stacked: !!listRect && !!sideRect && sideRect.top >= listRect.bottom - 1,
          split: !!listRect && !!sideRect && sideRect.left >= listRect.right - 1,
        };
      });
      assert(layout.overflow <= 1, "Archive layout must not overflow horizontally.", layout);
      if (width <= 980) {
        assert(layout.stacked, "Archive must stack its side panel on narrow widths.", layout);
      } else {
        assert(layout.split, "Archive must keep a split layout on wide widths.", layout);
      }
    }

    await frame.locator("#entry-search").fill("guide");
    await frame.waitForFunction(() => document.querySelectorAll(".entry-row").length === 1);
    const searchState = await frame.evaluate(() => ({
      rows: [...document.querySelectorAll(".entry-row .entry-title")].map((node) => node.textContent || ""),
    }));
    assert(searchState.rows.length === 1 && searchState.rows[0] === "docs/guide.md", "Archive search must filter the current entry rows.", searchState);
    await frame.locator("#entry-search").fill("");
    await frame.waitForFunction(() => document.querySelectorAll(".entry-row").length === 3);

    await frame.locator('.entry-row[data-path="docs/guide.md"]').click();
    await frame.waitForFunction((text) => document.querySelector("#entry-preview")?.textContent?.includes(text), previewText);
    assert(
      requestLog.some((entry) => entry.path === "/api/viewers/archive-manager/library-object" && entry.query?.preview_entry === "docs/guide.md"),
      "Archive must preview through the current preview route.",
      requestLog,
    );

    await frame.locator('.entry-row[data-path="docs/guide.md"]').focus();
    await page.keyboard.press("Space");
    await frame.waitForFunction(() => document.querySelector('.entry-check[value="docs/guide.md"]')?.checked === true);
    await postToArchive(page, { type: "elastos:menu-command", cmd: "clear-selection" });
    await frame.waitForFunction(() => !document.querySelector('.entry-check[value="docs/guide.md"]')?.checked);
    await postToArchive(page, { type: "elastos:menu-command", cmd: "select-all-safe" });
    await frame.waitForFunction(() => document.querySelectorAll(".entry-check:checked").length === 2);
    const selectionState = await frame.evaluate(() => ({
      checked: [...document.querySelectorAll(".entry-check:checked")].map((node) => node.value),
      blockedDisabled: document.querySelector('.entry-check[value="blocked/device.bin"]')?.disabled === true,
      status: document.querySelector("#entries-pill")?.textContent || "",
    }));
    assert(
      selectionState.checked.length === 2 &&
        selectionState.checked.includes("docs/guide.md") &&
        selectionState.checked.includes("images/logo.txt") &&
        selectionState.blockedDisabled,
      "Archive safe selection must keep blocked files out of the checked set.",
      selectionState,
    );

    await frame.locator("#extract-selected").click();
    await frame.waitForFunction(() => document.querySelector("#extract-status")?.textContent?.includes("written"));
    assert(
      extractBodies[0]?.destination_uri === "localhost://Users/archive/Documents" &&
        extractBodies[0]?.conflict_policy === "keep_both" &&
        Array.isArray(extractBodies[0]?.entries) &&
        extractBodies[0].entries.length === 2 &&
        extractBodies[0].entries.every((entry) => entry !== "blocked/device.bin"),
      "Archive Extract selected must keep the existing payload and safe-only entries.",
      extractBodies,
    );

    await postToArchive(page, { type: "elastos:menu-command", cmd: "clear-selection" });
    await frame.waitForFunction(() => document.querySelectorAll(".entry-check:checked").length === 0);
    await frame.locator("#extract-all").click();
    await frame.waitForFunction(() => document.querySelector("#extract-status")?.textContent?.includes("2 written"));
    assert(
      Array.isArray(extractBodies[1]?.entries) &&
        extractBodies[1].entries.length === 2 &&
        extractBodies[1].entries.includes("docs/guide.md") &&
        extractBodies[1].entries.includes("images/logo.txt"),
      "Archive Extract all must keep the existing safe-entry extract path.",
      extractBodies,
    );

    assert(notFoundPaths.length === 0, "Archive layout smoke hit an unknown server path.", notFoundPaths);
    assert(responseFailures.length === 0, "Archive layout smoke hit an HTTP error response.", responseFailures);
    assert(pageErrors.length === 0, "Archive layout smoke hit a page error.", pageErrors);
    assert(consoleErrors.length === 0, "Archive layout smoke hit a console error.", consoleErrors);
    assert(requestFailures.length === 0, "Archive layout smoke hit a failed request.", requestFailures);
    console.log("archive-product-layout-smoke: OK");
  } finally {
    await browser.close().catch(() => {});
    await new Promise((resolveClose) => server.close(() => resolveClose()));
  }
}

run().catch((error) => {
  console.error(error.stack || String(error));
  process.exitCode = 1;
});
