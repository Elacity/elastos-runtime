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
const browserReopenedToken = "fixture-browser-reopened-launch-token";
const openId = "browser-open-fixture-0001";
const pageId = "browser-page-fixture-0001";
const reopenedPageId = "browser-page-fixture-0002";
const principalId = "did:elastos:fixture-home-refresh";
const recoveredCleanupId = "browser-cleanup:fixture-recovered";
const openedCleanupId = "browser-cleanup:fixture-opened";
const retainedEngineId = "fixture-retained-engine";
const retainedExitId = "fixture-retained-exit";
const initialDisplayGeneration = `display:${"1".repeat(32)}`;
const restoredDisplayGeneration = `display:${"2".repeat(32)}`;
const reopenedDisplayGeneration = `display:${"3".repeat(32)}`;
let releaseBrowserWindowCloseTerminal;
const browserWindowCloseTerminalGate = new Promise((resolveGate) => {
  releaseBrowserWindowCloseTerminal = resolveGate;
});

const state = {
  phase: "placeholder",
  homeGuiLaunches: { placeholder: 0, restored: 0, reopened: 0 },
  browserLaunches: { placeholder: 0, restored: 0, reopened: 0 },
  browserOpenRequests: 0,
  browserSummaryBindings: 0,
  browserOpenBindings: 0,
  browserOpenEffects: 0,
  browserOpenIntent: null,
  browserOpenCompleted: false,
  browserStatusPolls: 0,
  browserPageStatusReads: 0,
  browserFastStatusReads: 0,
  browserDisplayAttachRequests: [],
  browserDisplayAttachEffects: 0,
  browserSignals: [],
  reopenedBrowserInstanceId: null,
  browserPageCount: 0,
  browserVmCount: 0,
  browserCleanupEffects: 0,
  browserWindowCloseRequests: 0,
  browserWindowCloseTerminalReceipts: 0,
  browserReapedCleanupId: null,
  activeCleanupId: null,
  activePageId: null,
  activeDisplayGeneration: initialDisplayGeneration,
  homeStateWrites: [],
  errors: [],
};
const pageErrors = [];

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
      device_did: null,
      profile_readiness: { schema: "elastos.profile.readiness/v1", status: "ready" },
      recovery_readiness: { schema: "elastos.recovery.readiness/v1", status: "ready" },
    },
    authority: {
      signed_in: true,
      principal_id: principalId,
      session_id: "fixture-session",
      proof_binding_id: "fixture-proof-binding",
      wallet_connected: false,
    },
    browser_state: {
      schema: "elastos.home.browser-state/v1",
      principal_id: principalId,
      localhost_root: "localhost://fixture",
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
              browser_engine_id: "fixture-default-engine",
              remote_exit_id: "fixture-other-exit",
            },
          },
        ],
      },
    },
    active_shell: {
      schema: "elastos.home.active-shell/v1",
      active: "home-gui",
      candidates: [
        {
          name: "home-gui",
          title: "Desktop",
          description: "Fixture Desktop", route: "/apps/home-gui/", role: "shell", trust_state: "trusted",
          launchable: true,
        },
      ],
    },
    // Current HomeSummaryResponse fields, including the strict appearance contract.
    appearance: {
      schema: "elastos.home.appearance/v1", revision: 0, theme: "dark", accent: "blue",
      accent_custom: "#4f7fff", dock_auto_hide: false, sounds: false, focus_mode: false,
      background_image_url: null, background_overlay_enabled: true, background_overlay_opacity: 0.55,
    },
    runtime: { running: true, kind: "local", version: null, api_url: null, pid: null, running_capsules: [], note: null },
    site: { staged: false, root_uri: "", path: "", active_release: null, active_channel: null, active_bundle_cid: null, release_count: 0 },
    room: {
      room_slug: "fixture", title: "", member_count: 0, active_member_count: 0, pending_count: 0, active_session_count: 0,
      latest_request_name: null, latest_request_device: null, local_runtime_did: null, local_runtime_role: null,
      canonical_hosted_guest_url: null, ephemeral_hosted_guest_url: null,
      browser_access_allowed: true, browser_access_block_reason: null, pending_requests: [], active_sessions: [],
    },
    people: { schema: "elastos.people.contacts/v1", contact_count: 0, contacts: [], service_offer_count: 0, service_offers: [] },
    services: {
      schema: "elastos.runtime.services/v1", local_offer_count: 0, remote_offer_count: 0,
      available_local_offer_count: 0, available_remote_offer_count: 0, local_offers: [], remote_offers: [],
      available_local_offers: [], available_remote_offers: [], grant_model: "principal_scoped_provider_grant",
      carrier_contract: "People discovers trusted offers; Carrier carries signed offer envelopes; providers enforce grants.",
      capsule_contract: "capsule -> runtime capability -> provider grant -> service",
    },
    notifications: { unread_count: 0, attention_count: 0, entries: [] },
    desktop_objects: { schema: "elastos.home.desktop-objects/v1", uri: "localhost://fixture/Desktop", objects: [], stale: false },
    capsule_catalog: {
      schema: "elastos.capsules.catalog/v1", capsules: [],
      counts: { total: 0, installed: 0, launchable: 0, interfaces: 0, methods: 0, apps: 0, viewers: 0, providers: 0, content: 0, shell: 0 },
      policy: { install_state: "signed-app-install-pending", install_note: "Fixture inventory", payment_state: "provider-rail-required", payment_note: "", drm_state: "provider-rail-required", drm_note: "" },
    },
    capsule_interfaces: {
      schema: "elastos.capsules.interfaces/v1", interfaces: [],
      counts: { capsules: 0, interfaces: 0, methods: 0, executable_methods: 0 },
      policy: { descriptor_state: "manifest-declared", descriptor_note: "Fixture inventory", invocation_state: "runtime-gated", invocation_note: "" },
    },
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
      status: "configured",
      capacity_available: true,
      fresh_start_allowed: state.activeCleanupId === null,
      active_sessions: state.browserPageCount,
      launching_sessions: 0,
      total_sessions: state.browserPageCount,
      recoverable_page: state.activeCleanupId
        ? {
            schema: "elastos.browser.recoverable-page/v1",
            state: "active",
            page_id: state.activePageId,
            cleanup: {
              schema: "elastos.browser.cleanup-handle/v1",
              id: state.activeCleanupId,
            },
            service_selection: {
              schema: "elastos.browser.service-selection/v1",
              engine_id: state.activePageId === pageId ? retainedEngineId : "",
              exit_id: state.activePageId === pageId ? retainedExitId : "",
            },
            engine_page: enginePage(state.activePageId, state.activeDisplayGeneration),
          }
        : null,
    },
    engine_adapter: {
      display_attach_supported: true,
      supported_display_modes: ["webrtc_remote_display"],
      supported_guarantee_levels: ["mechanism_microvm"],
      adapters: ["fixture-default-engine", retainedEngineId].map((id, index) => ({
          id,
          default: index === 0,
          engine: "fixture-microvm",
          backing_substrate: "local_microvm",
          direct_network: false,
          wallet_injection: false,
          supported_display_modes: ["webrtc_remote_display"],
          supported_guarantee_levels: ["mechanism_microvm"],
        })),
    },
    net: {
      exit_provider: {
        remote_carrier_exits: ["fixture-other-exit", retainedExitId].map((id) => ({
          id, grant_id: `grant:${id}`, display_name: id, state: "ready", allowed_for_principal: true,
        })),
      },
    },
  };
}

function enginePage(id, displayGeneration) {
  const retained = id === pageId;
  return {
    schema: "elastos.browser.engine.page/v1",
    page_id: id,
    url: retained ? "https://fixture.invalid/retained" : "https://ela.city/",
    actual_url: retained ? "https://fixture.invalid/retained" : "https://ela.city/",
    title: retained ? "Retained page" : "ela.city",
    view: { width: 1280, height: 720 },
    display_session: {
      schema: "elastos.browser.display-session/v1",
      mode: "webrtc_remote_display",
      signaling_url: `/api/apps/browser/pages/${id}/webrtc`,
      display_generation: displayGeneration,
      input: "runtime_route",
      input_protocol: "elastos_json",
      offerer: retained ? "engine" : "browser",
      media_transport: "runtime_relay",
      ice_servers: [],
      audio: false,
      video: true,
      direct_network: false,
      width: 1280,
      height: 720,
    },
  };
}

function browserOpenResult() {
  return {
    schema: "elastos.browser.open-result/v1",
    engine_page: enginePage(reopenedPageId, reopenedDisplayGeneration),
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
  if (target === "browser" && state.phase === "reopened") {
    params.set("fixture_duplicate_open", "1");
  }
  const token = target === "home-gui"
    ? homeGuiToken
    : state.phase === "reopened"
      ? browserReopenedToken
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
    if (bucket === "reopened") {
      assert(query?.browser_instance && query.browser_instance !== browserInstanceId,
        "Home fresh reopen reused the retired Browser instance", query);
      state.reopenedBrowserInstanceId = query.browser_instance;
    }
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

function currentBrowserToken() {
  return state.phase === "reopened" ? browserReopenedToken : browserRefreshedToken;
}

function currentBrowserInstance() {
  return state.phase === "reopened" ? state.reopenedBrowserInstanceId : browserInstanceId;
}

function fixtureOffer() {
  return {
    schema: "elastos.browser.webrtc-offer/v1", type: "offer",
    sdp: "v=0\r\no=- 0 0 IN IP4 127.0.0.1\r\ns=fixture\r\nt=0 0\r\n",
    candidates: [], end_of_candidates: true,
  };
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
  if (url.pathname === "/api/apps/home/collaboration/presence" && req.method === "POST") {
    if (!requireToken(req, homeAuthorityToken, res)) return true;
    assert(JSON.stringify(await readBody(req)) === "{}", "Home presence must use an empty admitted request");
    json(res, 200, {
      schema: "elastos.people.discovery/v1", configured: false, enabled: false,
      status: "unconfigured", status_message: "Discovery isn’t available on this Home.",
      discovered_count: 0, discovered_peers: [], request_count: 0,
    });
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
    if (!requireToken(req, currentBrowserToken(), res)) {
      return true;
    }
    assert(
      url.searchParams.get("browser_instance") === currentBrowserInstance(),
      "Browser summary omitted its exact instance binding",
      { actual: url.searchParams.get("browser_instance"), expected: currentBrowserInstance() },
    );
    state.browserSummaryBindings += 1;
    json(res, 200, browserSummary());
    return true;
  }
  if (url.pathname === "/api/apps/browser/open" && req.method === "POST") {
    if (!requireToken(req, browserReopenedToken, res)) {
      return true;
    }
    const intent = await readBody(req);
    assert(
      state.phase === "reopened" && intent?.browser_instance === state.reopenedBrowserInstanceId,
      "fresh Browser open omitted its new instance binding",
      { actual: intent?.browser_instance, expected: state.reopenedBrowserInstanceId },
    );
    assert(!intent.adapter_id && !intent.remote_exit_id,
      "fresh Home launch inherited retired Engine or Exit choices", intent);
    state.browserOpenBindings += 1;
    state.browserOpenRequests += 1;
    if (state.browserOpenIntent === null) {
      state.browserOpenIntent = intent;
      state.browserOpenEffects += 1;
      state.browserOpenCompleted = true;
      state.browserPageCount = 1;
      state.browserVmCount = 1;
      state.activeCleanupId = openedCleanupId;
      state.activePageId = reopenedPageId;
      state.activeDisplayGeneration = reopenedDisplayGeneration;
    } else if (!exactIntent(state.browserOpenIntent, intent)) {
      json(res, 409, { error: "fixture launch owns a different open intent" });
      return true;
    }
    json(res, 202, browserOpenAccepted());
    return true;
  }
  if (url.pathname === `/api/apps/browser/open/${openId}` && req.method === "GET") {
    if (!requireToken(req, browserReopenedToken, res)) {
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
  const pageRoute = url.pathname.match(/^\/api\/apps\/browser\/pages\/([^/]+)\/(webrtc|status|heartbeat|input|close)$/);
  const requestPageId = pageRoute?.[1];
  const pageToken = requestPageId === pageId ? browserRefreshedToken : browserReopenedToken;
  if (pageRoute && ![pageId, reopenedPageId].includes(requestPageId)) {
    json(res, 404, { error: "fixture page is not owned" });
    return true;
  }
  if (pageRoute?.[2] === "webrtc" && req.method === "POST") {
    if (!requireToken(req, pageToken, res)) {
      return true;
    }
    const signal = await readBody(req);
    assert(state.activePageId === requestPageId, "signaling used a retired page", signal);
    if (signal?.type === "display_attach") {
      assert(requestPageId === pageId && signal.display_generation === initialDisplayGeneration &&
        signal.request_id === initialDisplayGeneration.slice("display:".length) &&
        Object.keys(signal).sort().join(",") === "display_generation,request_id,type",
        "retained display attach changed its exact request contract", signal);
      const previous = state.browserDisplayAttachRequests[0];
      assert(!previous || exactIntent(previous, signal), "display attach retries changed request identity", signal);
      state.browserDisplayAttachRequests.push(signal);
      if (!previous) state.browserDisplayAttachEffects += 1;
      state.activeDisplayGeneration = restoredDisplayGeneration;
      json(res, 200, {
        schema: "elastos.browser.display-attach-result/v1", page_id: pageId,
        request_id: signal.request_id, previous_display_generation: signal.display_generation,
        display_generation: restoredDisplayGeneration, initial_offer: fixtureOffer(), audio_offer: fixtureOffer(),
      });
      return true;
    }
    assert(signal?.display_generation === state.activeDisplayGeneration,
      "Browser signaling used the wrong display generation", signal);
    state.browserSignals.push({ page_id: requestPageId, ...signal });
    if (signal?.type === "offer") {
      json(res, 200, {
        schema: "elastos.browser.webrtc-answer/v1",
        display_generation: signal.display_generation,
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
        display_generation: signal.display_generation,
        type: signal?.type || "end_of_candidates",
        accepted: true,
        candidates: [],
        end_of_candidates: true,
      });
    }
    return true;
  }
  if (pageRoute?.[2] === "status" && req.method === "GET") {
    if (!requireToken(req, pageToken, res)) {
      return true;
    }
    state.browserPageStatusReads += 1;
    if (url.searchParams.get("fast") === "1") state.browserFastStatusReads += 1;
    const current = enginePage(requestPageId, state.activeDisplayGeneration);
    json(res, 200, {
      schema: "elastos.browser.page-status/v1",
      page_id: requestPageId,
      actual_url: current.actual_url,
      title: requestPageId === pageId && url.searchParams.get("fast") === "1"
        ? "Retained page after status poll" : current.title,
      direct_network: false,
      can_go_back: false,
      can_go_forward: false,
      view: { width: 1280, height: 720 },
    });
    return true;
  }
  if (["heartbeat", "input"].includes(pageRoute?.[2]) && req.method === "POST") {
    if (!requireToken(req, pageToken, res)) {
      return true;
    }
    json(res, 200, {
      schema: "elastos.browser.input-result/v1",
      page_id: requestPageId,
      accepted: true,
    });
    return true;
  }
  if (pageRoute?.[2] === "close" && req.method === "POST") {
    if (!requireToken(req, pageToken, res)) {
      return true;
    }
    const closeRequest = await readBody(req);
    if (
      closeRequest?.schema !== "elastos.browser.close-request/v2" ||
      closeRequest.cleanup_id !== state.activeCleanupId ||
      requestPageId !== state.activePageId ||
      closeRequest.browser_instance !== currentBrowserInstance()
    ) {
      json(res, 400, { error: "fixture received a mismatched Browser cleanup identifier" });
      return true;
    }
    state.browserWindowCloseRequests += 1;
    if (closeRequest.cleanup_id === recoveredCleanupId) {
      if (state.browserWindowCloseRequests === 1) {
        json(res, 200, {
          schema: "elastos.browser.close-result/v1",
          page_id: pageId,
          closed: false,
          already_closed: false,
          cleanup_id: closeRequest.cleanup_id,
        });
        return true;
      }
      await browserWindowCloseTerminalGate;
    }
    state.browserCleanupEffects += 1;
    state.browserWindowCloseTerminalReceipts += 1;
    state.browserPageCount = 0;
    state.browserVmCount = 0;
    state.browserReapedCleanupId = state.activeCleanupId;
    state.activeCleanupId = null;
    state.activePageId = null;
    json(res, 200, {
      schema: "elastos.browser.close-result/v1",
      page_id: requestPageId,
      closed: true,
      cleanup_id: closeRequest.cleanup_id,
      terminal_effects: { page_absent: true },
      cleanup: { schema: "elastos.browser.runtime-session-cleanup/v1", ok: true, action: "closed" },
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
    assert(pageErrors.length === 0 && state.errors.length === 0, `fixture failed while waiting for ${label}`, { pageErrors, serverErrors: state.errors });
    if (await check()) {
      return;
    }
    await new Promise((resolveWait) => setTimeout(resolveWait, 50));
  }
  throw new Error(`timed out waiting for ${label}: ${JSON.stringify({ pageErrors, serverErrors: state.errors })}`);
}

function homeGuiFrame(page) {
  return page.frames().find((frame) => frame.url().includes("/apps/home-gui/")) || null;
}

function browserFrame(page) {
  return page.frames().find((frame) => frame.url().includes("/apps/browser/")) || null;
}

const origin = await listen();
let browser = null;
let page = null;
try {
  const imported = await import(playwrightSpecifier());
  const { chromium } = imported.default || imported;
  browser = await chromium.launch({
    ...(process.env.ELASTOS_BROWSER_EXECUTABLE ? { executablePath: process.env.ELASTOS_BROWSER_EXECUTABLE } : {}),
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

        createDataChannel() {
          const channel = new EventTarget();
          channel.readyState = "open";
          channel.send = () => {};
          channel.close = () => { channel.readyState = "closed"; };
          return channel;
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

        async createAnswer() {
          return { ...(await this.createOffer()), type: "answer" };
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

  page = await context.newPage();
  const recordPageError = (error) => {
    if (pageErrors.length < 20) pageErrors.push(String(error).slice(0, 2000));
  };
  page.on("pageerror", (error) => recordPageError(error?.stack || error));
  page.on("console", (message) => {
    if (message.type() === "error") {
      recordPageError(`console: ${message.text()}`);
    }
  });

  await page.goto(`${origin}/apps/home/`, { waitUntil: "domcontentloaded" });
  await waitFor(() => page.evaluate(() => document.body.dataset.homeStatus === "ready"), 15_000, "initial Home bootstrap");
  await waitFor(() => homeGuiFrame(page), 15_000, "first real Home GUI frame");
  const firstGui = homeGuiFrame(page);
  await waitFor(() => firstGui.locator('.window[data-target="browser"]').isVisible(), 15_000, "initial Browser window");
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
  state.activePageId = pageId;
  await page.reload({ waitUntil: "domcontentloaded" });
  await waitFor(() => page.evaluate(() => document.body.dataset.homeStatus === "ready"), 15_000, "restored Home bootstrap");
  await waitFor(() => homeGuiFrame(page), 15_000, "refreshed real Home GUI frame");
  const restoredGui = homeGuiFrame(page);
  await waitFor(() => restoredGui.locator('.window[data-target="browser"]').isVisible(), 15_000, "restored Browser window");
  await waitFor(
    () => browserFrame(page)?.url().includes(encodeURIComponent(browserRefreshedToken)),
    15_000,
    "restored real Browser frame",
  );
  const restoredBrowser = browserFrame(page);
  await waitFor(() => restoredBrowser.locator("#browser-form").isVisible(), 15_000, "restored Browser bootstrap");
  await waitFor(
    () => state.browserPageStatusReads > 0 && state.browserDisplayAttachEffects === 1,
    15_000,
    "retained page status and fresh display attachment",
  );
  await restoredBrowser.waitForFunction(
    (expectedPage) => document.body.dataset.loading === "false" &&
      window.__elastosBrowserCurrentPageId === expectedPage,
    pageId,
    { timeout: 15_000 },
  );
  await waitFor(() => state.browserFastStatusReads > 0, 5_000, "restored page periodic status polling");
  await restoredBrowser.waitForFunction(() => document.title.startsWith("Retained page after status poll"),
    null, { timeout: 5_000 });

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
  assert(state.browserOpenRequests === 0 && state.browserOpenBindings === 0 && state.browserOpenEffects === 0,
    "Home reload opened a replacement for the retained page", state);
  assert(state.browserSummaryBindings > 0, "refreshed Browser summary was not instance-bound", state);
  assert(state.browserCleanupEffects === 0, "Home reload reaped the retained page", state);
  assert(state.browserPageCount === 1, "restored Browser owns the wrong active page count", state);
  assert(state.browserVmCount === 1, "restored Browser owns the wrong active VM count", state);
  assert(state.activeCleanupId === recoveredCleanupId && state.activePageId === pageId,
    "restored Browser changed page or cleanup ownership", state);
  assert(state.browserStatusPolls === 0, "restored Browser polled a replacement open job", state);
  assert(state.browserDisplayAttachRequests.length === 1 && state.browserDisplayAttachEffects === 1,
    "restored Browser duplicated display attachment", state);
  assert(state.browserSignals.some((signal) => signal.type === "answer" &&
    signal.page_id === pageId && signal.display_generation === restoredDisplayGeneration),
    "restored viewer did not answer under the new display generation", state);
  assert(await restoredBrowser.locator("#browser-engine").inputValue() === retainedEngineId &&
    await restoredBrowser.locator("#browser-exit").inputValue() === retainedExitId,
    "retained nondefault Engine/Exit choices did not replace stale launch hints");
  assert(await restoredBrowser.evaluate(() => window.__elastosBrowserCurrentPageId) === pageId &&
    new URL(restoredBrowser.url()).searchParams.get("display_generation") === restoredDisplayGeneration,
    "viewer did not publish its retained page and new display generation");
  assert(
    !/failed|could not open|did not start|error/i.test(restoredStatus || ""),
    "restored Browser rendered a false startup failure",
    restoredStatus,
  );
  assert(visibleWindowErrors === 0, "Home GUI rendered a Browser replacement error");
  assert(hostRecoveryVisible === 0, "Home host entered recovery during Browser restore");
  assert(state.errors.length === 0, "fixture server recorded errors", state.errors);
  assert(pageErrors.length === 0, "real Home/GUI/Browser source logged errors", pageErrors);

  const restoredTaskbarButton = restoredGui.locator(
    '.taskbar-item[data-target="browser"]',
  );
  assert(
    await restoredTaskbarButton.getAttribute("data-open-windows") === "1",
    "restored Browser taskbar entry did not represent exactly one window",
  );
  await restoredGui.evaluate(() => {
    window.addEventListener("message", (event) => {
      if (event.data?.type === "elastos.browser.window-close.result/v1" && event.data.state === "terminal") {
        window.__fixtureRetiredBrowserClose = { data: event.data, source: event.source, origin: event.origin };
      }
    });
  });
  await restoredGui
    .locator('.window[data-target="browser"] [data-action="close"]')
    .click();
  await waitFor(
    () => state.browserWindowCloseRequests >= 1,
    5_000,
    "first nonterminal Browser window close",
  );
  assert(
    await restoredGui.locator('.window[data-target="browser"]').count() === 1 &&
      await restoredTaskbarButton.getAttribute("data-open-windows") === "1" &&
      state.browserPageCount === 1 && state.browserVmCount === 1 &&
      state.activeCleanupId === recoveredCleanupId && state.browserCleanupEffects === 0,
    "nonterminal Browser close removed its shell, taskbar or Runtime ownership",
  );
  await waitFor(() => state.browserWindowCloseRequests === 2, 5_000, "pending terminal close retry");
  assert(await restoredGui.locator('.window[data-target="browser"]').count() === 1 &&
    state.browserWindowCloseTerminalReceipts === 0, "pending close retry removed the shell before its receipt");
  releaseBrowserWindowCloseTerminal();
  await waitFor(
    async () =>
      await restoredGui.locator('.window[data-target="browser"]').count() === 0,
    10_000,
    "delayed terminal Browser window close",
  );
  const finalTaskbarButton = restoredGui.locator(
    '.taskbar-item[data-target="browser"]',
  );
  const finalTaskbarCount = await finalTaskbarButton.count();
  assert(
    finalTaskbarCount === 0 ||
      (await finalTaskbarButton.getAttribute("data-open-windows")) === "0",
    "terminal Browser close retained an open taskbar entry",
  );
  assert(
      state.browserWindowCloseRequests === 2 &&
      state.browserWindowCloseTerminalReceipts === 1 &&
      state.browserCleanupEffects === 1 &&
      state.browserPageCount === 0 &&
      state.browserVmCount === 0 &&
      state.activeCleanupId === null &&
      state.activePageId === null &&
      state.browserReapedCleanupId === recoveredCleanupId &&
      state.browserOpenRequests === 0 &&
      state.browserOpenEffects === 0,
    "terminal Browser close duplicated work or retained Runtime residue",
    state,
  );

  // The real Home taskbar opens a new window only after terminal close. The
  // fetch shim duplicates that fresh request; fixture dedup is not gateway proof.
  state.phase = "reopened";
  assert(finalTaskbarCount === 1, "fixture Browser pin is missing from the Home taskbar");
  await finalTaskbarButton.click();
  await waitFor(() => state.browserLaunches.reopened === 1 && browserFrame(page),
    15_000, "fresh Browser launched through Home");
  const reopenedBrowser = browserFrame(page);
  await waitFor(() => state.browserOpenCompleted && state.browserStatusPolls === 1,
    15_000, "completed matching fresh-open requests");
  await reopenedBrowser.waitForFunction(() =>
    document.querySelector("#browser-status .browser-status-message")?.textContent ===
    "Remote display negotiated. Waiting for video...", null, { timeout: 15_000 });
  assert(state.browserOpenRequests === 2 && state.browserOpenBindings === 2 && state.browserOpenEffects === 1,
    "matching fresh-open fixture requests duplicated effects or lost instance bindings", state);
  assert(state.activePageId === reopenedPageId && state.activeCleanupId === openedCleanupId &&
    state.browserPageCount === 1 && state.browserVmCount === 1 &&
    state.reopenedBrowserInstanceId !== browserInstanceId,
    "fresh Browser did not acquire distinct page, cleanup and window ownership", state);
  assert(await reopenedBrowser.evaluate(() => window.__elastosBrowserCurrentPageId) === reopenedPageId &&
    new URL(reopenedBrowser.url()).hash.includes(browserReopenedToken),
    "fresh Browser did not use its new launch authority and page");
  assert(await reopenedBrowser.locator("#browser-engine").inputValue() === "" &&
    await reopenedBrowser.locator("#browser-exit").inputValue() === "",
    "fresh Home launch did not retain its own Automatic Engine/local Exit choices");
  const replayed = await restoredGui.evaluate(() => {
    const retired = window.__fixtureRetiredBrowserClose;
    if (!retired) return false;
    window.dispatchEvent(new MessageEvent("message", retired));
    return true;
  });
  assert(replayed, "fixture did not observe the original terminal close delivery");
  assert(await restoredGui.locator('.window[data-target="browser"]').count() === 1 &&
    page.frames().filter((frame) => frame.url().includes("/apps/browser/")).length === 1 &&
    state.activePageId === reopenedPageId && state.activeCleanupId === openedCleanupId &&
    state.browserWindowCloseRequests === 2,
    "late retired close delivery changed the fresh Browser window or owner");
  await restoredGui.locator('.window[data-target="browser"] [data-action="close"]').click();
  await waitFor(async () => await restoredGui.locator('.window[data-target="browser"]').count() === 0,
    10_000, "fresh Browser terminal cleanup");
  assert(state.browserWindowCloseRequests === 3 && state.browserWindowCloseTerminalReceipts === 2 &&
    state.browserCleanupEffects === 2 && state.browserPageCount === 0 && state.browserVmCount === 0 &&
    state.activeCleanupId === null && state.activePageId === null &&
    state.browserOpenRequests === 2 && state.browserOpenEffects === 1,
    "fresh close duplicated work or retained Runtime residue", state);
  assert(state.errors.length === 0 && pageErrors.length === 0,
    "Home/Browser lifecycle recorded errors", { pageErrors, serverErrors: state.errors });

  console.log(
    "[home-browser-restored-lifecycle-headless] PASS " +
      `home_refresh=1 shell=${restoredWindowCount} frame=${restoredFrameCount} ` +
      `open_requests=${state.browserOpenRequests} fixture_open_effects=${state.browserOpenEffects} ` +
      `fixture_cleanup_effects=${state.browserCleanupEffects} fixture_pages=${state.browserPageCount} ` +
      `fixture_vms=${state.browserVmCount} retained_restore=1 display_attach=1 delayed_close=1 fresh_reopen=1 ` +
      `open_id=${openId}`,
  );
} catch (error) {
  const viewer = page ? await browserFrame(page)?.evaluate(() => ({
    status: document.querySelector("#browser-status .browser-status-message")?.textContent,
    loading: document.body.dataset.loading,
    pageId: window.__elastosBrowserCurrentPageId,
  })).catch(() => null) : null;
  console.error("[home-browser-restored-lifecycle-headless] diagnostics", JSON.stringify({ state, pageErrors, viewer }));
  throw error;
} finally {
  releaseBrowserWindowCloseTerminal();
  await browser?.close().catch(() => {});
  await new Promise((resolveClose) => server.close(resolveClose));
}
