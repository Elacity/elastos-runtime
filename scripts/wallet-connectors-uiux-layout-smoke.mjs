#!/usr/bin/env node

import { createServer } from "node:http";
import { createRequire } from "node:module";
import { mkdtemp, readFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { extname, join, relative, resolve, sep } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const repoRoot = resolve(fileURLToPath(new URL("../", import.meta.url)));
const homeRoot = join(repoRoot, "capsules/home/browser");
const connectorRoots = Object.freeze({
  "wallet-metamask": join(repoRoot, "capsules/wallet-metamask/browser"),
  "wallet-unisat": join(repoRoot, "capsules/wallet-unisat/browser"),
  "wallet-walletconnect": join(repoRoot, "capsules/wallet-walletconnect/browser"),
});
const brave = process.env.BRAVE_BIN || "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser";
const playwrightModule = process.env.ELASTOS_PLAYWRIGHT_MODULE
  ? await import(pathToFileURL(process.env.ELASTOS_PLAYWRIGHT_MODULE).href)
  : createRequire(new URL("../elastos/tools/browser-playwright-engine/package.json", import.meta.url))("playwright");
const { chromium } = playwrightModule.chromium ? playwrightModule : playwrightModule.default;

const BITCOIN_CHAIN_NAMESPACE = "bip122:000000000019d6689c085ae165831e93";
const screenshotRoot = await mkdtemp(join(tmpdir(), "wallet-connectors-uiux-"));
const fixtureGeneration = "wallet-connectors-uiux-layout-v1";

const connectors = Object.freeze([
  {
    id: "wallet-metamask",
    title: "MetaMask",
    lockedStatus: "Open from Wallet to review approval requests.",
    token: "wallet-metamask-layout-token",
    accountSummary: {
      accounts: [
        {
          chain_namespace: "eip155:20",
          address: "0x1111111111111111111111111111111111111111",
        },
      ],
    },
    requestSummary: {
      approval_requests: [
        {
          request_id: "metamask-request-1",
          connector_id: "wallet-metamask",
          intent: "browser_personal_sign",
          proof_type: "siwe",
          capsule_id: "browser",
          address: "0x1111111111111111111111111111111111111111",
          reason: "Review this action.",
        },
      ],
    },
  },
  {
    id: "wallet-unisat",
    title: "UniSat",
    lockedStatus: "Open from Wallet to review approval requests.",
    token: "wallet-unisat-layout-token",
    accountSummary: {
      accounts: [
        {
          chain_namespace: BITCOIN_CHAIN_NAMESPACE,
          address: "bc1qxy2kgdygjrsqtzq2n0yrf2493p83kkfjhx0wlh",
        },
      ],
    },
    requestSummary: {
      approval_requests: [
        {
          request_id: "unisat-request-1",
          connector_id: "wallet-unisat",
          intent: "bitcoin_bip322_proof",
          proof_type: "bip322_simple",
          chain_namespace: BITCOIN_CHAIN_NAMESPACE,
          capsule_id: "browser",
          address: "bc1qxy2kgdygjrsqtzq2n0yrf2493p83kkfjhx0wlh",
          reason: "Approve the Bitcoin proof.",
        },
      ],
    },
  },
  {
    id: "wallet-walletconnect",
    title: "WalletConnect",
    lockedStatus: "Open from Wallet to add an approval method.",
    token: "wallet-walletconnect-layout-token",
    accountSummary: {
      accounts: [
        {
          chain_namespace: "eip155:8453",
          address: "0x2222222222222222222222222222222222222222",
        },
      ],
    },
    requestSummary: {
      approval_requests: [
        {
          request_id: "walletconnect-request-1",
          connector_id: "wallet-walletconnect",
          intent: "transaction_intent",
          proof_type: "siwe",
          capsule_id: "services",
          address: "0x2222222222222222222222222222222222222222",
          reason: "Approve the transaction.",
        },
      ],
    },
  },
]);

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
    "access-control-allow-methods": "GET,POST,OPTIONS",
    "cache-control": "no-store",
    "content-length": body.length,
    "content-type": "application/json; charset=utf-8",
  });
  response.end(body);
}

function empty(response, status = 204) {
  response.writeHead(status, {
    "access-control-allow-origin": "*",
    "cache-control": "no-store",
  });
  response.end();
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

function staticPath(pathname) {
  const roots = [
    ["/apps/home/", homeRoot],
    ...Object.entries(connectorRoots).map(([id, root]) => [`/apps/${id}/`, root]),
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

function fixtureHtml(baseUrl, connector, mode) {
  const hasToken = mode === "verified";
  const query = new URLSearchParams({ home_origin: baseUrl });
  const src = `${baseUrl}/apps/${connector.id}/?${query.toString()}${
    hasToken ? `#home_token=${encodeURIComponent(connector.token)}` : ""
  }`;
  return `<!DOCTYPE html>
<html lang="en">
<body style="margin:0;background:#0b0d10;">
  <iframe
    id="connector-frame"
    src="${src}"
    style="border:0;display:block;width:100vw;height:100vh;"
  ></iframe>
  <script>
    const state = {
      appReadyCount: 0,
      clipboardRequests: [],
      homeMessages: [],
    };
    window.__connectorFixture = state;
    window.addEventListener("message", (event) => {
      const data = event.data || {};
      state.homeMessages.push({
        type: data.type || "",
        origin: event.origin,
        sourceIsFrame: event.source === document.getElementById("connector-frame").contentWindow,
      });
      if (data.type === "home:app-ready") {
        state.appReadyCount += 1;
        event.source.postMessage({
          type: "home:clipboard-ready",
          schema: "elastos.home.clipboard.ready/v1",
          targetId: "${connector.id}",
          homeToken: data.homeToken,
          parentOrigin: "${baseUrl}",
          generation: "${fixtureGeneration}",
        }, "${baseUrl}");
        return;
      }
      if (data.type === "home:clipboard-request") {
        state.clipboardRequests.push(data);
        event.source.postMessage({
          type: "home:clipboard-result",
          schema: "elastos.home.clipboard.result/v1",
          requestId: data.requestId,
          targetId: "${connector.id}",
          homeToken: data.homeToken,
          parentOrigin: data.parentOrigin,
          generation: data.generation,
          operation: data.operation,
          purpose: data.purpose,
          ok: true,
        }, "${baseUrl}");
      }
    });
  </script>
</body>
</html>`;
}

function startServer() {
  const traces = new Map();
  for (const connector of connectors) {
    traces.set(connector.id, {
      accountsCalls: 0,
      approvalsCalls: 0,
      tokens: [],
      unexpectedCalls: [],
    });
  }
  const server = createServer(async (request, response) => {
    try {
      const url = new URL(request.url, "http://127.0.0.1");
      if (request.method === "OPTIONS") {
        response.writeHead(204, {
          "access-control-allow-origin": "*",
          "access-control-allow-headers": "content-type,x-elastos-home-token",
          "access-control-allow-methods": "GET,POST,OPTIONS",
        });
        response.end();
        return;
      }
      if (url.pathname === "/favicon.ico") {
        empty(response);
        return;
      }
      if (url.pathname === "/fixture") {
        const connector = connectors.find((entry) => entry.id === url.searchParams.get("connector"));
        const mode = url.searchParams.get("mode");
        assert(connector, "unknown connector fixture");
        assert(mode === "locked" || mode === "verified", "unknown connector mode");
        const body = Buffer.from(fixtureHtml(baseUrl(server), connector, mode));
        response.writeHead(200, {
          "cache-control": "no-store",
          "content-length": body.length,
          "content-type": "text/html; charset=utf-8",
        });
        response.end(body);
        return;
      }
      const matchedConnector = connectors.find((entry) => (
        url.pathname === `/api/apps/${entry.id}/wallet/accounts`
        || url.pathname === `/api/apps/${entry.id}/wallet/approvals`
      ));
      if (matchedConnector) {
        const trace = traces.get(matchedConnector.id);
        const token = String(request.headers["x-elastos-home-token"] || "");
        trace.tokens.push(token);
        if (url.pathname.endsWith("/wallet/accounts")) {
          trace.accountsCalls += 1;
          if (token !== matchedConnector.token) {
            trace.unexpectedCalls.push({ path: url.pathname, token });
            json(response, 401, { error: "invalid token" });
            return;
          }
          json(response, 200, matchedConnector.accountSummary);
          return;
        }
        trace.approvalsCalls += 1;
        if (token !== matchedConnector.token) {
          trace.unexpectedCalls.push({ path: url.pathname, token });
          json(response, 401, { error: "invalid token" });
          return;
        }
        json(response, 200, matchedConnector.requestSummary);
        return;
      }
      const asset = staticPath(url.pathname);
      if (asset) {
        const body = await readFile(asset);
        response.writeHead(200, {
          "cache-control": "no-store",
          "content-length": body.length,
          "content-type": contentType(asset),
        });
        response.end(body);
        return;
      }
      const connectorId = connectors.find((entry) => url.pathname.includes(entry.id))?.id || null;
      if (connectorId) {
        traces.get(connectorId).unexpectedCalls.push({ path: url.pathname, token: String(request.headers["x-elastos-home-token"] || "") });
      }
      json(response, 404, { error: `unexpected fixture route: ${url.pathname}` });
    } catch (error) {
      json(response, 500, { error: String(error?.message || error) });
    }
  });
  return {
    traces,
    async listen() {
      await new Promise((resolveListen) => server.listen(0, "127.0.0.1", resolveListen));
      return {
        baseUrl: baseUrl(server),
        async close() {
          await new Promise((resolveClose) => server.close(resolveClose));
        },
      };
    },
  };
}

function baseUrl(server) {
  const address = server.address();
  assert(address && typeof address === "object", "fixture server is not listening");
  return `http://127.0.0.1:${address.port}`;
}

async function waitForConnectorFrame(page) {
  await page.waitForSelector("#connector-frame", { state: "attached", timeout: 5_000 });
  const handle = await page.$("#connector-frame");
  const frame = await handle?.contentFrame();
  assert(frame, "connector frame did not attach");
  await frame.waitForSelector("#wallet-connect", { state: "visible", timeout: 5_000 });
  return frame;
}

async function connectorSnapshot(frame, page, connectorId) {
  return frame.evaluate(({ screenshotRoot: root, connectorId: currentConnectorId }) => {
    const rectJson = (node) => {
      if (!node) {
        return null;
      }
      const rect = node.getBoundingClientRect();
      return {
        left: rect.left,
        top: rect.top,
        right: rect.right,
        bottom: rect.bottom,
        width: rect.width,
        height: rect.height,
      };
    };
    const shell = document.querySelector(".wallet-shell");
    const panels = [...document.querySelectorAll(".wallet-panel")];
    const theme = document.documentElement.getAttribute("data-el-theme") || "dark";
    return {
      connectorId: currentConnectorId,
      theme,
      colorScheme: getComputedStyle(document.documentElement).colorScheme,
      tokenBg: getComputedStyle(document.documentElement).getPropertyValue("--el-bg").trim(),
      bodyScrollWidth: document.body.scrollWidth,
      docScrollWidth: document.documentElement.scrollWidth,
      innerWidth: window.innerWidth,
      shellRect: rectJson(shell),
      panelRects: panels.map(rectJson),
      connectRect: rectJson(document.querySelector("#wallet-connect")),
      accountRect: rectJson(document.querySelector("#wallet-accounts .wallet-account")),
      requestRect: rectJson(document.querySelector("#wallet-requests .wallet-request")),
      copyRect: rectJson(document.querySelector("[data-wallet-copy-address]")),
      reviewRect: rectJson(document.querySelector("[data-wallet-request-sign]")),
      statusText: document.querySelector("#wallet-status")?.textContent?.trim() || "",
      stateText: document.querySelector("#wallet-state")?.textContent?.trim() || "",
      accountCount: document.querySelectorAll("#wallet-accounts .wallet-account").length,
      requestCount: document.querySelectorAll("#wallet-requests .wallet-request").length,
      connectVisible: !document.querySelector("#wallet-connect")?.hidden,
      copyVisible: !document.querySelector("[data-wallet-copy-address]")?.hidden,
      reviewVisible: !document.querySelector("[data-wallet-request-sign]")?.hidden,
      screenshotRoot: root,
    };
  }, { screenshotRoot, connectorId });
}

function assertRectWithinViewport(name, rect, width) {
  assert(rect, `${name} is missing`);
  assert(rect.width > 0 && rect.height > 0, `${name} has no size`, rect);
  assert(rect.left >= 0, `${name} extends past the left edge`, rect);
  assert(rect.right <= width, `${name} extends past the right edge`, rect);
}

function assertLayoutSnapshot(snapshot, connector) {
  assert(snapshot.tokenBg.length > 0, `${connector.title} did not load shared UI tokens`, snapshot);
  assert(snapshot.docScrollWidth <= snapshot.innerWidth, `${connector.title} document overflowed horizontally`, snapshot);
  assert(snapshot.bodyScrollWidth <= snapshot.innerWidth, `${connector.title} body overflowed horizontally`, snapshot);
  assert(snapshot.stateText === "1 linked", `${connector.title} did not render the linked account count`, snapshot);
  assert(snapshot.accountCount === 1, `${connector.title} did not render one linked account`, snapshot);
  assert(snapshot.requestCount === 1, `${connector.title} did not render one approval request`, snapshot);
  assert(snapshot.connectVisible, `${connector.title} connect button is hidden`, snapshot);
  assert(snapshot.copyVisible, `${connector.title} copy button is hidden`, snapshot);
  assert(snapshot.reviewVisible, `${connector.title} review button is hidden`, snapshot);
  assertRectWithinViewport(`${connector.title} shell`, snapshot.shellRect, snapshot.innerWidth);
  assertRectWithinViewport(`${connector.title} connect button`, snapshot.connectRect, snapshot.innerWidth);
  assertRectWithinViewport(`${connector.title} account card`, snapshot.accountRect, snapshot.innerWidth);
  assertRectWithinViewport(`${connector.title} request card`, snapshot.requestRect, snapshot.innerWidth);
  assertRectWithinViewport(`${connector.title} copy button`, snapshot.copyRect, snapshot.innerWidth);
  assertRectWithinViewport(`${connector.title} review button`, snapshot.reviewRect, snapshot.innerWidth);
  for (const [index, rect] of snapshot.panelRects.entries()) {
    assertRectWithinViewport(`${connector.title} panel ${index + 1}`, rect, snapshot.innerWidth);
  }
}

async function assertLockedState(page, frame, connector, traces) {
  await frame.waitForFunction((expected) => {
    const status = document.querySelector("#wallet-status");
    return Boolean(status && !status.hidden && status.textContent.trim() === expected);
  }, connector.lockedStatus, { timeout: 5_000 });
  const fixtureState = await page.evaluate(() => window.__connectorFixture);
  const trace = traces.get(connector.id);
  assert(trace.accountsCalls === 0, `${connector.title} locked launch still called accounts`, trace);
  assert(trace.approvalsCalls === 0, `${connector.title} locked launch still called approvals`, trace);
  assert(fixtureState.clipboardRequests.length === 0, `${connector.title} sent clipboard traffic before interaction`, fixtureState);
}

async function assertVerifiedState(page, frame, connector, traces, width) {
  await frame.waitForFunction(() => (
    document.querySelector("#wallet-state")?.textContent?.trim() === "1 linked"
    && document.querySelectorAll("#wallet-accounts .wallet-account").length === 1
    && document.querySelectorAll("#wallet-requests .wallet-request").length === 1
  ), { timeout: 5_000 });
  const fixtureState = await page.evaluate(() => window.__connectorFixture);
  const trace = traces.get(connector.id);
  assert(trace.accountsCalls === 1, `${connector.title} did not make one accounts call`, trace);
  assert(trace.approvalsCalls === 1, `${connector.title} did not make one approvals call`, trace);
  assert(trace.unexpectedCalls.length === 0, `${connector.title} hit an unexpected API path`, trace);
  assert(trace.tokens.every((token) => token === connector.token), `${connector.title} used the wrong launch token`, trace);
  assert(fixtureState.appReadyCount === 1, `${connector.title} announced home:app-ready more than once`, fixtureState);
  const snapshot = await connectorSnapshot(frame, page, connector.id);
  assert(snapshot.colorScheme === (width <= 640 ? "light" : "dark"), `${connector.title} theme did not settle`, snapshot);
  assertLayoutSnapshot(snapshot, connector);
}

async function setLightTheme(frame) {
  await frame.evaluate(() => {
    document.documentElement.setAttribute("data-el-theme", "light");
  });
  await frame.waitForFunction(() => getComputedStyle(document.documentElement).colorScheme === "light", { timeout: 5_000 });
}

async function saveScreenshot(page, name) {
  const frameLocator = page.locator("#connector-frame");
  const path = join(screenshotRoot, name);
  await frameLocator.screenshot({ path });
  return path;
}

async function run() {
  const fixture = startServer();
  const server = await fixture.listen();
  const browser = await chromium.launch({
    executablePath: brave,
    headless: true,
  });
  const screenshots = [];
  try {
    for (const connector of connectors) {
      const lockedPage = await browser.newPage({ viewport: { width: 1280, height: 900 } });
      lockedPage.setDefaultTimeout(5_000);
      const lockedPageErrors = [];
      const lockedConsoleErrors = [];
      const lockedRequestFailures = [];
      lockedPage.on("pageerror", (error) => lockedPageErrors.push(String(error?.message || error)));
      lockedPage.on("console", (message) => {
        if (message.type() === "error") {
          lockedConsoleErrors.push(message.text());
        }
      });
      lockedPage.on("requestfailed", (request) => {
        lockedRequestFailures.push({ url: request.url(), failure: request.failure()?.errorText || "" });
      });
      await lockedPage.goto(`${server.baseUrl}/fixture?connector=${connector.id}&mode=locked`, {
        waitUntil: "networkidle",
      });
      const lockedFrame = await waitForConnectorFrame(lockedPage);
      await assertLockedState(lockedPage, lockedFrame, connector, fixture.traces);
      assert(lockedPageErrors.length === 0, `${connector.title} locked page had page errors`, lockedPageErrors);
      assert(lockedConsoleErrors.length === 0, `${connector.title} locked page had console errors`, lockedConsoleErrors);
      assert(lockedRequestFailures.length === 0, `${connector.title} locked page had request failures`, lockedRequestFailures);
      await lockedPage.close();

      const page = await browser.newPage({ viewport: { width: 1280, height: 900 } });
      page.setDefaultTimeout(5_000);
      const pageErrors = [];
      const consoleErrors = [];
      const requestFailures = [];
      page.on("pageerror", (error) => pageErrors.push(String(error?.message || error)));
      page.on("console", (message) => {
        if (message.type() === "error") {
          consoleErrors.push(message.text());
        }
      });
      page.on("requestfailed", (request) => {
        requestFailures.push({ url: request.url(), failure: request.failure()?.errorText || "" });
      });
      await page.goto(`${server.baseUrl}/fixture?connector=${connector.id}&mode=verified`, {
        waitUntil: "networkidle",
      });
      const frame = await waitForConnectorFrame(page);
      await assertVerifiedState(page, frame, connector, fixture.traces, 1280);
      screenshots.push(await saveScreenshot(page, `${connector.id}-desktop.png`));
      await page.setViewportSize({ width: 640, height: 900 });
      await frame.waitForFunction(() => window.innerWidth === 640, { timeout: 5_000 });
      await setLightTheme(frame);
      await assertVerifiedState(page, frame, connector, fixture.traces, 640);
      screenshots.push(await saveScreenshot(page, `${connector.id}-narrow.png`));
      assert(pageErrors.length === 0, `${connector.title} page had page errors`, pageErrors);
      assert(consoleErrors.length === 0, `${connector.title} page had console errors`, consoleErrors);
      assert(requestFailures.length === 0, `${connector.title} page had request failures`, requestFailures);
      await page.close();
    }
  } finally {
    await browser.close();
    await server.close();
  }
  for (const screenshot of screenshots) {
    console.log(`screenshot: ${screenshot}`);
  }
  console.log("wallet connector UIUX layout smoke: ok");
}

await run();
