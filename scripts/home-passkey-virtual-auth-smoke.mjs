#!/usr/bin/env node

import {
  chmodSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createRequire } from "node:module";

const require = createRequire(new URL("../elastos/tools/browser-playwright-engine/package.json", import.meta.url));
const { chromium } = require("playwright");

const ELASTOS_BASE_URL = (process.env.ELASTOS_BASE_URL || "http://localhost:8090").replace(/\/+$/, "");
const HOME_URL = process.env.HOME_URL || `${ELASTOS_BASE_URL}/apps/home/`;
const TEST_NAME = process.env.HOME_VIRTUAL_AUTH_NAME || `Agent Smoke ${new Date().toISOString()}`;
const HEADLESS = process.env.HOME_VIRTUAL_AUTH_HEADED !== "1";
const PRESERVE_PROFILE = process.env.HOME_VIRTUAL_AUTH_PRESERVE_PROFILE === "1";
const CLEANUP_PASSKEY = process.env.HOME_VIRTUAL_AUTH_CLEANUP !== "0";
const INCLUDE_BROWSER = process.env.HOME_VIRTUAL_AUTH_BROWSER === "1";
const CHECK_APP_MATRIX = process.env.HOME_VIRTUAL_AUTH_APP_MATRIX === "1";
const CHECK_BROWSER_SUMMARY =
  process.env.HOME_VIRTUAL_AUTH_BROWSER_SUMMARY === "1" ||
  process.env.HOME_VIRTUAL_AUTH_BROWSER_OPEN === "1";
const OPEN_BROWSER = process.env.HOME_VIRTUAL_AUTH_BROWSER_OPEN === "1";
const CHECK_BROWSER_PROFILE_RESET =
  process.env.HOME_VIRTUAL_AUTH_BROWSER_PROFILE_RESET === "1";
const BROWSER_OPEN_CONCURRENT = parseBoundedIntegerEnv(
  "HOME_VIRTUAL_AUTH_BROWSER_OPEN_CONCURRENT",
  1,
  1,
  4,
);
const BROWSER_OPEN_HOLD_MS = parseBoundedIntegerEnv(
  "HOME_VIRTUAL_AUTH_BROWSER_OPEN_HOLD_MS",
  0,
  0,
  300_000,
);
const EXPECT_BROWSER_CAPACITY_REJECTION =
  process.env.HOME_VIRTUAL_AUTH_BROWSER_EXPECT_CAPACITY_REJECTION === "1";
const BROWSER_OPEN_DISPLAY_MODE = parseBrowserDisplayMode(
  process.env.HOME_VIRTUAL_AUTH_BROWSER_OPEN_DISPLAY_MODE || "webrtc_remote_display",
);
const BROWSER_REMOTE_EXIT_ID = parseOptionalSafeRuntimeId(
  process.env.HOME_VIRTUAL_AUTH_BROWSER_REMOTE_EXIT_ID || "",
  "HOME_VIRTUAL_AUTH_BROWSER_REMOTE_EXIT_ID",
);
const BROWSER_OPEN_GUARANTEE_LEVEL =
  process.env.HOME_VIRTUAL_AUTH_BROWSER_OPEN_GUARANTEE_LEVEL || "";
const CHECK_BROWSER_FRAME = process.env.HOME_VIRTUAL_AUTH_BROWSER_FRAME !== "0";
const CHECK_BROWSER_INPUT = process.env.HOME_VIRTUAL_AUTH_BROWSER_INPUT === "1";
const CHECK_BROWSER_DIAGNOSTICS =
  process.env.HOME_VIRTUAL_AUTH_BROWSER_DIAGNOSTICS === "1";
const BROWSER_DIAGNOSTIC_CLICK_TEXT_RE =
  process.env.HOME_VIRTUAL_AUTH_BROWSER_DIAGNOSTIC_CLICK_TEXT_RE || "";
const BROWSER_DIAGNOSTIC_CLICK_EXPECT_TEXT_RE =
  process.env.HOME_VIRTUAL_AUTH_BROWSER_DIAGNOSTIC_CLICK_EXPECT_TEXT_RE || "";
const BROWSER_DIAGNOSTIC_CLICK_OPTIONAL =
  process.env.HOME_VIRTUAL_AUTH_BROWSER_DIAGNOSTIC_CLICK_OPTIONAL === "1";
const BROWSER_DIAGNOSTIC_CLICK_WAIT_MS = parseBoundedIntegerEnv(
  "HOME_VIRTUAL_AUTH_BROWSER_DIAGNOSTIC_CLICK_WAIT_MS",
  1500,
  0,
  30_000,
);
const BROWSER_INPUT_CLICK_X = parseBoundedIntegerEnv(
  "HOME_VIRTUAL_AUTH_BROWSER_INPUT_CLICK_X",
  640,
  0,
  4096,
);
const BROWSER_INPUT_CLICK_Y = parseBoundedIntegerEnv(
  "HOME_VIRTUAL_AUTH_BROWSER_INPUT_CLICK_Y",
  350,
  0,
  4096,
);
const BROWSER_INPUT_MAX_MS = parseBoundedIntegerEnv(
  "HOME_VIRTUAL_AUTH_BROWSER_INPUT_MAX_MS",
  2500,
  1,
  60_000,
);
const BROWSER_INPUT_EXPECT_URL_RE = process.env.HOME_VIRTUAL_AUTH_BROWSER_INPUT_EXPECT_URL_RE || "";
const BROWSER_OPEN_VIEWPORT_WIDTH = parseBoundedIntegerEnv(
  "HOME_VIRTUAL_AUTH_BROWSER_OPEN_VIEWPORT_WIDTH",
  1280,
  320,
  3840,
);
const BROWSER_OPEN_VIEWPORT_HEIGHT = parseBoundedIntegerEnv(
  "HOME_VIRTUAL_AUTH_BROWSER_OPEN_VIEWPORT_HEIGHT",
  720,
  240,
  2160,
);
const CHECK_BROWSER_UI_INPUT = process.env.HOME_VIRTUAL_AUTH_BROWSER_UI_INPUT === "1";
const CHECK_BROWSER_UI_SETUP = process.env.HOME_VIRTUAL_AUTH_BROWSER_UI_SETUP === "1";
const BROWSER_UI_PAGE_ID_TIMEOUT_MS = parseBoundedIntegerEnv(
  "HOME_VIRTUAL_AUTH_BROWSER_UI_PAGE_ID_TIMEOUT_MS",
  180_000,
  1_000,
  300_000,
);
const BROWSER_REMOTE_VIDEO_TIMEOUT_MS = parseBoundedIntegerEnv(
  "HOME_VIRTUAL_AUTH_BROWSER_REMOTE_VIDEO_TIMEOUT_MS",
  180_000,
  1_000,
  300_000,
);
const CHECK_BROWSER_AUDIO_STATS =
  process.env.HOME_VIRTUAL_AUTH_BROWSER_AUDIO_STATS === "1";
const BROWSER_REMOTE_AUDIO_TIMEOUT_MS = parseBoundedIntegerEnv(
  "HOME_VIRTUAL_AUTH_BROWSER_REMOTE_AUDIO_TIMEOUT_MS",
  45_000,
  1_000,
  180_000,
);
const BROWSER_UI_CLICK_EXPECT_URL_RE =
  process.env.HOME_VIRTUAL_AUTH_BROWSER_UI_CLICK_EXPECT_URL_RE || "";
const BROWSER_UI_CLICK_HREF_RE =
  process.env.HOME_VIRTUAL_AUTH_BROWSER_UI_CLICK_HREF_RE || "";
const BROWSER_UI_CLICK_NAV_TIMEOUT_MS = parseBoundedIntegerEnv(
  "HOME_VIRTUAL_AUTH_BROWSER_UI_CLICK_NAV_TIMEOUT_MS",
  30_000,
  1_000,
  120_000,
);
const BROWSER_UI_CLICK_TARGET_TIMEOUT_MS = parseBoundedIntegerEnv(
  "HOME_VIRTUAL_AUTH_BROWSER_UI_CLICK_TARGET_TIMEOUT_MS",
  90_000,
  1_000,
  300_000,
);
const CHECK_BROWSER_EMBEDDED_UI_INPUT =
  process.env.HOME_VIRTUAL_AUTH_BROWSER_EMBEDDED_UI_INPUT === "1";
const CHECK_BROWSER_EMBEDDED_RECOVERY =
  process.env.HOME_VIRTUAL_AUTH_BROWSER_EMBEDDED_RECOVERY === "1";
const BROWSER_OPEN_URLS = parseBrowserOpenUrls(process.env.HOME_VIRTUAL_AUTH_BROWSER_OPEN_URLS);
const BROWSER_UI_NAV_URL = parseOptionalBrowserUrl(
  process.env.HOME_VIRTUAL_AUTH_BROWSER_UI_NAV_URL ||
    BROWSER_OPEN_URLS[1] ||
    "https://example.com/?elastos-browser-ui-nav-smoke=1",
  "HOME_VIRTUAL_AUTH_BROWSER_UI_NAV_URL",
);
const APP_MATRIX_TARGETS = parseAppMatrixTargets(process.env.HOME_VIRTUAL_AUTH_APP_MATRIX_TARGETS);
const ALLOW_REMOTE = process.env.HOME_VIRTUAL_AUTH_ALLOW_REMOTE === "1";
const PROFILE_DIR = process.env.HOME_VIRTUAL_AUTH_PROFILE
  || mkdtempSync(join(tmpdir(), "elastos-home-passkey-smoke-"));
const VIRTUAL_AUTH_CREDENTIAL_STORE = join(
  PROFILE_DIR,
  "elastos-virtual-authenticator-credentials.json",
);
let smokeStage = "init";

function markStage(stage) {
  smokeStage = stage;
}

function readVirtualAuthenticatorCredentialStore() {
  if (!existsSync(VIRTUAL_AUTH_CREDENTIAL_STORE)) {
    return [];
  }
  const parsed = JSON.parse(readFileSync(VIRTUAL_AUTH_CREDENTIAL_STORE, "utf8"));
  if (parsed?.schema !== "elastos.home.virtual-authenticator-credentials/v1") {
    throw new Error("virtual authenticator credential store has an unsupported schema");
  }
  if (!Array.isArray(parsed.credentials)) {
    throw new Error("virtual authenticator credential store is missing credentials");
  }
  return parsed.credentials;
}

function hasVirtualAuthenticatorCredentialStore() {
  return existsSync(VIRTUAL_AUTH_CREDENTIAL_STORE);
}

async function restoreVirtualAuthenticatorCredentials(cdp, authenticatorId) {
  for (const credential of readVirtualAuthenticatorCredentialStore()) {
    await cdp.send("WebAuthn.addCredential", {
      authenticatorId,
      credential,
    });
  }
}

async function persistVirtualAuthenticatorCredentials(authenticator) {
  if (!authenticator) {
    return { skipped: true };
  }
  const { credentials } = await authenticator.cdp.send("WebAuthn.getCredentials", {
    authenticatorId: authenticator.authenticatorId,
  });
  mkdirSync(PROFILE_DIR, { recursive: true });
  writeFileSync(
    VIRTUAL_AUTH_CREDENTIAL_STORE,
    `${JSON.stringify({
      schema: "elastos.home.virtual-authenticator-credentials/v1",
      generated_at: new Date().toISOString(),
      credentials,
    }, null, 2)}\n`,
    { mode: 0o600 },
  );
  chmodSync(VIRTUAL_AUTH_CREDENTIAL_STORE, 0o600);
  return { saved: true, credential_count: credentials.length };
}

function parseBoundedIntegerEnv(name, defaultValue, min, max) {
  const raw = process.env[name];
  if (raw == null || raw === "") {
    return defaultValue;
  }
  const value = Number(raw);
  if (!Number.isInteger(value) || value < min || value > max) {
    throw new Error(`${name} must be an integer between ${min} and ${max}`);
  }
  return value;
}

function parseBrowserOpenUrls(raw) {
  const defaults = [
    "https://example.com/",
    "https://example.org/",
    "https://example.net/",
    "https://example.edu/",
  ];
  if (raw == null || raw.trim() === "") {
    return defaults;
  }
  const urls = raw
    .split(/[\n,]/)
    .map((entry) => entry.trim())
    .filter(Boolean);
  if (urls.length === 0 || urls.length > 4) {
    throw new Error("HOME_VIRTUAL_AUTH_BROWSER_OPEN_URLS must include 1 to 4 http(s) URLs");
  }
  for (const value of urls) {
    const url = new URL(value);
    if (!["http:", "https:"].includes(url.protocol)) {
      throw new Error(`HOME_VIRTUAL_AUTH_BROWSER_OPEN_URLS contains unsupported URL: ${value}`);
    }
  }
  return urls;
}

function parseOptionalBrowserUrl(raw, name) {
  const value = String(raw || "").trim();
  if (!value) {
    return "";
  }
  let parsed;
  try {
    parsed = new URL(value);
  } catch {
    throw new Error(`${name} must be an http(s) URL`);
  }
  if (!["http:", "https:"].includes(parsed.protocol)) {
    throw new Error(`${name} must be an http(s) URL`);
  }
  return parsed.toString();
}

function parseOptionalSafeRuntimeId(raw, name) {
  const value = String(raw || "").trim();
  if (!value) {
    return "";
  }
  if (value.length > 128 || !/^[A-Za-z0-9:_-]+$/.test(value)) {
    throw new Error(`${name} must be a safe Runtime identifier up to 128 bytes`);
  }
  return value;
}

function redactSensitiveString(value) {
  return String(value)
    .replace(/([?&]home_token=)[^&#\s"]+/gi, "$1[redacted]")
    .replace(/(#home_token=)[^&#\s"]+/gi, "$1[redacted]")
    .replace(/("home_token"\s*:\s*")[^"]+(")/gi, "$1[redacted]$2")
    .replace(/\bperson:local:[a-z0-9]+\b/gi, "person:local:[redacted]")
    .replace(/\bproof:passkey:[^\s"',}]+\b/gi, "proof:passkey:[redacted]")
    .replace(/\bdid:key:[a-z0-9]+\b/gi, "did:key:[redacted]");
}

function redactSensitive(value) {
  if (typeof value === "string") {
    return redactSensitiveString(value);
  }
  if (Array.isArray(value)) {
    return value.map(redactSensitive);
  }
  if (!value || typeof value !== "object") {
    return value;
  }
  return Object.fromEntries(
    Object.entries(value).map(([key, entry]) => [
      key,
      ["credential", "auth_secret", "transport_secret"].includes(key)
        ? "[redacted]"
        : redactSensitive(entry),
    ]),
  );
}

function hasTurnIceServer(iceServers) {
  if (!Array.isArray(iceServers)) {
    return false;
  }
  return iceServers.some((server) => {
    const urls = Array.isArray(server?.urls) ? server.urls : [server?.urls];
    return urls.some((url) => /^turns?:/i.test(String(url || "").trim()));
  });
}

function hasCredentialedTurnIceServer(iceServers) {
  if (!Array.isArray(iceServers)) {
    return false;
  }
  return iceServers.some((server) => {
    const urls = Array.isArray(server?.urls) ? server.urls : [server?.urls];
    const hasTurn = urls.some((url) => /^turns?:/i.test(String(url || "").trim()));
    const usernamePresent =
      (typeof server?.username === "string" && server.username.trim() !== "") ||
      server?.username_present === true;
    const credentialPresent =
      (typeof server?.credential === "string" && server.credential !== "") ||
      server?.credential_present === true ||
      Number(server?.credential_length || 0) > 0;
    return hasTurn &&
      usernamePresent &&
      credentialPresent;
  });
}

function runtimeRelayIceContractOk(displaySession) {
  if (displaySession?.ice_connection_policy === "engine_relay_only") {
    return displaySession?.offerer === "engine" &&
      displaySession?.ice_servers === undefined;
  }
  return hasTurnIceServer(displaySession?.ice_servers) &&
    hasCredentialedTurnIceServer(displaySession?.ice_servers);
}

function summarizeDisplaySession(displaySession) {
  const iceServers = Array.isArray(displaySession?.ice_servers)
    ? displaySession.ice_servers
    : [];
  return {
    schema: displaySession?.schema || null,
    mode: displaySession?.mode || null,
    media_transport: displaySession?.media_transport || null,
    display_backend: displaySession?.display_backend || null,
    backend_class: displaySession?.backend_class || null,
    offerer: displaySession?.offerer || null,
    ice_connection_policy:
      displaySession?.ice_connection_policy || null,
    ice_servers: iceServers.map((server) => ({
      urls: Array.isArray(server?.urls) ? server.urls : [server?.urls].filter(Boolean),
      username_present: (typeof server?.username === "string" && server.username.trim() !== "") ||
        server?.username_present === true,
      credential_present: (typeof server?.credential === "string" && server.credential !== "") ||
        server?.credential_present === true ||
        Number(server?.credential_length || 0) > 0,
      credential_length: typeof server?.credential === "string"
        ? server.credential.length
        : Number(server?.credential_length || 0),
    })),
  };
}

function summarizeSdp(sdp) {
  const text = String(sdp || "");
  const lines = text.split(/\r?\n/).filter(Boolean);
  return {
    bytes: text.length,
    media: lines.filter((line) => line.startsWith("m=")).slice(0, 8),
    directions: lines.filter((line) =>
      ["a=sendrecv", "a=recvonly", "a=sendonly", "a=inactive"].includes(line)
    ).slice(0, 8),
    candidate_lines: lines.filter((line) => line.startsWith("a=candidate:")).length,
    end_of_candidates: lines.includes("a=end-of-candidates"),
    ice_ufrag_present: lines.some((line) => line.startsWith("a=ice-ufrag:")),
    ice_pwd_present: lines.some((line) => line.startsWith("a=ice-pwd:")),
  };
}

function summarizeWebrtcCandidate(candidate) {
  const line = String(candidate?.candidate || "");
  const tokens = line.trim().split(/\s+/);
  let type = "";
  for (let index = 0; index < tokens.length - 1; index += 1) {
    if (tokens[index].toLowerCase() === "typ") {
      type = tokens[index + 1];
      break;
    }
  }
  return {
    present: Boolean(line.trim()),
    type,
    protocol: tokens[2] || "",
    address: tokens[4] || "",
    port_present: Boolean(tokens[5]),
    sdp_mid: typeof candidate?.sdpMid === "string" ? candidate.sdpMid : null,
    sdp_mline_index: Number.isInteger(candidate?.sdpMLineIndex) ? candidate.sdpMLineIndex : null,
    bytes: line.length,
  };
}

function summarizeWebrtcMessage(body) {
  if (!body || typeof body !== "object") {
    return { body_type: typeof body };
  }
  return {
    schema: body.schema || null,
    type: body.type || null,
    channel: body.channel || null,
    accepted: body.accepted,
    reason: body.reason || null,
    sdp: body.sdp ? summarizeSdp(body.sdp) : null,
    candidate: body.candidate ? summarizeWebrtcCandidate(body.candidate) : null,
    candidates: Array.isArray(body.candidates)
      ? body.candidates.slice(0, 8).map(summarizeWebrtcCandidate)
      : null,
    candidate_count: Array.isArray(body.candidates) ? body.candidates.length : null,
    end_of_candidates: body.end_of_candidates,
  };
}

function attachWebrtcSignalCapture(page) {
  const signals = [];
  page.on("response", async (response) => {
    const request = response.request();
    if (request.method() !== "POST" || !response.url().includes("/webrtc")) {
      return;
    }
    let requestBody = null;
    try {
      requestBody = JSON.parse(request.postData() || "null");
    } catch {
      requestBody = request.postData() || "";
    }
    let responseBody = null;
    try {
      responseBody = await response.json();
    } catch {
      responseBody = await response.text().catch(() => "");
    }
    signals.push({
      url: response.url(),
      status: response.status(),
      request: summarizeWebrtcMessage(requestBody),
      response: summarizeWebrtcMessage(responseBody),
    });
  });
  return signals;
}

function parseBrowserDisplayMode(value) {
  if (value === "webrtc_remote_display") {
    return value;
  }
  throw new Error(
    "HOME_VIRTUAL_AUTH_BROWSER_OPEN_DISPLAY_MODE must be webrtc_remote_display",
  );
}

function parseBrowserGuaranteeLevel(value) {
  if (
    value === "mechanism_microvm" ||
    value === "operator_rbi" ||
    value === "policy_webview" ||
    value === "diagnostic"
  ) {
    return value;
  }
  throw new Error(
    "HOME_VIRTUAL_AUTH_BROWSER_OPEN_GUARANTEE_LEVEL must be mechanism_microvm, operator_rbi, policy_webview, or diagnostic",
  );
}

function browserOpenGuaranteeLevel(engineAdapter) {
  if (BROWSER_OPEN_GUARANTEE_LEVEL) {
    return parseBrowserGuaranteeLevel(BROWSER_OPEN_GUARANTEE_LEVEL);
  }
  const levels = Array.isArray(engineAdapter?.supported_guarantee_levels)
    ? engineAdapter.supported_guarantee_levels
    : [];
  if (levels.includes("mechanism_microvm")) {
    return "mechanism_microvm";
  }
  if (
    BROWSER_OPEN_DISPLAY_MODE === "webrtc_remote_display" &&
    levels.includes("operator_rbi")
  ) {
    return "operator_rbi";
  }
  return "mechanism_microvm";
}

function publicBrowserStreamSession(session) {
  if (!session || typeof session !== "object") {
    return null;
  }
  const carrier = session.carrier && typeof session.carrier === "object"
    ? session.carrier
    : {};
  return {
    schema: session.schema || null,
    byte_transport: session.byte_transport || null,
    grant_id: session.grant_id || null,
    stream_id: session.stream_id || null,
    target: session.target || null,
    carrier_service: session.carrier_service || carrier.carrier_service || null,
    backend: session.backend || null,
    carrier_schema: carrier.schema || null,
    carrier_peer_did: carrier.peer_did || null,
    carrier_connect_ticket_exposed: carrier.connect_ticket != null,
    adapter_ipc_exposed: session.adapter_ipc != null,
    relay_ipc_exposed: session.relay_ipc != null,
    accounting: session.accounting ? {
      max_active_streams: session.accounting.max_active_streams ?? null,
      active_streams: session.accounting.active_streams ?? null,
      max_active_streams_per_principal: session.accounting.max_active_streams_per_principal ?? null,
      principal_active_streams: session.accounting.principal_active_streams ?? null,
      principal_active_streams_remaining: session.accounting.principal_active_streams_remaining ?? null,
    } : null,
  };
}

function parseAppMatrixTargets(raw) {
  const defaults = [
    "system",
    "chat-room",
    "wallet",
    "library",
    "archive-manager",
    "marketplace",
    "browser",
    "documents",
    "inbox",
    "people",
    "gba-ucity",
  ];
  const values = raw == null || raw.trim() === ""
    ? defaults
    : raw
      .split(/[\n,]/)
      .map((entry) => entry.trim())
      .filter(Boolean);
  const seen = new Set();
  return values.filter((value) => {
    if (seen.has(value)) {
      return false;
    }
    seen.add(value);
    return true;
  });
}

function assert(condition, message, details = null) {
  if (!condition) {
    const error = new Error(message);
    error.details = details;
    throw error;
  }
}

function browserCloseAlreadyInactive(response) {
  if (!response || response.ok === true || response.status !== 404) {
    return false;
  }
  const text = JSON.stringify(response.body || response.error || "");
  return text.includes("browser session is not active");
}

function assertBrowserCloseOkOrInactive(response, message) {
  assert(response.ok || browserCloseAlreadyInactive(response), message, response);
}

function isLoopbackUrl(value) {
  const url = new URL(value);
  return url.hostname === "127.0.0.1" || url.hostname === "localhost" || url.hostname === "::1";
}

function isLocalhostWebAuthnUrl(value) {
  return new URL(value).hostname === "localhost";
}

async function waitForHomeReady(page, timeoutMs = 30_000) {
  await page.waitForFunction(
    () => document.body?.dataset?.homeStatus === "ready",
    null,
    { timeout: timeoutMs },
  );
}

function launchTokenFromRoute(route) {
  const url = new URL(route, HOME_URL);
  return new URLSearchParams(url.hash.replace(/^#/, "")).get("home_token") || "";
}

function capsuleFrameForTarget(page, target) {
  return page.frames().find((frame) => {
    try {
      const url = new URL(frame.url());
      return url.hostname.split(".")[0] === target && url.pathname.startsWith(`/apps/${target}/`);
    } catch {
      return false;
    }
  }) || null;
}

async function waitForCapsuleFrame(page, target, timeoutMs = 30_000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const frame = capsuleFrameForTarget(page, target);
    if (frame) {
      return frame;
    }
    await delay(100);
  }
  throw Object.assign(new Error(`Timed out waiting for isolated ${target} frame`), {
    details: { target, frames: page.frames().map((frame) => frame.url()) },
  });
}

function assertIsolatedLaunchRoute(route, target) {
  const url = new URL(route, HOME_URL);
  const token = launchTokenFromRoute(route);
  assert(token, `${target} launch route did not contain a fragment-scoped token`, { route });
  assert(!url.searchParams.has("home_token"), `${target} launch token leaked into the query`, { route });
  assert(
    url.origin !== new URL(HOME_URL).origin,
    `${target} launch route reused the trusted Home origin`,
    { route },
  );
  return token;
}

async function homeState(page) {
  return page.evaluate(() => ({
    status: document.body?.dataset?.homeStatus || "",
    authority: document.body?.dataset?.homeAuthority || "",
    shell: document.body?.dataset?.homeShell || "",
    gui: document.body?.dataset?.homeGui || "",
    unlockVisible: !document.querySelector("#home-unlock")?.hidden,
    unlockTitle: document.querySelector("#home-unlock-title")?.textContent?.trim() || "",
    unlockPrimary: document.querySelector("#home-unlock-primary")?.textContent?.trim() || "",
    unlockSecondary: document.querySelector("#home-unlock-secondary")?.textContent?.trim() || "",
    unlockSecondaryHidden: document.querySelector("#home-unlock-secondary")?.hidden ?? true,
    unlockNameVisible: !(document.querySelector("#home-unlock-name")?.hidden ?? true),
    unlockStatus: document.querySelector("#home-unlock-status")?.textContent?.trim() || "",
    activeShellRootHidden: document.querySelector("#active-shell-root")?.hidden !== false,
    activeShellFrameSrc: document.querySelector("#active-shell-frame")?.getAttribute("src") || "",
    hostGuiDomPresent: Boolean(document.querySelector(
      "#desktop, .desktop-backdrop, .toolbar, .desktop-workspace, .taskbar, #launcher, #window-template",
    )),
  }));
}

async function waitForSignedHome(page, timeoutMs = 30_000) {
  await page.waitForFunction(
    () => document.body?.dataset?.homeStatus === "ready"
      && document.body?.dataset?.homeAuthority === "signed"
      && document.querySelector("#active-shell-root")?.hidden === false
      && Boolean(document.querySelector("#active-shell-frame")?.getAttribute("src")),
    null,
    { timeout: timeoutMs },
  );
}

async function setupVirtualAuthenticator(context, page) {
  const cdp = await context.newCDPSession(page);
  await cdp.send("WebAuthn.enable");
  const { authenticatorId } = await cdp.send("WebAuthn.addVirtualAuthenticator", {
    options: {
      protocol: "ctap2",
      transport: "internal",
      hasResidentKey: true,
      hasUserVerification: true,
      isUserVerified: true,
      automaticPresenceSimulation: true,
    },
  });
  await restoreVirtualAuthenticatorCredentials(cdp, authenticatorId);
  return { cdp, authenticatorId };
}

function captureNextPasskeyToken(page, timeoutMs = 30_000) {
  return page.waitForResponse((response) => {
    const url = response.url();
    return response.request().method() === "POST"
      && (url.endsWith("/api/auth/passkey/register/complete")
        || url.endsWith("/api/auth/passkey/authenticate/complete"));
  }, { timeout: timeoutMs }).then(async (response) => {
    const text = await response.text();
    let body = {};
    try {
      body = text ? JSON.parse(text) : {};
    } catch {
      body = { raw: text };
    }
    assert(response.ok(), "passkey completion response failed", {
      status: response.status(),
      body,
    });
    assert(body.home_token, "passkey completion did not return a Home token", body);
    return body.home_token;
  });
}

function delay(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

async function browserApi(page, token, path, { method = "GET", body = null } = {}) {
  return page.evaluate(async ({ token, path, method, body }) => {
    const headers = { "x-elastos-home-token": token };
    let requestBody;
    if (body != null) {
      headers["content-type"] = "application/json";
      requestBody = JSON.stringify(body);
    }
    const response = await fetch(path, {
      method,
      headers,
      body: requestBody,
    });
    const text = await response.text();
    let payload = {};
    try {
      payload = text ? JSON.parse(text) : {};
    } catch {
      payload = { raw: text };
    }
    return { ok: response.ok, status: response.status, body: payload };
  }, { token, path, method, body });
}

async function waitForBrowserOpenResult(page, browserToken, initialResult, timeoutMs) {
  if (initialResult?.body?.schema === "elastos.browser.open-result/v1") {
    return initialResult;
  }
  const statusUrl = initialResult?.body?.status_url || "";
  if (
    !initialResult?.ok ||
    initialResult?.body?.schema !== "elastos.browser.open-accepted/v1" ||
    typeof statusUrl !== "string" ||
    !statusUrl
  ) {
    return initialResult;
  }
  const started = Date.now();
  let last = initialResult;
  while (Date.now() - started <= timeoutMs) {
    await delay(500);
    const status = await browserApi(page, browserToken, statusUrl);
    last = status;
    if (status.body?.schema !== "elastos.browser.open-status/v1") {
      return status;
    }
    if (status.body.status === "completed") {
      return {
        ok: true,
        status: 200,
        body: status.body.result || {},
      };
    }
    if (status.body.status === "failed") {
      return {
        ok: false,
        status: status.body.error?.http_status || 500,
        body: status.body.error || status.body,
      };
    }
  }
  return {
    ok: false,
    status: 408,
    body: {
      schema: "elastos.browser.open-timeout/v1",
      status_url: statusUrl,
      last,
    },
  };
}

async function waitForBrowserStatus(page, browserToken, pageId) {
  let lastStatus = null;
  for (let attempt = 0; attempt < 20; attempt += 1) {
    const status = await browserApi(
      page,
      browserToken,
      `/api/apps/browser/pages/${encodeURIComponent(pageId)}/status`,
    );
    assert(status.ok, `Browser status request failed for ${pageId}`, status);
    assert(
      status.body?.schema === "elastos.browser.page-status/v1",
      "Browser status returned wrong schema",
      status,
    );
    lastStatus = status.body;
    if (lastStatus.display_session?.mode === "webrtc_remote_display") {
      assert(lastStatus.direct_network === false, "Browser status reported direct network", lastStatus);
      return {
        actual_url: lastStatus.actual_url,
        display_backend: lastStatus.display_session?.display_backend,
        display_mode: lastStatus.display_session?.mode,
        media_transport: lastStatus.display_session?.media_transport,
        webrtc_connection_state: lastStatus.webrtc_connection_state,
      };
    }
    await delay(250);
  }
  throw Object.assign(new Error(`Browser status did not report a WebRTC display for ${pageId}`), {
    details: lastStatus,
  });
}

async function checkBrowserRuntimeInput(page, browserToken, pageId) {
  const started = Date.now();
  const input = await browserApi(
    page,
    browserToken,
    `/api/apps/browser/pages/${encodeURIComponent(pageId)}/input`,
    {
      method: "POST",
      body: {
        event: {
          type: "click",
          x: BROWSER_INPUT_CLICK_X,
          y: BROWSER_INPUT_CLICK_Y,
        },
      },
    },
  );
  const durationMs = Date.now() - started;
  assert(input.ok, `Browser input request failed for ${pageId}`, input);
  assert(input.body?.schema === "elastos.browser.input-result/v1", "Browser input returned wrong schema", input);
  assert(input.body.accepted === true, "Browser input was not accepted", input);
  assert(input.body.direct_network === false, "Browser input reported direct network", input.body);
  assert(
    durationMs <= BROWSER_INPUT_MAX_MS,
    "Browser input exceeded latency budget",
    { duration_ms: durationMs, max_ms: BROWSER_INPUT_MAX_MS, input: input.body },
  );
  const status = await waitForBrowserStatus(page, browserToken, pageId);
  if (BROWSER_INPUT_EXPECT_URL_RE) {
    const pattern = new RegExp(BROWSER_INPUT_EXPECT_URL_RE);
    assert(
      pattern.test(String(status.actual_url || "")),
      "Browser input did not produce the expected URL",
      { expected: BROWSER_INPUT_EXPECT_URL_RE, status },
    );
  }
  return {
    accepted: input.body.accepted,
    duration_ms: durationMs,
    click: { x: BROWSER_INPUT_CLICK_X, y: BROWSER_INPUT_CLICK_Y },
    seq: input.body.seq,
    actual_url: input.body.actual_url,
    title: input.body.title,
    status,
  };
}

async function checkBrowserPageStatus(page, browserToken, pageId) {
  const status = await browserApi(
    page,
    browserToken,
    `/api/apps/browser/pages/${encodeURIComponent(pageId)}/status`,
  );
  assert(status.ok, `Browser status request failed for ${pageId}`, status);
  assert(
    status.body?.schema === "elastos.browser.page-status/v1",
    "Browser status returned wrong schema",
    status,
  );
  assert(status.body.direct_network === false, "Browser status reported direct network", status.body);
  return {
    actual_url: status.body.actual_url,
    title: status.body.title,
    can_go_back: status.body.can_go_back,
    can_go_forward: status.body.can_go_forward,
    display_backend: status.body.display_backend,
    backend_class: status.body.backend_class,
    display_session: summarizeDisplaySession(status.body.display_session),
    engine_identity: status.body.engine_identity || null,
    input_protocol: status.body.input_protocol,
    audio: status.body.audio,
    video: status.body.video,
    frame_count: status.body.frame_count,
    last_frame_width: status.body.last_frame_width,
    last_frame_height: status.body.last_frame_height,
    webrtc_connection_state: status.body.webrtc_connection_state,
    webrtc_signaling: status.body.webrtc_signaling || null,
  };
}

async function waitForBrowserPageStatus(page, browserToken, pageId, predicate, label, timeoutMs = 60_000) {
  const started = Date.now();
  let last = null;
  while (Date.now() - started <= timeoutMs) {
    last = await checkBrowserPageStatus(page, browserToken, pageId);
    if (predicate(last)) {
      return last;
    }
    await delay(500);
  }
  throw Object.assign(new Error(`Timed out waiting for Browser status: ${label}`), {
    details: last,
  });
}

async function checkBrowserPageDiagnostics(page, browserToken, pageId) {
  const diagnostics = await browserApi(
    page,
    browserToken,
    `/api/apps/browser/pages/${encodeURIComponent(pageId)}/diagnostics`,
  );
  assert(diagnostics.ok, `Browser diagnostics request failed for ${pageId}`, diagnostics);
  assert(
    diagnostics.body?.schema === "elastos.browser.page-diagnostics/v1",
    "Browser diagnostics returned wrong schema",
    diagnostics,
  );
  assert(
    diagnostics.body.direct_network === false,
    "Browser diagnostics reported direct network",
    diagnostics.body,
  );
  return {
    url: diagnostics.body.url,
    title: diagnostics.body.title,
    ready_state: diagnostics.body.ready_state,
    viewport_width: diagnostics.body.viewport_width,
    viewport_height: diagnostics.body.viewport_height,
    device_pixel_ratio: diagnostics.body.device_pixel_ratio,
    body_text: diagnostics.body.body_text,
    body_html: diagnostics.body.body_html,
    body_child_count: diagnostics.body.body_child_count,
    root_child_count: diagnostics.body.root_child_count,
    root_html: diagnostics.body.root_html,
    root_outer_html: diagnostics.body.root_outer_html,
    cdp_events: Array.isArray(diagnostics.body.cdp_events)
      ? diagnostics.body.cdp_events.slice(0, 40)
      : [],
    vm_log_tails: diagnostics.body.vm_log_tails || null,
    wallet_bridge: diagnostics.body.wallet_bridge || null,
    storage: diagnostics.body.storage || null,
    image_count: diagnostics.body.image_count,
    media_element_count: diagnostics.body.media_element_count,
    media_elements: Array.isArray(diagnostics.body.media_elements)
      ? diagnostics.body.media_elements.slice(0, 20)
      : [],
    visible_image_count: diagnostics.body.visible_image_count,
    broken_image_count: diagnostics.body.broken_image_count,
    pending_image_count: diagnostics.body.pending_image_count,
    pending_ipfs_image_count: diagnostics.body.pending_ipfs_image_count,
    visible_broken_image_count: diagnostics.body.visible_broken_image_count,
    visible_pending_image_count: diagnostics.body.visible_pending_image_count,
    visible_pending_ipfs_image_count: diagnostics.body.visible_pending_ipfs_image_count,
    resource_count: diagnostics.body.resource_count,
    resources: Array.isArray(diagnostics.body.resources)
      ? diagnostics.body.resources.slice(0, 80)
      : [],
    ipfs_resources: Array.isArray(diagnostics.body.ipfs_resources)
      ? diagnostics.body.ipfs_resources.slice(0, 80)
      : [],
    clickable_count: diagnostics.body.clickable_count,
    clickable_elements: Array.isArray(diagnostics.body.clickable_elements)
      ? diagnostics.body.clickable_elements.slice(0, 80)
      : [],
    visible_text_sample_count: diagnostics.body.visible_text_sample_count,
    visible_text_samples: Array.isArray(diagnostics.body.visible_text_samples)
      ? diagnostics.body.visible_text_samples.slice(0, 80)
      : [],
    dialog_count: diagnostics.body.dialog_count,
    dialog_elements: Array.isArray(diagnostics.body.dialog_elements)
      ? diagnostics.body.dialog_elements.slice(0, 20)
      : [],
    images: Array.isArray(diagnostics.body.images)
      ? diagnostics.body.images.slice(0, 80)
      : [],
    navigation: diagnostics.body.navigation || null,
  };
}

function diagnosticClickPatterns() {
  return String(BROWSER_DIAGNOSTIC_CLICK_TEXT_RE || "")
    .split(/\s*=>\s*/)
    .map((entry) => entry.trim())
    .filter(Boolean)
    .map((source) => ({ source, regex: new RegExp(source, "i") }));
}

function diagnosticElementText(element) {
  return [
    element?.text,
    element?.aria_label,
    element?.title,
    element?.test_id,
    element?.role,
    element?.tag,
    element?.href,
    element?.top_element?.action_text,
    element?.top_element?.action_href,
  ].filter(Boolean).join("\n");
}

function diagnosticTextCorpus(diagnostics) {
  return [
    diagnostics?.body_text,
    ...(Array.isArray(diagnostics?.visible_text_samples)
      ? diagnostics.visible_text_samples.flatMap((item) => [
          item.text,
          item.aria_label,
          item.title,
          item.test_id,
        ])
      : []),
    ...(Array.isArray(diagnostics?.dialog_elements)
      ? diagnostics.dialog_elements.flatMap((item) => [
          item.text,
          item.aria_label,
          item.title,
          item.test_id,
        ])
      : []),
    ...(Array.isArray(diagnostics?.clickable_elements)
      ? diagnostics.clickable_elements.map(diagnosticElementText)
      : []),
  ].filter(Boolean).join("\n");
}

function summarizeDiagnosticClickTarget(element) {
  return {
    text: element?.text || "",
    aria_label: element?.aria_label || "",
    title: element?.title || "",
    role: element?.role || "",
    tag: element?.tag || "",
    test_id: element?.test_id || "",
    href: element?.href || "",
    rect: element?.rect || null,
  };
}

function diagnosticTargetIsInViewport(diagnostics, element) {
  const rect = element?.rect || {};
  const width = Number(rect.width || 0);
  const height = Number(rect.height || 0);
  const x = Number(rect.x || 0);
  const y = Number(rect.y || 0);
  const centerX = x + width / 2;
  const centerY = y + height / 2;
  const viewportWidth = Number(diagnostics?.viewport_width || 0);
  const viewportHeight = Number(diagnostics?.viewport_height || 0);
  return element?.visible !== false &&
    width > 0 &&
    height > 0 &&
    centerX >= 0 &&
    centerY >= 0 &&
    (!viewportWidth || centerX <= viewportWidth) &&
    (!viewportHeight || centerY <= viewportHeight);
}

function findBrowserHrefClickTarget(diagnostics, pattern) {
  return (diagnostics?.clickable_elements || []).find((element) =>
    diagnosticTargetIsInViewport(diagnostics, element) &&
      pattern.test(String(element.href || "")) &&
      pattern.test(String(element.top_element?.action_href || element.href || "")),
  );
}

async function waitForBrowserHrefClickTarget(
  page,
  browserToken,
  pageId,
  hrefPatternSource,
  timeoutMs = BROWSER_UI_CLICK_TARGET_TIMEOUT_MS,
) {
  const pattern = new RegExp(hrefPatternSource);
  const started = Date.now();
  let diagnostics = null;
  while (Date.now() - started <= timeoutMs) {
    diagnostics = await checkBrowserPageDiagnostics(page, browserToken, pageId);
    const target = findBrowserHrefClickTarget(diagnostics, pattern);
    if (target) {
      return {
        target,
        diagnostics,
        waited_ms: Date.now() - started,
      };
    }
    await delay(1000);
  }
  throw Object.assign(new Error("Browser UI click target was not found in page diagnostics"), {
    details: {
      expected_href_re: hrefPatternSource,
      timeout_ms: timeoutMs,
      diagnostics,
    },
  });
}

async function runBrowserDiagnosticClickSequence(page, browserToken, pageId, initialDiagnostics) {
  const patterns = diagnosticClickPatterns();
  if (patterns.length === 0) {
    return [];
  }
  const actions = [];
  let diagnostics = initialDiagnostics;
  for (const pattern of patterns) {
    const target = (diagnostics.clickable_elements || []).find((element) =>
      diagnosticTargetIsInViewport(diagnostics, element) &&
        pattern.regex.test(diagnosticElementText(element)),
    );
    if (!target && BROWSER_DIAGNOSTIC_CLICK_OPTIONAL) {
      actions.push({
        ok: false,
        error: "target_not_found",
        expected_text_re: pattern.source,
        diagnostics,
      });
      return actions;
    }
    assert(target, "Browser diagnostics click target was not found", {
      page_id: pageId,
      expected_text_re: pattern.source,
      visible_text_samples: diagnostics.visible_text_samples,
      clickable_elements: diagnostics.clickable_elements,
      dialog_elements: diagnostics.dialog_elements,
    });
    const click = {
      x: Math.round(Number(target.rect.x || 0) + Number(target.rect.width || 0) / 2),
      y: Math.round(Number(target.rect.y || 0) + Number(target.rect.height || 0) / 2),
    };
    const response = await browserApi(
      page,
      browserToken,
      `/api/apps/browser/pages/${encodeURIComponent(pageId)}/input`,
      { method: "POST", body: { event: { type: "click", ...click } } },
    );
    assert(response.ok, "Browser diagnostics click input failed", {
      pattern: pattern.source,
      click,
      response,
    });
    assert(
      response.body?.schema === "elastos.browser.input-result/v1" &&
        response.body?.accepted === true &&
        response.body?.direct_network === false,
      "Browser diagnostics click input returned an invalid receipt",
      response.body,
    );
    if (BROWSER_DIAGNOSTIC_CLICK_WAIT_MS > 0) {
      await delay(BROWSER_DIAGNOSTIC_CLICK_WAIT_MS);
    }
    diagnostics = await checkBrowserPageDiagnostics(page, browserToken, pageId);
    actions.push({
      ok: true,
      expected_text_re: pattern.source,
      click,
      target: summarizeDiagnosticClickTarget(target),
      input: {
        accepted: response.body.accepted,
        actual_url: response.body.actual_url || null,
        title: response.body.title || null,
      },
      diagnostics,
    });
  }
  if (BROWSER_DIAGNOSTIC_CLICK_EXPECT_TEXT_RE) {
    const expected = new RegExp(BROWSER_DIAGNOSTIC_CLICK_EXPECT_TEXT_RE, "i");
    if (!expected.test(diagnosticTextCorpus(diagnostics)) && BROWSER_DIAGNOSTIC_CLICK_OPTIONAL) {
      actions.push({
        ok: false,
        error: "expected_text_not_found",
        expected_text_re: BROWSER_DIAGNOSTIC_CLICK_EXPECT_TEXT_RE,
        diagnostics,
      });
      return actions;
    }
    assert(expected.test(diagnosticTextCorpus(diagnostics)), "Browser diagnostics post-click expected text was not found", {
      expected_text_re: BROWSER_DIAGNOSTIC_CLICK_EXPECT_TEXT_RE,
      diagnostics,
    });
  }
  return actions;
}

async function browserRemoteVideoMetrics(appPage) {
  return appPage.evaluate(() => {
    const video = document.querySelector("#browser-remote-display");
    if (!video) {
      return { present: false };
    }
    const rect = video.getBoundingClientRect();
    return {
      present: true,
      hidden: video.hidden,
      ready_state: video.readyState,
      paused: video.paused,
      current_time: Number(video.currentTime || 0),
      video_width: Number(video.videoWidth || 0),
      video_height: Number(video.videoHeight || 0),
      decoded_frames: Number(video.webkitDecodedFrameCount || 0),
      dropped_frames: Number(video.webkitDroppedFrameCount || 0),
      client_width: Math.round(rect.width || 0),
      client_height: Math.round(rect.height || 0),
    };
  });
}

async function browserRemoteDisplayMetrics(appPage) {
  return appPage.evaluate(() => window.__elastosBrowserRemoteDisplayMetrics || null);
}

function browserAudioBytes(metrics) {
  return Number(
    metrics?.latestAudioWebrtcStats?.audio_bytes_received ??
      metrics?.latestWebrtcStats?.audio_bytes_received ??
      0,
  );
}

async function waitForBrowserRemoteAudio(
  appPage,
  { timeoutMs = BROWSER_REMOTE_AUDIO_TIMEOUT_MS, browserToken = "", pageId = "" } = {},
) {
  const started = Date.now();
  let last = null;
  while (Date.now() - started <= timeoutMs) {
    last = await browserRemoteDisplayMetrics(appPage).catch(() => null);
    const audioBytes = browserAudioBytes(last);
    if (
      last?.remoteAudioExpected === true &&
      last?.remoteAudioUnlocked === true &&
      Number(last?.remoteAudioTrackCount || 0) > 0 &&
      audioBytes > 0
    ) {
      return {
        duration_ms: Date.now() - started,
        audio_bytes_received: audioBytes,
        audio_track_count: Number(last.remoteAudioTrackCount || 0),
        remote_audio_muted: last.remoteAudioMuted === true,
        remote_audio_paused: last.remoteAudioPaused === true,
        metrics: last,
      };
    }
    await delay(500);
  }
  const statusText = await appPage.locator("#browser-status").innerText().catch(() => "");
  const runtimeDiagnostics = browserToken && pageId
    ? await checkBrowserPageDiagnostics(appPage, browserToken, pageId).catch((error) => ({
        error: error.message || String(error),
        details: error.details || null,
      }))
    : null;
  throw Object.assign(new Error("Browser remote audio did not receive WebRTC audio frames"), {
    details: {
      duration_ms: Date.now() - started,
      metrics: last,
      status: statusText,
      runtime_diagnostics: runtimeDiagnostics,
    },
  });
}

async function waitForBrowserUiAddressMatch(appPage, expectedUrlRe, timeoutMs) {
  const pattern = new RegExp(expectedUrlRe);
  const started = Date.now();
  let lastAddressValue = "";
  while (Date.now() - started <= timeoutMs) {
    lastAddressValue = await appPage.locator("#browser-url").inputValue().catch(() => "");
    if (pattern.test(lastAddressValue)) {
      return {
        address_value: lastAddressValue,
        duration_ms: Date.now() - started,
      };
    }
    await delay(500);
  }
  throw Object.assign(new Error("Browser UI address did not track remote navigation"), {
    details: {
      expected_url_re: expectedUrlRe,
      address_value: lastAddressValue,
      timeout_ms: timeoutMs,
    },
  });
}

function normalizeRemoteDisplayClickInputEvidence(input, status, addressMatch) {
  if (input?.ok === true) {
    return input;
  }
  const statusUrl = String(status?.actual_url || "");
  const addressValue = String(addressMatch?.address_value || "");
  const selkiesDatachannel =
    status?.display_backend === "selkies_gstreamer_webrtc" &&
    status?.input_protocol === "selkies_v1";
  if (!selkiesDatachannel || !statusUrl || statusUrl !== addressValue) {
    return input;
  }
  return {
    ok: true,
    accepted: true,
    transport: "datachannel",
    protocol: "selkies_v1",
    evidence: "browser_address_status_url_sync",
    actual_url: statusUrl,
    previous_http_probe: input?.ok === false ? input : null,
  };
}

async function remoteVideoClickPositionForPagePoint(appPage, point) {
  return appPage.evaluate(({ x, y }) => {
    const video = document.querySelector("#browser-remote-display");
    if (!video) {
      return null;
    }
    const rect = video.getBoundingClientRect();
    const mediaWidth = Number(video.videoWidth || rect.width || 0);
    const mediaHeight = Number(video.videoHeight || rect.height || 0);
    if (rect.width <= 0 || rect.height <= 0 || mediaWidth <= 0 || mediaHeight <= 0) {
      return null;
    }
    const objectFit = getComputedStyle(video).objectFit || "";
    let content = {
      left: rect.left,
      top: rect.top,
      width: rect.width,
      height: rect.height,
    };
    if (objectFit !== "fill") {
      const elementRatio = rect.width / rect.height;
      const mediaRatio = mediaWidth / mediaHeight;
      if (Math.abs(elementRatio - mediaRatio) >= 0.001) {
        if (elementRatio > mediaRatio) {
          const width = rect.height * mediaRatio;
          content = {
            left: rect.left + (rect.width - width) / 2,
            top: rect.top,
            width,
            height: rect.height,
          };
        } else {
          const height = rect.width / mediaRatio;
          content = {
            left: rect.left,
            top: rect.top + (rect.height - height) / 2,
            width: rect.width,
            height,
          };
        }
      }
    }
    return {
      x: content.left - rect.left + (x / mediaWidth) * content.width,
      y: content.top - rect.top + (y / mediaHeight) * content.height,
      media_width: mediaWidth,
      media_height: mediaHeight,
      video_width: rect.width,
      video_height: rect.height,
    };
  }, point);
}

async function waitForBrowserRemoteVideo(
  appPage,
  { browserToken = "", pageId = "", displaySession = null, timeoutMs = 180_000 } = {},
) {
  const started = Date.now();
  let last = null;
  let lastRuntimeStatus = null;
  let videoVisible = false;
  try {
    await appPage.locator("#browser-remote-display").waitFor({
      state: "visible",
      timeout: timeoutMs,
    });
    videoVisible = true;
  } catch {
    last = await browserRemoteVideoMetrics(appPage).catch(() => null);
  }
  while (videoVisible && Date.now() - started <= timeoutMs) {
    last = await browserRemoteVideoMetrics(appPage);
    if (browserToken && pageId) {
      lastRuntimeStatus = await checkBrowserPageStatus(appPage, browserToken, pageId)
        .catch((error) => ({
          error: error.message || String(error),
          details: error.details || null,
        }));
    }
    if (
      last.present &&
      !last.hidden &&
      Number(last.video_width || 0) > 0 &&
      Number(last.video_height || 0) > 0 &&
      (Number(last.current_time || 0) > 0 || Number(last.ready_state || 0) >= 2)
    ) {
      return {
        ...last,
        ready_duration_ms: Date.now() - started,
      };
    }
    await delay(500);
  }
  const statusText = await appPage.locator("#browser-status").innerText().catch(() => "");
  const domState = await appPage.evaluate(() => ({
    href: window.location.href,
    title: document.title,
    body_loading: document.body?.dataset?.loading || "",
    current_page_id: window.__elastosBrowserCurrentPageId || "",
    status: document.querySelector("#browser-status")?.textContent?.trim() || "",
    address: document.querySelector("#browser-url")?.value || "",
  })).catch(() => null);
  const runtimeStatus = browserToken && pageId
    ? await checkBrowserPageStatus(appPage, browserToken, pageId).catch((error) => ({
        error: error.message || String(error),
        details: error.details || null,
      }))
    : null;
  const runtimeDiagnostics = browserToken && pageId
    ? await checkBrowserPageDiagnostics(appPage, browserToken, pageId).catch((error) => ({
        error: error.message || String(error),
        details: error.details || null,
      }))
    : null;
  throw Object.assign(new Error("Browser remote video did not become renderable"), {
    details: {
      duration_ms: Date.now() - started,
      video_visible: videoVisible,
      video: last,
      status: statusText,
      dom: domState,
      display_session: summarizeDisplaySession(displaySession),
      runtime_status: runtimeStatus,
      last_runtime_status_before_failure: lastRuntimeStatus,
      runtime_diagnostics: runtimeDiagnostics,
    },
  });
}

async function embeddedBrowserGeometry(windowLocator, appFrame) {
  const shell = await windowLocator.evaluate((node) => {
    const rect = node.getBoundingClientRect();
    const frame = node.querySelector(".window-frame");
    const frameRect = frame?.getBoundingClientRect();
    const restoreWidth = Number.parseFloat(node.dataset.restoreWidth || "");
    const restoreHeight = Number.parseFloat(node.dataset.restoreHeight || "");
    return {
      active: node.classList.contains("window-active"),
      hidden: node.classList.contains("hidden"),
      aria_hidden: node.getAttribute("aria-hidden") || "",
      width: Math.round(rect.width || 0),
      height: Math.round(rect.height || 0),
      restore_width: Number.isFinite(restoreWidth) ? Math.round(restoreWidth) : null,
      restore_height: Number.isFinite(restoreHeight) ? Math.round(restoreHeight) : null,
      frame_pointer_events: frame ? getComputedStyle(frame).pointerEvents : "",
      frame_width: Math.round(frameRect?.width || 0),
      frame_height: Math.round(frameRect?.height || 0),
    };
  });
  const panel = await appFrame.locator("#browser-render-panel")
    .boundingBox()
    .then((box) => box ? {
      width: Math.round(box.width || 0),
      height: Math.round(box.height || 0),
    } : null)
    .catch(() => null);
  const video = await appFrame.locator("#browser-remote-display")
    .boundingBox()
    .then((box) => box ? {
      width: Math.round(box.width || 0),
      height: Math.round(box.height || 0),
    } : null)
    .catch(() => null);
  return { shell, panel, video };
}

async function embeddedBrowserDebugState(windowLocator, appFrame) {
  const geometry = await embeddedBrowserGeometry(windowLocator, appFrame).catch((error) => ({
    error: error instanceof Error ? error.message : String(error),
  }));
  const frame = await appFrame.evaluate(() => {
    const text = (selector) => document.querySelector(selector)?.textContent?.trim() || "";
    const value = (selector) => document.querySelector(selector)?.value || "";
    const has = (selector) => Boolean(document.querySelector(selector));
    return {
      ready_state: document.readyState,
      href: window.location.href,
      title: document.title,
      current_page_id: window.__elastosBrowserCurrentPageId || "",
      status: text("#browser-status"),
      url_value: value("#browser-url"),
      has_render_panel: has("#browser-render-panel"),
      has_remote_display: has("#browser-remote-display"),
      has_remote_video: has("#browser-remote-display"),
      body_text: (document.body?.innerText || "").slice(0, 2000),
    };
  }).catch((error) => ({
    error: error instanceof Error ? error.message : String(error),
  }));
  return { geometry, frame };
}

function assertEmbeddedBrowserAspect(geometry) {
  const panel = geometry?.panel;
  assert(
    panel && panel.width > 0 && panel.height > 0,
    "Embedded Browser geometry did not expose a render panel",
    geometry,
  );
  const ratio = panel.width / panel.height;
  assert(
    Math.abs(ratio - (16 / 9)) <= 0.04,
    "Embedded Browser render panel is not fitted to the 16:9 remote display aspect",
    { ratio, geometry },
  );
}

async function checkBrowserUiInput(context, browserToken, route) {
  const appPage = await context.newPage();
  let pageId = "";
  let openedPageId = "";
  let openResult = null;
  let closed = null;
  try {
    const appUrl = new URL(route, HOME_URL);
    appUrl.searchParams.set("url", BROWSER_OPEN_URLS[0] || "https://example.com/");
    appUrl.searchParams.set("display", BROWSER_OPEN_DISPLAY_MODE);
    if (CHECK_BROWSER_AUDIO_STATS) {
      appUrl.searchParams.set("metrics", "1");
    }
    if (BROWSER_REMOTE_EXIT_ID) {
      appUrl.searchParams.set("remote_exit_id", BROWSER_REMOTE_EXIT_ID);
    }
    const openResultPromise = appPage.waitForResponse(
      (response) => {
        const request = response.request();
        return request.method() === "POST" && response.url().endsWith("/api/apps/browser/open");
      },
      { timeout: BROWSER_UI_PAGE_ID_TIMEOUT_MS },
    ).then(async (response) => {
      const body = await response.json().catch(() => ({}));
      const result = await waitForBrowserOpenResult(appPage, browserToken, {
        ok: response.ok(),
        status: response.status(),
        body,
      }, BROWSER_UI_PAGE_ID_TIMEOUT_MS);
      openedPageId = String(result.body?.engine_page?.page_id || "");
      return result;
    }).catch((error) => ({
      ok: false,
      error: error.message || String(error),
    }));
    await appPage.goto(appUrl.toString(), { waitUntil: "domcontentloaded" });
    await appPage.evaluate(() => {
      window.__elastosBrowserSmokeClicks = [];
      const panel = document.querySelector("#browser-render-panel");
      panel?.addEventListener("click", (event) => {
        window.__elastosBrowserSmokeClicks.push({
          target: event.target?.id || event.target?.tagName || "",
          currentTarget: event.currentTarget?.id || "",
          clientX: event.clientX,
          clientY: event.clientY,
        });
      }, { capture: true });
    });
    try {
      pageId = await appPage.waitForFunction(
        () => window.__elastosBrowserCurrentPageId || "",
        null,
        { timeout: BROWSER_UI_PAGE_ID_TIMEOUT_MS },
      ).then((handle) => handle.jsonValue());
    } catch (error) {
      openResult = await openResultPromise;
      const domState = await appPage.evaluate(() => ({
        href: window.location.href,
        title: document.title,
        body_loading: document.body?.dataset?.loading || "",
        current_page_id: window.__elastosBrowserCurrentPageId || "",
        status: document.querySelector("#browser-status")?.textContent?.trim() || "",
        address: document.querySelector("#browser-url")?.value || "",
      })).catch(() => null);
      throw Object.assign(new Error("Browser UI did not publish the current page id"), {
        details: { error: error.message || String(error), open_result: openResult, dom: domState },
      });
    }
    openResult = await openResultPromise;
    assert(pageId, "Browser UI did not publish the current page id", openResult);
    assert(openResult.ok, "Browser UI open request failed", openResult);
    const panel = appPage.locator("#browser-render-panel");
    const box = await panel.boundingBox();
    assert(box && box.width > 0 && box.height > 0, "Browser render panel has no clickable box", box);
    if (BROWSER_OPEN_DISPLAY_MODE === "webrtc_remote_display") {
      const displaySession = openResult.body?.engine_page?.display_session || {};
      assert(
        displaySession.media_transport === "runtime_relay",
        "Browser WebRTC UI did not use Runtime relay media transport",
        displaySession,
      );
      assert(
        runtimeRelayIceContractOk(displaySession),
        "Browser WebRTC UI Runtime relay ICE contract is invalid",
        summarizeDisplaySession(displaySession),
      );
      const videoReady = await waitForBrowserRemoteVideo(appPage, {
        browserToken,
        pageId,
        displaySession,
        timeoutMs: BROWSER_REMOTE_VIDEO_TIMEOUT_MS,
      });
      let clickTarget = null;
      let clickX = Math.max(1, Math.min(box.width - 1, BROWSER_INPUT_CLICK_X));
      let clickY = Math.max(1, Math.min(box.height - 1, BROWSER_INPUT_CLICK_Y));
      if (BROWSER_UI_CLICK_HREF_RE) {
        const targetProof = await waitForBrowserHrefClickTarget(
          appPage,
          browserToken,
          pageId,
          BROWSER_UI_CLICK_HREF_RE,
        );
        clickTarget = targetProof.target;
        const targetPoint = {
          x: clickTarget.rect.x + clickTarget.rect.width / 2,
          y: clickTarget.rect.y + clickTarget.rect.height / 2,
        };
        const mappedPoint = await remoteVideoClickPositionForPagePoint(appPage, targetPoint);
        const videoBox = await appPage.locator("#browser-remote-display").boundingBox();
        assert(mappedPoint && videoBox, "Browser UI could not map page click target into video", {
          target: clickTarget,
          mapped: mappedPoint,
          video_box: videoBox,
        });
        clickX = Math.max(1, Math.min(videoBox.width - 1, Math.round(mappedPoint.x)));
        clickY = Math.max(1, Math.min(videoBox.height - 1, Math.round(mappedPoint.y)));
      }
      const beforeClickVideo = await browserRemoteVideoMetrics(appPage);
      let clickInput = null;
      const clickInputResponsePromise = BROWSER_UI_CLICK_EXPECT_URL_RE || BROWSER_UI_CLICK_HREF_RE
        ? appPage.waitForResponse(
            (response) => {
              const request = response.request();
              return request.method() === "POST" &&
                response.url().includes(`/api/apps/browser/pages/${encodeURIComponent(pageId)}/input`);
            },
            { timeout: 2500 },
          ).then(async (response) => ({
            ok: response.ok(),
            status: response.status(),
            body: await response.json().catch(() => ({})),
          })).catch((error) => ({
            ok: false,
            error: error.message || String(error),
          }))
        : null;
      await appPage.locator("#browser-remote-display").click({ position: { x: clickX, y: clickY } });
      clickInput = clickInputResponsePromise ? await clickInputResponsePromise : null;
      await delay(750);
      const audioProof = CHECK_BROWSER_AUDIO_STATS
        ? await waitForBrowserRemoteAudio(appPage, { browserToken, pageId })
        : null;
      const afterClickVideo = await browserRemoteVideoMetrics(appPage);
      const statusTextAfterClick = await appPage.locator("#browser-status").innerText().catch(() => "");
      assert(
        !/input channel is not open|failed closed|Browser remote display .*failed/i.test(statusTextAfterClick),
        "Browser WebRTC UI click left an input/display error",
        { status: statusTextAfterClick },
      );
      let clickNavigation = null;
      if (BROWSER_UI_CLICK_EXPECT_URL_RE) {
        const addressMatch = await waitForBrowserUiAddressMatch(
          appPage,
          BROWSER_UI_CLICK_EXPECT_URL_RE,
          BROWSER_UI_CLICK_NAV_TIMEOUT_MS,
        ).catch(async (error) => {
          const runtimeStatus = await checkBrowserPageStatus(
            appPage,
            browserToken,
            pageId,
          ).catch((statusError) => ({
            error: statusError.message || String(statusError),
            details: statusError.details || null,
          }));
          error.details = {
            ...(error.details || {}),
            click: { x: clickX, y: clickY, target: clickTarget },
            click_input: clickInput,
            runtime_status: runtimeStatus,
          };
          throw error;
        });
        const status = await checkBrowserPageStatus(appPage, browserToken, pageId);
        clickInput = normalizeRemoteDisplayClickInputEvidence(
          clickInput,
          status,
          addressMatch,
        );
        clickNavigation = {
          expected_url_re: BROWSER_UI_CLICK_EXPECT_URL_RE,
          ...addressMatch,
          input: clickInput,
          status,
        };
      }
      const navStarted = Date.now();
      const inputResponsePromise = appPage.waitForResponse(
        (response) => {
          const request = response.request();
          return request.method() === "POST" &&
            response.url().includes(`/api/apps/browser/pages/${encodeURIComponent(pageId)}/input`);
        },
        { timeout: 60_000 },
      );
      await appPage.locator("#browser-url").fill(BROWSER_UI_NAV_URL);
      await appPage.locator("#browser-url").press("Enter");
      const inputResponse = await inputResponsePromise;
      const inputResponseMs = Date.now() - navStarted;
      const inputBody = await inputResponse.json();
      assert(inputResponse.ok(), "Browser WebRTC UI navigation request failed", {
        status: inputResponse.status(),
        body: inputBody,
      });
      assert(inputBody?.schema === "elastos.browser.input-result/v1", "Browser WebRTC UI navigation returned wrong schema", inputBody);
      assert(inputBody.accepted === true, "Browser WebRTC UI navigation was not accepted", inputBody);
      assert(inputBody.direct_network === false, "Browser WebRTC UI navigation reported direct network", inputBody);
      const navStatus = await waitForBrowserPageStatus(
        appPage,
        browserToken,
        pageId,
        (status) => status.actual_url === BROWSER_UI_NAV_URL,
        `actual_url=${BROWSER_UI_NAV_URL}`,
        60_000,
      );
      const statusMatchMs = Date.now() - navStarted;
      await appPage.waitForFunction(
        (expected) => document.querySelector("#browser-url")?.value === expected,
        navStatus.actual_url,
        { timeout: 15_000 },
      );
      const addressMatchMs = Date.now() - navStarted;
      const addressValue = await appPage.locator("#browser-url").inputValue();
      const clicks = await appPage.evaluate(() => window.__elastosBrowserSmokeClicks || []);
      assert(clicks.length > 0, "Browser WebRTC UI click did not reach the render panel");
      return {
        page_id: pageId,
        url: appUrl.searchParams.get("url"),
        remote_exit_id: BROWSER_REMOTE_EXIT_ID || null,
        display_mode: BROWSER_OPEN_DISPLAY_MODE,
        video: {
          ready: videoReady,
          before_click: beforeClickVideo,
          after_click: afterClickVideo,
        },
        audio: audioProof,
        click: { x: clickX, y: clickY, target: clickTarget },
        click_navigation: clickNavigation,
        navigation: {
          requested_url: BROWSER_UI_NAV_URL,
          duration_ms: Date.now() - navStarted,
          input_response_ms: inputResponseMs,
          status_match_ms: statusMatchMs,
          address_match_ms: addressMatchMs,
          input: {
            accepted: inputBody.accepted,
            actual_url: inputBody.actual_url,
            title: inputBody.title,
          },
          status: navStatus,
          address_value: addressValue,
        },
        dom_clicks: clicks.slice(-3),
      };
    }
    await appPage.locator("#browser-remote-display").waitFor({ state: "visible", timeout: 180_000 });
    const remoteDisplay = appPage.locator("#browser-remote-display");
    const inputResponsePromise = appPage.waitForResponse(
      (response) => {
        const request = response.request();
        return request.method() === "POST" &&
          response.url().includes(`/api/apps/browser/pages/${encodeURIComponent(pageId)}/input`);
      },
      { timeout: BROWSER_INPUT_MAX_MS },
    );
    const clickX = Math.max(1, Math.min(box.width - 1, BROWSER_INPUT_CLICK_X));
    const clickY = Math.max(1, Math.min(box.height - 1, BROWSER_INPUT_CLICK_Y));
    const started = Date.now();
    await remoteDisplay.click({ position: { x: clickX, y: clickY } });
    const inputResponse = await inputResponsePromise;
    const durationMs = Date.now() - started;
    const inputBody = await inputResponse.json();
    assert(inputResponse.ok(), "Browser UI input request failed", {
      status: inputResponse.status(),
      body: inputBody,
    });
    assert(inputBody?.schema === "elastos.browser.input-result/v1", "Browser UI input returned wrong schema", inputBody);
    assert(inputBody.accepted === true, "Browser UI input was not accepted", inputBody);
    assert(inputBody.direct_network === false, "Browser UI input reported direct network", inputBody);
    assert(
      durationMs <= BROWSER_INPUT_MAX_MS,
      "Browser UI input exceeded latency budget",
      { duration_ms: durationMs, max_ms: BROWSER_INPUT_MAX_MS, input: inputBody },
    );
    const clicks = await appPage.evaluate(() => window.__elastosBrowserSmokeClicks || []);
    assert(clicks.length > 0, "Browser UI click did not reach the render panel");
    return {
      page_id: pageId,
      url: appUrl.searchParams.get("url"),
      remote_exit_id: BROWSER_REMOTE_EXIT_ID || null,
      display_mode: BROWSER_OPEN_DISPLAY_MODE,
      click: { x: clickX, y: clickY },
      input: {
        accepted: inputBody.accepted,
        duration_ms: durationMs,
        actual_url: inputBody.actual_url,
        title: inputBody.title,
      },
      dom_clicks: clicks.slice(-3),
    };
  } finally {
    const closePageId = pageId || openedPageId;
    if (closePageId) {
      closed = await browserApi(
        appPage,
        browserToken,
        `/api/apps/browser/pages/${encodeURIComponent(closePageId)}/close`,
        { method: "POST", body: {} },
      ).catch((error) => ({ ok: false, error: error.message }));
      assertBrowserCloseOkOrInactive(
        closed,
        `Browser UI smoke could not close Runtime Browser page ${closePageId}`,
      );
    }
    await appPage.close().catch(() => {});
  }
}

async function holdBrowserUiForSetup(context, browserToken, route) {
  const appPage = await context.newPage();
  const webrtcSignals = attachWebrtcSignalCapture(appPage);
  let pageId = "";
  let openedPageId = "";
  let openResult = null;
  let closed = null;
  try {
    const appUrl = new URL(route, HOME_URL);
    appUrl.searchParams.set("url", BROWSER_OPEN_URLS[0] || "https://example.com/");
    appUrl.searchParams.set("display", BROWSER_OPEN_DISPLAY_MODE);
    if (CHECK_BROWSER_AUDIO_STATS) {
      appUrl.searchParams.set("metrics", "1");
    }
    if (BROWSER_REMOTE_EXIT_ID) {
      appUrl.searchParams.set("remote_exit_id", BROWSER_REMOTE_EXIT_ID);
    }
    const openResultPromise = appPage.waitForResponse(
      (response) => {
        const request = response.request();
        return request.method() === "POST" && response.url().endsWith("/api/apps/browser/open");
      },
      { timeout: BROWSER_UI_PAGE_ID_TIMEOUT_MS },
    ).then(async (response) => {
      const body = await response.json().catch(() => ({}));
      const result = await waitForBrowserOpenResult(appPage, browserToken, {
        ok: response.ok(),
        status: response.status(),
        body,
      }, BROWSER_UI_PAGE_ID_TIMEOUT_MS);
      openedPageId = String(result.body?.engine_page?.page_id || "");
      return result;
    }).catch((error) => ({
      ok: false,
      error: error.message || String(error),
    }));
    await appPage.goto(appUrl.toString(), { waitUntil: "domcontentloaded" });
    try {
      pageId = await appPage.waitForFunction(
        () => window.__elastosBrowserCurrentPageId || "",
        null,
        { timeout: BROWSER_UI_PAGE_ID_TIMEOUT_MS },
      ).then((handle) => handle.jsonValue());
    } catch (error) {
      openResult = await openResultPromise;
      const domState = await appPage.evaluate(() => ({
        href: window.location.href,
        title: document.title,
        body_loading: document.body?.dataset?.loading || "",
        current_page_id: window.__elastosBrowserCurrentPageId || "",
        status: document.querySelector("#browser-status")?.textContent?.trim() || "",
        address: document.querySelector("#browser-url")?.value || "",
      })).catch(() => null);
      throw Object.assign(new Error("Browser setup UI did not publish the current page id"), {
        details: { error: error.message || String(error), open_result: openResult, dom: domState },
      });
    }
    openResult = await openResultPromise;
    assert(pageId, "Browser setup UI did not publish the current page id", openResult);
    assert(openResult.ok, "Browser setup UI open request failed", openResult);
    const panel = appPage.locator("#browser-render-panel");
    const box = await panel.boundingBox();
    assert(box && box.width > 0 && box.height > 0, "Browser setup render panel has no visible box", box);
    const displaySession = openResult.body?.engine_page?.display_session || {};
    let videoReady = null;
    if (BROWSER_OPEN_DISPLAY_MODE === "webrtc_remote_display") {
      assert(
        displaySession.media_transport === "runtime_relay",
        "Browser setup UI did not use Runtime relay media transport",
        displaySession,
      );
      assert(
        runtimeRelayIceContractOk(displaySession),
        "Browser setup UI Runtime relay ICE contract is invalid",
        summarizeDisplaySession(displaySession),
      );
      try {
        videoReady = await waitForBrowserRemoteVideo(appPage, {
          browserToken,
          pageId,
          displaySession,
          timeoutMs: BROWSER_REMOTE_VIDEO_TIMEOUT_MS,
        });
      } catch (error) {
        error.details = {
          ...(error.details || {}),
          webrtc_signals: webrtcSignals,
        };
        throw error;
      }
    }
    const holdStartedAt = Date.now();
    while (Date.now() - holdStartedAt < BROWSER_OPEN_HOLD_MS) {
      await delay(Math.min(5000, Math.max(250, BROWSER_OPEN_HOLD_MS - (Date.now() - holdStartedAt))));
      const response = await browserApi(
        appPage,
        browserToken,
        `/api/apps/browser/pages/${encodeURIComponent(pageId)}/heartbeat`,
        { method: "POST" },
      );
      assert(response.ok, "Browser setup heartbeat failed", response);
    }
    const status = await checkBrowserPageStatus(appPage, browserToken, pageId);
    const diagnostics = CHECK_BROWSER_DIAGNOSTICS
      ? await checkBrowserPageDiagnostics(appPage, browserToken, pageId)
      : null;
    return {
      page_id: pageId,
      url: appUrl.searchParams.get("url"),
      remote_exit_id: BROWSER_REMOTE_EXIT_ID || null,
      display_mode: BROWSER_OPEN_DISPLAY_MODE,
      hold_ms: BROWSER_OPEN_HOLD_MS,
      video_ready: videoReady,
      status,
      diagnostics,
      webrtc_signals: webrtcSignals,
    };
  } finally {
    const closePageId = pageId || openedPageId;
    if (closePageId) {
      closed = await browserApi(
        appPage,
        browserToken,
        `/api/apps/browser/pages/${encodeURIComponent(closePageId)}/close`,
        { method: "POST", body: {} },
      ).catch((error) => ({ ok: false, error: error.message }));
      assertBrowserCloseOkOrInactive(
        closed,
        `Browser setup UI could not close Runtime Browser page ${closePageId}`,
      );
    }
    await appPage.close().catch(() => {});
  }
}

async function checkBrowserEmbeddedUiInput(page) {
  const webrtcSignals = [];
  const captureWebrtcResponse = async (response) => {
    const request = response.request();
    if (request.method() !== "POST" || !response.url().includes("/webrtc")) {
      return;
    }
    let requestBody = null;
    try {
      requestBody = JSON.parse(request.postData() || "null");
    } catch {
      requestBody = request.postData() || "";
    }
    let responseBody = null;
    try {
      responseBody = await response.json();
    } catch {
      responseBody = await response.text().catch(() => "");
    }
    webrtcSignals.push({
      url: response.url(),
      status: response.status(),
      request: requestBody,
      response: responseBody,
    });
  };
  page.on("response", captureWebrtcResponse);
  await page.goto(HOME_URL, { waitUntil: "domcontentloaded" });
  await waitForSignedHome(page);
  const homeGuiFrame = await waitForCapsuleFrame(page, "home-gui");
  const browserShortcut = homeGuiFrame.locator(
    '#desktop-shortcuts .desktop-shortcut[data-target="browser"]',
  );
  await browserShortcut.waitFor({ state: "visible", timeout: 30_000 });
  await browserShortcut.dblclick();

  const windowLocator = homeGuiFrame.locator('.window[data-target="browser"]:not(.hidden)').first();
  await windowLocator.waitFor({ state: "visible", timeout: 30_000 });
  await homeGuiFrame.waitForFunction(() => {
    const node = [...document.querySelectorAll('.window[data-target="browser"]')]
      .find((candidate) => !candidate.classList.contains("hidden"));
    return node?.classList.contains("window-active") &&
      getComputedStyle(node.querySelector(".window-frame")).pointerEvents === "auto";
  }, null, { timeout: 10_000 });
  const frameHandle = await windowLocator.locator("iframe.window-frame").elementHandle();
  assert(frameHandle, "Home Browser window did not contain an iframe");
  const route = await frameHandle.getAttribute("src") || "";
  const browserToken = assertIsolatedLaunchRoute(route, "browser");
  const appFrame = await frameHandle.contentFrame();
  assert(appFrame, "Embedded Browser iframe did not expose a content frame");

  let pageId = "";
  try {
    await appFrame.evaluate(() => {
      window.__elastosBrowserSmokeClicks = [];
      const panel = document.querySelector("#browser-render-panel");
      panel?.addEventListener("click", (event) => {
        window.__elastosBrowserSmokeClicks.push({
          target: event.target?.id || event.target?.tagName || "",
          currentTarget: event.currentTarget?.id || "",
          clientX: event.clientX,
          clientY: event.clientY,
        });
      }, { capture: true });
    });
    pageId = await appFrame.waitForFunction(
      () => window.__elastosBrowserCurrentPageId || "",
      null,
      { timeout: BROWSER_UI_PAGE_ID_TIMEOUT_MS },
    ).then((handle) => handle.jsonValue()).catch(async (error) => {
      throw Object.assign(new Error("Embedded Browser UI did not publish the current page id before timeout"), {
        details: await embeddedBrowserDebugState(windowLocator, appFrame),
        cause: error,
      });
    });
    assert(pageId, "Embedded Browser UI did not publish the current page id");
    const panelBox = await appFrame.locator("#browser-render-panel").boundingBox();
    assert(panelBox && panelBox.width > 0 && panelBox.height > 0, "Embedded Browser render panel has no clickable box", panelBox);
    const clickX = Math.max(1, Math.min(panelBox.width - 1, BROWSER_INPUT_CLICK_X));
    const clickY = Math.max(1, Math.min(panelBox.height - 1, BROWSER_INPUT_CLICK_Y));
    const initialGeometry = await embeddedBrowserGeometry(windowLocator, appFrame);
    assertEmbeddedBrowserAspect(initialGeometry);
    if (BROWSER_OPEN_DISPLAY_MODE === "webrtc_remote_display") {
      const initialStatus = await checkBrowserPageStatus(page, browserToken, pageId);
      const displaySession = initialStatus.display_session || {};
      assert(
        displaySession.media_transport === "runtime_relay",
        "Embedded Browser WebRTC UI did not use Runtime relay media transport",
        displaySession,
      );
      assert(
        runtimeRelayIceContractOk(displaySession),
        "Embedded Browser WebRTC UI Runtime relay ICE contract is invalid",
        displaySession,
      );
      const videoReady = await waitForBrowserRemoteVideo(appFrame, {
        browserToken,
        pageId,
        displaySession,
        timeoutMs: BROWSER_REMOTE_VIDEO_TIMEOUT_MS,
        webrtcSignals,
      }).catch((error) => {
        error.details = {
          ...(error.details || {}),
          webrtc_signals: webrtcSignals,
        };
        throw error;
      });
      const beforeClickVideo = await browserRemoteVideoMetrics(appFrame);
      const videoBox = await appFrame.locator("#browser-remote-display").boundingBox();
      assert(videoBox && videoBox.width > 0 && videoBox.height > 0, "Embedded Browser WebRTC video has no clickable box", {
        video_box: videoBox,
        geometry: initialGeometry,
      });
      let clickTarget = null;
      let videoClickX = Math.max(1, Math.min(videoBox.width - 1, BROWSER_INPUT_CLICK_X));
      let videoClickY = Math.max(1, Math.min(videoBox.height - 1, BROWSER_INPUT_CLICK_Y));
      if (BROWSER_UI_CLICK_HREF_RE) {
        const targetProof = await waitForBrowserHrefClickTarget(
          page,
          browserToken,
          pageId,
          BROWSER_UI_CLICK_HREF_RE,
        );
        clickTarget = targetProof.target;
        const targetPoint = {
          x: clickTarget.rect.x + clickTarget.rect.width / 2,
          y: clickTarget.rect.y + clickTarget.rect.height / 2,
        };
        const mappedPoint = await remoteVideoClickPositionForPagePoint(appFrame, targetPoint);
        assert(mappedPoint, "Embedded Browser UI could not map page click target into video", {
          target: clickTarget,
          mapped: mappedPoint,
          video_box: videoBox,
        });
        videoClickX = Math.max(1, Math.min(videoBox.width - 1, Math.round(mappedPoint.x)));
        videoClickY = Math.max(1, Math.min(videoBox.height - 1, Math.round(mappedPoint.y)));
      }
      const clickInputResponsePromise = BROWSER_UI_CLICK_EXPECT_URL_RE || BROWSER_UI_CLICK_HREF_RE
        ? page.waitForResponse(
            (response) => {
              const request = response.request();
              return request.method() === "POST" &&
                response.url().includes(`/api/apps/browser/pages/${encodeURIComponent(pageId)}/input`);
            },
            { timeout: 2500 },
          ).then(async (response) => ({
            ok: response.ok(),
            status: response.status(),
            body: await response.json().catch(() => ({})),
          })).catch((error) => ({
            ok: false,
            error: error.message || String(error),
          }))
        : null;
      await appFrame.locator("#browser-remote-display").click({ position: { x: videoClickX, y: videoClickY } });
      let clickInput = clickInputResponsePromise ? await clickInputResponsePromise : null;
      await delay(750);
      const audioProof = CHECK_BROWSER_AUDIO_STATS
        ? await waitForBrowserRemoteAudio(appFrame, { browserToken, pageId })
        : null;
      const afterClickVideo = await browserRemoteVideoMetrics(appFrame);
      const statusTextAfterClick = await appFrame.locator("#browser-status").innerText().catch(() => "");
      assert(
        !/input channel is not open|failed closed|Browser remote display .*failed/i.test(statusTextAfterClick),
        "Embedded Browser WebRTC click left an input/display error",
        { status: statusTextAfterClick },
      );
      let clickNavigation = null;
      if (BROWSER_UI_CLICK_EXPECT_URL_RE) {
        const addressMatch = await waitForBrowserUiAddressMatch(
          appFrame,
          BROWSER_UI_CLICK_EXPECT_URL_RE,
          BROWSER_UI_CLICK_NAV_TIMEOUT_MS,
        ).catch(async (error) => {
          const runtimeStatus = await checkBrowserPageStatus(
            page,
            browserToken,
            pageId,
          ).catch((statusError) => ({
            error: statusError.message || String(statusError),
            details: statusError.details || null,
          }));
          error.details = {
            ...(error.details || {}),
            click: { x: Math.round(videoClickX), y: Math.round(videoClickY), target: clickTarget },
            click_input: clickInput,
            runtime_status: runtimeStatus,
          };
          throw error;
        });
        const status = await checkBrowserPageStatus(page, browserToken, pageId);
        clickInput = normalizeRemoteDisplayClickInputEvidence(
          clickInput,
          status,
          addressMatch,
        );
        clickNavigation = {
          expected_url_re: BROWSER_UI_CLICK_EXPECT_URL_RE,
          ...addressMatch,
          input: clickInput,
          status,
        };
      }
      const navStarted = Date.now();
      const inputResponsePromise = page.waitForResponse(
        (response) => {
          const request = response.request();
          return request.method() === "POST" &&
            response.url().includes(`/api/apps/browser/pages/${encodeURIComponent(pageId)}/input`);
        },
        { timeout: 60_000 },
      );
      await appFrame.locator("#browser-url").fill(BROWSER_UI_NAV_URL);
      await appFrame.locator("#browser-url").press("Enter");
      const inputResponse = await inputResponsePromise;
      const inputResponseMs = Date.now() - navStarted;
      const inputBody = await inputResponse.json();
      assert(inputResponse.ok(), "Embedded Browser WebRTC navigation request failed", {
        status: inputResponse.status(),
        body: inputBody,
      });
      assert(inputBody?.schema === "elastos.browser.input-result/v1", "Embedded Browser WebRTC navigation returned wrong schema", inputBody);
      assert(inputBody.accepted === true, "Embedded Browser WebRTC navigation was not accepted", inputBody);
      assert(inputBody.direct_network === false, "Embedded Browser WebRTC navigation reported direct network", inputBody);
      const navStatus = await waitForBrowserPageStatus(
        page,
        browserToken,
        pageId,
        (status) => status.actual_url === BROWSER_UI_NAV_URL,
        `actual_url=${BROWSER_UI_NAV_URL}`,
        60_000,
      );
      const statusMatchMs = Date.now() - navStarted;
      await appFrame.waitForFunction(
        (expected) => document.querySelector("#browser-url")?.value === expected,
        navStatus.actual_url,
        { timeout: 15_000 },
      );
      const addressMatchMs = Date.now() - navStarted;
      const addressValue = await appFrame.locator("#browser-url").inputValue();
      const clicks = await appFrame.evaluate(() => window.__elastosBrowserSmokeClicks || []);
      assert(clicks.length > 0, "Embedded Browser WebRTC click did not reach the render panel");
      return {
        page_id: pageId,
        route_prefix: route.split("?")[0],
        remote_exit_id: BROWSER_REMOTE_EXIT_ID || null,
        display_mode: BROWSER_OPEN_DISPLAY_MODE,
        display_session: summarizeDisplaySession(displaySession),
        geometry: {
          initial: initialGeometry,
          after_navigation: await embeddedBrowserGeometry(windowLocator, appFrame),
        },
        video: {
          ready: videoReady,
          before_click: beforeClickVideo,
          after_click: afterClickVideo,
        },
        audio: audioProof,
        click: { x: Math.round(videoClickX), y: Math.round(videoClickY) },
        click_navigation: clickNavigation,
        navigation: {
          requested_url: BROWSER_UI_NAV_URL,
          duration_ms: Date.now() - navStarted,
          input_response_ms: inputResponseMs,
          status_match_ms: statusMatchMs,
          address_match_ms: addressMatchMs,
          input: {
            accepted: inputBody.accepted,
            actual_url: inputBody.actual_url,
            title: inputBody.title,
          },
          status: navStatus,
          address_value: addressValue,
        },
        window_active: await windowLocator.evaluate((node) => node.classList.contains("window-active")),
        frame_pointer_events: await windowLocator.evaluate((node) =>
          getComputedStyle(node.querySelector(".window-frame")).pointerEvents,
        ),
        dom_clicks: clicks.slice(-3),
      };
    }
    await appFrame.locator("#browser-remote-display").waitFor({ state: "visible", timeout: 180_000 });
    if (CHECK_BROWSER_EMBEDDED_RECOVERY) {
      const oldPageId = pageId;
      const closed = await browserApi(
        page,
        browserToken,
        `/api/apps/browser/pages/${encodeURIComponent(oldPageId)}/close`,
        { method: "POST", body: {} },
      );
      assert(closed.ok, `Embedded Browser recovery smoke could not close Runtime Browser page ${oldPageId}`, closed);
      pageId = "";
      const matchesBrowserOpen = (requestOrResponse) => {
        const request = requestOrResponse.request?.() || requestOrResponse;
        return request.method() === "POST" && request.url().endsWith("/api/apps/browser/open");
      };
      const openRequestPromise = page.waitForRequest(matchesBrowserOpen, {
        timeout: 5_000,
      });
      const openResponsePromise = page.waitForResponse(
        (response) => {
          return matchesBrowserOpen(response);
        },
        { timeout: 160_000 },
      );
      const started = Date.now();
      await appFrame.locator("#browser-render-panel").click({ position: { x: clickX, y: clickY } });
      await openRequestPromise;
      const requestDurationMs = Date.now() - started;
      const openResponse = await openResponsePromise;
      const durationMs = Date.now() - started;
      assert(openResponse.ok(), "Embedded Browser recovery open request failed", {
        status: openResponse.status(),
        body: await openResponse.text(),
      });
      pageId = await appFrame.waitForFunction(
        (previous) => {
          const next = window.__elastosBrowserCurrentPageId || "";
          return next && next !== previous ? next : "";
        },
        oldPageId,
        { timeout: 20_000 },
      ).then((handle) => handle.jsonValue());
      assert(pageId, "Embedded Browser recovery did not publish a replacement page id");
      await appFrame.locator("#browser-remote-display").waitFor({ state: "visible", timeout: 30_000 });
      const clicks = await appFrame.evaluate(() => window.__elastosBrowserSmokeClicks || []);
      return {
        page_id: pageId,
        route_prefix: route.split("?")[0],
        remote_exit_id: BROWSER_REMOTE_EXIT_ID || null,
        display_mode: BROWSER_OPEN_DISPLAY_MODE,
        geometry: {
          initial: initialGeometry,
          after_recovery: await embeddedBrowserGeometry(windowLocator, appFrame),
        },
        window_active: await windowLocator.evaluate((node) => node.classList.contains("window-active")),
        frame_pointer_events: await windowLocator.evaluate((node) =>
          getComputedStyle(node.querySelector(".window-frame")).pointerEvents,
        ),
        click: { x: clickX, y: clickY },
        recovery: {
          old_page_id: oldPageId,
          new_page_id: pageId,
          request_duration_ms: requestDurationMs,
          duration_ms: durationMs,
        },
        dom_clicks: clicks.slice(-3),
      };
    }
    const inputResponsePromise = page.waitForResponse(
      (response) => {
        const request = response.request();
        return request.method() === "POST" &&
          response.url().includes(`/api/apps/browser/pages/${encodeURIComponent(pageId)}/input`);
      },
      { timeout: BROWSER_INPUT_MAX_MS },
    );
    const started = Date.now();
    await appFrame.locator("#browser-render-panel").click({ position: { x: clickX, y: clickY } });
    const inputResponse = await inputResponsePromise;
    const durationMs = Date.now() - started;
    const inputBody = await inputResponse.json();
    assert(inputResponse.ok(), "Embedded Browser UI input request failed", {
      status: inputResponse.status(),
      body: inputBody,
    });
    assert(inputBody?.schema === "elastos.browser.input-result/v1", "Embedded Browser UI input returned wrong schema", inputBody);
    assert(inputBody.accepted === true, "Embedded Browser UI input was not accepted", inputBody);
    assert(inputBody.direct_network === false, "Embedded Browser UI input reported direct network", inputBody);
    assert(
      durationMs <= BROWSER_INPUT_MAX_MS,
      "Embedded Browser UI input exceeded latency budget",
      { duration_ms: durationMs, max_ms: BROWSER_INPUT_MAX_MS, input: inputBody },
    );
    const clicks = await appFrame.evaluate(() => window.__elastosBrowserSmokeClicks || []);
    assert(clicks.length > 0, "Embedded Browser UI click did not reach the render panel");
    return {
      page_id: pageId,
      route_prefix: route.split("?")[0],
      remote_exit_id: BROWSER_REMOTE_EXIT_ID || null,
      display_mode: BROWSER_OPEN_DISPLAY_MODE,
      geometry: {
        initial: initialGeometry,
        after_input: await embeddedBrowserGeometry(windowLocator, appFrame),
      },
      window_active: await windowLocator.evaluate((node) => node.classList.contains("window-active")),
      frame_pointer_events: await windowLocator.evaluate((node) =>
        getComputedStyle(node.querySelector(".window-frame")).pointerEvents,
      ),
      click: { x: clickX, y: clickY },
      input: {
        accepted: inputBody.accepted,
        duration_ms: durationMs,
        actual_url: inputBody.actual_url,
        title: inputBody.title,
      },
      dom_clicks: clicks.slice(-3),
    };
  } finally {
    page.off("response", captureWebrtcResponse);
    if (pageId && browserToken) {
      const closed = await browserApi(
        page,
        browserToken,
        `/api/apps/browser/pages/${encodeURIComponent(pageId)}/close`,
        { method: "POST", body: {} },
      ).catch((error) => ({ ok: false, error: error.message }));
      const inactiveCleanup =
        closed.status === 404 &&
        /browser session is not active/i.test(String(closed.body?.raw || ""));
      assert(
        closed.ok || inactiveCleanup,
        `Embedded Browser UI smoke could not close Runtime Browser page ${pageId}`,
        closed,
      );
    }
  }
}

function settleTokenWithin(promise, timeoutMs) {
  return Promise.race([
    promise.catch(() => null),
    delay(timeoutMs).then(() => null),
  ]);
}

async function statusFromServer(page) {
  return page.evaluate(async () => {
    const response = await fetch("/api/auth/passkey/status");
    const text = await response.text();
    let body = {};
    try {
      body = text ? JSON.parse(text) : {};
    } catch {
      body = { raw: text };
    }
    return {
      ok: response.ok,
      status: response.status,
      body,
    };
  });
}

async function createPasskeyFromCurrentUnlock(page, mode) {
  // First boot opens on the welcome beat; "Get started" advances to the
  // create-passkey form. Returning flows (guest create) land on the form
  // directly, so the click is conditional.
  const primary = page.locator("#home-unlock-primary");
  if ((await primary.textContent())?.trim() === "Get started") {
    await primary.click();
  }
  const name = page.locator("#home-unlock-name");
  await name.waitFor({ state: "visible", timeout: 10_000 });
  await name.fill(TEST_NAME);
  const tokenPromise = captureNextPasskeyToken(page);
  await page.locator("#home-unlock-primary").click();
  await waitForSignedHome(page);
  return { mode, homeToken: await tokenPromise };
}

async function ensureSignedWithVirtualPasskey(page) {
  await waitForHomeReady(page);
  let state = await homeState(page);
  if (state.authority === "signed") {
    await signOut(page);
    try {
      const homeToken = await signBackIn(page);
      return { created: false, mode: "existing-session", homeToken };
    } catch (error) {
      state = await homeState(page);
      if (!state.unlockVisible) {
        throw error;
      }
    }
  }

  const status = await statusFromServer(page);
  assert(status.ok, "passkey status endpoint failed", status);
  const registered = status.body.registered === true;
  const guestRegistrationEnabled = status.body.guest_registration_enabled === true;

  if (!registered) {
    const created = await createPasskeyFromCurrentUnlock(page, "admin");
    return { created: true, ...created };
  }

  if (hasVirtualAuthenticatorCredentialStore()) {
    try {
      const homeToken = await signBackIn(page);
      return { created: false, mode: "existing-passkey", homeToken };
    } catch {
      await page.goto(HOME_URL, { waitUntil: "domcontentloaded" });
      await waitForHomeReady(page);
    }
  }

  if (!guestRegistrationEnabled) {
    const skip = new Error("SKIP virtual passkey smoke: existing Home has guest registration disabled");
    skip.skip = true;
    skip.details = { registered, guestRegistrationEnabled, state };
    throw skip;
  }

  const secondary = page.locator("#home-unlock-secondary");
  await secondary.waitFor({ state: "visible", timeout: 15_000 });
  await secondary.click();
  state = await homeState(page);
  assert(
    state.unlockTitle === "Create guest account" && state.unlockNameVisible,
    "Home did not enter guest passkey creation mode",
    state,
  );
  const created = await createPasskeyFromCurrentUnlock(page, "guest");
  return { created: true, ...created };
}

async function currentPasskey(page, homeToken) {
  assert(homeToken, "currentPasskey requires a passkey-issued Home token");
  return page.evaluate(async (token) => {
    const response = await fetch("/api/auth/passkeys", {
      headers: { "x-elastos-home-token": token },
    });
    const text = await response.text();
    let body = {};
    try {
      body = text ? JSON.parse(text) : {};
    } catch {
      body = { raw: text };
    }
    if (!response.ok) {
      throw new Error(`GET /api/auth/passkeys -> ${response.status} ${text}`);
    }
    return (body.passkeys || []).find((passkey) => passkey.current) || null;
  }, homeToken);
}

async function refreshCurrentHomeToken(page) {
  return page.evaluate(async () => {
    const response = await fetch("/api/auth/sessions/refresh", {
      method: "POST",
      credentials: "same-origin",
    });
    const text = await response.text();
    let body = {};
    try {
      body = text ? JSON.parse(text) : {};
    } catch {
      body = { raw: text };
    }
    return {
      ok: response.ok,
      status: response.status,
      body,
      homeToken: typeof body.home_token === "string" ? body.home_token : "",
    };
  });
}

async function signOut(page, homeToken = "") {
  const refreshed = homeToken ? null : await refreshCurrentHomeToken(page);
  const activeHomeToken = homeToken || refreshed?.homeToken || "";
  const signedOut = await page.evaluate(async (token) => {
    const headers = { "content-type": "application/json" };
    if (token) {
      headers["x-elastos-home-token"] = token;
    }
    const response = await fetch("/api/auth/sessions/sign-out", {
      method: "POST",
      credentials: "same-origin",
      headers,
    });
    const text = await response.text();
    let body = {};
    try {
      body = text ? JSON.parse(text) : {};
    } catch {
      body = { raw: text };
    }
    return {
      ok: response.ok,
      status: response.status,
      body,
    };
  }, activeHomeToken);
  assert(signedOut.ok, "Home sign-out request failed", {
    refreshed,
    signed_out: signedOut,
    token_present: activeHomeToken.length > 0,
  });
  await page.goto(HOME_URL, { waitUntil: "domcontentloaded" });
}

async function signBackIn(page) {
  const tokenPromise = captureNextPasskeyToken(page, 20_000).catch(() => null);
  await page.goto(HOME_URL, { waitUntil: "domcontentloaded" });
  await waitForHomeReady(page);
  let signed = false;
  try {
    await waitForSignedHome(page, 8_000);
    signed = true;
  } catch {
    signed = false;
  }
  if (signed) {
    const token = await settleTokenWithin(tokenPromise, 1_000);
    assert(token, "Home remained signed after sign-out without completing passkey authentication", await homeState(page));
    return token;
  }

  const state = await homeState(page);
  assert(state.unlockVisible, "Home did not show the unlock prompt after sign-out", state);
  assert(
    state.shell === "resolving" &&
      state.gui === "dormant" &&
      state.activeShellRootHidden &&
      !state.activeShellFrameSrc &&
      !state.hostGuiDomPresent,
    "A Home shell remained mounted behind the passkey prompt",
    state,
  );
  const clickTokenPromise = captureNextPasskeyToken(page).catch(() => null);
  await page.locator("#home-unlock-primary").click();
  await waitForSignedHome(page);
  const token = await settleTokenWithin(clickTokenPromise, 1_000)
    || await settleTokenWithin(tokenPromise, 1_000);
  assert(token, "manual virtual passkey sign-in completed without a captured Home token", await homeState(page));
  return token;
}

async function checkHomePublicCopy(page) {
  await waitForSignedHome(page);
  const homeGuiFrame = await waitForCapsuleFrame(page, "home-gui");
  const state = await homeGuiFrame.evaluate(() => {
    const visible = (node) => {
      const style = window.getComputedStyle(node);
      return style.display !== "none"
        && style.visibility !== "hidden"
        && node.getClientRects().length > 0;
    };
    const text = (document.body.innerText || "").replace(/\s+/g, " ").trim();
    const headings = [...document.querySelectorAll("h1,h2,h3,[role=heading]")]
      .filter(visible)
      .map((node) => (node.innerText || node.textContent || "").replace(/\s+/g, " ").trim())
      .filter(Boolean);
    const counts = new Map();
    for (const heading of headings) counts.set(heading, (counts.get(heading) || 0) + 1);
    return {
      text,
      duplicate_headings: [...counts.entries()].filter(([, count]) => count > 1),
      horizontal_overflow: document.documentElement.scrollWidth > document.documentElement.clientWidth + 2,
    };
  });
  const internalCopy = state.text.match(/\b(runtime mirror|permissioned runtime|projection|schema|derived facts?|runtime facts?|capsules?|providers?|capabilit(?:y|ies)|affordances?|authority boundary|provider boundary|gate preview|runtime-owned|host-loaded|structured home intents?|provider operation|launch token|hostcall|objects?)\b/i);
  assert(!internalCopy, "Home GUI exposed implementation copy", { match: internalCopy?.[0], state });
  assert(state.duplicate_headings.length === 0, "Home GUI rendered duplicate visible headings", state);
  assert(!state.horizontal_overflow, "Home GUI rendered horizontal overflow", state);
  return {
    text_length: state.text.length,
    duplicate_headings: state.duplicate_headings,
    horizontal_overflow: state.horizontal_overflow,
  };
}

async function launchSystem(page, homeToken) {
  assert(homeToken, "launchSystem requires a passkey-issued Home token");
  const route = await page.evaluate(async (token) => {
    const response = await fetch("/api/apps/home/launch", {
      method: "POST",
      headers: { "content-type": "application/json", "x-elastos-home-token": token },
      body: JSON.stringify({ target: "system" }),
    });
    const text = await response.text();
    let body = {};
    try {
      body = text ? JSON.parse(text) : {};
    } catch {
      body = { raw: text };
    }
    if (!response.ok) {
      throw new Error(`POST /api/apps/home/launch system -> ${response.status} ${text}`);
    }
    return body.route || "";
  }, homeToken);
  assertIsolatedLaunchRoute(route, "system");
  await page.goto(new URL(route, HOME_URL).toString(), { waitUntil: "domcontentloaded" });
  await page.locator(".settings-container").waitFor({ state: "visible", timeout: 20_000 });
  const system = await page.evaluate(() => ({
    title: document.title,
    tabs: [...document.querySelectorAll(".settings-sidebar-text")].map((node) => node.textContent?.trim() || ""),
    sections: [...document.querySelectorAll(".pc2-section-title")].map((node) => node.textContent?.trim() || ""),
    fields: [...document.querySelectorAll(".system-fields dt")].map((node) => node.textContent?.trim() || ""),
    walletControlsRemoved: !document.querySelector("#wallet-create")
      && !document.querySelector("#wallet-approvals")
      && !document.querySelector("#wallet-accounts"),
    errorText: document.querySelector(".system-error:not([hidden])")?.textContent?.trim() || "",
  }));
  assert(system.title === "System · ElastOS", "System title mismatch after signed launch", system);
  assert(
    system.tabs.includes("About") &&
      system.sections.includes("Appearance") &&
      system.sections.includes("This Device"),
    "System sections did not render",
    system,
  );
  assert(system.fields.includes("Accounts") && system.fields.includes("Recovery"), "System signed account fields did not render", system);
  assert(!system.fields.includes("Wallet"), "System should not duplicate Wallet controls", system);
  assert(!system.fields.includes("Documents"), "System should not duplicate Documents controls", system);
  assert(system.walletControlsRemoved, "System should not include wallet account or approval controls", system);
  assert(!system.errorText, "System rendered an access error after signed launch", system);
  return system;
}

async function checkShellSwitchJourney(page, homeToken) {
  assert(homeToken, "checkShellSwitchJourney requires a passkey-issued Home token");
  let switchedToCli = false;
  const shellConsole = [];
  const shellPageErrors = [];
  const shellRequestFailures = [];
  const shellResponses = [];
  const captureConsole = (message) => {
    shellConsole.push({ type: message.type(), text: redactSensitiveString(message.text()) });
    if (shellConsole.length > 50) {
      shellConsole.shift();
    }
  };
  const capturePageError = (error) => {
    shellPageErrors.push(redactSensitiveString(error?.stack || error?.message || String(error)));
  };
  const captureRequestFailure = (request) => {
    shellRequestFailures.push({
      error: request.failure()?.errorText || "request failed",
      method: request.method(),
      url: redactSensitiveString(request.url()),
    });
  };
  const captureResponse = (response) => {
    const url = response.url();
    if (
      url.includes("/api/apps/home/active-shell") ||
      url.includes("/api/apps/home-cli/terminal/")
    ) {
      shellResponses.push({
        method: response.request().method(),
        status: response.status(),
        url: redactSensitiveString(url),
      });
    }
  };
  page.on("console", captureConsole);
  page.on("pageerror", capturePageError);
  page.on("requestfailed", captureRequestFailure);
  page.on("response", captureResponse);
  try {
    markStage("shell-switch:launch-system");
    if (!page.url().includes("/apps/system/")) {
      await launchSystem(page, homeToken);
    }

    markStage("shell-switch:open-system-shell");
    await page.locator('button.settings-sidebar-item[data-settings="shell"]').click();
    await page.locator("#active-shell-options").waitFor({ state: "visible", timeout: 15_000 });
    await page.waitForFunction(() => {
      const names = [...document.querySelectorAll("#active-shell-options [data-shell-name]")]
        .map((button) => button.dataset.shellName);
      return names.includes("home-gui") && names.includes("home-cli");
    }, null, { timeout: 15_000 });
    const shellOptions = await page.evaluate(() => (
      [...document.querySelectorAll("#active-shell-options [data-shell-name]")].map((button) => ({
        value: button.dataset.shellName,
        label: button.textContent?.trim() || "",
      }))
    ));

    const switchToCli = page.waitForResponse((response) => (
      response.request().method() === "POST" &&
      response.url().endsWith("/api/apps/home/active-shell")
    ), { timeout: 15_000 });
    markStage("shell-switch:system-post-home-cli");
    await page.locator('#active-shell-options [data-shell-name="home-cli"]').click();
    const switchResponse = await switchToCli;
    assert(switchResponse.ok(), "System shell picker failed to switch to Home CLI", {
      status: switchResponse.status(),
      body: await switchResponse.text().catch(() => ""),
    });
    switchedToCli = true;

    markStage("shell-switch:load-home-cli");
    await page.goto(HOME_URL, { waitUntil: "domcontentloaded" });
    await waitForSignedHome(page);
    const cliFrame = await waitForCapsuleFrame(page, "home-cli");
    await cliFrame.waitForFunction(() => (
      document.body?.dataset?.runtimeTerminal === "attached" &&
      document.querySelector("#xterm-terminal")?.hidden === false
    ), null, { timeout: 20_000 });
    const cliRoot = await page.evaluate(() => {
      const root = document.querySelector("#active-shell-root");
      const rect = root?.getBoundingClientRect();
      return {
        body: { ...document.body.dataset },
        root: rect ? { top: rect.top, left: rect.left, width: rect.width, height: rect.height } : null,
        viewport: { width: window.innerWidth, height: window.innerHeight },
        frame_src: document.querySelector("#active-shell-frame")?.getAttribute("src") || "",
        root_hidden: root?.hidden !== false,
        host_gui_dom_present: Boolean(document.querySelector(
          "#desktop, .desktop-backdrop, .toolbar, .desktop-workspace, .taskbar, #launcher, #window-template",
        )),
        unlock_visible: document.querySelector("#home-unlock")?.hidden === false,
      };
    });
    const cliState = await cliFrame.evaluate(() => ({
      origin: window.location.origin,
      runtime_terminal: document.body?.dataset?.runtimeTerminal || "",
      terminal_visible: document.querySelector("#xterm-terminal")?.hidden === false,
    }));
    assert(cliRoot.root && !cliRoot.root_hidden, "Home CLI root was not visible", cliRoot);
    assert(
      Math.abs(cliRoot.root.top) <= 1 &&
        Math.abs(cliRoot.root.left) <= 1 &&
        cliRoot.root.width >= cliRoot.viewport.width - 2 &&
        cliRoot.root.height >= cliRoot.viewport.height - 2,
      "Home CLI did not fill the root viewport",
      cliRoot,
    );
    assert(!cliRoot.host_gui_dom_present, "trusted Home host contained Home GUI DOM", cliRoot);
    assert(!cliRoot.unlock_visible, "Home unlock prompt remained visible behind Home CLI", cliRoot);
    assert(cliRoot.frame_src.includes("/apps/home-cli/"), "Home CLI was not the active root", cliRoot);
    assert(cliState.origin !== new URL(HOME_URL).origin, "Home CLI reused the trusted Home origin", cliState);
    assert(cliState.runtime_terminal === "attached" && cliState.terminal_visible, "Home CLI terminal was not ready", cliState);
    assert(!capsuleFrameForTarget(page, "home-gui"), "Home GUI remained loaded behind Home CLI", {
      frames: page.frames().map((frame) => frame.url()),
    });

    markStage("shell-switch:cli-switch-home-gui");
    await pressHomeCliKey(cliFrame, "q");
    try {
      await page.waitForFunction(() => (
        document.body?.dataset?.homeStatus === "ready" &&
        document.body?.dataset?.homeAuthority === "signed" &&
        document.querySelector("#active-shell-root")?.hidden === false &&
        document.querySelector("#active-shell-frame")?.getAttribute("src")?.includes("/apps/home-gui/")
      ), null, { timeout: 30_000 });
    } catch (error) {
      error.details = {
        host: await page.evaluate(async () => {
          const summaryResponse = await fetch("/api/apps/home/summary");
          return {
            body: { ...(document.body?.dataset || {}) },
            frame_src: document.querySelector("#active-shell-frame")?.getAttribute("src") || "",
            frame_route: document.querySelector("#active-shell-frame")?.dataset?.route || "",
            root_target: document.querySelector("#active-shell-root")?.dataset?.target || "",
            summary_status: summaryResponse.status,
            summary: await summaryResponse.json().then((value) => ({
              authority: value?.authority || null,
              active_shell: value?.active_shell || null,
            })).catch(() => null),
          };
        }).catch((detailError) => ({ error: detailError.message || String(detailError) })),
        cli: await cliFrame.evaluate(() => ({
          body: { ...(document.body?.dataset || {}) },
          terminal_text_tail: document.querySelector("#xterm-terminal")?.textContent?.slice(-3000) || "",
          fallback_text_tail: document.querySelector("#terminal-output")?.textContent?.slice(-3000) || "",
        })).catch((detailError) => ({ error: detailError.message || String(detailError) })),
        frames: page.frames().map((frame) => redactSensitiveString(frame.url())),
        console: [...shellConsole],
        page_errors: [...shellPageErrors],
        request_failures: [...shellRequestFailures],
        responses: [...shellResponses],
      };
      throw error;
    }
    const homeGuiFrame = await waitForCapsuleFrame(page, "home-gui");
    try {
      await homeGuiFrame.locator('#desktop-shortcuts .desktop-shortcut[data-target="system"]')
        .waitFor({ state: "visible", timeout: 30_000 });
    } catch (error) {
      error.details = {
        host: await page.evaluate(() => ({
          body: { ...(document.body?.dataset || {}) },
          frame_src: document.querySelector("#active-shell-frame")?.getAttribute("src") || "",
          frame_route: document.querySelector("#active-shell-frame")?.dataset?.route || "",
          root_target: document.querySelector("#active-shell-root")?.dataset?.target || "",
        })).catch((detailError) => ({ error: detailError.message || String(detailError) })),
        gui: await homeGuiFrame.evaluate(() => ({
          body: { ...(document.body?.dataset || {}) },
          document_text: document.body?.innerText?.slice(0, 3000) || "",
          shortcut_count: document.querySelectorAll("#desktop-shortcuts .desktop-shortcut").length,
          toolbar_hidden: document.querySelector(".toolbar")?.hidden,
          taskbar_hidden: document.querySelector(".taskbar")?.hidden,
        })).catch((detailError) => ({ error: detailError.message || String(detailError) })),
        frames: page.frames().map((frame) => redactSensitiveString(frame.url())),
        console: [...shellConsole],
        page_errors: [...shellPageErrors],
        request_failures: [...shellRequestFailures],
        responses: [...shellResponses],
      };
      throw error;
    }
    const restored = {
      host: await page.evaluate(() => ({
        body: { ...document.body.dataset },
        frame_src: document.querySelector("#active-shell-frame")?.getAttribute("src") || "",
        host_gui_dom_present: Boolean(document.querySelector(
          "#desktop, .desktop-backdrop, .toolbar, .desktop-workspace, .taskbar, #launcher, #window-template",
        )),
      })),
      gui: await homeGuiFrame.evaluate(() => ({
        origin: window.location.origin,
        toolbar_visible: document.querySelector(".toolbar")?.hidden === false,
        taskbar_visible: document.querySelector(".taskbar")?.hidden === false,
        system_shortcut_present: Boolean(
          document.querySelector('#desktop-shortcuts .desktop-shortcut[data-target="system"]'),
        ),
      })),
    };
    assert(!restored.host.host_gui_dom_present, "trusted Home host absorbed Home GUI implementation", restored);
    assert(restored.gui.origin !== new URL(HOME_URL).origin, "Home GUI reused the trusted Home origin", restored);
    assert(
      restored.gui.toolbar_visible && restored.gui.taskbar_visible && restored.gui.system_shortcut_present,
      "Home GUI did not restore as the root shell",
      restored,
    );
    assert(!capsuleFrameForTarget(page, "home-cli"), "Home CLI remained loaded behind Home GUI", {
      frames: page.frames().map((frame) => frame.url()),
    });

    markStage("shell-switch:direct-home-cli");
    const directSwitch = await browserApi(page, homeToken, "/api/apps/home/active-shell", {
      method: "POST",
      body: { active: "home-cli" },
    });
    assert(directSwitch.ok, "direct Home CLI switch failed", directSwitch);
    switchedToCli = true;
    await page.goto(HOME_URL, { waitUntil: "domcontentloaded" });
    await waitForSignedHome(page);
    const chatCliFrame = await waitForCapsuleFrame(page, "home-cli");
    await chatCliFrame.waitForFunction(() => (
      document.body?.dataset?.runtimeTerminal === "attached" &&
      document.querySelector("#xterm-terminal")?.hidden === false
    ), null, { timeout: 20_000 });

    markStage("shell-switch:cli-open-chat");
    await pressHomeCliKey(chatCliFrame, "1");
    await chatCliFrame.waitForFunction(() => (
      (document.querySelector("#xterm-terminal")?.textContent || "").includes("Type /home to return Home")
    ), null, { timeout: 30_000 });
    const chatHost = await page.evaluate(() => ({
      body: { ...document.body.dataset },
      frame_src: document.querySelector("#active-shell-frame")?.getAttribute("src") || "",
      host_gui_dom_present: Boolean(document.querySelector(
        "#desktop, .desktop-backdrop, .toolbar, .desktop-workspace, .taskbar, #launcher, #window-template",
      )),
    }));
    const chatTerminal = await chatCliFrame.evaluate(() => {
      const terminalText = document.querySelector("#xterm-terminal")?.textContent || "";
      return {
        terminal_has_chat_prompt: terminalText.includes("Type /home to return Home"),
        terminal_has_chat_identity: terminalText.includes("Chat #general as "),
      };
    });
    const chatNative = { host: chatHost, terminal: chatTerminal };
    assert(chatHost.frame_src.includes("/apps/home-cli/"), "CLI Chat replaced the root shell", chatNative);
    assert(!chatHost.host_gui_dom_present, "CLI Chat instantiated Home GUI DOM", chatNative);
    assert(!capsuleFrameForTarget(page, "home-gui"), "CLI Chat loaded Home GUI", {
      frames: page.frames().map((frame) => frame.url()),
    });
    assert(chatTerminal.terminal_has_chat_prompt, "Home CLI did not enter CLI Chat", chatNative);
    assert(chatTerminal.terminal_has_chat_identity, "Home CLI Chat did not show its identity", chatNative);

    markStage("shell-switch:cli-chat-return-home");
    const cliTextarea = await homeCliXtermTextarea(chatCliFrame);
    await cliTextarea.pressSequentially("/home");
    await cliTextarea.press("Enter");
    await chatCliFrame.waitForFunction(() => {
      const terminalText = document.querySelector("#xterm-terminal")?.textContent || "";
      return /Home\s+Inbox\s+People\s+Apps\s+System/.test(terminalText) &&
        terminalText.includes("Chat [ready]");
    }, null, { timeout: 20_000 });

    markStage("shell-switch:cli-browser-boundary");
    await pressHomeCliKey(chatCliFrame, "b");
    await page.waitForTimeout(500);
    const browserBoundary = {
      host: await page.evaluate(() => ({
        body: { ...document.body.dataset },
        frame_src: document.querySelector("#active-shell-frame")?.getAttribute("src") || "",
        host_gui_dom_present: Boolean(document.querySelector(
          "#desktop, .desktop-backdrop, .toolbar, .desktop-workspace, .taskbar, #launcher, #window-template",
        )),
      })),
      cli: await chatCliFrame.evaluate(() => ({
        runtime_terminal: document.body?.dataset?.runtimeTerminal || "",
        terminal_text_tail: (document.querySelector("#xterm-terminal")?.textContent || "").slice(-2000),
      })),
      gui_frame_present: Boolean(capsuleFrameForTarget(page, "home-gui")),
    };
    assert(browserBoundary.host.frame_src.includes("/apps/home-cli/"), "CLI Browser action replaced the root shell", browserBoundary);
    assert(!browserBoundary.host.host_gui_dom_present, "CLI Browser action instantiated Home GUI DOM", browserBoundary);
    assert(!browserBoundary.gui_frame_present, "CLI Browser action loaded Home GUI", browserBoundary);

    return {
      shell_options: shellOptions,
      cli_root: { fills_viewport: true, origin: cliState.origin, terminal_ready: true },
      restored,
      chat_cli: chatNative,
      browser_boundary: browserBoundary,
    };
  } finally {
    page.off("console", captureConsole);
    page.off("pageerror", capturePageError);
    page.off("requestfailed", captureRequestFailure);
    page.off("response", captureResponse);
    if (switchedToCli) {
      await browserApi(page, homeToken, "/api/apps/home/active-shell", {
        method: "POST",
        body: { active: "home-gui" },
      }).catch(() => null);
      await page.goto(HOME_URL, { waitUntil: "domcontentloaded" }).catch(() => null);
    }
  }
}

async function homeCliXtermTextarea(frame) {
  await frame.locator("#xterm-terminal").click();
  const textarea = frame.locator("#xterm-terminal textarea").first();
  await textarea.waitFor({ state: "attached", timeout: 5_000 });
  await textarea.focus();
  return textarea;
}

async function pressHomeCliKey(frame, key) {
  try {
    const textarea = await homeCliXtermTextarea(frame);
    await textarea.press(key);
  } catch (error) {
    error.details = {
      ...(error.details || {}),
      home_cli_keypress: await frame.evaluate(() => ({
        url: window.location.href,
        body: { ...(document.body?.dataset || {}) },
        terminal_text_tail: document.querySelector("#xterm-terminal")?.textContent?.slice(-2000) || "",
        fallback_text_tail: document.querySelector("#terminal-output")?.textContent?.slice(-2000) || "",
        textarea_present: Boolean(document.querySelector("#xterm-terminal textarea")),
      })).catch((detailError) => ({ error: detailError.message || String(detailError) })),
    };
    throw error;
  }
}

async function typeHomeCliText(frame, text) {
  const textarea = await homeCliXtermTextarea(frame);
  await textarea.pressSequentially(text);
}

async function checkBrowserLaunchGrant(page, homeToken) {
  assert(homeToken, "checkBrowserLaunchGrant requires a passkey-issued Home token");
  const launched = await page.evaluate(async (token) => {
    const response = await fetch("/api/apps/home/launch", {
      method: "POST",
      headers: { "content-type": "application/json", "x-elastos-home-token": token },
      body: JSON.stringify({ target: "browser" }),
    });
    const text = await response.text();
    let body = {};
    try {
      body = text ? JSON.parse(text) : {};
    } catch {
      body = { raw: text };
    }
    return {
      ok: response.ok,
      status: response.status,
      body,
    };
  }, homeToken);
  assert(launched.ok, "Browser launch grant failed", launched);
  assert(launched.body?.target === "browser", "Browser launch did not resolve the Browser capsule", launched);
  const route = String(launched.body?.route || "");
  const browserToken = assertIsolatedLaunchRoute(route, "browser");
  if (CHECK_BROWSER_UI_SETUP) {
    launched.body.browser_ui_setup = await holdBrowserUiForSetup(page.context(), browserToken, route);
  }
  if (CHECK_BROWSER_UI_INPUT) {
    launched.body.browser_ui_input = await checkBrowserUiInput(page.context(), browserToken, route);
  }
  if (CHECK_BROWSER_EMBEDDED_UI_INPUT) {
    launched.body.browser_embedded_ui_input = await checkBrowserEmbeddedUiInput(page);
  }
  if (OPEN_BROWSER) {
    assert(
      BROWSER_OPEN_CONCURRENT <= BROWSER_OPEN_URLS.length,
      "HOME_VIRTUAL_AUTH_BROWSER_OPEN_CONCURRENT exceeds HOME_VIRTUAL_AUTH_BROWSER_OPEN_URLS",
      { concurrent: BROWSER_OPEN_CONCURRENT, urls: BROWSER_OPEN_URLS },
    );
    let summaryBefore = null;
    let baselinePrincipalSessions = 0;
    if (CHECK_BROWSER_SUMMARY) {
      summaryBefore = await browserApi(page, browserToken, "/api/apps/browser/summary");
      assert(summaryBefore.ok, "Browser summary failed before open", summaryBefore);
      assert(
        summaryBefore.body?.sessions?.schema === "elastos.browser.session-capacity/v1",
        "Browser summary did not include the session-capacity receipt",
        summaryBefore,
      );
      baselinePrincipalSessions = Number(summaryBefore.body.sessions.principal_sessions || 0);
      launched.body.browser_summary = {
        sessions: summaryBefore.body.sessions,
        engine_adapter: summaryBefore.body.engine_adapter,
        net: summaryBefore.body.net,
      };
    }
    const guaranteeLevel = browserOpenGuaranteeLevel(summaryBefore?.body?.engine_adapter);
    const urls = BROWSER_OPEN_URLS;
    const pages = [];
    const closeResults = [];
    let capacityRejection = null;
    try {
      const openAttempts = await Promise.allSettled(
        Array.from({ length: BROWSER_OPEN_CONCURRENT }, async (_, index) => {
          const opened = await browserApi(page, browserToken, "/api/apps/browser/open", {
            method: "POST",
            body: {
              url: urls[index],
              reason: `virtual passkey Browser open smoke ${index + 1}`,
              viewport: { width: BROWSER_OPEN_VIEWPORT_WIDTH, height: BROWSER_OPEN_VIEWPORT_HEIGHT },
              display_mode: BROWSER_OPEN_DISPLAY_MODE,
              guarantee_level: guaranteeLevel,
              ...(BROWSER_REMOTE_EXIT_ID ? { remote_exit_id: BROWSER_REMOTE_EXIT_ID } : {}),
            },
          });
          assert(opened.ok, `Browser app token could not open Runtime Browser page ${index + 1}`, opened);
          const pageId = opened.body?.engine_page?.page_id || "";
          assert(opened.body?.schema === "elastos.browser.open-result/v1", "Browser open returned wrong schema", opened);
          assert(opened.body?.engine_page?.schema === "elastos.browser.engine.page/v1", "Browser open returned wrong engine page schema", opened);
          assert(opened.body.engine_page.direct_network === false, "Browser open reported direct network", opened.body.engine_page);
          assert(pageId, "Browser open did not return a page id", opened.body.engine_page);
          assert(
            String(opened.body.engine_page.display_session?.signaling_url || "").includes(encodeURIComponent(pageId)),
            "Browser open did not return a page-scoped signaling route",
            opened.body.engine_page,
          );
          assert(
            opened.body.engine_page.display_session?.mode === BROWSER_OPEN_DISPLAY_MODE,
            "Browser open returned the wrong display mode",
            opened.body.engine_page,
          );
          if (BROWSER_REMOTE_EXIT_ID) {
            assert(
              opened.body?.stream_session?.backend === BROWSER_REMOTE_EXIT_ID,
              "Browser open did not use the requested remote Exit Node",
              {
                requested_remote_exit_id: BROWSER_REMOTE_EXIT_ID,
                stream_session: publicBrowserStreamSession(opened.body?.stream_session),
              },
            );
          }
          const entry = {
            page_id: pageId,
            url: urls[index],
            requested_remote_exit_id: BROWSER_REMOTE_EXIT_ID || null,
            stream_session: publicBrowserStreamSession(opened.body?.stream_session),
            display_backend: opened.body.engine_page.display_session.display_backend,
            display_mode: opened.body.engine_page.display_session.mode,
            control_scope: "page_route",
            isolated_engine_session: true,
            direct_network: opened.body.engine_page.direct_network,
            actual_url: opened.body.engine_page.actual_url,
            input: null,
            status: null,
            diagnostics: null,
          };
          pages.push(entry);
          entry.status = CHECK_BROWSER_FRAME
            ? await waitForBrowserStatus(page, browserToken, pageId)
            : null;
          entry.input = CHECK_BROWSER_INPUT
            ? await checkBrowserRuntimeInput(page, browserToken, pageId)
            : null;
        }),
      );
      const failedOpen = openAttempts.find((attempt) => attempt.status === "rejected");
      if (failedOpen) {
        throw failedOpen.reason;
      }
      const uniquePageIds = new Set(pages.map((entry) => entry.page_id));
      assert(uniquePageIds.size === pages.length, "Browser concurrent open returned duplicate page IDs", pages);

      const summaryAfterOpen = await browserApi(page, browserToken, "/api/apps/browser/summary");
      assert(summaryAfterOpen.ok, "Browser summary failed after open", summaryAfterOpen);
      assert(
        Number(summaryAfterOpen.body?.sessions?.principal_sessions || 0)
          >= baselinePrincipalSessions + pages.length,
        "Browser session-capacity receipt did not account for opened pages",
        { before: summaryBefore?.body?.sessions, after: summaryAfterOpen.body?.sessions, pages },
      );
      if (EXPECT_BROWSER_CAPACITY_REJECTION) {
        const rejected = await browserApi(page, browserToken, "/api/apps/browser/open", {
          method: "POST",
          body: {
            url: urls[pages.length] || urls[0],
            reason: "virtual passkey Browser capacity rejection smoke",
            viewport: { width: BROWSER_OPEN_VIEWPORT_WIDTH, height: BROWSER_OPEN_VIEWPORT_HEIGHT },
            display_mode: BROWSER_OPEN_DISPLAY_MODE,
            guarantee_level: guaranteeLevel,
            ...(BROWSER_REMOTE_EXIT_ID ? { remote_exit_id: BROWSER_REMOTE_EXIT_ID } : {}),
          },
        });
        assert(!rejected.ok, "Browser capacity rejection smoke unexpectedly opened an extra page", rejected);
        assert(
          rejected.status === 503,
          "Browser capacity rejection must use HTTP 503 Service Unavailable",
          rejected,
        );
        assert(
          rejected.body?.code === "browser_capacity_unavailable",
          "Browser capacity rejection did not preserve the provider error code",
          rejected,
        );
        capacityRejection = {
          status: rejected.status,
          code: rejected.body.code,
          message: rejected.body.message || "",
        };
      }

      const heartbeat = async () => {
        await Promise.all(pages.map(async (entry) => {
          const response = await browserApi(
            page,
            browserToken,
            `/api/apps/browser/pages/${encodeURIComponent(entry.page_id)}/heartbeat`,
            { method: "POST" },
          );
          assert(response.ok, `Browser heartbeat failed for ${entry.page_id}`, response);
          assert(response.body?.schema === "elastos.browser.page-heartbeat/v1", "Browser heartbeat returned wrong schema", response);
        }));
      };
      await heartbeat();
      const holdStartedAt = Date.now();
      while (Date.now() - holdStartedAt < BROWSER_OPEN_HOLD_MS) {
        await delay(Math.min(5000, Math.max(250, BROWSER_OPEN_HOLD_MS - (Date.now() - holdStartedAt))));
        await heartbeat();
      }
      await Promise.all(pages.map(async (entry) => {
        entry.status = await checkBrowserPageStatus(page, browserToken, entry.page_id);
        entry.actual_url = entry.status.actual_url || entry.actual_url;
        if (CHECK_BROWSER_DIAGNOSTICS) {
          entry.diagnostics = await checkBrowserPageDiagnostics(page, browserToken, entry.page_id);
          entry.diagnostic_click_actions = await runBrowserDiagnosticClickSequence(
            page,
            browserToken,
            entry.page_id,
            entry.diagnostics,
          );
        }
      }));
    } finally {
      await Promise.all(pages.map(async (entry) => {
        const closed = await browserApi(
          page,
          browserToken,
          `/api/apps/browser/pages/${encodeURIComponent(entry.page_id)}/close`,
          { method: "POST", body: {} },
        );
        assert(closed.ok, `Browser open smoke could not close Runtime Browser page ${entry.page_id}`, closed);
        assert(
          closed.body?.schema === "elastos.browser.close-result/v1",
          `Browser close for ${entry.page_id} did not return the close-result receipt`,
          closed,
        );
        assert(
          closed.body?.closed === true,
          `Browser close for ${entry.page_id} did not report closed=true`,
          closed,
        );
        if (entry.isolated_engine_session) {
          const reconciledAlreadyClosed =
            closed.body?.reconciled === true &&
            closed.body?.already_closed === true &&
            closed.body?.cleanup?.schema === "elastos.browser.runtime-session-cleanup/v1" &&
            closed.body?.cleanup?.ok === true;
          const isolatedShutdown =
            closed.body?.isolated_session === true &&
            (closed.body?.shutdown?.ok === true || closed.body?.cleanup?.ok === true);
          assert(
            isolatedShutdown || reconciledAlreadyClosed,
            `Browser close for ${entry.page_id} did not shutdown or cleanup the isolated session`,
            closed,
          );
        }
        closeResults.push(closed.body);
      }));
    }
    const summaryAfterClose = await browserApi(page, browserToken, "/api/apps/browser/summary");
    assert(summaryAfterClose.ok, "Browser summary failed after close", summaryAfterClose);
    assert(
      Number(summaryAfterClose.body?.sessions?.principal_sessions || 0) <= baselinePrincipalSessions,
      "Browser session-capacity receipt still counted closed smoke pages",
      {
        before: summaryBefore?.body?.sessions,
        after: summaryAfterClose.body?.sessions,
        pages,
        close_results: closeResults,
      },
    );
    launched.body.browser_open = {
      concurrent_pages: pages.length,
      display_mode: BROWSER_OPEN_DISPLAY_MODE,
      guarantee_level: guaranteeLevel,
      remote_exit_id: BROWSER_REMOTE_EXIT_ID || null,
      hold_ms: BROWSER_OPEN_HOLD_MS,
      baseline_principal_sessions: baselinePrincipalSessions,
      final_principal_sessions: Number(summaryAfterClose.body?.sessions?.principal_sessions || 0),
      capacity_rejection: capacityRejection,
      pages,
      close_results: closeResults,
    };
  } else if (CHECK_BROWSER_SUMMARY) {
    const summary = await browserApi(page, browserToken, "/api/apps/browser/summary");
    assert(summary.ok, "Browser summary failed", summary);
    assert(
      summary.body?.sessions?.schema === "elastos.browser.session-capacity/v1",
      "Browser summary did not include the session-capacity receipt",
      summary,
    );
    launched.body.browser_summary = {
      sessions: summary.body.sessions,
      engine_adapter: summary.body.engine_adapter,
      net: summary.body.net,
    };
  }
  if (CHECK_BROWSER_PROFILE_RESET) {
    const reset = await browserApi(page, browserToken, "/api/apps/browser/profile/reset", {
      method: "POST",
    });
    assert(reset.ok, "Browser profile reset failed", reset);
    assert(
      reset.body?.schema === "elastos.browser.profile-reset/v1" &&
        reset.body?.profile?.scope === "active_principal" &&
        reset.body?.profile?.storage === "principal_owned_profile_disk" &&
        reset.body?.profile?.storage_posture === "principal_owned_reset_scoped_unprotected" &&
        reset.body?.profile?.protected_storage === false &&
        reset.body?.profile?.encrypted === false &&
        reset.body?.profile?.recoverable === false &&
        reset.body?.profile?.recovery === "not_recovery_kit_packaged" &&
        reset.body?.profile?.reset === "whole_profile" &&
        reset.body?.profile?.profile_key == null &&
        reset.body?.profile?.principal_id == null,
      "Browser profile reset returned an unsafe receipt",
      reset.body,
    );
    launched.body.browser_profile_reset = {
      schema: reset.body.schema,
      status: reset.body.status,
      profile: reset.body.profile,
      removed_profile_disk: reset.body.removed_profile_disk === true,
    };
  }
  return launched.body;
}

async function checkAppLaunchMatrix(page, homeToken) {
  assert(homeToken, "checkAppLaunchMatrix requires a passkey-issued Home token");
  await page.goto(HOME_URL, { waitUntil: "domcontentloaded" });
  await waitForSignedHome(page);
  const summary = await browserApi(page, homeToken, "/api/apps/home/summary");
  assert(summary.ok, "Home summary failed before app matrix", summary);
  const targets = Array.isArray(summary.body?.targets) ? summary.body.targets : [];
  const advertised = new Map(
    targets
      .filter((target) => typeof target?.target === "string")
      .map((target) => [target.target, target]),
  );
  const results = [];
  for (const target of APP_MATRIX_TARGETS) {
    const summaryTarget = advertised.get(target);
    if (!summaryTarget) {
      results.push({ target, skipped: "not-advertised" });
      continue;
    }
    const launched = await page.evaluate(async ({ token, appTarget }) => {
      const response = await fetch("/api/apps/home/launch", {
        method: "POST",
        headers: { "content-type": "application/json", "x-elastos-home-token": token },
        body: JSON.stringify({ target: appTarget }),
      });
      const text = await response.text();
      let body = {};
      try {
        body = text ? JSON.parse(text) : {};
      } catch {
        body = { raw: text };
      }
      return {
        ok: response.ok,
        status: response.status,
        body,
      };
    }, { token: homeToken, appTarget: target });
    assert(launched.ok, `Home launch failed for ${target}`, launched);
    assert(launched.body?.target === target, `Home launch resolved the wrong target for ${target}`, launched);
    const route = String(launched.body?.route || "");
    assertIsolatedLaunchRoute(route, target);
    const appPage = await page.context().newPage();
    try {
      const response = await appPage.goto(new URL(route, HOME_URL).toString(), {
        waitUntil: "domcontentloaded",
        timeout: 25_000,
      });
      assert(response?.ok(), `App route did not load successfully for ${target}`, {
        target,
        status: response?.status(),
        route,
      });
      const appState = await appPage.evaluate(() => ({
        title: document.title,
        bodyStatus: document.body?.dataset?.status || document.body?.dataset?.appStatus || "",
        visibleError: [...document.querySelectorAll("[role='alert'], .error, .system-error")]
          .map((node) => node.textContent?.trim() || "")
          .filter(Boolean)
          .slice(0, 3),
      }));
      assert(
        !appState.visibleError.some((text) => /failed to open|access denied|invalid home launch token/i.test(text)),
        `App route rendered an authority error for ${target}`,
        { target, route, appState },
      );
      results.push({
        target,
        title: summaryTarget.title || "",
        route_prefix: route.split("#")[0],
        status: response.status(),
        document_title: appState.title,
        body_status: appState.bodyStatus,
      });
    } finally {
      await appPage.close().catch(() => {});
    }
  }
  return results;
}

async function revokeCurrentPasskey(page, proofBindingId, homeToken) {
  if (!proofBindingId) {
    return { skipped: true, reason: "missing proof binding" };
  }
  assert(homeToken, "revokeCurrentPasskey requires a passkey-issued Home token");
  return page.evaluate(async ({ id, token }) => {
    const response = await fetch(`/api/auth/passkeys/${encodeURIComponent(id)}/revoke`, {
      method: "POST",
      headers: { "x-elastos-home-token": token },
    });
    const text = await response.text();
    let body = {};
    try {
      body = text ? JSON.parse(text) : {};
    } catch {
      body = { raw: text };
    }
    return {
      ok: response.ok,
      status: response.status,
      body,
    };
  }, { id: proofBindingId, token: homeToken });
}

async function main() {
  if (!ALLOW_REMOTE) {
    assert(
      isLoopbackUrl(HOME_URL),
      "Refusing to create a virtual passkey on a non-loopback Home URL without HOME_VIRTUAL_AUTH_ALLOW_REMOTE=1",
      { HOME_URL },
    );
    assert(
      isLocalhostWebAuthnUrl(HOME_URL),
      "WebAuthn virtual passkey smoke must use http://localhost, not a loopback IP, because browsers reject IP addresses as relying-party IDs",
      { HOME_URL },
    );
  }

  const context = await chromium.launchPersistentContext(PROFILE_DIR, {
    headless: HEADLESS,
    ignoreHTTPSErrors: true,
    viewport: { width: 1280, height: 900 },
  });
  let page = context.pages()[0] || await context.newPage();
  let created = null;
  let passkey = null;
  let cleanupResult = null;
  let homeToken = "";
  let cleanupAttempted = false;
  let virtualAuthenticator = null;
  async function cleanupCreatedPasskey() {
    if (
      cleanupAttempted
      || !created?.created
      || !CLEANUP_PASSKEY
      || !passkey?.proof_binding_id
      || !homeToken
    ) {
      return cleanupResult || { skipped: !created?.created || !CLEANUP_PASSKEY };
    }
    cleanupAttempted = true;
    cleanupResult = await revokeCurrentPasskey(page, passkey.proof_binding_id, homeToken);
    return cleanupResult;
  }
  try {
    virtualAuthenticator = await setupVirtualAuthenticator(context, page);
    await page.goto(HOME_URL, { waitUntil: "domcontentloaded" });
    created = await ensureSignedWithVirtualPasskey(page);
    homeToken = created.homeToken;
    passkey = await currentPasskey(page, homeToken);
    assert(passkey?.proof_binding_id, "signed virtual passkey was not visible through the passkey list", passkey);

    await signOut(page, homeToken);
    homeToken = await signBackIn(page);
    const afterSignIn = await currentPasskey(page, homeToken);
    assert(
      afterSignIn?.proof_binding_id === passkey.proof_binding_id,
      "virtual passkey sign-in did not restore the same proof binding",
      { before: passkey, after: afterSignIn },
    );
    const credentialStore = (!created.created || !CLEANUP_PASSKEY)
      ? await persistVirtualAuthenticatorCredentials(virtualAuthenticator)
      : { skipped: true, reason: "created credential will be cleaned up" };

    const homePublicCopy = await checkHomePublicCopy(page);
    const system = await launchSystem(page, homeToken);
    const shellSwitch = await checkShellSwitchJourney(page, homeToken);
    const browserLaunch = INCLUDE_BROWSER ? await checkBrowserLaunchGrant(page, homeToken) : null;
    const appMatrix = CHECK_APP_MATRIX ? await checkAppLaunchMatrix(page, homeToken) : null;

    if (created.created && CLEANUP_PASSKEY) {
      cleanupResult = await cleanupCreatedPasskey();
      assert(cleanupResult.ok, "virtual test passkey cleanup failed", cleanupResult);
    }

    const report = {
      schema: "elastos.home.passkey-virtual-auth-smoke/v1",
      ok: true,
      home_url: HOME_URL,
      profile_dir: PROFILE_DIR,
      created_mode: created.mode,
      proof_binding_id: passkey.proof_binding_id,
      principal_id: passkey.principal_id,
      role: passkey.role,
      virtual_authenticator_credentials: credentialStore,
      system_fields: system.fields,
      home_public_copy: homePublicCopy,
      shell_switch: shellSwitch,
      browser_launch_checked: Boolean(browserLaunch),
      browser_ui_setup: browserLaunch?.browser_ui_setup || null,
      browser_ui_input: browserLaunch?.browser_ui_input || null,
      browser_embedded_ui_input: browserLaunch?.browser_embedded_ui_input || null,
      browser_profile_reset: browserLaunch?.browser_profile_reset || null,
      browser_open_checked: Boolean(browserLaunch?.browser_open),
      browser_open: browserLaunch?.browser_open || null,
      app_matrix_checked: Boolean(appMatrix),
      app_matrix: appMatrix,
      cleanup: cleanupResult || { skipped: !created.created || !CLEANUP_PASSKEY },
    };
    console.log(JSON.stringify(redactSensitive(report), null, 2));
  } catch (error) {
    if (error.skip) {
      console.log(error.message);
      if (error.details) {
        console.log(JSON.stringify(redactSensitive(error.details), null, 2));
      }
      return;
    }
    try {
      const cleanup = await cleanupCreatedPasskey();
      if (cleanup && cleanup.ok === false) {
        console.error("virtual test passkey cleanup failed after smoke error");
        console.error(JSON.stringify(cleanup, null, 2));
      }
    } catch (cleanupError) {
      console.error("virtual test passkey cleanup threw after smoke error");
      console.error(cleanupError.message || cleanupError);
    }
    console.error("FAIL home-passkey-virtual-auth-smoke");
    console.error(error.message || error);
    if (error.stack) {
      console.error(error.stack);
    }
    if (error.details) {
      console.error(JSON.stringify(redactSensitive(error.details), null, 2));
    } else {
      const state = page ? await homeState(page).catch(() => null) : null;
      if (state) {
        state.stage = smokeStage;
        console.error(JSON.stringify(state, null, 2));
      }
    }
    process.exitCode = 1;
  } finally {
    await context.close().catch(() => {});
    if (!PRESERVE_PROFILE && !process.env.HOME_VIRTUAL_AUTH_PROFILE) {
      rmSync(PROFILE_DIR, { recursive: true, force: true });
    }
  }
}

await main();
