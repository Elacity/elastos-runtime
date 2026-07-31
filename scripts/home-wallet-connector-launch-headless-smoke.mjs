#!/usr/bin/env node

import { createServer } from "node:http";
import { existsSync, readFileSync } from "node:fs";
import { extname, isAbsolute, join, relative, resolve, sep } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const repoRoot = fileURLToPath(new URL("../", import.meta.url));
const principalId = "did:elastos:fixture-wallet-connector-launch";
const homeAuthorityToken = "fixture-home-authority-token";
const homeGuiToken = "fixture-home-gui-token";
const walletToken = "fixture-wallet-token";
const connectorTokens = Object.freeze({
  "wallet-metamask": "fixture-metamask-token",
  "wallet-unisat": "fixture-unisat-token",
});
const connectorTitles = Object.freeze({
  "wallet-metamask": "MetaMask",
  "wallet-unisat": "UniSat",
});
const state = {
  launches: {
    "home-gui": 0,
    wallet: 0,
    "wallet-metamask": 0,
    "wallet-unisat": 0,
  },
  connectorEffects: 0,
  homeStateWrites: [],
  serverErrors: [],
};

function assert(condition, message, details = undefined) {
  if (condition) {
    return;
  }
  const suffix = details === undefined ? "" : `\n${JSON.stringify(details, null, 2)}`;
  throw new Error(`${message}${suffix}`);
}

function json(res, status, value) {
  const body = JSON.stringify(value);
  res.writeHead(status, {
    "access-control-allow-origin": "*",
    "cache-control": "no-store",
    "content-length": Buffer.byteLength(body),
    "content-type": "application/json; charset=utf-8",
  });
  res.end(body);
}

function empty(res) {
  res.writeHead(204, {
    "access-control-allow-origin": "*",
    "cache-control": "no-store",
  });
  res.end();
}

function contentType(path) {
  return {
    ".css": "text/css; charset=utf-8",
    ".html": "text/html; charset=utf-8",
    ".js": "text/javascript; charset=utf-8",
    ".json": "application/json; charset=utf-8",
    ".png": "image/png",
    ".svg": "image/svg+xml",
    ".webp": "image/webp",
  }[extname(path)] || "application/octet-stream";
}

function staticPath(pathname) {
  const roots = [
    ["/apps/home/", join(repoRoot, "capsules/home/browser")],
    ["/apps/home-gui/", join(repoRoot, "capsules/home-gui/browser")],
    ["/apps/wallet/", join(repoRoot, "capsules/wallet/browser")],
    ["/apps/wallet-metamask/", join(repoRoot, "capsules/wallet-metamask/browser")],
    ["/apps/wallet-unisat/", join(repoRoot, "capsules/wallet-unisat/browser")],
  ];
  for (const [prefix, root] of roots) {
    if (!pathname.startsWith(prefix)) {
      continue;
    }
    const suffix = decodeURIComponent(pathname.slice(prefix.length)) || "index.html";
    const candidate = resolve(root, suffix);
    const escaped = relative(root, candidate);
    if (escaped.startsWith(`..${sep}`) || escaped === ".." || isAbsolute(escaped)) {
      return null;
    }
    return candidate;
  }
  return null;
}

async function readBody(req) {
  const chunks = [];
  let size = 0;
  for await (const chunk of req) {
    size += chunk.length;
    if (size > 1_048_576) {
      throw new Error("fixture request exceeds 1 MiB");
    }
    chunks.push(chunk);
  }
  const text = Buffer.concat(chunks).toString("utf8");
  return text ? JSON.parse(text) : null;
}

function homeSummary() {
  return {
    home: { route: "/apps/home/", attach_kind: "iframe" },
    app: { id: "home", route: "/apps/home/" },
    identity: { id: principalId, display_name: "Connector Fixture" },
    authority: {
      signed_in: true,
      principal_id: principalId,
      proof_binding_id: "fixture-proof-binding",
    },
    active_shell: {
      active: "home-gui",
      candidates: [{
        name: "home-gui",
        title: "Desktop",
        launchable: true,
      }],
    },
    appearance: {},
    browser_state: {
      schema: "elastos.home.browser-state/v1",
      principal_id: principalId,
      layout: {
        desktop: {},
        taskbar: [],
        desktopHidden: [],
        desktopIconsVisible: true,
      },
      recent_targets: [],
      session: null,
    },
    runtime: {},
    site: {},
    room: {},
    people: {},
    services: {},
    notifications: [],
    desktop_objects: [],
    capsule_catalog: [],
    capsule_interfaces: [],
    targets: [{
      target: "wallet",
      title: "Wallet",
      description: "Private Wallet",
      role: "app",
      target_kind: "app",
      attach_kind: "iframe",
    }],
  };
}

function launchToken(target) {
  if (target === "home-gui") {
    return homeGuiToken;
  }
  if (target === "wallet") {
    return walletToken;
  }
  return connectorTokens[target] || "";
}

function launchResponse(origin, target, query) {
  state.launches[target] += 1;
  const params = new URLSearchParams();
  for (const [key, value] of Object.entries(query || {})) {
    params.set(key, String(value));
  }
  params.set("home_origin", origin);
  return {
    target,
    title: connectorTitles[target] || (target === "home-gui" ? "Desktop" : "Wallet"),
    attach_kind: "iframe",
    launch_status: "launched",
    route:
      `/apps/${target}/?${params.toString()}` +
      `#home_token=${encodeURIComponent(launchToken(target))}`,
  };
}

function requireToken(req, token, res) {
  if (req.headers["x-elastos-home-token"] === token) {
    return true;
  }
  json(res, 401, { error: "fixture launch owner rejected" });
  return false;
}

async function handleApi(req, res, url) {
  if (req.method === "OPTIONS") {
    res.writeHead(204, {
      "access-control-allow-headers": "content-type,x-elastos-home-token",
      "access-control-allow-methods": "GET,POST,OPTIONS",
      "access-control-allow-origin": "*",
    });
    res.end();
    return true;
  }
  if (url.pathname === "/api/auth/sessions/refresh" && req.method === "POST") {
    json(res, 200, { home_token: homeAuthorityToken });
    return true;
  }
  if (url.pathname === "/api/apps/home/summary" && req.method === "GET") {
    json(res, 200, homeSummary());
    return true;
  }
  if (url.pathname === "/api/apps/home/runtime/ensure" && req.method === "POST") {
    json(res, 200, { ready: true });
    return true;
  }
  if (url.pathname === "/api/apps/home/events/stream" && req.method === "GET") {
    empty(res);
    return true;
  }
  if (url.pathname === "/api/apps/home/events" && req.method === "GET") {
    json(res, 200, { events: [], cursor: "" });
    return true;
  }
  if (url.pathname === "/api/apps/home/launch" && req.method === "POST") {
    if (!requireToken(req, homeAuthorityToken, res)) {
      return true;
    }
    const input = await readBody(req);
    const target = typeof input?.target === "string" ? input.target : "";
    if (!Object.hasOwn(state.launches, target)) {
      json(res, 404, { error: "fixture target not found" });
      return true;
    }
    const origin = String(input?.query?.home_origin || "");
    json(res, 200, launchResponse(origin, target, input?.query || {}));
    return true;
  }
  if (url.pathname === "/api/apps/home/state" && req.method === "POST") {
    if (!requireToken(req, homeGuiToken, res)) {
      return true;
    }
    state.homeStateWrites.push(await readBody(req));
    json(res, 200, { saved: true });
    return true;
  }
  if (url.pathname === "/api/apps/wallet/wallet/summary" && req.method === "GET") {
    if (!requireToken(req, walletToken, res)) {
      return true;
    }
    json(res, 200, {
      wallet_accounts: { accounts: [], default_accounts: [] },
      wallet_approvals: { approval_requests: [] },
      approval_methods: {},
    });
    return true;
  }
  if (url.pathname === "/api/wallet/prices" && req.method === "GET") {
    if (!requireToken(req, walletToken, res)) {
      return true;
    }
    json(res, 200, { prices: {}, stale: false, unavailable: false });
    return true;
  }
  for (const [target, token] of Object.entries(connectorTokens)) {
    if (
      url.pathname === `/api/apps/${target}/wallet/accounts` &&
      req.method === "GET"
    ) {
      if (!requireToken(req, token, res)) {
        return true;
      }
      json(res, 200, { accounts: [] });
      return true;
    }
    if (
      url.pathname === `/api/apps/${target}/wallet/approvals` &&
      req.method === "GET"
    ) {
      if (!requireToken(req, token, res)) {
        return true;
      }
      json(res, 200, { approval_requests: [] });
      return true;
    }
  }
  if (url.pathname.includes("/wallet-connector/")) {
    state.connectorEffects += 1;
    json(res, 500, { error: "credential-free fixture forbids connector effects" });
    return true;
  }
  return false;
}

const server = createServer(async (req, res) => {
  try {
    const url = new URL(req.url || "/", "http://127.0.0.1");
    if (await handleApi(req, res, url)) {
      return;
    }
    const path = staticPath(url.pathname);
    if (!path || !existsSync(path)) {
      json(res, 404, { error: "fixture route not found", path: url.pathname });
      return;
    }
    const body = readFileSync(path);
    res.writeHead(200, {
      "access-control-allow-origin": "*",
      "cache-control": "no-store",
      "content-length": body.length,
      "content-type": contentType(path),
    });
    res.end(body);
  } catch (error) {
    state.serverErrors.push(String(error?.stack || error));
    if (!res.headersSent) {
      json(res, 500, { error: String(error?.message || error) });
    } else {
      res.end();
    }
  }
});

async function listen() {
  await new Promise((resolveListen, rejectListen) => {
    server.once("error", rejectListen);
    server.listen(0, "127.0.0.1", resolveListen);
  });
  const address = server.address();
  assert(address && typeof address === "object", "fixture server did not bind");
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

function frameFor(page, target) {
  return page.frames().find((frame) => frame.url().includes(`/apps/${target}/`)) || null;
}

async function waitFor(check, timeoutMs, label) {
  const startedAt = Date.now();
  while (Date.now() - startedAt <= timeoutMs) {
    if (await check()) {
      return;
    }
    await new Promise((resolveWait) => setTimeout(resolveWait, 50));
  }
  throw new Error(`timed out waiting for ${label}`);
}

async function postGuiCommandFromHost(page, descriptor) {
  await page.evaluate(({ value }) => {
    const frame = document.querySelector("#active-shell-frame");
    frame?.contentWindow?.postMessage({
      type: "home:gui-command",
      command: "attach-authorized-target",
      descriptor: value,
    }, "*");
  }, { value: descriptor });
}

async function homeWindowSnapshot(homeGui, target, { rememberWallet = false } = {}) {
  return homeGui.evaluate(({ targetId, remember }) => {
    const node = document.querySelector(`.window[data-target="${targetId}"]`);
    const frame = node?.querySelector(".window-frame");
    if (!node || !frame) {
      return null;
    }
    if (remember) {
      globalThis.__walletContinuityWindow = node;
      globalThis.__walletContinuityFrame = frame;
      globalThis.__walletContinuityContentWindow = frame.contentWindow;
    }
    const rect = node.getBoundingClientRect();
    return {
      id: node.dataset.windowId || "",
      target: node.dataset.target || "",
      route: frame.dataset.route || frame.getAttribute("src") || "",
      hidden:
        node.classList.contains("hidden") ||
        node.getAttribute("aria-hidden") !== "false",
      active: node.classList.contains("window-active"),
      maximized: node.dataset.maximized === "true",
      rect: {
        x: rect.x,
        y: rect.y,
        width: rect.width,
        height: rect.height,
        right: rect.right,
        bottom: rect.bottom,
      },
      walletWindowPreserved:
        !remember && globalThis.__walletContinuityWindow === node,
      walletFramePreserved:
        !remember && globalThis.__walletContinuityFrame === frame,
      walletContentWindowPreserved:
        !remember &&
        globalThis.__walletContinuityContentWindow === frame.contentWindow,
      distinctFromWalletFrame:
        !remember && globalThis.__walletContinuityFrame !== frame,
      distinctFromWalletContentWindow:
        !remember &&
        globalThis.__walletContinuityContentWindow !== frame.contentWindow,
    };
  }, { targetId: target, remember: rememberWallet });
}

async function walletUiSnapshot(wallet, { remember = false } = {}) {
  return wallet.evaluate(({ rememberState }) => {
    if (rememberState) {
      globalThis.__walletContinuityState = { marker: "wallet-state-preserved" };
    }
    return {
      route: window.location.href,
      settingsOpen:
        document.querySelector("#wallet-settings-drawer")?.hidden === false,
      statePreserved:
        !rememberState &&
        globalThis.__walletContinuityState?.marker === "wallet-state-preserved",
    };
  }, { rememberState: remember });
}

function rectsEqual(left, right) {
  return ["x", "y", "width", "height"].every(
    (key) => Math.abs(Number(left?.[key]) - Number(right?.[key])) < 0.5,
  );
}

function rectangleIntersectionRatio(subject, overlay) {
  const width = Math.max(
    0,
    Math.min(subject.right, overlay.right) - Math.max(subject.x, overlay.x),
  );
  const height = Math.max(
    0,
    Math.min(subject.bottom, overlay.bottom) - Math.max(subject.y, overlay.y),
  );
  const area = Math.max(1, subject.width * subject.height);
  return (width * height) / area;
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
      "--host-resolver-rules=MAP * ~NOTFOUND, EXCLUDE 127.0.0.1, EXCLUDE localhost",
      "--no-first-run",
      "--no-proxy-server",
    ],
  });
  const context = await browser.newContext();
  const page = await context.newPage();
  const pageErrors = [];
  page.on("pageerror", (error) => pageErrors.push(String(error?.stack || error)));

  await page.goto(`${origin}/apps/home/`, { waitUntil: "domcontentloaded" });
  await page.waitForFunction(
    () => document.body.dataset.homeStatus === "ready",
    null,
    { timeout: 15_000 },
  );
  await waitFor(() => frameFor(page, "home-gui"), 15_000, "real Home GUI frame");
  const homeGui = frameFor(page, "home-gui");
  await homeGui.dblclick('#desktop-shortcut-wallet');
  await waitFor(() => frameFor(page, "wallet"), 15_000, "real Wallet frame");
  const wallet = frameFor(page, "wallet");
  await wallet.waitForSelector("#wallet-settings-open", { timeout: 15_000 });
  const connectorLabels = Object.freeze({
    "wallet-metamask": "Connect wallet",
    "wallet-unisat": "Connect UniSat",
  });
  const walletWindowBeforeConnectors = await homeWindowSnapshot(
    homeGui,
    "wallet",
    { rememberWallet: true },
  );
  const walletUiBeforeConnectors = await walletUiSnapshot(wallet, {
    remember: true,
  });
  assert(walletWindowBeforeConnectors, "Home GUI did not expose the Wallet window");
  assert(walletUiBeforeConnectors.settingsOpen === false, "Wallet fixture opened settings early");

  for (const [target, connectLabel] of Object.entries(connectorLabels)) {
    await wallet.evaluate(({ connectorTarget }) => {
      const drawer = document.querySelector("#wallet-settings-drawer");
      if (drawer?.hidden !== false) {
        document.querySelector("#wallet-settings-open")?.click();
      }
      document
        .querySelector(`[data-wallet-open-method="${connectorTarget}"]`)
        ?.click();
    }, { connectorTarget: target });
    await waitFor(() => frameFor(page, target), 15_000, `${target} connector UI`);
    const connector = frameFor(page, target);
    await connector.waitForSelector("#wallet-connect", { timeout: 15_000 });
    assert(
      await connector.locator("#wallet-connect").textContent() === connectLabel,
      `${target} did not render its credential-free UI`,
    );

    const walletWindowDuringConnector = await homeWindowSnapshot(
      homeGui,
      "wallet",
    );
    const connectorWindow = await homeWindowSnapshot(homeGui, target);
    const walletUiDuringConnector = await walletUiSnapshot(wallet);
    const viewport = await homeGui.evaluate(() => ({
      width: window.innerWidth,
      height: window.innerHeight,
    }));
    assert(
      walletWindowDuringConnector?.id === walletWindowBeforeConnectors.id &&
        connectorWindow?.id &&
        connectorWindow.id !== walletWindowBeforeConnectors.id,
      `${target} replaced the Wallet window identity`,
      { walletWindowBeforeConnectors, walletWindowDuringConnector, connectorWindow },
    );
    assert(
      walletWindowDuringConnector.walletWindowPreserved === true &&
        walletWindowDuringConnector.walletFramePreserved === true &&
        walletWindowDuringConnector.walletContentWindowPreserved === true &&
        connectorWindow.distinctFromWalletFrame === true &&
        connectorWindow.distinctFromWalletContentWindow === true,
      `${target} did not retain distinct Wallet and connector frames`,
      { walletWindowDuringConnector, connectorWindow },
    );
    assert(
      walletWindowDuringConnector.hidden === false &&
        walletWindowDuringConnector.route === walletWindowBeforeConnectors.route &&
        rectsEqual(
          walletWindowDuringConnector.rect,
          walletWindowBeforeConnectors.rect,
        ),
      `${target} hid, navigated, or resized the Wallet window`,
      { walletWindowBeforeConnectors, walletWindowDuringConnector },
    );
    assert(
      walletUiDuringConnector.route === walletUiBeforeConnectors.route &&
        walletUiDuringConnector.settingsOpen === true &&
        walletUiDuringConnector.statePreserved === true,
      `${target} replaced or reset the Wallet frame state`,
      { walletUiBeforeConnectors, walletUiDuringConnector },
    );
    assert(
      connectorWindow.hidden === false &&
        connectorWindow.active === true &&
        connectorWindow.maximized === false &&
        connectorWindow.rect.width >= 320 &&
        connectorWindow.rect.width <= 520 &&
        connectorWindow.rect.height >= 220 &&
        connectorWindow.rect.height <= 620 &&
        connectorWindow.rect.x >= 0 &&
        connectorWindow.rect.y >= 0 &&
        connectorWindow.rect.right <= viewport.width &&
        connectorWindow.rect.bottom <= viewport.height &&
        rectangleIntersectionRatio(
          walletWindowDuringConnector.rect,
          connectorWindow.rect,
        ) < 0.55,
      `${target} did not open as a bounded, non-covering connector window`,
      { viewport, walletWindowDuringConnector, connectorWindow },
    );
    assert(
      await homeGui.locator(`.window[data-target="${target}"]`).count() === 1,
      `${target} click attached more than one connector window`,
    );

    const connectorClose = homeGui.locator(
      `.window[data-target="${target}"] [data-action="close"]`,
    );
    assert(
      await connectorClose.count() === 1,
      `${target} did not expose exactly one independent close control`,
    );
    await page.waitForTimeout(2_700);
    await connectorClose.click();
    await waitFor(
      async () =>
        await homeGui.locator(`.window[data-target="${target}"]`).count() === 0,
      15_000,
      `${target} independent close`,
    );
    const walletWindowAfterClose = await homeWindowSnapshot(homeGui, "wallet");
    const walletUiAfterClose = await walletUiSnapshot(wallet);
    assert(
      walletWindowAfterClose?.active === true &&
        walletWindowAfterClose.hidden === false &&
        walletWindowAfterClose.id === walletWindowBeforeConnectors.id &&
        walletWindowAfterClose.route === walletWindowBeforeConnectors.route &&
        walletWindowAfterClose.walletWindowPreserved === true &&
        walletWindowAfterClose.walletFramePreserved === true &&
        walletWindowAfterClose.walletContentWindowPreserved === true &&
        rectsEqual(walletWindowAfterClose.rect, walletWindowBeforeConnectors.rect) &&
        walletUiAfterClose.route === walletUiBeforeConnectors.route &&
        walletUiAfterClose.settingsOpen === true &&
        walletUiAfterClose.statePreserved === true,
      `${target} close did not return focus to the unchanged Wallet`,
      {
        walletWindowBeforeConnectors,
        walletWindowAfterClose,
        walletUiBeforeConnectors,
        walletUiAfterClose,
      },
    );
  }

  assert(state.launches["home-gui"] === 1, "Home launched multiple GUI roots", state);
  assert(state.launches.wallet === 1, "Home launched Wallet more than once", state);
  assert(
    state.launches["wallet-metamask"] === 1 &&
      state.launches["wallet-unisat"] === 1,
    "one Wallet click did not produce exactly one Runtime connector launch",
    state,
  );
  assert(
    await homeGui.locator('.window[data-target="wallet-metamask"]').count() === 0 &&
      await homeGui.locator('.window[data-target="wallet-unisat"]').count() === 0,
    "Home GUI did not close connector windows independently",
  );

  const windowCountBeforeRejectedDescriptors = await homeGui.locator(".window").count();
  await postGuiCommandFromHost(page, {
    schema: "elastos.home.authorized-target-attachment/v1",
    receipt_id: "forged-target",
    target: "wallet-walletconnect",
    title: "WalletConnect",
    attach_kind: "iframe",
    route:
      `/apps/wallet-walletconnect/?home_origin=${encodeURIComponent(origin)}` +
      "#home_token=forged",
  });
  await postGuiCommandFromHost(page, {
    schema: "elastos.home.authorized-target-attachment/v1",
    receipt_id: "substituted-route",
    target: "wallet-metamask",
    title: "MetaMask",
    attach_kind: "iframe",
    route:
      `/apps/wallet-unisat/?home_origin=${encodeURIComponent(origin)}` +
      "#home_token=substituted",
  });
  await wallet.evaluate((targetOrigin) => {
    window.parent.postMessage({
      type: "home:gui-command",
      command: "attach-authorized-target",
      descriptor: {
        schema: "elastos.home.authorized-target-attachment/v1",
        receipt_id: "wrong-source",
        target: "wallet-metamask",
        title: "MetaMask",
        attach_kind: "iframe",
        route:
          `/apps/wallet-metamask/?home_origin=${encodeURIComponent(targetOrigin)}` +
          "#home_token=wrong-source",
      },
    }, targetOrigin);
  }, origin);
  await page.waitForTimeout(100);
  assert(
    await homeGui.locator(".window").count() === windowCountBeforeRejectedDescriptors,
    "forged, substituted, or wrong-source descriptor attached a connector window",
  );
  assert(
    state.launches["wallet-metamask"] === 1 &&
      state.launches["wallet-unisat"] === 1,
    "descriptor rejection invoked the generic Runtime launch path",
    state,
  );

  const replayDescriptor = {
    schema: "elastos.home.authorized-target-attachment/v1",
    receipt_id: "bounded-replay-proof",
    target: "wallet-metamask",
    title: "MetaMask",
    attach_kind: "iframe",
    route:
      `/apps/wallet-metamask/?home_origin=${encodeURIComponent(origin)}` +
      "#home_token=fixture-replay-token",
  };
  await postGuiCommandFromHost(page, replayDescriptor);
  await waitFor(
    async () =>
      await homeGui.locator('.window[data-target="wallet-metamask"]').count() === 1,
    15_000,
    "first bounded descriptor attachment",
  );
  await postGuiCommandFromHost(page, replayDescriptor);
  await page.waitForTimeout(100);
  assert(
    await homeGui.locator('.window[data-target="wallet-metamask"]').count() === 1,
    "Home GUI accepted a replayed authorized attachment descriptor",
  );
  assert(
    state.launches["wallet-metamask"] === 1,
    "authorized attachment replay invoked Runtime launch",
    state,
  );

  const persistedState = JSON.stringify(state.homeStateWrites);
  assert(
    !persistedState.includes("wallet-metamask") &&
      !persistedState.includes("wallet-unisat") &&
      !persistedState.includes("home_token") &&
      !persistedState.includes("fixture-metamask-token") &&
      !persistedState.includes("fixture-unisat-token") &&
      !persistedState.includes("fixture-replay-token"),
    "Home persisted a hidden connector descriptor or launch token",
    state.homeStateWrites,
  );
  assert(state.connectorEffects === 0, "credential-free UI proof invoked a connector effect");
  assert(state.serverErrors.length === 0, "fixture server recorded errors", state.serverErrors);
  assert(pageErrors.length === 0, "real Home/GUI/Wallet source raised page errors", pageErrors);

  console.log(
    "[home-wallet-connector-launch-headless] PASS " +
      `wallet_launches=${state.launches.wallet} ` +
      `metamask_launches=${state.launches["wallet-metamask"]} ` +
      `unisat_launches=${state.launches["wallet-unisat"]} ` +
      `connector_effects=${state.connectorEffects}`,
  );
} finally {
  await browser?.close().catch(() => {});
  await new Promise((resolveClose) => server.close(resolveClose));
}
