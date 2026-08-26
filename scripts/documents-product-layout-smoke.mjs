#!/usr/bin/env node
import { createReadStream } from "node:fs";
import { stat } from "node:fs/promises";
import { createServer } from "node:http";
import { createRequire } from "node:module";
import path from "node:path";

const brave = process.env.BRAVE_BIN || "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser";
const require = createRequire(new URL("../elastos/tools/browser-playwright-engine/package.json", import.meta.url));
const { chromium } = require("playwright");

const capsuleRoot = path.resolve("capsules/documents/browser");
const homeClipboardClientPath = path.resolve("capsules/home/browser/home-clipboard-client.js");
const homeClipboardProtocolPath = path.resolve("capsules/home/browser/home-clipboard-protocol.js");

const documents = new Map();
const saveCalls = [];
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
  return {
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
  };
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

function contentType(filePath) {
  if (filePath.endsWith(".html")) return "text/html; charset=utf-8";
  if (filePath.endsWith(".css")) return "text/css; charset=utf-8";
  if (filePath.endsWith(".js")) return "text/javascript; charset=utf-8";
  if (filePath.endsWith(".woff2")) return "font/woff2";
  return "application/octet-stream";
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
          json(res, { documents: listSummary() });
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
          const current = documents.get(payload.doc_did);
          saveCalls.push({
            docDid: payload.doc_did,
            title: payload.title || current?.title || "",
            body: payload.body || "",
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
            const updated = {
              ...current,
              title: payload.title || current.title,
              body: payload.body || "",
              updated_at: current.updated_at + 1,
            };
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
          const duplicate = {
            ...current,
            doc_did: docDid,
            title: payload.title || `${current.title} copy`,
            file_name: payload.file_name || `${current.file_name.replace(/\.md$/i, "")}-copy.md`,
            body: payload.body || current.body,
            latest_published_cid: null,
            publish_history: [],
          };
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
  const browser = await chromium.launch({ headless: true, executablePath: brave });
  try {
    const page = await browser.newPage({ viewport: { width: 1280, height: 900 } });
    const consoleErrors = [];
    const pageErrors = [];
    const failedRequests = [];
    const errorResponses = [];
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
      copyHidden: document.getElementById("copy-published-link")?.classList.contains("hidden") || false,
      copyText: document.getElementById("copy-published-link")?.textContent?.trim() || "",
      titleValue: document.getElementById("title-input")?.value || "",
    }));
    assert(!selected.copyHidden, "Published documents must expose Copy Published Link.");
    assert(selected.copyText === "", "Copy Published Link must stay icon-only.");
    assert(selected.titleValue === "Alpha", "Documents layout fixture must load the selected document.");

    await page.locator(".document-list-item").first().click({ button: "right" });
    await page.locator("#context-menu:not(.hidden)").waitFor();
    const contextItems = await page.locator("#context-menu .context-menu-item").evaluateAll((nodes) =>
      nodes.map((node) => node.textContent?.trim() || ""),
    );
    assert(
      JSON.stringify(contextItems) === JSON.stringify(["Duplicate", "Unpublish", "Copy Published Link", "Delete"]),
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
    await page.setViewportSize({ width: 390, height: 844 });
    await assertNoHorizontalOverflow(page, "narrow");

    await page.close();
    resetDocumentsFixture();
    const delayedSave = armDelayedSave();
    const racePage = await browser.newPage({ viewport: { width: 1280, height: 900 } });
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

    assert(failedRequests.length === 0, `Documents layout emitted failed requests: ${JSON.stringify(failedRequests)}`);
    assert(consoleErrors.length === 0, `Documents layout emitted console errors: ${JSON.stringify({ consoleErrors, failedRequests, errorResponses })}`);
    assert(pageErrors.length === 0, `Documents layout emitted page errors: ${JSON.stringify(pageErrors)}`);
    console.log("documents-product-layout-smoke: OK");
  } finally {
    await browser.close();
    await new Promise((resolve) => server.close(resolve));
  }
}

run().catch((error) => {
  console.error(error?.stack || String(error));
  process.exit(1);
});
