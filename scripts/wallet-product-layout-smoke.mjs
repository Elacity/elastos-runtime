#!/usr/bin/env node

import { createServer } from "node:http";
import { createRequire } from "node:module";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { extname, join, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = resolve(fileURLToPath(new URL("../", import.meta.url)));
const walletRoot = join(repoRoot, "capsules/wallet/browser");
const homeRoot = join(repoRoot, "capsules/home/browser");
const brave = process.env.BRAVE_BIN || "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser";
const require = createRequire(new URL("../elastos/tools/browser-playwright-engine/package.json", import.meta.url));
const { chromium } = require("playwright");

const groupAddress = "0x1111111111111111111111111111111111111111";
const sendAddress = "0x2222222222222222222222222222222222222222";
const clipboardGeneration = "wallet-clipboard-generation-1";
const defaultGenericError = "Wallet action could not be completed.";

function assert(condition, message, details = undefined) {
  if (condition) {
    return;
  }
  const suffix = details === undefined ? "" : `\n${JSON.stringify(details, null, 2)}`;
  throw new Error(`${message}${suffix}`);
}

function json(response, status, value) {
  const body = Buffer.from(JSON.stringify(value));
  response.writeHead(status, {
    "access-control-allow-origin": "*",
    "access-control-allow-headers": "content-type,x-elastos-home-token",
    "access-control-allow-methods": "GET,POST,PUT,DELETE,OPTIONS",
    "cache-control": "no-store",
    "content-length": body.length,
    "content-type": "application/json; charset=utf-8",
  });
  response.end(body);
}

function text(response, status, value) {
  const body = Buffer.from(String(value));
  response.writeHead(status, {
    "access-control-allow-origin": "*",
    "cache-control": "no-store",
    "content-length": body.length,
    "content-type": "text/plain; charset=utf-8",
  });
  response.end(body);
}

function contentType(path) {
  return {
    ".css": "text/css; charset=utf-8",
    ".html": "text/html; charset=utf-8",
    ".js": "text/javascript; charset=utf-8",
    ".png": "image/png",
    ".svg": "image/svg+xml",
    ".woff2": "font/woff2",
  }[extname(path)] || "application/octet-stream";
}

function defaultState() {
  return {
    summaryCalls: 0,
    priceCalls: 0,
    balanceCalls: [],
    qrCalls: [],
    sendBodies: [],
    defaultBodies: [],
    renameBodies: [],
    deleteBodies: [],
    recoveryCalls: [],
    clipboardRequests: [],
    clipboardShouldFail: false,
    openTargets: [],
    refreshSummaryCount: 0,
    appReadyCount: 0,
    privacyMessages: [],
    pendingMessages: [],
    stepUps: [],
    requestFailures: [],
    requestErrors: [],
    homeMessages: [],
    accounts: [
      {
        account_id: "acc-esc",
        chain_namespace: "eip155:20",
        address: groupAddress,
        label: "Family",
        proof_type: "managed_evm",
        signing_status: "managed_key_available",
        signing_available: true,
      },
      {
        account_id: "acc-base",
        chain_namespace: "eip155:8453",
        address: groupAddress,
        label: "Family",
        proof_type: "managed_evm",
        signing_status: "managed_key_available",
        signing_available: true,
      },
    ],
    defaultAccounts: [
      {
        account_id: "acc-esc",
        chain_namespace: "eip155:20",
        intent: "transaction_intent",
        set_at: 10,
      },
    ],
    approvals: [
      {
        request_id: "req-wallet-1",
        capsule_id: "browser",
        address: groupAddress,
        reason: "Review this action.",
        created_at: Math.floor(Date.now() / 1000) - 30,
        expires_at: Math.floor(Date.now() / 1000) + 600,
        status: "pending",
        proof_type: "managed_evm",
        intent: "browser_personal_sign",
      },
    ],
  };
}

function summaryResponse(state) {
  return {
    wallet_accounts: {
      accounts: state.accounts,
      default_accounts: state.defaultAccounts,
    },
    wallet_approvals: {
      approval_requests: state.approvals,
    },
    approval_methods: {
      walletconnect: { available: true },
    },
  };
}

function hexUnits(units, decimals) {
  const raw = BigInt(Math.round(units * (10 ** Math.min(decimals, 6)))) * (10n ** BigInt(decimals - Math.min(decimals, 6)));
  return `0x${raw.toString(16)}`;
}

function staticPath(pathname) {
  const roots = [
    ["/apps/wallet/", walletRoot],
    ["/apps/home/", homeRoot],
  ];
  for (const [prefix, root] of roots) {
    if (!pathname.startsWith(prefix)) {
      continue;
    }
    const suffix = decodeURIComponent(pathname.slice(prefix.length)) || "index.html";
    const candidate = resolve(root, suffix);
    const escaped = relative(root, candidate);
    if (escaped.startsWith(`..${sep}`) || escaped === "..") {
      return null;
    }
    return candidate;
  }
  return null;
}

async function readBody(request) {
  const chunks = [];
  let size = 0;
  for await (const chunk of request) {
    size += chunk.length;
    if (size > 1_048_576) {
      throw new Error("fixture request exceeds 1 MiB");
    }
    chunks.push(Buffer.from(chunk));
  }
  const textValue = Buffer.concat(chunks).toString("utf8");
  return textValue ? JSON.parse(textValue) : {};
}

function fixtureHtml(baseUrl, mode) {
  const homeToken = mode === "locked" ? "" : "wallet-home-token";
  const search = new URLSearchParams({
    home_origin: baseUrl,
    ...(mode === "rail" ? { presentation: "rail" } : {}),
  });
  const walletSrc = `${baseUrl}/apps/wallet/?${search.toString()}${homeToken ? `#home_token=${encodeURIComponent(homeToken)}` : ""}`;
  return `<!DOCTYPE html>
<html lang="en">
<body style="margin:0;background:#101012;">
  <iframe id="wallet-frame" src="${walletSrc}" style="border:0;width:100vw;height:100vh;"></iframe>
  <script>
    const clipboardTargetId = "wallet";
    const state = {
      clipboardGeneration: "${clipboardGeneration}",
      clipboardShouldFail: false,
      appReadyCount: 0,
      openTargets: [],
      refreshSummaryCount: 0,
      privacyMessages: [],
      pendingMessages: [],
      clipboardRequests: [],
      stepUps: [],
      homeMessages: [],
    };
    window.__walletFixture = state;
    window.addEventListener("message", (event) => {
      const data = event.data || {};
      state.homeMessages.push({ origin: event.origin, sourceIsFrame: event.source === document.getElementById("wallet-frame").contentWindow, type: data.type || "" });
      if (data.type === "home:app-ready") {
        state.appReadyCount += 1;
        event.source.postMessage({
          type: "home:clipboard-ready",
          schema: "elastos.home.clipboard.ready/v1",
          targetId: clipboardTargetId,
          homeToken: data.homeToken,
          parentOrigin: "${baseUrl}",
          generation: state.clipboardGeneration,
        }, "${baseUrl}");
        return;
      }
      if (data.type === "home:clipboard-request") {
        state.clipboardRequests.push(data);
        event.source.postMessage(
          state.clipboardShouldFail
            ? {
                type: "home:clipboard-result",
                schema: "elastos.home.clipboard.result/v1",
                requestId: data.requestId,
                targetId: clipboardTargetId,
                homeToken: data.homeToken,
                parentOrigin: data.parentOrigin,
                generation: data.generation,
                operation: data.operation,
                purpose: data.purpose,
                ok: false,
                error: "denied",
              }
            : {
                type: "home:clipboard-result",
                schema: "elastos.home.clipboard.result/v1",
                requestId: data.requestId,
                targetId: clipboardTargetId,
                homeToken: data.homeToken,
                parentOrigin: data.parentOrigin,
                generation: data.generation,
                operation: data.operation,
                purpose: data.purpose,
                ok: true,
              },
          "${baseUrl}",
        );
        state.clipboardShouldFail = false;
        return;
      }
      if (data.type === "elastos.home.passkey-step-up.request/v1") {
        state.stepUps.push(data);
        event.source.postMessage({
          type: "elastos.home.passkey-step-up.result/v1",
          requestId: data.requestId,
          stepUpToken: "fixture-step-up-token-" + state.stepUps.length,
        }, "${baseUrl}");
        return;
      }
      if (data.type === "home:open-target") {
        state.openTargets.push(data);
        return;
      }
      if (data.type === "home:refresh-summary") {
        state.refreshSummaryCount += 1;
        return;
      }
      if (data.type === "wallet:privacy-state") {
        state.privacyMessages.push(data);
        return;
      }
      if (data.type === "wallet:pending-count") {
        state.pendingMessages.push(data);
      }
    });
  </script>
</body>
</html>`;
}

async function startServer() {
  const state = defaultState();
  const server = createServer(async (request, response) => {
    try {
      const url = new URL(request.url || "/", "http://127.0.0.1");
      if (request.method === "OPTIONS") {
        response.writeHead(204, {
          "access-control-allow-origin": "*",
          "access-control-allow-headers": "content-type,x-elastos-home-token",
          "access-control-allow-methods": "GET,POST,PUT,DELETE,OPTIONS",
        });
        response.end();
        return;
      }
      if (url.pathname === "/favicon.ico") {
        response.writeHead(204, { "cache-control": "no-store" });
        response.end();
        return;
      }
      if (url.pathname === "/fixture-host.html") {
        const baseUrl = `http://127.0.0.1:${server.address().port}`;
        const html = fixtureHtml(baseUrl, url.searchParams.get("mode") || "window");
        response.writeHead(200, {
          "cache-control": "no-store",
          "content-length": Buffer.byteLength(html),
          "content-type": "text/html; charset=utf-8",
        });
        response.end(html);
        return;
      }
      const staticFile = staticPath(url.pathname);
      if (staticFile) {
        const body = await readFile(staticFile);
        response.writeHead(200, {
          "access-control-allow-origin": "*",
          "cache-control": "no-store",
          "content-length": body.length,
          "content-type": contentType(staticFile),
        });
        response.end(body);
        return;
      }
      if (url.pathname === "/api/apps/wallet/wallet/summary" && request.method === "GET") {
        state.summaryCalls += 1;
        const token = request.headers["x-elastos-home-token"];
        if (token !== "wallet-home-token") {
          json(response, 401, { error: "fixture token rejected" });
          return;
        }
        json(response, 200, summaryResponse(state));
        return;
      }
      if (url.pathname === "/api/wallet/prices" && request.method === "GET") {
        state.priceCalls += 1;
        json(response, 200, {
          stale: false,
          unavailable: false,
          prices: {
            ELA: { usd: 2 },
            ETH: { usd: 3000 },
          },
        });
        return;
      }
      if (url.pathname === "/api/provider/chain/balance" && request.method === "POST") {
        const body = await readBody(request);
        state.balanceCalls.push(body);
        if (body.network === "esc-mainnet" && body.address === groupAddress) {
          json(response, 200, { status: "ok", data: { balance_hex: hexUnits(12, 18) } });
          return;
        }
        if (body.network === "base-mainnet" && body.address === groupAddress) {
          json(response, 200, { status: "ok", data: { balance_hex: hexUnits(1.5, 18) } });
          return;
        }
        json(response, 200, { status: "error", message: "Unknown fixture balance target" });
        return;
      }
      if (url.pathname === "/api/wallet/qr" && request.method === "POST") {
        const body = await readBody(request);
        state.qrCalls.push(body);
        json(response, 200, {
          svg: '<svg viewBox="0 0 64 64" xmlns="http://www.w3.org/2000/svg"><rect width="64" height="64" fill="white"/><rect x="8" y="8" width="16" height="16" fill="black"/><rect x="40" y="8" width="16" height="16" fill="black"/><rect x="24" y="24" width="16" height="16" fill="black"/><rect x="8" y="40" width="16" height="16" fill="black"/><rect x="40" y="40" width="16" height="16" fill="black"/></svg>',
        });
        return;
      }
      if (url.pathname === "/api/apps/wallet/wallet/send" && request.method === "POST") {
        const body = await readBody(request);
        state.sendBodies.push(body);
        json(response, 200, {
          transaction_hash: "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
          approval_request: {
            request_id: `completed-${state.sendBodies.length}`,
            status: "completed",
          },
        });
        return;
      }
      if (url.pathname === "/api/apps/wallet/wallet/default" && request.method === "POST") {
        const body = await readBody(request);
        state.defaultBodies.push(body);
        state.defaultAccounts = [{
          account_id: body.account_id,
          chain_namespace: body.chain_namespace,
          intent: body.intent,
          set_at: 100 + state.defaultBodies.length,
        }];
        json(response, 200, {});
        return;
      }
      if (/^\/api\/apps\/wallet\/wallet\/accounts\/[^/]+\/recovery-key$/.test(url.pathname) && request.method === "POST") {
        const body = await readBody(request);
        const accountId = decodeURIComponent(url.pathname.split("/")[6]);
        state.recoveryCalls.push({ accountId, body });
        const account = state.accounts.find((item) => item.account_id === accountId);
        json(response, 200, {
          account_id: accountId,
          chain_namespace: account?.chain_namespace || "",
          address: account?.address || "",
          secret_type: "hex",
          private_key_hex: "ab".repeat(32),
          note: "Fixture recovery key",
        });
        return;
      }
      if (/^\/api\/apps\/wallet\/wallet\/accounts\/[^/]+$/.test(url.pathname) && request.method === "PUT") {
        const body = await readBody(request);
        const accountId = decodeURIComponent(url.pathname.split("/")[6]);
        state.renameBodies.push({ accountId, body });
        state.accounts = state.accounts.map((item) =>
          item.account_id === accountId ? { ...item, label: body.label } : item,
        );
        json(response, 200, {});
        return;
      }
      if (/^\/api\/apps\/wallet\/wallet\/accounts\/[^/]+$/.test(url.pathname) && request.method === "DELETE") {
        const body = await readBody(request);
        const accountId = decodeURIComponent(url.pathname.split("/")[6]);
        state.deleteBodies.push({ accountId, body });
        state.accounts = state.accounts.filter((item) => item.account_id !== accountId);
        state.defaultAccounts = state.defaultAccounts.filter((item) => item.account_id !== accountId);
        json(response, 200, {});
        return;
      }
      text(response, 500, `Unexpected fixture route: ${request.method} ${url.pathname}`);
    } catch (error) {
      state.requestErrors.push(String(error.message || error));
      text(response, 500, String(error.message || error));
    }
  });

  await new Promise((resolvePromise) => server.listen(0, "127.0.0.1", resolvePromise));
  return {
    baseUrl: `http://127.0.0.1:${server.address().port}`,
    state,
    async close() {
      await new Promise((resolvePromise, rejectPromise) =>
        server.close((error) => (error ? rejectPromise(error) : resolvePromise())),
      );
    },
  };
}

async function walletFrame(page) {
  await page.waitForSelector("#wallet-frame");
  await page.waitForFunction(() => {
    const frame = document.querySelector("#wallet-frame");
    return frame && frame.contentWindow && frame.contentWindow.location.href.includes("/apps/wallet/");
  }, { timeout: 5000 });
  const frame = page.frames().find((item) => item.url().includes("/apps/wallet/"));
  assert(frame, "Wallet iframe did not load.");
  return frame;
}

async function fixtureState(page) {
  return page.evaluate(() => structuredClone(window.__walletFixture));
}

async function waitForCondition(check, timeoutMs, label) {
  const startedAt = Date.now();
  while (Date.now() - startedAt <= timeoutMs) {
    if (await check()) {
      return;
    }
    await new Promise((resolvePromise) => setTimeout(resolvePromise, 50));
  }
  throw new Error(`timed out waiting for ${label}`);
}

async function assertNoErrors(page, server, label) {
  const details = await page.evaluate(() => ({
    fixture: structuredClone(window.__walletFixture),
  }));
  assert(server.state.requestErrors.length === 0, `${label}: fixture server errors must stay empty.`, server.state.requestErrors);
  assert(server.state.requestFailures.length === 0, `${label}: unexpected request failures must stay empty.`, server.state.requestFailures);
  assert((page.__pageErrors || []).length === 0, `${label}: page errors must stay empty.`, page.__pageErrors);
  assert((page.__consoleErrors || []).length === 0, `${label}: console errors must stay empty.`, page.__consoleErrors);
  assert((page.__requestFailures || []).length === 0, `${label}: request failures must stay empty.`, {
    requestFailures: page.__requestFailures,
    fixture: details.fixture,
  });
}

async function screenshot(frame, path) {
  await frame.locator("main.wallet-shell").screenshot({ path });
}

async function assertNoHorizontalOverflow(frame, label) {
  const layout = await frame.evaluate(() => ({
    documentScrollWidth: document.documentElement.scrollWidth,
    innerWidth: window.innerWidth,
    shellScrollWidth: document.querySelector("main.wallet-shell")?.scrollWidth || 0,
  }));
  assert(
    layout.documentScrollWidth <= layout.innerWidth && layout.shellScrollWidth <= layout.innerWidth,
    `${label}: Wallet must not overflow horizontally.`,
    layout,
  );
}

async function controlRect(frame, selector, label) {
  const rect = await frame.locator(selector).evaluate((node) => {
    const box = node.getBoundingClientRect();
    return {
      left: box.left,
      top: box.top,
      right: box.right,
      bottom: box.bottom,
      width: box.width,
      height: box.height,
      viewportWidth: window.innerWidth,
      viewportHeight: window.innerHeight,
    };
  });
  assert(rect.width > 0 && rect.height > 0, `${label}: control must be visible.`, rect);
  assert(rect.left >= 0 && rect.top >= 0 && rect.right <= rect.viewportWidth && rect.bottom <= rect.viewportHeight, `${label}: control must fit inside the viewport.`, rect);
}

async function openAccountMenu(frame) {
  await frame.locator(".wallet-account .wallet-more-button").click();
  await frame
    .locator(".wallet-flow-row")
    .filter({ hasText: "Use by default" })
    .waitFor({ state: "visible", timeout: 5000 });
}

async function run() {
  const server = await startServer();
  const tempDir = await mkdtemp(join(tmpdir(), "wallet-uiux-"));
  const desktopShot = join(tempDir, "wallet-desktop-1280x900.png");
  const narrowShot = join(tempDir, "wallet-narrow-640x900.png");
  const browser = await chromium.launch({
    executablePath: brave,
    headless: true,
  });

  try {
    const lockedPage = await browser.newPage({ viewport: { width: 1280, height: 900 } });
    lockedPage.__pageErrors = [];
    lockedPage.__consoleErrors = [];
    lockedPage.__requestFailures = [];
    lockedPage.on("pageerror", (error) => lockedPage.__pageErrors.push(String(error.message || error)));
    lockedPage.on("console", (message) => {
      if (message.type() === "error") {
        lockedPage.__consoleErrors.push(message.text());
      }
    });
    lockedPage.on("requestfailed", (request) => lockedPage.__requestFailures.push(request.url()));
    await lockedPage.goto(`${server.baseUrl}/fixture-host.html?mode=locked`, { waitUntil: "networkidle", timeout: 5000 });
    const lockedFrame = await walletFrame(lockedPage);
    await lockedFrame.locator("#wallet-status").waitFor({ state: "visible", timeout: 5000 });
    assert(await lockedFrame.locator("#wallet-status").textContent() === "Open Wallet from Home.", "Locked launch must stay locked until Home provides a token.");
    assert(server.state.summaryCalls === 0 && server.state.priceCalls === 0 && server.state.balanceCalls.length === 0, "Locked launch must not call Wallet APIs.");
    await assertNoErrors(lockedPage, server, "locked");
    await lockedPage.close();

    const page = await browser.newPage({ viewport: { width: 1280, height: 900 } });
    page.__pageErrors = [];
    page.__consoleErrors = [];
    page.__requestFailures = [];
    page.on("pageerror", (error) => page.__pageErrors.push(String(error.message || error)));
    page.on("console", (message) => {
      if (message.type() === "error") {
        page.__consoleErrors.push(message.text());
      }
    });
    page.on("requestfailed", (request) => page.__requestFailures.push(request.url()));

    await page.goto(`${server.baseUrl}/fixture-host.html?mode=window`, { waitUntil: "networkidle", timeout: 5000 });
    const frame = await walletFrame(page);
    await frame.locator(".wallet-account").waitFor({ state: "visible", timeout: 5000 });

    const hostState = await fixtureState(page);
    assert(hostState.appReadyCount === 1, "Verified Wallet launch must announce home:app-ready exactly once.");
    assert(hostState.pendingMessages.at(-1)?.count === 1, "Wallet must send the pending-count chrome fact.");
    assert(server.state.summaryCalls >= 1 && server.state.priceCalls >= 1, "Verified Wallet launch must load summary and prices.");
    assert(server.state.balanceCalls.length === 2, "Grouped EVM account must load one exact balance per network record.");

    assert(await frame.locator(".wallet-account").count() === 1, "Grouped EVM records with the same address must render as one display card.");
    await frame.locator(".wallet-account").click();
    await frame.locator("#wallet-account-card").waitFor({ state: "visible", timeout: 5000 });
    assert(await frame.locator("#wallet-account-card-name").textContent() === "Family", "Hero card must reflect the selected grouped account.");
    assert(await frame.locator("#wallet-account-card-network").textContent() === "Built-in · EVM", "Hero card must keep the grouped EVM label.");

    await frame.locator("#wallet-send").click();
    await frame
      .locator(".wallet-flow-row")
      .filter({ hasText: "1.5 ETH" })
      .filter({ hasText: "Base" })
      .click();
    await frame.locator('input[name="amount"]').fill("0.25");
    await frame.locator('input[name="to"]').fill(sendAddress);
    await frame.getByRole("button", { name: "Review" }).click();
    await frame.getByRole("button", { name: "Sign" }).click();
    await waitForCondition(() => server.state.sendBodies.length === 1, 5000, "wallet send request");
    assert(server.state.sendBodies.length === 1, "Wallet Send must submit exactly one request.");
    assert(
      server.state.sendBodies[0].account_id === "acc-base" && server.state.sendBodies[0].chain_namespace === "eip155:8453",
      "Wallet Send must bind the selected ETH asset to the exact Base account record.",
      server.state.sendBodies[0],
    );

    await frame.locator("#wallet-receive").click();
    await frame.locator(".wallet-qr").waitFor({ state: "visible", timeout: 5000 });
    assert(server.state.qrCalls.length === 1 && server.state.qrCalls[0].address === groupAddress, "Receive must stay group-level and bind only the shared address.");
    assert(
      (await frame.locator(".wallet-flow-hint").last().textContent()).includes("Always choose the correct chain"),
      "Receive must keep the grouped EVM chain warning.",
    );
    await frame.getByRole("button", { name: "Done" }).click();

    await frame.locator("#wallet-settings-open").click();
    await frame.locator("#wallet-settings-drawer").waitFor({ state: "visible", timeout: 5000 });
    await frame.locator('#wallet-currency-settings [data-wallet-currency="usd"]').click();
    assert(
      await frame.evaluate(() => window.localStorage.getItem("wallet.displayCurrency")) === "usd",
      "Wallet currency preference must stay in the display-only local storage key.",
    );
    await frame.getByRole("button", { name: "Close settings" }).click();
    await frame.locator("#wallet-settings-drawer").waitFor({ state: "hidden", timeout: 5000 });
    const privacyMessageCount = (await fixtureState(page)).privacyMessages.length;
    await frame.locator("#wallet-privacy").click();
    await waitForCondition(async () => {
      const messages = (await fixtureState(page)).privacyMessages;
      return messages.length === privacyMessageCount + 1
        && messages.at(-1)?.privacyMode === true;
    }, 5000, "wallet privacy chrome fact");
    assert(
      (await fixtureState(page)).privacyMessages.at(-1)?.privacyMode === true,
      "Wallet privacy changes must stay a chrome fact.",
    );

    const copyButton = frame.locator("#wallet-account-card-copy .wallet-copy-icon");
    const clipboardSuccessCount = (await fixtureState(page)).clipboardRequests.length;
    await copyButton.click();
    await waitForCondition(async () => {
      const requests = (await fixtureState(page)).clipboardRequests;
      return requests.length === clipboardSuccessCount + 1
        && requests.at(-1)?.purpose === "wallet.address"
        && requests.at(-1)?.text === groupAddress;
    }, 5000, "wallet clipboard success request");
    await frame.waitForFunction(
      () => {
        const button = document.querySelector("#wallet-account-card-copy .wallet-copy-icon");
        return button?.getAttribute("title") === "Copied"
          && button?.getAttribute("aria-label") === "Copied";
      },
      undefined,
      { timeout: 5000 },
    );
    const clipboardSuccess = (await fixtureState(page)).clipboardRequests.at(-1);
    assert(
      clipboardSuccess?.purpose === "wallet.address" && clipboardSuccess?.text === groupAddress,
      "Wallet copy must use the protected Home Clipboard with the wallet.address purpose.",
      clipboardSuccess,
    );
    await page.evaluate(() => {
      window.__walletFixture.clipboardShouldFail = true;
    });
    const clipboardFailureCount = (await fixtureState(page)).clipboardRequests.length;
    await copyButton.click();
    await waitForCondition(async () => {
      const requests = (await fixtureState(page)).clipboardRequests;
      return requests.length === clipboardFailureCount + 1
        && requests.at(-1)?.purpose === "wallet.address"
        && requests.at(-1)?.text === groupAddress;
    }, 5000, "wallet clipboard failure request");
    await frame.waitForFunction(
      (message) => document.querySelector("#wallet-status")?.textContent === message,
      defaultGenericError,
      { timeout: 5000 },
    );
    assert(
      await frame.locator("#wallet-status").textContent() === defaultGenericError,
      "Wallet copy failures must render the public error, not raw clipboard details.",
    );

    const openTargetCount = (await fixtureState(page)).openTargets.length;
    await frame.locator('#wallet-methods [data-wallet-open-method="wallet-metamask"]').waitFor({ state: "visible", timeout: 5000 });
    await frame.locator('#wallet-methods [data-wallet-open-method="wallet-metamask"]').click();
    await waitForCondition(async () => {
      const openTargets = (await fixtureState(page)).openTargets;
      return openTargets.length === openTargetCount + 1
        && openTargets.at(-1)?.target === "wallet-metamask";
    }, 5000, "wallet connector launch");
    const openTarget = (await fixtureState(page)).openTargets.at(-1);
    assert(openTarget?.target === "wallet-metamask", "Wallet connector launch must stay on the exact Home open-target path.", openTarget);

    await openAccountMenu(frame);
    const defaultRequestCount = server.state.defaultBodies.length;
    await frame.locator(".wallet-flow-row").filter({ hasText: "Use by default" }).click();
    const defaultDialog = frame.getByRole("dialog", { name: "Use by default" });
    await defaultDialog.waitFor({ state: "visible", timeout: 5000 });
    await defaultDialog.getByText("Choose the exact network", { exact: true }).waitFor({ state: "visible", timeout: 5000 });
    await defaultDialog.locator(".wallet-flow-row").filter({ hasText: "Base" }).click();
    await waitForCondition(() => server.state.defaultBodies.length === defaultRequestCount + 1, 5000, "wallet default request");
    assert(
      server.state.defaultBodies.at(-1)?.account_id === "acc-base" && server.state.defaultBodies.at(-1)?.chain_namespace === "eip155:8453",
      "Use by default must choose an exact grouped network record, not the primary grouped record.",
      server.state.defaultBodies.at(-1),
    );

    await openAccountMenu(frame);
    const recoveryRequestCount = server.state.recoveryCalls.length;
    const recoveryStepUpCount = (await fixtureState(page)).stepUps.length;
    await frame.locator(".wallet-flow-row").filter({ hasText: "Show recovery key" }).click();
    const recoveryDialog = frame.getByRole("dialog", { name: "Recovery key" });
    await recoveryDialog.waitFor({ state: "visible", timeout: 5000 });
    await recoveryDialog.getByText("Choose the exact network", { exact: true }).waitFor({ state: "visible", timeout: 5000 });
    await recoveryDialog.locator(".wallet-flow-row").filter({ hasText: "Elastos Smart Chain" }).click();
    await waitForCondition(() => server.state.recoveryCalls.length === recoveryRequestCount + 1, 5000, "wallet recovery request");
    await waitForCondition(
      () => fixtureState(page).then((state) => state.stepUps.length === recoveryStepUpCount + 1),
      5000,
      "wallet recovery step-up",
    );
    assert(
      server.state.recoveryCalls.at(-1)?.accountId === "acc-esc" && (await fixtureState(page)).stepUps.at(-1)?.request?.account_id === "acc-esc",
      "Recovery must bind one exact grouped record.",
      {
        recovery: server.state.recoveryCalls.at(-1),
        stepUp: (await fixtureState(page)).stepUps.at(-1),
      },
    );
    await frame.getByRole("button", { name: "Done" }).click();

    await openAccountMenu(frame);
    const renameRequestCount = server.state.renameBodies.length;
    await frame.locator(".wallet-flow-row").filter({ hasText: "Rename" }).click();
    await frame.locator(".wallet-flow-hint").waitFor({ state: "visible", timeout: 5000 });
    assert(
      (await frame.locator(".wallet-flow-hint").textContent()).includes("updates 2 exact account records"),
      "Grouped rename must name the full grouped scope.",
    );
    await frame.locator('input[name="label"]').fill("Family Office");
    const renameDialog = frame.getByRole("dialog", { name: "Rename account" });
    await frame.getByRole("button", { name: "Save" }).click();
    await waitForCondition(() => server.state.renameBodies.length === renameRequestCount + 2, 5000, "wallet rename requests");
    assert(
      server.state.renameBodies.length === 2
        && server.state.renameBodies.some((item) => item.accountId === "acc-esc")
        && server.state.renameBodies.some((item) => item.accountId === "acc-base"),
      "Grouped rename must update each exact grouped account record.",
      server.state.renameBodies,
    );
    await renameDialog.waitFor({ state: "hidden", timeout: 5000 });
    await frame.waitForFunction(() => new Promise((resolve) => {
      const hero = document.querySelector(".wallet-hero");
      const account = document.querySelector("#wallet-accounts .wallet-account");
      const modal = document.querySelector("#wallet-modal");
      if (!hero || !account || !modal || !modal.hidden) {
        resolve(false);
        return;
      }
      const heroRect = hero.getBoundingClientRect();
      const accountRect = account.getBoundingClientRect();
      requestAnimationFrame(() => {
        requestAnimationFrame(() => {
          const nextHeroRect = hero.getBoundingClientRect();
          const nextAccountRect = account.getBoundingClientRect();
          resolve(
            modal.hidden
              && heroRect.width > 0
              && heroRect.height > 0
              && accountRect.width > 0
              && accountRect.height > 0
              && heroRect.top === nextHeroRect.top
              && heroRect.height === nextHeroRect.height
              && accountRect.top === nextAccountRect.top
              && accountRect.height === nextAccountRect.height,
          );
        });
      });
    }), undefined, { timeout: 5000 });

    await frame.locator(".wallet-hero").scrollIntoViewIfNeeded();
    await screenshot(frame, desktopShot);
    await assertNoHorizontalOverflow(frame, "desktop");
    await controlRect(frame, ".wallet-hero", "desktop hero");
    await frame.locator("#wallet-settings-open").click();
    await frame.locator("#wallet-settings-drawer").waitFor({ state: "visible", timeout: 5000 });
    await frame.waitForFunction(() => {
      const drawer = document.querySelector("#wallet-settings-drawer");
      if (!drawer || !drawer.classList.contains("is-open")) {
        return false;
      }
      const rect = drawer.getBoundingClientRect();
      return rect.left >= 0 && rect.right <= window.innerWidth;
    }, undefined, { timeout: 5000 });
    await controlRect(frame, "#wallet-settings-drawer", "desktop settings drawer");
    await frame.getByRole("button", { name: "Close settings" }).click();
    await frame.locator("#wallet-settings-drawer").waitFor({ state: "hidden", timeout: 5000 });

    await page.setViewportSize({ width: 640, height: 900 });
    await frame.waitForFunction(() => window.innerWidth === 640, undefined, { timeout: 5000 });
    await frame.locator(".wallet-hero").scrollIntoViewIfNeeded();
    await assertNoHorizontalOverflow(frame, "narrow");
    await controlRect(frame, ".wallet-hero", "narrow hero");
    await screenshot(frame, narrowShot);

    await openAccountMenu(frame);
    const deleteRequestCount = server.state.deleteBodies.length;
    const deleteStepUpCount = (await fixtureState(page)).stepUps.length;
    await frame.locator(".wallet-flow-row").filter({ hasText: "Delete account" }).click();
    const deleteHint = await frame.locator(".wallet-flow-hint").textContent();
    assert(deleteHint.includes("removes 2 exact account records"), "Grouped delete must name the full grouped scope.");
    assert(server.state.deleteBodies.length === 0, "Delete must not run before confirmation.");
    await frame.getByRole("button", { name: "Delete" }).click();
    await waitForCondition(() => server.state.deleteBodies.length === deleteRequestCount + 2, 5000, "wallet delete requests");
    await waitForCondition(
      () => fixtureState(page).then((state) => state.stepUps.length === deleteStepUpCount + 2),
      5000,
      "wallet delete step-ups",
    );
    await frame.waitForFunction(() => document.querySelector("#wallet-account-state")?.textContent === "0 accounts", undefined, { timeout: 5000 });
    assert(
      server.state.deleteBodies.length === 2
        && server.state.deleteBodies.some((item) => item.accountId === "acc-esc")
        && server.state.deleteBodies.some((item) => item.accountId === "acc-base"),
      "Grouped delete must bind each exact grouped account id.",
      server.state.deleteBodies,
    );

    await assertNoErrors(page, server, "window");
    await page.close();

    const railFixtureState = defaultState();
    server.state.accounts = structuredClone(railFixtureState.accounts);
    server.state.defaultAccounts = structuredClone(railFixtureState.defaultAccounts);

    const railPage = await browser.newPage({ viewport: { width: 640, height: 900 } });
    railPage.__pageErrors = [];
    railPage.__consoleErrors = [];
    railPage.__requestFailures = [];
    railPage.on("pageerror", (error) => railPage.__pageErrors.push(String(error.message || error)));
    railPage.on("console", (message) => {
      if (message.type() === "error") {
        railPage.__consoleErrors.push(message.text());
      }
    });
    railPage.on("requestfailed", (request) => railPage.__requestFailures.push(request.url()));
    await railPage.goto(`${server.baseUrl}/fixture-host.html?mode=rail`, { waitUntil: "networkidle", timeout: 5000 });
    const railFrame = await walletFrame(railPage);
    await railFrame.locator(".wallet-account").waitFor({ state: "visible", timeout: 5000 });
    const railState = await railFrame.evaluate(() => ({
      presentation: document.documentElement.dataset.walletPresentation,
      heroNavHidden: document.querySelector(".wallet-hero-nav")?.hasAttribute("hidden") === true,
    }));
    assert(railState.presentation === "rail" && railState.heroNavHidden, "Rail mode must keep the reviewed Wallet rail presentation.");
    await assertNoErrors(railPage, server, "rail");
    await railPage.close();

    console.log(`wallet-product-layout-smoke: OK\n${desktopShot}\n${narrowShot}`);
  } finally {
    await browser.close().catch(() => {});
    await server.close().catch(() => {});
    if (process.env.KEEP_WALLET_UIUX_SMOKE_TMP !== "1") {
      await rm(tempDir, { recursive: true, force: true }).catch(() => {});
    }
  }
}

await run();
