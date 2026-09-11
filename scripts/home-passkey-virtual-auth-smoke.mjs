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
import { randomUUID } from "node:crypto";
import { browserOpenResponseEvidence } from "./lib/browser-open-failure.mjs";
import { browserJourneyTargetConfig, readBrowserJourneyHealth, readBrowserJourneyReceipt,
  browserJourneyEngineChoice, browserJourneyEngineRoute, browserJourneyFixtureUrl,
  browserJourneyProfileStorage, browserJourneyProfileBinding } from "./lib/browser-journey-target.mjs";
import { installBrowserJourneyAudioProbe, controlledTonePresent } from "./lib/browser-journey-audio.mjs";
import { diagnoseBrowserJourneyRecovery } from "./lib/browser-journey-recovery.mjs";
import { diagnoseBrowserViewerReload, readBrowserViewerReloadDocument, browserViewerSignalMetadata } from "./lib/browser-journey-viewer-reload.mjs";
import { runBrowserOperatorJourney } from "./lib/browser-journey-operator.mjs";
import { qualificationOptions, createQualificationHarness, createQualificationCancellation, qualificationInteraction } from "./lib/browser-qualification-observer.mjs";

const require = createRequire(new URL("../elastos/tools/browser-playwright-engine/package.json", import.meta.url));
const { chromium } = require("playwright");

const ELASTOS_BASE_URL = (process.env.ELASTOS_BASE_URL || "http://localhost:8090").replace(/\/+$/, "");
const HOME_URL = process.env.HOME_URL || `${ELASTOS_BASE_URL}/home/`;
const TEST_NAME = process.env.HOME_VIRTUAL_AUTH_NAME || `Agent Smoke ${new Date().toISOString()}`;
const HEADLESS = process.env.HOME_VIRTUAL_AUTH_HEADED !== "1";
const PRESERVE_PROFILE = process.env.HOME_VIRTUAL_AUTH_PRESERVE_PROFILE === "1";
const CLEANUP_PASSKEY = process.env.HOME_VIRTUAL_AUTH_CLEANUP !== "0";
const INCLUDE_BROWSER = process.env.HOME_VIRTUAL_AUTH_BROWSER === "1";
const CHECK_BROWSER_VIEWER_PREFLIGHT = process.env.HOME_VIRTUAL_AUTH_BROWSER_VIEWER_PREFLIGHT === "1";
const CHECK_APP_MATRIX = process.env.HOME_VIRTUAL_AUTH_APP_MATRIX === "1";
const CHECK_RECOVERY_EXPORT = process.env.HOME_VIRTUAL_AUTH_RECOVERY_EXPORT === "1";
const CHECK_SHELL_SWITCH = process.env.HOME_VIRTUAL_AUTH_SHELL_SWITCH !== "0";
const CHECK_SYSTEM = process.env.HOME_VIRTUAL_AUTH_SYSTEM !== "0";
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
const CHECK_BROWSER_CONTROLLED_JOURNEY =
  process.env.HOME_VIRTUAL_AUTH_BROWSER_CONTROLLED_JOURNEY === "1";
const CHECK_BROWSER_CONTROLLED_MEDIA = process.env.HOME_VIRTUAL_AUTH_BROWSER_CONTROLLED_MEDIA === "1";
const CHECK_BROWSER_CONTROLLED_INSPECTION = process.env.HOME_VIRTUAL_AUTH_BROWSER_CONTROLLED_INSPECTION === "1";
const CHECK_BROWSER_CONTROLLED_OPERATOR = process.env.HOME_VIRTUAL_AUTH_BROWSER_CONTROLLED_OPERATOR === "1";
const REUSE_SIGNED_HOME = process.env.HOME_VIRTUAL_AUTH_REUSE_SIGNED_HOME === "1";
const BROWSER_QUALIFICATION_OPTIONS = qualificationOptions(process.env);
const qualificationCancellation = createQualificationCancellation(BROWSER_QUALIFICATION_OPTIONS);
let browserQualification = null;
const BROWSER_OPERATOR_COORDS_PATH = process.env.HOME_VIRTUAL_AUTH_BROWSER_OPERATOR_COORDS || "";
const CHECK_BROWSER_CONTROLLED_RECOVERY = process.env.HOME_VIRTUAL_AUTH_BROWSER_CONTROLLED_RECOVERY === "1";
const CHECK_BROWSER_VIEWER_RELOAD = process.env.HOME_VIRTUAL_AUTH_BROWSER_CONTROLLED_VIEWER_RELOAD === "1";
const BROWSER_CONTROLLED_TURN_TEST_HOME = process.env.HOME_VIRTUAL_AUTH_BROWSER_CONTROLLED_TURN_TEST_HOME || "";
const REQUIRE_BROWSER_VZ_TRANSPORT = process.env.HOME_VIRTUAL_AUTH_BROWSER_REQUIRE_VZ_TRANSPORT === "1";
const BROWSER_JOURNEY_TARGET = CHECK_BROWSER_CONTROLLED_JOURNEY || process.env.HOME_VIRTUAL_AUTH_BROWSER_PROFILE_MODE ||
  process.env.HOME_VIRTUAL_AUTH_BROWSER_PROFILE_MARKER ? browserJourneyTargetConfig(process.env) : null;
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
  if (!stage.includes("close") && !stage.includes("cleanup")) qualificationCancellation?.check();
  smokeStage = stage;
  if (CHECK_BROWSER_CONTROLLED_JOURNEY) {
    console.error(JSON.stringify({ stage, at: new Date().toISOString() }));
  }
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
  // WebAuthn clone detection demands a strictly increasing sign counter, and
  // a replayed snapshot would sit at or below the server's stored count.
  // Timer-based counters are valid authenticator behaviour, so resume from
  // wall-clock seconds — always ahead of any prior run.
  const timerCount = Math.floor(Date.now() / 1000) - 1_767_225_600;
  for (const credential of readVirtualAuthenticatorCredentialStore()) {
    await cdp.send("WebAuthn.addCredential", {
      authenticatorId,
      credential: {
        ...credential,
        signCount: Math.max(Number(credential.signCount) || 0, timerCount),
      },
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
      ["credential", "auth_secret", "transport_secret"].includes(key) ||
      ["home_token", "homeToken"].includes(key)
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
  const homeOrigin = new URL(HOME_URL).origin;
  return page.frames().find((frame) => {
    try {
      const url = new URL(frame.url());
      if (!url.pathname.startsWith(`/apps/${target}/`)) {
        return false;
      }
      // The shell mounts capsule frames on the Home origin and isolates them
      // through iframe sandboxing; older builds used per-app localhost hosts.
      return url.origin === homeOrigin || url.hostname.split(".")[0] === target;
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
    url.origin === new URL(HOME_URL).origin,
    `${target} launch route left the trusted Home origin`,
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
    activeShellFrameHidden: document.querySelector("#active-shell-frame")?.hidden === true,
    activeShellFrameSrc: document.querySelector("#active-shell-frame")?.getAttribute("src") || "",
    activeShellFrameHasSrcdoc: document.querySelector("#active-shell-frame")?.hasAttribute("srcdoc") !== false,
    hostGuiDomPresent: Boolean(document.querySelector(
      "#desktop, .desktop-backdrop, .toolbar, .desktop-workspace, .taskbar, #launcher, #window-template",
    )),
  }));
}

async function assertSignedOutShell(page, state) {
  assert(
    state.authority === "unsigned" && state.shell === "resolving" && state.gui === "dormant"
      && state.activeShellRootHidden && state.activeShellFrameHidden
      && state.activeShellFrameSrc === "about:blank" && !state.activeShellFrameHasSrcdoc
      && !state.hostGuiDomPresent,
    "A Home shell remained mounted behind the passkey prompt",
    state,
  );
  // Inspect through Playwright because Home's opaque iframe sandbox owns a
  // separate origin. A blank src attribute alone does not prove it unloaded.
  const element = await page.locator("#active-shell-frame").elementHandle();
  const frame = await element?.contentFrame();
  assert(frame, "Home's cleared shell frame was missing", state);
  await frame.waitForFunction(
    () => document.URL === "about:blank" && document.head?.childNodes.length === 0
      && document.body?.childNodes.length === 0,
    null,
    { timeout: 5_000 },
  );
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

async function browserApi(page, token, path, { method = "GET", body = null, timeoutMs = null } = {}) {
  return page.evaluate(async ({ token, path, method, body, timeoutMs }) => {
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
      ...(timeoutMs === null ? {} : { signal: AbortSignal.timeout(timeoutMs) }),
    });
    const text = await response.text();
    let payload = {};
    try {
      payload = text ? JSON.parse(text) : {};
    } catch {
      payload = { raw: text };
    }
    return { ok: response.ok, status: response.status, body: payload };
  }, { token, path, method, body, timeoutMs });
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

async function waitForEmbeddedBrowserPage(appFrame, failures, ignoredOpenIds = new Set()) {
  const deadline = Date.now() + BROWSER_UI_PAGE_ID_TIMEOUT_MS;
  while (Date.now() < deadline) {
    const failure = failures.find(entry => entry.frame === appFrame &&
      !(entry.open_id && ignoredOpenIds.has(entry.open_id)));
    if (failure) {
      throw Object.assign(new Error("Embedded Browser startup returned a failed open settlement"), {
        details: { stage: smokeStage, status: failure.status, settlement: failure.body,
          response_format: failure.response_format, admission_reason: failure.admission_reason },
      });
    }
    const pageId = await appFrame.evaluate(() => window.__elastosBrowserCurrentPageId || "");
    if (pageId) return pageId;
    await delay(200);
  }
  throw new Error("Embedded Browser UI did not publish the current page id before timeout");
}

async function waitForJourneyEvidence(read, predicate, label, timeoutMs = 30_000) {
  const deadline = performance.now() + timeoutMs;
  let last;
  while (performance.now() < deadline) {
    last = await read();
    if (predicate(last)) return last;
    await delay(200);
  }
  throw Object.assign(new Error(`Controlled Browser journey timed out: ${label}`), { details: { last } });
}

function browserJourneyRuntimeEmpty(sessions) {
  return sessions?.schema === "elastos.browser.session-capacity/v1" && sessions.status === "configured" &&
    sessions.recoverable_page === null && Array.isArray(sessions.lifecycle?.sessions) && sessions.lifecycle.sessions.length === 0 &&
    ["active_sessions", "principal_sessions", "total_sessions", "launching_sessions",
      "engine_cleanup_obligations", "launch_reconciliation_obligations"].every(key => sessions[key] === 0);
}

async function captureBrowserWindowIdentity(appFrame) {
  const iframe = await appFrame.frameElement();
  const section = (await iframe.evaluateHandle(node => node.closest('section.window[data-target="browser"]'))).asElement();
  assert(section, "Exact Browser frame has no Home window section");
  const windowId = await section.getAttribute("data-window-id");
  assert(typeof windowId === "string" && /^[A-Za-z0-9:_-]{1,128}$/.test(windowId),
    "Exact Browser window has no stable identity");
  const route = await iframe.getAttribute("src") || "";
  const token = assertIsolatedLaunchRoute(route, "browser");
  const instance = new URL(route, HOME_URL).searchParams.get("browser_instance") || "";
  const parent = appFrame.parentFrame();
  const locator = parent.locator(`section.window[data-target="browser"][data-window-id="${windowId}"]`);
  const identity = { appFrame, iframe, section, parent, locator, windowId, token, instance };
  await assertBrowserWindowIdentity(identity, appFrame, token);
  return identity;
}

async function assertBrowserWindowIdentity(identity, appFrame, token) {
  assert(identity?.appFrame === appFrame && identity.token === token && identity.instance &&
    new URL(appFrame.url()).searchParams.get("browser_instance") === identity.instance &&
    assertIsolatedLaunchRoute(appFrame.url(), "browser") === token,
  "Browser frame, window and authority identity differ");
  assert(await identity.locator.count() === 1 &&
    await identity.locator.evaluate((node, captured) => node === captured, identity.section),
  "Browser window identity was replaced or duplicated");
  assert(await identity.section.evaluate((node, { iframe, windowId }) => node.isConnected &&
    node.dataset.windowId === windowId && iframe.isConnected &&
    node.querySelector("iframe.window-frame") === iframe &&
    iframe.closest('section.window[data-target="browser"]') === node,
  { iframe: await appFrame.frameElement(), windowId: identity.windowId }),
  "Browser frame no longer belongs to the captured Home window");
}

async function clickBrowserWindowClose(identity, appFrame, token) {
  await focusCapturedBrowserWindow(identity, appFrame, token);
  await assertBrowserWindowIdentity(identity, appFrame, token);
  // ElementHandle.click cannot retarget another window during auto-wait.
  const button = await identity.section.$('[data-action="close"]');
  assert(button && /^(Close|Retry Browser close)$/.test(await button.getAttribute("aria-label")),
    "Captured Browser window has no ordinary close control");
  try { await button.click(); } finally { await button.dispose(); }
}

async function focusCapturedBrowserWindow(identity, appFrame, token) {
  await assertBrowserWindowIdentity(identity, appFrame, token);
  const actionable = await identity.section.evaluate(node => {
    const button = node.querySelector('[data-action="close"]');
    const rect = button?.getBoundingClientRect();
    return node.classList.contains("window-active") && rect?.width > 0 && rect?.height > 0 &&
      button.contains(node.ownerDocument.elementFromPoint(rect.x + rect.width / 2, rect.y + rect.height / 2));
  });
  if (actionable) return;
  // Home's per-window Shelf action raises this window without toggling another Browser.
  await identity.parent.locator('#taskbar-targets [data-target="browser"]').first().click({ button: "right", timeout: 5_000 });
  await identity.parent.locator(`#desktop-context-menu [data-context-action="focus-window:${identity.windowId}"]`)
    .click({ timeout: 5_000 });
  await identity.parent.waitForFunction(node => node.isConnected && node.classList.contains("window-active"),
    identity.section, { timeout: 5_000 });
  await assertBrowserWindowIdentity(identity, appFrame, token);
}

async function waitForBrowserWindowDetached(identity) {
  await identity.parent.waitForFunction(node => !node.isConnected, identity.section,
    { timeout: 15_000 });
}

async function closeControlledBrowserWindow(page, appFrame, windowIdentity, token, baseline,
  { expectedPageId = null, requireEmptyRuntime = CHECK_BROWSER_CONTROLLED_JOURNEY } = {}) {
  markStage("browser:ui-close");
  // The opaque Home GUI survives Browser frame removal and supplies Origin: null.
  const apiFrame = appFrame.parentFrame();
  const instance = new URL(appFrame.url()).searchParams.get("browser_instance") || "";
  const summaryPath = `/api/apps/browser/summary?browser_instance=${encodeURIComponent(instance)}`;
  let before = await browserApi(apiFrame, token, summaryPath);
  if (!before.body?.sessions?.recoverable_page) {
    // Browser owns an in-flight open until its existing settlement finishes.
    await appFrame.waitForFunction(() => document.body?.dataset.loading !== "true", null,
      { timeout: BROWSER_UI_PAGE_ID_TIMEOUT_MS }).catch(() => null);
    before = await browserApi(apiFrame, token, summaryPath);
  }
  const owner = before.body?.sessions?.recoverable_page;
  assert(before.ok, "Browser close summary failed", before);
  const started = performance.now();
  const evidence = { messages: [], frames: [], close_responses: [], dropped_messages: 0, dropped_frames: 0, dropped_responses: 0 };
  // Signed Home can restore Browser before the harness reads its entry summary.
  // That observation is useful evidence, but a controlled task must end empty.
  const counts = sessions => Object.fromEntries(["active_sessions", "principal_sessions", "total_sessions",
    "launching_sessions", "engine_cleanup_obligations", "launch_reconciliation_obligations"]
    .map(key => [key, Number.isSafeInteger(sessions?.[key]) ? sessions[key] : null]));
  evidence.runtime_counts = { entry: counts(baseline), before_close: counts(before.body?.sessions),
    terminal_requirement: requireEmptyRuntime ? "configured_empty_runtime" : "entry_baseline" };
  const safeText = value => redactSensitiveString(typeof value === "string" ? value : "")
    .split(token || "\0").join("[redacted]").slice(0, 256);
  const append = (list, dropped, value) => {
    if (list.length === 32) { list.shift(); evidence[dropped]++; }
    list.push(value);
  };
  const frameEvent = (frame, event) => {
    if (frame !== appFrame && frame !== apiFrame) return;
    const url = new URL(frame.url());
    append(evidence.frames, "dropped_frames", { event, at_ms: Math.round(performance.now() - started),
      frame: frame === appFrame ? "browser" : "home", url: safeText(`${url.origin}${url.pathname}`),
      instance_matches: url.searchParams.get("browser_instance") === instance });
  };
  const navigated = frame => frameEvent(frame, "navigated");
  const detached = frame => frameEvent(frame, "detached");
  // Install in Home before clicking. Home survives removal/navigation of the Browser document.
  const messages = await apiFrame.evaluateHandle(({ sourceFrame, instance, token }) => {
    const source = sourceFrame.contentWindow;
    const events = [];
    const started = performance.now();
    let dropped = 0;
    let pending = null;
    let settle;
    const terminal = new Promise(resolve => { settle = resolve; });
    const listener = event => {
      const data = event.data;
      if (event.source !== source || event.origin !== "null" || data?.homeToken !== token ||
        data?.type !== "elastos.browser.window-close.result/v1" ||
        data.browserInstance !== instance || !["pending", "error", "terminal"].includes(data.state)) return;
      const safe = value => {
        let text = typeof value === "string" ? value : "";
        for (const secret of [token, data.homeToken]) if (typeof secret === "string" && secret) text = text.split(secret).join("[redacted]");
        return text.replace(/https?:\/\/[^\s]+/gi, "[url]")
          .replace(/home_token=[^&\s]+/gi, "home_token=[redacted]").slice(0, 256);
      };
      if (events.length === 32) { events.shift(); dropped++; }
      events.push({ at_ms: Math.round(performance.now() - started), requestId: safe(data.requestId),
        state: data.state, reason: safe(data.reason), terminalKind: safe(data.terminalKind),
        pageId: safe(data.pageId), cleanupId: safe(data.cleanupId),
        generation: Number.isSafeInteger(data.generation) ? data.generation : null });
      const message = { request_id: data.requestId, state: data.state, terminal_kind: data.terminalKind,
        page_id: data.pageId, cleanup_id: data.cleanupId, generation: data.generation };
      if (!pending && data.state === "pending" && data.pageId && data.cleanupId) pending = message;
      if (data.state === "terminal") settle({ message, pending });
    };
    const timer = setTimeout(() => settle({ timed_out: true }), 60_000);
    window.addEventListener("message", listener);
    return { terminal, stop: () => {
      clearTimeout(timer);
      window.removeEventListener("message", listener);
      settle({ stopped: true });
      return { events, dropped };
    } };
  }, { sourceFrame: await appFrame.frameElement(), instance, token });
  const closeResponses = [];
  const runtimeOrigin = new URL(appFrame.url()).origin;
  const effects = ["page_absent", "child_absent", "vm_absent", "route_absent", "socket_absent"];
  const transportEffects = ["transport_session_absent", "turn_process_absent", "turn_listener_absent",
    "turn_relay_ports_absent", "ordinary_vsock_bridge_absent", "media_vsock_bridge_absent",
    "bootstrap_vsock_bridge_absent", "hibernation_state_absent"];
  const closeResponse = async response => {
    try {
      const request = response.request();
      const url = new URL(response.url());
      const path = url.pathname.match(/^\/api\/apps\/browser\/pages\/([^/]+)\/close$/);
      if (request.frame() !== appFrame || request.method() !== "POST" || url.origin !== runtimeOrigin || !path) return;
      if (closeResponses.length === 32) { evidence.dropped_responses++; return; }
      const body = request.postDataJSON();
      const record = { page_id: decodeURIComponent(path[1]), request: body, receipt: null,
        ok: response.ok(), authority_matches: request.headers()["x-elastos-home-token"] === token };
      closeResponses.push(record);
      const detail = { at_ms: Math.round(performance.now() - started), status: response.status(),
        page_id: safeText(record.page_id), authority_matches: record.authority_matches,
        request: { schema: safeText(body?.schema), cleanup_id: safeText(body?.cleanup_id),
          browser_instance: safeText(body?.browser_instance) }, receipt: null };
      record.detail = detail;
      evidence.close_responses.push(detail);
      const receipt = await response.json().catch(() => null);
      record.receipt = receipt;
      if (receipt) detail.receipt = {
        schema: safeText(receipt.schema), page_id: safeText(receipt.page_id), cleanup_id: safeText(receipt.cleanup_id),
        browser_instance: safeText(receipt.browser_instance), closed: receipt.closed === true,
        already_closed: receipt.already_closed === true,
        cleanup: { schema: safeText(receipt.cleanup?.schema), ok: receipt.cleanup?.ok === true,
          action: safeText(receipt.cleanup?.action) },
        terminal_effects: Object.fromEntries([...effects, ...transportEffects]
          .filter(key => key in (receipt.terminal_effects || {})).map(key => [key, receipt.terminal_effects[key] === true])),
        transport_proof_present: Boolean(receipt.transport_proof),
      };
    } catch (error) {
      evidence.response_error = safeText(error.message);
    }
  };
  page.on("response", closeResponse);
  page.on("framenavigated", navigated);
  page.on("framedetached", detached);
  try {
    assert(!expectedPageId || owner?.page_id === expectedPageId,
      "Successful Browser journey requires fresh close of its last exercised page", { expectedPageId, owner });
    if (!owner) {
      const messagePromise = messages.evaluate(observer => observer.terminal)
        .then(result => result, error => ({ error }));
      await clickBrowserWindowClose(windowIdentity, appFrame, token);
      const terminal = await messagePromise;
      if (terminal.error) throw terminal.error;
      assert(!terminal.timed_out, "Browser startup close remained pending");
      const message = terminal.message;
      let receipt;
      if (message.terminal_kind === "already_absent") {
        const pending = terminal.pending;
        assert(instance && pending && typeof message.request_id === "string" && message.request_id &&
          Number.isSafeInteger(message.generation) && message.generation >= 0 &&
          pending.request_id === message.request_id && pending.page_id === message.page_id &&
          pending.cleanup_id === message.cleanup_id && pending.generation === message.generation,
        "Browser prior-page close did not bind the pending UI handle");
        const observed = await waitForJourneyEvidence(() => closeResponses.find(record =>
          record.page_id === message.page_id && record.request?.cleanup_id === message.cleanup_id &&
          record.receipt?.schema === "elastos.browser.close-result/v1" && record.receipt.already_closed === true),
        value => Boolean(value), "exact Runtime receipt for the retained Browser UI handle");
        receipt = observed.receipt;
        assert(observed.ok && observed.authority_matches && observed.request.schema === "elastos.browser.close-request/v2" &&
          observed.request.browser_instance === instance && receipt.schema === "elastos.browser.close-result/v1" &&
          receipt.page_id === message.page_id && receipt.cleanup_id === message.cleanup_id && receipt.browser_instance === instance &&
          receipt.closed === false && receipt.already_closed === true &&
          receipt.cleanup?.schema === "elastos.browser.runtime-session-cleanup/v1" && receipt.cleanup.ok === true &&
          receipt.cleanup.action === "already_absent" && receipt.terminal_effects?.page_absent === true &&
          Object.values(receipt.terminal_effects).every(value => value === true) && !receipt.transport_proof,
        "Browser prior-page close lacks an exact Runtime tombstone receipt");
        // Runtime's tombstone proves prior settlement; it does not repeat the Engine transport receipt.
        receipt = observed.detail.receipt;
      } else {
        assert(message.terminal_kind === "no_page" && !message.page_id && !message.cleanup_id,
          "Browser startup close did not confirm absent ownership", message);
      }
      await waitForBrowserWindowDetached(windowIdentity);
      const after = await browserApi(apiFrame, token, summaryPath);
      assert(after.ok && (requireEmptyRuntime ? browserJourneyRuntimeEmpty(after.body?.sessions) :
        after.body?.sessions?.recoverable_page === null &&
        after.body.sessions.principal_sessions <= baseline.principal_sessions &&
        after.body.sessions.total_sessions <= baseline.total_sessions &&
        after.body.sessions.launching_sessions <= baseline.launching_sessions &&
        after.body.sessions.engine_cleanup_obligations <= baseline.engine_cleanup_obligations &&
        after.body.sessions.launch_reconciliation_obligations <= baseline.launch_reconciliation_obligations),
      "Browser startup close retained Runtime ownership", after);
      return { startup_close: message, receipt, window_detached: true, sessions_after_close: after.body.sessions, close_evidence: evidence };
    }
    assert(owner.page_id && owner.cleanup?.schema === "elastos.browser.cleanup-handle/v1",
      "Controlled Browser close requires the exact Runtime page and cleanup handle", before);
    const messagePromise = messages.evaluate(observer => observer.terminal)
      .then(result => result, error => ({ error }));
    await clickBrowserWindowClose(windowIdentity, appFrame, token);
    const terminal = await messagePromise;
    if (terminal.error) throw terminal.error;
    assert(!terminal.timed_out, "Browser UI close remained pending");
    const { message, pending } = terminal;
    assert(instance && pending && message?.terminal_kind === "closed" &&
      typeof message.request_id === "string" && message.request_id &&
      Number.isSafeInteger(message.generation) && message.generation >= 0 &&
      pending.request_id === message.request_id && pending.generation === message.generation &&
      pending.page_id === owner.page_id && message.page_id === owner.page_id &&
      pending.cleanup_id === owner.cleanup.id && message.cleanup_id === owner.cleanup.id,
    "Browser UI close did not bind its pending and terminal ownership", { message, pending });
    const observed = await waitForJourneyEvidence(() => closeResponses.find(record =>
      record.page_id === owner.page_id && record.request?.cleanup_id === owner.cleanup.id &&
      record.receipt?.schema === "elastos.browser.close-result/v1" && record.receipt.closed === true),
    value => Boolean(value), "exact Runtime receipt for the exercised Browser page");
    const { receipt, request } = observed;
    assert(observed.ok && observed.authority_matches && request?.schema === "elastos.browser.close-request/v2" &&
      request.cleanup_id === owner.cleanup.id && receipt.cleanup_id === owner.cleanup.id &&
      receipt.closed === true && receipt.already_closed !== true &&
      receipt.page_id === owner.page_id && request.browser_instance === instance && receipt.browser_instance === instance,
    "UI close returned a mismatched Runtime cleanup receipt", { request, receipt });
    if (REQUIRE_BROWSER_VZ_TRANSPORT) {
      assert(receipt.transport_proof?.schema === "elastos.browser.vz-transport-public-proof/v1" &&
        receipt.transport_proof.page_id === owner.page_id,
      "UI close requires the selected VZ transport proof", receipt);
    }
    const requiredEffects = receipt.transport_proof ? [...effects, ...transportEffects] : effects;
    assert(receipt.cleanup?.schema === "elastos.browser.runtime-session-cleanup/v1" &&
      receipt.cleanup.ok === true && requiredEffects.every(key => receipt.terminal_effects?.[key] === true),
    "UI close did not confirm Runtime and Engine cleanup", receipt);
    await waitForBrowserWindowDetached(windowIdentity);
    const after = await waitForJourneyEvidence(
      () => browserApi(apiFrame, token, summaryPath),
      value => value.ok && (requireEmptyRuntime ? browserJourneyRuntimeEmpty(value.body?.sessions) :
        value.body?.sessions?.schema === "elastos.browser.session-capacity/v1" &&
        value.body.sessions.principal_sessions === baseline.principal_sessions &&
        value.body.sessions.total_sessions === baseline.total_sessions &&
        value.body.sessions.launching_sessions === baseline.launching_sessions &&
        value.body.sessions.engine_cleanup_obligations <= baseline.engine_cleanup_obligations &&
        value.body.sessions.launch_reconciliation_obligations <= baseline.launch_reconciliation_obligations &&
        value.body.sessions.recoverable_page === null),
      requireEmptyRuntime ? "Controlled Runtime sessions and cleanup obligations are empty" :
        "Runtime sessions and cleanup obligations return to baseline");
    markStage("browser:cleanup-confirmed");
    return { receipt, window_detached: true, sessions_before_launch: baseline, sessions_after_close: after.body.sessions, close_evidence: evidence };
  } catch (error) {
    error.details = { ...error.details, close_evidence: evidence };
    throw error;
  } finally {
    page.off("response", closeResponse);
    page.off("framenavigated", navigated);
    page.off("framedetached", detached);
    const collected = await messages.evaluate(observer => observer.stop()).catch(error => ({ error: safeText(error.message) }));
    evidence.messages = collected.events || [];
    evidence.dropped_messages = collected.dropped || 0;
    if (collected.error) evidence.message_error = collected.error;
    await messages.dispose().catch(() => {});
  }
}

async function browserRecoveryCdpSession(page, appFrame) {
  let ancestorDepth = 0;
  for (let frame = appFrame; frame; frame = frame.parentFrame(), ancestorDepth++) {
    try {
      const session = await page.context().newCDPSession(frame.parentFrame() ? frame : page);
      return { session, ancestorDepth };
    } catch (error) {
      if (!frame.parentFrame() || !String(error.message).includes("This frame does not have a separate CDP session")) throw error;
    }
  }
  throw new Error("Browser viewer CDP session is unavailable");
}

async function observeControlledBrowserRequests(page, appFrame, token, record,
  { probeId = randomUUID(), recordNavigation = false, signal } = {}) {
  const ensureActive = () => { if (signal?.aborted) throw new Error("Browser observer setup canceled"); };
  ensureActive();
  const instance = new URL(appFrame.url()).searchParams.get("browser_instance");
  const runtimeOrigin = new URL(appFrame.url()).origin;
  const sourceChain = [];
  for (let frame = appFrame; frame.parentFrame(); frame = frame.parentFrame()) {
    const element = await frame.frameElement();
    let index;
    try {
      ensureActive();
      index = await frame.parentFrame().evaluate(node =>
        Array.from(document.querySelectorAll("iframe,frame")).indexOf(node), element);
    } finally { await element.dispose(); }
    ensureActive();
    assert(index >= 0, "Recovery observer could not identify the Browser frame");
    sourceChain.unshift(index);
  }
  const requests = new WeakMap();
  const pendingResponses = new Set();
  let sequence = 0, documentGeneration = 0;
  const request = req => {
    if (req.frame() !== appFrame) return;
    const url = new URL(req.url());
    if (url.origin !== runtimeOrigin) return;
    const kind = url.searchParams.get("recovery_probe") === probeId ? "probe"
      : /^\/api\/apps\/browser\/open(?:\/|$)/.test(url.pathname) && req.method() === "POST" ? "opening"
      : /\/pages\/[^/]+\/close$/.test(url.pathname) ? "closing"
      : /\/pages\/[^/]+\/status$/.test(url.pathname) ? "status"
      : recordNavigation && url.pathname === "/api/apps/browser/summary" && url.searchParams.get("browser_instance") === instance ? "summary"
      : /\/pages\/[^/]+\/heartbeat$/.test(url.pathname) ? "heartbeat"
      : recordNavigation && /\/pages\/[^/]+\/webrtc$/.test(url.pathname) ? "signaling" : null;
    if (!kind) return;
    let details = {};
    if (kind === "signaling") {
      try { details = browserViewerSignalMetadata(req.postDataJSON()); } catch {}
    }
    const event = { kind, ...details, request_id: `viewer-request-${++sequence}`, source_matches: true, document_generation: documentGeneration };
    requests.set(req, event);
    record({ ...event, phase: "request" });
  };
  const failed = req => { const event = requests.get(req); if (event) record({ ...event, phase: "failed" }); };
  const response = res => {
    const event = requests.get(res.request());
    if (!event) return;
    if (event.kind !== "signaling" || (event.signal_type !== "display_attach" && res.status() < 400)) {
      record({ ...event, phase: "response", status: res.status() });
      return;
    }
    record({ ...event, phase: "headers", status: res.status() });
    const pending = res.json().then(body => browserViewerSignalMetadata(body), () => ({}))
      .then(details => record({ ...event, ...details, phase: "response", status: res.status() }))
      .finally(() => pendingResponses.delete(pending));
    pendingResponses.add(pending);
  };
  const navigation = frame => {
    if (recordNavigation && frame === appFrame) record({ kind: "navigation", phase: "commit",
      request_id: `viewer-navigation-${++documentGeneration}`, source_matches: true, document_generation: documentGeneration });
  };
  let messages = null, cleanup = Promise.resolve();
  const stop = () => {
    page.off("framenavigated", navigation);
    page.off("request", request);
    page.off("requestfailed", failed);
    page.off("response", response);
    signal?.removeEventListener("abort", onAbort);
    if (messages) {
      const handle = messages;
      messages = null;
      cleanup = cleanup.then(async () => {
        try { await handle.evaluate(observer => observer.stop()); }
        finally { await handle.dispose(); }
      });
    }
    return Promise.all([cleanup, Promise.allSettled([...pendingResponses])]);
  };
  const onAbort = () => { void stop().catch(() => {}); };
  signal?.addEventListener("abort", onAbort, { once: true });
  try {
    ensureActive();
    const bindingName = `__browserRecovery_${probeId.replaceAll("-", "")}`;
    await page.exposeBinding(bindingName, (source, event) => {
      if (source.frame === page.mainFrame()) record(event);
    });
    ensureActive();
    messages = await page.evaluateHandle(({ sourceChain, token, instance, bindingName }) => {
      const source = sourceChain.reduce((frame, index) => frame.frames[index], window);
      const pending = new Set();
      let overflow = false;
      const listener = event => {
        const data = event.data;
        if (event.source !== source || event.origin !== "null" || data?.homeToken !== token ||
          data.browserInstance !== instance || data.type !== "elastos.home.browser-authority-renew.request/v1" ||
          typeof data.requestId !== "string" || data.requestId.length > 512) return;
        if (pending.size >= 64) { overflow = true; return; }
        const promise = window[bindingName]({ kind: "renewal", phase: "request", request_id: data.requestId, source_matches: true });
        pending.add(promise);
        promise.finally(() => pending.delete(promise)).catch(() => {});
      };
      window.addEventListener("message", listener);
      return { stop: async () => {
        window.removeEventListener("message", listener);
        await Promise.allSettled(pending);
        if (overflow) throw new Error("Recovery authority observer overflow");
      } };
    }, { sourceChain, token, instance, bindingName });
    ensureActive();
    page.on("framenavigated", navigation);
    page.on("request", request);
    page.on("requestfailed", failed);
    page.on("response", response);
    return stop;
  } catch (error) {
    await stop();
    throw error;
  }
}

async function runControlledBrowserRecovery(page, appFrame, token, readReceipt, expectedUrl) {
  markStage("browser:controlled-recovery");
  const instance = new URL(appFrame.url()).searchParams.get("browser_instance");
  assert(instance, "Recovery probe requires the current Browser instance");
  const runtimeOrigin = new URL(appFrame.url()).origin;
  const summaryUrl = new URL(`/api/apps/browser/summary?browser_instance=${encodeURIComponent(instance)}`, runtimeOrigin);
  const probeId = randomUUID();
  const mediaInterruption = BROWSER_CONTROLLED_TURN_TEST_HOME
    ? await (await import("./lib/browser-journey-turn-interruption.mjs")).createBrowserTurnInterruption({
      testHome: BROWSER_CONTROLLED_TURN_TEST_HOME,
      runtimeOrigin,
      pageId: await appFrame.evaluate(() => window.__elastosBrowserCurrentPageId),
    }).catch(error => {
      error.details = { recovery: { ok: false, failure: "media_driver_binding_failed",
        media_driver: error.evidence } };
      throw error;
    }) : null;
  const { session: cdp, ancestorDepth } = await browserRecoveryCdpSession(page, appFrame);
  try {
    const evidence = await diagnoseBrowserJourneyRecovery({
      cdp,
      expectedUrl,
      mediaInterruption,
      readBinding: async ({ signal }) => {
        // This observer uses the same scoped authority outside the cut viewer.
        const response = await fetch(summaryUrl, { headers: { Origin: "null", "x-elastos-home-token": token }, signal });
        assert(response.ok, "Recovery Runtime summary failed");
        const body = await response.json();
        const viewer = await appFrame.evaluate(() => ({
          page_id: window.__elastosBrowserCurrentPageId || "",
          browser_instance: new URL(location.href).searchParams.get("browser_instance"),
          actual_url: document.querySelector("#browser-url")?.value || "",
          engine_id: document.querySelector("#browser-engine")?.value,
          exit_id: document.querySelector("#browser-exit")?.value,
        }));
        const statusResponse = await fetch(new URL(`/api/apps/browser/pages/${encodeURIComponent(viewer.page_id)}/status`, runtimeOrigin),
          { headers: { Origin: "null", "x-elastos-home-token": token }, signal });
        assert(statusResponse.ok, "Recovery page status failed");
        return { sessions: body.sessions, page_status: await statusResponse.json(), ...viewer };
      },
      readVideo: async () => {
        const metrics = await browserRemoteVideoMetrics(appFrame);
        const display = await browserRemoteDisplayMetrics(appFrame);
        const bytes = display?.latestVideoWebrtcStats?.video_bytes_received ?? display?.latestWebrtcStats?.video_bytes_received;
        return { ...metrics, ...(Number.isSafeInteger(bytes) ? { video_bytes_received: bytes } : {}) };
      },
      readReceipt,
      extendInput: (suffix, { timeoutMs }) => appFrame.locator("#browser-keyboard-capture")
        .pressSequentially(suffix, { timeout: Math.ceil(timeoutMs) }),
      probeViewerRequest: ({ timeoutMs }) => appFrame.evaluate(async ({ probeId, timeoutMs }) => {
        const id = window.__elastosBrowserCurrentPageId;
        const token = new URLSearchParams(location.hash.slice(1)).get("home_token");
        try {
          const response = await fetch(`/api/apps/browser/pages/${encodeURIComponent(id)}/status?recovery_probe=${probeId}`,
            { cache: "no-store", headers: { "x-elastos-home-token": token }, signal: AbortSignal.timeout(Math.ceil(timeoutMs)) });
          await response.text();
        } catch {}
      }, { probeId, timeoutMs }),
      observeRequests: (record, { signal }) => observeControlledBrowserRequests(page, appFrame, token, record, { probeId, signal }),
    });
    return { ...evidence, cdp_ancestor_depth: ancestorDepth,
      ...(mediaInterruption ? { media_driver: mediaInterruption.evidence } : {}) };
  } catch (error) {
    error.details = { ...error.details, recovery: { ...error.evidence,
      failure: error.evidence?.failure || "probe_setup_or_observation_failed", cdp_ancestor_depth: ancestorDepth,
      ...(mediaInterruption ? { media_driver: mediaInterruption.evidence } : {}) } };
    throw error;
  }
}

async function runControlledBrowserViewerReload(page, appFrame, token, readReceipt, expectedUrl) {
  markStage("browser:controlled-viewer-reload");
  const instance = new URL(appFrame.url()).searchParams.get("browser_instance");
  const runtimeOrigin = new URL(appFrame.url()).origin;
  assert(instance, "Viewer reload requires the current Browser instance");
  const stateObservation = { clock: "performance.now", steps: [], dropped_steps: 0 };
  let sampleId = 0;
  try {
    const evidence = await diagnoseBrowserViewerReload({
      expectedUrl, readReceipt,
      readState: async ({ signal, deadlineMs }) => {
        const sample = ++sampleId;
        const step = async (name, action) => {
          const row = { sample, step: name, start_ms: performance.now(), deadline_ms: deadlineMs };
          if (stateObservation.steps.length < 64) stateObservation.steps.push(row);
          else stateObservation.dropped_steps++;
          const finish = outcome => {
            if (row.end_ms === undefined) { row.end_ms = performance.now(); row.outcome = outcome; }
          };
          const aborted = () => finish("aborted");
          signal.addEventListener("abort", aborted, { once: true });
          try {
            if (signal.aborted) { aborted(); throw new Error("Viewer reload observation canceled"); }
            const result = await action(() => { if (row.end_ms === undefined) row.headers_ms = performance.now(); });
            if (signal.aborted) throw new Error("Viewer reload observation canceled");
            finish("complete");
            return result;
          } catch (error) { finish(signal.aborted ? "aborted" : "failed"); throw error; }
          finally { signal.removeEventListener("abort", aborted); }
        };
        const headers = { Origin: "null", "x-elastos-home-token": token };
        const { sessions } = await step("runtime_summary", async headersReceived => {
          const response = await fetch(new URL(`/api/apps/browser/summary?browser_instance=${encodeURIComponent(instance)}`, runtimeOrigin),
            { headers, signal });
          headersReceived();
          assert(response.ok, "Viewer reload Runtime summary failed");
          return response.json();
        });
        const pageId = sessions?.recoverable_page?.page_id;
        let page_status = null;
        if (pageId) {
          page_status = await step("remote_page_status", async headersReceived => {
            const status = await fetch(new URL(`/api/apps/browser/pages/${encodeURIComponent(pageId)}/status`, runtimeOrigin), { headers, signal });
            headersReceived();
            if (status.ok) return status.json();
            assert(status.status === 404, "Viewer reload Runtime page status failed");
            return null;
          });
        }
        let visible = { viewer: null, video: null };
        try {
          // Read the document identity and media in one execution context.
          visible = await step("viewer_document_metrics", () => appFrame.evaluate(readBrowserViewerReloadDocument));
        } catch (error) {
          if (!/Execution context was destroyed|Cannot find context with specified id/.test(String(error.message))) throw error;
        }
        return { sessions, page_status, ...visible };
      },
      reloadViewer: async ({ timeoutMs }) => {
        await Promise.all([
          appFrame.waitForNavigation({ waitUntil: "commit", timeout: Math.ceil(timeoutMs) }),
          appFrame.evaluate(() => location.reload()),
        ]);
      },
      extendInput: (suffix, { timeoutMs }) => appFrame.locator("#browser-keyboard-capture")
        .pressSequentially(suffix, { timeout: Math.ceil(timeoutMs) }),
      observeRequests: (record, { signal }) => observeControlledBrowserRequests(page, appFrame, token, record, { recordNavigation: true, signal }),
    });
    return { ...evidence, state_observation: stateObservation };
  } catch (error) {
    error.details = { ...error.details, viewer_reload: { ...(error.evidence || { ok: false, failure: "probe_setup_or_observation_failed" }),
      state_observation: stateObservation } };
    throw error;
  }
}

async function observeControlledBrowserInput(page, appFrame, token, pageId) {
  const started = performance.now();
  const origin = new URL(appFrame.url()).origin;
  const instance = new URL(appFrame.url()).searchParams.get("browser_instance");
  const inputPath = `/api/apps/browser/pages/${encodeURIComponent(pageId)}/input`;
  const evidence = { schema: "elastos.browser.journey-input-observation/v1", requests: [],
    dropped_requests: 0, filter_rejections: { foreign_origin: 0, authority_mismatch: 0 },
    observer: { stopped: false } };
  const requests = new WeakMap(), pending = new Set();
  let sequence = 0, handle, stopped = false, finished, collecting = true;
  const bounded = async (promise, fallback, timeoutMs = 2000) => {
    let timer;
    try { return await Promise.race([promise, new Promise(resolve => {
      timer = setTimeout(() => resolve(fallback), Math.max(0, timeoutMs));
    })]); } finally { clearTimeout(timer); }
  };
  const record = value => {
    if (evidence.requests.length < 32) evidence.requests.push({ at_ms: Math.round(performance.now() - started), ...value });
    else evidence.dropped_requests++;
  };
  const request = req => {
    if (!collecting) return;
    try {
      const url = new URL(req.url()), headers = req.headers();
      if (req.frame() !== appFrame || url.origin !== origin || url.pathname !== inputPath || req.method() !== "POST") return;
      // Playwright's initial header view can omit Origin. The request URL still
      // binds the Runtime origin; retain the exact frame, page and launch grant.
      const rejection = ![undefined, "null", origin].includes(headers.origin) ? "foreign_origin"
        : headers["x-elastos-home-token"] !== token ? "authority_mismatch" : null;
      if (rejection) {
        evidence.filter_rejections[rejection] = Math.min(32, evidence.filter_rejections[rejection] + 1);
        return;
      }
      if (sequence >= 16) { evidence.dropped_requests++; return; }
      let event;
      try { event = req.postDataJSON()?.event; } catch {}
      const metadata = { request_id: ++sequence,
        event_type: ["click", "paste_text", "key", "wheel"].includes(event?.type) ? event.type : "other",
        ...(event?.type === "paste_text" && typeof event.text === "string" ? { text_length: event.text.length } : {}) };
      let settle;
      const complete = new Promise(resolve => { settle = resolve; });
      pending.add(complete);
      requests.set(req, { metadata, settle: () => { pending.delete(complete); settle(); } });
      record({ ...metadata, phase: "request" });
    } catch { /* Detached or unrelated requests carry no input evidence. */ }
  };
  const failed = req => {
    const entry = requests.get(req);
    if (entry && !stopped) {
      record({ ...entry.metadata, phase: "failed" });
      requests.delete(req);
      entry.settle();
    }
  };
  const response = res => {
    const entry = requests.get(res.request());
    if (!entry || stopped) return;
    requests.delete(res.request());
    const at_ms = Math.round(performance.now() - started), status = res.status();
    void bounded(Promise.resolve().then(() => res.json()).then(body => ({
      schema_matches: body?.schema === "elastos.browser.input-result/v1",
      page_matches: body?.page_id === pageId,
      ...(typeof body?.accepted === "boolean" ? { accepted: body.accepted } : {}),
    }), () => ({ body_unavailable: true })), { body_unavailable: true })
      .then(details => { if (!stopped) record({ ...entry.metadata, phase: "response", at_ms, status, ...details }); })
      .finally(entry.settle);
  };
  page.on("request", request);
  page.on("requestfailed", failed);
  page.on("response", response);
  const setup = appFrame.evaluateHandle(({ pageId, origin, instance }) => {
    const events = [];
    let dropped = 0, expired = false;
    const target = node => ["browser-keyboard-capture", "browser-render-panel", "browser-remote-display", "browser-url"]
      .includes(node?.id) ? node.id : "other";
    const ownerMatches = () => window.__elastosBrowserCurrentPageId === pageId &&
      new URL(location.href).origin === origin && new URL(location.href).searchParams.get("browser_instance") === instance;
    const listener = event => {
      if (!ownerMatches()) return;
      if (events.length >= 8) { dropped++; return; }
      events.push({ viewer_at_ms: Math.round(performance.now() - started), target: target(event.target),
        active_target: target(document.activeElement), default_prevented: event.defaultPrevented,
        printable: typeof event.key === "string" && [...event.key].length === 1,
        control: event.ctrlKey, meta: event.metaKey, alt: event.altKey, shift: event.shiftKey });
    };
    const started = performance.now();
    document.addEventListener("keydown", listener);
    const stop = failure => {
      document.removeEventListener("keydown", listener);
      clearTimeout(timer);
      if (!failure) return null;
      if (!ownerMatches()) return { owner_matches: false };
      const status = document.querySelector("#browser-status")?.textContent || "";
      // Fixed categories keep backend errors, website text and authority out of receipts.
      const category = /input channel is not open/i.test(status) ? "input_channel_unavailable"
        : /input is busy/i.test(status) ? "input_busy"
        : /input was canceled/i.test(status) ? "input_canceled"
        : /could not send that input/i.test(status) ? "input_rejected"
        : /temporarily unavailable/i.test(status) ? "browser_unavailable"
        : /Remote display ready/i.test(status) ? "display_ready"
        : status ? "other" : "empty";
      return { owner_matches: true, expired, has_focus: document.hasFocus(), active_target: target(document.activeElement),
        loading: document.body?.dataset?.loading === "true", address_disabled: document.querySelector("#browser-url")?.disabled === true,
        status: category, keys: events, dropped_keys: dropped };
    };
    const timer = setTimeout(() => { expired = true; stop(false); }, 40_000);
    return { stop };
  }, { pageId, origin, instance }).then(async value => {
    if (!collecting) {
      try { await value.evaluate(observer => observer.stop(false)); } finally { await value.dispose(); }
      return null;
    }
    handle = value;
    return value;
  }).catch(() => { evidence.observer.setup_failed = true; return null; });
  if (!await bounded(setup, null)) evidence.observer.setup_failed = true;
  const stop = failure => finished ||= (async () => {
    collecting = false;
    page.off("request", request);
    const deadline = performance.now() + 2000;
    const value = handle;
    handle = null;
    const collected = value ? bounded(value.evaluate((observer, failure) => observer.stop(failure), failure)
      .then(viewer => ({ viewer, stopped: true }), () => ({ stopped: false }))
      .finally(() => value.dispose().catch(() => {})), { stopped: false }) : Promise.resolve({ stopped: false });
    const [snapshot, drained] = await Promise.all([collected,
      bounded(Promise.allSettled([...pending]).then(() => true), false, deadline - performance.now())]);
    stopped = true;
    page.off("requestfailed", failed);
    page.off("response", response);
    evidence.observer.drained = drained;
    if (failure) evidence.viewer = snapshot.viewer || { unavailable: true };
    evidence.observer.stopped = snapshot.stopped;
    return evidence;
  })();
  return { evidence, stop };
}

async function runControlledBrowserOperator(appFrame, token, pageId, expectedUrl, readReceipt, closeWindow) {
  const runtimeOrigin = new URL(appFrame.url()).origin;
  const instance = new URL(appFrame.url()).searchParams.get("browser_instance");
  assert(instance, "Operator journey requires the current Browser instance");
  let runtimeCoords;
  try {
    const text = readFileSync(BROWSER_OPERATOR_COORDS_PATH, "utf8");
    assert(text.length <= 32768, "Operator Runtime coordinates exceed their bound");
    runtimeCoords = JSON.parse(text);
  } catch { throw new Error("Operator Runtime coordinates are unavailable or invalid"); }
  return runBrowserOperatorJourney({
    runtimeOrigin, runtimeCoords, homeToken: token, pageId, expectedUrl,
    ...(qualificationCancellation ? { fetchImpl: qualificationCancellation.fetch } : {}),
    readReceipt, markStage, requireVzTransport: REQUIRE_BROWSER_VZ_TRANSPORT,
    readState: async ({ signal }) => {
      if (qualificationCancellation) signal = AbortSignal.any([signal, qualificationCancellation.signal]);
      const headers = { Origin: "null", "x-elastos-home-token": token };
      const response = await fetch(new URL(`/api/apps/browser/summary?browser_instance=${encodeURIComponent(instance)}`, runtimeOrigin),
        { headers, signal });
      assert(response.ok, "Operator Runtime summary failed");
      const { sessions } = await response.json();
      const status = await fetch(new URL(`/api/apps/browser/pages/${encodeURIComponent(pageId)}/status`, runtimeOrigin), { headers, signal });
      assert(status.ok, "Operator Runtime page status failed");
      const page_status = await status.json();
      const visible = await appFrame.evaluate(readBrowserViewerReloadDocument);
      return { sessions, page_status, ...visible };
    },
    humanInput: (text, { timeoutMs }) => appFrame.locator("#browser-keyboard-capture")
      .pressSequentially(text, { timeout: Math.ceil(timeoutMs) }),
    closeWindow,
  });
}

async function runControlledBrowserJourney(page, appFrame, windowIdentity, token, baseline, failures) {
  const fixture = BROWSER_JOURNEY_TARGET;
  const run = randomUUID();
  const result = { schema: "elastos.browser.controlled-journey/v1", run, pages: [] };
  if (fixture.profile) result.profile = { schema: "elastos.browser.profile-journey/v1", ...fixture.profile,
    run, started_at: Date.now(), complete: false, documents: [] };
  let engineChoice = null;
  const openRoutes = new Map();
  const captureRoute = async response => {
    if (!/^\/api\/apps\/browser\/open(?:\/[^/]+)?$/.test(new URL(response.url()).pathname)) return;
    try { if (response.request().frame() !== appFrame) return; } catch { return; }
    const body = await response.json().catch(() => null);
    const opened = body?.schema === "elastos.browser.open-result/v1" ? body :
      body?.schema === "elastos.browser.open-status/v1" && body.status === "completed" ? body.result : null;
    if (opened?.engine_page?.page_id && openRoutes.size < 16) {
      openRoutes.set(opened.engine_page.page_id, publicBrowserStreamSession(opened.stream_session));
    }
  };
  page.on("response", captureRoute);
  const readReceipt = options => readBrowserJourneyReceipt(fixture, run, options);
  const checkProfileDocument = async (name, pageId, afterSequence = 0) => {
    if (!fixture.profile) return;
    markStage(`browser:profile-${fixture.profile.mode}-${name}`);
    const event = await waitForJourneyEvidence(async () => {
      const receipt = await readReceipt();
      result.profile.receipt = receipt;
      return browserJourneyProfileStorage(fixture, run, receipt, name, afterSequence);
    }, value => Boolean(value), "Engine profile storage completion", 20_000);
    const summary = await browserApi(appFrame, token,
      `/api/apps/browser/summary?browser_instance=${encodeURIComponent(windowIdentity.instance)}`, { timeoutMs: 15_000 });
    assert(summary.ok, "Browser profile ownership summary is unavailable");
    const binding = browserJourneyProfileBinding(fixture, summary.body, pageId, appFrame.url(), token);
    assert(!result.profile.binding || JSON.stringify(binding) === JSON.stringify(result.profile.binding),
      "Browser profile identity changed within this phase", { previous: result.profile.binding, observed: binding });
    result.profile.binding = binding;
    result.profile.documents.push({ name, page_id: pageId, event });
  };
  let failure = null, inputObserver = null, operatorClosePromise = null;
  const stopInputObservation = async failed => {
    try { await inputObserver?.stop(failed); }
    catch {
      result.input_observation.observer = { ...result.input_observation.observer, stopped: false, stop_failed: true };
    }
  };
  try {
    if (fixture.engineId) {
      markStage("browser:controlled-engine-selection");
      result.requested_engine_id = fixture.engineId;
      const summary = await browserApi(appFrame, token,
        // Remote offers can consume 5s; allow the other sequential summary reads and transport.
        `/api/apps/browser/summary?browser_instance=${encodeURIComponent(windowIdentity.instance)}`, { timeoutMs: 15_000 });
      assert(summary.ok, "Browser Engine selection summary is unavailable");
      engineChoice = browserJourneyEngineChoice(summary.body, fixture.engineId);
    }
    // Ordinary address entry is available after Browser settles its current open.
    // This source disables the field while opening; the harness preserves that UI rule.
    markStage("browser:controlled-address-ready");
    await appFrame.locator("#browser-url").waitFor({ state: "visible", timeout: 15_000 });
    await appFrame.waitForFunction(() => document.querySelector("#browser-url")?.disabled === false,
      null, { timeout: BROWSER_UI_PAGE_ID_TIMEOUT_MS });
    if (fixture.engineId) {
      await appFrame.locator("#browser-settings").click();
      await appFrame.locator("#browser-engine").selectOption(fixture.engineId);
      assert(await appFrame.locator("#browser-engine").inputValue() === fixture.engineId,
        "Requested Browser Engine UI selection failed");
      await appFrame.locator("#browser-settings-close").click();
    }
    if (BROWSER_REMOTE_EXIT_ID) {
      await appFrame.locator("#browser-settings").click();
      await appFrame.locator("#browser-exit").selectOption(BROWSER_REMOTE_EXIT_ID);
      assert(await appFrame.locator("#browser-exit").inputValue() === BROWSER_REMOTE_EXIT_ID,
        "Controlled Browser did not select the requested remote Exit");
      result.remote_exit_id = BROWSER_REMOTE_EXIT_ID;
      await appFrame.locator("#browser-settings-close").click();
    }
    const settled = failures.filter(entry => {
      const outcome = entry.body?.error?.outcome || entry.body?.outcome;
      return entry.frame === appFrame && entry.open_id && outcome?.schema === "elastos.browser.open-outcome/v1" &&
        ["terminal_pre_effect_failure", "terminal_post_effect_cleanup"].includes(outcome.state);
    });
    const ignoredOpenIds = new Set(settled.map(entry => entry.open_id));
    result.prior_open_settlements = settled.map(({ open_id, body }) => ({ open_id, settlement: body }));
    for (const name of ["main", "nav"]) {
      markStage(`browser:controlled-${name}`);
      const documentSequence = fixture.profile ? (await readReceipt()).events.at(-1)?.sequence || 0 : 0;
      const url = browserJourneyFixtureUrl(fixture, run, name,
        { media: CHECK_BROWSER_CONTROLLED_MEDIA, qualification: Boolean(browserQualification) });
      const navigationStarted = performance.now();
      await appFrame.locator("#browser-url").fill(url);
      await appFrame.locator("#browser-url").press("Enter");
      const status = await waitForJourneyEvidence(async () => {
        const id = await waitForEmbeddedBrowserPage(appFrame, failures, ignoredOpenIds);
        const value = await browserApi(appFrame, token, `/api/apps/browser/pages/${encodeURIComponent(id)}/status`);
        return { page_id: id, ok: value.ok, ...value.body };
      }, value => value.ok && value.schema === "elastos.browser.page-status/v1" && value.actual_url === url,
      `Runtime navigates to ${name}`, BROWSER_UI_PAGE_ID_TIMEOUT_MS);
      const statusReadyMs = Math.round(performance.now() - navigationStarted);
      if (engineChoice) {
        const summary = await browserApi(appFrame, token,
          `/api/apps/browser/summary?browser_instance=${encodeURIComponent(windowIdentity.instance)}`, { timeoutMs: 15_000 });
        assert(summary.ok, "Browser Engine ownership summary is unavailable");
        result.engine_route = browserJourneyEngineRoute(summary.body, engineChoice, status.page_id);
      }
      if (BROWSER_REMOTE_EXIT_ID) {
        const route = await waitForJourneyEvidence(async () => openRoutes.get(status.page_id),
          value => Boolean(value), "remote Exit open receipt");
        assert(route.backend === BROWSER_REMOTE_EXIT_ID,
          "Controlled Browser used a different Exit", { expected: BROWSER_REMOTE_EXIT_ID, route });
        result.exit_route = route;
      }
      assert(status.direct_network === false && status.display_session?.media_transport === "runtime_relay" &&
        runtimeRelayIceContractOk(status.display_session), "Controlled Browser lost Runtime relay authority", status);
      await appFrame.waitForFunction(expected => document.querySelector("#browser-url")?.value === expected,
        url, { timeout: 15_000 });
      const receipt = await waitForJourneyEvidence(readReceipt,
        value => value.events.some(event => event.type === "load" && event.page === name), `${name} page load`);
      const load = receipt.events.find(event => event.type === "load" && event.page === name);
      const ready = await waitForBrowserRemoteVideo(appFrame, { browserToken: token, pageId: status.page_id,
        timeoutMs: BROWSER_REMOTE_VIDEO_TIMEOUT_MS });
      const decoded = await waitForJourneyEvidence(() => browserRemoteVideoMetrics(appFrame),
        value => value.decoded_frames > ready.decoded_frames && value.decoded_frames > 0,
        `${name} decoded WebRTC frame progress`, BROWSER_REMOTE_VIDEO_TIMEOUT_MS);
      const navigationStatus = await appFrame.locator("#browser-status").evaluate(node => ({
        visible: node.dataset.visible === "true",
        opening: /^Opening /.test(node.querySelector(".browser-status-message")?.textContent || ""),
      }));
      assert(!(navigationStatus.visible && navigationStatus.opening),
        "Browser retained navigation progress after the controlled page loaded", { name, navigationStatus });
      result.pages.push({ name, url, page_id: status.page_id, load, video: { ready, decoded }, navigation_status: navigationStatus,
        ...(engineChoice ? { engine_route: result.engine_route } : {}),
        timing: { status_ready_ms: statusReadyMs, decoded_progress_ms: Math.round(performance.now() - navigationStarted) } });
      await checkProfileDocument(name, status.page_id, documentSequence);
    }
    const current = result.pages.at(-1);
    inputObserver = await observeControlledBrowserInput(page, appFrame, token, current.page_id);
    result.input_observation = inputObserver.evidence;
    const rect = current.load.input_rect;
    const point = await remoteVideoClickPositionForPagePoint(appFrame,
      { x: rect.x + rect.width / 2, y: rect.y + rect.height / 2 });
    assert(point, "Controlled Browser input could not be mapped into the decoded video");
    markStage("browser:type");
    await appFrame.locator("#browser-remote-display").click({ position: { x: point.x, y: point.y } });
    const text = `Browser-${run.slice(0, 8)}`;
    let typed = "";
    for (const character of text) {
      await appFrame.locator("#browser-keyboard-capture").pressSequentially(character);
      typed += character;
      await waitForJourneyEvidence(readReceipt,
        value => value.events.some(event => event.page === "nav" && event.type === "input" && event.value === typed),
        "Engine page receives typed text");
      if (typed.length === 1) await stopInputObservation(false);
    }
    if (CHECK_BROWSER_CONTROLLED_INSPECTION) {
      markStage("browser:operator-inspection");
      const path = `/api/apps/browser/pages/${encodeURIComponent(current.page_id)}/inspect`;
      const capabilities = await browserApi(appFrame, token, path);
      result.inspection = { capabilities, pages: [] };
      assert(capabilities.ok && capabilities.body?.formats?.includes("accessibility_tree"),
        "The installed Browser does not provide Engine-page inspection", result.inspection);
      let cursor = null, snapshotId = null, generation = null;
      const nodes = [];
      do {
        const response = await browserApi(appFrame, token, path, { method: "POST",
          body: { schema: "elastos.browser.inspect-request/v1", limit: 8, cursor } });
        result.inspection.pages.push(response);
        assert(response.ok && response.body?.schema === "elastos.browser.inspect-result/v1" &&
          response.body.page_id === current.page_id && Array.isArray(response.body.nodes),
          "Engine-page inspection failed", result.inspection);
        const body = response.body;
        snapshotId ||= body.snapshot_id; generation ||= body.document_generation;
        assert(body.snapshot_id === snapshotId && body.document_generation === generation,
          "Engine inspection changed document during pagination", result.inspection);
        nodes.push(...body.nodes);
        cursor = body.next_cursor;
        assert(result.inspection.pages.length <= 8 && nodes.length <= 512 &&
          (!cursor || result.inspection.pages.length < 8), "Engine inspection pagination exceeded its bounds", result.inspection);
      } while (cursor);
      assert(nodes.some(node => node.role === "textbox" && node.name === "Test text" && node.value === text),
        "The operator did not inspect the text entered through Browser UI", result.inspection);
      assert(result.inspection.pages.length > 1, "The controlled operator proof did not exercise pagination", result.inspection);
    }
    if (CHECK_BROWSER_CONTROLLED_MEDIA) {
      markStage("browser:decoded-audio");
      const toneReceipt = await waitForJourneyEvidence(readReceipt,
        value => value.events.some(event => event.page === "nav" && event.type === "audio" &&
          event.audio_state === "running" && event.frequency_hz === 440), "controlled Engine tone starts");
      result.audio = await appFrame.evaluate(() => window.__readBrowserJourneyAudio());
      result.audio.fixture_event = toneReceipt.events.find(event => event.page === "nav" && event.type === "audio");
      if (!controlledTonePresent(result.audio)) {
        result.audio.receiver_metrics = await browserRemoteDisplayMetrics(appFrame).catch(() => null);
        result.audio.engine_diagnostics = await checkBrowserPageDiagnostics(appFrame, token, current.page_id)
          .catch(error => ({ error: error.message, details: error.details }));
      }
      assert(controlledTonePresent(result.audio), "Controlled 440 Hz tone was not decoded by the product receiver", result.audio);
    }
    markStage("browser:scroll");
    await appFrame.locator("#browser-remote-display").hover();
    await page.mouse.wheel(0, 640);
    const receipt = await waitForJourneyEvidence(readReceipt,
      value => value.events.some(event => event.page === "nav" && event.type === "scroll" &&
        event.value === text && event.scroll_y >= current.load.scroll_y + 100), "Engine page scroll movement");
    const afterInput = await waitForJourneyEvidence(() => browserRemoteVideoMetrics(appFrame),
      value => value.decoded_frames > current.video.decoded.decoded_frames, "decoded frames after input");
    result.input = { text, receipt, video_after_input: afterInput };
    if (browserQualification) {
      result.qualification = await browserQualification.observe({ appFrame, pageId: current.page_id, readReceipt,
        readStatus: () => browserApi(appFrame, token, `/api/apps/browser/pages/${encodeURIComponent(current.page_id)}/status`),
        interact: (index, { signal, timeoutMs }) => qualificationInteraction({
          index, pageId: current.page_id, run, text, readReceipt, signal, timeoutMs,
          key: (method, value, budget) => appFrame.locator("#browser-keyboard-capture")[method](value, budget),
          wheel: async (delta, budget) => {
            await appFrame.locator("#browser-remote-display").hover(budget());
            budget(); await page.mouse.wheel(0, delta);
          },
        }) });
    }
    if (CHECK_BROWSER_CONTROLLED_RECOVERY) result.recovery = await runControlledBrowserRecovery(page, appFrame, token, readReceipt, result.pages.at(-1).url);
    if (CHECK_BROWSER_VIEWER_RELOAD) result.viewer_reload = await runControlledBrowserViewerReload(page, appFrame, token, readReceipt, result.pages.at(-1).url);
    if (CHECK_BROWSER_CONTROLLED_MEDIA && (CHECK_BROWSER_CONTROLLED_RECOVERY || CHECK_BROWSER_VIEWER_RELOAD)) {
      markStage("browser:decoded-audio-after-recovery");
      result.audio_after_recovery = await appFrame.evaluate(() => window.__readBrowserJourneyAudio());
      assert(controlledTonePresent(result.audio_after_recovery),
        "Controlled tone did not resume at the product audio receiver", result.audio_after_recovery);
    }
    if (CHECK_BROWSER_CONTROLLED_INSPECTION) {
      markStage("browser:operator-stale-reference");
      const resetSequence = (await readReceipt()).events.at(-1)?.sequence || 0;
      const url = browserJourneyFixtureUrl(fixture, run, "main",
        { media: CHECK_BROWSER_CONTROLLED_MEDIA, qualification: Boolean(browserQualification) });
      await appFrame.locator("#browser-url").fill(url);
      await appFrame.locator("#browser-url").press("Enter");
      await waitForJourneyEvidence(() => browserApi(appFrame, token,
        `/api/apps/browser/pages/${encodeURIComponent(current.page_id)}/status`),
      value => value.ok && value.body?.actual_url === url, "UI navigation keeps the acquired Engine page");
      const response = await browserApi(appFrame, token,
        `/api/apps/browser/pages/${encodeURIComponent(current.page_id)}/inspect`,
        { method: "POST", body: { schema: "elastos.browser.inspect-request/v1", limit: 8,
          cursor: result.inspection.pages[0].body.next_cursor } });
      result.inspection.after_navigation = response;
      result.inspection.after_navigation_url = url;
      assert(response.status === 409 && response.body?.code === "stale_inspection",
        "The old inspection cursor survived UI navigation", result.inspection);
      const resetReceipt = await waitForJourneyEvidence(readReceipt, value => value.events.some(event =>
        event.type === "load" && event.page === "main" && event.sequence > resetSequence), "controlled main document reload");
      result.inspection.after_navigation_load = resetReceipt.events.find(event =>
        event.type === "load" && event.page === "main" && event.sequence > resetSequence);
      await checkProfileDocument("main", current.page_id, resetSequence);
    }
    // Recheck after optional recovery/reload, including every retained failure event.
    if (fixture.profile) await checkProfileDocument(CHECK_BROWSER_CONTROLLED_INSPECTION ? "main" : "nav", current.page_id);
    if (CHECK_BROWSER_CONTROLLED_OPERATOR) {
      try {
        result.operator = await runControlledBrowserOperator(appFrame, token, current.page_id,
          result.inspection.after_navigation_url, readReceipt, () => {
            // Share the original close attempt with the outer finally even if
            // the operator probe times out while Home is still settling close.
            operatorClosePromise ||= closeControlledBrowserWindow(page, appFrame, windowIdentity, token, baseline,
              { expectedPageId: current.page_id });
            return operatorClosePromise;
          });
      } catch (error) {
        result.operator = error.evidence || { ok: false, failure: "operator_setup_failed" };
        throw error;
      }
    }
  } catch (error) {
    if (error.qualification) result.qualification = error.qualification;
    else if (browserQualification) result.qualification ||= browserQualification.snapshot?.(result.pages.at(-1)?.page_id);
    error.details = { stage: smokeStage, ...error.details };
    failure = error;
  } finally {
    page.off("response", captureRoute);
    await stopInputObservation(Boolean(failure));
    try {
      result.close = await (operatorClosePromise || closeControlledBrowserWindow(page, appFrame, windowIdentity, token, baseline,
        { expectedPageId: failure ? null : result.pages.at(-1)?.page_id }));
    } catch (error) {
      if (failure) failure.details = { ...failure.details, cleanup_error: error.message, cleanup_details: error.details };
      else failure = error;
    }
  }
  if (failure) {
    failure.details = { ...failure.details, controlled_journey: result };
    throw failure;
  }
  if (fixture.profile) { result.profile.complete = true; result.profile.completed_at = Date.now(); }
  return { page_id: result.pages.at(-1).page_id, display_mode: BROWSER_OPEN_DISPLAY_MODE, controlled_journey: result };
}

async function checkBrowserEmbeddedUiInput(page, baselineToken) {
  let baseline = null;
  if (baselineToken) {
    // Home restoration is already permitted here. This is an entry observation,
    // not an empty pre-launch state; controlled cleanup independently requires 0.
    const apiFrame = await homeGuiFrameForPage(page);
    const summary = await browserApi(apiFrame, baselineToken, "/api/apps/browser/summary");
    assert(summary.ok && summary.body?.sessions?.schema === "elastos.browser.session-capacity/v1",
      "Controlled Browser baseline summary failed", summary);
    baseline = summary.body.sessions;
  }
  const webrtcSignals = [];
  const openFailures = [];
  const captureOpenFailure = async response => {
    const request = response.request();
    if (!/^\/api\/apps\/browser\/open(?:\/[^/]+)?$/.test(new URL(response.url()).pathname)) return;
    const evidence = await browserOpenResponseEvidence(response);
    const { body } = evidence;
    if ((request.method() === "POST" && !response.ok()) ||
      (body?.schema === "elastos.browser.open-status/v1" && body.status === "failed")) {
      openFailures.push({ frame: request.frame(), open_id: body?.open_id || "",
        at: Date.now(), status: response.status(), ...evidence });
      if (openFailures.length > 8) openFailures.shift();
    }
  };
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
  page.on("response", captureOpenFailure);
  markStage("browser:home-launch");
  let appFrame = null;
  let windowLocator = null;
  let windowIdentity = null;
  let browserToken = "";
  let pageId = "";
  let controlledAttemptStarted = false;
  let primaryFailure = null;
  try {
    appFrame = await openDesktopAppWindow(page, "browser", selected => { appFrame = selected; });
    windowIdentity = await captureBrowserWindowIdentity(appFrame);
    windowLocator = windowIdentity.locator;
    browserToken = windowIdentity.token;
    await focusCapturedBrowserWindow(windowIdentity, appFrame, browserToken);
    await windowLocator.waitFor({ state: "visible", timeout: 30_000 });
    await windowIdentity.parent.waitForFunction(node => {
      return node.isConnected && node.classList.contains("window-active") &&
        getComputedStyle(node.querySelector(".window-frame")).pointerEvents === "auto";
    }, windowIdentity.section, { timeout: 10_000 });
    if (CHECK_BROWSER_CONTROLLED_JOURNEY) {
      controlledAttemptStarted = true;
      return await runControlledBrowserJourney(page, appFrame, windowIdentity, browserToken, baseline, openFailures);
    }
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
    markStage("browser:page-acquisition");
    pageId = await waitForEmbeddedBrowserPage(appFrame, openFailures).catch(async error => {
      error.details = { ...error.details, ui: await embeddedBrowserDebugState(windowLocator, appFrame) };
      throw error;
    });
    assert(pageId, "Embedded Browser UI did not publish the current page id");
    const panelBox = await appFrame.locator("#browser-render-panel").boundingBox();
    assert(panelBox && panelBox.width > 0 && panelBox.height > 0, "Embedded Browser render panel has no clickable box", panelBox);
    const clickX = Math.max(1, Math.min(panelBox.width - 1, BROWSER_INPUT_CLICK_X));
    const clickY = Math.max(1, Math.min(panelBox.height - 1, BROWSER_INPUT_CLICK_Y));
    const initialGeometry = await embeddedBrowserGeometry(windowLocator, appFrame);
    assertEmbeddedBrowserAspect(initialGeometry);
    if (BROWSER_OPEN_DISPLAY_MODE === "webrtc_remote_display") {
      const initialStatus = await checkBrowserPageStatus(appFrame, browserToken, pageId);
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
          appFrame,
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
            appFrame,
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
        const status = await checkBrowserPageStatus(appFrame, browserToken, pageId);
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
        appFrame,
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
        appFrame,
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
  } catch (error) {
    primaryFailure = error;
    console.error(JSON.stringify(redactSensitive({
      stage: smokeStage,
      event: "journey-failed-before-cleanup",
      error: error.message,
      stack: error.stack,
    })));
    throw error;
  } finally {
    page.off("response", captureWebrtcResponse);
    if (primaryFailure && !controlledAttemptStarted) {
      try {
        if (!windowIdentity) {
          assert(appFrame, "Startup cleanup requires the exact opened Browser frame");
          windowIdentity = await captureBrowserWindowIdentity(appFrame);
          browserToken = windowIdentity.token;
        }
        const cleanup = await closeControlledBrowserWindow(page, appFrame, windowIdentity, browserToken, baseline);
        primaryFailure.details = { ...primaryFailure.details, startup_cleanup: cleanup };
      } catch (error) {
        primaryFailure.details = { ...primaryFailure.details, cleanup_error: error.message, cleanup_details: error.details };
      }
    }
    page.off("response", captureOpenFailure);
    if (!CHECK_BROWSER_CONTROLLED_JOURNEY && !primaryFailure && pageId && browserToken) {
      const closed = await browserApi(
        appFrame,
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
    const response = await fetch("/api/auth/passkey/status", { signal: AbortSignal.timeout(30_000) });
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

async function createPasskeyFromCurrentUnlock(page, mode, onCreated) {
  const name = page.locator("#home-unlock-name");
  await name.waitFor({ state: "visible", timeout: 10_000 });
  await name.fill(TEST_NAME);
  const tokenPromise = captureNextPasskeyToken(page);
  await page.locator("#home-unlock-primary").click();
  const created = { created: true, mode, homeToken: await tokenPromise };
  // Registration can succeed even when the next shell-readiness check fails.
  if (onCreated) await onCreated(created);
  await waitForSignedHome(page);
  return created;
}

async function ensureSignedWithVirtualPasskey(page, onCreated) {
  await waitForHomeReady(page);
  let state = await homeState(page);
  if (state.authority === "signed") {
    if (REUSE_SIGNED_HOME) {
      assert(CHECK_BROWSER_CONTROLLED_JOURNEY, "Signed Home reuse requires a controlled Browser journey");
      const refreshed = await refreshCurrentHomeToken(page);
      assert(refreshed.ok && refreshed.homeToken, "Signed Home session refresh failed");
      await waitForSignedHome(page);
      return { created: false, mode: "reused-signed-home", homeToken: refreshed.homeToken };
    }
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
    const created = await createPasskeyFromCurrentUnlock(page, "admin", onCreated);
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
  const created = await createPasskeyFromCurrentUnlock(page, "guest", onCreated);
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
      signal: AbortSignal.timeout(30_000),
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
      signal: AbortSignal.timeout(30_000),
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
  await assertSignedOutShell(page, state);
  const clickTokenPromise = captureNextPasskeyToken(page).catch(() => null);
  await page.locator("#home-unlock-person").click();
  await waitForSignedHome(page);
  const token = await settleTokenWithin(clickTokenPromise, 1_000)
    || await settleTokenWithin(tokenPromise, 1_000);
  assert(token, "manual virtual passkey sign-in completed without a captured Home token", await homeState(page));
  return token;
}

async function checkHomePublicCopy(page) {
  await waitForSignedHome(page);
  const homeGuiFrame = await waitForCapsuleFrame(page, "home-gui");
  await homeGuiFrame.waitForFunction(
    () => document.body?.dataset.homeStatus === "ready",
    null,
    { timeout: 15_000 },
  );
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
  assert(state.text.length > 0, "Home GUI copy check needs rendered content", state);
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

async function homeGuiFrameForPage(page) {
  return waitForCapsuleFrame(page, "home-gui");
}

async function openDesktopAppWindow(page, target, onFrame = null) {
  const current = new URL(page.url());
  const home = new URL(HOME_URL);
  if (current.origin !== home.origin || current.pathname !== home.pathname) {
    await page.goto(HOME_URL, { waitUntil: "domcontentloaded" });
  }
  await waitForSignedHome(page);
  const homeGuiFrame = await waitForCapsuleFrame(page, "home-gui");
  await homeGuiFrame.waitForFunction(
    () => document.body?.dataset.homeStatus === "ready",
    null,
    { timeout: 20_000 },
  );
  // This smoke checks app entry. Recovery Kit and Profile completion have
  // their own journey proof; use the visible reminder's close control here.
  const setupReminder = homeGuiFrame.locator("#setup-sheet");
  if (await setupReminder.isVisible()) {
    await homeGuiFrame.locator("#setup-sheet-close").click();
    await setupReminder.waitFor({ state: "hidden", timeout: 5_000 });
  }
  if (target === "browser") {
    // Match Home's foreground choice once. DOM order can differ from z-order,
    // and a later restored window must not replace the chosen frame.
    const selected = (await homeGuiFrame.evaluateHandle(() => {
      const windows = [...document.querySelectorAll('section.window[data-target="browser"]')];
      const active = windows.filter(node => node.classList.contains("window-active"));
      if (active.length > 1) throw new Error("Home has ambiguous active Browser windows");
      const visible = windows.filter(node => !node.classList.contains("hidden"));
      return active[0] || (visible.length ? visible : windows)
        .sort((a, b) => Number(b.style.zIndex || 0) - Number(a.style.zIndex || 0))[0] || null;
    })).asElement();
    let handle;
    if (selected) {
      handle = await selected.$("iframe.window-frame");
    } else {
      const shelf = homeGuiFrame.locator('#taskbar-targets [data-target="browser"]').first();
      if (await shelf.isVisible()) await shelf.click();
      else {
        await homeGuiFrame.locator("#launcher-toggle").click();
        await homeGuiFrame.locator('#launcher-grid [data-target="browser"]').first().click();
      }
      // There was no Browser at selection. Ambiguous concurrent creations fail
      // the strict locator instead of assigning another window's authority.
      const created = homeGuiFrame.locator('section.window[data-target="browser"] iframe.window-frame');
      await created.waitFor({ state: "attached", timeout: 20_000 });
      handle = await created.elementHandle();
    }
    const appFrame = handle ? await handle.contentFrame() : null;
    assert(appFrame, "Selected Home Browser window has no content frame");
    if (onFrame) await onFrame(appFrame);
    const deadline = Date.now() + 20_000;
    while (Date.now() < deadline && !appFrame.url().includes("/apps/browser/")) await delay(100);
    assert(appFrame.url().includes("/apps/browser/"), "Selected Home Browser window did not load its capsule document");
    const identity = await captureBrowserWindowIdentity(appFrame);
    await focusCapturedBrowserWindow(identity, appFrame, identity.token);
    await handle.waitForElementState("visible", { timeout: 20_000 });
    return appFrame;
  }
  const shelfItem = homeGuiFrame.locator(`#taskbar-targets [data-target="${target}"]`).first();
  const existingWindow = homeGuiFrame.locator(`section.window[data-target="${target}"]`).last();
  const alreadyActive = await existingWindow.isVisible() &&
    await existingWindow.evaluate(node => node.classList.contains("window-active"));
  // A Shelf click minimizes an active window. Its title can also be hidden in
  // fullscreen, so reuse active content without clicking the hidden chrome.
  if (alreadyActive) {
    // The requested window is already the active user surface.
  } else if (target === "system") {
    await homeGuiFrame.locator("#toolbar-home").click();
    await homeGuiFrame.locator("#identity-menu-system").click();
  } else if (await shelfItem.isVisible()) {
    await shelfItem.click();
  } else {
    await homeGuiFrame.locator("#launcher-toggle").click();
    const card = homeGuiFrame.locator(`#launcher-grid [data-target="${target}"]`).first();
    await card.waitFor({ state: "visible", timeout: 10_000 });
    await card.click();
  }
  // The desktop restores persisted windows at boot and restore can steal
  // focus from the window the launcher just opened, so bind to the newest
  // window for the target rather than whichever one holds the active class.
  const windowFrameEl = homeGuiFrame
    .locator(`section.window[data-target="${target}"] iframe.window-frame`)
    .last();
  await windowFrameEl.waitFor({ state: onFrame ? "attached" : "visible", timeout: 20_000 });
  const handle = await windowFrameEl.elementHandle();
  const appFrame = handle ? await handle.contentFrame() : null;
  assert(appFrame, `desktop window for ${target} had no content frame`, { target });
  if (onFrame) {
    // Preserve this selected frame for exact cleanup if a later launch wait fails.
    await onFrame(appFrame);
    await handle.waitForElementState("visible", { timeout: 20_000 });
  }
  const deadline = Date.now() + 20_000;
  while (Date.now() < deadline && !appFrame.url().includes(`/apps/${target}/`)) {
    await delay(100);
  }
  assert(
    appFrame.url().includes(`/apps/${target}/`),
    `desktop window for ${target} never loaded its capsule document`,
    { target, url: appFrame.url() },
  );
  return appFrame;
}

async function launchSystem(page, homeToken, passkey) {
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
  // Capsule documents only accept API calls from their sandboxed (opaque
  // origin) window frames, so open System through the ElastOS menu the
  // way a person does instead of navigating the trusted Home page to it.
  const systemFrame = await openDesktopAppWindow(page, "system");
  await systemFrame.locator(".settings-container").waitFor({ state: "visible", timeout: 20_000 });
  const system = await systemFrame.evaluate(() => ({
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
  const recoveryExport = CHECK_RECOVERY_EXPORT
    ? await checkSystemRecoveryExport(page, systemFrame, passkey)
    : null;
  return { ...system, recoveryExport };
}

async function readRecoveryExportDownload(download, expected) {
  // Read only in memory, bound the allocation, and give parse failures a fixed
  // message: JSON syntax errors can otherwise quote recovery key material.
  const stream = await download.createReadStream();
  assert(stream, "Recovery Kit download did not provide a readable stream");
  const chunks = [];
  let bytes = 0;
  for await (const chunk of stream) {
    bytes += chunk.length;
    assert(bytes <= 8 * 1024 * 1024, "Recovery Kit download exceeds the smoke limit");
    chunks.push(chunk);
  }
  let bundle;
  try {
    bundle = JSON.parse(Buffer.concat(chunks).toString("utf8"));
  } catch {
    throw new Error("Recovery Kit download is not valid JSON");
  }
  assert(bundle?.schema === "elastos.full-recovery-bundle/v1", "Recovery Kit download has the wrong schema");
  assert(typeof expected.principal_id === "string" && expected.principal_id.length > 0
    && typeof expected.localhost_root === "string" && expected.localhost_root.startsWith("localhost://")
    && bundle.principal_id === expected.principal_id && bundle.localhost_root === expected.localhost_root,
  "Recovery Kit download has the wrong principal binding");
  assert(bundle.included?.data_kit === true && bundle.data_kit?.schema === "elastos.recovery-kit/v1"
    && bundle.data_kit.principal_id === expected.principal_id
    && bundle.data_kit.localhost_root === expected.localhost_root,
  "Recovery Kit download is missing its bound data kit");
  const identity = bundle.people_identity;
  const profile = identity?.profile_authority_bundle;
  assert(bundle.included?.people_identity === true
    && identity?.schema === "elastos.people.recovery-identity/v1"
    && profile?.schema === "elastos.profile-authority-bundle/v1"
    && /^[0-9a-f]{64}$/.test(profile.profile_signing_seed_hex || "")
    && typeof profile.signed_profile?.payload?.profile_did === "string"
    && profile.signed_profile.payload.profile_did.startsWith("did:key:z"),
  "Recovery Kit download is missing its Profile authority");
  // Keep the bundle, key material, and download path out of the report.
  return { bytes, profile_included: true, principal_binding_checked: true };
}

async function checkSystemRecoveryExport(page, systemFrame, passkey) {
  markStage("recovery-export:system-ui");
  assert(passkey?.principal_id, "Recovery Kit export requires the recorded signed-in principal");
  // Static HTML is visible before System binds navigation and loads state.
  // Wait for the recovery note to be populated even while its tab is hidden.
  await systemFrame.waitForFunction(() => {
    const note = document.querySelector('[data-field="recovery-note"]');
    return note && !note.hidden && note.textContent.trim().length > 0;
  }, null, { timeout: 30_000 });
  // Restored windows can finish opening after the menu launch. Interact with
  // the foreground System window, rather than a covered earlier instance.
  const desktop = await homeGuiFrameForPage(page);
  const activeSystem = desktop.locator('section.window.window-active[data-target="system"] iframe.window-frame');
  await activeSystem.waitFor({ state: "visible", timeout: 10_000 });
  const activeHandle = await activeSystem.elementHandle();
  systemFrame = await activeHandle.contentFrame();
  assert(systemFrame, "Foreground System window has no frame");
  await systemFrame.waitForFunction(() => {
    const note = document.querySelector('[data-field="recovery-note"]');
    return note && !note.hidden && note.textContent.trim().length > 0;
  }, null, { timeout: 30_000 });
  await systemFrame.locator('.settings-sidebar-item[data-settings="security"]').click();
  await systemFrame.locator('[data-field="recovery-note"]').waitFor({ state: "visible", timeout: 10_000 });
  await systemFrame.locator('#recovery-password').fill("");
  const profileName = systemFrame.locator('#recovery-profile-name');
  if (await profileName.isVisible()) {
    await profileName.fill(TEST_NAME);
  }
  let download = null;
  const responseAt = (path, method) => response => response.request().method() === method
    && new URL(response.url()).origin === new URL(HOME_URL).origin
    && new URL(response.url()).pathname === path;
  const responses = [];
  const messages = [];
  const captureResponse = response => {
    const url = new URL(response.url());
    if (url.origin === new URL(HOME_URL).origin
      && /^\/api\/auth\/(recovery|passkey-step-up)\//.test(url.pathname)) {
      responses.push({ path: url.pathname, status: response.status() });
    }
  };
  const captureConsole = message => {
    if (["warning", "error"].includes(message.type())) {
      messages.push(redactSensitiveString(message.text()).slice(0, 1000));
      if (messages.length > 20) messages.shift();
    }
  };
  page.on("response", captureResponse);
  page.on("console", captureConsole);
  try {
    markStage("recovery-export:download-and-step-up");
    const [status, stepUp] = await Promise.all([
      page.waitForResponse(responseAt("/api/auth/recovery/status", "GET"), { timeout: 120_000 }),
      page.waitForResponse(responseAt("/api/auth/passkey-step-up/complete", "POST"), { timeout: 120_000 }),
      page.waitForEvent("download", { timeout: 120_000 }).then(value => { download = value; }),
      systemFrame.locator('#recovery-download').click(),
    ]);
    assert(status.ok() && stepUp.ok(), "System Recovery export did not complete its status and passkey step-up");
    let expected;
    try {
      expected = await status.json();
    } catch {
      throw new Error("System Recovery status is not valid JSON");
    }
    assert(expected?.principal_id === passkey.principal_id, "System Recovery status changed the signed-in principal");
    const result = await readRecoveryExportDownload(download, expected);
    return { ...result, virtual_step_up_checked: true, download_deleted: true };
  } catch (error) {
    const ui = await systemFrame.evaluate(() => ({
      status: document.querySelector('[data-field="recovery-status"]')?.textContent,
      note: document.querySelector('[data-field="recovery-note"]')?.textContent,
      downloadDisabled: document.querySelector('#recovery-download')?.disabled,
    })).catch(() => ({ unavailable: true }));
    error.details = redactSensitive({ stage: smokeStage, responses, messages, ui });
    throw error;
  } finally {
    page.off("response", captureResponse);
    page.off("console", captureConsole);
    // Browser download storage has a separate lifecycle from a retained
    // virtual-authenticator profile. Remove the exported secrets on failure too.
    if (download) await download.delete();
  }
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
    const systemFrame = await openDesktopAppWindow(page, "system");
    await systemFrame.locator(".settings-container").waitFor({ state: "visible", timeout: 20_000 });

    markStage("shell-switch:open-system-shell");
    // The window chrome's edge hit-zones can overlap the System sidebar
    // depending on where the desktop placed the window, so drive System's
    // own controls with DOM clicks; pointer fidelity is proven at the
    // launcher and window layers above.
    await systemFrame
      .locator('button.settings-sidebar-item[data-settings="shell"]')
      .waitFor({ state: "visible", timeout: 15_000 });
    await systemFrame.evaluate(() => {
      document.querySelector('button.settings-sidebar-item[data-settings="shell"]').click();
    });
    await systemFrame.locator("#active-shell-options").waitFor({ state: "attached", timeout: 15_000 });
    await systemFrame.waitForFunction(() => {
      const names = [...document.querySelectorAll("#active-shell-options [data-shell-name]")]
        .map((button) => button.dataset.shellName);
      return names.includes("home-gui") && names.includes("home-cli");
    }, null, { timeout: 15_000 });
    const shellOptions = await systemFrame.evaluate(() => (
      [...document.querySelectorAll("#active-shell-options [data-shell-name]")].map((button) => ({
        value: button.dataset.shellName,
        label: button.textContent?.trim() || "",
      }))
    ));

    const switchToCli = page.waitForResponse((response) => (
      response.request().method() === "POST" &&
      response.url().endsWith("/api/apps/home/active-shell")
    ), { timeout: 30_000 });
    // A rejection must wait for the click below to settle, not crash the run.
    switchToCli.catch(() => {});
    markStage("shell-switch:system-post-home-cli");
    await systemFrame
      .locator('#active-shell-options [data-shell-name="home-cli"]')
      .waitFor({ state: "attached", timeout: 15_000 });
    await systemFrame.evaluate(() => {
      document.querySelector('#active-shell-options [data-shell-name="home-cli"]').click();
    });
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
      // window.origin is the effective origin — "null" inside the opaque
      // sandbox — while location.origin would echo the Home URL either way.
      origin: window.origin,
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
        // Effective origin: "null" inside the opaque sandbox.
        origin: window.origin,
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

async function checkBrowserViewerPreflight(page) {
  let openRequests = 0;
  const observeOpen = (request) => {
    if (request.method() === "POST" && new URL(request.url()).pathname === "/api/apps/browser/open") {
      openRequests += 1;
    }
  };
  page.on("request", observeOpen);
  try {
    await page.addInitScript(() => {
      Object.defineProperty(window, "RTCPeerConnection", { value: undefined, configurable: true });
    });
    const appFrame = await openDesktopAppWindow(page, "browser");
    const browserToken = assertIsolatedLaunchRoute(appFrame.url(), "browser");
    const message = "This browser cannot show the Browser session. Use a supported browser or enable WebRTC.";
    await appFrame.getByText(message, { exact: true }).waitFor({ state: "visible", timeout: 15_000 });
    assert(openRequests === 0, "Unsupported viewer dispatched an Engine open", { openRequests });
    const summary = await browserApi(appFrame, browserToken, "/api/apps/browser/summary");
    assert(summary.ok, "Viewer preflight summary failed", summary);
    assert(
      summary.body?.sessions?.schema === "elastos.browser.session-capacity/v1" &&
        summary.body.sessions.principal_sessions === 0,
      "Fresh viewer preflight fixture retained Runtime sessions",
      summary.body?.sessions,
    );
    const gui = await homeGuiFrameForPage(page);
    const window = gui.locator('section.window[data-target="browser"]').last();
    await window.getByRole("button", { name: "Close", exact: true }).click();
    await window.waitFor({ state: "hidden", timeout: 30_000 });
    return {
      target: "browser",
      viewer_preflight: { unsupported_viewer_reported: true, open_requests: openRequests, principal_sessions: 0, window_closed: true },
      browser_summary: { sessions: summary.body.sessions, engine_adapter: summary.body.engine_adapter },
    };
  } finally {
    page.off("request", observeOpen);
  }
}

async function checkBrowserLaunchGrant(page, homeToken) {
  assert(homeToken, "checkBrowserLaunchGrant requires a passkey-issued Home token");
  if (CHECK_BROWSER_VIEWER_PREFLIGHT) {
    assert(!OPEN_BROWSER && !CHECK_BROWSER_UI_SETUP && !CHECK_BROWSER_UI_INPUT && !CHECK_BROWSER_EMBEDDED_UI_INPUT,
      "Viewer rejection proof runs separately from Engine/media tests");
    return checkBrowserViewerPreflight(page);
  }
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
    launched.body.browser_embedded_ui_input = await checkBrowserEmbeddedUiInput(page, browserToken);
    if (CHECK_BROWSER_CONTROLLED_JOURNEY) {
      console.error(JSON.stringify(redactSensitive({
        stage: "browser:controlled-journey-result",
        result: launched.body.browser_embedded_ui_input,
      })));
    }
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
    const summaryFrame = CHECK_BROWSER_CONTROLLED_JOURNEY ? await homeGuiFrameForPage(page) : page;
    const summary = await browserApi(summaryFrame, browserToken, "/api/apps/browser/summary");
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
    // Capsule documents demand their sandboxed (opaque origin) window frame,
    // so open each app through the desktop launcher like a person would and
    // judge the window's rendered document, not a bare top-level navigation.
    const appFrame = await openDesktopAppWindow(page, target);
    const appState = await appFrame.evaluate(() => ({
      title: document.title,
      bodyStatus: document.body?.dataset?.status || document.body?.dataset?.appStatus || "",
      renderedNodes: document.body ? document.body.querySelectorAll("*").length : 0,
      bodyTextLength: (document.body?.innerText || "").trim().length,
      visibleError: [...document.querySelectorAll("[role='alert'], .error, .system-error")]
        .map((node) => node.textContent?.trim() || "")
        .filter(Boolean)
        .slice(0, 3),
    }));
    assert(
      !appState.visibleError.some((text) => /failed to open|access denied|invalid home launch token/i.test(text)),
      `App window rendered an authority error for ${target}`,
      { target, route, appState },
    );
    assert(
      appState.renderedNodes > 0,
      `App window stayed empty for ${target}`,
      { target, route, appState },
    );
    // Markup is not paint. A window whose frame never becomes visible, or
    // reveals only by timeout, still fails the product.
    const gui = await homeGuiFrameForPage(page);
    const readFramePaint = () => gui.evaluate((appTarget) => {
      const section = [...document.querySelectorAll(`section.window[data-target="${appTarget}"]`)].pop();
      const frame = section?.querySelector("iframe.window-frame");
      if (!frame) {
        return { found: false };
      }
      const style = window.getComputedStyle(frame);
      const rect = frame.getBoundingClientRect();
      let ancestorVisible = true;
      for (let node = frame; node; node = node.parentElement) {
        const nodeStyle = window.getComputedStyle(node);
        if (
          nodeStyle.display === "none" ||
          nodeStyle.visibility === "hidden" ||
          Number(nodeStyle.opacity || "1") === 0
        ) {
          ancestorVisible = false;
          break;
        }
      }
      const probeX = rect.left + rect.width / 2;
      const probeY = rect.top + rect.height / 2;
      const topElement = document.elementFromPoint(probeX, probeY);
      const topWindow = topElement?.closest?.("section.window") || null;
      return {
        found: true,
        opacity: Number(style.opacity),
        visibility: style.visibility,
        display: style.display,
        width: Math.round(rect.width),
        height: Math.round(rect.height),
        revealCause: frame.dataset.frameVisibleCause || "",
        ancestorVisible,
        pointVisible: topElement === frame || topWindow === section,
      };
    }, target);
    const framePainted = (state) => Boolean(
      state.found
        && state.opacity > 0
        && state.visibility === "visible"
        && state.display !== "none"
        && state.width > 0
        && state.height > 0
        && state.revealCause === "load"
        && state.ancestorVisible === true
        && state.pointVisible === true,
    );
    let framePaint = await readFramePaint();
    const paintDeadline = Date.now() + 8_000;
    while (Date.now() < paintDeadline && !framePainted(framePaint)) {
      await delay(150);
      framePaint = await readFramePaint();
    }
    assert(
      framePainted(framePaint),
      `App window frame never became visible for ${target}`,
      { target, route, framePaint },
    );
    results.push({
      target,
      title: summaryTarget.title || "",
      route_prefix: route.split("#")[0],
      document_title: appState.title,
      body_status: appState.bodyStatus,
      rendered_nodes: appState.renderedNodes,
      body_text_length: appState.bodyTextLength,
      frame_opacity: framePaint.opacity,
    });
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
  assert(!BROWSER_CONTROLLED_TURN_TEST_HOME || CHECK_BROWSER_CONTROLLED_RECOVERY,
    "The task TURN interruption requires controlled Browser recovery");
  assert(!CHECK_BROWSER_CONTROLLED_MEDIA || CHECK_BROWSER_CONTROLLED_JOURNEY,
    "Controlled audio proof requires the controlled Browser journey");
  assert(!BROWSER_QUALIFICATION_OPTIONS || (CHECK_BROWSER_CONTROLLED_JOURNEY && CHECK_BROWSER_CONTROLLED_MEDIA &&
    CHECK_BROWSER_CONTROLLED_INSPECTION && CHECK_BROWSER_CONTROLLED_OPERATOR && CHECK_BROWSER_VIEWER_RELOAD &&
    REQUIRE_BROWSER_VZ_TRANSPORT), "Qualification requires the full controlled Mac operator/media/reload journey");
  assert(!CHECK_BROWSER_CONTROLLED_INSPECTION || CHECK_BROWSER_CONTROLLED_JOURNEY,
    "Controlled inspection requires the controlled Browser journey");
  assert(!CHECK_BROWSER_CONTROLLED_OPERATOR || (CHECK_BROWSER_CONTROLLED_JOURNEY && CHECK_BROWSER_CONTROLLED_INSPECTION &&
    BROWSER_OPERATOR_COORDS_PATH && existsSync(BROWSER_OPERATOR_COORDS_PATH)),
  "Controlled operator proof requires the controlled inspection journey and explicit Runtime coordinates");
  assert(!CHECK_BROWSER_VIEWER_RELOAD || CHECK_BROWSER_CONTROLLED_JOURNEY,
    "Browser viewer reload requires the controlled journey");
  assert(!CHECK_BROWSER_CONTROLLED_RECOVERY || CHECK_BROWSER_CONTROLLED_JOURNEY,
    "Controlled Browser recovery requires the controlled journey");
  assert(!CHECK_BROWSER_CONTROLLED_JOURNEY || (INCLUDE_BROWSER && CHECK_BROWSER_EMBEDDED_UI_INPUT &&
    !OPEN_BROWSER && !CHECK_BROWSER_UI_INPUT && !CHECK_BROWSER_UI_SETUP && !CHECK_BROWSER_VIEWER_PREFLIGHT),
  "Controlled journey requires BROWSER=1 and BROWSER_EMBEDDED_UI_INPUT=1, with other Browser runs disabled");
  if (CHECK_BROWSER_CONTROLLED_JOURNEY) {
    markStage("browser:fixture-preflight");
    await readBrowserJourneyHealth(BROWSER_JOURNEY_TARGET);
  }
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

  if (CHECK_BROWSER_CONTROLLED_JOURNEY) markStage("home:viewer-launch");
  const contextPromise = chromium.launchPersistentContext(PROFILE_DIR, {
    headless: HEADLESS,
    executablePath: process.env.ELASTOS_BROWSER_EXECUTABLE || undefined,
    ignoreHTTPSErrors: true,
    viewport: { width: 1280, height: 900 },
  });
  const context = qualificationCancellation ? await qualificationCancellation.ownContext(contextPromise) : await contextPromise;
  if (CHECK_BROWSER_CONTROLLED_MEDIA) await context.addInitScript(installBrowserJourneyAudioProbe);
  let page = context.pages()[0] || await context.newPage();
  let created = null;
  let passkey = null;
  let cleanupResult = null;
  let homeToken = "";
  let cleanupAttempted = false;
  let virtualAuthenticator = null;
  let credentialStore = { skipped: true };
  let failure = null;
  async function cleanupCreatedPasskey() {
    if (
      cleanupAttempted
      || !created?.created
      || !CLEANUP_PASSKEY
    ) {
      return cleanupResult || { skipped: !created?.created || !CLEANUP_PASSKEY };
    }
    cleanupAttempted = true;
    assert(passkey?.proof_binding_id, "Test passkey cleanup needs its recorded proof binding");
    // Sign-out invalidates the earlier Home token. Recover authority, then
    // check ownership before revoking the one passkey created by this run.
    const refreshed = await refreshCurrentHomeToken(page);
    const cleanupToken = refreshed.ok && refreshed.homeToken
      ? refreshed.homeToken : await signBackIn(page);
    const authenticated = await currentPasskey(page, cleanupToken);
    assert(
      authenticated?.proof_binding_id === passkey.proof_binding_id,
      "Cleanup authentication selected a different passkey; test credential retained",
      { expected: passkey.proof_binding_id, current: authenticated?.proof_binding_id },
    );
    cleanupResult = await revokeCurrentPasskey(page, passkey.proof_binding_id, cleanupToken);
    return cleanupResult;
  }
  try {
    browserQualification = await createQualificationHarness(context, page, BROWSER_QUALIFICATION_OPTIONS, undefined, qualificationCancellation);
    if (CHECK_BROWSER_CONTROLLED_JOURNEY) markStage("home:virtual-authenticator");
    virtualAuthenticator = await setupVirtualAuthenticator(context, page);
    if (CHECK_BROWSER_CONTROLLED_JOURNEY) markStage("home:navigate");
    await page.goto(HOME_URL, { waitUntil: "domcontentloaded" });
    if (CHECK_BROWSER_CONTROLLED_JOURNEY) markStage("home:sign-in");
    created = await ensureSignedWithVirtualPasskey(page, async (registered) => {
      created = registered;
      homeToken = registered.homeToken;
      credentialStore = await persistVirtualAuthenticatorCredentials(virtualAuthenticator);
      passkey = await currentPasskey(page, homeToken);
    });
    homeToken = created.homeToken;
    if (CHECK_BROWSER_CONTROLLED_JOURNEY) markStage("home:passkey-read");
    passkey = await currentPasskey(page, homeToken);
    assert(passkey?.proof_binding_id, "signed virtual passkey was not visible through the passkey list", passkey);
    credentialStore = await persistVirtualAuthenticatorCredentials(virtualAuthenticator);

    assert(!REUSE_SIGNED_HOME || CHECK_BROWSER_CONTROLLED_JOURNEY,
      "Signed Home reuse requires a controlled Browser journey");
    if (!REUSE_SIGNED_HOME) {
      if (CHECK_BROWSER_CONTROLLED_JOURNEY) markStage("home:sign-out");
      await signOut(page, homeToken);
      if (CHECK_BROWSER_CONTROLLED_JOURNEY) markStage("home:sign-in-again");
      homeToken = await signBackIn(page);
    }
    if (CHECK_BROWSER_CONTROLLED_JOURNEY) markStage("home:credential-store");
    const afterSignIn = await currentPasskey(page, homeToken);
    assert(
      afterSignIn?.proof_binding_id === passkey.proof_binding_id,
      "virtual passkey sign-in did not restore the same proof binding",
      { before: passkey, after: afterSignIn },
    );
    credentialStore = await persistVirtualAuthenticatorCredentials(virtualAuthenticator);

    if (CHECK_BROWSER_CONTROLLED_JOURNEY) markStage("home:public-copy");
    const homePublicCopy = await checkHomePublicCopy(page);
    const system = CHECK_SYSTEM ? await launchSystem(page, homeToken, passkey) : null;
    const shellSwitch = CHECK_SHELL_SWITCH
      ? await checkShellSwitchJourney(page, homeToken)
      : null;
    const browserLaunch = INCLUDE_BROWSER ? await checkBrowserLaunchGrant(page, homeToken) : null;
    const appMatrix = CHECK_APP_MATRIX ? await checkAppLaunchMatrix(page, homeToken) : null;

    if (created.created && CLEANUP_PASSKEY) {
      cleanupResult = await cleanupCreatedPasskey();
      assert(cleanupResult.ok, "virtual test passkey cleanup failed", cleanupResult);
    }

    const report = {
      schema: "elastos.home.passkey-virtual-auth-smoke/v1",
      ok: true,
      viewer: {
        version: context.browser()?.version() || null,
        executable_override: Boolean(process.env.ELASTOS_BROWSER_EXECUTABLE),
        headed: !HEADLESS,
      },
      home_url: HOME_URL,
      profile_dir: PROFILE_DIR,
      created_mode: created.mode,
      sign_in_out_round_trip: !REUSE_SIGNED_HOME,
      proof_binding_id: passkey.proof_binding_id,
      principal_id: passkey.principal_id,
      role: passkey.role,
      virtual_authenticator_credentials: credentialStore,
      first_run_setup_checked: false,
      system_checked: Boolean(system),
      recovery_export_checked: Boolean(system?.recoveryExport),
      recovery_export: system?.recoveryExport ?? null,
      system_fields: system?.fields || null,
      home_public_copy: homePublicCopy,
      shell_switch: shellSwitch,
      browser_launch_checked: Boolean(browserLaunch),
      browser_viewer_preflight: browserLaunch?.viewer_preflight || null,
      browser_summary: browserLaunch?.browser_summary || null,
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
    failure = { message: String(error.message || error), stage: smokeStage };
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
        console.error(JSON.stringify(redactSensitive(cleanup), null, 2));
      }
    } catch (cleanupError) {
      cleanupResult = { ok: false, error: String(cleanupError.message || cleanupError) };
      console.error("virtual test passkey cleanup threw after smoke error");
      console.error(redactSensitiveString(cleanupError.message || cleanupError));
    }
    console.error("FAIL home-passkey-virtual-auth-smoke");
    console.error(redactSensitiveString(error.message || error));
    if (error.stack) {
      console.error(redactSensitiveString(error.stack));
    }
    if (error.details) {
      console.error(JSON.stringify(redactSensitive(error.details), null, 2));
    } else {
      const state = page ? await homeState(page).catch(() => null) : null;
      if (state) {
        state.stage = smokeStage;
        console.error(JSON.stringify(redactSensitive(state), null, 2));
      }
    }
    process.exitCode = 1;
  } finally {
    await browserQualification?.stop();
    // A virtual authenticator's private key lives in CDP memory, not in the
    // browser profile. Export it before closing, including failed registration.
    let credentialSaveError = null;
    try {
      credentialStore = await persistVirtualAuthenticatorCredentials(virtualAuthenticator);
    } catch (error) {
      credentialSaveError = String(error.message || error);
    }
    const retainForRecovery = cleanupResult?.ok !== true && (
      created?.created || (credentialStore.credential_count || 0) > 0 || credentialSaveError
    );
    if (retainForRecovery) {
      const recovery = {
        schema: "elastos.home.virtual-authenticator-recovery/v1",
        recorded_at: new Date().toISOString(),
        home_url: HOME_URL,
        profile_dir: PROFILE_DIR,
        credential_store: VIRTUAL_AUTH_CREDENTIAL_STORE,
        virtual_authenticator_credentials: credentialStore,
        credential_save_error: credentialSaveError,
        created_mode: created?.mode || "registration outcome unknown",
        proof_binding_id: passkey?.proof_binding_id || null,
        cleanup: cleanupResult || { skipped: !CLEANUP_PASSKEY },
        failure,
        cleanup_condition: "Keep this profile until its test passkey is revoked in Home. Set HOME_VIRTUAL_AUTH_PROFILE to this profile_dir and use the same HOME_URL to restore its authenticator.",
      };
      const recoveryPath = join(PROFILE_DIR, "elastos-virtual-authenticator-recovery.json");
      try {
        mkdirSync(PROFILE_DIR, { recursive: true, mode: 0o700 });
        writeFileSync(recoveryPath, `${JSON.stringify(recovery, null, 2)}\n`, { mode: 0o600 });
        chmodSync(recoveryPath, 0o600);
      } catch (error) {
        console.error(`Could not save recovery receipt: ${error.message || error}`);
        process.exitCode = 1;
      }
      console.error("Virtual passkey profile retained until credential cleanup completes");
      console.error(JSON.stringify(redactSensitive({ recovery_path: recoveryPath, ...recovery }), null, 2));
    }
    await context.close().catch(() => {});
    if (!retainForRecovery && !PRESERVE_PROFILE && !process.env.HOME_VIRTUAL_AUTH_PROFILE) {
      rmSync(PROFILE_DIR, { recursive: true, force: true });
    }
  }
}

try { await main(); }
finally {
  const cancellation = await qualificationCancellation?.stop();
  if (cancellation?.cancelled) console.error(JSON.stringify({ stage: "qualification:cancelled", cancellation }));
  if (BROWSER_QUALIFICATION_OPTIONS) console.error(JSON.stringify({ stage: "qualification:open-attempts", evidence: browserQualification?.snapshot() }));
}
