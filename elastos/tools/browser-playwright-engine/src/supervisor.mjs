#!/usr/bin/env node

import http from "node:http";
import net from "node:net";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { createHash, randomBytes } from "node:crypto";
import { once } from "node:events";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";
import jpeg from "jpeg-js";
import { chromium } from "playwright";
import wrtc from "@roamhq/wrtc";

const REQUEST_ENV = "ELASTOS_BROWSER_ENGINE_REQUEST";
const CONFIG_ENV = "ELASTOS_BROWSER_PLAYWRIGHT_ENGINE_CONFIG";
const DEFAULT_VIEWPORT = { width: 1280, height: 720 };
const WEBRTC_INITIAL_CANDIDATE_WAIT_MS = 1500;
const WEBRTC_MAX_BITRATE_BPS = 12_000_000;
const WEBRTC_MAX_FRAMERATE = 30;
const SCREENCAST_JPEG_QUALITY = 95;
const DISPLAY_BACKEND = "cdp_screencast_i420";
const DISPLAY_BACKEND_CLASS = "proof_surface";

const __filename = fileURLToPath(import.meta.url);
const { RTCVideoSource, rgbaToI420 } = wrtc.nonstandard;

const args = process.argv.slice(2);
if (args.includes("--daemon")) {
  runDaemon().catch((error) => {
    console.error(error.stack || error.message || String(error));
    process.exit(1);
  });
} else {
  runSupervisor().catch((error) => {
    console.error(error.stack || error.message || String(error));
    process.exit(1);
  });
}

async function runSupervisor() {
  const request = parseJsonEnv(REQUEST_ENV);
  const config = normalizeConfig(loadConfig());
  validateLaunchRequest(request);
  await ensureDaemon(config, request.display_mode);
  const result = await controlRequest(config.control_socket_path, "POST", "/launch", request);
  process.stdout.write(`${JSON.stringify(result)}\n`);
}

async function runDaemon() {
  const config = normalizeConfig(loadConfig());
  await unlinkSocket(config.control_socket_path);
  const runtimeProxy = await startRuntimeProxy(config);
  const state = {
    config,
    browser: null,
    pages: new Map(),
    server: null,
    runtimeProxy,
  };
  const server = http.createServer((req, res) => {
    handleControlRequest(state, req, res).catch((error) => {
      sendJson(res, 500, { error: error.message || String(error) });
    });
  });
  state.server = server;
  const shutdown = () => {
    shutdownDaemon(state).catch(() => {}).finally(() => process.exit(0));
  };
  process.once("SIGTERM", shutdown);
  process.once("SIGINT", shutdown);
  server.listen(config.control_socket_path);
  await once(server, "listening");
  process.stdout.write(
    `${JSON.stringify({
      schema: "elastos.browser.playwright-engine.ready/v1",
      control_socket_path: config.control_socket_path,
      local_exit_socket_path: config.local_exit_socket_path,
      runtime_proxy: publicRuntimeProxyStatus(runtimeProxy),
      direct_network: false,
    })}\n`,
  );
}

async function shutdownDaemon(state) {
  for (const pageState of state.pages.values()) {
    await closeWebrtcSession(pageState);
    await pageState.page.close().catch(() => {});
    await pageState.context.close().catch(() => {});
  }
  state.pages.clear();
  if (state.browser) {
    await state.browser.close().catch(() => {});
  }
  if (state.runtimeProxy) {
    await closeRuntimeProxy(state.runtimeProxy);
    state.runtimeProxy = null;
  }
  if (state.server) {
    await new Promise((resolve) => state.server.close(resolve));
  }
}

function parseJsonEnv(name) {
  const raw = process.env[name];
  if (!raw) {
    throw new Error(`${name} is required`);
  }
  return parseJson(raw, name);
}

function loadConfig() {
  const raw = process.env[CONFIG_ENV];
  if (raw) {
    return parseJson(raw, CONFIG_ENV);
  }
  const configPath = process.env.ELASTOS_BROWSER_PLAYWRIGHT_ENGINE_CONFIG_FILE || defaultConfigPath();
  const file = fs.readFileSync(configPath, "utf8");
  return parseJson(file, configPath);
}

function defaultConfigPath() {
  const dataHome = process.env.XDG_DATA_HOME || path.join(process.env.HOME || os.homedir(), ".local", "share");
  return path.join(dataHome, "elastos", "config", "browser-playwright-engine.json");
}

function parseJson(raw, label) {
  try {
    return JSON.parse(raw);
  } catch (error) {
    throw new Error(`${label} is invalid JSON: ${error.message}`);
  }
}

function normalizeConfig(input) {
  if (input?.schema !== "elastos.browser.playwright-engine.config/v1") {
    throw new Error("unsupported Playwright Browser Engine config schema");
  }
  const control_socket_path = requireAbsPath(input.control_socket_path, "control_socket_path");
  const local_exit_socket_path = requireAbsPath(input.local_exit_socket_path, "local_exit_socket_path");
  const profile_root = requireAbsPath(
    input.profile_root || path.join(os.tmpdir(), "elastos-browser-playwright-profile"),
    "profile_root",
  );
  const viewport = {
    width: Number(input.viewport?.width || DEFAULT_VIEWPORT.width),
    height: Number(input.viewport?.height || DEFAULT_VIEWPORT.height),
  };
  if (viewport.width < 320 || viewport.width > 3840 || viewport.height < 240 || viewport.height > 2160) {
    throw new Error("viewport must be within 320x240 and 3840x2160");
  }
  return {
    schema: input.schema,
    control_socket_path,
    local_exit_socket_path,
    profile_root,
    allowed_hosts: Array.isArray(input.allowed_hosts) ? input.allowed_hosts : [],
    allowed_protocols: normalizeAllowedProtocols(input.allowed_protocols),
    allowed_ports: normalizeAllowedPorts(input.allowed_ports),
    ice_servers: normalizeIceServers(input.ice_servers),
    viewport,
    headless: input.headless !== false,
    launch_timeout_ms: Number(input.launch_timeout_ms || 30000),
  };
}

function normalizeAllowedProtocols(value) {
  const protocols = Array.isArray(value) && value.length ? value : ["http", "https"];
  const normalized = protocols.map((item) => String(item || "").toLowerCase().replace(/:$/, ""));
  for (const protocol of normalized) {
    if (!["http", "https"].includes(protocol)) {
      throw new Error("allowed_protocols may contain only http or https");
    }
  }
  return [...new Set(normalized)];
}

function normalizeAllowedPorts(value) {
  const ports = Array.isArray(value) && value.length ? value : [80, 443];
  const normalized = ports.map((item) => Number(item));
  for (const port of normalized) {
    if (!Number.isInteger(port) || port < 1 || port > 65535) {
      throw new Error("allowed_ports must contain TCP ports between 1 and 65535");
    }
  }
  return [...new Set(normalized)];
}

function normalizeIceServers(value) {
  const source = Array.isArray(value) ? value : [
    { urls: ["stun:stun.cloudflare.com:3478", "stun:stun.l.google.com:19302"] },
  ];
  const servers = [];
  for (const item of source) {
    if (!item || typeof item !== "object") {
      continue;
    }
    const urls = Array.isArray(item.urls) ? item.urls : [item.urls];
    const normalizedUrls = urls
      .filter((entry) => typeof entry === "string")
      .map((entry) => entry.trim())
      .filter((entry) => entry.startsWith("stun:") || entry.startsWith("turn:") || entry.startsWith("turns:"));
    if (normalizedUrls.length === 0) {
      continue;
    }
    const server = { urls: normalizedUrls };
    if (typeof item.username === "string" && item.username.trim() !== "") {
      server.username = item.username.trim();
    }
    if (typeof item.credential === "string" && item.credential !== "") {
      server.credential = item.credential;
    }
    servers.push(server);
  }
  return servers;
}

function requireAbsPath(value, label) {
  if (typeof value !== "string" || !value.startsWith("/") || /\s|\0/.test(value)) {
    throw new Error(`${label} must be an absolute path without whitespace`);
  }
  return value;
}

function validateLaunchRequest(request) {
  if (request?.schema !== "elastos.browser.engine.launch-request/v1") {
    throw new Error("unsupported browser engine launch request schema");
  }
  if (request.network_mode !== "runtime_net_only" || request.direct_network !== false) {
    throw new Error("browser engine requires runtime_net_only without direct network");
  }
  if (!["diagnostic_frame", "webrtc_remote_display"].includes(request.display_mode)) {
    throw new Error(`${request.display_mode || "missing"} display mode is not supported by this Browser Engine`);
  }
  const parsed = new URL(request.url);
  if (!["http:", "https:"].includes(parsed.protocol)) {
    throw new Error("browser URL must use http or https");
  }
  if (request.referer != null) {
    const referer = new URL(String(request.referer));
    if (!["http:", "https:"].includes(referer.protocol)) {
      throw new Error("browser referer must use http or https");
    }
  }
}

async function ensureDaemon(config, requestedDisplayMode) {
  const status = await daemonStatus(config.control_socket_path);
  if (daemonMatchesRequest(status, config, requestedDisplayMode)) {
    return;
  }
  await stopStaleDaemon(status);
  await fs.promises.mkdir(path.dirname(config.control_socket_path), { recursive: true });
  await fs.promises.mkdir(config.profile_root, { recursive: true });
  await unlinkSocket(config.control_socket_path);
  const child = spawn(process.execPath, [__filename, "--daemon"], {
    detached: true,
    stdio: ["ignore", "ignore", "ignore"],
    env: {
      ...process.env,
      [CONFIG_ENV]: JSON.stringify(config),
    },
  });
  child.unref();
  const deadline = Date.now() + 15000;
  while (Date.now() < deadline) {
    if (daemonMatchesRequest(await daemonStatus(config.control_socket_path), config, requestedDisplayMode)) {
      return;
    }
    await sleep(120);
  }
  throw new Error("Playwright Browser Engine daemon did not become ready");
}

async function socketResponds(socketPath) {
  return Boolean(await daemonStatus(socketPath));
}

async function daemonStatus(socketPath) {
  try {
    const result = await controlRequest(socketPath, "GET", "/status", null, 750);
    return result.schema === "elastos.browser.playwright-engine.status/v1" ? result : null;
  } catch {
    return null;
  }
}

function daemonMatchesRequest(status, config, requestedDisplayMode) {
  if (!status) {
    return false;
  }
  if (status.config_fingerprint !== configFingerprint(config)) {
    return false;
  }
  if (
    !Array.isArray(status.supported_display_modes) ||
    !status.supported_display_modes.includes(requestedDisplayMode)
  ) {
    return false;
  }
  return Array.isArray(status.operations) && status.operations.includes("launch");
}

async function stopStaleDaemon(status) {
  if (!Number.isInteger(status?.pid) || status.pid <= 1 || status.pid === process.pid) {
    return;
  }
  try {
    process.kill(status.pid, "SIGTERM");
  } catch {
    return;
  }
  await sleep(400);
}

function configFingerprint(config) {
  return createHash("sha256").update(JSON.stringify(config)).digest("hex");
}

async function handleControlRequest(state, req, res) {
  const requestUrl = new URL(req.url, "http://browser-engine.local");
  const pathname = requestUrl.pathname;
  if (req.method === "GET" && pathname === "/status") {
    sendJson(res, 200, {
      schema: "elastos.browser.playwright-engine.status/v1",
      pages: state.pages.size,
      page_metrics: Array.from(state.pages.entries()).map(([pageId, pageState]) =>
        publicPageStatus(pageId, pageState),
      ),
      pid: process.pid,
      config_fingerprint: configFingerprint(state.config),
      supported_display_modes: ["diagnostic_frame", "webrtc_remote_display"],
      operations: ["launch", "status", "screenshot", "frame", "input", "webrtc", "media", "close"],
      direct_network: false,
      runtime_proxy: publicRuntimeProxyStatus(state.runtimeProxy),
    });
    return;
  }
  if (req.method === "POST" && pathname === "/launch") {
    const request = await readJsonBody(req);
    const result = await launchPage(state, request);
    sendJson(res, 200, result);
    return;
  }
  const match = pathname.match(/^\/pages\/([^/]+)\/(status|screenshot|frame|input|webrtc|media|close)$/);
  if (!match) {
    sendJson(res, 404, { error: "unknown browser engine route" });
    return;
  }
  const [, encodedPageId, op] = match;
  const pageId = decodeURIComponent(encodedPageId);
  const pageState = state.pages.get(pageId);
  if (!pageState) {
    sendJson(res, 404, { error: "browser page not found" });
    return;
  }
  if (req.method === "GET" && op === "status") {
    sendJson(res, 200, await publicPageStatus(pageId, pageState));
    return;
  }
  if (req.method === "GET" && op === "screenshot") {
    const image = await pageState.page.screenshot({ type: "png", fullPage: false });
    sendJson(res, 200, {
      schema: "elastos.browser.screenshot/v1",
      page_id: pageId,
      content_type: "image/png",
      base64: image.toString("base64"),
      width: pageState.viewport.width,
      height: pageState.viewport.height,
    });
    return;
  }
  if (req.method === "GET" && op === "frame") {
    const since = Number(requestUrl.searchParams.get("since") || 0);
    const waitMs = Math.max(0, Math.min(5000, Number(requestUrl.searchParams.get("wait_ms") || 1200)));
    if (Number.isFinite(since) && since >= pageState.frameSeq && waitMs > 0) {
      await waitForFrameChange(pageState, waitMs);
    }
    if (Number.isFinite(since) && since >= pageState.frameSeq) {
      sendJson(res, 200, {
        schema: "elastos.browser.frame/v1",
        page_id: pageId,
        seq: pageState.frameSeq,
        changed: false,
        width: pageState.viewport.width,
        height: pageState.viewport.height,
        actual_url: pageState.page.url(),
        title: await pageState.page.title().catch(() => ""),
      });
      return;
    }
    const image = await pageState.page.screenshot({ type: "png", fullPage: false });
    sendJson(res, 200, {
      schema: "elastos.browser.frame/v1",
      page_id: pageId,
      seq: pageState.frameSeq,
      changed: true,
      content_type: "image/png",
      base64: image.toString("base64"),
      width: pageState.viewport.width,
      height: pageState.viewport.height,
      actual_url: pageState.page.url(),
      title: await pageState.page.title().catch(() => ""),
    });
    return;
  }
  if (req.method === "POST" && op === "input") {
    const body = await readJsonBody(req);
    await applyInputEvent(pageState, body.event || {});
    markFrameChanged(pageState);
    const image = await pageState.page.screenshot({ type: "png", fullPage: false });
    const title = await pageState.page.title().catch(() => "");
    const navigation = await navigationState(pageState);
    sendJson(res, 200, {
      schema: "elastos.browser.input-result/v1",
      page_id: pageId,
      seq: pageState.frameSeq,
      screenshot: image.toString("base64"),
      content_type: "image/png",
      width: pageState.viewport.width,
      height: pageState.viewport.height,
      actual_url: pageState.page.url(),
      title,
      ...navigation,
    });
    return;
  }
  if (req.method === "POST" && op === "webrtc") {
    const body = await readJsonBody(req);
    const answer = await handleWebrtcSignal(pageState, body.signal || {});
    sendJson(res, 200, answer);
    return;
  }
  if (req.method === "GET" && op === "media") {
    sendJson(res, 200, await mediaStatus(pageState));
    return;
  }
  if (req.method === "POST" && op === "close") {
    await closeWebrtcSession(pageState);
    await pageState.page.close().catch(() => {});
    state.pages.delete(pageId);
    sendJson(res, 200, {
      schema: "elastos.browser.close-result/v1",
      page_id: pageId,
      closed: true,
    });
    return;
  }
  sendJson(res, 405, { error: "method not allowed" });
}

async function launchPage(state, request) {
  validateLaunchRequest(request);
  const browser = await ensureBrowser(state);
  const viewport = normalizeViewport(request.viewport, state.config.viewport);
  const context = await browser.newContext({
    viewport,
  });
  const page = await context.newPage();
  const pageId = newPageId(request.stream_id);
  const wallet = normalizeWalletBridge(request.wallet || {});
  const pageState = {
    pageId,
    page,
    context,
    config: state.config,
    wallet,
    request,
    viewport,
    frameSeq: 0,
    frameWaiters: [],
    webrtc: null,
    metrics: newPageMetrics(request.display_mode),
  };

  await page.exposeBinding("__elastosWalletRequest", async (source, payload) => {
    return handleWalletRequest(wallet, source, payload);
  });
  await page.addInitScript(walletInitScript(wallet));
  page.on("framenavigated", (frame) => {
    if (frame === page.mainFrame()) {
      markFrameChanged(pageState);
    }
  });
  page.on("load", () => markFrameChanged(pageState));
  const gotoOptions = {
    waitUntil: "domcontentloaded",
    timeout: state.config.launch_timeout_ms,
  };
  if (request.referer) {
    gotoOptions.referer = String(request.referer);
  }
  await page.goto(request.url, gotoOptions);
  markFrameChanged(pageState);
  const actualUrl = page.url();
  const title = await page.title().catch(() => "");
  if (state.pages.has(pageId)) {
    const previous = state.pages.get(pageId);
    await closeWebrtcSession(previous);
    await previous.page.close().catch(() => {});
    await previous.context.close().catch(() => {});
  }
  state.pages.set(pageId, pageState);
  const isWebrtc = request.display_mode === "webrtc_remote_display";
  const displaySession = isWebrtc
    ? {
        schema: "elastos.browser.display-session/v1",
        session_id: `display:${request.stream_id}`,
        mode: "webrtc_remote_display",
        display_backend: DISPLAY_BACKEND,
        backend_class: DISPLAY_BACKEND_CLASS,
        network_mode: "runtime_net_only",
        direct_network: false,
        input: "datachannel",
        audio: false,
        video: true,
        signaling_url: `/api/apps/browser/pages/${encodeURIComponent(pageId)}/webrtc`,
        ice_servers: state.config.ice_servers,
      }
    : {
        schema: "elastos.browser.display-session/v1",
        session_id: `display:${request.stream_id}`,
        mode: "diagnostic_frame",
        display_backend: "diagnostic_frame",
        backend_class: "diagnostic",
        network_mode: "runtime_net_only",
        direct_network: false,
        input: "runtime_route",
        audio: false,
        video: false,
      };
  return {
    schema: "elastos.browser.engine.supervisor-result/v1",
    page_id: pageId,
    adapter: request.adapter,
    engine: request.engine,
    stream_id: request.stream_id,
    actual_url: actualUrl,
    title,
    network_mode: "runtime_net_only",
    direct_network: false,
    wallet_injection: false,
    display_session: displaySession,
    view: isWebrtc
      ? null
      : {
          schema: "elastos.browser.view/v1",
          mode: "runtime_frame",
          frame_url: `/api/apps/browser/pages/${encodeURIComponent(pageId)}/frame`,
          input_url: `/api/apps/browser/pages/${encodeURIComponent(pageId)}/input`,
          width: viewport.width,
          height: viewport.height,
        },
    wallet_bridge: {
      schema: "elastos.browser.wallet-bridge/v1",
      mode: "runtime_mediated_eip1193",
      accounts: wallet.accounts.length,
      default_chain_namespace: wallet.default_chain_namespace,
      signing: "fail_closed",
    },
  };
}

async function handleWebrtcSignal(pageState, signal) {
  const signalType = validateWebrtcSignal(signal);
  if (signalType === "candidate") {
    return handleWebrtcCandidate(pageState, signal.candidate);
  }
  if (signalType === "end_of_candidates") {
    return handleWebrtcEndOfCandidates(pageState);
  }

  await closeWebrtcSession(pageState);

  const peer = new wrtc.RTCPeerConnection({
    iceServers: pageState.request?.ice_servers || pageState.config?.ice_servers || [],
    bundlePolicy: "max-bundle",
    rtcpMuxPolicy: "require",
  });
  const videoSource = new RTCVideoSource();
  const videoTrack = videoSource.createTrack();
  const videoSender = peer.addTrack(videoTrack);

  const session = {
    peer,
    videoSource,
    videoTrack,
    videoSender,
    cdpSession: null,
    frameBusy: false,
    closed: false,
    pendingLocalCandidates: [],
    localEndOfCandidates: false,
    waitingForLocalCandidateResolvers: [],
  };
  pageState.webrtc = session;

  peer.ondatachannel = ({ channel }) => {
    channel.onmessage = (event) => {
      handleWebrtcInputMessage(pageState, event.data).catch(() => {});
    };
  };
  peer.onconnectionstatechange = () => {
    pageState.metrics.webrtc_connection_state = peer.connectionState;
    if (["closed", "failed", "disconnected"].includes(peer.connectionState)) {
      closeWebrtcSession(pageState).catch(() => {});
    }
  };
  peer.oniceconnectionstatechange = () => {
    pageState.metrics.ice_connection_state = peer.iceConnectionState;
  };
  peer.onicegatheringstatechange = () => {
    pageState.metrics.ice_gathering_state = peer.iceGatheringState;
  };
  peer.onicecandidate = (event) => {
    if (!pageState.webrtc || pageState.webrtc !== session || session.closed) {
      return;
    }
    if (event.candidate) {
      const serialized = serializeLocalIceCandidate(event.candidate);
      if (serialized) {
        session.pendingLocalCandidates.push(serialized);
      }
    } else {
      session.localEndOfCandidates = true;
    }
    flushLocalCandidateWaiters(session);
  };

  const normalizedOffer = normalizeWebrtcOfferForWrtc(signal.sdp);
  await peer.setRemoteDescription({ type: "offer", sdp: normalizedOffer });
  await startWebrtcScreencast(pageState, session);
  const answer = await peer.createAnswer();
  await peer.setLocalDescription(answer);
  await configureVideoSender(videoSender);
  await waitForInitialLocalCandidates(peer, session, WEBRTC_INITIAL_CANDIDATE_WAIT_MS);
  const localDescription = peer.localDescription || answer;
  const answerSdp =
    typeof localDescription?.sdp === "string" ? localDescription.sdp : "";
  if (!answerSdp.trim()) {
    throw new Error("WebRTC answer SDP is unavailable");
  }
  const localCandidateSignals = drainLocalCandidateSignals(session);
  return {
    schema: "elastos.browser.webrtc-answer/v1",
    type: "answer",
    sdp: answerSdp,
    candidates: localCandidateSignals.candidates,
    end_of_candidates: localCandidateSignals.end_of_candidates,
  };
}

async function handleWebrtcCandidate(pageState, candidate) {
  const peer = pageState.webrtc?.peer;
  if (!peer) {
    return withLocalCandidateSignals(pageState.webrtc, {
      schema: "elastos.browser.webrtc-signal-ack/v1",
      type: "candidate",
      accepted: false,
      reason: "session_not_ready",
    });
  }
  const normalized = normalizeWebrtcCandidateForWrtc(candidate);
  try {
    await peer.addIceCandidate(normalized);
  } catch (error) {
    const repaired = repairWebrtcCandidateForWrtc(normalized);
    if (!repaired) {
      return withLocalCandidateSignals(pageState.webrtc, {
        schema: "elastos.browser.webrtc-signal-ack/v1",
        type: "candidate",
        accepted: false,
        reason: "candidate_rejected",
      });
    }
    try {
      await peer.addIceCandidate(repaired);
    } catch (_repairError) {
      return withLocalCandidateSignals(pageState.webrtc, {
        schema: "elastos.browser.webrtc-signal-ack/v1",
        type: "candidate",
        accepted: false,
        reason: "candidate_rejected",
      });
    }
  }
  return withLocalCandidateSignals(pageState.webrtc, {
    schema: "elastos.browser.webrtc-signal-ack/v1",
    type: "candidate",
    accepted: true,
  });
}

async function handleWebrtcEndOfCandidates(pageState) {
  if (!pageState.webrtc?.peer) {
    return withLocalCandidateSignals(pageState.webrtc, {
      schema: "elastos.browser.webrtc-signal-ack/v1",
      type: "end_of_candidates",
      accepted: false,
      reason: "session_not_ready",
    });
  }
  await pageState.webrtc.peer.addIceCandidate(null).catch(() => {});
  return withLocalCandidateSignals(pageState.webrtc, {
    schema: "elastos.browser.webrtc-signal-ack/v1",
    type: "end_of_candidates",
    accepted: true,
  });
}

function validateWebrtcSignal(signal) {
  if (signal?.schema === "elastos.browser.webrtc-offer/v1") {
    if (signal.type !== "offer") {
      throw new Error("WebRTC signal must be an offer");
    }
    if (typeof signal.sdp !== "string" || signal.sdp.trim() === "" || signal.sdp.length > 256 * 1024) {
      throw new Error("WebRTC offer has invalid SDP");
    }
    for (const line of signal.sdp.split(/\r?\n/)) {
      if (line.startsWith("a=candidate:") || line === "a=end-of-candidates") {
        throw new Error("WebRTC offer must send ICE candidates through candidate messages");
      }
    }
    return "offer";
  }
  if (signal?.schema === "elastos.browser.webrtc-candidate/v1") {
    if (signal.type !== "candidate") {
      throw new Error("WebRTC signal must be a candidate");
    }
    if (signal.sdp !== undefined) {
      throw new Error("WebRTC candidate must not include SDP");
    }
    if (!signal.candidate || typeof signal.candidate.candidate !== "string" || signal.candidate.candidate.trim() === "") {
      throw new Error("WebRTC candidate has invalid candidate line");
    }
    return "candidate";
  }
  if (signal?.schema === "elastos.browser.webrtc-end-of-candidates/v1") {
    if (signal.type !== "end_of_candidates") {
      throw new Error("WebRTC signal must be end_of_candidates");
    }
    if (signal.sdp !== undefined || signal.candidate !== undefined) {
      throw new Error("WebRTC end_of_candidates must not include SDP or candidate");
    }
    return "end_of_candidates";
  }
  throw new Error("WebRTC signal uses an unsupported schema");
}

function normalizeWebrtcOfferForWrtc(sdp) {
  return String(sdp || "")
    .split(/\r?\n/)
    .map((line) => line.trimEnd())
    .filter((line) => line.length > 0)
    .filter((line) => !line.startsWith("a=candidate:") && line !== "a=end-of-candidates")
    .join("\r\n")
    .concat("\r\n");
}

function normalizeWebrtcCandidateForWrtc(candidate) {
  const normalized = { ...(candidate || {}) };
  const line = String(normalized.candidate || "").trim();
  normalized.candidate = line.startsWith("a=") ? line.slice(2) : line;
  if (typeof normalized.sdpMid === "string") {
    const value = normalized.sdpMid.trim();
    normalized.sdpMid = value || undefined;
  } else {
    delete normalized.sdpMid;
  }
  if (!Number.isInteger(normalized.sdpMLineIndex) || normalized.sdpMLineIndex < 0) {
    if (!normalized.sdpMid) {
      normalized.sdpMLineIndex = 0;
    } else {
      delete normalized.sdpMLineIndex;
    }
  }
  if (typeof normalized.usernameFragment !== "string" || normalized.usernameFragment.trim() === "") {
    delete normalized.usernameFragment;
  } else {
    normalized.usernameFragment = normalized.usernameFragment.trim();
  }
  return normalized;
}

function repairWebrtcCandidateForWrtc(candidate) {
  const line = String(candidate?.candidate || "").trim();
  if (!line) {
    return null;
  }
  const tokens = line.split(/\s+/);
  if (tokens.length < 8) {
    return null;
  }
  const keep = new Set(["raddr", "rport", "tcptype", "ufrag", "generation"]);
  const out = tokens.slice(0, 8);
  let changed = false;
  for (let index = 8; index < tokens.length; index += 1) {
    const key = String(tokens[index] || "").toLowerCase();
    if (index + 1 >= tokens.length) {
      changed = true;
      break;
    }
    const value = tokens[index + 1];
    index += 1;
    if (!keep.has(key)) {
      changed = true;
      continue;
    }
    out.push(key, value);
  }
  if (!changed) {
    return null;
  }
  return {
    ...candidate,
    candidate: out.join(" "),
  };
}

function serializeLocalIceCandidate(candidate) {
  if (!candidate || typeof candidate.toJSON !== "function") {
    return null;
  }
  const normalized = normalizeWebrtcCandidateForWrtc(candidate.toJSON());
  if (!normalized?.candidate) {
    return null;
  }
  return normalized;
}

function flushLocalCandidateWaiters(session) {
  if (!session || !Array.isArray(session.waitingForLocalCandidateResolvers)) {
    return;
  }
  if (!session.localEndOfCandidates && session.pendingLocalCandidates.length === 0) {
    return;
  }
  const waiters = session.waitingForLocalCandidateResolvers.splice(0);
  for (const resolve of waiters) {
    resolve();
  }
}

async function waitForInitialLocalCandidates(peer, session, timeoutMs) {
  if (
    peer.iceGatheringState === "complete" ||
    session.localEndOfCandidates ||
    session.pendingLocalCandidates.length > 0
  ) {
    return;
  }
  await new Promise((resolve) => {
    let settled = false;
    const previousGatherHandler = peer.onicegatheringstatechange;
    const finish = () => {
      if (settled) {
        return;
      }
      settled = true;
      peer.onicegatheringstatechange = previousGatherHandler || null;
      resolve();
    };
    const timer = setTimeout(finish, Math.max(0, timeoutMs));
    session.waitingForLocalCandidateResolvers.push(() => {
      clearTimeout(timer);
      finish();
    });
    peer.onicegatheringstatechange = () => {
      if (peer.iceGatheringState === "complete") {
        clearTimeout(timer);
        finish();
        return;
      }
      if (typeof previousGatherHandler === "function") {
        previousGatherHandler();
      }
    };
  });
}

function drainLocalCandidateSignals(session) {
  if (!session) {
    return { candidates: [], end_of_candidates: false };
  }
  const candidates = session.pendingLocalCandidates.splice(0);
  const end_of_candidates = Boolean(session.localEndOfCandidates);
  if (end_of_candidates) {
    session.localEndOfCandidates = false;
  }
  return { candidates, end_of_candidates };
}

function withLocalCandidateSignals(session, payload) {
  const signals = drainLocalCandidateSignals(session);
  return {
    ...payload,
    candidates: signals.candidates,
    end_of_candidates: signals.end_of_candidates,
  };
}

async function startWebrtcScreencast(pageState, session) {
  const cdpSession = await pageState.context.newCDPSession(pageState.page);
  session.cdpSession = cdpSession;
  cdpSession.on("Page.screencastFrame", async (frame) => {
    if (session.closed || session.frameBusy) {
      pageState.metrics.dropped_frames += 1;
      await ackScreencastFrame(cdpSession, frame.sessionId);
      return;
    }
    session.frameBusy = true;
    const startedAt = Date.now();
    try {
      const decoded = jpeg.decode(Buffer.from(frame.data, "base64"), { useTArray: true });
      const rgbaFrame = evenRgbaFrame(decoded);
      const i420Frame = {
        width: rgbaFrame.width,
        height: rgbaFrame.height,
        data: Buffer.alloc((rgbaFrame.width * rgbaFrame.height * 3) / 2),
      };
      rgbaToI420(rgbaFrame, i420Frame);
      session.videoSource.onFrame(i420Frame);
      pageState.metrics.frame_count += 1;
      pageState.metrics.last_frame_at_ms = Date.now();
      pageState.metrics.last_frame_decode_ms = pageState.metrics.last_frame_at_ms - startedAt;
      pageState.metrics.last_frame_width = rgbaFrame.width;
      pageState.metrics.last_frame_height = rgbaFrame.height;
      markFrameChanged(pageState);
    } finally {
      session.frameBusy = false;
      await ackScreencastFrame(cdpSession, frame.sessionId);
    }
  });
  await cdpSession.send("Page.startScreencast", {
    format: "jpeg",
    quality: SCREENCAST_JPEG_QUALITY,
    everyNthFrame: 1,
  });
}

async function configureVideoSender(videoSender) {
  if (
    !videoSender ||
    typeof videoSender.getParameters !== "function" ||
    typeof videoSender.setParameters !== "function"
  ) {
    return;
  }
  const params = videoSender.getParameters() || {};
  const encodings = Array.isArray(params.encodings) && params.encodings.length ? params.encodings : [{}];
  encodings[0] = {
    ...encodings[0],
    maxBitrate: WEBRTC_MAX_BITRATE_BPS,
    maxFramerate: WEBRTC_MAX_FRAMERATE,
  };
  params.encodings = encodings;
  params.degradationPreference = "maintain-resolution";
  await videoSender.setParameters(params).catch(() => {});
}

async function ackScreencastFrame(cdpSession, sessionId) {
  await cdpSession.send("Page.screencastFrameAck", { sessionId }).catch(() => {});
}

function evenRgbaFrame(decoded) {
  const width = decoded.width - (decoded.width % 2);
  const height = decoded.height - (decoded.height % 2);
  if (width < 2 || height < 2) {
    throw new Error("browser screencast frame is too small");
  }
  if (width === decoded.width && height === decoded.height) {
    return { width, height, data: decoded.data };
  }
  const data = Buffer.alloc(width * height * 4);
  for (let row = 0; row < height; row += 1) {
    const sourceStart = row * decoded.width * 4;
    const sourceEnd = sourceStart + width * 4;
    data.set(decoded.data.subarray(sourceStart, sourceEnd), row * width * 4);
  }
  return { width, height, data };
}

async function handleWebrtcInputMessage(pageState, raw) {
  const payload = parseMaybeJson(raw);
  if (payload?.schema !== "elastos.browser.input-event/v1" || !payload.event) {
    return;
  }
  await applyInputEvent(pageState, payload.event);
  markFrameChanged(pageState);
}

function parseMaybeJson(raw) {
  try {
    if (typeof raw === "string") {
      return JSON.parse(raw);
    }
    if (Buffer.isBuffer(raw)) {
      return JSON.parse(raw.toString("utf8"));
    }
    if (raw instanceof ArrayBuffer) {
      return JSON.parse(Buffer.from(raw).toString("utf8"));
    }
    if (ArrayBuffer.isView(raw)) {
      return JSON.parse(Buffer.from(raw.buffer, raw.byteOffset, raw.byteLength).toString("utf8"));
    }
  } catch {
    return null;
  }
  return null;
}

async function closeWebrtcSession(pageState) {
  const session = pageState?.webrtc;
  if (!session) {
    return;
  }
  pageState.webrtc = null;
  pageState.metrics.webrtc_connection_state = "closed";
  session.closed = true;
  session.pendingLocalCandidates = [];
  session.waitingForLocalCandidateResolvers = [];
  if (session.cdpSession) {
    await session.cdpSession.send("Page.stopScreencast").catch(() => {});
    await session.cdpSession.detach().catch(() => {});
  }
  if (session.videoTrack) {
    session.videoTrack.stop();
  }
  if (session.peer) {
    session.peer.close();
  }
}

function normalizeViewport(value, defaultViewport) {
  const width = Number(value?.width || defaultViewport.width);
  const height = Number(value?.height || defaultViewport.height);
  if (!Number.isInteger(width) || !Number.isInteger(height)) {
    throw new Error("viewport width and height must be integers");
  }
  if (width < 320 || width > 3840 || height < 240 || height > 2160) {
    throw new Error("viewport must be within 320x240 and 3840x2160");
  }
  return { width, height };
}

function markFrameChanged(pageState) {
  pageState.frameSeq = Number(pageState.frameSeq || 0) + 1;
  const waiters = pageState.frameWaiters.splice(0);
  for (const resolve of waiters) {
    resolve(true);
  }
}

async function waitForFrameChange(pageState, waitMs) {
  await new Promise((resolve) => {
    const timer = setTimeout(() => {
      pageState.frameWaiters = pageState.frameWaiters.filter((item) => item !== done);
      resolve(false);
    }, waitMs);
    const done = () => {
      clearTimeout(timer);
      resolve(true);
    };
    pageState.frameWaiters.push(done);
  });
}

function newPageMetrics(displayMode) {
  return {
    schema: "elastos.browser.page-metrics/v1",
    display_mode: displayMode,
    display_backend: displayMode === "webrtc_remote_display" ? DISPLAY_BACKEND : "diagnostic_frame",
    backend_class: displayMode === "webrtc_remote_display" ? DISPLAY_BACKEND_CLASS : "diagnostic",
    started_at_ms: Date.now(),
    frame_count: 0,
    dropped_frames: 0,
    last_frame_at_ms: null,
    last_frame_decode_ms: null,
    last_frame_width: null,
    last_frame_height: null,
    webrtc_connection_state: "new",
    ice_connection_state: "new",
    ice_gathering_state: "new",
  };
}

async function publicPageStatus(pageId, pageState) {
  const metrics = pageState.metrics || newPageMetrics(pageState.request?.display_mode || "unknown");
  const lastFrameAgeMs = metrics.last_frame_at_ms ? Date.now() - metrics.last_frame_at_ms : null;
  const navigation = await navigationState(pageState);
  return {
    schema: "elastos.browser.page-status/v1",
    page_id: pageId,
    display_mode: metrics.display_mode,
    display_backend: metrics.display_backend,
    backend_class: metrics.backend_class,
    proof_surface: metrics.backend_class === DISPLAY_BACKEND_CLASS,
    actual_url: pageState.page.url(),
    frame_seq: pageState.frameSeq,
    frame_count: metrics.frame_count,
    dropped_frames: metrics.dropped_frames,
    last_frame_age_ms: lastFrameAgeMs,
    last_frame_decode_ms: metrics.last_frame_decode_ms,
    last_frame_width: metrics.last_frame_width,
    last_frame_height: metrics.last_frame_height,
    webrtc_connection_state: metrics.webrtc_connection_state,
    ice_connection_state: metrics.ice_connection_state,
    ice_gathering_state: metrics.ice_gathering_state,
    direct_network: false,
    ...navigation,
  };
}

async function navigationState(pageState) {
  try {
    const session = await pageState.context.newCDPSession(pageState.page);
    try {
      const history = await session.send("Page.getNavigationHistory");
      const currentIndex = Number(history.currentIndex);
      const entries = Array.isArray(history.entries) ? history.entries : [];
      return {
        can_go_back: currentIndex > 0,
        can_go_forward: currentIndex >= 0 && currentIndex < entries.length - 1,
      };
    } finally {
      await session.detach().catch(() => {});
    }
  } catch {
    return {
      can_go_back: false,
      can_go_forward: false,
    };
  }
}

async function mediaStatus(pageState) {
  const elements = [];
  const frames = pageState.page.frames();
  const frame_summaries = [];
  for (let frameIndex = 0; frameIndex < frames.length; frameIndex += 1) {
    const frame = frames[frameIndex];
    const frameElements = await mediaStatusForFrame(frame, frameIndex).catch(() => []);
    elements.push(...frameElements);
    const summary = await mediaFrameSummary(frame, frameIndex).catch(() => null);
    if (summary) {
      frame_summaries.push(summary);
    }
  }
  return {
    schema: "elastos.browser.media-status/v1",
    page_id: pageState.pageId,
    actual_url: pageState.page.url(),
    element_count: elements.length,
    elements,
    frame_summaries,
    direct_network: false,
  };
}

async function mediaFrameSummary(frame, frameIndex) {
  return frame.evaluate((frameIndexInPage) => ({
    frame_index: frameIndexInPage,
    frame_url: String(window.location.href || ""),
    title: String(document.title || ""),
    text_sample: String(document.body?.innerText || "").replace(/\s+/g, " ").trim().slice(0, 240),
  }), frameIndex);
}

async function mediaStatusForFrame(frame, frameIndex) {
  return frame.evaluate((frameIndexInPage) => {
    const numeric = (value) => (Number.isFinite(Number(value)) ? Number(value) : null);
    return Array.from(document.querySelectorAll("video,audio")).map((element, index) => ({
      frame_index: frameIndexInPage,
      frame_url: String(window.location.href || ""),
      index,
      tag: element.tagName.toLowerCase(),
      current_time: numeric(element.currentTime),
      duration: numeric(element.duration),
      paused: Boolean(element.paused),
      ended: Boolean(element.ended),
      muted: Boolean(element.muted),
      volume: numeric(element.volume),
      ready_state: Number(element.readyState),
      network_state: Number(element.networkState),
      current_src: String(element.currentSrc || element.src || ""),
      video_width: Number(element.videoWidth || 0),
      video_height: Number(element.videoHeight || 0),
      audio_decoded_bytes: Number(element.webkitAudioDecodedByteCount || 0),
      video_decoded_bytes: Number(element.webkitVideoDecodedByteCount || 0),
    }));
  }, frameIndex);
}

async function ensureBrowser(state) {
  if (state.browser) {
    return state.browser;
  }
  if (!state.runtimeProxy?.url) {
    throw new Error("browser runtime proxy is unavailable");
  }
  state.browser = await chromium.launch({
    headless: state.config.headless,
    args: [
      "--disable-background-networking",
      "--disable-component-update",
      "--disable-default-apps",
      "--disable-extensions",
      "--disable-features=OptimizationHints,Translate,MediaRouter",
      "--autoplay-policy=no-user-gesture-required",
      "--disable-quic",
      "--disable-sync",
      "--host-resolver-rules=MAP * 0.0.0.0, EXCLUDE localhost, EXCLUDE 127.0.0.1",
      "--no-first-run",
      "--no-sandbox",
      "--proxy-bypass-list=<-loopback>",
      `--proxy-server=${state.runtimeProxy.url}`,
    ],
  });
  return state.browser;
}

function normalizeWalletBridge(wallet) {
  const accounts = Array.isArray(wallet.accounts)
    ? wallet.accounts
        .filter((account) => typeof account.address === "string" && account.chain_namespace?.startsWith("eip155:"))
        .map((account) => ({
          account_id: String(account.account_id || ""),
          chain_namespace: account.chain_namespace,
          address: account.address,
          label: account.label || null,
          proof_type: account.proof_type || null,
        }))
    : [];
  const default_chain_namespace =
    wallet.default_chain_namespace && accounts.some((account) => account.chain_namespace === wallet.default_chain_namespace)
      ? wallet.default_chain_namespace
      : accounts[0]?.chain_namespace || "eip155:20";
  return {
    accounts,
    default_chain_namespace,
    approval_url: typeof wallet.approval_url === "string" && wallet.approval_url.trim() !== "" ? wallet.approval_url.trim() : null,
    transaction_url: typeof wallet.transaction_url === "string" && wallet.transaction_url.trim() !== "" ? wallet.transaction_url.trim() : null,
    read_url: typeof wallet.read_url === "string" && wallet.read_url.trim() !== "" ? wallet.read_url.trim() : null,
    transaction_broadcast_url:
      typeof wallet.transaction_broadcast_url === "string" && wallet.transaction_broadcast_url.trim() !== ""
        ? wallet.transaction_broadcast_url.trim()
        : null,
    approval_status_url: typeof wallet.approval_status_url === "string" && wallet.approval_status_url.trim() !== "" ? wallet.approval_status_url.trim() : null,
    home_token: typeof wallet.home_token === "string" && wallet.home_token.trim() !== "" ? wallet.home_token.trim() : null,
    pending_approval_keys: new Set(),
  };
}

async function handleWalletRequest(wallet, source, payload) {
  const method = payload?.method;
  const params = Array.isArray(payload?.params) ? payload.params : [];
  const current = wallet.accounts.find((account) => account.chain_namespace === wallet.default_chain_namespace) || wallet.accounts[0];
  if (method === "eth_requestAccounts" || method === "eth_accounts") {
    return current ? [current.address] : [];
  }
  if (method === "eth_chainId") {
    return chainNamespaceToHex(wallet.default_chain_namespace);
  }
  if (method === "net_version") {
    return String(chainNamespaceToDecimal(wallet.default_chain_namespace));
  }
  if (method === "wallet_switchEthereumChain") {
    const chainId = params[0]?.chainId;
    const next = wallet.accounts.find((account) => chainNamespaceToHex(account.chain_namespace) === String(chainId).toLowerCase());
    if (!next) {
      const error = new Error("Requested chain is not available through the current ElastOS Wallet account.");
      error.code = 4902;
      throw error;
    }
    wallet.default_chain_namespace = next.chain_namespace;
    return null;
  }
  if (method === "wallet_addEthereumChain") {
    return null;
  }
  if (
    method === "eth_blockNumber" ||
    method === "eth_getBalance" ||
    method === "eth_call" ||
    method === "eth_estimateGas" ||
    method === "eth_getTransactionByHash" ||
    method === "eth_getTransactionReceipt"
  ) {
    return requestRuntimeWalletRead(wallet, source, method, params, current);
  }
  if (method === "personal_sign") {
    return requestRuntimeWalletApproval(wallet, source, method, params, current);
  }
  if (method === "eth_sendTransaction") {
    return requestRuntimeWalletTransaction(wallet, source, method, params, current);
  }
  const error = new Error(`${method} requires Runtime wallet approval and is not exposed to the Browser engine yet.`);
  error.code = 4100;
  throw error;
}

async function requestRuntimeWalletRead(wallet, source, method, params, current) {
  if (!current) {
    const error = new Error("No ElastOS Wallet account is available for this Browser page.");
    error.code = 4100;
    throw error;
  }
  if (!wallet.read_url || !wallet.home_token) {
    const error = new Error("Runtime chain provider reads are unavailable for this Browser page.");
    error.code = 4100;
    throw error;
  }
  const pageUrl = typeof source?.frame?.url === "function" ? source.frame.url() : "";
  if (!pageUrl || !/^https?:\/\//.test(pageUrl)) {
    const error = new Error("Runtime chain provider reads require an http or https Browser page.");
    error.code = 4100;
    throw error;
  }
  const response = await fetch(wallet.read_url, {
    method: "POST",
    headers: {
      "content-type": "application/json",
      "x-elastos-home-token": wallet.home_token,
    },
    body: JSON.stringify({
      method,
      params,
      chain_namespace: current.chain_namespace,
      address: current.address,
      page_url: pageUrl,
      origin: originFromUrl(pageUrl),
    }),
  });
  const text = await response.text();
  let body = null;
  if (text.trim() !== "") {
    try {
      body = JSON.parse(text);
    } catch {
      body = { message: text };
    }
  }
  if (!response.ok) {
    const error = new Error(body?.message || body?.error || `Runtime chain provider read failed with ${response.status}`);
    error.code = response.status === 400 ? 4001 : 4100;
    throw error;
  }
  if (body?.schema !== "elastos.browser.wallet-read-result/v1" || body.requires_approval !== false) {
    const error = new Error("Runtime chain provider returned an invalid Browser wallet read response.");
    error.code = 4100;
    throw error;
  }
  return body.result;
}

async function requestRuntimeWalletApproval(wallet, source, method, params, current) {
  if (!current) {
    const error = new Error("No ElastOS Wallet account is available for this Browser page.");
    error.code = 4100;
    throw error;
  }
  if (!wallet.approval_url || !wallet.approval_status_url || !wallet.home_token) {
    const error = new Error("Runtime wallet approval is unavailable for this Browser page.");
    error.code = 4100;
    throw error;
  }
  const pageUrl = typeof source?.frame?.url === "function" ? source.frame.url() : "";
  if (!pageUrl || !/^https?:\/\//.test(pageUrl)) {
    const error = new Error("Runtime wallet approval requires an http or https Browser page.");
    error.code = 4100;
    throw error;
  }
  const approvalKey = walletApprovalKey(method, params, current, pageUrl);
  if (wallet.pending_approval_keys.has(approvalKey)) {
    const error = new Error("This wallet request is already waiting in ElastOS Wallet/Inbox.");
    error.code = 4100;
    throw error;
  }
  wallet.pending_approval_keys.add(approvalKey);
  let response;
  try {
    response = await fetch(wallet.approval_url, {
      method: "POST",
      headers: {
        "content-type": "application/json",
        "x-elastos-home-token": wallet.home_token,
      },
      body: JSON.stringify({
        method,
        params,
        account_id: current.account_id,
        chain_namespace: current.chain_namespace,
        address: current.address,
        page_url: pageUrl,
        origin: originFromUrl(pageUrl),
      }),
    });
  } catch (error) {
    wallet.pending_approval_keys.delete(approvalKey);
    throw error;
  }
  const text = await response.text();
  let body = null;
  if (text.trim() !== "") {
    try {
      body = JSON.parse(text);
    } catch {
      body = { message: text };
    }
  }
  if (!response.ok) {
    wallet.pending_approval_keys.delete(approvalKey);
    const error = new Error(body?.message || body?.error || `Runtime wallet approval failed: ${response.status}`);
    error.code = 4100;
    throw error;
  }
  const requestId = body?.approval_request?.request_id;
  if (typeof requestId !== "string" || requestId.trim() === "") {
    wallet.pending_approval_keys.delete(approvalKey);
    const error = new Error("Runtime wallet approval response did not include a request id.");
    error.code = 4100;
    throw error;
  }
  try {
    return await waitForWalletApprovalSignature(wallet, requestId);
  } finally {
    wallet.pending_approval_keys.delete(approvalKey);
  }
}

async function requestRuntimeWalletTransaction(wallet, source, method, params, current) {
  if (!current) {
    const error = new Error("No ElastOS Wallet account is available for this Browser page.");
    error.code = 4100;
    throw error;
  }
  if (!wallet.transaction_url || !wallet.transaction_broadcast_url || !wallet.approval_status_url || !wallet.home_token) {
    const error = new Error("Runtime wallet transaction approval is unavailable for this Browser page.");
    error.code = 4100;
    throw error;
  }
  const pageUrl = typeof source?.frame?.url === "function" ? source.frame.url() : "";
  if (!pageUrl || !/^https?:\/\//.test(pageUrl)) {
    const error = new Error("Runtime wallet approval requires an http or https Browser page.");
    error.code = 4100;
    throw error;
  }
  const approvalKey = walletApprovalKey(method, params, current, pageUrl);
  if (wallet.pending_approval_keys.has(approvalKey)) {
    const error = new Error("This wallet request is already waiting in ElastOS Wallet/Inbox.");
    error.code = 4100;
    throw error;
  }
  wallet.pending_approval_keys.add(approvalKey);
  let response;
  try {
    response = await fetch(wallet.transaction_url, {
      method: "POST",
      headers: {
        "content-type": "application/json",
        "x-elastos-home-token": wallet.home_token,
      },
      body: JSON.stringify({
        method,
        params,
        account_id: current.account_id,
        chain_namespace: current.chain_namespace,
        address: current.address,
        page_url: pageUrl,
        origin: originFromUrl(pageUrl),
      }),
    });
  } catch (error) {
    wallet.pending_approval_keys.delete(approvalKey);
    throw error;
  }
  const text = await response.text();
  let body = null;
  if (text.trim() !== "") {
    try {
      body = JSON.parse(text);
    } catch {
      body = { message: text };
    }
  }
  if (!response.ok) {
    wallet.pending_approval_keys.delete(approvalKey);
    const error = new Error(body?.message || body?.error || `Runtime wallet transaction approval failed: ${response.status}`);
    error.code = 4100;
    throw error;
  }
  const requestId = body?.approval_request?.request_id;
  if (typeof requestId !== "string" || requestId.trim() === "") {
    wallet.pending_approval_keys.delete(approvalKey);
    const error = new Error("Runtime wallet transaction approval response did not include a request id.");
    error.code = 4100;
    throw error;
  }
  try {
    return await waitForWalletApprovalTransaction(wallet, requestId);
  } finally {
    wallet.pending_approval_keys.delete(approvalKey);
  }
}

function walletApprovalKey(method, params, account, pageUrl) {
  return createHash("sha256")
    .update(JSON.stringify({
      method,
      params,
      account_id: account.account_id,
      chain_namespace: account.chain_namespace,
      page_url: pageUrl,
    }))
    .digest("hex");
}

async function waitForWalletApprovalSignature(wallet, requestId) {
  const deadline = Date.now() + 5 * 60 * 1000;
  while (Date.now() < deadline) {
    const status = await fetchWalletApprovalStatus(wallet, requestId);
    if (status.status === "completed") {
      if (typeof status.signature !== "string" || status.signature.trim() === "") {
        const error = new Error("Runtime wallet approval completed without a signature.");
        error.code = 4100;
        throw error;
      }
      return status.signature;
    }
    if (status.status === "rejected") {
      const error = new Error("Wallet request was rejected in ElastOS Wallet/Inbox.");
      error.code = 4001;
      throw error;
    }
    if (status.status === "expired") {
      const error = new Error("Wallet request expired before approval.");
      error.code = 4001;
      throw error;
    }
    await sleep(1000);
  }
  const error = new Error("Wallet request timed out before approval.");
  error.code = 4001;
  throw error;
}

async function waitForWalletApprovalTransaction(wallet, requestId) {
  const deadline = Date.now() + 5 * 60 * 1000;
  while (Date.now() < deadline) {
    const status = await fetchWalletApprovalStatus(wallet, requestId);
    if (status.status === "completed") {
      if (typeof status.transaction_hash === "string" && status.transaction_hash.trim() !== "" && !status.signed_transaction) {
        return status.transaction_hash;
      }
      if (typeof status.signed_transaction !== "string" || status.signed_transaction.trim() === "") {
        const error = new Error("Runtime wallet approval completed without a signed transaction.");
        error.code = 4100;
        throw error;
      }
      const receipt = await broadcastWalletTransaction(wallet, requestId);
      const hash = receipt?.transaction_hash;
      if (typeof hash !== "string" || hash.trim() === "") {
        const error = new Error("Runtime transaction broadcast did not return a transaction hash.");
        error.code = 4100;
        throw error;
      }
      return hash;
    }
    if (status.status === "rejected") {
      const error = new Error("Wallet request was rejected in ElastOS Wallet/Inbox.");
      error.code = 4001;
      throw error;
    }
    if (status.status === "expired") {
      const error = new Error("Wallet request expired before approval.");
      error.code = 4001;
      throw error;
    }
    await sleep(1000);
  }
  const error = new Error("Wallet request timed out before approval.");
  error.code = 4001;
  throw error;
}

async function broadcastWalletTransaction(wallet, requestId) {
  const response = await fetch(wallet.transaction_broadcast_url, {
    method: "POST",
    headers: {
      "content-type": "application/json",
      "x-elastos-home-token": wallet.home_token,
    },
    body: JSON.stringify({ request_id: requestId }),
  });
  const text = await response.text();
  let body = null;
  if (text.trim() !== "") {
    try {
      body = JSON.parse(text);
    } catch {
      body = { message: text };
    }
  }
  if (!response.ok) {
    const error = new Error(body?.message || body?.error || `Runtime transaction broadcast failed: ${response.status}`);
    error.code = 4100;
    throw error;
  }
  return body || {};
}

async function fetchWalletApprovalStatus(wallet, requestId) {
  const response = await fetch(`${wallet.approval_status_url}/${encodeURIComponent(requestId)}`, {
    headers: {
      "x-elastos-home-token": wallet.home_token,
    },
  });
  const text = await response.text();
  let body = null;
  if (text.trim() !== "") {
    try {
      body = JSON.parse(text);
    } catch {
      body = { message: text };
    }
  }
  if (!response.ok) {
    const error = new Error(body?.message || body?.error || `Runtime wallet approval status failed: ${response.status}`);
    error.code = 4100;
    throw error;
  }
  return body || {};
}

function originFromUrl(value) {
  try {
    return new URL(value).origin;
  } catch {
    return null;
  }
}

function chainNamespaceToDecimal(namespace) {
  const [, value] = String(namespace || "").split(":");
  const parsed = Number(value || 20);
  return Number.isFinite(parsed) ? parsed : 20;
}

function chainNamespaceToHex(namespace) {
  return `0x${chainNamespaceToDecimal(namespace).toString(16)}`;
}

function walletInitScript(wallet) {
  const current =
    wallet.accounts.find((account) => account.chain_namespace === wallet.default_chain_namespace) || wallet.accounts[0] || null;
  const initialState = {
    chainId: chainNamespaceToHex(wallet.default_chain_namespace),
    selectedAddress: current?.address || null,
  };
  return `
(() => {
  const initial = ${JSON.stringify(initialState)};
  const listeners = new Map();
  const emit = (event, payload) => {
    for (const handler of listeners.get(event) || []) {
      try { handler(payload); } catch {}
    }
  };
  const refreshIdentity = async () => {
    const [accounts, chainId] = await Promise.all([
      window.__elastosWalletRequest({ method: "eth_accounts", params: [] }),
      window.__elastosWalletRequest({ method: "eth_chainId", params: [] })
    ]);
    provider.selectedAddress = Array.isArray(accounts) && accounts.length ? accounts[0] : null;
    provider.chainId = chainId;
    provider.networkVersion = String(parseInt(chainId, 16));
    return accounts;
  };
  const request = async (payload) => {
    const result = await window.__elastosWalletRequest({
    method: payload && payload.method,
    params: payload && payload.params
    });
    if (payload && (payload.method === "eth_requestAccounts" || payload.method === "eth_accounts")) {
      provider.selectedAddress = Array.isArray(result) && result.length ? result[0] : null;
      emit("accountsChanged", result);
    }
    if (payload && payload.method === "eth_chainId") {
      provider.chainId = result;
      provider.networkVersion = String(parseInt(result, 16));
    }
    if (payload && payload.method === "wallet_switchEthereumChain") {
      const accounts = await refreshIdentity();
      emit("chainChanged", provider.chainId);
      emit("accountsChanged", accounts);
    }
    if (payload && payload.method === "eth_requestAccounts") {
      emit("connect", { chainId: provider.chainId });
    }
    return result;
  };
  const provider = {
    isElastOS: true,
    isMetaMask: true,
    isConnected: () => true,
    request,
    selectedAddress: initial.selectedAddress,
    chainId: initial.chainId,
    networkVersion: String(parseInt(initial.chainId, 16)),
    on(event, handler) {
      if (typeof handler !== "function") return this;
      const handlers = listeners.get(event) || [];
      handlers.push(handler);
      listeners.set(event, handlers);
      return this;
    },
    removeListener(event, handler) {
      const handlers = listeners.get(event) || [];
      listeners.set(event, handlers.filter((item) => item !== handler));
      return this;
    },
    enable: async () => request({ method: "eth_requestAccounts" }),
    _metamask: { isUnlocked: async () => true }
  };
  Object.defineProperty(window, "ethereum", {
    value: provider,
    configurable: false,
    enumerable: false,
    writable: false
  });
  window.dispatchEvent(new Event("ethereum#initialized"));
})();
`;
}

async function startRuntimeProxy(config) {
  const server = http.createServer((req, res) => {
    handleProxyHttpRequest(config, req, res).catch((error) => {
      failProxyResponse(res, 502, error.message || "browser proxy request failed");
    });
  });
  server.on("connect", (req, clientSocket, head) => {
    handleProxyConnect(config, req, clientSocket, head).catch((error) => {
      failProxySocket(clientSocket, 502, error.message || "browser proxy tunnel failed");
    });
  });
  server.on("upgrade", (req, clientSocket, head) => {
    handleProxyUpgrade(config, req, clientSocket, head).catch((error) => {
      failProxySocket(clientSocket, 502, error.message || "browser proxy upgrade failed");
    });
  });
  server.on("clientError", (_error, socket) => {
    failProxySocket(socket, 400, "bad browser proxy request");
  });
  await listenLocalhost(server);
  const address = server.address();
  if (!address || typeof address !== "object" || !Number.isInteger(address.port)) {
    throw new Error("browser runtime proxy did not bind a TCP port");
  }
  return {
    server,
    host: "127.0.0.1",
    port: address.port,
    url: `http://127.0.0.1:${address.port}`,
  };
}

async function listenLocalhost(server) {
  await new Promise((resolve, reject) => {
    const onError = (error) => {
      server.off("listening", onListening);
      reject(error);
    };
    const onListening = () => {
      server.off("error", onError);
      resolve();
    };
    server.once("error", onError);
    server.once("listening", onListening);
    server.listen(0, "127.0.0.1");
  });
}

async function closeRuntimeProxy(runtimeProxy) {
  await new Promise((resolve) => runtimeProxy.server.close(resolve));
}

function publicRuntimeProxyStatus(runtimeProxy) {
  if (!runtimeProxy) {
    return null;
  }
  return {
    mode: "http_connect",
    host: runtimeProxy.host,
    port: runtimeProxy.port,
    direct_network: false,
  };
}

async function handleProxyConnect(config, req, clientSocket, head) {
  if (!config.allowed_protocols.includes("https")) {
    throw new Error("https is not allowlisted for browser exit");
  }
  const target = parseProxyAuthority(req.url, 443);
  const remote = await openExitRelay(config, {
    ...target,
    streamId: `browser:connect:${hash64(req.url || "")}`,
    reason: "browser HTTPS tunnel",
  });
  clientSocket.write("HTTP/1.1 200 Connection Established\r\nProxy-Agent: ElastOS Runtime Browser\r\n\r\n");
  if (head?.length) {
    remote.write(head);
  }
  pipeSocketPair(clientSocket, remote);
}

async function handleProxyUpgrade(config, req, clientSocket, head) {
  if (!config.allowed_protocols.includes("http")) {
    throw new Error("http is not allowlisted for browser exit");
  }
  const target = parseProxyRequestTarget(req, true);
  const remote = await openExitRelay(config, {
    host: target.host,
    port: target.port,
    streamId: `browser:upgrade:${hash64(`${req.method}:${req.url || ""}`)}`,
    reason: "browser WebSocket upgrade",
  });
  remote.write(serializeRawRequest(req, target.path));
  if (head?.length) {
    remote.write(head);
  }
  pipeSocketPair(clientSocket, remote);
}

async function handleProxyHttpRequest(config, req, res) {
  if (!config.allowed_protocols.includes("http")) {
    throw new Error("http is not allowlisted for browser exit");
  }
  const target = parseProxyRequestTarget(req, false);
  const proxyReq = http.request(
    {
      host: target.host,
      port: target.port,
      method: req.method,
      path: target.path,
      headers: sanitizeRequestHeaders(req.headers),
      timeout: 30000,
      createConnection: (_options, callback) => {
        openExitRelay(config, {
          host: target.host,
          port: target.port,
          streamId: `browser:http:${hash64(`${req.method}:${req.url || ""}`)}`,
          reason: "browser HTTP request",
        })
          .then((socket) => callback(null, socket))
          .catch((error) => callback(error));
      },
    },
    (proxyRes) => {
      res.writeHead(proxyRes.statusCode || 502, sanitizeResponseHeaders(proxyRes.headers));
      proxyRes.pipe(res);
    },
  );
  proxyReq.on("timeout", () => proxyReq.destroy(new Error("browser proxy request timed out")));
  proxyReq.on("error", (error) => {
    failProxyResponse(res, 502, error.message || "browser proxy request failed");
  });
  req.pipe(proxyReq);
}

async function openExitRelay(config, target) {
  if (!config.allowed_ports.includes(Number(target.port))) {
    throw new Error(`port is not allowlisted for browser exit: ${target.port}`);
  }
  if (!hostAllowed(target.host, config.allowed_hosts)) {
    throw new Error(`host is not allowlisted for browser exit: ${target.host}`);
  }
  const raw = net.createConnection(config.local_exit_socket_path);
  await once(raw, "connect");
  raw.write(
    `${JSON.stringify({
      schema: "elastos.exit.relay-open/v1",
      stream_id: safeId(target.streamId),
      target: `tcp://${target.host}:${target.port}`,
      host: target.host,
      scheme: "tcp",
      principal_id: null,
      reason: target.reason,
    })}\n`,
  );
  return raw;
}

function parseProxyAuthority(authority, defaultPort) {
  const value = String(authority || "").trim();
  if (!value || /[\s/\\\0]/.test(value)) {
    throw new Error("browser proxy target authority is invalid");
  }
  const parsed = new URL(`tcp://${value}`);
  const port = Number(parsed.port || defaultPort);
  if (!Number.isInteger(port) || port < 1 || port > 65535) {
    throw new Error("browser proxy target port is invalid");
  }
  return {
    host: parsed.hostname,
    port,
  };
}

function parseProxyRequestTarget(req, allowWebSocket) {
  const rawUrl = String(req.url || "/");
  const hostHeader = String(req.headers.host || "");
  let parsed;
  if (/^[A-Za-z][A-Za-z0-9+.-]*:\/\//.test(rawUrl)) {
    parsed = new URL(rawUrl);
  } else {
    if (!hostHeader) {
      throw new Error("browser proxy request is missing Host");
    }
    parsed = new URL(`http://${hostHeader}${rawUrl.startsWith("/") ? rawUrl : `/${rawUrl}`}`);
  }
  if (parsed.protocol === "https:") {
    throw new Error("HTTPS browser proxy requests must use CONNECT");
  }
  if (!["http:", "ws:"].includes(parsed.protocol) || (parsed.protocol === "ws:" && !allowWebSocket)) {
    throw new Error(`unsupported browser proxy protocol: ${parsed.protocol}`);
  }
  const port = Number(parsed.port || 80);
  if (!Number.isInteger(port) || port < 1 || port > 65535) {
    throw new Error("browser proxy request port is invalid");
  }
  return {
    host: parsed.hostname,
    port,
    path: `${parsed.pathname || "/"}${parsed.search || ""}`,
  };
}

function serializeRawRequest(req, pathName) {
  const lines = [`${req.method || "GET"} ${pathName || "/"} HTTP/${req.httpVersion || "1.1"}`];
  for (const [key, value] of Object.entries(sanitizeRequestHeaders(req.headers))) {
    if (Array.isArray(value)) {
      for (const item of value) {
        lines.push(`${key}: ${item}`);
      }
    } else if (value !== undefined) {
      lines.push(`${key}: ${value}`);
    }
  }
  return `${lines.join("\r\n")}\r\n\r\n`;
}

function pipeSocketPair(left, right) {
  left.on("error", () => right.destroy());
  right.on("error", () => left.destroy());
  left.on("close", () => right.destroy());
  right.on("close", () => left.destroy());
  left.pipe(right);
  right.pipe(left);
}

function failProxyResponse(res, status, message) {
  if (res.headersSent) {
    res.destroy();
    return;
  }
  const body = `${message}\n`;
  res.writeHead(status, {
    "content-type": "text/plain; charset=utf-8",
    "content-length": Buffer.byteLength(body),
    "cache-control": "no-store",
  });
  res.end(body);
}

function failProxySocket(socket, status, message) {
  if (!socket || socket.destroyed) {
    return;
  }
  const body = `${message}\n`;
  socket.end(
    `HTTP/1.1 ${status} ${status === 400 ? "Bad Request" : "Bad Gateway"}\r\n` +
      "content-type: text/plain; charset=utf-8\r\n" +
      `content-length: ${Buffer.byteLength(body)}\r\n` +
      "connection: close\r\n" +
      "\r\n" +
      body,
  );
}

function sanitizeRequestHeaders(headers) {
  const output = {};
  for (const [key, value] of Object.entries(headers || {})) {
    const lower = key.toLowerCase();
    if (
      [
        "connection",
        "keep-alive",
        "proxy-authenticate",
        "proxy-authorization",
        "proxy-connection",
        "te",
        "trailers",
        "transfer-encoding",
      ].includes(lower)
    ) {
      continue;
    }
    output[key] = value;
  }
  return output;
}

function sanitizeResponseHeaders(headers) {
  const output = {};
  for (const [key, value] of Object.entries(headers || {})) {
    const lower = key.toLowerCase();
    if (["connection", "proxy-connection", "transfer-encoding", "upgrade"].includes(lower)) {
      continue;
    }
    if (Array.isArray(value)) {
      output[key] = value.join(", ");
    } else if (value != null) {
      output[key] = String(value);
    }
  }
  return output;
}

async function applyInputEvent(pageState, event) {
  const page = pageState.page;
  if (event.type === "browser_command") {
    const command = String(event.command || "");
    const options = { waitUntil: "domcontentloaded", timeout: 15000 };
    if (command === "back") {
      await page.goBack(options).catch(() => null);
      return;
    }
    if (command === "forward") {
      await page.goForward(options).catch(() => null);
      return;
    }
    if (command === "reload") {
      await page.reload(options).catch(() => null);
      return;
    }
    throw new Error("unsupported browser command");
  }
  if (event.type === "resize") {
    const viewport = normalizeViewport(event.viewport, pageState.viewport);
    await page.setViewportSize(viewport);
    pageState.viewport = viewport;
    return;
  }
  if (event.type === "click") {
    await page.mouse.click(Number(event.x || 0), Number(event.y || 0));
    return;
  }
  if (event.type === "wheel") {
    await page.mouse.wheel(Number(event.delta_x || 0), Number(event.delta_y || 0));
    return;
  }
  if (event.type === "text") {
    const text = String(event.text || "").slice(0, 4096);
    if (text) {
      await page.keyboard.type(text);
    }
    return;
  }
  if (event.type === "key") {
    const namedKeys = new Set([
      "Backspace",
      "Delete",
      "Enter",
      "Escape",
      "Tab",
      "ArrowUp",
      "ArrowDown",
      "ArrowLeft",
      "ArrowRight",
      "Home",
      "End",
      "PageUp",
      "PageDown",
      " ",
    ]);
    if (namedKeys.has(event.key)) {
      await page.keyboard.press(event.key === " " ? "Space" : event.key);
      return;
    }
    if (typeof event.key === "string" && event.key.length === 1) {
      await page.keyboard.type(event.key);
    }
  }
}

async function readJsonBody(req) {
  const chunks = [];
  let total = 0;
  for await (const chunk of req) {
    total += chunk.length;
    if (total > 1024 * 1024) {
      throw new Error("request body too large");
    }
    chunks.push(chunk);
  }
  if (!chunks.length) {
    return {};
  }
  return JSON.parse(Buffer.concat(chunks).toString("utf8"));
}

function sendJson(res, status, payload) {
  const body = JSON.stringify(payload);
  res.writeHead(status, {
    "content-type": "application/json",
    "content-length": Buffer.byteLength(body),
    "cache-control": "no-store",
  });
  res.end(body);
}

async function controlRequest(socketPath, method, route, body = null, timeoutMs = 10000) {
  return await new Promise((resolve, reject) => {
    let req;
    const timer = setTimeout(() => {
      req?.destroy(new Error("control request timed out"));
    }, timeoutMs);
    req = http.request(
      {
        socketPath,
        path: route,
        method,
        headers: body ? { "content-type": "application/json" } : {},
      },
      (res) => {
        const chunks = [];
        res.on("data", (chunk) => chunks.push(chunk));
        res.on("end", () => {
          clearTimeout(timer);
          const text = Buffer.concat(chunks).toString("utf8");
          let payload = {};
          try {
            payload = text ? JSON.parse(text) : {};
          } catch (error) {
            reject(new Error(`invalid control response JSON: ${error.message}`));
            return;
          }
          if ((res.statusCode || 500) >= 400) {
            reject(new Error(payload.error || "control request failed"));
          } else {
            resolve(payload);
          }
        });
      },
    );
    req.on("error", (error) => {
      clearTimeout(timer);
      reject(error);
    });
    if (body) {
      req.write(JSON.stringify(body));
    }
    req.end();
  });
}

async function unlinkSocket(socketPath) {
  try {
    const stat = await fs.promises.lstat(socketPath);
    if (stat.isSocket()) {
      await fs.promises.unlink(socketPath);
    }
  } catch (error) {
    if (error.code !== "ENOENT") {
      throw error;
    }
  }
}

function hostAllowed(host, allowedHosts) {
  const lower = host.toLowerCase();
  return allowedHosts.some((entry) => {
    const normalized = String(entry || "").toLowerCase();
    if (normalized === "*") {
      return true;
    }
    if (normalized.startsWith("*.")) {
      const suffix = normalized.slice(1);
      return lower.endsWith(suffix);
    }
    return lower === normalized;
  });
}

function newPageId(streamId) {
  const nonce = randomBytes(8).toString("hex");
  return `page:${hash64(`${safeId(streamId)}:${Date.now()}:${nonce}`)}`;
}

function hash64(value) {
  let hash = 0xcbf29ce484222325n;
  for (const byte of Buffer.from(String(value))) {
    hash ^= BigInt(byte);
    hash = BigInt.asUintN(64, hash * 0x100000001b3n);
  }
  return hash.toString(16).padStart(16, "0");
}

function safeId(value) {
  return String(value || "stream:browser")
    .replace(/[^A-Za-z0-9:_-]/g, "_")
    .slice(0, 180);
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}
