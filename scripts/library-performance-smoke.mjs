#!/usr/bin/env node
import { createReadStream } from "node:fs";
import { stat } from "node:fs/promises";
import { createServer } from "node:http";
import { createRequire } from "node:module";
import path from "node:path";
import { pathToFileURL } from "node:url";

const playwrightModule = process.env.ELASTOS_PLAYWRIGHT_MODULE
  ? await import(pathToFileURL(process.env.ELASTOS_PLAYWRIGHT_MODULE).href)
  : createRequire(new URL("../elastos/tools/browser-playwright-engine/package.json", import.meta.url))("playwright");
const { chromium } = playwrightModule.chromium ? playwrightModule : playwrightModule.default;

const capsuleRoot = path.resolve("capsules/library/browser");
const homeClipboardClientPath = path.resolve(
  "capsules/home/browser/home-clipboard-client.js",
);
const homeClipboardProtocolPath = path.resolve(
  "capsules/home/browser/home-clipboard-protocol.js",
);
const homeNavigationClientPath = path.resolve("capsules/home/browser/home-navigation-client.js");
const token = "library-performance-smoke-token";
const principalRoot = "localhost://Users/perf";
const documentsUri = `${principalRoot}/Documents`;
const desktopUri = `${principalRoot}/Desktop`;

function assert(condition, message) {
  if (!condition) throw new Error(message);
}

function object(uri, name, kind = "file", index = 0) {
  return {
    schema: "elastos.library.object/v1",
    uri,
    name,
    kind,
    mime: kind === "directory" ? "inode/directory" : "text/plain",
    size: kind === "directory" ? 0 : 128 + index,
    created_at: 1_780_000_000 + index,
    modified_at: 1_780_000_000 + index,
    revision: `rev:${index}`,
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

const roots = [
  { schema: "elastos.library.root/v1", id: "home", label: "Home", uri: principalRoot, kind: "principal-root" },
  { schema: "elastos.library.root/v1", id: "desktop", label: "Desktop", uri: desktopUri, kind: "directory" },
  { schema: "elastos.library.root/v1", id: "documents", label: "Documents", uri: documentsUri, kind: "directory" },
  { schema: "elastos.library.root/v1", id: "webspaces", label: "Spaces", uri: "localhost://WebSpaces", kind: "webspace-root" },
];

const objectCount = 5_000;
const documents = Array.from({ length: objectCount }, (_, index) => {
  const number = String(index + 1).padStart(4, "0");
  const kind = index % 25 === 24 ? "directory" : "file";
  const name = kind === "directory" ? `Folder-${number}` : `File-${number}.txt`;
  return object(`${documentsUri}/${name}`, name, kind, index);
});
// A repeatable permutation exercises comparison work instead of an already sorted folder.
const listedDocuments = documents.map((_, index) => documents[(index * 7919) % objectCount]);
const writePayloads = [];
const pendingUploads = new Map();
const uploadWaiters = new Map();
let holdUploads = false;

function contentType(filePath) {
  if (filePath.endsWith(".html")) return "text/html; charset=utf-8";
  if (filePath.endsWith(".css")) return "text/css; charset=utf-8";
  if (filePath.endsWith(".js")) return "text/javascript; charset=utf-8";
  if (filePath.endsWith(".svg")) return "image/svg+xml";
  return "application/octet-stream";
}

function createAppServer() {
  return createServer(async (req, res) => {
    const url = new URL(req.url || "/", "http://127.0.0.1");
    if (url.pathname === "/favicon.ico") {
      res.writeHead(204);
      res.end();
      return;
    }
    if (url.pathname === "/api/provider/object/events/stream") {
      res.writeHead(200, {
        "content-type": "text/event-stream",
        "cache-control": "no-cache",
        connection: "keep-alive",
      });
      res.write('event: library-events\ndata: {"schema":"elastos.library.events/v1","events":[]}\n\n');
      return;
    }
    if (url.pathname === "/api/provider/object/upload") {
      const chunks = [];
      req.on("data", (chunk) => { chunks.push(chunk); });
      req.on("end", () => {
        const uri = url.searchParams.get("uri") || "";
        const name = path.basename(uri || "upload.txt");
        const payload = {
          uri,
          body: Buffer.concat(chunks).toString("utf8"),
          contentType: req.headers["content-type"] || "",
        };
        writePayloads.push(payload);
        const finish = () => {
          res.writeHead(200, { "content-type": "application/json" });
          res.end(JSON.stringify({
            status: "ok",
            data: {
              transport: "raw-body",
              object: object(uri, name, "file", writePayloads.length),
            },
          }));
        };
        if (holdUploads) {
          pendingUploads.set(name, finish);
          uploadWaiters.get(name)?.();
        } else {
          finish();
        }
      });
      return;
    }
    const providerMatch = url.pathname.match(/^\/api\/provider\/object\/([^/]+)$/);
    if (providerMatch) {
      const op = decodeURIComponent(providerMatch[1]);
      let body = "";
      req.on("data", (chunk) => { body += chunk; });
      req.on("end", () => {
        const payload = body ? JSON.parse(body) : {};
        let data;
        if (op === "roots") {
          data = { roots };
        } else if (op === "list") {
          const uri = payload.uri || documentsUri;
          data = {
            uri,
            objects: uri === documentsUri ? listedDocuments : [],
          };
        } else if (op === "write") {
          writePayloads.push(payload);
          data = {
            object: object(payload.uri, path.basename(payload.uri || "upload.txt"), "file", writePayloads.length),
          };
        } else {
          data = {};
        }
        res.writeHead(200, { "content-type": "application/json" });
        res.end(JSON.stringify({ status: "ok", data }));
      });
      return;
    }

    if (url.pathname === "/apps/home/home-navigation-client.js") {
      res.writeHead(200, { "content-type": "text/javascript; charset=utf-8" });
      createReadStream(homeNavigationClientPath).pipe(res);
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
      if (!info.isFile()) throw new Error("not a file");
      res.writeHead(200, { "content-type": contentType(filePath) });
      createReadStream(filePath).pipe(res);
    } catch {
      res.writeHead(404);
      res.end("Not found");
    }
  });
}

async function sortFolder(page, label) {
  await page.keyboard.press("Escape");
  await page.locator("#more-button").click();
  const parent = page.locator("#context-menu > .menu-entry > .menu-item").filter({ hasText: "Sort By" });
  await parent.click();
  const button = parent.locator("xpath=..").locator(".menu-submenu .menu-item").filter({ hasText: label });
  await button.evaluate((element) => {
    element.addEventListener("click", () => {
      const startedAt = performance.now();
      const renderCount = window.__libraryPerf.contentRenderCount;
      const probe = window.__librarySortProbe = { partialFrames: 0, firstPaintMs: null, completeMs: null };
      const observe = () => {
        const perf = window.__libraryPerf;
        if (perf.contentRenderCount > renderCount) {
          probe.firstPaintMs ??= performance.now() - startedAt;
          if (perf.lastContentRender.complete) {
            probe.completeMs = performance.now() - startedAt;
            return;
          }
          probe.partialFrames += 1;
        }
        requestAnimationFrame(observe);
      };
      requestAnimationFrame(observe);
    }, { once: true, capture: true });
  });
  await button.click();
  await page.waitForFunction(() => window.__librarySortProbe?.completeMs !== null);
  const result = await page.evaluate(() => ({ ...window.__librarySortProbe, ...window.__libraryPerf.lastContentRender }));
  assert(result.objectCount === objectCount && await page.locator(".item").count() === objectCount,
    `${label} sort must retain every object`);
  assert(result.partialFrames > 0, `${label} sort must yield frames while the remaining rows render`);
  assert(result.firstPaintMs < 750, `${label} sort first paint took ${result.firstPaintMs}ms`);
  assert(result.completeMs < 5_000, `${label} sort completion took ${result.completeMs}ms`);
  return { sort: label, ...result };
}

async function assertNameOrder(page, descending) {
  const rows = await page.locator(".item").evaluateAll((items) => items.map((item) => ({
    name: item.querySelector(".item-name").textContent,
    kind: item.dataset.kind,
  })));
  const expected = ["directory", "file"].flatMap((kind) => {
    const names = documents.filter((entry) => entry.kind === kind).map((entry) => entry.name).sort();
    return descending ? names.reverse() : names;
  });
  assert(rows.every((row, index) => row.name === expected[index]),
    `Name sort must keep directories first and names ${descending ? "descending" : "ascending"}`);
}

async function waitForUpload(name) {
  if (pendingUploads.has(name)) return;
  let timer;
  try {
    await new Promise((resolve, reject) => {
      uploadWaiters.set(name, resolve);
      timer = setTimeout(() => reject(new Error(`Upload fixture did not receive ${name}`)), 5_000);
    });
  } finally {
    clearTimeout(timer);
    uploadWaiters.delete(name);
  }
}

function finishUpload(name) {
  const finish = pendingUploads.get(name);
  assert(finish, `Upload ${name} must be pending before it can complete`);
  pendingUploads.delete(name);
  finish();
}

async function assertFolderLoadRecovery(page) {
  const firstFolder = documents.find((entry) => entry.kind === "directory");
  const secondFolder = documents.find((entry) => entry.kind === "directory" && entry !== firstFolder);
  const held = new Map();
  const waiters = new Map();
  const controlledUris = new Set([firstFolder.uri, secondFolder.uri]);
  await page.route("**/api/provider/object/list", (route) => {
    const uri = route.request().postDataJSON()?.uri;
    if (!controlledUris.has(uri)) return route.continue();
    held.set(uri, route);
    waiters.get(uri)?.();
  });
  async function requestFor(uri) {
    if (!held.has(uri)) {
      let timer;
      try {
        await new Promise((resolve, reject) => {
          waiters.set(uri, resolve);
          timer = setTimeout(() => reject(new Error(`Folder request did not arrive: ${uri}`)), 5_000);
        });
      } finally {
        clearTimeout(timer);
        waiters.delete(uri);
      }
    }
    const route = held.get(uri);
    held.delete(uri);
    return route;
  }
  const folderRow = (entry) => page.locator(`.item[data-uri="${entry.uri}"]`);
  const respond = (route, envelope) => route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(envelope) });

  await page.locator(".place").filter({ hasText: "Documents" }).first().click();
  await page.waitForFunction((count) => window.__libraryPerf.lastContentRender?.complete
    && document.querySelectorAll(".item").length === count, objectCount);
  await page.locator(".item").filter({ hasText: "File-0001.txt" }).first().click();
  await folderRow(firstFolder).dblclick();
  const failing = await requestFor(firstFolder.uri);
  assert(await page.locator(".item").count() === 0, "A pending folder must clear rows from the previous folder");
  await page.locator(".empty h2").filter({ hasText: "Loading folder..." }).waitFor();
  assert(await page.locator("#footer-left").textContent() === "0 items", "Pending navigation must clear previous selection and count");
  await respond(failing, { status: "error", message: "This folder is temporarily unavailable." });
  await page.locator(".empty h2").filter({ hasText: "This folder could not be opened" }).waitFor();
  assert(await page.locator(".item").count() === 0, "A failed folder must keep previous rows out of its view");
  assert((await page.locator(".empty").textContent()).includes("Choose Refresh from More"),
    "A failed folder must explain how to retry");

  await page.locator("#more-button").click();
  await page.locator("#context-menu .menu-item").filter({ hasText: /^Refresh$/ }).click();
  const retry = await requestFor(firstFolder.uri);
  await page.locator(".empty h2").filter({ hasText: "Loading folder..." }).waitFor();
  const recovered = object(`${firstFolder.uri}/Recovered.txt`, "Recovered.txt");
  await respond(retry, { status: "ok", data: { object: firstFolder, objects: [recovered] } });
  await page.locator(".item").filter({ hasText: "Recovered.txt" }).waitFor();
  assert(await page.locator(".item").count() === 1, "Refresh must recover the requested folder's items");

  await page.locator("#back-button").click();
  await folderRow(secondFolder).waitFor();
  await folderRow(secondFolder).dblclick();
  const obsolete = await requestFor(secondFolder.uri);
  await page.locator(".place").filter({ hasText: "Desktop" }).first().click();
  await page.locator(".crumb-current").filter({ hasText: "Desktop" }).waitFor();
  await respond(obsolete, { status: "error", message: "The old folder is unavailable." });
  await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))));
  assert(await page.locator("#status-text").textContent() === "", "An obsolete failure must leave the current folder's status unchanged");
  assert(await page.locator(".crumb-current").textContent() === "Desktop", "An obsolete response must preserve the current folder");
  assert(await page.locator(".item").count() === 0, "An obsolete response must preserve the current folder's rows");
}

async function run() {
  const server = createAppServer();
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  const { port } = server.address();
  let browser;
  try {
    browser = await chromium.launch({ headless: true, executablePath: process.env.BRAVE_BIN || undefined });
    const page = await browser.newPage();
    const pageErrors = [];
    const consoleErrors = [];
    const failedRequests = [];
    page.on("pageerror", (error) => {
      pageErrors.push(String(error?.stack || error));
    });
    page.on("console", (message) => {
      if (message.type() === "error") {
        consoleErrors.push({ message: message.text(), location: message.location() });
      }
    });
    page.on("requestfailed", (request) => {
      failedRequests.push({
        url: request.url(),
        error: request.failure()?.errorText || "",
      });
    });
    page.on("response", (response) => {
      if (response.status() >= 400) {
        failedRequests.push({
          url: response.url(),
          error: `HTTP ${response.status()}`,
        });
      }
    });
    await page.addInitScript(() => {
      const observer = new MutationObserver(() => {
        const content = document.getElementById("content");
        const first = content?.querySelector(".item");
        if (!first || content.getAttribute("aria-busy") !== "true") return;
        observer.disconnect();
        window.__libraryEarlyEnd = { initialCount: content.querySelectorAll(".item").length };
        first.focus();
        first.dispatchEvent(new KeyboardEvent("keydown", { key: "End", bubbles: true, cancelable: true }));
      });
      observer.observe(document, { childList: true, subtree: true });
    });
    await page.goto(
      `http://127.0.0.1:${port}/apps/library/#home_token=${encodeURIComponent(token)}`,
    );
    try {
      await page.locator(".item").filter({ hasText: "File-0001.txt" }).first().waitFor();
    } catch (error) {
      throw new Error(
        `Library performance fixture did not boot: ${String(error)}\n${JSON.stringify(
          { pageErrors, consoleErrors, failedRequests },
          null,
          2,
        )}`,
      );
    }

    const initialPerf = await page.evaluate(() => window.__libraryPerf);
    assert(initialPerf?.iconFetchCount === 0, "Library must not fetch/hydrate SVG icons from JavaScript");
    assert(initialPerf?.renderPlacesCount === 1, "Library sidebar should render once on boot");
    assert(initialPerf?.lastContentRender?.objectCount === objectCount, "Library must render the full large folder");
    assert(initialPerf?.lastContentRender?.chunked === true, "Large folders should render in chunks for fast first paint");
    assert(initialPerf?.lastContentRender?.initialRenderedCount < objectCount, "Initial large-folder paint should not build every row synchronously");
    assert(initialPerf?.lastContentRender?.durationMs < 750, `Large folder render too slow: ${initialPerf?.lastContentRender?.durationMs}ms`);
    assert(await page.locator("[data-icon-src], [data-icon-ready]").count() === 0, "Library DOM must not expose async icon hydration markers");
    await page.waitForFunction(() => window.__libraryPerf?.lastContentRender?.complete === true);
    assert(await page.locator(".item").count() === objectCount, "Every object must finish rendering");
    const expectedLastUri = documents.filter((entry) => entry.kind === "file")
      .sort((left, right) => left.name.localeCompare(right.name)).at(-1).uri;
    await page.waitForFunction((uri) => document.activeElement?.dataset.uri === uri, expectedLastUri);
    const earlyEnd = await page.evaluate(() => ({
      ...window.__libraryEarlyEnd,
      focusedUri: document.activeElement?.dataset.uri,
      lastUri: document.querySelector("#content .item:last-child")?.dataset.uri,
      selected: Array.from(document.querySelectorAll('#content .item[aria-selected="true"]'), (item) => item.dataset.uri),
      tabStops: Array.from(document.querySelectorAll('#content .item[tabindex="0"]'), (item) => item.dataset.uri),
    }));
    assert(earlyEnd.initialCount > 0 && earlyEnd.initialCount < objectCount,
      "End must be pressed while only part of the folder has rendered");
    assert(earlyEnd.focusedUri === expectedLastUri && earlyEnd.lastUri === expectedLastUri,
      "End during rendering must focus the last object in the complete folder");
    assert(earlyEnd.selected.length === 1 && earlyEnd.selected[0] === expectedLastUri,
      "End must select the actual last object");
    assert(earlyEnd.tabStops.length === 1 && earlyEnd.tabStops[0] === expectedLastUri,
      "The last object must become the folder's single tab stop");

    await page.locator("#content .item").nth(1_000).evaluate((item) => item.focus());
    const refreshEnd = await page.evaluate(() => new Promise((resolve) => {
      document.querySelector("#sort-select").dispatchEvent(new Event("change", { bubbles: true }));
      requestAnimationFrame(() => {
        const probe = {
          busy: document.getElementById("content").getAttribute("aria-busy"),
          focus: document.activeElement.id,
          renderedCount: document.querySelectorAll("#content .item").length,
        };
        document.activeElement.dispatchEvent(new KeyboardEvent("keydown", { key: "End", bubbles: true, cancelable: true }));
        resolve(probe);
      });
    }));
    assert(refreshEnd.busy === "true" && refreshEnd.focus === "content" && refreshEnd.renderedCount < objectCount,
      "Refresh End must start while the previously focused row is outside the rendered prefix");
    await page.waitForFunction((uri) => window.__libraryPerf.lastContentRender.complete
      && document.activeElement?.dataset.uri === uri, expectedLastUri);
    assert(await page.locator("#content .item").count() === objectCount,
      "End during refresh must retain the complete folder");
    assert(await page.locator('#content .item[aria-selected="true"]').count() === 1
      && await page.locator('#content .item[aria-selected="true"]').getAttribute("data-uri") === expectedLastUri,
    "End during refresh must select the last object after focus restoration");
    assert(await page.locator('#content .item[tabindex="0"]').count() === 1
      && await page.locator('#content .item[tabindex="0"]').getAttribute("data-uri") === expectedLastUri,
    "End during refresh must retain one tab stop on the last object");

    await page.waitForFunction(() => (window.__libraryPerf?.folderCacheSize || 0) >= 3);
    await page.locator(".place").filter({ hasText: "Desktop" }).first().click();
    await page.locator(".crumb-current").filter({ hasText: "Desktop" }).first().waitFor();
    await page.locator(".place").filter({ hasText: "Documents" }).first().click();
    await page.waitForFunction((count) => window.__libraryPerf?.lastContentRender?.objectCount === count
      && window.__libraryPerf.lastContentRender.complete === true, objectCount);
    assert((await page.locator('.place:focus').textContent())?.trim() === "Documents",
      "Completed End navigation must preserve later sidebar focus while a folder renders");
    const afterNavigation = await page.evaluate(() => window.__libraryPerf);
    assert(afterNavigation.renderPlacesCount === 1, "Folder navigation must not rebuild the sidebar");
    assert(afterNavigation.folderCacheHits >= 2, "Root navigation should render from prefetched folder cache");
    assert(afterNavigation.objectNodeCacheHits >= 1000, "Returning to a large folder should reuse existing item DOM nodes");

    const sorts = [await sortFolder(page, "Descending")];
    await assertNameOrder(page, true);
    sorts.push(await sortFolder(page, "Size"));
    const fileSizes = await page.locator('.item[data-kind="file"]').evaluateAll((items) =>
      items.map((item) => Number(item.dataset.uri.match(/File-(\d+)/)[1])));
    assert(fileSizes.every((size, index) => index === 0 || fileSizes[index - 1] > size),
      "Descending size sort must order every file by size");
    sorts.push(await sortFolder(page, "Name"));
    sorts.push(await sortFolder(page, "Ascending"));
    await assertNameOrder(page, false);

    await page.locator("#search-toggle-button").click();
    await page.locator("#search").fill("File-04");
    const matchingCount = documents.filter((entry) => entry.name.includes("File-04")).length;
    await page.waitForFunction((count) => window.__libraryPerf?.lastContentRender?.objectCount === count,
      matchingCount);
    assert(await page.locator(".item").count() === matchingCount, "Search must show every matching object");
    await page.locator("#search").fill("");
    await page.waitForFunction((count) => window.__libraryPerf?.lastContentRender?.objectCount === count
      && window.__libraryPerf.lastContentRender.complete === true, objectCount);

    await page.locator(".item").filter({ hasText: "File-0001.txt" }).first().click({ button: "right" });
    await page.locator("#context-menu button").filter({ hasText: "Open" }).first().waitFor();
    const afterMenu = await page.evaluate(() => window.__libraryPerf);
    assert(afterMenu?.lastMenuRender?.durationMs < 100, `Context menu render too slow: ${afterMenu?.lastMenuRender?.durationMs}ms`);

    const beforeUpload = await page.evaluate(() => ({
      renderCount: window.__libraryPerf?.uploadRenderCount || 0,
      scheduledCount: window.__libraryPerf?.uploadRenderScheduledCount || 0,
    }));
    await page.setInputFiles("#file-input", {
      name: "perf-upload.txt",
      mimeType: "text/plain",
      buffer: Buffer.from("upload smoke"),
    });
    await page.waitForFunction(
      (scheduledCount) => (window.__libraryPerf?.uploadRenderScheduledCount || 0) > scheduledCount,
      beforeUpload.scheduledCount,
    );
    await page.waitForFunction(() => !document.querySelector("#upload-progress")?.classList.contains("hidden"));
    assert(writePayloads.length === 1, "Upload must use the raw Library upload transport");
    assert(writePayloads[0]?.uri === `${documentsUri}/perf-upload.txt`, "Upload must target the current Library folder");
    assert(writePayloads[0]?.body === "upload smoke", "Upload must send the file body without base64 JSON wrapping");
    const afterUpload = await page.evaluate(() => window.__libraryPerf);
    const uploadScheduledDelta = afterUpload.uploadRenderScheduledCount - beforeUpload.scheduledCount;
    const uploadRenderDelta = afterUpload.uploadRenderCount - beforeUpload.renderCount;
    assert(uploadScheduledDelta >= 1, "Upload progress must render through a scheduled frame");
    assert(
      uploadRenderDelta <= uploadScheduledDelta + 3,
      `Upload progress rendered too often: ${uploadRenderDelta} renders for ${uploadScheduledDelta} scheduled frames`,
    );

    // A refreshed listing can change presentation and available actions without changing its revision.
    documents[0].mime = "application/json";
    documents[0].capabilities = ["properties"];
    await page.locator(".place").filter({ hasText: "Desktop" }).first().click();
    await page.locator(".crumb-current").filter({ hasText: "Desktop" }).first().waitFor();
    await page.locator(".place").filter({ hasText: "Documents" }).first().click();
    const changedItem = page.locator(".item").filter({ hasText: "File-0001.txt" }).first();
    await changedItem.locator(".item-type").filter({ hasText: "application/json" }).waitFor();
    await changedItem.click({ button: "right" });
    assert(await page.locator("#context-menu .menu-item").filter({ hasText: /^Download$/ }).count() === 0,
      "Refreshed capabilities must replace cached menu actions");
    await page.keyboard.press("Escape");

    holdUploads = true;
    const file = (name) => ({ name, mimeType: "text/plain", buffer: Buffer.from(name) });
    await page.setInputFiles("#file-input", [file("documents-one.txt"), file("documents-two.txt")]);
    await waitForUpload("documents-one.txt");
    await page.locator(".place").filter({ hasText: "Desktop" }).first().click();
    await page.locator(".crumb-current").filter({ hasText: "Desktop" }).first().waitFor();
    await page.setInputFiles("#file-input", file("desktop-one.txt"));
    await waitForUpload("desktop-one.txt");
    for (const name of ["documents-one.txt", "documents-two.txt", "desktop-one.txt"]) {
      await page.locator(".upload-name").filter({ hasText: name }).waitFor();
    }
    finishUpload("documents-one.txt");
    await waitForUpload("documents-two.txt");
    finishUpload("documents-two.txt");
    await page.locator(".upload-name").filter({ hasText: "documents-one.txt" }).waitFor({ state: "detached" });
    assert(await page.locator(".upload-name").filter({ hasText: "desktop-one.txt" }).count() === 1,
      "Completing one batch must preserve another batch's progress");
    finishUpload("desktop-one.txt");
    await page.waitForFunction(() => document.querySelector("#upload-progress").classList.contains("hidden"));
    assert(writePayloads.length === 4, "Every chosen file must be dispatched exactly once");
    for (const name of ["documents-one.txt", "documents-two.txt", "desktop-one.txt"]) {
      const destination = name.startsWith("documents-") ? documentsUri : desktopUri;
      assert(writePayloads.some((entry) => entry.uri === `${destination}/${name}` && entry.body === name),
        `${name} must retain its selected destination and exact bytes during navigation`);
    }
    assert(pageErrors.length === 0 && consoleErrors.length === 0 && failedRequests.length === 0,
      `Library fixture reported errors: ${JSON.stringify({ pageErrors, consoleErrors, failedRequests })}`);
    await assertFolderLoadRecovery(page);
    assert(pageErrors.length === 0 && failedRequests.length === 0,
      `Folder recovery fixture reported unexpected errors: ${JSON.stringify({ pageErrors, failedRequests })}`);
    assert(consoleErrors.length === 1 && consoleErrors[0].message.includes("This folder is temporarily unavailable."),
      `Only the current folder failure should be reported: ${JSON.stringify(consoleErrors)}`);
    console.log(JSON.stringify({ objectCount, browser: browser.version(), earlyEnd, refreshEnd, sorts }, null, 2));
  } finally {
    await browser?.close();
    server.closeAllConnections();
    await new Promise((resolve) => server.close(resolve));
  }
}

run().then(
  () => console.log("PASS Library performance smoke"),
  (error) => {
    console.error(error);
    process.exit(1);
  },
);
