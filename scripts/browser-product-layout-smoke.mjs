#!/usr/bin/env node

import { createServer } from "node:http";
import { createRequire } from "node:module";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { extname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = resolve(fileURLToPath(new URL("../", import.meta.url)));
const browserRoot = join(repoRoot, "capsules/browser/browser");
const brave = process.env.BRAVE_BIN || "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser";
const require = createRequire(new URL("../elastos/tools/browser-playwright-engine/package.json", import.meta.url));
const { chromium } = require("playwright");

function assert(condition, message, details = undefined) {
  if (!condition) {
    throw new Error(`${message}${details ? `\n${JSON.stringify(details, null, 2)}` : ""}`);
  }
}

async function buildFixtureRoot() {
  const fixtureRoot = await mkdtemp(join(tmpdir(), "browser-product-layout-"));
  const indexHtml = await readFile(join(browserRoot, "index.html"), "utf8");
  const staticHtml = indexHtml.replace(
    /\s*<script type="module" src="\.\/browser\.js\?v=[^"]+"><\/script>\s*/u,
    "\n",
  );
  await writeFile(join(fixtureRoot, "index.html"), staticHtml);
  return fixtureRoot;
}

async function serveFile(response, fixtureRoot, pathname) {
  if (pathname === "/favicon.ico") {
    response.writeHead(204);
    response.end();
    return;
  }
  const relative = pathname === "/" ? "index.html" : pathname.slice(1);
  const root = relative === "index.html" ? fixtureRoot : browserRoot;
  const path = join(root, relative);
  assert(path.startsWith(`${root}/`) || path === join(root, "index.html"), "invalid Browser asset path", {
    pathname,
    path,
  });
  const body = await readFile(path);
  const contentType = {
    ".css": "text/css",
    ".html": "text/html; charset=utf-8",
    ".js": "text/javascript",
    ".woff2": "font/woff2",
  }[extname(path)] || "application/octet-stream";
  response.writeHead(200, {
    "cache-control": "no-store",
    "content-length": body.length,
    "content-type": contentType,
  });
  response.end(body);
}

async function startServer(fixtureRoot) {
  const requestFailures = [];
  const server = createServer(async (request, response) => {
    try {
      const url = new URL(request.url ?? "/", "http://127.0.0.1");
      await serveFile(response, fixtureRoot, url.pathname);
    } catch (error) {
      requestFailures.push(error instanceof Error ? error.message : String(error));
      response.writeHead(500, { "content-type": "text/plain; charset=utf-8" });
      response.end("fixture error");
    }
  });
  await new Promise((resolvePromise) => server.listen(0, "127.0.0.1", resolvePromise));
  const address = server.address();
  assert(address && typeof address === "object", "Browser layout server did not bind");
  return {
    baseUrl: `http://127.0.0.1:${address.port}/`,
    requestFailures,
    async close() {
      await new Promise((resolvePromise, rejectPromise) => {
        server.close((error) => {
          if (error) {
            rejectPromise(error);
            return;
          }
          resolvePromise();
        });
      });
    },
  };
}

async function setScenario(page) {
  await page.click("#browser-settings");
  await page.evaluate(() => {
    const engine = document.querySelector("#browser-engine");
    const exit = document.querySelector("#browser-exit");
    const metrics = document.querySelector("#browser-metrics");
    const status = document.querySelector("#browser-status");
    const renderEmpty = document.querySelector("#browser-render-empty");
    if (!(engine instanceof HTMLSelectElement)) {
      throw new Error("missing Browser engine select");
    }
    if (!(exit instanceof HTMLSelectElement)) {
      throw new Error("missing Browser exit select");
    }
    if (!(metrics instanceof HTMLElement)) {
      throw new Error("missing Browser metrics");
    }
    if (!(status instanceof HTMLElement)) {
      throw new Error("missing Browser status");
    }
    if (!(renderEmpty instanceof HTMLElement)) {
      throw new Error("missing Browser empty render state");
    }
    if (engine.options.length === 1) {
      engine.add(new Option("Local Browser Engine", "engine/local"));
    }
    if (exit.options.length === 1) {
      exit.add(new Option("This device", "exit/local"));
    }
    renderEmpty.hidden = false;
    renderEmpty.innerHTML = "<strong>Browser stage ready</strong>";
    metrics.hidden = false;
    metrics.textContent = "1280×720 · WebRTC ready · pointer attached";
    status.dataset.visible = "true";
    status.textContent = "Connected to Browser Engine";
  });
  await page.evaluate(() => new Promise((resolvePromise) => requestAnimationFrame(() => requestAnimationFrame(resolvePromise))));
}

async function assertScenario(page, width, height, screenshotPath) {
  await page.setViewportSize({ width, height });
  await page.goto(page.url(), { waitUntil: "networkidle" });
  await setScenario(page);
  const result = await page.evaluate(() => {
    const measure = (selector) => {
      const element = document.querySelector(selector);
      if (!(element instanceof HTMLElement)) {
        throw new Error(`missing ${selector}`);
      }
      const rect = element.getBoundingClientRect();
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
      scrollWidth: document.scrollingElement?.scrollWidth ?? 0,
      toolbar: measure(".browser-chrome"),
      address: measure("#browser-url"),
      settings: measure("#browser-settings"),
      stage: measure("#browser-render-panel"),
      panel: measure("#browser-settings-panel"),
    };
  });
  assert(result.scrollWidth <= result.innerWidth, "Browser page scrolls sideways", result);
  assert(result.toolbar.left >= 0 && result.toolbar.right <= result.innerWidth + 0.5, "Browser toolbar escapes viewport", result);
  assert(result.address.right <= result.settings.left, "Browser address overlaps settings button", result);
  assert(result.stage.width > 0 && result.stage.height > 0, "Browser stage is not visible", result);
  assert(result.panel.left >= 0 && result.panel.right <= result.innerWidth + 0.5, "Browser settings panel escapes viewport width", result);
  assert(result.panel.top >= 0 && result.panel.bottom <= result.innerHeight + 0.5, "Browser settings panel escapes viewport height", result);
  await page.screenshot({ path: screenshotPath, fullPage: false });
}

async function main() {
  const fixtureRoot = await buildFixtureRoot();
  const server = await startServer(fixtureRoot);
  const pageErrors = [];
  const consoleErrors = [];
  const failedRequests = [];
  let browser;
  try {
    browser = await chromium.launch({
      headless: true,
      executablePath: brave,
    });
    const page = await browser.newPage({ viewport: { width: 1280, height: 900 } });
    page.on("pageerror", (error) => {
      pageErrors.push(error.message);
    });
    page.on("console", (message) => {
      if (message.type() === "error") {
        consoleErrors.push(message.text());
      }
    });
    page.on("requestfailed", (request) => {
      failedRequests.push(`${request.url()} ${request.failure()?.errorText ?? "request failed"}`);
    });
    await page.goto(server.baseUrl, { waitUntil: "networkidle" });
    const desktopScreenshot = "/tmp/browser-uiux-desktop-1280x900.png";
    const narrowScreenshot = "/tmp/browser-uiux-narrow-640x900.png";
    await assertScenario(page, 1280, 900, desktopScreenshot);
    await assertScenario(page, 640, 900, narrowScreenshot);
    assert(server.requestFailures.length === 0, "Browser layout fixture returned 500", server.requestFailures);
    assert(pageErrors.length === 0, "Browser layout page emitted page errors", pageErrors);
    assert(consoleErrors.length === 0, "Browser layout page emitted console errors", consoleErrors);
    assert(failedRequests.length === 0, "Browser layout page had failed requests", failedRequests);
    console.log(JSON.stringify({
      screenshots: [desktopScreenshot, narrowScreenshot],
    }, null, 2));
  } finally {
    await browser?.close();
    await server.close();
    await rm(fixtureRoot, { recursive: true, force: true });
  }
}

await main();
