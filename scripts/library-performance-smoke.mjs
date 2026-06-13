#!/usr/bin/env node
import { createReadStream } from "node:fs";
import { stat } from "node:fs/promises";
import { createServer } from "node:http";
import { createRequire } from "node:module";
import path from "node:path";

const require = createRequire(new URL("../elastos/tools/browser-playwright-engine/package.json", import.meta.url));
const { chromium } = require("playwright");

const capsuleRoot = path.resolve("capsules/library");
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

const documents = Array.from({ length: 1000 }, (_, index) => {
  const number = String(index + 1).padStart(4, "0");
  return object(`${documentsUri}/File-${number}.txt`, `File-${number}.txt`, "file", index);
});
const writePayloads = [];

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
        res.writeHead(200, { "content-type": "application/json" });
        res.end(JSON.stringify({
          status: "ok",
          data: {
            transport: "raw-body",
            object: object(uri, name, "file", writePayloads.length),
          },
        }));
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
            objects: uri === documentsUri ? documents : [],
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

async function run() {
  const server = createAppServer();
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  const { port } = server.address();
  const browser = await chromium.launch({ headless: true });
  try {
    const page = await browser.newPage();
    page.on("pageerror", (error) => {
      throw error;
    });
    await page.goto(`http://127.0.0.1:${port}/apps/library/?home_token=${encodeURIComponent(token)}`);
    await page.locator(".item").filter({ hasText: "File-0001.txt" }).first().waitFor();

    const initialPerf = await page.evaluate(() => window.__libraryPerf);
    assert(initialPerf?.iconFetchCount === 0, "Library must not fetch/hydrate SVG icons from JavaScript");
    assert(initialPerf?.renderPlacesCount === 1, "Library sidebar should render once on boot");
    assert(initialPerf?.lastContentRender?.objectCount === 1000, "Library must render the full large folder");
    assert(initialPerf?.lastContentRender?.chunked === true, "Large folders should render in chunks for fast first paint");
    assert(initialPerf?.lastContentRender?.initialRenderedCount < 1000, "Initial large-folder paint should not build every row synchronously");
    assert(initialPerf?.lastContentRender?.durationMs < 750, `Large folder render too slow: ${initialPerf?.lastContentRender?.durationMs}ms`);
    assert(await page.locator("[data-icon-src], [data-icon-ready]").count() === 0, "Library DOM must not expose async icon hydration markers");
    await page.locator(".item").filter({ hasText: "File-1000.txt" }).first().waitFor();
    await page.waitForFunction(() => window.__libraryPerf?.lastContentRender?.complete === true);

    await page.waitForFunction(() => (window.__libraryPerf?.folderCacheSize || 0) >= 3);
    await page.locator(".place").filter({ hasText: "Desktop" }).first().click();
    await page.locator(".crumb-current").filter({ hasText: "Desktop" }).first().waitFor();
    await page.locator(".place").filter({ hasText: "Documents" }).first().click();
    await page.locator(".item").filter({ hasText: "File-1000.txt" }).first().waitFor();
    const afterNavigation = await page.evaluate(() => window.__libraryPerf);
    assert(afterNavigation.renderPlacesCount === 1, "Folder navigation must not rebuild the sidebar");
    assert(afterNavigation.folderCacheHits >= 2, "Root navigation should render from prefetched folder cache");
    assert(afterNavigation.objectNodeCacheHits >= 1000, "Returning to a large folder should reuse existing item DOM nodes");

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
  } finally {
    await browser.close();
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
