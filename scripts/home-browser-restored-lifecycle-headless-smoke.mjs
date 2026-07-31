#!/usr/bin/env node

import { createServer } from "node:http";
import { existsSync, readFileSync } from "node:fs";
import { extname, isAbsolute, join, relative, resolve, sep } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const repoRoot = fileURLToPath(new URL("../", import.meta.url));
const browserContextId = "browser:00112233445566778899aabbccddeeff";
const browserInstanceId = "browser:11223344-5566-7788-99aa-bbccddeeff00";
const homeAuthorityToken = "fixture-home-authority-token";
const homeGuiToken = "fixture-home-gui-launch-token";
const browserOwnerToken = "fixture-browser-owner-launch-token";
const browserRefreshedToken = "fixture-browser-refreshed-launch-token";
const openId = "browser-open-fixture-0001";
const pageId = "browser-page-fixture-0001";
const principalId = "did:elastos:fixture-home-refresh";
const recoveredCleanupId = "browser-cleanup:fixture-recovered";
const openedCleanupId = "browser-cleanup:fixture-opened";

const state = {
  phase: "placeholder",
  homeGuiLaunches: { placeholder: 0, restored: 0 },
  browserLaunches: { placeholder: 0, restored: 0 },
  browserOpenRequests: 0,
  browserSummaryBindings: 0,
  browserOpenBindings: 0,
  browserOpenEffects: 0,
  browserOpenIntent: null,
  browserOpenCompleted: false,
  browserStatusPolls: 0,
  browserPageCount: 0,
  browserVmCount: 0,
  browserCleanupEffects: 0,
  activeCleanupId: null,
  homeStateWrites: [],
  errors: [],
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

function empty(res, status = 204) {
  res.writeHead(status, {
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
    ".mjs": "text/javascript; charset=utf-8",
    ".png": "image/png",
    ".svg": "image/svg+xml",
    ".webp": "image/webp",
  }[extname(path)] || "application/octet-stream";
}

function staticPath(pathname) {
  const roots = [
    ["/apps/home/", join(repoRoot, "capsules/home/browser")],
    ["/apps/home-gui/", join(repoRoot, "capsules/home-gui/browser")],
    ["/apps/browser/", join(repoRoot, "capsules/browser/browser")],
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
  let bytes = 0;
  for await (const chunk of req) {
    bytes += chunk.length;
    if (bytes > 1_048_576) {
      throw new Error("fixture request body exceeds 1 MiB");
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
    identity: {
      id: principalId,
      display_name: "Lifecycle Fixture",
    },
    authority: {
      signed_in: true,
      principal_id: principalId,
      proof_binding_id: "fixture-proof-binding",
    },
    browser_state: {
      schema: "elastos.home.browser-state/v1",
      principal_id: principalId,
      layout: null,
      recent_targets: ["browser"],
      session: {
        browser_context_id: browserContextId,
        root_shell: "home-gui",
        windows: [
          {
            target: "browser",
            active: true,
            hidden: false,
            maximized: true,
            x: 48,
            y: 60,
            width: 1120,
            height: 720,
            query: {
              browser_instance: browserInstanceId,
            },
          },
        ],
      },
    },
    active_shell: {
      active: "home-gui",
      candidates: [
        {
          name: "home-gui",
          title: "Desktop",
          launchable: true,
        },
      ],
    },
    appearance: {},
    runtime: {},
    site: {},
    room: {},
    people: {},
    services: {},
    notifications: [],
    desktop_objects: [],
    capsule_catalog: [],
    capsule_interfaces: [],
    targets: [
      {
        target: "browser",
        title: "Browser",
        description: "Private Runtime Browser",
        role: "app",
        target_kind: "app",
        attach_kind: "iframe",
      },
    ],
  };
}

function browserSummary() {
  return {
    sessions: {
      schema: "elastos.browser.session-capacity/v1",
      active_sessions: state.browserPageCount,
      launching_sessions: 0,
      total_sessions: state.browserPageCount,
      recoverable_page: state.activeCleanupId
        ? {
            schema: "elastos.browser.recoverable-page/v1",
            state: "active",
            page_id: pageId,
            cleanup: {
              schema: "elastos.browser.cleanup-handle/v1",
              id: state.activeCleanupId,
            },
            engine_page: {
              schema: "elastos.browser.engine.page/v1",
              page_id: pageId,
              url: "https://ela.city/",
            },
          }
        : null,
    },
    engine_adapter: {
      supported_display_modes: ["webrtc_remote_display"],
      supported_guarantee_levels: ["mechanism_microvm"],
      adapters: [
        {
          id: "fixture-engine",
          default: true,
          engine: "fixture-microvm",
          backing_substrate: "local_microvm",
          direct_network: false,
          wallet_injection: false,
          supported_display_modes: ["webrtc_remote_display"],
          supported_guarantee_levels: ["mechanism_microvm"],
        },
      ],
    },
    net: {
      exit_provider: {
        remote_carrier_exits: [],
      },
    },
  };
}

function browserOpenResult() {
  return {
    schema: "elastos.browser.open-result/v1",
    engine_page: {
      schema: "elastos.browser.engine.page/v1",
      page_id: pageId,
      url: "https://ela.city/",
      actual_url: "https://ela.city/",
      title: "ela.city",
      view: { width: 1280, height: 720 },
      display_session: {
        schema: "elastos.browser.display-session/v1",
        mode: "webrtc_remote_display",
        signaling_url: `/api/apps/browser/pages/${pageId}/webrtc`,
        input: "runtime_route",
        input_protocol: "elastos_json",
        offerer: "browser",
        media_transport: "runtime_relay",
        ice_servers: [],
        audio: false,
        video: true,
        direct_network: false,
      },
    },
    stream_session: {
      schema: "elastos.net.stream-session/v1",
      backend: "fixture-local-exit",
      direct_network: false,
    },
    runtime_cleanup: {
      schema: "elastos.browser.cleanup-handle/v1",
      id: openedCleanupId,
    },
  };
}

function browserOpenAccepted() {
  return {
    schema: "elastos.browser.open-accepted/v1",
    open_id: openId,
    status_url: `/api/apps/browser/open/${openId}`,
  };
}

function launchRoute(origin, target, query) {
  const params = new URLSearchParams();
  for (const [key, value] of Object.entries(query || {})) {
    params.set(key, String(value));
  }
  params.set("home_origin", origin);
  if (target === "browser" && state.phase === "restored") {
    params.set("fixture_duplicate_open", "1");
  }
  const token = target === "home-gui"
    ? homeGuiToken
    : state.phase === "restored"
      ? browserRefreshedToken
      : browserOwnerToken;
  return `/apps/${target}/?${params.toString()}#home_token=${encodeURIComponent(token)}`;
}

function launchResponse(origin, target, query) {
  const bucket = state.phase;
  if (target === "home-gui") {
    state.homeGuiLaunches[bucket] += 1;
  } else if (target === "browser") {
    state.browserLaunches[bucket] += 1;
  }
  return {
    target,
    title: target === "home-gui" ? "Desktop" : "Browser",
    attach_kind: "iframe",
    launch_status: "launched",
    route: launchRoute(origin, target, query),
  };
}

function exactIntent(left, right) {
  return JSON.stringify(left) === JSON.stringify(right);
}

function requireToken(req, expected, res) {
  if (req.headers["x-elastos-home-token"] === expected) {
    return true;
  }
  json(res, 401, { error: "fixture launch owner rejected" });
  return false;
}

function placeholderBrowser(res) {
  const body = `<!doctype html>
<html lang="en">
  <head><meta charset="utf-8"><title>Browser restore placeholder</title></head>
  <body data-fixture-browser="placeholder">Browser session descriptor retained for outer Home refresh.</body>
</html>`;
  res.writeHead(200, {
    "access-control-allow-origin": "*",
    "cache-control": "no-store",
    "content-length": Buffer.byteLength(body),
    "content-type": "text/html; charset=utf-8",
  });
  res.end(body);
}

async function handleApi(req, res, url) {
  if (req.method === "OPTIONS") {
    res.writeHead(204, {
      "access-control-allow-headers": "content-type,x-elastos-home-token",
      "access-control-allow-methods": "GET,POST,OPTIONS",
      "access-control-allow-origin": "*",
      "access-control-max-age": "60",
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
    if (target !== "home-gui" && target !== "browser") {
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
  if (url.pathname === "/api/apps/browser/summary" && req.method === "GET") {
    if (!requireToken(req, browserRefreshedToken, res)) {
      return true;
    }
    assert(
      url.searchParams.get("browser_instance") === browserInstanceId,
      "refreshed Browser summary omitted its exact stable instance binding",
      { actual: url.searchParams.get("browser_instance"), expected: browserInstanceId },
    );
    state.browserSummaryBindings += 1;
    json(res, 200, browserSummary());
    return true;
  }
  if (url.pathname === "/api/apps/browser/open" && req.method === "POST") {
    if (!requireToken(req, browserRefreshedToken, res)) {
      return true;
    }
    const intent = await readBody(req);
    assert(
      intent?.browser_instance === browserInstanceId,
      "refreshed Browser open omitted its exact stable instance binding",
      { actual: intent?.browser_instance, expected: browserInstanceId },
    );
    state.browserOpenBindings += 1;
    state.browserOpenRequests += 1;
    if (state.browserOpenIntent === null) {
      state.browserOpenIntent = intent;
      state.browserOpenEffects += 1;
      state.browserOpenCompleted = true;
      state.browserPageCount = 1;
      state.browserVmCount = 1;
      state.activeCleanupId = openedCleanupId;
    } else if (!exactIntent(state.browserOpenIntent, intent)) {
      json(res, 409, { error: "fixture launch owns a different open intent" });
      return true;
    }
    json(res, 202, browserOpenAccepted());
    return true;
  }
  if (url.pathname === `/api/apps/browser/open/${openId}` && req.method === "GET") {
    if (!requireToken(req, browserRefreshedToken, res)) {
      return true;
    }
    state.browserStatusPolls += 1;
    json(res, 200, {
      schema: "elastos.browser.open-status/v1",
      open_id: openId,
      status: "completed",
      result: browserOpenResult(),
    });
    return true;
  }
  if (
    url.pathname === `/api/apps/browser/pages/${pageId}/webrtc` &&
    req.method === "POST"
  ) {
    if (!requireToken(req, browserRefreshedToken, res)) {
      return true;
    }
    const signal = await readBody(req);
    if (signal?.type === "offer") {
      json(res, 200, {
        schema: "elastos.browser.webrtc-answer/v1",
        type: "answer",
        sdp: [
          "v=0",
          "o=- 0 0 IN IP4 127.0.0.1",
          "s=fixture",
          "t=0 0",
          "m=video 9 UDP/TLS/RTP/SAVPF 96",
          "a=mid:0",
          "a=recvonly",
          "",
        ].join("\r\n"),
        candidates: [],
        end_of_candidates: true,
      });
    } else {
      json(res, 200, {
        schema: "elastos.browser.webrtc-signal-ack/v1",
        type: signal?.type || "end_of_candidates",
        accepted: true,
        candidates: [],
        end_of_candidates: true,
      });
    }
    return true;
  }
  if (
    url.pathname === `/api/apps/browser/pages/${pageId}/status` &&
    req.method === "GET"
  ) {
    if (!requireToken(req, browserRefreshedToken, res)) {
      return true;
    }
    json(res, 200, {
      schema: "elastos.browser.page-status/v1",
      page_id: pageId,
      actual_url: "https://ela.city/",
      title: "ela.city",
      can_go_back: false,
      can_go_forward: false,
      view: { width: 1280, height: 720 },
    });
    return true;
  }
  if (
    (url.pathname === `/api/apps/browser/pages/${pageId}/heartbeat` ||
      url.pathname === `/api/apps/browser/pages/${pageId}/input`) &&
    req.method === "POST"
  ) {
    if (!requireToken(req, browserRefreshedToken, res)) {
      return true;
    }
    json(res, 200, {
      schema: "elastos.browser.input-result/v1",
      page_id: pageId,
      accepted: true,
    });
    return true;
  }
  if (
    url.pathname === `/api/apps/browser/pages/${pageId}/close` &&
    req.method === "POST"
  ) {
    if (!requireToken(req, browserRefreshedToken, res)) {
      return true;
    }
    const closeRequest = await readBody(req);
    if (
      closeRequest?.schema !== "elastos.browser.close-request/v2" ||
      closeRequest.cleanup_id !== state.activeCleanupId
    ) {
      json(res, 400, { error: "fixture received a mismatched Browser cleanup identifier" });
      return true;
    }
    state.browserCleanupEffects += 1;
    state.browserPageCount = 0;
    state.browserVmCount = 0;
    state.activeCleanupId = null;
    json(res, 200, {
      schema: "elastos.browser.close-result/v1",
      page_id: pageId,
      closed: true,
      cleanup_id: closeRequest.cleanup_id,
    });
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
    if (
      state.phase === "placeholder" &&
      url.pathname === "/apps/browser/"
    ) {
      placeholderBrowser(res);
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
    state.errors.push(String(error?.stack || error));
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

function homeGuiFrame(page) {
  return page.frames().find((frame) => frame.url().includes("/apps/home-gui/")) || null;
}

function browserFrame(page) {
  return page.frames().find((frame) => frame.url().includes("/apps/browser/")) || null;
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
  await context.addInitScript(
    ({ contextId }) => {
      try {
        if (window.top === window && location.pathname === "/apps/home/") {
          localStorage.setItem("elastos.home.browser-context-id", contextId);
        }
      } catch (_error) {
        // Opaque capsule frames deliberately have no browser-profile storage.
      }

      class FixturePeerConnection extends EventTarget {
        constructor() {
          super();
          this.connectionState = "connected";
          this.iceConnectionState = "connected";
          this.iceGatheringState = "complete";
          this.signalingState = "stable";
          this.localDescription = null;
          this.remoteDescription = null;
        }

        addTransceiver() {
          return {};
        }

        async createOffer() {
          return {
            type: "offer",
            sdp: [
              "v=0",
              "o=- 0 0 IN IP4 127.0.0.1",
              "s=fixture",
              "t=0 0",
              "m=video 9 UDP/TLS/RTP/SAVPF 96",
              "a=mid:0",
              "a=recvonly",
              "",
            ].join("\r\n"),
          };
        }

        async setLocalDescription(description) {
          this.localDescription = description;
        }

        async setRemoteDescription(description) {
          this.remoteDescription = description;
        }

        async addIceCandidate() {}

        async getStats() {
          return new Map();
        }

        close() {
          this.connectionState = "closed";
          this.iceConnectionState = "closed";
        }
      }
      Object.defineProperty(window, "RTCPeerConnection", {
        configurable: true,
        value: FixturePeerConnection,
      });

      const runtimeFetch = window.fetch.bind(window);
      window.fetch = async (input, init = {}) => {
        const requestUrl = new URL(
          typeof input === "string" ? input : input.url,
          location.href,
        );
        const duplicateCompletedOpen =
          location.search.includes("fixture_duplicate_open=1") &&
          requestUrl.pathname === "/api/apps/browser/open" &&
          String(init.method || "GET").toUpperCase() === "POST";
        if (!duplicateCompletedOpen) {
          return runtimeFetch(input, init);
        }
        const first = await runtimeFetch(input, init);
        if (!first.ok) {
          return first;
        }
        return runtimeFetch(input, init);
      };
    },
    { contextId: browserContextId },
  );

  const page = await context.newPage();
  const pageErrors = [];
  page.on("pageerror", (error) => pageErrors.push(String(error?.stack || error)));
  page.on("console", (message) => {
    if (message.type() === "error") {
      pageErrors.push(`console: ${message.text()}`);
    }
  });

  await page.goto(`${origin}/apps/home/`, { waitUntil: "domcontentloaded" });
  await page.waitForFunction(
    () => document.body.dataset.homeStatus === "ready",
    null,
    { timeout: 15_000 },
  );
  await waitFor(() => homeGuiFrame(page), 15_000, "first real Home GUI frame");
  const firstGui = homeGuiFrame(page);
  await firstGui.waitForSelector('.window[data-target="browser"]', { timeout: 15_000 });
  assert(
    await firstGui.locator('.window[data-target="browser"]').count() === 1,
    "first Home load did not restore exactly one Browser shell",
  );
  await waitFor(
    () => browserFrame(page)?.url().includes("/apps/browser/"),
    15_000,
    "first Browser placeholder frame",
  );
  const firstBrowser = browserFrame(page);
  assert(
    await firstBrowser.locator('[data-fixture-browser="placeholder"]').count() === 1,
    "first Home load did not retain the Browser descriptor in a real isolated frame",
  );
  assert(
    state.browserOpenEffects === 0,
    "placeholder phase unexpectedly created a Runtime Browser page",
    state,
  );

  state.phase = "restored";
  state.browserPageCount = 1;
  state.browserVmCount = 1;
  state.activeCleanupId = recoveredCleanupId;
  await page.reload({ waitUntil: "domcontentloaded" });
  await page.waitForFunction(
    () => document.body.dataset.homeStatus === "ready",
    null,
    { timeout: 15_000 },
  );
  await waitFor(() => homeGuiFrame(page), 15_000, "refreshed real Home GUI frame");
  const restoredGui = homeGuiFrame(page);
  await restoredGui.waitForSelector('.window[data-target="browser"]', { timeout: 15_000 });
  await waitFor(
    () => browserFrame(page)?.url().includes("fixture_duplicate_open=1"),
    15_000,
    "restored real Browser frame",
  );
  const restoredBrowser = browserFrame(page);
  await restoredBrowser.waitForSelector("#browser-form", { timeout: 15_000 });
  await waitFor(
    () => state.browserStatusPolls > 0 && state.browserOpenCompleted,
    15_000,
    "completed coalesced Browser open",
  );
  await restoredBrowser.waitForFunction(
    () =>
      document.querySelector("#browser-status .browser-status-message")?.textContent ===
      "Remote display negotiated. Waiting for video...",
    null,
    { timeout: 15_000 },
  );

  const restoredWindowCount = await restoredGui
    .locator('.window[data-target="browser"]')
    .count();
  const restoredFrameCount = page.frames().filter(
    (frame) => frame.url().includes("/apps/browser/"),
  ).length;
  const restoredStatus = await restoredBrowser
    .locator("#browser-status .browser-status-message")
    .textContent();
  const visibleWindowErrors = await restoredGui
    .locator(".window-error:not([hidden])")
    .count();
  const hostRecoveryVisible = await page.locator("#shell-host-recovery:not([hidden])").count();

  assert(restoredWindowCount === 1, "outer Home refresh created a replacement Browser shell");
  assert(restoredFrameCount === 1, "outer Home refresh created multiple Browser frames");
  assert(state.homeGuiLaunches.placeholder === 1, "first Home loaded multiple GUI roots", state);
  assert(state.homeGuiLaunches.restored === 1, "refresh loaded multiple GUI roots", state);
  assert(state.browserLaunches.placeholder === 1, "first Home replaced the Browser window", state);
  assert(state.browserLaunches.restored === 1, "refresh replaced the Browser window", state);
  assert(state.browserOpenRequests === 2, "completed-open race did not issue two matching requests", state);
  assert(state.browserSummaryBindings > 0, "refreshed Browser summary was not instance-bound", state);
  assert(state.browserOpenBindings === 2, "refreshed Browser opens were not instance-bound", state);
  assert(state.browserOpenEffects === 1, "matching completed-open race duplicated provider work", state);
  assert(state.browserCleanupEffects === 1, "opaque Home reload did not reap exactly one recovered page", state);
  assert(state.browserPageCount === 1, "restored Browser owns the wrong active page count", state);
  assert(state.browserVmCount === 1, "restored Browser owns the wrong active VM count", state);
  assert(state.activeCleanupId === openedCleanupId, "restored Browser cleanup ownership is not recoverable", state);
  assert(state.browserStatusPolls === 1, "restored Browser polled a replacement open job", state);
  assert(
    !/failed|could not open|did not start|error/i.test(restoredStatus || ""),
    "restored Browser rendered a false startup failure",
    restoredStatus,
  );
  assert(visibleWindowErrors === 0, "Home GUI rendered a Browser replacement error");
  assert(hostRecoveryVisible === 0, "Home host entered recovery during Browser restore");
  assert(state.errors.length === 0, "fixture server recorded errors", state.errors);
  assert(pageErrors.length === 0, "real Home/GUI/Browser source logged errors", pageErrors);

  console.log(
    "[home-browser-restored-lifecycle-headless] PASS " +
      `home_refresh=1 shell=${restoredWindowCount} frame=${restoredFrameCount} ` +
      `open_requests=${state.browserOpenRequests} provider_effects=${state.browserOpenEffects} ` +
      `cleanup_effects=${state.browserCleanupEffects} pages=${state.browserPageCount} ` +
      `vms=${state.browserVmCount} recoverable_cleanup=1 open_id=${openId}`,
  );
} finally {
  await browser?.close().catch(() => {});
  await new Promise((resolveClose) => server.close(resolveClose));
}
