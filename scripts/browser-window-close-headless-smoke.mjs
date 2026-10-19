#!/usr/bin/env node

import { createServer } from "node:http";
import { existsSync, readFileSync } from "node:fs";
import { extname, isAbsolute, join, relative, resolve, sep } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { DEFAULT_URL } from "../capsules/browser/browser/browser-runtime-api.js";

const repoRoot = fileURLToPath(new URL("../", import.meta.url));
const browserInstance = "browser:0123456789abcdef0123456789abcdef";
const browserToken = "browser-window-close-headless-token";
const browserEngineId = "close-proof-engine";
const remoteExitId = "close-proof-exit";
const state = {
  authorityErrors: [],
  closeCalls: [],
  closeOwnershipBrowserInstance: browserInstance,
  freshStartAllowed: true,
  openCalls: 0,
  openRequests: 0,
  openRequestBodies: [],
  openStatusRequests: 0,
  pages: {},
  resetCalls: [],
  releaseInitialOpen: null,
  serverErrors: [],
  statusCalls: [],
  requests: [],
  unknownOpenResult: null,
};

function assert(condition, message, details = undefined) {
  if (condition) return;
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

function json(res, status, value) {
  send(res, status, "application/json; charset=utf-8", JSON.stringify(value));
}

function contentType(path) {
  return {
    ".css": "text/css; charset=utf-8",
    ".html": "text/html; charset=utf-8",
    ".js": "text/javascript; charset=utf-8",
    ".svg": "image/svg+xml",
  }[extname(path)] || "application/octet-stream";
}

function staticPath(pathname) {
  const roots = [
    ["/apps/browser/", join(repoRoot, "capsules/browser/browser")],
    ["/apps/home/", join(repoRoot, "capsules/home/browser")],
  ];
  for (const [prefix, root] of roots) {
    if (!pathname.startsWith(prefix)) continue;
    const suffix = decodeURIComponent(pathname.slice(prefix.length)) || "index.html";
    const candidate = resolve(root, suffix);
    const escaped = relative(root, candidate);
    if (escaped === ".." || escaped.startsWith(`..${sep}`) || isAbsolute(escaped)) {
      return null;
    }
    return candidate;
  }
  return null;
}

async function readBody(req) {
  const chunks = [];
  for await (const chunk of req) chunks.push(chunk);
  const text = Buffer.concat(chunks).toString("utf8");
  return text ? JSON.parse(text) : null;
}

function topDocument(origin) {
  return `<!doctype html>
<html lang="en">
  <head><meta charset="utf-8"><title>Browser close host proof</title></head>
  <body>
    <iframe
      id="shell"
      sandbox="allow-scripts"
      src="/shell?home_origin=${encodeURIComponent(origin)}"
    ></iframe>
  </body>
</html>`;
}

function browserRoute(homeOrigin, includeUrl = true) {
  const query = new URLSearchParams({
    browser_instance: browserInstance,
    browser_engine_id: browserEngineId,
    remote_exit_id: remoteExitId,
    display_mode: "webrtc_remote_display",
    guarantee_level: "mechanism_microvm",
    home_origin: homeOrigin,
  });
  if (includeUrl) query.set("url", "https://example.com/close-proof");
  return `/apps/browser/?${query}#home_token=${browserToken}`;
}

function shellDocument(homeOrigin) {
  return `<!doctype html>
<html lang="en">
  <head><meta charset="utf-8"><title>Opaque Home GUI close proof</title></head>
  <body>
    <iframe id="browser" sandbox="allow-scripts allow-forms" src="${browserRoute(homeOrigin)}"></iframe>
    <iframe id="attacker" sandbox="allow-scripts" src="/attacker"></iframe>
    <script>
      const browser = document.querySelector("#browser");
      const results = [];
      window.addEventListener("message", (event) => {
        if (
          event.source === browser.contentWindow &&
          event.origin === "null" &&
          event.data?.type === "elastos.browser.window-close.result/v1"
        ) {
          results.push(event.data);
        }
      });
      window.__browserCloseProof = {
        browserRoute: ${JSON.stringify(browserRoute(homeOrigin))},
        freshRoute: ${JSON.stringify(browserRoute(homeOrigin, false))},
        navigateAway() {
          browser.src = "/blank";
        },
        reopen() {
          browser.src = this.browserRoute;
        },
        openFresh() {
          browser.src = this.freshRoute;
        },
        resultCount() {
          return results.length;
        },
        results() {
          return results;
        },
        send(message) {
          browser.contentWindow.postMessage(message, "*");
        },
      };
    </script>
  </body>
</html>`;
}

function attackerDocument() {
  return `<!doctype html>
<html lang="en">
  <head><meta charset="utf-8"><title>Wrong source</title></head>
  <body>
    <script>
      window.__sendForgedBrowserClose = (message) => {
        window.parent.frames[0].postMessage(message, "*");
      };
    </script>
  </body>
</html>`;
}

function openResult(url) {
  state.openCalls += 1;
  const ordinal = state.openCalls;
  const pageId = `page-${ordinal}`;
  state.pages[pageId] = url;
  return {
    schema: "elastos.browser.open-result/v1",
    stream_session: {
      schema: "elastos.net.stream-session/v1",
      backend: remoteExitId,
      direct_network: false,
    },
    engine_page: {
      schema: "elastos.browser.engine.page/v1",
      page_id: pageId,
      url,
      actual_url: url,
      title: "Close proof",
      display_session: {
        schema: "elastos.browser.display-session/v1",
        mode: "webrtc_remote_display",
        signaling_url: `/api/apps/browser/pages/${pageId}/signal`,
        offerer: "browser",
        input: "runtime_route",
        input_protocol: "elastos_json",
        ice_servers: [],
        audio: false,
      },
      view: {},
    },
    runtime_cleanup: {
      schema: "elastos.browser.cleanup-handle/v1",
      id: `cleanup-${ordinal}`,
    },
  };
}

async function handleApi(req, res, url) {
  if (url.pathname.startsWith("/api/")) state.requests.push(`${req.method} ${url.pathname}`);
  if (req.method === "OPTIONS" && url.pathname.startsWith("/api/")) {
    res.writeHead(204, {
      "access-control-allow-headers": "content-type,x-elastos-home-token",
      "access-control-allow-methods": "GET,POST,OPTIONS",
      "access-control-allow-origin": "*",
    });
    res.end();
    return true;
  }
  if (url.pathname.startsWith("/api/") && req.headers["x-elastos-home-token"] !== browserToken) {
    state.authorityErrors.push(`${req.method} ${url.pathname}: wrong Browser token`);
    json(res, 403, { error: "invalid fixture Browser authority" });
    return true;
  }
  if (url.pathname === "/api/apps/browser/summary" && req.method === "GET") {
    json(res, 200, {
      schema: "elastos.browser.runtime/v1",
      sessions: {
        schema: "elastos.browser.session-capacity/v1",
        status: "configured",
        fresh_start_allowed: state.freshStartAllowed,
        recoverable_page: null,
        window_close_ownership: {
          schema: "elastos.browser.window-close-ownership/v1",
          browser_instance: state.closeOwnershipBrowserInstance,
          state: "absent",
        },
      },
      engine_adapter: {
        adapters: [{
          id: browserEngineId,
          backing_substrate: "local_microvm",
          direct_network: false,
          wallet_injection: false,
        }],
      },
      net: { exit_provider: { remote_carrier_exits: [{ id: remoteExitId }] } },
    });
    return true;
  }
  if (url.pathname === "/api/apps/browser/profile/reset" && req.method === "POST") {
    state.resetCalls.push({
      token: req.headers["x-elastos-home-token"],
      closeCallsAtReset: state.closeCalls.length,
      openRequestsAtReset: state.openRequests,
    });
    json(res, 200, { schema: "elastos.browser.profile-reset/v1", status: "ok", removed_profile_disk: true });
    return true;
  }
  if (url.pathname === "/api/apps/browser/open" && req.method === "POST") {
    const body = await readBody(req);
    if (
      body?.browser_instance !== browserInstance ||
      body?.adapter_id !== browserEngineId ||
      body?.remote_exit_id !== remoteExitId
    ) {
      state.authorityErrors.push("open changed the Browser instance, Engine, or Exit selection");
      json(res, 403, { error: "invalid fixture Browser owner" });
      return true;
    }
    state.openRequests += 1;
    state.openRequestBodies.push(body);
    if (state.openRequests === 1) {
      await new Promise((resolveOpen) => {
        state.releaseInitialOpen = resolveOpen;
      });
      json(res, 200, openResult(body.url));
      return true;
    }
    if (state.openRequests === 2) {
      state.unknownOpenResult = openResult(body.url);
      json(res, 202, {
        schema: "elastos.browser.open-accepted/v1",
        status: "pending",
        open_id: "open-unknown-owner",
        status_url: "/api/apps/browser/open/open-unknown-owner",
      });
      return true;
    }
    if (state.openRequests === 3) {
      json(res, 200, openResult(body.url));
      return true;
    }
    json(res, 409, { error: "unexpected Browser open" });
    return true;
  }
  if (
    url.pathname === "/api/apps/browser/open/open-unknown-owner" &&
    req.method === "GET"
  ) {
    state.openStatusRequests += 1;
    if (state.openStatusRequests === 1) {
      json(res, 503, { error: "simulated lost open poll" });
      return true;
    }
    if (state.openStatusRequests === 2) {
      json(res, 200, {
        schema: "elastos.browser.open-status/v1",
        open_id: "open-unknown-owner",
        status: "pending",
      });
      return true;
    }
    json(res, 200, {
      schema: "elastos.browser.open-status/v1",
      open_id: "open-unknown-owner",
      status: "completed",
      result: state.unknownOpenResult,
    });
    return true;
  }
  const signal = url.pathname.match(/^\/api\/apps\/browser\/pages\/([^/]+)\/signal$/);
  if (signal && req.method === "POST") {
    const body = await readBody(req);
    if (body?.type === "candidate" || body?.type === "end_of_candidates") {
      json(res, 200, {
        schema: "elastos.browser.webrtc-signal-ack/v1",
        type: body.type,
        accepted: true,
        candidates: [],
        end_of_candidates: true,
      });
      return true;
    }
    assert(body?.type === "offer", "fixture received an unexpected display signal", body);
    json(res, 200, {
      schema: "elastos.browser.webrtc-answer/v1",
      type: "answer",
      sdp: "v=0\r\n",
      candidates: [],
      end_of_candidates: true,
    });
    return true;
  }
  const close = url.pathname.match(/^\/api\/apps\/browser\/pages\/([^/]+)\/close$/);
  if (close && req.method === "POST") {
    const pageId = decodeURIComponent(close[1]);
    const body = await readBody(req);
    if (
      body?.schema !== "elastos.browser.close-request/v2" ||
      body?.browser_instance !== browserInstance ||
      body?.cleanup_id !== pageId.replace("page-", "cleanup-") ||
      !Object.hasOwn(state.pages, pageId)
    ) {
      state.authorityErrors.push(`close changed the exact Runtime owner for ${pageId}`);
      json(res, 403, { error: "invalid fixture Browser cleanup authority" });
      return true;
    }
    state.closeCalls.push({ pageId, body, token: req.headers["x-elastos-home-token"] });
    const alreadyReaped = pageId === "page-2";
    json(res, 200, {
      schema: "elastos.browser.close-result/v1",
      page_id: pageId,
      cleanup_id: body?.cleanup_id || "",
      closed: !alreadyReaped,
      already_closed: alreadyReaped,
    });
    return true;
  }
  const status = url.pathname.match(/^\/api\/apps\/browser\/pages\/([^/]+)\/status$/);
  if (status && req.method === "GET") {
    const pageId = decodeURIComponent(status[1]);
    state.statusCalls.push(pageId);
    json(res, 200, {
      schema: "elastos.browser.page-status/v1",
      page_id: pageId,
      direct_network: false,
      actual_url: state.pages[pageId],
      title: `Observed ${pageId}`,
      audio: false,
      video: false,
    });
    return true;
  }
  return false;
}

const server = createServer(async (req, res) => {
  try {
    const url = new URL(req.url || "/", "http://127.0.0.1");
    if (await handleApi(req, res, url)) return;
    if (url.pathname === "/") {
      send(res, 200, "text/html; charset=utf-8", topDocument(`http://${req.headers.host}`));
      return;
    }
    if (url.pathname === "/shell") {
      send(res, 200, "text/html; charset=utf-8", shellDocument(url.searchParams.get("home_origin") || ""));
      return;
    }
    if (url.pathname === "/attacker") {
      send(res, 200, "text/html; charset=utf-8", attackerDocument());
      return;
    }
    if (url.pathname === "/blank") {
      send(res, 200, "text/html; charset=utf-8", "<!doctype html><title>Blank</title>");
      return;
    }
    const path = staticPath(url.pathname);
    if (path && existsSync(path)) {
      if (path.endsWith(".html")) {
        res.setHeader("content-security-policy", "sandbox allow-scripts allow-forms allow-downloads");
      }
      send(res, 200, contentType(path), readFileSync(path));
      return;
    }
    send(res, 404, "text/plain; charset=utf-8", "not found");
  } catch (error) {
    state.serverErrors.push(String(error?.stack || error));
    send(res, 500, "text/plain; charset=utf-8", String(error?.stack || error));
  }
});

async function listen() {
  await new Promise((resolveListen, rejectListen) => {
    server.once("error", rejectListen);
    server.listen(0, "127.0.0.1", resolveListen);
  });
  const address = server.address();
  assert(address && typeof address === "object", "Browser close fixture did not bind");
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

async function waitForFrame(page, predicate, label) {
  for (let attempt = 0; attempt < 200; attempt += 1) {
    const frame = page.frames().find(predicate);
    if (frame) return frame;
    await new Promise((resolveWait) => setTimeout(resolveWait, 10));
  }
  throw new Error(`${label} frame is missing`);
}

const origin = await listen();
let chromiumBrowser = null;
try {
  const imported = await import(playwrightSpecifier());
  const { chromium } = imported.default || imported;
  chromiumBrowser = await chromium.launch({
    headless: true,
    ...(process.env.ELASTOS_BROWSER_CLOSE_CHROMIUM_EXECUTABLE
      ? { executablePath: process.env.ELASTOS_BROWSER_CLOSE_CHROMIUM_EXECUTABLE }
      : {}),
    args: ["--disable-background-networking", "--no-first-run", "--no-proxy-server"],
  });
  const context = await chromiumBrowser.newContext();
  await context.addInitScript(() => {
    class FakePeerConnection {
      constructor() {
        this.connectionState = "new";
        this.iceConnectionState = "new";
        this.iceGatheringState = "complete";
        this.localDescription = null;
      }
      addEventListener() {}
      addIceCandidate() { return Promise.resolve(); }
      addTransceiver() {}
      createDataChannel() { return { readyState: "open", close() {} }; }
      close() { this.connectionState = "closed"; }
      createOffer() { return Promise.resolve({ type: "offer", sdp: "v=0\r\n" }); }
      getStats() { return Promise.resolve(new Map()); }
      setLocalDescription(description) {
        this.localDescription = description;
        return Promise.resolve();
      }
      setRemoteDescription() { return Promise.resolve(); }
    }
    globalThis.RTCPeerConnection = FakePeerConnection;
  });
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
  await page.goto(origin, { waitUntil: "domcontentloaded" });

  const shellFrame = await waitForFrame(
    page,
    (frame) => frame.url().includes("/shell?"),
    "opaque Home GUI proof",
  );
  await shellFrame.waitForFunction(() => Boolean(window.__browserCloseProof));
  let browserFrame = await waitForFrame(
    page,
    (frame) => frame.url().includes("/apps/browser/"),
    "actual Browser capsule",
  );
  for (let attempt = 0; attempt < 500 && state.openRequests < 1; attempt += 1) {
    await new Promise((resolveWait) => setTimeout(resolveWait, 10));
  }
  assert(state.openRequests === 1, "initial Browser open did not enter the fixture", {
    frames: page.frames().map((frame) => frame.url()),
    pageErrors,
    consoleErrors,
    failedRequests,
    serverErrors: state.serverErrors,
    requests: state.requests,
    browserText: await browserFrame.locator("body").innerText(),
  });
  const transitionRequest = {
    type: "elastos.browser.window-close.request/v1",
    requestId: "headless-close-during-open",
    homeToken: browserToken,
    browserInstance,
  };
  await shellFrame.evaluate(
    (message) => window.__browserCloseProof.send(message),
    transitionRequest,
  );
  await shellFrame.waitForFunction(() => window.__browserCloseProof.resultCount() === 1);
  const [transitionResult] = await shellFrame.evaluate(
    () => window.__browserCloseProof.results(),
  );
  assert(
    state.closeCalls.length === 0 &&
      transitionResult.state === "pending" &&
      transitionResult.terminalKind === "" &&
      transitionResult.reason === "runtime_open_in_flight",
    "Browser claimed terminal cleanup during an ownership-changing open",
    { state, transitionResult },
  );
  state.releaseInitialOpen();
  await browserFrame.waitForFunction(
    () => window.__elastosBrowserCurrentPageId === "page-1",
  );
  assert((await shellFrame.evaluate(() => self.origin)) === "null", "Home GUI fixture is not opaque");
  assert((await browserFrame.evaluate(() => self.origin)) === "null", "Browser fixture is not opaque");

  await shellFrame.evaluate(() => window.__browserCloseProof.navigateAway());
  await waitForFrame(
    page,
    (frame) => frame.url().includes("/blank"),
    "navigated-away Browser",
  );
  await new Promise((resolveWait) => setTimeout(resolveWait, 100));
  assert(
    state.closeCalls.length === 0,
    "generic Browser iframe navigation called Runtime close",
    state,
  );
  await shellFrame.evaluate(() => window.__browserCloseProof.reopen());
  browserFrame = await waitForFrame(
    page,
    (frame) => frame.url().includes("/apps/browser/"),
    "reopened Browser capsule",
  );
  try {
    await browserFrame.waitForFunction(
      () =>
        document.body?.dataset.loading === "false" &&
        !window.__elastosBrowserCurrentPageId,
      null,
      { timeout: 10_000 },
    );
  } catch (error) {
    const browserState = await browserFrame.evaluate(() => ({
      currentPageId: window.__elastosBrowserCurrentPageId,
      loading: document.body?.dataset.loading ?? null,
      status: document.querySelector("#browser-status")?.textContent || "",
    })).catch(() => null);
    throw new Error(`unknown-owner Browser did not settle after the lost poll: ${JSON.stringify({
      browserState,
      pageErrors,
      consoleErrors,
      failedRequests,
      state,
    })}`, { cause: error });
  }
  assert(
    state.openStatusRequests === 1 && state.closeCalls.length === 0,
    "fixture did not lose the first async open poll before owner capture",
    state,
  );

  const pendingRequest = {
    type: "elastos.browser.window-close.request/v1",
    requestId: "headless-close-unknown-pending",
    homeToken: browserToken,
    browserInstance,
  };
  for (const forged of [
    { ...pendingRequest, homeToken: "wrong-token" },
    { ...pendingRequest, browserInstance: "browser:wrong" },
    { ...pendingRequest, type: "browser:close" },
    { ...pendingRequest, extra: true },
  ]) {
    await shellFrame.evaluate((message) => window.__browserCloseProof.send(message), forged);
  }
  const attackerFrame = await waitForFrame(
    page,
    (frame) => frame.url().includes("/attacker"),
    "wrong-source proof",
  );
  await attackerFrame.evaluate(
    (message) => window.__sendForgedBrowserClose(message),
    pendingRequest,
  );
  await new Promise((resolveWait) => setTimeout(resolveWait, 100));
  assert(
    state.closeCalls.length === 0 &&
      (await shellFrame.evaluate(() => window.__browserCloseProof.resultCount())) === 1,
    "a source, token, instance, type, or shape substitution reached Runtime close",
    state,
  );

  await shellFrame.evaluate(
    (message) => window.__browserCloseProof.send(message),
    pendingRequest,
  );
  await shellFrame.waitForFunction(() => window.__browserCloseProof.resultCount() === 2);
  const pendingResult = await shellFrame.evaluate(
    () => window.__browserCloseProof.results().at(-1),
  );
  assert(
    state.closeCalls.length === 0 &&
      pendingResult.state === "pending" &&
      pendingResult.terminalKind === "" &&
      pendingResult.reason === "runtime_open_status_pending",
    "ownerless close claimed terminal while the exact async open was pending",
    { state, pendingResult },
  );

  const request = {
    ...pendingRequest,
    requestId: "headless-close-unknown-completed",
  };
  await shellFrame.evaluate((message) => window.__browserCloseProof.send(message), request);
  await shellFrame.waitForFunction(() => window.__browserCloseProof.resultCount() === 4);
  const results = await shellFrame.evaluate(() => window.__browserCloseProof.results());
  const binding = results.at(-2);
  const result = results.at(-1);
  assert(
    state.openRequests === 2 &&
      state.closeCalls.length === 1 &&
      state.closeCalls[0].pageId === "page-2" &&
      state.closeCalls[0].body?.schema === "elastos.browser.close-request/v2" &&
      state.closeCalls[0].body?.cleanup_id === "cleanup-2" &&
      state.closeCalls[0].body?.browser_instance === browserInstance,
    "actual Browser did not close the exact Runtime owner once",
    state,
  );
  assert(
    binding.requestId === request.requestId &&
      binding.state === "pending" &&
      binding.pageId === "page-2" &&
      binding.generation === 0 &&
      binding.cleanupId === "cleanup-2" &&
      binding.terminalKind === "" &&
      binding.reason === "cleanup_in_flight",
    "old retained Browser frame did not bind its exact lifecycle before close",
    binding,
  );
  assert(
    Object.keys(result).sort().join(",") ===
      "browserInstance,cleanupId,generation,homeToken,pageId,reason,requestId,state,terminalKind,type" &&
      result.type === "elastos.browser.window-close.result/v1" &&
      result.requestId === request.requestId &&
      result.homeToken === browserToken &&
      result.browserInstance === browserInstance &&
      result.state === "terminal" &&
      result.pageId === "page-2" &&
      result.cleanupId === "cleanup-2" &&
      result.terminalKind === "already_absent" &&
      result.reason === "",
    "old retained Browser frame did not accept the exact already-absent reap receipt",
    result,
  );
  await shellFrame.evaluate(() => window.__browserCloseProof.openFresh());
  browserFrame = await waitForFrame(
    page,
    (frame) => frame.url().includes("/apps/browser/") && !new URL(frame.url()).searchParams.has("url"),
    "fresh Browser capsule",
  );
  await browserFrame.waitForFunction(() => document.body?.dataset.loading === "false");
  assert(
    state.openRequests === 3 &&
      state.openRequestBodies[2].url === DEFAULT_URL &&
      state.openRequestBodies[2].adapter_id === browserEngineId &&
      state.openRequestBodies[2].remote_exit_id === remoteExitId &&
      (await browserFrame.evaluate(() => window.__elastosBrowserCurrentPageId)) === "page-3",
    "a fresh Browser must open its default URL with the selected Engine and Exit",
    state.openRequestBodies,
  );
  await browserFrame.waitForFunction(
    () => document.title === "Observed page-3 - ElastOS Browser" &&
      window.__elastosBrowserCurrentPageId === "page-3",
    null,
    { timeout: 10_000 },
  );
  assert(state.statusCalls.includes("page-3"), "the fresh page did not complete a status observation", state.statusCalls);
  const opensBeforeReset = state.openRequests;
  const closesBeforeReset = state.closeCalls.length;
  async function assertResetUnchanged(action) {
    assert(
      state.resetCalls.length === 0 &&
        state.closeCalls.length === closesBeforeReset &&
        state.openRequests === opensBeforeReset &&
        (await browserFrame.evaluate(() => window.__elastosBrowserCurrentPageId)) === "page-3",
      `${action} changed the Browser profile or page`,
      state,
    );
  }
  await browserFrame.locator("#browser-settings").click();
  const confirmation = browserFrame.locator("#browser-profile-reset-confirmation");
  await browserFrame.locator("#browser-profile-reset").click();
  await confirmation.waitFor({ state: "visible" });
  await browserFrame.locator("#browser-profile-reset-cancel").click();
  await confirmation.waitFor({ state: "hidden" });
  await assertResetUnchanged("Cancel");
  await browserFrame.locator("#browser-profile-reset").click();
  await browserFrame.locator("#browser-profile-reset-cancel").press("Escape");
  await confirmation.waitFor({ state: "hidden" });
  await browserFrame.locator("#browser-settings").click();
  await assertResetUnchanged("Escape");
  await browserFrame.locator("#browser-profile-reset").click();
  await browserFrame.locator("#browser-settings-close").click();
  await browserFrame.locator("#browser-settings").click();
  await confirmation.waitFor({ state: "hidden" });
  await assertResetUnchanged("Closing Settings");
  await browserFrame.locator("#browser-profile-reset").click();
  await browserFrame.locator("#browser-profile-reset-commit").click();
  await browserFrame.locator("#browser-status").getByText("Browser profile reset. Open the address again.", { exact: true }).waitFor();
  assert(
    state.resetCalls.length === 1 &&
      state.resetCalls[0].token === browserToken &&
      state.resetCalls[0].closeCallsAtReset === closesBeforeReset + 1 &&
      state.resetCalls[0].openRequestsAtReset === opensBeforeReset &&
      state.closeCalls.length === closesBeforeReset + 1 &&
      state.closeCalls.at(-1).pageId === "page-3" &&
      state.closeCalls.at(-1).body.cleanup_id === "cleanup-3" &&
      state.openRequests === opensBeforeReset &&
      !(await browserFrame.evaluate(() => window.__elastosBrowserCurrentPageId)),
    "Confirmed reset must close the exact page before using window authority once, without a replacement open",
    { resetCalls: state.resetCalls, closeCalls: state.closeCalls, openRequests: state.openRequests },
  );
  await shellFrame.evaluate(
    (message) => window.__browserCloseProof.send(message),
    { ...request, requestId: "headless-close-exact-absence" },
  );
  await shellFrame.waitForFunction(() => window.__browserCloseProof.resultCount() === 5);
  const resetResult = await shellFrame.evaluate(() => window.__browserCloseProof.results().at(-1));
  assert(
    state.openRequests === opensBeforeReset && state.closeCalls.length === closesBeforeReset + 1 &&
      resetResult.state === "terminal" &&
      resetResult.terminalKind === "no_page" &&
      resetResult.pageId === "" &&
      resetResult.cleanupId === "",
    "Browser did not retain terminal page absence after profile reset",
    { resetResult, closeCalls: state.closeCalls },
  );

  // An independent window never starts a page. Only Runtime's exact ownership
  // receipt may make its close terminal, even when fresh starts are unavailable.
  state.freshStartAllowed = false;
  state.closeOwnershipBrowserInstance = "browser:another-window";
  await shellFrame.evaluate(() => window.__browserCloseProof.navigateAway());
  await waitForFrame(page, (frame) => frame.url().includes("/blank"), "reset Browser navigation");
  await shellFrame.evaluate(() => window.__browserCloseProof.openFresh());
  browserFrame = await waitForFrame(
    page,
    (frame) => frame.url().includes("/apps/browser/") && !new URL(frame.url()).searchParams.has("url"),
    "Browser without a page",
  );
  await browserFrame.waitForFunction(() => document.body?.dataset.loading === "false");
  assert(
    state.openRequests === opensBeforeReset &&
      !(await browserFrame.evaluate(() => window.__elastosBrowserCurrentPageId)),
    "Browser opened a page while Runtime withheld fresh-start permission",
    state,
  );
  await shellFrame.evaluate(
    (message) => window.__browserCloseProof.send(message),
    { ...request, requestId: "headless-close-other-window-absence" },
  );
  await shellFrame.waitForFunction(() => window.__browserCloseProof.resultCount() === 6);
  const unrelatedAbsence = await shellFrame.evaluate(() => window.__browserCloseProof.results().at(-1));
  assert(
    unrelatedAbsence.state === "pending" &&
      unrelatedAbsence.terminalKind === "" &&
      unrelatedAbsence.reason === "runtime_ownership_unproven",
    "Browser accepted another window's absence as its own terminal close",
    unrelatedAbsence,
  );
  state.closeOwnershipBrowserInstance = browserInstance;
  const freshRequest = { ...request, requestId: "headless-close-fresh-exact-absence" };
  await shellFrame.evaluate((message) => window.__browserCloseProof.send(message), freshRequest);
  await shellFrame.waitForFunction(() => window.__browserCloseProof.resultCount() === 7);
  const freshResult = await shellFrame.evaluate(() => window.__browserCloseProof.results().at(-1));
  assert(
    state.openRequests === opensBeforeReset && state.closeCalls.length === closesBeforeReset + 1 &&
      freshResult.requestId === freshRequest.requestId &&
      freshResult.homeToken === browserToken &&
      freshResult.browserInstance === browserInstance &&
      freshResult.state === "terminal" &&
      freshResult.terminalKind === "no_page" &&
      freshResult.pageId === "" &&
      freshResult.cleanupId === "",
    "Browser without a page did not accept Runtime's exact absence without closing another page",
    { freshResult, closeCalls: state.closeCalls },
  );
  assert(state.authorityErrors.length === 0, "Browser changed fixture authority", state.authorityErrors);
  assert(pageErrors.length === 0, "Browser close fixture raised page errors", pageErrors);
  assert(state.serverErrors.length === 0, "Browser close fixture server failed", state.serverErrors);
  console.log(JSON.stringify({
    schema: "elastos.browser.window-close-headless-smoke/v1",
    ok: true,
    close_calls: state.closeCalls.length,
    default_startup: state.openRequestBodies[2].url,
    fresh_page_status_observed: state.statusCalls.includes("page-3"),
    fresh_close: freshResult.terminalKind,
    unrelated_absence: unrelatedAbsence.state,
    unload_close_calls: 0,
    reset_calls: state.resetCalls.length,
  }));
} finally {
  await chromiumBrowser?.close();
  await new Promise((resolveClose) => server.close(resolveClose));
}
