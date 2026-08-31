#!/usr/bin/env node
import { createReadStream } from "node:fs";
import { stat } from "node:fs/promises";
import { createServer } from "node:http";
import { createRequire } from "node:module";
import path from "node:path";

const brave = process.env.BRAVE_BIN || "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser";
const require = createRequire(new URL("../elastos/tools/browser-playwright-engine/package.json", import.meta.url));
const { chromium } = require("playwright");

const capsuleRoot = path.resolve("capsules/library/browser");
const homeClipboardClientPath = path.resolve("capsules/home/browser/home-clipboard-client.js");
const homeClipboardProtocolPath = path.resolve("capsules/home/browser/home-clipboard-protocol.js");
const token = "library-product-layout-token";
const principalRoot = "localhost://Users/layout";
const documentsUri = `${principalRoot}/Documents`;

const roots = [
  { schema: "elastos.library.root/v1", id: "home", label: "Home", uri: principalRoot, kind: "principal-root" },
  { schema: "elastos.library.root/v1", id: "documents", label: "Documents", uri: documentsUri, kind: "directory" },
];

const objects = [
  object(`${documentsUri}/Alpha.txt`, "Alpha.txt", "file"),
  object(`${documentsUri}/Beta Folder`, "Beta Folder", "directory"),
  object(`${documentsUri}/Gamma.md`, "Gamma.md", "file"),
];

function assert(condition, message) {
  if (!condition) {
    throw new Error(message);
  }
}

function parseColor(color) {
  const match = /^rgba?\((\d+), (\d+), (\d+)(?:, ([0-9.]+))?\)$/.exec(color);
  if (!match) return null;
  return {
    r: Number.parseInt(match[1], 10),
    g: Number.parseInt(match[2], 10),
    b: Number.parseInt(match[3], 10),
    a: match[4] == null ? 1 : Number.parseFloat(match[4]),
  };
}

function flattenColor(foreground, background) {
  const fg = parseColor(foreground);
  const bg = parseColor(background);
  if (!fg) return null;
  if (!bg || fg.a >= 1) {
    return [fg.r, fg.g, fg.b];
  }
  return [
    Math.round((fg.a * fg.r) + ((1 - fg.a) * bg.r)),
    Math.round((fg.a * fg.g) + ((1 - fg.a) * bg.g)),
    Math.round((fg.a * fg.b) + ((1 - fg.a) * bg.b)),
  ];
}

function colorDistance(left, right, base = null) {
  const a = flattenColor(left, base || left);
  const b = flattenColor(right, base || right);
  if (!a || !b) return 0;
  return Math.sqrt(
    ((a[0] - b[0]) ** 2)
    + ((a[1] - b[1]) ** 2)
    + ((a[2] - b[2]) ** 2),
  );
}

function object(uri, name, kind) {
  return {
    schema: "elastos.library.object/v1",
    uri,
    name,
    kind,
    mime: kind === "directory" ? "inode/directory" : "text/plain",
    size: kind === "directory" ? 0 : 128,
    created_at: 1_780_000_000,
    modified_at: 1_780_000_000,
    revision: `rev:${name}`,
    viewer: null,
    viewers: [],
    availability: "local-only",
    published: false,
    shared: false,
    capabilities: kind === "directory"
      ? ["open", "rename", "move", "copy", "trash", "properties"]
      : ["open", "download", "rename", "move", "copy", "publish", "trash", "properties"],
  };
}

function contentType(filePath) {
  if (filePath.endsWith(".html")) return "text/html; charset=utf-8";
  if (filePath.endsWith(".css")) return "text/css; charset=utf-8";
  if (filePath.endsWith(".js")) return "text/javascript; charset=utf-8";
  if (filePath.endsWith(".svg")) return "image/svg+xml";
  if (filePath.endsWith(".woff2")) return "font/woff2";
  return "application/octet-stream";
}

function createAppServer() {
  return createServer(async (req, res) => {
    const url = new URL(req.url || "/", "http://127.0.0.1");
    if (url.pathname === "/api/provider/object/events/stream") {
      res.writeHead(200, {
        "content-type": "text/event-stream",
        "cache-control": "no-cache",
        connection: "keep-alive",
      });
      res.write('event: library-events\ndata: {"schema":"elastos.library.events/v1","events":[]}\n\n');
      return;
    }
    const providerMatch = url.pathname.match(/^\/api\/provider\/object\/([^/]+)$/);
    if (providerMatch) {
      const op = decodeURIComponent(providerMatch[1]);
      let body = "";
      req.on("data", (chunk) => {
        body += chunk;
      });
      req.on("end", () => {
        const payload = body ? JSON.parse(body) : {};
        let data = {};
        if (op === "roots") {
          data = { roots };
        } else if (op === "list") {
          data = {
            uri: payload.uri || documentsUri,
            objects,
            object: {
              schema: "elastos.library.object/v1",
              uri: payload.uri || documentsUri,
              name: "Documents",
              kind: "directory",
              mime: "inode/directory",
              size: 0,
              revision: "rev:documents",
              metadata: { readonly: false },
              capabilities: ["open", "properties"],
            },
          };
        }
        res.writeHead(200, { "content-type": "application/json" });
        res.end(JSON.stringify({ status: "ok", data }));
      });
      return;
    }

    if (url.pathname === "/apps/home/home-clipboard-client.js") {
      res.writeHead(200, { "content-type": "text/javascript; charset=utf-8" });
      createReadStream(homeClipboardClientPath).pipe(res);
      return;
    }
    if (url.pathname === "/apps/home/home-clipboard-protocol.js") {
      res.writeHead(200, { "content-type": "text/javascript; charset=utf-8" });
      createReadStream(homeClipboardProtocolPath).pipe(res);
      return;
    }

    let relative = url.pathname.replace(/^\/apps\/library\/?/, "") || "index.html";
    relative = relative.replace(/^\/+/, "");
    const filePath = path.join(capsuleRoot, relative);
    if (!filePath.startsWith(capsuleRoot)) {
      res.writeHead(403);
      res.end("Forbidden");
      return;
    }
    try {
      const info = await stat(filePath);
      if (!info.isFile()) {
        throw new Error("not a file");
      }
      res.writeHead(200, { "content-type": contentType(filePath) });
      createReadStream(filePath).pipe(res);
    } catch {
      res.writeHead(404);
      res.end("Not found");
    }
  });
}

async function assertNoHorizontalOverflow(page, label) {
  const overflow = await page.evaluate(() => ({
    body: document.documentElement.scrollWidth - document.documentElement.clientWidth,
    shell: (() => {
      const shell = document.getElementById("library-shell");
      return shell ? shell.scrollWidth - shell.clientWidth : 0;
    })(),
    main: (() => {
      const main = document.querySelector(".main");
      return main ? main.scrollWidth - main.clientWidth : 0;
    })(),
  }));
  assert(overflow.body <= 1, `${label}: document overflowed horizontally (${overflow.body}px)`);
  assert(overflow.shell <= 1, `${label}: shell overflowed horizontally (${overflow.shell}px)`);
  assert(overflow.main <= 1, `${label}: main overflowed horizontally (${overflow.main}px)`);
}

async function openProperties(page, name) {
  await page.locator(".item").filter({ hasText: name }).first().click({ button: "right" });
  await page.locator("#context-menu:not(.hidden)").waitFor();
  await page.locator("#context-menu .menu-item").filter({ hasText: "Properties" }).first().click();
  await page.locator(".window-item-properties").filter({ hasText: `${name} properties` }).first().waitFor();
}

async function readThemeSnapshot(page) {
  return page.evaluate(() => {
    const read = (selector) => {
      const element = document.querySelector(selector);
      if (!element) {
        return { backgroundColor: "", color: "" };
      }
      const style = getComputedStyle(element);
      return {
        backgroundColor: style.backgroundColor,
        color: style.color,
      };
    };
    return {
      activeTab: document.querySelector(".item-props-tab-selected")?.textContent?.trim() || "",
      body: read("body"),
      statusbar: read(".statusbar"),
      card: read(".window-item-properties"),
      title: read(".properties-window-title"),
      panel: read(".item-props-tab-content-selected"),
      label: read(".item-prop-label"),
      value: read(".item-prop-val"),
      copyButton: read(".props-copy-btn"),
    };
  });
}

function assertReadableTheme(snapshot, label) {
  assert(snapshot.activeTab.length > 0, `${label}: Properties tab did not stay selected.`);
  assert(
    colorDistance(snapshot.statusbar.backgroundColor, snapshot.statusbar.color, snapshot.body.backgroundColor) > 40,
    `${label}: footer text lost readable contrast.`,
  );
  assert(colorDistance(snapshot.title.backgroundColor, snapshot.title.color) > 40, `${label}: Properties title lost readable contrast.`);
  assert(
    colorDistance(snapshot.panel.backgroundColor, snapshot.panel.color, snapshot.card.backgroundColor) > 40,
    `${label}: Properties panel lost readable contrast.`,
  );
  assert(
    colorDistance(snapshot.panel.backgroundColor, snapshot.label.color, snapshot.card.backgroundColor) > 40,
    `${label}: Properties labels lost readable contrast.`,
  );
  assert(
    colorDistance(snapshot.panel.backgroundColor, snapshot.value.color, snapshot.card.backgroundColor) > 40,
    `${label}: Properties values lost readable contrast.`,
  );
}

async function run() {
  const server = createAppServer();
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  const { port } = server.address();
  const browser = await chromium.launch({ headless: true, executablePath: brave });
  try {
    const page = await browser.newPage({ viewport: { width: 1280, height: 900 } });
    const consoleErrors = [];
    const pageErrors = [];
    const failedRequests = [];
    const requestLog = [];
    page.on("console", (message) => {
      if (message.type() === "error") {
        consoleErrors.push(message.text());
      }
    });
    page.on("pageerror", (error) => {
      pageErrors.push(error?.stack || String(error));
    });
    page.on("requestfailed", (request) => {
      failedRequests.push({
        url: request.url(),
        method: request.method(),
        failure: request.failure()?.errorText || "unknown",
      });
    });
    page.on("requestfinished", async (request) => {
      if (!request.url().startsWith(`http://127.0.0.1:${port}/`)) return;
      if (
        !request.url().includes("/api/provider/object/")
        && !request.url().includes("/apps/library/")
        && !request.url().includes("/apps/home/")
      ) {
        return;
      }
      try {
        const response = await request.response();
        requestLog.push({
          url: request.url(),
          method: request.method(),
          status: response?.status() || 0,
        });
      } catch {
        requestLog.push({
          url: request.url(),
          method: request.method(),
          status: -1,
        });
      }
    });
    await page.goto(`http://127.0.0.1:${port}/apps/library/?home_origin=${encodeURIComponent("https://home.example")}#home_token=${encodeURIComponent(token)}`);
    try {
      await page.locator(".item").filter({ hasText: "Alpha.txt" }).first().waitFor();
    } catch (error) {
      const rendered = await page.evaluate(() => ({
        contentText: document.getElementById("content")?.textContent || "",
        contentHtml: document.getElementById("content")?.innerHTML || "",
        statusText: document.getElementById("status-text")?.textContent || "",
        shellHidden: document.getElementById("library-shell")?.classList.contains("hidden") || false,
        lockedHidden: document.getElementById("locked-shell")?.classList.contains("hidden") || false,
      }));
      throw new Error([
        error?.stack || String(error),
        `console errors: ${JSON.stringify(consoleErrors)}`,
        `page errors: ${JSON.stringify(pageErrors)}`,
        `failed requests: ${JSON.stringify(failedRequests)}`,
        `request log: ${JSON.stringify(requestLog)}`,
        `rendered content: ${JSON.stringify(rendered)}`,
      ].join("\n"));
    }
    const initialChrome = await page.evaluate(() => ({
      tokenLink: !!document.querySelector('link[href="./elastos-ui.css"]'),
      themeScript: !!document.querySelector('script[src="./elastos-theme.js"]'),
      accent: getComputedStyle(document.documentElement).getPropertyValue("--el-accent").trim(),
      contentView: document.getElementById("content")?.dataset.view || "",
      searchIcon: document.querySelector('#search-toggle-button img')?.getAttribute("src") || "",
      searchText: document.getElementById("search-toggle-button")?.textContent?.trim() || "",
      moreIcon: document.querySelector('#more-button img')?.getAttribute("src") || "",
      moreText: document.getElementById("more-button")?.textContent?.trim() || "",
      refreshHidden: document.getElementById("refresh-button")?.classList.contains("hidden") || false,
      sortHidden: document.getElementById("sort-select")?.classList.contains("hidden") || false,
      searchExpanded: document.getElementById("search-toggle-button")?.getAttribute("aria-expanded") || "",
      resizeNow: document.getElementById("sidebar-resizer")?.getAttribute("aria-valuenow") || "",
      placeMask: document.querySelector('.place .place-icon-accent')?.style.getPropertyValue("--place-mask") || "",
      placeHasImg: !!document.querySelector('.place .place-icon-accent img'),
    }));
    assert(initialChrome.tokenLink, "Library must load the vendored shared token sheet.");
    assert(initialChrome.themeScript, "Library must load the vendored shared theme runtime.");
    assert(initialChrome.accent.length > 0, "Library token design must expose shared accent variables.");
    assert(initialChrome.contentView === "list", "Library must open in list view by default.");
    assert(initialChrome.searchIcon === "icons/search.svg", "Library Search must render the capsule-owned search icon.");
    assert(initialChrome.searchText === "", "Library Search must stay an icon button.");
    assert(initialChrome.moreIcon === "icons/more.svg", "Library More must render the capsule-owned more icon.");
    assert(initialChrome.moreText === "", "Library More must stay an icon button.");
    assert(initialChrome.refreshHidden, "Library toolbar Refresh must stay hidden.");
    assert(initialChrome.sortHidden, "Library toolbar Sort must stay hidden.");
    assert(initialChrome.searchExpanded === "false", "Library Search must start collapsed.");
    assert(initialChrome.resizeNow === "220", "Library sidebar resizer must expose the live width value.");
    assert(initialChrome.placeMask.includes("sidebar-folder"), "Library places must render accent-masked sidebar icons.");
    assert(!initialChrome.placeHasImg, "Library accent-masked place icons must not fall back to nested image tags.");

    await assertNoHorizontalOverflow(page, "wide");
    await page.locator("#search-toggle-button").click();
    await page.locator("#toolbar-search.open").waitFor();
    const searchFocused = await page.evaluate(() => ({
      activeId: document.activeElement?.id || "",
      expanded: document.getElementById("search-toggle-button")?.getAttribute("aria-expanded") || "",
    }));
    assert(searchFocused.activeId === "search", "Search toggle must focus the search field.");
    assert(searchFocused.expanded === "true", "Search toggle must expose the open state.");
    await assertNoHorizontalOverflow(page, "wide search-open");

    await page.locator("#more-button").click();
    await page.locator("#context-menu:not(.hidden)").waitFor();
    await assertNoHorizontalOverflow(page, "wide menu-open");

    const beforeResize = await page.evaluate(() => getComputedStyle(document.querySelector(".shell")).getPropertyValue("--library-sidebar-width").trim());
    await page.locator("#sidebar-resizer").dispatchEvent("pointerdown", { clientX: 220 });
    await page.evaluate(() => {
      window.dispatchEvent(new PointerEvent("pointermove", { clientX: 280 }));
      window.dispatchEvent(new PointerEvent("pointerup", { clientX: 280 }));
    });
    const afterResize = await page.evaluate(() => ({
      width: getComputedStyle(document.querySelector(".shell")).getPropertyValue("--library-sidebar-width").trim(),
      aria: document.getElementById("sidebar-resizer")?.getAttribute("aria-valuenow") || "",
    }));
    assert(beforeResize !== afterResize.width, "Sidebar resize must update the shell width variable.");
    assert(afterResize.aria === "280", "Sidebar resize must update aria-valuenow with the live width.");

    await page.locator('.content[data-view="list"] .item').first().waitFor();
    await assertNoHorizontalOverflow(page, "wide list");

    await openProperties(page, "Alpha.txt");
    const darkGeneral = await readThemeSnapshot(page);
    assert(darkGeneral.activeTab === "General", "Properties must open on the General tab.");
    assertReadableTheme(darkGeneral, "dark general");
    await page.locator(".item-props-tab-btn").filter({ hasText: "Technical" }).first().click();
    await page.locator('.item-props-tab-content-selected[data-tab="technical"]').waitFor();
    const darkTechnical = await readThemeSnapshot(page);
    assert(darkTechnical.activeTab === "Technical", "Properties must switch to the Technical tab.");
    assertReadableTheme(darkTechnical, "dark technical");
    await page.evaluate(() => {
      document.documentElement.setAttribute("data-el-theme", "light");
    });
    const lightTechnical = await readThemeSnapshot(page);
    assertReadableTheme(lightTechnical, "light technical");
    await page.locator(".item-props-tab-btn").filter({ hasText: "General" }).first().click();
    await page.locator('.item-props-tab-content-selected[data-tab="general"]').waitFor();
    const lightGeneral = await readThemeSnapshot(page);
    assert(lightGeneral.activeTab === "General", "Properties must return to the General tab.");
    assertReadableTheme(lightGeneral, "light general");
    assert(darkGeneral.statusbar.backgroundColor !== lightGeneral.statusbar.backgroundColor, "Library footer must react to theme changes.");
    assert(darkGeneral.panel.backgroundColor !== lightGeneral.panel.backgroundColor, "Library Properties panel must react to theme changes.");
    await page.locator(".properties-window-actions [data-dialog-close]").click();

    await page.fill("#search", "missing");
    await page.locator(".empty").waitFor();
    await assertNoHorizontalOverflow(page, "wide empty");

    await page.setViewportSize({ width: 560, height: 900 });
    await page.fill("#search", "");
    await page.locator(".item").filter({ hasText: "Alpha.txt" }).first().waitFor();
    await page.locator("#list-button").click();
    await assertNoHorizontalOverflow(page, "narrow list");

    const narrowState = await page.evaluate(() => ({
      searchWidth: getComputedStyle(document.getElementById("search")).width,
      resizerHidden: getComputedStyle(document.getElementById("sidebar-resizer")).display === "none",
    }));
    assert(narrowState.resizerHidden, "Sidebar resizer must hide on narrow layouts.");
    assert(Number.parseFloat(narrowState.searchWidth) > 0, "Search must keep a bounded width on narrow layouts.");

    console.log("PASS Library product layout smoke");
  } finally {
    await browser.close();
    await new Promise((resolve) => server.close(resolve));
  }
}

run().catch((error) => {
  console.error(error?.stack || String(error));
  process.exit(1);
});
