#!/usr/bin/env node

import { createServer } from "node:http";
import { readFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const repoRoot = fileURLToPath(new URL("../", import.meta.url));
const homeRoot = join(repoRoot, "capsules/home/browser");
const fixtureTargets = Object.freeze({
  browser: "headless-browser-clipboard-token",
  library: "headless-library-clipboard-token",
  wallet: "headless-wallet-clipboard-token",
});

function assert(condition, message, details = undefined) {
  if (condition) {
    return;
  }
  const suffix = details === undefined ? "" : `\n${JSON.stringify(details, null, 2)}`;
  throw new Error(`${message}${suffix}`);
}

function send(res, status, contentType, body) {
  const bytes = Buffer.from(body);
  res.writeHead(status, {
    "access-control-allow-origin": "*",
    "cache-control": "no-store",
    "content-length": bytes.length,
    "content-type": contentType,
  });
  res.end(bytes);
}

function topDocument(homeOrigin) {
  const frames = Object.entries(fixtureTargets)
    .map(([targetId, homeToken]) => `
      <iframe
        id="${targetId}-frame"
        title="Opaque ${targetId} Clipboard fixture"
        sandbox="allow-scripts"
        src="/child?target=${targetId}&home_origin=${encodeURIComponent(homeOrigin)}#home_token=${homeToken}"
      ></iframe>`)
    .join("");
  return `<!doctype html>
<html lang="en">
  <head><meta charset="utf-8"><title>Home Clipboard host fixture</title></head>
  <body>
    ${frames}
    <section
      id="home-clipboard-prompt"
      role="dialog"
      aria-modal="true"
      aria-labelledby="home-clipboard-title"
      aria-describedby="home-clipboard-copy"
      aria-hidden="true"
      hidden
    >
      <h1 id="home-clipboard-title">Clipboard request</h1>
      <p id="home-clipboard-copy"></p>
      <button id="home-clipboard-allow" type="button">Continue</button>
      <button id="home-clipboard-cancel" type="button">Cancel</button>
    </section>
    <script type="module">
      import {
        createHomeClipboardFrameState,
        createHomeClipboardHost,
        createHomeClipboardPrompt,
      } from "/home-clipboard-host.js";

      const targetConfig = new Map(
        ${JSON.stringify(Object.entries(fixtureTargets))}.map(([targetId, homeToken]) => [
          homeToken,
          {
            targetId,
            homeToken,
            frame: document.querySelector(\`#\${targetId}-frame\`),
            state: createHomeClipboardFrameState(),
          },
        ]),
      );
      const effects = { reads: 0, writes: 0, written: [] };
      const prompt = createHomeClipboardPrompt({
        root: document.querySelector("#home-clipboard-prompt"),
        title: document.querySelector("#home-clipboard-title"),
        copy: document.querySelector("#home-clipboard-copy"),
        allowButton: document.querySelector("#home-clipboard-allow"),
        cancelButton: document.querySelector("#home-clipboard-cancel"),
      });
      const host = createHomeClipboardHost({
        clipboard: {
          async readText() {
            effects.reads += 1;
            return navigator.clipboard.readText();
          },
          async writeText(text) {
            effects.writes += 1;
            effects.written.push(text);
            return navigator.clipboard.writeText(text);
          },
        },
        prompt,
        timeoutMs: 150,
      });
      window.__clipboardFixture = { effects, host, targetConfig };
      window.addEventListener("message", (event) => {
        const data = event.data;
        const config = targetConfig.get(data?.homeToken || "");
        if (!config || event.source !== config.frame.contentWindow || event.origin !== "null") {
          return;
        }
        const context = {
          kind: "app-frame",
          targetId: config.targetId,
          homeToken: config.homeToken,
          origin: "null",
          parentOrigin: location.origin,
          source: config.frame.contentWindow,
          clipboardState: config.state,
        };
        if (
          data.type === "home:app-ready" &&
          Object.keys(data).sort().join(",") === "homeToken,type"
        ) {
          host.resetFrame(config.state, context);
          return;
        }
        host.handle(event, context, data);
      });
    </script>
  </body>
</html>`;
}

function childDocument() {
  return `<!doctype html>
<html lang="en">
  <head><meta charset="utf-8"><title>Opaque Clipboard fixture</title></head>
  <body>
    <output id="result"></output>
    <script type="module">
      import {
        createHomeClipboardClient,
      } from "/apps/home/home-clipboard-client.js";

      const query = new URL(location.href).searchParams;
      const targetId = query.get("target") || "";
      const homeOrigin = query.get("home_origin") || "";
      const homeToken = new URLSearchParams(location.hash.replace(/^#/, "")).get("home_token") || "";
      const result = document.querySelector("#result");
      let lastReady = null;
      let lastHostResult = null;
      window.addEventListener("message", (event) => {
        if (event.source !== window.top || event.origin !== homeOrigin) return;
        if (event.data?.type === "home:clipboard-ready") lastReady = event.data;
        if (event.data?.type === "home:clipboard-result") lastHostResult = event.data;
      });
      const client = createHomeClipboardClient({
        targetId,
        homeOrigin,
        homeToken,
      });
      client.start();

      async function capture(promise) {
        try {
          const value = await promise;
          result.dataset.status = "ok";
          result.textContent = typeof value === "string" ? value : "ok";
          return { ok: true, value };
        } catch (error) {
          result.dataset.status = error?.code || "failed";
          result.textContent = error?.code || String(error);
          return { ok: false, error: error?.code || String(error) };
        }
      }

      function forgedRequest({
        requestId,
        operation = "write",
        purpose,
        text,
        extra = {},
      }) {
        lastHostResult = null;
        const message = {
          type: "home:clipboard-request",
          schema: "elastos.home.clipboard.request/v1",
          requestId,
          homeToken,
          parentOrigin: homeOrigin,
          generation: lastReady?.generation || "",
          operation,
          purpose,
          mime_type: "text/plain",
          ...extra,
        };
        if (operation === "write") message.text = text;
        window.top.postMessage(message, homeOrigin);
      }

      window.__clipboardChild = {
        address: (requestId = undefined) =>
          capture(client.writeText("0x1234567890abcdef", {
            purpose: "wallet.address",
            requestId,
          })),
        browserRead: () => capture(client.readText()),
        browserWrite: (requestId = undefined) =>
          capture(client.writeText("headless Browser copy", { requestId })),
        cancel() {
          const requestId = client.newRequestId();
          const pending = capture(client.writeText("0xcancel", {
            purpose: "wallet.address",
            requestId,
          }));
          client.cancel(requestId);
          return pending;
        },
        forge: forgedRequest,
        lastHostResult: () => lastHostResult,
        libraryIdentifier: () =>
          capture(client.writeText(
            "bafkreihdwdcefgh4dqkjv67uzcmw7ojee6xedzdetojuzjevtenxquvyku",
            { purpose: "resource.identifier" },
          )),
        libraryUri: () =>
          capture(client.writeText(
            "elastos://bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi",
            { purpose: "resource.uri" },
          )),
        ready: () => lastReady,
        recovery: () =>
          capture(client.writeText(
            JSON.stringify({
              schema: "elastos.wallet.recovery-key/v1",
              private_key_hex: "headless-secret-do-not-render",
            }),
            { purpose: "wallet.recovery-key" },
          )),
        replay: (requestId) =>
          capture(client.writeText("headless Browser copy", { requestId })),
        teardown() {
          client.teardown();
        },
        timedWrite: () => capture(client.writeText("time me out")),
      };
      document.body.dataset.clientReady = "true";
    </script>
  </body>
</html>`;
}

const server = createServer((req, res) => {
  try {
    const url = new URL(req.url || "/", "http://127.0.0.1");
    if (url.pathname === "/") {
      send(
        res,
        200,
        "text/html; charset=utf-8",
        topDocument(`http://${req.headers.host}`),
      );
      return;
    }
    if (url.pathname === "/child") {
      send(res, 200, "text/html; charset=utf-8", childDocument());
      return;
    }
    if (url.pathname === "/home-clipboard-host.js") {
      send(
        res,
        200,
        "text/javascript; charset=utf-8",
        readFileSync(join(homeRoot, "home-clipboard-host.js")),
      );
      return;
    }
    if (
      url.pathname === "/home-clipboard-protocol.js" ||
      url.pathname === "/apps/home/home-clipboard-protocol.js"
    ) {
      send(
        res,
        200,
        "text/javascript; charset=utf-8",
        readFileSync(join(homeRoot, "home-clipboard-protocol.js")),
      );
      return;
    }
    if (url.pathname === "/apps/home/home-clipboard-client.js") {
      send(
        res,
        200,
        "text/javascript; charset=utf-8",
        readFileSync(join(homeRoot, "home-clipboard-client.js")),
      );
      return;
    }
    send(res, 404, "text/plain; charset=utf-8", "not found");
  } catch (error) {
    send(res, 500, "text/plain; charset=utf-8", String(error?.stack || error));
  }
});

async function listen() {
  await new Promise((resolveListen, rejectListen) => {
    server.once("error", rejectListen);
    server.listen(0, "127.0.0.1", resolveListen);
  });
  const address = server.address();
  assert(address && typeof address === "object", "Clipboard fixture did not bind");
  return `http://127.0.0.1:${address.port}`;
}

function playwrightSpecifier() {
  const configured = process.env.ELASTOS_PLAYWRIGHT_MODULE || "";
  if (configured) {
    return configured.startsWith("file:")
      ? configured
      : pathToFileURL(resolve(configured)).href;
  }
  return pathToFileURL(
    join(
      repoRoot,
      "elastos/tools/browser-playwright-engine/node_modules/playwright/index.js",
    ),
  ).href;
}

async function waitForHostResult(frame, predicate) {
  await frame.waitForFunction(
    (serializedPredicate) => {
      const result = window.__clipboardChild.lastHostResult();
      if (!result) return false;
      const expected = JSON.parse(serializedPredicate);
      return Object.entries(expected).every(([key, value]) => result[key] === value);
    },
    JSON.stringify(predicate),
  );
  return frame.evaluate(() => window.__clipboardChild.lastHostResult());
}

const origin = await listen();
let browser = null;
try {
  const imported = await import(playwrightSpecifier());
  const { chromium } = imported.default || imported;
  browser = await chromium.launch({
    headless: true,
    args: [
      "--disable-background-networking",
      "--disable-breakpad",
      "--disable-component-update",
      "--disable-domain-reliability",
      "--disable-extensions",
      "--disable-sync",
      "--no-first-run",
      "--no-proxy-server",
    ],
  });
  const context = await browser.newContext();
  await context.grantPermissions(["clipboard-read", "clipboard-write"], { origin });
  const page = await context.newPage();
  const pageErrors = [];
  const consoleErrors = [];
  const failedRequests = [];
  page.on("pageerror", (error) => pageErrors.push(String(error?.stack || error)));
  page.on("console", (message) => {
    if (message.type() === "error") consoleErrors.push(message.text());
  });
  page.on("requestfailed", (request) => {
    failedRequests.push({
      url: request.url(),
      error: request.failure()?.errorText || "",
    });
  });
  await page.goto(origin, { waitUntil: "networkidle" });

  const browserFrame = page.frames().find((frame) =>
    frame.url().includes("target=browser"));
  const libraryFrame = page.frames().find((frame) =>
    frame.url().includes("target=library"));
  const walletFrame = page.frames().find((frame) =>
    frame.url().includes("target=wallet"));
  assert(
    browserFrame && libraryFrame && walletFrame,
    "opaque Clipboard fixture frames are missing",
  );
  for (const frame of [browserFrame, libraryFrame, walletFrame]) {
    await frame.waitForFunction(
      () => document.body.dataset.clientReady === "true" &&
        window.__clipboardChild.ready(),
    );
    assert(
      (await frame.evaluate(() => self.origin)) === "null",
      "Clipboard fixture must have an opaque origin",
    );
  }
  assert(pageErrors.length === 0, "Clipboard fixture module failed", {
    pageErrors,
    consoleErrors,
    failedRequests,
  });

  const browserWrite = browserFrame.evaluate(
    () => window.__clipboardChild.browserWrite("browser-write:1"),
  );
  await page.locator("#home-clipboard-prompt").waitFor({ state: "visible" });
  assert(
    (await page.evaluate(() => window.__clipboardFixture.effects.writes)) === 0,
    "Home wrote Clipboard text before its visible user action",
  );
  await page.locator("#home-clipboard-allow").click();
  assert((await browserWrite).ok, "Browser write did not complete");
  assert(
    (await page.evaluate(() => navigator.clipboard.readText())) ===
      "headless Browser copy",
    "Home did not write the exact Browser text",
  );

  await page.evaluate(() => navigator.clipboard.writeText("headless host paste"));
  const browserRead = browserFrame.evaluate(
    () => window.__clipboardChild.browserRead(),
  );
  await page.locator("#home-clipboard-prompt").waitFor({ state: "visible" });
  assert(
    (await page.evaluate(() => window.__clipboardFixture.effects.reads)) === 0,
    "Home read Clipboard text before its visible user action",
  );
  await page.locator("#home-clipboard-allow").click();
  assert(
    (await browserRead).value === "headless host paste",
    "Browser did not receive the exact Home Clipboard text",
  );

  assert(
    !page.isClosed() && !walletFrame.isDetached(),
    "Wallet Clipboard fixture detached during Browser proof",
    {
      pageClosed: page.isClosed(),
      walletDetached: walletFrame.isDetached(),
      pageErrors,
      consoleErrors,
      failedRequests,
    },
  );
  const addressWrite = walletFrame
    .evaluate(() => window.__clipboardChild.address())
    .catch((error) => ({ playwrightError: String(error?.stack || error) }));
  const addressStart = await Promise.race([
    addressWrite.then((result) => ({ result })),
    page.locator("#home-clipboard-prompt")
      .waitFor({ state: "visible" })
      .then(() => ({ prompt: true })),
  ]);
  assert(addressStart.prompt === true, "Wallet address request did not reach Home", {
    addressStart,
    pageErrors,
    consoleErrors,
    failedRequests,
  });
  assert(
    /Wallet address/.test(
      await page.locator("#home-clipboard-title").textContent(),
    ),
    "Wallet address prompt was not purpose-specific",
  );
  await page.locator("#home-clipboard-allow").click();
  assert((await addressWrite).ok, "Wallet address write did not complete");

  const recoveryWrite = walletFrame.evaluate(
    () => window.__clipboardChild.recovery(),
  );
  await page.locator("#home-clipboard-prompt").waitFor({ state: "visible" });
  const recoveryPrompt = [
    await page.locator("#home-clipboard-title").textContent(),
    await page.locator("#home-clipboard-copy").textContent(),
  ].join(" ");
  assert(
    /secret material/i.test(recoveryPrompt),
    "Recovery prompt did not classify secret material",
  );
  assert(
    !recoveryPrompt.includes("headless-secret-do-not-render"),
    "Recovery prompt exposed secret material",
  );
  await page.locator("#home-clipboard-allow").click();
  assert((await recoveryWrite).ok, "Recovery Key write did not complete");

  const identifierWrite = libraryFrame.evaluate(
    () => window.__clipboardChild.libraryIdentifier(),
  );
  await page.locator("#home-clipboard-prompt").waitFor({ state: "visible" });
  assert(
    /Library identifier/.test(
      await page.locator("#home-clipboard-title").textContent(),
    ),
    "Library identifier did not receive its explicit Home prompt",
  );
  await page.locator("#home-clipboard-allow").click();
  assert((await identifierWrite).ok, "Library identifier write did not complete");

  const resourceUriWrite = libraryFrame.evaluate(
    () => window.__clipboardChild.libraryUri(),
  );
  await page.locator("#home-clipboard-prompt").waitFor({ state: "visible" });
  assert(
    /Library resource link/.test(
      await page.locator("#home-clipboard-title").textContent(),
    ),
    "Library resource URI did not receive its explicit Home prompt",
  );
  await page.locator("#home-clipboard-allow").click();
  assert((await resourceUriWrite).ok, "Library resource URI write did not complete");

  const cancelled = walletFrame.evaluate(() => window.__clipboardChild.cancel());
  const cancelledResult = await cancelled;
  assert(cancelledResult.error === "cancelled", "client cancellation failed", {
    cancelledResult,
  });
  assert(
    (await page.evaluate(() => window.__clipboardFixture.effects.writes)) === 5,
    "cancelled request reached the OS Clipboard",
  );

  const timeout = browserFrame.evaluate(
    () => window.__clipboardChild.timedWrite(),
  );
  await page.locator("#home-clipboard-prompt").waitFor({ state: "visible" });
  assert(
    (await timeout).error === "timeout",
    "host timeout did not fail closed",
  );
  await page.locator("#home-clipboard-prompt").waitFor({ state: "hidden" });

  const replay = browserFrame.evaluate(
    () => window.__clipboardChild.replay("browser-write:1"),
  );
  assert(
    (await replay).error === "replay",
    "request replay was not rejected",
  );

  await browserFrame.evaluate(() => window.__clipboardChild.forge({
    requestId: "substitution:1",
    purpose: "wallet.address",
    text: "0xsubstituted",
  }));
  assert(
    (await waitForHostResult(browserFrame, {
      requestId: "substitution:1",
      error: "malformed",
    })).targetId === "browser",
    "Home did not derive the substituted request target from frame context",
  );

  await browserFrame.evaluate(() => window.__clipboardChild.forge({
    requestId: "target-field:1",
    purpose: "browser.text",
    text: "extra target",
    extra: { targetId: "wallet" },
  }));
  await waitForHostResult(browserFrame, {
    requestId: "target-field:1",
    error: "malformed",
  });

  await browserFrame.evaluate(() => window.__clipboardChild.forge({
    requestId: "inherited-purpose:1",
    purpose: "__proto__",
    text: "must not copy",
  }));
  assert(
    (await waitForHostResult(browserFrame, {
      requestId: "inherited-purpose:1",
      error: "malformed",
    })).purpose === "invalid",
    "Inherited purpose was not rejected with a bounded result",
  );

  await browserFrame.evaluate(() => window.__clipboardChild.forge({
    requestId: "oversized-purpose:1",
    purpose: "p".repeat(65_537),
    text: "must not copy",
  }));
  assert(
    (await waitForHostResult(browserFrame, {
      requestId: "oversized-purpose:1",
      error: "malformed",
    })).purpose === "invalid",
    "Oversized purpose was echoed or accepted",
  );

  await browserFrame.evaluate(() => window.__clipboardChild.forge({
    requestId: "oversize:1",
    purpose: "browser.text",
    text: "a".repeat(65_537),
  }));
  await waitForHostResult(browserFrame, {
    requestId: "oversize:1",
    error: "malformed",
  });

  const teardown = walletFrame.evaluate(
    () => window.__clipboardChild.address("teardown:1"),
  );
  await page.locator("#home-clipboard-prompt").waitFor({ state: "visible" });
  await walletFrame.evaluate(() => window.__clipboardChild.teardown());
  assert(
    (await teardown).error === "retired",
    "frame teardown did not retire request",
  );
  await page.locator("#home-clipboard-prompt").waitFor({ state: "hidden" });
  const walletRetired = await page.evaluate(() =>
    window.__clipboardFixture.targetConfig
      .get("headless-wallet-clipboard-token")
      .state.retired,
  );
  assert(walletRetired, "Home did not retire the Wallet Clipboard lifecycle");

  const effects = await page.evaluate(() => window.__clipboardFixture.effects);
  assert(
    effects.reads === 1 && effects.writes === 5,
    "Home Clipboard edge performed an unexpected OS effect count",
    effects,
  );
  process.stdout.write(
    `${JSON.stringify({
      schema: "elastos.home.clipboard-headless-smoke/v1",
      ok: true,
      opaque_frames: 3,
      browser_read: true,
      browser_write: true,
      wallet_address_write: true,
      recovery_key_write: true,
      library_identifier_write: true,
      library_resource_uri_write: true,
      recovery_prompt_secret_safe: true,
      cancellation: true,
      timeout: true,
      replay: true,
      teardown: true,
      target_substitution: true,
      inherited_purpose: true,
      oversized_purpose: true,
      oversize: true,
      clipboard_reads: effects.reads,
      clipboard_writes: effects.writes,
    })}\n`,
  );
} finally {
  await browser?.close().catch(() => {});
  await new Promise((resolveClose) => server.close(resolveClose));
}
