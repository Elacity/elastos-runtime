#!/usr/bin/env node
import { createReadStream } from "node:fs";
import { createHash } from "node:crypto";
import { stat } from "node:fs/promises";
import { createServer } from "node:http";
import { createRequire } from "node:module";
import path from "node:path";
import { pathToFileURL } from "node:url";
import { assertPhoneDrawer, DRAWER_SETTLE_MS, PHONE_VIEWPORT, setPhoneFormFactor } from "./lib/phone-drawer-assert.mjs";

const PHONE_SCREENSHOT = "/tmp/documents-phone-390x844.png";
const PHONE_DRAWER_SCREENSHOT = "/tmp/documents-phone-drawer-390x844.png";
const PHONE_MORE_SCREENSHOT = "/tmp/documents-phone-more-390x844.png";
const PHONE_TOUCH_TARGET_PX = 44;

const brave = process.env.BRAVE_BIN || "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser";
const playwrightModule = process.env.ELASTOS_PLAYWRIGHT_MODULE
  ? await import(pathToFileURL(process.env.ELASTOS_PLAYWRIGHT_MODULE).href)
  : createRequire(new URL("../elastos/tools/browser-playwright-engine/package.json", import.meta.url))("playwright");
const { chromium } = playwrightModule.chromium ? playwrightModule : playwrightModule.default;

const capsuleRoot = path.resolve("capsules/documents/browser");
const homeClipboardClientPath = path.resolve("capsules/home/browser/home-clipboard-client.js");
const homeNavigationClientPath = path.resolve("capsules/home/browser/home-navigation-client.js");
const homeClipboardProtocolPath = path.resolve("capsules/home/browser/home-clipboard-protocol.js");

const documents = new Map();
const saveCalls = [];
const publishedFixtureCid = "bafypublishedfixture";
const publishedWebLink = "https://example.invalid/published-link";
let delayNextSave = false;
let delayedSaveStart = null;
let delayedSaveRelease = null;
let activeSaveCalls = 0;
let maxConcurrentSaveCalls = 0;

function assert(condition, message) {
  if (!condition) {
    throw new Error(message);
  }
}

async function waitForCondition(predicate, timeoutMs, label) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (predicate()) {
      return;
    }
    await new Promise((resolve) => setTimeout(resolve, 25));
  }
  throw new Error(label);
}

function createDocument(docDid, title, fileName, body, cid) {
  return withRevision({
    doc_did: docDid,
    document_uri: `localhost://ElastOS/Documents/${docDid}`,
    title,
    file_name: fileName,
    working_copy_uri: `localhost://ElastOS/Documents/${docDid}`,
    body,
    created_at: 1_780_000_000,
    updated_at: 1_780_000_600,
    latest_published_cid: cid,
    publish_history: cid ? [{ cid, published_at: 1_780_000_600 }] : [],
  });
}

function withRevision(document) {
  return { ...document, revision: createHash("sha256").update(JSON.stringify([
    "person:local:documents-fixture", document.doc_did, document.working_copy_uri, document.title, document.body,
  ])).digest("hex") };
}

function resetDocumentsFixture() {
  documents.clear();
  documents.set("doc-alpha", createDocument("doc-alpha", "Alpha", "alpha.md", "# Alpha\n\n- [ ] Check this item\n- [x] Keep this item\n\n## Notes\n\nShared notes here.", "bafyalpha"));
  documents.set("doc-beta", createDocument("doc-beta", "Beta", "beta.md", "# Beta\n\nRegular paragraph text.", null));
  saveCalls.length = 0;
  delayNextSave = false;
  delayedSaveStart = null;
  delayedSaveRelease = null;
  activeSaveCalls = 0;
  maxConcurrentSaveCalls = 0;
}

function createDeferred() {
  let resolve = () => {};
  const promise = new Promise((resolvePromise) => {
    resolve = resolvePromise;
  });
  return { promise, resolve };
}

function armDelayedSave() {
  delayNextSave = true;
  delayedSaveStart = createDeferred();
  delayedSaveRelease = createDeferred();
  return {
    started: delayedSaveStart.promise,
    release() {
      delayedSaveRelease.resolve();
    },
  };
}

function listSummary() {
  return [...documents.values()].map((document) => ({
    doc_did: document.doc_did,
    title: document.title,
    file_name: document.file_name,
    working_copy_uri: document.working_copy_uri,
    updated_at: document.updated_at,
    latest_published_cid: document.latest_published_cid || null,
  }));
}

// A plain file in the Documents folder with no Documents object (made in Library).
const folderFile = {
  uri: "localhost://Users/fixture/Documents/Notes/Plain.txt",
  name: "Plain.txt",
  path: "Notes/Plain.txt",
  updated_at: 1_779_000_000,
};
const folderFileBody = "Plain text from Library.";

function contentType(filePath) {
  if (filePath.endsWith(".html")) return "text/html; charset=utf-8";
  if (filePath.endsWith(".css")) return "text/css; charset=utf-8";
  if (filePath.endsWith(".js")) return "text/javascript; charset=utf-8";
  if (filePath.endsWith(".woff2")) return "font/woff2";
  return "application/octet-stream";
}

function trackPage(page, consoleErrors, pageErrors, failedRequests, errorResponses) {
  page.on("console", (message) => {
    if (message.type() === "error") {
      consoleErrors.push({
        text: message.text(),
        location: message.location(),
      });
    }
  });
  page.on("pageerror", (error) => {
    pageErrors.push(error?.stack || String(error));
  });
  page.on("requestfailed", (request) => {
    failedRequests.push({
      url: request.url(),
      method: request.method(),
      error: request.failure()?.errorText || "request failed",
    });
  });
  page.on("response", (response) => {
    if (response.status() >= 400) {
      errorResponses.push({
        url: response.url(),
        status: response.status(),
      });
    }
  });
}

async function fillMarkdownCase(page, source, predicate, label) {
  await page.locator("#editor").fill(source);
  await page.waitForFunction(predicate);
  const preview = await page.evaluate(() => ({
    html: document.getElementById("preview")?.innerHTML || "",
    text: document.getElementById("preview")?.textContent || "",
    outlineCount: document.querySelectorAll("#outline-list button").length,
  }));
  assert(preview.html.length > 0, `Documents parser made no progress for ${label}.`);
  return preview;
}

function json(res, data) {
  res.writeHead(200, { "content-type": "application/json" });
  res.end(JSON.stringify({ status: "ok", data }));
}

function createAppServer() {
  return createServer(async (req, res) => {
    const url = new URL(req.url || "/", "http://127.0.0.1");
    const providerMatch = url.pathname.match(/^\/api\/provider\/documents\/([^/]+)$/);
    if (providerMatch) {
      const op = decodeURIComponent(providerMatch[1]);
      let body = "";
      req.on("data", (chunk) => {
        body += chunk;
      });
      req.on("end", async () => {
        const payload = body ? JSON.parse(body) : {};
        if (op === "summary") {
          json(res, { documents: listSummary(), files: [folderFile] });
          return;
        }
        if (op === "get") {
          json(res, { document: documents.get(payload.doc_did) || null });
          return;
        }
        if (op === "create") {
          const docDid = `doc-${documents.size + 1}`;
          const document = createDocument(docDid, payload.title || "Untitled document", `${(payload.title || "untitled").toLowerCase().replace(/\s+/g, "-")}.md`, "", null);
          documents.set(docDid, document);
          json(res, { document });
          return;
        }
        if (op === "save") {
          saveCalls.push({
            docDid: payload.doc_did,
            title: payload.title,
            body: payload.body || "",
            if_revision: payload.if_revision,
          });
          activeSaveCalls += 1;
          maxConcurrentSaveCalls = Math.max(maxConcurrentSaveCalls, activeSaveCalls);
          try {
            if (delayNextSave) {
              delayNextSave = false;
              delayedSaveStart?.resolve();
              await delayedSaveRelease?.promise;
              delayedSaveStart = null;
              delayedSaveRelease = null;
            }
            const current = documents.get(payload.doc_did);
            if (!current || !/^[0-9a-f]{64}$/.test(payload.if_revision || "") || payload.if_revision !== current.revision) {
              res.writeHead(200, { "content-type": "application/json" });
              res.end(JSON.stringify({ status: "error", code: "documents_error", message: "Document changed since it was read. Read the current document before saving." }));
              return;
            }
            const updated = withRevision({
              ...current,
              title: payload.title.trim() || "Untitled",
              body: payload.body || "",
              updated_at: current.updated_at + 1,
            });
            documents.set(payload.doc_did, updated);
            json(res, { document: updated });
            return;
          } finally {
            activeSaveCalls -= 1;
          }
        }
        if (op === "save_as") {
          const current = documents.get(payload.doc_did);
          const docDid = `doc-${documents.size + 1}`;
          const duplicate = withRevision({
            ...current,
            doc_did: docDid,
            document_uri: `localhost://ElastOS/Documents/${docDid}`,
            working_copy_uri: `localhost://ElastOS/Documents/${docDid}`,
            title: payload.title || `${current.title} copy`,
            file_name: payload.file_name || `${current.file_name.replace(/\.md$/i, "")}-copy.md`,
            body: payload.body || current.body,
            latest_published_cid: null,
            publish_history: [],
          });
          documents.set(docDid, duplicate);
          json(res, { document: duplicate });
          return;
        }
        if (op === "publish") {
          const current = documents.get(payload.doc_did);
          const updated = {
            ...current,
            latest_published_cid: current.latest_published_cid || `bafy${payload.doc_did}`,
            publish_history: [{ cid: current.latest_published_cid || `bafy${payload.doc_did}`, published_at: current.updated_at + 1 }],
          };
          documents.set(payload.doc_did, updated);
          json(res, { uri: `elastos://${updated.latest_published_cid}` });
          return;
        }
        if (op === "unpublish") {
          const current = documents.get(payload.doc_did);
          const updated = { ...current, latest_published_cid: null, publish_history: [] };
          documents.set(payload.doc_did, updated);
          json(res, { ok: true });
          return;
        }
        if (op === "delete") {
          documents.delete(payload.doc_did);
          json(res, { ok: true });
          return;
        }
        json(res, {});
      });
      return;
    }

    if (url.pathname === "/api/viewers/documents/library-object" && req.method === "GET") {
      if (url.searchParams.get("uri") !== folderFile.uri) {
        res.writeHead(404);
        res.end("Library object not found");
        return;
      }
      json(res, {
        object: { uri: folderFile.uri, name: folderFile.name, revision: "fixture-revision", mime: "text/plain", modified_at: folderFile.updated_at },
        data: Buffer.from(folderFileBody).toString("base64"),
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
    if (url.pathname === "/favicon.ico") {
      res.writeHead(204);
      res.end();
      return;
    }
    if (url.pathname === "/home-fixture") {
      // Documents framed as Home frames it; the page records what it asks Home.
      // ?without_origin drops home_origin, so Documents has no Home bridge;
      // ?cid opens a published revision that is not in the local list.
      const homeOrigin = `http://${req.headers.host}`;
      const params = new URLSearchParams();
      if (!url.searchParams.has("without_origin")) params.set("home_origin", homeOrigin);
      if (url.searchParams.has("cid")) params.set("cid", url.searchParams.get("cid"));
      const query = params.size ? `?${params}` : "";
      res.writeHead(200, { "content-type": "text/html; charset=utf-8" });
      res.end(`<!doctype html><script>window.homeMessages = [];
addEventListener("message", (event) => window.homeMessages.push(event.data));</script>
<iframe title="Documents" style="width:1200px;height:850px" src="/apps/documents/${query}#home_token=documents-link-token"></iframe>`);
      return;
    }
    if (url.pathname.startsWith(`/s/${publishedFixtureCid}/`)) {
      // A published two-file revision with links of every kind.
      const file = url.pathname.slice(`/s/${publishedFixtureCid}/`.length);
      const bodies = {
        "_files.json": JSON.stringify(["first.md", "second.md"]),
        "_share.json": JSON.stringify({ share_id: "share-fixture", version: 1 }),
        "first.md": `# Published\n\n[Out](${publishedWebLink}) [Same](http://${req.headers.host}/s/${publishedFixtureCid}/_share.json) [Jump](#notes-2) [Rel](/apps/documents/)\n\n`
          + Array.from({ length: 80 }, (_, index) => `Filler line ${index}`).join("\n\n") + "\n\n# Notes\n\nEnd.",
        "second.md": `# Second file\n\n[Same second](http://${req.headers.host}/s/${publishedFixtureCid}/_share.json) [Top](#second-file-1)\n\nSecond body.`,
      };
      if (!Object.hasOwn(bodies, file)) {
        res.writeHead(404);
        res.end("Not found");
        return;
      }
      res.writeHead(200, { "content-type": file.endsWith(".json") ? "application/json" : "text/markdown; charset=utf-8" });
      res.end(bodies[file]);
      return;
    }

    let relative = url.pathname.replace(/^\/apps\/documents\/?/, "") || "index.html";
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

// Phone keeps view and Save in the row; the secondary actions live behind More.
async function assertPhoneMoreMenu(page) {
  const readMenu = () => page.evaluate(() => {
    const visible = (node) => node instanceof HTMLElement && node.getClientRects().length > 0;
    const row = document.querySelector(".toolbar-actions");
    const menu = document.getElementById("more-menu");
    const menuRect = menu.getBoundingClientRect();
    const items = [...menu.querySelectorAll("button")].filter(visible).map((button) => ({
      id: button.id,
      height: Math.round(button.getBoundingClientRect().height),
      label: getComputedStyle(button, "::after").content.replace(/^"|"$/g, ""),
    }));
    return {
      rowOverflow: row.scrollWidth - row.clientWidth,
      rowTargets: [...row.querySelectorAll("button")].filter(visible).filter((button) => !menu.contains(button))
        .map((button) => ({ id: button.id, height: Math.round(button.getBoundingClientRect().height) })),
      titleHeight: Math.round(document.getElementById("title-input").getBoundingClientRect().height),
      expanded: document.getElementById("more-button").getAttribute("aria-expanded"),
      menuOpen: visible(menu) && menuRect.height > 0,
      menuOnTop: menu.contains(document.elementFromPoint(menuRect.left + menuRect.width / 2, menuRect.top + menuRect.height / 2)),
      clippedBy: (() => {
        const chain = [];
        for (let node = menu.parentElement; node; node = node.parentElement) {
          const style = getComputedStyle(node);
          if (style.overflow !== "visible" || style.zIndex !== "auto") {
            chain.push(`${node.tagName}.${node.className} overflow=${style.overflow} z=${style.zIndex}`);
          }
        }
        return chain;
      })(),
      menuInViewport: menuRect.left >= 0 && menuRect.right <= window.innerWidth,
      items,
    };
  });
  const closed = await readMenu();
  assert(
    closed.rowOverflow <= 1 && !closed.menuOpen && closed.expanded === "false"
      && closed.rowTargets.map((target) => target.id).join(",") === "mode-write,mode-read,save-button,more-button",
    `Documents phone: the row holds Write, Read, Save and More without scrolling; the rest waits in More. Got ${JSON.stringify(closed)}`,
  );
  assert(
    closed.titleHeight >= PHONE_TOUCH_TARGET_PX && closed.rowTargets.every((target) => target.height >= PHONE_TOUCH_TARGET_PX),
    `Documents phone: the title, Write, Read, Save and More must be 44 px targets. Got ${JSON.stringify(closed)}`,
  );

  await page.locator("#more-button").click();
  const open = await readMenu();
  await page.screenshot({ path: PHONE_MORE_SCREENSHOT });
  const ids = open.items.map((item) => item.id);
  assert(
    open.menuOpen && open.menuOnTop && open.expanded === "true" && open.menuInViewport
      && ["save-as-button", "delete-button"].every((id) => ids.includes(id))
      && !ids.some((id) => ["publish-button", "unpublish-button"].includes(id)),
    `Documents phone: More must open on screen with Save as and Delete, and no Publish or Unpublish. Got ${JSON.stringify(open)}`,
  );
  assert(
    open.items.every((item) => item.height >= PHONE_TOUCH_TARGET_PX && item.label.length > 0),
    `Documents phone: each More action must be a labelled 44 px row. Got ${JSON.stringify(open.items)}`,
  );

  await page.keyboard.press("Escape");
  const escaped = await readMenu();
  const focused = await page.evaluate(() => document.activeElement?.id || "");
  assert(!escaped.menuOpen && escaped.expanded === "false" && focused === "more-button", `Documents phone: Escape must close More and return focus. Got ${JSON.stringify({ escaped, focused })}`);

  await page.locator("#more-button").click();
  await page.locator("#editor").click();
  const dismissed = await readMenu();
  assert(!dismissed.menuOpen && dismissed.expanded === "false", `Documents phone: tapping outside must close More. Got ${JSON.stringify(dismissed)}`);
}

async function assertNoHorizontalOverflow(page, label) {
  const overflow = await page.evaluate(() => ({
    body: document.documentElement.scrollWidth - document.documentElement.clientWidth,
    shell: (() => {
      const shell = document.getElementById("documents-shell");
      return shell ? shell.scrollWidth - shell.clientWidth : 0;
    })(),
    main: (() => {
      const main = document.querySelector(".documents-main");
      return main ? main.scrollWidth - main.clientWidth : 0;
    })(),
  }));
  if (overflow.body > 1 || overflow.shell > 1 || overflow.main > 1) {
    const details = await page.evaluate(() => {
      const viewportWidth = window.innerWidth;
      const main = document.querySelector(".documents-main");
      const interesting = [
        ".documents-toolbar",
        ".toolbar-actions",
        ".documents-identity",
        ".title-input",
        ".find-bar",
        ".find-input",
        ".find-actions",
        ".documents-workspace",
        ".editor-pane",
        ".preview-pane",
        ".preview-scroll",
      ];
      return {
        viewportWidth,
        main: main
          ? {
              clientWidth: main.clientWidth,
              scrollWidth: main.scrollWidth,
            }
          : null,
        elements: interesting.map((selector) => {
          const node = document.querySelector(selector);
          if (!(node instanceof HTMLElement)) {
            return null;
          }
          const rect = node.getBoundingClientRect();
          const style = window.getComputedStyle(node);
          return {
            selector,
            width: rect.width,
            left: rect.left,
            right: rect.right,
            scrollWidth: node.scrollWidth,
            clientWidth: node.clientWidth,
            overflowX: style.overflowX,
            flexWrap: style.flexWrap,
            minWidth: style.minWidth,
          };
        }).filter(Boolean),
      };
    });
    console.error(JSON.stringify({ label, overflow, details }, null, 2));
    assert(false, `${label}: horizontal overflow`);
  }
}

async function run() {
  resetDocumentsFixture();
  const server = createAppServer();
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  const { port } = server.address();
  let browser;
  try {
    browser = await chromium.launch({ headless: true, executablePath: brave });
    const consoleErrors = [];
    const pageErrors = [];
    const failedRequests = [];
    const errorResponses = [];
    const page = await browser.newPage({ viewport: { width: 1280, height: 900 } });
    trackPage(page, consoleErrors, pageErrors, failedRequests, errorResponses);
    await page.goto(`http://127.0.0.1:${port}/apps/documents/?home_origin=${encodeURIComponent("https://home.example")}#home_token=documents-layout-token`);
    await page.locator(".document-list-item").first().waitFor();

    const initial = await page.evaluate(() => ({
      tokenLink: !!document.querySelector('link[href="./elastos-ui.css"]'),
      themeScript: !!document.querySelector('script[src="./elastos-theme.js"]'),
      writeText: document.getElementById("mode-write")?.textContent?.trim() || "",
      splitText: document.getElementById("mode-split")?.textContent?.trim() || "",
      readText: document.getElementById("mode-read")?.textContent?.trim() || "",
      writePressed: document.getElementById("mode-write")?.getAttribute("aria-pressed") || "",
      statusHidden: document.getElementById("status-row")?.classList.contains("hidden") || false,
    }));
    assert(initial.tokenLink && initial.themeScript, "Documents must load the vendored theme assets.");
    assert(initial.writeText === "" && initial.splitText === "" && initial.readText === "", "Documents mode buttons must stay icon-only.");
    assert(initial.writePressed === "true", "Documents must default to Write view.");
    assert(initial.statusHidden, "Documents idle status row must start hidden.");

    await page.locator(".document-list-item").first().click();
    await page.waitForFunction(() => document.getElementById("title-input")?.value === "Alpha");
    const selected = await page.evaluate(() => ({
      copyButton: !!document.getElementById("copy-published-link"),
      publishControls: !!document.querySelector("#publish-button, #unpublish-button, #pane-history, #history-panel, .document-list-published-icon"),
      titleValue: document.getElementById("title-input")?.value || "",
    }));
    assert(!selected.copyButton, "Published documents must not offer Copy Published Link.");
    assert(!selected.publishControls, "Documents must not show Publish, Unpublish, History or the Published icon.");
    assert(selected.titleValue === "Alpha", "Documents layout fixture must load the selected document.");

    await page.locator(".document-list-item").first().click({ button: "right" });
    await page.locator("#context-menu:not(.hidden)").waitFor();
    const contextItems = await page.locator("#context-menu .context-menu-item").evaluateAll((nodes) =>
      nodes.map((node) => node.textContent?.trim() || ""),
    );
    assert(
      JSON.stringify(contextItems) === JSON.stringify(["Duplicate", "Unpublish", "Delete"]),
      `Documents published row context menu must stay item-aware. Got ${JSON.stringify(contextItems)}`,
    );
    await page.evaluate(() => {
      const item = document.querySelector(".document-list-item");
      if (!(item instanceof HTMLElement)) {
        throw new Error("document row missing");
      }
      item.dispatchEvent(new MouseEvent("contextmenu", {
        bubbles: true,
        cancelable: true,
        clientX: window.innerWidth - 2,
        clientY: window.innerHeight - 2,
      }));
    });
    await page.locator("#context-menu:not(.hidden)").waitFor();
    const clampedMenu = await page.evaluate(() => {
      const menu = document.getElementById("context-menu");
      if (!(menu instanceof HTMLElement)) {
        return null;
      }
      const rect = menu.getBoundingClientRect();
      return {
        left: rect.left,
        top: rect.top,
        right: rect.right,
        bottom: rect.bottom,
        viewportWidth: window.innerWidth,
        viewportHeight: window.innerHeight,
      };
    });
    assert(!!clampedMenu, "Documents context menu must render for clamp verification.");
    assert(clampedMenu.left >= 0 && clampedMenu.top >= 0, `Documents context menu must stay inside the top-left viewport edge. Got ${JSON.stringify(clampedMenu)}`);
    assert(
      clampedMenu.right <= clampedMenu.viewportWidth &&
        clampedMenu.bottom <= clampedMenu.viewportHeight,
      `Documents context menu must clamp to the bottom-right viewport edge. Got ${JSON.stringify(clampedMenu)}`,
    );

    await page.evaluate(() => {
      document.dispatchEvent(new KeyboardEvent("keydown", { key: "f", metaKey: true, bubbles: true }));
    });
    await page.locator("#find-bar:not(.hidden)").waitFor();
    await page.locator("#find-input").fill("item");
    await page.waitForFunction(() => (document.getElementById("find-count")?.textContent || "") === "1 / 2");
    const writeFindState = await page.evaluate(() => ({
      count: document.getElementById("find-count")?.textContent || "",
      selection: (() => {
        const editor = document.getElementById("editor");
        if (!(editor instanceof HTMLTextAreaElement)) {
          return "";
        }
        return editor.value.slice(editor.selectionStart, editor.selectionEnd);
      })(),
      highlights: document.querySelectorAll("mark.find-highlight").length,
    }));
    assert(writeFindState.count === "1 / 2", `Documents find must count source matches in Write view. Got ${writeFindState.count}`);
    assert(writeFindState.selection.toLowerCase() === "item", `Documents find must select the active editor match in Write view. Got ${JSON.stringify(writeFindState.selection)}`);
    assert(writeFindState.highlights === 0, "Documents find must not require preview highlights in Write view.");

    await page.evaluate(() => {
      document.dispatchEvent(new KeyboardEvent("keydown", { key: "g", metaKey: true, bubbles: true }));
    });
    await page.waitForFunction(() => (document.getElementById("find-count")?.textContent || "") === "2 / 2");
    const steppedFindState = await page.evaluate(() => ({
      count: document.getElementById("find-count")?.textContent || "",
      selectionStart: document.getElementById("editor")?.selectionStart ?? -1,
      selection: (() => {
        const editor = document.getElementById("editor");
        if (!(editor instanceof HTMLTextAreaElement)) {
          return "";
        }
        return editor.value.slice(editor.selectionStart, editor.selectionEnd);
      })(),
    }));
    assert(steppedFindState.count === "2 / 2", "Documents find must step to the next source match.");
    assert(steppedFindState.selection.toLowerCase() === "item", "Documents find step must keep the editor selection on the active match.");

    await page.locator("#mode-split").click();
    await page.locator(".task-checkbox").first().waitFor();
    await page.waitForFunction(() => document.querySelectorAll("mark.find-highlight").length >= 2);
    const splitFindState = await page.evaluate(() => ({
      count: document.getElementById("find-count")?.textContent || "",
      highlights: document.querySelectorAll("mark.find-highlight").length,
      activeHighlights: document.querySelectorAll("mark.find-highlight.active").length,
    }));
    assert(splitFindState.count === "2 / 2", "Documents split view must keep the same find state.");
    assert(splitFindState.highlights >= 2 && splitFindState.activeHighlights === 1, "Documents split view must highlight the visible preview match.");

    await page.evaluate(() => {
      const editor = document.getElementById("editor");
      if (!(editor instanceof HTMLTextAreaElement)) {
        throw new Error("editor missing");
      }
      const target = editor.value.indexOf("Check this item");
      editor.focus();
      editor.setSelectionRange(target, target);
      editor.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", metaKey: true, bubbles: true }));
    });
    await page.waitForFunction(() => (document.getElementById("editor")?.value || "").includes("- [x] Check this item"));

    await page.evaluate(() => {
      const editor = document.getElementById("editor");
      if (!(editor instanceof HTMLTextAreaElement)) {
        throw new Error("editor missing");
      }
      const target = editor.value.indexOf("Shared notes here.");
      editor.focus();
      editor.setSelectionRange(target, target);
      editor.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", metaKey: true, bubbles: true }));
    });
    await page.waitForFunction(() => (document.getElementById("editor")?.value || "").includes("- [ ] Shared notes here."));

    await page.locator("#mode-read").click();
    const readMode = await page.evaluate(() => ({
      readPressed: document.getElementById("mode-read")?.getAttribute("aria-pressed") || "",
      writePressed: document.getElementById("mode-write")?.getAttribute("aria-pressed") || "",
      highlights: document.querySelectorAll("mark.find-highlight").length,
    }));
    assert(readMode.readPressed === "true" && readMode.writePressed === "false", "Documents mode buttons must keep aria-pressed in sync.");
    assert(readMode.highlights >= 2, "Documents read view must keep preview find highlights visible.");

    await assertNoHorizontalOverflow(page, "wide");
    // Hide list sits on the list itself; collapsing gives the page the room and a
    // Show list control at the title's leading edge brings the list back.
    const desktopLayout = () => page.evaluate(() => {
      const visible = (node) => !!node && getComputedStyle(node).display !== "none" && node.getBoundingClientRect().width > 0;
      return {
        toggleInList: !!document.querySelector("#documents-sidebar #sidebar-toggle"),
        sidebar: visible(document.getElementById("documents-sidebar")),
        showList: visible(document.getElementById("sidebar-show")),
        mainLeft: Math.round(document.querySelector(".documents-main").getBoundingClientRect().left),
      };
    });
    const expanded = await desktopLayout();
    assert(expanded.toggleInList && expanded.sidebar && !expanded.showList && expanded.mainLeft >= 200,
      `Documents desktop: Hide list belongs to the open list. Got ${JSON.stringify(expanded)}`);
    const searchFits = await page.evaluate(() => {
      const input = document.getElementById("sidebar-search");
      const style = getComputedStyle(input);
      const context = document.createElement("canvas").getContext("2d");
      context.font = style.font;
      const room = input.clientWidth - parseFloat(style.paddingLeft) - parseFloat(style.paddingRight);
      return context.measureText(input.placeholder).width <= room;
    });
    assert(searchFits, "Documents desktop: the list's search placeholder must fit beside New and Hide list.");
    await page.locator("#sidebar-toggle").click();
    const collapsed = await desktopLayout();
    assert(!collapsed.sidebar && collapsed.showList && collapsed.mainLeft === 0,
      `Documents desktop: Hide list must give the page the list's room and offer Show list. Got ${JSON.stringify(collapsed)}`);
    await page.locator("#sidebar-show").click();
    const reopened = await desktopLayout();
    assert(reopened.sidebar && !reopened.showList && reopened.mainLeft >= 200,
      `Documents desktop: Show list must bring the list back. Got ${JSON.stringify(reopened)}`);
    await assertNoHorizontalOverflow(page, "wide");
    // A list hidden on the desktop must still fill the phone drawer.
    await page.locator("#sidebar-toggle").click();
    await page.setViewportSize(PHONE_VIEWPORT);
    await assertNoHorizontalOverflow(page, "narrow");

    await page.locator("#mode-write").click();
    await setPhoneFormFactor(page);
    await page.waitForTimeout(DRAWER_SETTLE_MS);
    await assertNoHorizontalOverflow(page, "phone");
    const phoneList = await page.evaluate(() => ({
      collapsed: document.querySelector(".documents-shell")?.dataset.sidebarCollapsed || "",
      list: getComputedStyle(document.getElementById("documents-list")).display,
      search: getComputedStyle(document.querySelector(".sidebar-search")).display,
    }));
    assert(
      phoneList.collapsed === "true" && phoneList.list !== "none" && phoneList.search !== "none",
      `Documents phone: a list hidden on the desktop must still fill the drawer. Got ${JSON.stringify(phoneList)}`,
    );
    const phoneEditor = await page.evaluate(() => {
      const editor = document.getElementById("editor").getBoundingClientRect();
      const hidden = (id) => getComputedStyle(document.getElementById(id)).display === "none";
      return {
        editorWidth: Math.round(editor.width),
        editorHeight: Math.round(editor.height),
        editorFont: getComputedStyle(document.getElementById("editor")).fontSize,
        splitHidden: hidden("mode-split"),
        hideListHidden: hidden("sidebar-toggle") && hidden("sidebar-show"),
      };
    });
    await page.screenshot({ path: PHONE_SCREENSHOT });
    assert(
      phoneEditor.editorWidth >= PHONE_VIEWPORT.width - 40 && phoneEditor.editorHeight >= PHONE_VIEWPORT.height * 0.6,
      `Documents phone: the editor must take the whole stage. Got ${JSON.stringify(phoneEditor)}`,
    );
    assert(
      phoneEditor.editorFont === "16px" && phoneEditor.splitHidden && phoneEditor.hideListHidden,
      `Documents phone: 16 px editor (no iOS focus zoom), no Split or Hide list controls. Got ${JSON.stringify(phoneEditor)}`,
    );
    await assertPhoneMoreMenu(page);
    await assertPhoneDrawer(page, page, {
      label: "Documents",
      drawer: "#documents-sidebar",
      room: ".documents-main",
      closeTarget: '.document-list-item[data-doc-did="doc-beta"]',
      screenshot: PHONE_DRAWER_SCREENSHOT,
    });
    await setPhoneFormFactor(page, false);
    const desktopSidebar = await page.evaluate(() => getComputedStyle(document.getElementById("documents-sidebar")).display);
    assert(desktopSidebar === "none", `Documents desktop: Hide list must still hide the list after the phone layout. Got ${desktopSidebar}`);

    await page.close();
    resetDocumentsFixture();
    const parserPage = await browser.newPage({ viewport: { width: 1280, height: 900 } });
    trackPage(parserPage, consoleErrors, pageErrors, failedRequests, errorResponses);
    try {
      await parserPage.goto(`http://127.0.0.1:${port}/apps/documents/?home_origin=${encodeURIComponent("https://home.example")}#home_token=documents-layout-token`);
      await parserPage.locator(".document-list-item").first().waitFor();
      await parserPage.locator('.document-list-item[data-doc-did="doc-beta"]').click();
      await parserPage.waitForFunction(() => document.getElementById("title-input")?.value === "Beta");
      const parserCases = [
        { label: "h1-open", source: "# ", predicate: () => (document.getElementById("preview")?.textContent || "").includes("# "), check: (preview) => preview.text.includes("# ") && preview.outlineCount === 0 },
        { label: "h2-open", source: "## ", predicate: () => (document.getElementById("preview")?.textContent || "").includes("## "), check: (preview) => preview.text.includes("## ") && preview.outlineCount === 0 },
        { label: "h3-open", source: "### ", predicate: () => (document.getElementById("preview")?.textContent || "").includes("### "), check: (preview) => preview.text.includes("### ") && preview.outlineCount === 0 },
        { label: "h4-open", source: "#### ", predicate: () => (document.getElementById("preview")?.textContent || "").includes("#### "), check: (preview) => preview.text.includes("#### ") && preview.outlineCount === 0 },
        { label: "h5-open", source: "##### ", predicate: () => (document.getElementById("preview")?.textContent || "").includes("##### "), check: (preview) => preview.text.includes("##### ") && preview.outlineCount === 0 },
        { label: "h6-open", source: "###### ", predicate: () => (document.getElementById("preview")?.textContent || "").includes("###### "), check: (preview) => preview.text.includes("###### ") && preview.outlineCount === 0 },
        { label: "unordered-open", source: "- ", predicate: () => (document.getElementById("preview")?.innerHTML || "").includes("<ul><li></li></ul>"), check: (preview) => preview.html.includes("<ul><li></li></ul>") },
        { label: "ordered-open", source: "1. ", predicate: () => (document.getElementById("preview")?.innerHTML || "").includes("<ol><li></li></ol>"), check: (preview) => preview.html.includes("<ol><li></li></ol>") },
        { label: "task-open", source: "- [ ]", predicate: () => (document.getElementById("preview")?.textContent || "").includes("[ ]"), check: (preview) => preview.text.includes("[ ]") },
        { label: "fence-open", source: "```", predicate: () => (document.getElementById("preview")?.innerHTML || "").includes("<pre><code"), check: (preview) => preview.html.includes("<pre><code") },
        { label: "table-open", source: "| head |\n| ", predicate: () => (document.getElementById("preview")?.textContent || "").includes("| head |"), check: (preview) => preview.text.includes("| head |") },
      ];
      for (const item of parserCases) {
        const preview = await fillMarkdownCase(parserPage, item.source, item.predicate, item.label);
        assert(item.check(preview), `Documents parser did not preserve ${item.label}. Got ${JSON.stringify({ item, preview })}`);
      }

      const hostileText = '<img src=x onerror="globalThis.markdownInjection=1"><script>globalThis.markdownInjection=1</script>';
      const hostileMarkdown = [
        ["fence-language", '```js" onmouseover="globalThis.markdownInjection=1">' + hostileText + "\nbody\n```"],
        ["fence-body", "```js\n" + hostileText + "\n```"],
        ["paragraph", hostileText],
        ["heading", "# " + hostileText],
        ["quote", "> " + hostileText],
        ["unordered-item", "- " + hostileText],
        ["ordered-item", "1. " + hostileText],
        ["task-label", "- [ ] " + hostileText],
        ["table-heading", "| " + hostileText + " |\n| --- |\n| safe |"],
        ["table-cell", "| safe |\n| --- |\n| " + hostileText + " |"],
        ["inline-code", "`" + hostileText + "`"],
        ["strong", "**" + hostileText + "**"],
        ["emphasis", "*" + hostileText + "*"],
        ["link-label", "[" + hostileText + "](https://example.test/)"],
        ["image-label", "![" + hostileText + "](https://example.test/image.png)"],
        ["link-url", '[label](https://example.test/" onmouseover="globalThis.markdownInjection=1)' ],
        ["image-url", '![label](https://example.test/" onerror="globalThis.markdownInjection=1)' ],
        ["link-scheme", "[label](javascript:globalThis.markdownInjection=1)"],
        ["image-scheme", "![label](data:text/html,<script>globalThis.markdownInjection=1</script>)"],
      ];
      for (const [field, markdown] of hostileMarkdown) {
        const marker = "markdown-safety-" + field;
        await parserPage.locator("#editor").fill(markdown + "\n\n" + marker);
        await parserPage.waitForFunction((value) => document.getElementById("preview")?.textContent.includes(value), marker);
        const result = await parserPage.evaluate(() => {
          const preview = document.getElementById("preview");
          const nodes = [...preview.querySelectorAll("*")];
          return {
            injectedElements: preview.querySelectorAll("script, img, iframe, svg, object, embed").length,
            eventAttributes: nodes.flatMap((node) => [...node.attributes].filter((attribute) => /^on/i.test(attribute.name)).map((attribute) => attribute.name)),
            unsafeUrls: [...preview.querySelectorAll("a")].map((link) => link.getAttribute("href")).filter((href) => /^(javascript|data|vbscript):/i.test(href)),
            executed: globalThis.markdownInjection === 1,
          };
        });
        assert(result.injectedElements === 0 && result.eventAttributes.length === 0 && result.unsafeUrls.length === 0 && !result.executed,
          `Documents ${field} created executable DOM: ${JSON.stringify(result)}`);
      }

      await parserPage.locator("#editor").fill("# ");
      await parserPage.waitForFunction(() => (document.getElementById("preview")?.textContent || "").includes("# "));

      const incrementalBody = "# Typed heading\n\nBody after incremental edit.";
      // Autosave stays quiet: the line under the title shows errors only.
      await parserPage.evaluate(() => {
        window.__statusShown = [];
        const row = document.getElementById("status-row");
        new MutationObserver(() => {
          if (!row.classList.contains("hidden")) {
            window.__statusShown.push(row.textContent.trim());
          }
        }).observe(row, { attributes: true, childList: true, subtree: true, characterData: true });
      });
      await parserPage.locator("#editor").fill(incrementalBody);
      await parserPage.waitForFunction(() => document.querySelector("#preview h1")?.textContent === "Typed heading");
      await waitForCondition(
        () => saveCalls.some((call) => call.docDid === "doc-beta" && call.body === incrementalBody),
        4000,
        `Documents incremental autosave did not persist the final markdown. Got ${JSON.stringify(saveCalls)}`,
      );
      assert(
        documents.get("doc-beta")?.body === incrementalBody,
        `Documents incremental autosave must preserve the final markdown body. Got ${JSON.stringify(documents.get("doc-beta"))}`,
      );
      await parserPage.waitForFunction(() => document.getElementById("save-button")?.disabled === true);
      const statusShown = await parserPage.evaluate(() => window.__statusShown);
      assert(statusShown.length === 0, `Documents typing and autosave must not flash a status line. Got ${JSON.stringify(statusShown)}`);
    } finally {
      await parserPage.close();
    }

    // Plain .md and .txt files in the Documents folder list beside Documents'
    // own, by the same sort and search, and open as Library files.
    resetDocumentsFixture();
    const filesPage = await browser.newPage({ viewport: { width: 1280, height: 900 } });
    trackPage(filesPage, consoleErrors, pageErrors, failedRequests, errorResponses);
    try {
      await filesPage.goto(`http://127.0.0.1:${port}/apps/documents/?home_origin=${encodeURIComponent("https://home.example")}#home_token=documents-layout-token`);
      const fileItem = filesPage.locator(`.document-list-item[data-object-uri="${folderFile.uri}"]`);
      await fileItem.waitFor();
      const listed = await filesPage.evaluate(() => [...document.querySelectorAll(".document-list-item")]
        .map((item) => item.querySelector(".document-list-title")?.textContent || ""));
      assert(JSON.stringify(listed) === JSON.stringify(["Alpha", "Beta", "Plain.txt"]),
        `Documents list must show the folder's .txt file, sorted with the rest by time. Got ${JSON.stringify(listed)}`);
      await filesPage.locator("#sidebar-search").fill("notes/plain");
      const searched = await filesPage.evaluate(() => [...document.querySelectorAll(".document-list-item")]
        .map((item) => item.dataset.objectUri || item.dataset.docDid));
      assert(JSON.stringify(searched) === JSON.stringify([folderFile.uri]),
        `Documents search must filter folder files by name and path. Got ${JSON.stringify(searched)}`);
      await filesPage.locator("#sidebar-search").fill("");
      await fileItem.click();
      await filesPage.waitForFunction((body) => document.getElementById("editor")?.value === body, folderFileBody);
      const opened = await filesPage.evaluate(() => ({
        title: document.getElementById("title-input")?.value || "",
        active: document.querySelector(".document-list-item.active")?.dataset.objectUri || "",
      }));
      assert(opened.title === folderFile.name && opened.active === folderFile.uri,
        `Documents must open a folder file from the list as the Library file. Got ${JSON.stringify(opened)}`);
    } finally {
      await filesPage.close();
    }

    // #436 invariant inside Home: after every step the Markdown views and
    // the published file list hold no a[href], the frame address (hash
    // included) is still the launch address, and no page opened.
    const pageCount = () => browser.contexts().reduce((count, context) => count + context.pages().length, 0);
    const assertHomeFrameInvariant = async (frame, launch, step) => {
      const url = frame.url();
      const pages = pageCount();
      const hrefs = await frame.evaluate(() => [...document.querySelectorAll(
        "#preview a[href], #share-preview a[href], #share-doc-list a[href]")].map((anchor) => anchor.getAttribute("href")));
      assert(url === launch.url && pages === launch.pages && hrefs.length === 0,
        `Inside Home after "${step}": frame ${url} (launch ${launch.url}), pages ${pages} (launch ${launch.pages}), hrefs ${JSON.stringify(hrefs)}`);
    };
    const scrollsTo = async (frame, id, activate) => {
      const top = () => frame.evaluate((target) => document.getElementById(target).getBoundingClientRect().top, id);
      const before = await top();
      await activate();
      await frame.page().waitForTimeout(200);
      const after = await top();
      const viewportHeight = await frame.evaluate(() => window.innerHeight);
      assert(before > viewportHeight && after >= 0 && after < viewportHeight,
        `The link to #${id} did not scroll to its heading (${before} -> ${after}).`);
    };
    const filler = "\n\n" + Array.from({ length: 80 }, (_, index) => `Filler line ${index}`).join("\n\n");

    // Editor preview: web links (same origin included) ask Home with click,
    // Enter and Space (middle-click does nothing); "#" links scroll; other
    // links are text. Without the Home bridge a web link refuses with a
    // message. Sequence: render, follow a fragment, edit, switch document,
    // re-render, then a same-origin link.
    for (const withoutOrigin of [false, true]) {
      resetDocumentsFixture();
      const linkPage = await browser.newPage({ viewport: { width: 1280, height: 900 } });
      trackPage(linkPage, consoleErrors, pageErrors, failedRequests, errorResponses);
      try {
        await linkPage.goto(`http://127.0.0.1:${port}/home-fixture${withoutOrigin ? "?without_origin" : ""}`);
        const frame = await (await linkPage.locator('iframe[title="Documents"]').elementHandle()).contentFrame();
        await frame.locator('.document-list-item[data-doc-did="doc-beta"]').click();
        await frame.waitForFunction(() => document.getElementById("title-input")?.value === "Beta");
        const launch = { url: frame.url(), pages: pageCount() };
        const webLink = "https://example.invalid/documents-link?from=preview";
        // A same-origin page that loads: a native href would replace the
        // Documents frame, which then cannot answer Home's close request.
        const sameOriginLink = `${new URL(frame.url()).origin}/s/${publishedFixtureCid}/_share.json`;
        // Refused spellings ("\" reads as "/", tabs are dropped), relative,
        // elastos:// and localhost:// links are text inside Home.
        const textLinks = ["//example.invalid/a", "/\\example.invalid/b", "\\\\example.invalid/c", "/\t/example.invalid/d",
          "/apps/documents/", "./other", "../other", "elastos://bafyexample", "localhost://ElastOS/Documents/x"];
        const markdown = `[Example](${webLink}) [Same](${sameOriginLink}) [Section](#notes-1) `
          + textLinks.map((href, index) => `[Text ${index}](${href})`).join(" ") + filler + "\n\n# Notes\n\nEnd.";
        await frame.locator("#mode-split").click();
        await frame.locator("#editor").fill(markdown);
        await assertHomeFrameInvariant(frame, launch, "render");
        const controls = frame.locator("#preview a[data-open-external]");
        assert(JSON.stringify(await controls.evaluateAll((nodes) => nodes.map((node) => node.dataset.openExternal)))
          === JSON.stringify([webLink, sameOriginLink]),
          "Every preview web link inside Home, same origin included, must be a Home control.");
        for (let index = 0; index < textLinks.length; index += 1) {
          assert(await frame.getByText(`Text ${index}`, { exact: true }).evaluate((node) => !node.closest("a")),
            `Inside Home ${JSON.stringify(textLinks[index])} must be plain text.`);
          await frame.getByText(`Text ${index}`, { exact: true }).click();
        }
        const homeRequests = () => linkPage.evaluate(() => window.homeMessages
          .filter((message) => message?.type === "home:open-external"));
        const link = controls.first();
        await link.click({ button: "middle" });
        await link.click();
        await link.focus();
        await linkPage.keyboard.press("Enter");
        await linkPage.keyboard.press("Space");
        await linkPage.waitForTimeout(200);
        await assertHomeFrameInvariant(frame, launch, "web link");
        if (withoutOrigin) {
          assert(await frame.locator("#status-text").textContent() === "Open Documents from Home to open web links.",
            "Without the Home bridge a preview web link must say why it did not open.");
        }
        await scrollsTo(frame, "notes-1", () => frame.locator('#preview [data-scroll-to="notes-1"]').click());
        await assertHomeFrameInvariant(frame, launch, "follow fragment");
        await frame.locator("#editor").fill(markdown + "\n\nEdited.");
        await assertHomeFrameInvariant(frame, launch, "edit");
        await frame.locator('.document-list-item[data-doc-did="doc-alpha"]').click();
        await frame.locator("#confirm-action").click();
        await frame.waitForFunction(() => document.getElementById("title-input")?.value === "Alpha");
        await assertHomeFrameInvariant(frame, launch, "switch document");
        await frame.locator("#editor").fill(markdown);
        await assertHomeFrameInvariant(frame, launch, "re-render");
        await frame.getByText("Same", { exact: true }).click();
        await linkPage.waitForTimeout(300);
        await assertHomeFrameInvariant(frame, launch, "same-origin link");
        const request = (url) => ({ type: "home:open-external", url, homeToken: "documents-link-token" });
        const expected = withoutOrigin ? [] : [request(webLink), request(webLink), request(webLink), request(sameOriginLink)];
        const requests = await homeRequests();
        assert(JSON.stringify(requests) === JSON.stringify(expected),
          `Documents preview links (click, Enter, Space, same-origin; middle-click and text send nothing) must send ${expected.length} exact Home requests. Got ${JSON.stringify(requests)}`);
      } finally {
        await linkPage.close();
      }
    }

    // Published revision Home opens (not in the local list), with the
    // sequence found in review: follow a fragment, switch file, then a
    // same-origin link.
    resetDocumentsFixture();
    const publishedPage = await browser.newPage({ viewport: { width: 1280, height: 900 } });
    trackPage(publishedPage, consoleErrors, pageErrors, failedRequests, errorResponses);
    try {
      await publishedPage.goto(`http://127.0.0.1:${port}/home-fixture?cid=${publishedFixtureCid}`);
      const frame = await (await publishedPage.locator('iframe[title="Documents"]').elementHandle()).contentFrame();
      await frame.locator("#share-preview h1").first().waitFor({ state: "visible" });
      const origin = new URL(frame.url()).origin;
      const launch = { url: frame.url(), pages: pageCount() };
      await assertHomeFrameInvariant(frame, launch, "published render");
      const controls = frame.locator("#share-preview a[data-open-external]");
      assert(JSON.stringify(await controls.evaluateAll((nodes) => nodes.map((node) => node.dataset.openExternal)))
        === JSON.stringify([publishedWebLink, `${origin}/s/${publishedFixtureCid}/_share.json`]),
        "Published preview web links inside Home, same origin included, must be Home controls.");
      assert(await frame.getByText("Rel", { exact: true }).evaluate((node) => !node.closest("a")),
        "A relative link in the published preview inside Home must be plain text.");
      await controls.nth(0).click();
      await controls.nth(1).focus();
      await publishedPage.keyboard.press("Enter");
      await frame.getByText("Rel", { exact: true }).click();
      await publishedPage.waitForTimeout(200);
      await assertHomeFrameInvariant(frame, launch, "published web links");
      await scrollsTo(frame, "notes-2", () => frame.getByText("Jump", { exact: true }).click());
      await assertHomeFrameInvariant(frame, launch, "published follow fragment");
      await frame.locator(".share-doc-link").nth(1).click();
      await frame.waitForFunction(() => document.getElementById("share-title")?.textContent === "Second file");
      await assertHomeFrameInvariant(frame, launch, "switch published file");
      await frame.getByText("Same second", { exact: true }).click();
      await publishedPage.waitForTimeout(300);
      await assertHomeFrameInvariant(frame, launch, "published same-origin link");
      const requests = await publishedPage.evaluate(() => window.homeMessages
        .filter((message) => message?.type === "home:open-external"));
      const request = (url) => ({ type: "home:open-external", url, homeToken: "documents-link-token" });
      assert(JSON.stringify(requests) === JSON.stringify([request(publishedWebLink),
        request(`${origin}/s/${publishedFixtureCid}/_share.json`), request(`${origin}/s/${publishedFixtureCid}/_share.json`)]),
      `Published preview links must send exact Home requests. Got ${JSON.stringify(requests)}`);
    } finally {
      await publishedPage.close();
    }

    resetDocumentsFixture();
    const delayedSave = armDelayedSave();
    const racePage = await browser.newPage({ viewport: { width: 1280, height: 900 } });
    trackPage(racePage, consoleErrors, pageErrors, failedRequests, errorResponses);
    try {
      await racePage.goto(`http://127.0.0.1:${port}/apps/documents/?home_origin=${encodeURIComponent("https://home.example")}#home_token=documents-layout-token`);
      await racePage.locator(".document-list-item").first().waitFor();
      await racePage.locator('.document-list-item[data-doc-did="doc-alpha"]').click();
      await racePage.waitForFunction(() => document.getElementById("title-input")?.value === "Alpha");
      const alphaSavedBody = "# Alpha\n\nThis autosave belongs to Alpha only.";
      const betaSavedBody = "# Beta\n\nThis autosave belongs to Beta only.";
      await racePage.locator("#editor").fill(alphaSavedBody);
      await delayedSave.started;
      await racePage.locator('.document-list-item[data-doc-did="doc-beta"]').click();
      await racePage.locator("#confirm-modal.open").waitFor();
      await racePage.locator("#confirm-action").click();
      await racePage.waitForFunction(() => document.getElementById("title-input")?.value === "Beta");
      await racePage.locator("#editor").fill(betaSavedBody);
      await racePage.waitForTimeout(1200);
      delayedSave.release();
      await waitForCondition(
        () => saveCalls.length === 2,
        4000,
        `Documents queued autosave did not emit two serial saves. Got ${JSON.stringify(saveCalls)}`,
      );
      await racePage.waitForFunction(() => document.getElementById("title-input")?.value === "Beta");
      const raceState = await racePage.evaluate(() => ({
        currentTitle: document.getElementById("title-input")?.value || "",
        currentBody: document.getElementById("editor")?.value || "",
      }));
      assert(raceState.currentTitle === "Beta", `Delayed Alpha autosave must not replace the selected document. Got ${JSON.stringify(raceState)}`);
      assert(raceState.currentBody === betaSavedBody, `Delayed Alpha autosave must not contaminate Beta body. Got ${JSON.stringify(raceState)}`);
      assert(saveCalls.length === 2, `Documents must keep two serial saves in the queued autosave race. Got ${JSON.stringify(saveCalls)}`);
      assert(saveCalls[0].docDid === "doc-alpha", `Delayed autosave must keep Alpha identity. Got ${JSON.stringify(saveCalls)}`);
      assert(saveCalls[0].body === alphaSavedBody, `Delayed autosave must send only Alpha's saved body first. Got ${JSON.stringify(saveCalls)}`);
      assert(saveCalls[1].docDid === "doc-beta", `Queued autosave must keep Beta identity second. Got ${JSON.stringify(saveCalls)}`);
      assert(saveCalls[1].body === betaSavedBody, `Queued autosave must send only Beta's saved body second. Got ${JSON.stringify(saveCalls)}`);
      assert(maxConcurrentSaveCalls === 1, `Documents saves must remain serial with no overlap. Got ${JSON.stringify({ maxConcurrentSaveCalls, saveCalls })}`);
      const alphaStored = documents.get("doc-alpha");
      const betaStored = documents.get("doc-beta");
      assert(alphaStored?.body === alphaSavedBody, `Delayed autosave must persist only Alpha's body. Got ${JSON.stringify({ alphaStored, saveCalls })}`);
      assert(betaStored?.body === betaSavedBody, `Queued autosave must persist only Beta's body. Got ${JSON.stringify({ betaStored, saveCalls })}`);
    } finally {
      await racePage.close();
    }

    resetDocumentsFixture();
    const windows = await Promise.all([browser.newPage(), browser.newPage()]);
    try {
      for (const page of windows) {
        trackPage(page, consoleErrors, pageErrors, failedRequests, errorResponses);
        await page.goto(`http://127.0.0.1:${port}/apps/documents/?home_origin=${encodeURIComponent("https://home.example")}#home_token=documents-layout-token`);
        await page.locator('.document-list-item[data-doc-did="doc-alpha"]').click();
        await page.waitForFunction(() => document.getElementById("title-input")?.value === "Alpha");
      }
      const held = armDelayedSave();
      try {
        await windows[0].locator("#editor").fill("Keep losing draft");
        await windows[0].getByRole("button", { name: "Save", exact: true }).click();
        await waitForCondition(() => activeSaveCalls === 1 && saveCalls.length === 1, 5000, "First page Save did not reach the held request.");
        await windows[1].locator("#editor").fill("Winning body");
        await windows[1].getByRole("button", { name: "Save", exact: true }).click();
        await waitForCondition(() => documents.get("doc-alpha").body === "Winning body", 5000, "Second page Save did not persist its body.");
        await windows[1].waitForFunction(() => document.getElementById("save-button")?.disabled === true);
      } finally {
        held.release();
      }
      await windows[0].waitForFunction(() => document.getElementById("status-text")?.textContent.includes("changed since it was read"));
      assert(documents.get("doc-alpha").body === "Winning body", "Stale page overwrote the winner.");
      assert(await windows[0].locator("#editor").inputValue() === "Keep losing draft", "Conflict erased the losing page draft.");
      assert(await windows[0].getByRole("button", { name: "Save", exact: true }).isEnabled(), "Conflicted draft must remain dirty and explicitly recoverable.");
      await windows[0].locator("#editor").fill("Newer losing draft");
      // Observe beyond the actual 900 ms autosave interval after another edit.
      await windows[0].waitForTimeout(1200);
      assert(saveCalls.length === 2, "Conflict must pause automatic writes after further edits.");
      assert(documents.get("doc-alpha").body === "Winning body", "Paused draft changed the stored winner.");
      assert(await windows[0].locator("#editor").inputValue() === "Newer losing draft", "Paused autosave erased newer edits.");
      assert(saveCalls.every((call) => /^[0-9a-f]{64}$/.test(call.if_revision)), "Every page save must carry a revision.");
    } finally {
      await Promise.all(windows.map((page) => page.close()));
    }

    assert(failedRequests.length === 0, `Documents layout emitted failed requests: ${JSON.stringify(failedRequests)}`);
    assert(consoleErrors.length === 0, `Documents layout emitted console errors: ${JSON.stringify({ consoleErrors, failedRequests, errorResponses })}`);
    assert(pageErrors.length === 0, `Documents layout emitted page errors: ${JSON.stringify(pageErrors)}`);
    console.log("documents-product-layout-smoke: OK");
  } finally {
    delayedSaveRelease?.resolve();
    try {
      await browser?.close();
    } finally {
      server.closeAllConnections();
      await new Promise((resolve) => server.close(resolve));
    }
  }
}

run().catch((error) => {
  console.error(error?.stack || String(error));
  process.exit(1);
});
