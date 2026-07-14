#!/usr/bin/env node
import crypto from "node:crypto";
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import http from "node:http";
import net from "node:net";
import os from "node:os";
import path from "node:path";
import process from "node:process";
import tls from "node:tls";
import { URL } from "node:url";

const CONFIG_ENV = "ELASTOS_BROWSER_SELKIES_CONTROL_CONFIG";
const HOSTED_PRODUCT_OPEN_SCHEMA = "elastos.browser.hosted-product.open/v1";
const VM_GUEST_OPEN_SCHEMA = "elastos.browser.vm-guest.open/v1";
const VM_LOG_DIR = "/var/log/elastos";
const VM_LOG_NAMES = [
  "browser-vm-initrd.log",
  "browser-vm-rootfs-entry.log",
  "browser-vm-init.log",
  "browser-vm-selkies-control.log",
  "browser-vm-xvfb.log",
  "browser-vm-native-proxy.log",
  "browser-vm-chromium.log",
  "browser-vm-selkies.log",
  "browser-vm-pipewire.log",
  "browser-vm-wireplumber.log",
  "browser-vm-wireplumber-config.log",
  "browser-vm-pipewire-pulse.log",
  "browser-vm-pipewire-null-sink.log",
  "browser-vm-pipewire-summary.log",
  "browser-vm-pipewire-dump.log",
];
const MAX_WEBSOCKET_FRAME_BYTES = 16 * 1024 * 1024;
const MAX_BROWSER_FILE_UPLOAD_BYTES = 16 * 1024 * 1024;
const WALLET_RUNTIME_BINDING = "__elastosBrowserWalletRuntime";
const WALLET_RUNTIME_RESULT = "__elastosBrowserWalletRuntimeResult";
const WALLET_RUNTIME_POST_URL_FIELDS = {
  approval: "approval_url",
  read: "read_url",
  transaction: "transaction_url",
  transactionBroadcast: "transaction_broadcast_url",
};

function serialLogLine(line) {
  const target = process.env.ELASTOS_BROWSER_VM_SERIAL_LOG_DEV || "";
  if (!target) return;
  try {
    fs.appendFileSync(target, `${line}\n`);
  } catch {}
}

function errorDetails(error) {
  if (error instanceof Error) {
    return {
      message: error.message,
      stack: error.stack || null,
    };
  }
  return {
    message: String(error),
    stack: null,
  };
}

function logFatalError(kind, error) {
  console.error(JSON.stringify({
    schema: "elastos.browser.selkies-control.fatal/v1",
    kind,
    ...errorDetails(error),
  }));
}

function logControlEvent(kind, fields = {}) {
  const line = JSON.stringify({
    schema: "elastos.browser.selkies-control.event/v1",
    kind,
    at: nowIso(),
    ...fields,
  });
  console.error(line);
  serialLogLine(line);
}

process.on("uncaughtException", (error) => {
  logFatalError("uncaught_exception", error);
  process.exit(1);
});

process.on("unhandledRejection", (reason) => {
  logFatalError("unhandled_rejection", reason);
  process.exit(1);
});

function fail(message) {
  console.error(message);
  process.exit(1);
}

function safeId(value) {
  return typeof value === "string" && /^[A-Za-z0-9:_-]+$/.test(value);
}

function readConfig() {
  const raw = process.env[CONFIG_ENV];
  if (!raw) {
    fail(`${CONFIG_ENV} is required`);
  }
  let config;
  try {
    config = JSON.parse(raw);
  } catch (error) {
    fail(`${CONFIG_ENV} is invalid JSON: ${error.message}`);
  }
  if (config.schema !== "elastos.browser.selkies-control.config/v1") {
    fail("unsupported Selkies control config schema");
  }
  if (typeof config.control_socket_path !== "string" || !config.control_socket_path.startsWith("/") || /[\s\0]/.test(config.control_socket_path)) {
    fail("control_socket_path must be an absolute Unix socket path without whitespace");
  }
  const wsUrl = new URL(config.selkies_ws_url || "");
  if (!["ws:", "wss:"].includes(wsUrl.protocol)) {
    fail("selkies_ws_url must use ws or wss");
  }
  const browserControl = readBrowserControlConfig(config.browser_control);
  const basicAuth = readBasicAuthConfig(config.basic_auth);
  const iceServers = readIceServersConfig(config.ice_servers);
  const displaySurface = readDisplaySurfaceConfig(config.display_surface);
  const runtimeFetchProxyUrl = readRuntimeFetchProxyConfig(config.runtime_fetch_proxy_url);
  const signalingProtocol = config.signaling_protocol || "auto";
  if (!["legacy", "current", "auto"].includes(signalingProtocol)) {
    fail("signaling_protocol must be legacy, current, or auto");
  }
  return {
    schema: config.schema,
    controlSocketPath: config.control_socket_path,
    replaceExistingSocket: config.replace_existing_socket === true,
    selkiesWsUrl: wsUrl,
    signalingProtocol,
    browserControl,
    adapterId: config.adapter_id || "hosted-product",
    connectTimeoutMs: numberOr(config.connect_timeout_ms, 10_000),
    signalTimeoutMs: numberOr(config.signal_timeout_ms, 10_000),
    sessionCooldownMs: numberOr(config.session_cooldown_ms, 1_500),
    stackReadyTimeoutMs: numberOr(config.stack_ready_timeout_ms, 90_000),
    pageOpenTimeoutMs: numberOr(config.page_open_timeout_ms, 105_000),
    basicAuth,
    iceServers,
    displaySurface,
    runtimeFetchProxyUrl,
  };
}

function readBrowserControlConfig(value) {
  if (!value || value.kind !== "cdp_http") {
    fail("browser_control.kind=cdp_http is required");
  }
  const endpoint = new URL(value.endpoint || "");
  if (!["http:", "https:"].includes(endpoint.protocol)) {
    fail("browser_control.endpoint must use http or https");
  }
  if (!["127.0.0.1", "::1", "localhost"].includes(endpoint.hostname)) {
    fail("browser_control.endpoint must be loopback/private to the operator service");
  }
  return {
    kind: "cdp_http",
    endpoint,
    timeoutMs: numberOr(value.timeout_ms, 5_000),
  };
}

function readBasicAuthConfig(value) {
  if (value == null) {
    return null;
  }
  if (typeof value !== "object" || Array.isArray(value)) {
    fail("basic_auth must be an object when provided");
  }
  const user = value.user;
  const password = value.password;
  if (typeof user !== "string" || user.length === 0 || /[\r\n\0]/.test(user)) {
    fail("basic_auth.user must be a non-empty string without control characters");
  }
  if (typeof password !== "string" || password.length === 0 || /[\r\n\0]/.test(password)) {
    fail("basic_auth.password must be a non-empty string without control characters");
  }
  return { user, password };
}

function readIceServersConfig(value) {
  if (value == null) {
    return [];
  }
  if (!Array.isArray(value)) {
    fail("ice_servers must be an array when provided");
  }
  if (value.length > 8) {
    fail("ice_servers may contain at most 8 entries");
  }
  return value.map((entry, index) => readIceServerConfig(entry, index));
}

function readDisplaySurfaceConfig(value) {
  const streamWidth = numberOr(value?.stream_width, 1920);
  const streamHeight = numberOr(value?.stream_height, 1080);
  const cssWidth = numberOr(value?.css_width, streamWidth);
  const cssHeight = numberOr(value?.css_height, streamHeight);
  if (streamWidth < 640 || streamWidth > 3840 || streamHeight < 360 || streamHeight > 2160) {
    fail("display_surface stream size must be within 640x360 and 3840x2160");
  }
  if (cssWidth < 320 || cssWidth > 3840 || cssHeight < 240 || cssHeight > 2160) {
    fail("display_surface CSS viewport must be within 320x240 and 3840x2160");
  }
  return {
    stream: { width: streamWidth, height: streamHeight },
    css: { width: cssWidth, height: cssHeight },
    deviceScaleFactor: streamWidth / cssWidth,
  };
}

function readRuntimeFetchProxyConfig(value) {
  if (value == null || value === "") {
    return null;
  }
  if (typeof value !== "string" || /[\r\n\0]/.test(value)) {
    fail("runtime_fetch_proxy_url must be a URL without control characters");
  }
  let proxyUrl;
  try {
    proxyUrl = new URL(value);
  } catch {
    fail("runtime_fetch_proxy_url must be a valid URL");
  }
  if (proxyUrl.protocol !== "http:") {
    fail("runtime_fetch_proxy_url must use http");
  }
  if (!["127.0.0.1", "::1", "localhost"].includes(proxyUrl.hostname)) {
    fail("runtime_fetch_proxy_url must be loopback/private to the Browser VM");
  }
  if (proxyUrl.username || proxyUrl.password || proxyUrl.pathname !== "/" || proxyUrl.search || proxyUrl.hash) {
    fail("runtime_fetch_proxy_url must not include credentials, path, query, or fragment");
  }
  return proxyUrl;
}

function mediaKindsForSdp(sdp) {
  const text = typeof sdp === "string" ? sdp : "";
  return {
    audio: /(?:^|\r?\n)m=audio\s/.test(text),
    video: /(?:^|\r?\n)m=video\s/.test(text),
  };
}

function normalizeAudioOfferSdp(sdp) {
  return String(sdp || "")
    .split(/\r?\n/)
    .map((line) => {
      const match = line.match(/^a=rtpmap:(\d+)\s+opus\/48000$/i);
      return match ? `a=rtpmap:${match[1]} opus/48000/2` : line;
    })
    .join("\r\n");
}

function publicDisplaySession(displaySession) {
  const iceServers = Array.isArray(displaySession?.ice_servers)
    ? displaySession.ice_servers
    : [];
  return {
    schema: displaySession?.schema || null,
    session_id: displaySession?.session_id || null,
    mode: displaySession?.mode || null,
    width: displaySession?.width || null,
    height: displaySession?.height || null,
    input: displaySession?.input || null,
    input_protocol: displaySession?.input_protocol || null,
    offerer: displaySession?.offerer || null,
    display_backend: displaySession?.display_backend || null,
    backend_class: displaySession?.backend_class || null,
    media_transport: displaySession?.media_transport || null,
    audio: displaySession?.audio === true,
    video: displaySession?.video === true,
    network_mode: displaySession?.network_mode || null,
    direct_network: displaySession?.direct_network === true,
    signaling_url: displaySession?.signaling_url || null,
    ice_servers: iceServers.map((server) => ({
      urls: Array.isArray(server?.urls) ? server.urls : [server?.urls].filter(Boolean),
      username_present: typeof server?.username === "string" && server.username.trim() !== "",
      credential_present: typeof server?.credential === "string" && server.credential !== "",
      credential_length: typeof server?.credential === "string" ? server.credential.length : 0,
    })),
  };
}

function readIceServerConfig(value, index) {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    fail(`ice_servers[${index}] must be an object`);
  }
  const urls = readIceUrls(value.urls, index);
  const server = { urls };
  if (value.username != null) {
    if (typeof value.username !== "string" || value.username.length === 0 || /[\r\n\0]/.test(value.username)) {
      fail(`ice_servers[${index}].username must be a non-empty string without control characters`);
    }
    server.username = value.username;
  }
  if (value.credential != null) {
    if (typeof value.credential !== "string" || value.credential.length === 0 || /[\r\n\0]/.test(value.credential)) {
      fail(`ice_servers[${index}].credential must be a non-empty string without control characters`);
    }
    server.credential = value.credential;
  }
  return server;
}

function readIceUrls(value, index) {
  const urls = Array.isArray(value) ? value : [value];
  if (urls.length === 0 || urls.length > 8) {
    fail(`ice_servers[${index}].urls must contain 1..8 URLs`);
  }
  return urls.map((url, urlIndex) => {
    if (typeof url !== "string") {
      fail(`ice_servers[${index}].urls[${urlIndex}] must be a string`);
    }
    const trimmed = url.trim();
    if (!/^(stun|turns?):/i.test(trimmed) || /[\r\n\0]/.test(trimmed) || trimmed.length > 512) {
      fail(`ice_servers[${index}].urls[${urlIndex}] must be a stun:, turn:, or turns: URL without control characters`);
    }
    return trimmed;
  });
}

function numberOr(value, defaultValue) {
  return Number.isInteger(value) && value > 0 ? value : defaultValue;
}

function gcd(left, right) {
  let a = Math.abs(left);
  let b = Math.abs(right);
  while (b) {
    const next = a % b;
    a = b;
    b = next;
  }
  return a || 1;
}

function aspectPreservingDisplaySize(requested, config) {
  const ratioBase = gcd(
    config.displaySurface.stream.width,
    config.displaySurface.stream.height,
  );
  const unitWidth = config.displaySurface.stream.width / ratioBase;
  const unitHeight = config.displaySurface.stream.height / ratioBase;
  const minScale = Math.max(
    Math.ceil(320 / unitWidth),
    Math.ceil(240 / unitHeight),
    1,
  );
  const maxScale = Math.min(
    Math.floor(3840 / unitWidth),
    Math.floor(2160 / unitHeight),
  );
  const requestedScale = Math.min(
    Math.floor(requested.width / unitWidth),
    Math.floor(requested.height / unitHeight),
  );
  const scale = Math.max(minScale, Math.min(maxScale, requestedScale));
  return {
    width: unitWidth * scale,
    height: unitHeight * scale,
  };
}

function validateLaunchViewport(launch) {
  const viewport = launch?.viewport;
  if (viewport == null) {
    return;
  }
  if (
    !Number.isInteger(viewport.width) ||
    !Number.isInteger(viewport.height) ||
    viewport.width < 320 ||
    viewport.width > 3840 ||
    viewport.height < 240 ||
    viewport.height > 2160
  ) {
    throw new Error("launch viewport must be within 320x240 and 3840x2160");
  }
}

function displaySizeForLaunch(launch, config) {
  validateLaunchViewport(launch);
  if (launch?.viewport) {
    return aspectPreservingDisplaySize(launch.viewport, config);
  }
  return aspectPreservingDisplaySize(config.displaySurface.css, config);
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function withTimeout(label, timeoutMs, promise) {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      reject(new Error(`${label} timed out after ${timeoutMs} ms`));
    }, timeoutMs);
    Promise.resolve(promise)
      .then(resolve, reject)
      .finally(() => clearTimeout(timer));
  });
}

function createBrowserFileChooserState() {
  return {
    pending: null,
    events: [],
    serial: 0,
  };
}

function browserFileChooserState(browserPage) {
  if (!browserPage.file_chooser) {
    browserPage.file_chooser = createBrowserFileChooserState();
  }
  return browserPage.file_chooser;
}

function cleanupBrowserUploadTempFiles(browserPage) {
  const files = Array.isArray(browserPage?.upload_temp_files)
    ? browserPage.upload_temp_files.splice(0)
    : [];
  for (const file of files) {
    try {
      fs.rmSync(file, { force: true });
      fs.rmdirSync(path.dirname(file));
    } catch {}
  }
}

function summarizeBrowserFileChooser(browserPage) {
  const state = browserPage?.file_chooser || null;
  const pending = state?.pending || null;
  return {
    schema: "elastos.browser.file-chooser/v1",
    pending: Boolean(pending),
    request_id: pending?.request_id || null,
    mode: pending?.mode || null,
    multiple: pending?.multiple === true,
    opened_at: pending?.opened_at || null,
    event_count: Array.isArray(state?.events) ? state.events.length : 0,
  };
}

function noteBrowserFileChooserOpened(state, params) {
  state.serial += 1;
  const pending = {
    schema: "elastos.browser.file-chooser-request/v1",
    request_id: `file-chooser:${state.serial}`,
    mode: typeof params?.mode === "string" ? params.mode : "",
    multiple: params?.mode === "selectMultiple",
    backend_node_id: Number.isInteger(params?.backendNodeId)
      ? params.backendNodeId
      : null,
    frame_id: typeof params?.frameId === "string" ? params.frameId : "",
    opened_at: nowIso(),
  };
  state.pending = pending;
  state.events.push({
    request_id: pending.request_id,
    mode: pending.mode,
    multiple: pending.multiple,
    backend_node_id_present: Number.isInteger(pending.backend_node_id),
    frame_id: pending.frame_id,
    opened_at: pending.opened_at,
  });
  while (state.events.length > 20) {
    state.events.shift();
  }
  return pending;
}

async function ensureBrowserFileChooserInterception(cdp, browserPage) {
  const state = browserFileChooserState(browserPage);
  if (cdp.fileChooserInterceptionInstalled) {
    return;
  }
  await cdp.request("Page.setInterceptFileChooserDialog", { enabled: true });
  await cdp.request("DOM.enable").catch(() => {});
  cdp.onEvent("Page.fileChooserOpened", (params) => {
    const pending = noteBrowserFileChooserOpened(state, params);
    logControlEvent("file_chooser_opened", {
      request_id: pending.request_id,
      mode: pending.mode,
      multiple: pending.multiple,
      backend_node_id_present: Number.isInteger(pending.backend_node_id),
      frame_id: pending.frame_id || null,
    });
  });
  cdp.fileChooserInterceptionInstalled = true;
}

async function waitHttpOk(url, accepted, timeoutMs) {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), timeoutMs);
  try {
    const response = await fetch(url, { method: "GET", signal: controller.signal });
    if (!accepted(response.status)) {
      throw new Error(`HTTP ${response.status}`);
    }
  } finally {
    clearTimeout(timer);
  }
}

async function waitForBrowserStack(config) {
  const deadline = Date.now() + config.stackReadyTimeoutMs;
  const healthUrl = new URL(config.selkiesWsUrl);
  healthUrl.protocol = healthUrl.protocol === "wss:" ? "https:" : "http:";
  healthUrl.pathname = "/health";
  healthUrl.search = "";
  let lastError = "not checked";
  while (Date.now() < deadline) {
    try {
      await fetchBrowserControlJson(config.browserControl, "/json/version");
      await waitHttpOk(healthUrl, (status) => status === 200 || status === 401, config.browserControl.timeoutMs);
      return;
    } catch (error) {
      lastError = error instanceof Error ? error.message : String(error);
      await sleep(250);
    }
  }
  throw new Error(`Browser VM stack did not become ready: ${lastError}`);
}

function pageIdFor() {
  return `page:selkies-${crypto.randomBytes(8).toString("hex")}`;
}

function normalizeWalletBridge(wallet) {
  const accounts = Array.isArray(wallet?.accounts)
    ? wallet.accounts
        .map((account) => ({
          account_id: String(account?.account_id || ""),
          chain_namespace: String(account?.chain_namespace || ""),
          address: String(account?.address || "").toLowerCase(),
          label: account?.label ? String(account.label) : null,
        }))
        .filter((account) => safeId(account.account_id) && /^eip155:\d+$/.test(account.chain_namespace) && /^0x[0-9a-f]{40}$/.test(account.address))
    : [];
  const defaultChain =
    typeof wallet?.default_chain_namespace === "string" &&
    accounts.some((account) => account.chain_namespace === wallet.default_chain_namespace)
      ? wallet.default_chain_namespace
      : accounts[0]?.chain_namespace || "";
  const defaultAccountId =
    typeof wallet?.default_account_id === "string" &&
    accounts.some((account) => account.account_id === wallet.default_account_id)
      ? wallet.default_account_id
      : accounts[0]?.account_id || "";
  return {
    accounts,
    default_chain_namespace: defaultChain,
    default_account_id: defaultAccountId,
    bridge_url: typeof wallet?.bridge_url === "string" ? wallet.bridge_url : "",
    approval_url: typeof wallet?.approval_url === "string" ? wallet.approval_url : "",
    transaction_url: typeof wallet?.transaction_url === "string" ? wallet.transaction_url : "",
    read_url: typeof wallet?.read_url === "string" ? wallet.read_url : "",
    transaction_broadcast_url:
      typeof wallet?.transaction_broadcast_url === "string" ? wallet.transaction_broadcast_url : "",
    approval_status_url: typeof wallet?.approval_status_url === "string" ? wallet.approval_status_url : "",
    home_token: typeof wallet?.home_token === "string" ? wallet.home_token : "",
  };
}

function chainNamespaceToDecimal(namespace) {
  const [, value] = String(namespace || "").split(":");
  const parsed = Number(value || "");
  return Number.isFinite(parsed) && parsed > 0 ? parsed : null;
}

function chainNamespaceToHex(namespace) {
  const decimal = chainNamespaceToDecimal(namespace);
  return decimal == null ? null : `0x${decimal.toString(16)}`;
}

function walletInitScript(wallet) {
  const current =
    wallet.accounts.find(
      (account) =>
        account.account_id === wallet.default_account_id &&
        account.chain_namespace === wallet.default_chain_namespace,
    ) ||
    wallet.accounts.find((account) => account.account_id === wallet.default_account_id) ||
    wallet.accounts.find((account) => account.chain_namespace === wallet.default_chain_namespace) ||
    wallet.accounts[0] ||
    null;
  const initialState = {
    chainId: chainNamespaceToHex(wallet.default_chain_namespace),
    selectedAddress: current?.address || null,
    accounts: wallet.accounts,
    defaultChainNamespace: wallet.default_chain_namespace,
    defaultAccountId: wallet.default_account_id,
    runtimeBinding: WALLET_RUNTIME_BINDING,
    runtimeResult: WALLET_RUNTIME_RESULT,
  };
  return `
(() => {
      if (!globalThis.__elastosBrowserNavigationPolicyInstalled) {
        Object.defineProperty(globalThis, "__elastosBrowserNavigationPolicyInstalled", {
          value: true,
          configurable: false,
          enumerable: false
        });
        const resolveElastosBrowserNavigationUrl = (value) => {
          try {
            if (value == null || String(value).trim() === "") {
              return "";
            }
            const url = new URL(String(value), window.location.href);
            return url.protocol === "http:" || url.protocol === "https:" ? url.href : "";
          } catch {
            return "";
          }
        };
        const navigateElastosBrowserInPlace = (value) => {
          const href = resolveElastosBrowserNavigationUrl(value);
          if (!href) {
            return false;
          }
          window.location.assign(href);
          return true;
        };
        window.open = (url) => {
          return navigateElastosBrowserInPlace(url) ? window : null;
        };
        document.addEventListener("click", (event) => {
          if (
            event.defaultPrevented ||
            event.button !== 0 ||
            event.metaKey ||
            event.ctrlKey ||
            event.shiftKey ||
            event.altKey
          ) {
            return;
          }
          const anchor =
            event.target && typeof event.target.closest === "function"
              ? event.target.closest("a[target]")
              : null;
          if (!anchor || anchor.download) {
            return;
          }
          const target = String(anchor.target || "").toLowerCase();
          if (target !== "_blank" && target !== "blank") {
            return;
          }
          if (!navigateElastosBrowserInPlace(anchor.href)) {
            return;
          }
          event.preventDefault();
          event.stopImmediatePropagation();
        }, true);
      }
      const nextState = ${JSON.stringify(initialState)};
      if (globalThis.ethereum?.isElastOS) {
        if (typeof globalThis.ethereum.__elastosRefreshWallet === "function") {
          globalThis.ethereum.__elastosRefreshWallet({ force: true }).catch(() => {
            if (typeof globalThis.ethereum.__elastosUpdateWallet === "function") {
              globalThis.ethereum.__elastosUpdateWallet(nextState);
            }
          });
        } else if (typeof globalThis.ethereum.__elastosUpdateWallet === "function") {
          globalThis.ethereum.__elastosUpdateWallet(nextState);
        }
        if (typeof globalThis.ethereum.__elastosAnnounce === "function") {
          globalThis.ethereum.__elastosAnnounce();
        }
        return;
      }
        const state = nextState;
        const listeners = new Map();
        const runtimePending = new Map();
        const walletApprovalPending = new Map();
        let runtimeRequestId = 0;
        const walletDebugEvents = [];
        const pushWalletDebug = (event, detail = {}) => {
          try {
            walletDebugEvents.push({
              at: new Date().toISOString(),
              event: String(event || ""),
              ...detail,
            });
            while (walletDebugEvents.length > 80) {
              walletDebugEvents.shift();
            }
          } catch {}
        };
        const walletRequestSuffix = (requestId) => {
          const text = String(requestId || "");
          return text ? text.slice(-8) : "";
        };
        const walletApprovalCacheKey = (kind, account, method, params) => {
          let paramsKey = "";
          try {
            paramsKey = JSON.stringify(params || []);
          } catch {
            paramsKey = String(params || "");
          }
          return [
            String(kind || ""),
            String(method || ""),
            String(account?.account_id || ""),
            String(account?.chain_namespace || ""),
            String(account?.address || "").toLowerCase(),
            String(pageOrigin() || ""),
            paramsKey,
            ].join("\\n");
        };
        const waitForCachedWalletApproval = (cacheKey, createApproval, options = {}) => {
          const existing = walletApprovalPending.get(cacheKey);
          if (existing) {
            pushWalletDebug("approval_reuse", {
              kind: String(options.kind || ""),
              method: String(options.method || ""),
              request_suffix: existing.requestSuffix || "",
            });
            return existing.promise;
          }
          const entry = { requestSuffix: "" };
          const promise = (async () => {
            const approval = await createApproval();
            const requestId = approval?.approval_request?.request_id;
            if (!requestId) {
              throw runtimeError(options.missingMessage || "Runtime wallet approval request was not created.");
            }
            entry.requestSuffix = walletRequestSuffix(requestId);
            pushWalletDebug("approval_request", {
              kind: String(options.kind || ""),
              method: String(options.method || ""),
              request_suffix: entry.requestSuffix,
            });
            return waitForApproval(requestId, { transaction: Boolean(options.transaction) });
          })();
          entry.promise = promise.finally(() => {
            walletApprovalPending.delete(cacheKey);
          });
          walletApprovalPending.set(cacheKey, entry);
          return entry.promise;
        };
        Object.defineProperty(globalThis, "__elastosWalletDebugSnapshot", {
          value: () => ({
            event_count: walletDebugEvents.length,
            events: walletDebugEvents.slice(-40),
            has_provider: Boolean(globalThis.ethereum?.isElastOS),
            runtime_binding_available: typeof globalThis[state.runtimeBinding] === "function",
            runtime_result_available: typeof globalThis[state.runtimeResult] === "function",
          }),
          configurable: false,
          enumerable: false,
          writable: false
        });
        const runtimeCall = (message = {}) => {
          const bindingName = state.runtimeBinding;
          const resultName = state.runtimeResult;
          const binding = globalThis[bindingName];
          if (typeof binding !== "function" || typeof globalThis[resultName] !== "function") {
            pushWalletDebug("runtime_binding_unavailable", {
              action: String(message.action || ""),
              has_binding: typeof binding === "function",
              has_result: typeof globalThis[resultName] === "function"
            });
            return Promise.reject(runtimeError("Runtime wallet bridge binding is unavailable for this Browser session."));
          }
          const id = "wallet:" + (++runtimeRequestId);
          pushWalletDebug("runtime_call", { id, action: String(message.action || ""), operation: String(message.operation || "") });
          return new Promise((resolve, reject) => {
            const timer = window.setTimeout(() => {
              runtimePending.delete(id);
              pushWalletDebug("runtime_timeout", { id, action: String(message.action || ""), operation: String(message.operation || "") });
              reject(runtimeError("Runtime wallet bridge request timed out.", 4001));
            }, 60000);
          runtimePending.set(id, { resolve, reject, timer });
          try {
            binding(JSON.stringify({ id, ...message }));
          } catch (error) {
            window.clearTimeout(timer);
            runtimePending.delete(id);
            reject(runtimeError(error && error.message ? error.message : "Runtime wallet bridge request failed."));
          }
        });
      };
      if (typeof globalThis[state.runtimeResult] !== "function") {
        Object.defineProperty(globalThis, state.runtimeResult, {
          value: (raw) => {
              let response = null;
              try { response = JSON.parse(String(raw || "{}")); } catch {}
              const pending = response && runtimePending.get(response.id);
              if (!pending) {
                return;
              }
              runtimePending.delete(response.id);
              window.clearTimeout(pending.timer);
              if (response.ok) {
                pushWalletDebug("runtime_result", { id: response.id, ok: true });
                pending.resolve(response.result);
                return;
              }
              pushWalletDebug("runtime_result", {
                id: response.id,
                ok: false,
                error: String(response.error?.message || "Runtime wallet bridge request failed.").slice(0, 240),
                code: Number.isInteger(response.error?.code) ? response.error.code : 4100
              });
              const error = runtimeError(response.error?.message || "Runtime wallet bridge request failed.", response.error?.code || 4100);
              pending.reject(error);
            },
          configurable: false,
          enumerable: false,
          writable: false
        });
      }
    const providerInfo = {
      uuid: "9a9a76a8-f36e-4a1f-9a3d-3d9d0f4b4e1a",
      name: "ElastOS Wallet",
      icon: "data:image/svg+xml,<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 64 64'><rect width='64' height='64' rx='16' fill='%230E0E0C'/><text x='32' y='41' text-anchor='middle' font-family='Arial' font-size='34' font-weight='700' fill='white'>e</text></svg>",
      rdns: "com.elacitylabs.elastos.wallet"
    };
    const emit = (event, payload) => {
      for (const handler of listeners.get(event) || []) {
        try { handler(payload); } catch {}
      }
    };
  const chainNamespaceToDecimal = (namespace) => {
    const parsed = Number(String(namespace || "").split(":")[1] || "");
    return Number.isFinite(parsed) && parsed > 0 ? parsed : null;
  };
  const chainNamespaceToHex = (namespace) => {
    const decimal = chainNamespaceToDecimal(namespace);
    return decimal == null ? null : "0x" + decimal.toString(16);
  };
    const accountForChain = (namespace) => state.accounts.find((account) => account.chain_namespace === namespace) || null;
    const accountForId = (accountId) => state.accounts.find((account) => account.account_id === accountId) || null;
    const currentAccount = () =>
      state.accounts.find(
        (account) =>
          account.account_id === state.defaultAccountId &&
          account.chain_namespace === state.defaultChainNamespace
      ) ||
      accountForId(state.defaultAccountId) ||
      accountForChain(state.defaultChainNamespace) ||
      state.accounts[0] ||
      null;
    const currentAccounts = () => {
      const account = currentAccount();
      return account ? [account.address] : [];
    };
    const runtimeError = (message, code = 4100) => {
      const error = new Error(message);
      error.code = code;
      return error;
    };
    const runtimePost = async (operation, body) =>
      runtimeCall({ action: "post", operation, body });
    let walletRefreshInFlight = null;
    let lastWalletRefreshAt = 0;
    const refreshWalletState = async (options = {}) => {
      const force = options.force === true;
      if (!force && Date.now() - lastWalletRefreshAt < 1500) {
        return;
      }
      if (walletRefreshInFlight) {
        return walletRefreshInFlight;
      }
      walletRefreshInFlight = (async () => {
        const payload = await runtimeCall({ action: "bridge" });
        if (payload?.schema !== "elastos.browser.wallet-bridge/v1") {
          throw runtimeError("Runtime wallet bridge refresh failed.");
        }
        provider.__elastosUpdateWallet({
          accounts: Array.isArray(payload.accounts) ? payload.accounts : [],
          defaultChainNamespace: typeof payload.default_chain_namespace === "string" ? payload.default_chain_namespace : "",
          defaultAccountId: typeof payload.default_account_id === "string" ? payload.default_account_id : "",
        });
        lastWalletRefreshAt = Date.now();
      })().finally(() => {
        walletRefreshInFlight = null;
      });
      return walletRefreshInFlight;
    };
    const runtimeRead = async (method, params) => {
      const account = selectedEvmAccount();
      const payload = await runtimePost("read", {
        method,
        params,
        chain_namespace: account.chain_namespace,
        address: account.address,
        page_url: pageUrl(),
        origin: pageOrigin()
      });
      if (payload?.schema !== "elastos.browser.wallet-read-result/v1" || payload.requires_approval !== false) {
        throw runtimeError("Runtime chain provider returned an invalid Browser wallet read response.");
      }
      return payload.result;
    };
    const runtimeGetApproval = async (requestId) => {
      return runtimeCall({ action: "approvalStatus", request_id: requestId });
    };
    const waitForApproval = async (requestId, { transaction = false } = {}) => {
      const deadline = Date.now() + 5 * 60 * 1000;
      while (Date.now() < deadline) {
        const status = await runtimeGetApproval(requestId);
        if (status?.status === "completed") {
          if (transaction) {
            if (status.transaction_hash) return status.transaction_hash;
            const broadcast = await runtimePost("transactionBroadcast", { request_id: requestId });
            const transactionHash = broadcast.transaction_hash || status.transaction_hash;
            if (typeof transactionHash === "string" && transactionHash) {
              pushWalletDebug("transaction_broadcast", { request_id: requestId, hash_suffix: transactionHash.slice(-8) });
              return transactionHash;
            }
            throw runtimeError("Runtime transaction broadcast completed without a transaction hash.");
          }
          if (status.signature) return status.signature;
          throw runtimeError("Runtime wallet approval completed without a signature.");
        }
        if (status?.status === "rejected" || status?.status === "expired") {
          throw runtimeError("Runtime wallet approval was " + status.status + ".", 4001);
        }
        await new Promise((resolve) => setTimeout(resolve, 1200));
      }
      throw runtimeError("Runtime wallet approval timed out.", 4001);
    };
    const pageUrl = () => {
      try { return window.location.href; } catch { return ""; }
    };
    const pageOrigin = () => {
      try { return window.location.origin; } catch { return null; }
    };
    const selectedEvmAccount = () => {
      const account = currentAccount();
      if (!account) {
        throw runtimeError("No ElastOS Wallet EVM account is available for this Runtime principal. Open Wallet to create or link an EVM account first.");
      }
      return account;
    };
    const normalizePersonalSignParams = (params) => {
      const account = selectedEvmAccount();
      const first = typeof params[0] === "string" ? params[0] : "";
      const second = typeof params[1] === "string" ? params[1] : "";
      if (first.toLowerCase() === account.address.toLowerCase() && second) {
        return [second, account.address];
      }
      return [first, second || account.address];
    };
    const applyChain = (account) => {
      state.defaultChainNamespace = account.chain_namespace;
      state.chainId = chainNamespaceToHex(account.chain_namespace);
      state.selectedAddress = account.address;
      provider.chainId = state.chainId;
      provider.networkVersion = String(chainNamespaceToDecimal(account.chain_namespace) || "");
      provider.selectedAddress = account.address;
      emit("chainChanged", provider.chainId);
      emit("accountsChanged", [account.address]);
    };
    const switchToChainId = (chainIdValue) => {
      const chainId = String(chainIdValue || "").toLowerCase();
      if (!/^0x[0-9a-f]+$/.test(chainId)) {
        const error = new Error("Wallet network switch requires a hex EIP-155 chain id.");
        error.code = 4902;
        throw error;
      }
      const next =
        state.accounts.find(
          (account) =>
            account.account_id === state.defaultAccountId &&
            chainNamespaceToHex(account.chain_namespace) === chainId
        ) ||
        state.accounts.find((account) => chainNamespaceToHex(account.chain_namespace) === chainId);
      if (!next) {
        const decimal = parseInt(chainId, 16);
        const error = new Error("No ElastOS Wallet account is available for eip155:" + decimal + ". Open Wallet to create or link this network first.");
        error.code = 4902;
        throw error;
      }
      applyChain(next);
      return null;
    };
    const forceRefreshIfNoAccounts = async () => {
      if (currentAccounts().length === 0) {
        await refreshWalletState({ force: true }).catch(() => {});
      }
      return currentAccounts();
    };
      const request = async (payload = {}) => {
        const method = payload && payload.method;
        const params = Array.isArray(payload && payload.params) ? payload.params : [];
        pushWalletDebug("request", { method: String(method || ""), params_len: params.length });
        await refreshWalletState().catch(() => {});
      if (method === "eth_accounts") {
        const accounts = await forceRefreshIfNoAccounts();
        provider.selectedAddress = accounts[0] || null;
        return accounts;
      }
      if (method === "eth_requestAccounts") {
        const accounts = await forceRefreshIfNoAccounts();
        if (accounts.length === 0) {
          const error = new Error("No ElastOS Wallet EVM account is available for this Runtime principal. Open Wallet to create or link an EVM account first.");
          error.code = 4100;
          throw error;
        }
        provider.selectedAddress = accounts[0];
        emit("accountsChanged", accounts);
        emit("connect", { chainId: provider.chainId });
        return accounts;
      }
      if (method === "eth_coinbase") return currentAccounts()[0] || null;
      if (method === "eth_chainId") {
        if (!provider.chainId) {
          const error = new Error("No ElastOS Wallet EVM chain is selected for this Runtime principal.");
          error.code = 4900;
          throw error;
        }
        return provider.chainId;
      }
      if (method === "net_version") return provider.chainId ? String(parseInt(provider.chainId, 16)) : "";
      if (method === "wallet_getPermissions" || method === "wallet_requestPermissions") {
        return currentAccounts().length > 0 ? [{ parentCapability: "eth_accounts", caveats: [] }] : [];
      }
        if (method === "wallet_switchEthereumChain") {
          return switchToChainId(params[0] && params[0].chainId);
      }
      if (method === "wallet_addEthereumChain") {
        return switchToChainId(params[0] && params[0].chainId);
      }
      if (
        method === "eth_blockNumber" ||
        method === "eth_getBalance" ||
        method === "eth_call" ||
        method === "eth_estimateGas" ||
        method === "eth_getTransactionCount" ||
        method === "eth_gasPrice" ||
        method === "eth_feeHistory" ||
        method === "eth_getCode" ||
        method === "eth_getLogs" ||
        method === "eth_getTransactionByHash" ||
        method === "eth_getTransactionReceipt"
      ) {
        return runtimeRead(method, params);
      }
        if (method === "personal_sign" || method === "eth_sign") {
          const account = selectedEvmAccount();
          const normalizedParams = normalizePersonalSignParams(params);
          pushWalletDebug("signature_request", {
            method,
            account_suffix: String(account.address || "").slice(-6),
            message_len: typeof normalizedParams[0] === "string" ? normalizedParams[0].length : 0
          });
          const cacheKey = walletApprovalCacheKey("signature", account, "personal_sign", normalizedParams);
          return waitForCachedWalletApproval(cacheKey, () => runtimePost("approval", {
            method: "personal_sign",
            params: normalizedParams,
            account_id: account.account_id,
            chain_namespace: account.chain_namespace,
            address: account.address,
            page_url: pageUrl(),
            origin: pageOrigin()
          }), {
            kind: "signature",
            method: "personal_sign",
            missingMessage: "Runtime wallet approval request was not created.",
          });
      }
        if (method === "eth_signTypedData" || method === "eth_signTypedData_v3" || method === "eth_signTypedData_v4") {
          const account = selectedEvmAccount();
          pushWalletDebug("signature_request", {
            method,
            account_suffix: String(account.address || "").slice(-6),
            params_len: params.length
          });
        const first = params[0];
        const second = params[1];
        const firstIsAccount = typeof first === "string" && first.toLowerCase() === account.address.toLowerCase();
        const secondIsAccount = typeof second === "string" && second.toLowerCase() === account.address.toLowerCase();
        const normalizedParams = firstIsAccount
          ? [account.address, second]
          : secondIsAccount
            ? [account.address, first]
            : params;
        const cacheKey = walletApprovalCacheKey("typed-data", account, method, normalizedParams);
        return waitForCachedWalletApproval(cacheKey, () => runtimePost("approval", {
          method,
          params: normalizedParams,
          account_id: account.account_id,
          chain_namespace: account.chain_namespace,
          address: account.address,
          page_url: pageUrl(),
          origin: pageOrigin()
        }), {
          kind: "typed-data",
          method,
          missingMessage: "Runtime wallet typed-data approval request was not created.",
        });
      }
      if (method === "eth_sendTransaction") {
        const account = selectedEvmAccount();
        const approval = await runtimePost("transaction", {
          method,
          params,
          account_id: account.account_id,
          chain_namespace: account.chain_namespace,
          address: account.address,
          page_url: pageUrl(),
          origin: pageOrigin()
        });
        const requestId = approval?.approval_request?.request_id;
        if (!requestId) {
          throw runtimeError("Runtime transaction approval request was not created.");
        }
        return waitForApproval(requestId, { transaction: true });
      }
    const error = new Error(method + " requires Runtime Wallet/Inbox approval and is not exposed by this hosted Browser adapter yet.");
    error.code = 4100;
    throw error;
    };
      const provider = {
      isElastOS: true,
      isMetaMask: true,
      isConnected: () => Boolean(provider.chainId),
      request,
    selectedAddress: state.selectedAddress,
    chainId: state.chainId,
    networkVersion: state.chainId ? String(parseInt(state.chainId, 16)) : "",
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
    provider.__elastosUpdateWallet = (next) => {
      state.accounts = Array.isArray(next?.accounts) ? next.accounts : [];
      state.defaultAccountId = typeof next?.defaultAccountId === "string" ? next.defaultAccountId : state.defaultAccountId;
      const preferred = typeof next?.defaultChainNamespace === "string"
        ? state.accounts.find((account) => account.account_id === state.defaultAccountId && account.chain_namespace === next.defaultChainNamespace) || accountForChain(next.defaultChainNamespace)
        : null;
      const retainedOrFirst = currentAccount();
      const account = preferred || retainedOrFirst;
      if (account) {
        applyChain(account);
      } else {
        state.defaultChainNamespace = typeof next?.defaultChainNamespace === "string" ? next.defaultChainNamespace : "";
        state.chainId = chainNamespaceToHex(state.defaultChainNamespace);
        state.selectedAddress = null;
        provider.chainId = state.chainId;
        provider.networkVersion = state.chainId ? String(chainNamespaceToDecimal(state.defaultChainNamespace) || "") : "";
        provider.selectedAddress = null;
        emit("accountsChanged", []);
      }
    };
    provider.__elastosRefreshWallet = refreshWalletState;
    window.setInterval(() => {
      refreshWalletState().catch(() => {});
    }, 3000);
    provider.providers = [provider];
    provider.send = (methodOrPayload, paramsOrCallback) => {
      if (typeof methodOrPayload === "string") {
        return request({ method: methodOrPayload, params: Array.isArray(paramsOrCallback) ? paramsOrCallback : [] });
      }
      const payload = methodOrPayload || {};
      const callback = typeof paramsOrCallback === "function" ? paramsOrCallback : null;
      const promise = request(payload);
      if (callback) {
        promise.then((result) => callback(null, { id: payload.id, jsonrpc: "2.0", result })).catch((error) => callback(error));
      }
      return promise;
    };
    provider.sendAsync = (payload, callback) => {
      request(payload || {}).then((result) => callback(null, { id: payload?.id, jsonrpc: "2.0", result })).catch((error) => callback(error));
    };
      Object.defineProperty(window, "ethereum", {
        value: provider,
        configurable: false,
      enumerable: false,
      writable: false
    });
      const announceProvider = () => {
        window.dispatchEvent(new CustomEvent("eip6963:announceProvider", {
          detail: { info: providerInfo, provider }
        }));
      };
      provider.__elastosAnnounce = announceProvider;
    window.addEventListener("eip6963:requestProvider", announceProvider);
    const announceProviderWhenWalletReady = () => {
      refreshWalletState({ force: true }).catch(() => {}).finally(() => {
        announceProvider();
        window.dispatchEvent(new Event("ethereum#initialized"));
      });
    };
    queueMicrotask(announceProviderWhenWalletReady);
  })();
`;
}

async function readBrowserPageState(cdp, fallbackUrl, fallbackTitle, timeoutMs) {
  const current = await cdp.request(
    "Runtime.evaluate",
    {
      expression: "JSON.stringify({ url: window.location.href, title: document.title })",
      returnByValue: true,
    },
    Math.min(timeoutMs, 5000),
  );
  let page = {};
  try {
    page = JSON.parse(current?.result?.value || "{}");
  } catch {
    page = {};
  }
  return {
    url: typeof page.url === "string" && page.url ? page.url : fallbackUrl,
    title: typeof page.title === "string" && page.title ? page.title : fallbackTitle,
  };
}

function isNetworkChangedError(value) {
  return String(value || "").includes("ERR_NETWORK_CHANGED");
}

function isConnectionClosedError(value) {
  return String(value || "").includes("ERR_CONNECTION_CLOSED");
}

function isNavigationAbortedError(value) {
  return String(value || "").includes("ERR_ABORTED");
}

function isNavigationTimedOutNetError(value) {
  return String(value || "").includes("ERR_TIMED_OUT");
}

function isRetryableRuntimeNavigationError(value) {
  return isNetworkChangedError(value) ||
    isConnectionClosedError(value) ||
    isNavigationAbortedError(value) ||
    isNavigationTimedOutNetError(value);
}

function isPageNavigateTimeoutError(value) {
  return String(value || "").includes("timed out waiting for browser CDP response Page.navigate");
}

function isRetryableBrowserNavigationException(value) {
  return isRetryableRuntimeNavigationError(value) || isPageNavigateTimeoutError(value);
}

function assertBrowserNavigationSucceeded(navigation, label) {
  if (navigation?.errorText) {
    throw new Error(`browser CDP ${label} failed: ${navigation.errorText}`);
  }
}

function isBrowserErrorUrl(value) {
  const text = String(value || "").trim().toLowerCase();
  return text === "chrome-error://chromewebdata/" || text.startsWith("chrome-error://");
}

function assertBrowserStateDidNotLandOnErrorPage(state, label) {
  if (isBrowserErrorUrl(state?.url)) {
    throw new Error(`browser CDP ${label} failed: ${state.url}`);
  }
}

function sameBrowserUrl(left, right) {
  try {
    return new URL(String(left || "")).href === new URL(String(right || "")).href;
  } catch {
    return String(left || "") === String(right || "");
  }
}

function summarizeNetworkFailure(params, source = "Network.loadingFailed") {
  return {
    source,
    request_id: typeof params?.requestId === "string" ? params.requestId : "",
    type: typeof params?.type === "string" ? params.type : "",
    error_text: typeof params?.errorText === "string" ? params.errorText : "",
    canceled: params?.canceled === true,
  };
}

async function waitForInitialNavigation(cdp, browserControl, navigation, label = "initial") {
  const failures = [];
  if (navigation.errorText) {
    if (isRetryableRuntimeNavigationError(navigation.errorText)) {
      failures.push(summarizeNetworkFailure({ errorText: navigation.errorText }, "Page.navigate"));
      return {
        failures,
        network_changed: isNetworkChangedError(navigation.errorText),
        retryable: true,
      };
    }
    throw new Error(`browser CDP navigation failed: ${navigation.errorText}`);
  }
  try {
    await cdp.waitForEvent("Page.domContentEventFired", browserControl.timeoutMs, "domcontent");
    console.error(JSON.stringify({
      schema: "elastos.browser.selkies-control.navigation-ready/v1",
      label,
      event: "domcontent",
      ok: true,
    }));
  } catch (error) {
    console.error(JSON.stringify({
      schema: "elastos.browser.selkies-control.navigation-ready/v1",
      label,
      event: "domcontent",
      ok: false,
      warning: error instanceof Error ? error.message : String(error),
    }));
  }
  try {
    await cdp.waitForEvent("Page.loadEventFired", Math.min(browserControl.timeoutMs, 5000), "load");
    console.error(JSON.stringify({
      schema: "elastos.browser.selkies-control.navigation-ready/v1",
      label,
      event: "load",
      ok: true,
    }));
  } catch (error) {
    console.error(JSON.stringify({
      schema: "elastos.browser.selkies-control.navigation-ready/v1",
      label,
      event: "load",
      ok: false,
      warning: error instanceof Error ? error.message : String(error),
    }));
  }
  return { failures, network_changed: false, retryable: false };
}

async function observeInitialNavigation(cdp, browserControl, label, action) {
  const failures = [];
  cdp.clearEvents(["Page.domContentEventFired", "Page.loadEventFired", "Network.loadingFailed"]);
  const removeFailureHandler = cdp.onEvent("Network.loadingFailed", (params) => {
    failures.push(summarizeNetworkFailure(params));
  });
  try {
    let navigation;
    try {
      navigation = await action();
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      if (!isRetryableBrowserNavigationException(message)) {
        throw error;
      }
      failures.push(summarizeNetworkFailure({ errorText: message }, "Page.navigate"));
      return {
        navigation: { errorText: message },
        failures,
        network_changed: isNetworkChangedError(message),
        retryable: true,
        timed_out: isPageNavigateTimeoutError(message),
      };
    }
    const readiness = await waitForInitialNavigation(cdp, browserControl, navigation, label);
    const allFailures = [...failures, ...(readiness.failures || [])];
    const networkChanged = readiness.network_changed === true ||
      allFailures.some((failure) => isNetworkChangedError(failure.error_text));
    const retryable = readiness.retryable === true ||
      allFailures.some((failure) => isRetryableBrowserNavigationException(failure.error_text));
    return {
      navigation,
      failures: allFailures,
      network_changed: networkChanged,
      retryable,
      timed_out: allFailures.some((failure) => isPageNavigateTimeoutError(failure.error_text)),
    };
  } finally {
    removeFailureHandler();
  }
}

async function navigateInitialBrowserPage(cdp, browserControl, url, label = "initial") {
  const first = await observeInitialNavigation(
    cdp,
    browserControl,
    label,
    () => cdp.request("Page.navigate", { url }, browserControl.timeoutMs),
  );
  if (first.retryable !== true) {
    return first.navigation;
  }
  const reason = first.timed_out
    ? "navigation_timeout"
    : first.network_changed
      ? "network_changed"
      : first.failures.some((failure) => isNavigationAbortedError(failure.error_text))
        ? "navigation_aborted"
        : first.failures.some((failure) => isNavigationTimedOutNetError(failure.error_text))
          ? "network_timeout"
          : "connection_closed";
  console.error(JSON.stringify({
    schema: "elastos.browser.selkies-control.navigation-repair/v1",
    reason,
    url,
    failures: first.failures.slice(0, 8),
    route: "runtime_net_only",
    direct_network: false,
  }));
  if (first.timed_out) {
    const settled = await readSettledNavigationState(cdp, browserControl, url, `${label}-timeout`);
    if (settled) {
      return { frameId: "", loaderId: "" };
    }
  }
  const second = await observeInitialNavigation(
    cdp,
    browserControl,
    `${label}-repair`,
    () => cdp.request("Page.navigate", { url }, browserControl.timeoutMs),
  );
  if (second.retryable) {
    if (second.timed_out || second.failures.some((failure) => isNavigationAbortedError(failure.error_text))) {
      const settled = await readSettledNavigationState(cdp, browserControl, url, `${label}-repair`);
      if (settled) {
        return { frameId: "", loaderId: "" };
      }
    }
    const errorText = second.failures.find((failure) => failure.error_text)?.error_text || "retryable network error";
    throw new Error(`browser ${label} navigation still reported ${errorText} after Runtime route repair`);
  }
  return second.navigation;
}

async function readSettledNavigationState(cdp, browserControl, url, label) {
  try {
    const state = await readBrowserPageState(cdp, url, "Selkies Browser", Math.min(browserControl.timeoutMs, 10000));
    const ok = sameBrowserUrl(state.url, url) && !isBrowserErrorUrl(state.url);
    console.error(JSON.stringify({
      schema: "elastos.browser.selkies-control.navigation-settle/v1",
      label,
      url,
      actual_url: state.url,
      ok,
    }));
    return ok;
  } catch (error) {
    console.error(JSON.stringify({
      schema: "elastos.browser.selkies-control.navigation-settle/v1",
      label,
      url,
      ok: false,
      warning: error instanceof Error ? error.message : String(error),
    }));
    return false;
  }
}

function httpJson(res, status, body) {
  const data = Buffer.from(JSON.stringify(body));
  res.writeHead(status, {
    "content-type": "application/json",
    "content-length": data.length,
  });
  res.end(data);
}

function readTail(path, maxBytes) {
  const stat = fs.statSync(path);
  const length = Math.min(stat.size, maxBytes);
  const buffer = Buffer.alloc(length);
  const fd = fs.openSync(path, "r");
  try {
    fs.readSync(fd, buffer, 0, length, stat.size - length);
  } finally {
    fs.closeSync(fd);
  }
  return buffer.toString("utf8");
}

function guestAudioEnv() {
  const runtimeDir = process.env.XDG_RUNTIME_DIR || "/run/elastos/browser-runtime";
  const pulseRuntimePath = process.env.PULSE_RUNTIME_PATH || `${runtimeDir}/pulse`;
  return {
    ...process.env,
    XDG_RUNTIME_DIR: runtimeDir,
    PIPEWIRE_RUNTIME_DIR: process.env.PIPEWIRE_RUNTIME_DIR || runtimeDir,
    PULSE_RUNTIME_PATH: pulseRuntimePath,
    PULSE_SERVER: process.env.PULSE_SERVER || `unix:${pulseRuntimePath}/native`,
  };
}

function runGuestAudioCommand(command, args = [], timeoutMs = 2500) {
  try {
    return execFileSync(command, args, {
      encoding: "utf8",
      timeout: timeoutMs,
      maxBuffer: 512 * 1024,
      env: guestAudioEnv(),
      stdio: ["ignore", "pipe", "pipe"],
    });
  } catch (error) {
    const stdout = error?.stdout ? String(error.stdout) : "";
    const stderr = error?.stderr ? String(error.stderr) : "";
    const message = error instanceof Error ? error.message : String(error);
    return [stdout, stderr, `[${command} failed: ${message}]`].filter(Boolean).join("\n");
  }
}

function compactPipewireDump() {
  const raw = runGuestAudioCommand("pw-dump", [], 3000);
  let objects = [];
  try {
    objects = JSON.parse(raw);
  } catch {
    return raw
      .split(/\r?\n/)
      .filter((line) => /"(type|id|node\.name|node\.description|media\.class|application\.name|client\.api|object\.path|factory\.name|pulse\.server\.type|audio\.position)"/.test(line))
      .join("\n");
  }
  const interestingKeys = [
    "node.name",
    "node.description",
    "media.class",
    "application.name",
    "client.api",
    "object.path",
    "factory.name",
    "pulse.server.type",
    "audio.position",
    "node.target",
    "target.object",
    "link.output.node",
    "link.input.node",
  ];
  return objects
    .map((object) => {
      const props = object?.info?.props || {};
      const facts = interestingKeys
        .filter((key) => props[key] !== undefined)
        .map((key) => `${key}=${JSON.stringify(props[key])}`)
        .join(" ");
      if (!facts && !/Client|Node|Link|Metadata|Module|Factory/.test(String(object?.type || ""))) {
        return "";
      }
      return `${object?.id ?? "?"} ${object?.type || "unknown"}${facts ? ` ${facts}` : ""}`;
    })
    .filter(Boolean)
    .join("\n");
}

function refreshBrowserVmAudioSummary() {
  const path = `${VM_LOG_DIR}/browser-vm-pipewire-summary.log`;
  try {
    const env = guestAudioEnv();
    const runtimeDir = env.XDG_RUNTIME_DIR;
    const pulseRuntimePath = env.PULSE_RUNTIME_PATH;
    const lines = [
      "=== browser audio environment ===",
      `XDG_RUNTIME_DIR=${runtimeDir}`,
      `PIPEWIRE_RUNTIME_DIR=${env.PIPEWIRE_RUNTIME_DIR}`,
      `PULSE_RUNTIME_PATH=${pulseRuntimePath}`,
      `PULSE_SERVER=${env.PULSE_SERVER}`,
      "=== browser audio sockets ===",
    ];
    for (const dir of [runtimeDir, pulseRuntimePath].filter(Boolean)) {
      try {
        lines.push(`${dir}: ${fs.readdirSync(dir).join(" ")}`);
      } catch (error) {
        lines.push(`${dir}: ${error instanceof Error ? error.message : String(error)}`);
      }
    }
    lines.push("=== pw-cli info 0 ===");
    lines.push(runGuestAudioCommand("pw-cli", ["info", "0"]));
    lines.push("=== pw-cli ls Node ===");
    lines.push(runGuestAudioCommand("pw-cli", ["ls", "Node"]));
    lines.push("=== pw-cli ls Client ===");
    lines.push(runGuestAudioCommand("pw-cli", ["ls", "Client"]));
    lines.push("=== pw-cli ls Port ===");
    lines.push(runGuestAudioCommand("pw-cli", ["ls", "Port"]));
    lines.push("=== pw-cli ls Link ===");
    lines.push(runGuestAudioCommand("pw-cli", ["ls", "Link"]));
    lines.push("=== pw-link outputs ===");
    lines.push(runGuestAudioCommand("pw-link", ["-o"]));
    lines.push("=== pw-link inputs ===");
    lines.push(runGuestAudioCommand("pw-link", ["-i"]));
    lines.push("=== pw-link links ===");
    lines.push(runGuestAudioCommand("pw-link", ["-l"]));
    lines.push("=== pw-dump compact audio facts ===");
    lines.push(compactPipewireDump());
    fs.writeFileSync(path, `${lines.join("\n")}\n`);
  } catch {}
}

function readBrowserVmLogTails() {
  refreshBrowserVmAudioSummary();
  const logs = {};
  for (const name of VM_LOG_NAMES) {
    const path = `${VM_LOG_DIR}/${name}`;
    try {
      if (!fs.existsSync(path)) {
        logs[name] = { present: false };
        continue;
      }
      const stat = fs.statSync(path);
      logs[name] = {
        present: true,
        bytes: stat.size,
        tail: readTail(path, 8192),
      };
    } catch (error) {
      logs[name] = {
        present: false,
        error: error instanceof Error ? error.message : String(error),
      };
    }
  }
  return logs;
}

function nowIso() {
  return new Date().toISOString();
}

function summarizeIceCandidate(candidate) {
  const line = String(candidate?.candidate || "").trim();
  if (!line) {
    return null;
  }
  const tokens = line.split(/\s+/);
  const address = tokens[4] || "";
  const addressFamily = net.isIP(address);
  let candidateType = null;
  for (let index = 0; index < tokens.length - 1; index += 1) {
    if (tokens[index].toLowerCase() === "typ") {
      candidateType = tokens[index + 1].toLowerCase();
      break;
    }
  }
  return {
    type: candidateType,
    protocol: tokens[2]?.toLowerCase() || null,
    component: tokens[1] || null,
    address_kind: address.endsWith(".local")
      ? "mdns"
      : addressFamily === 4
        ? "ipv4"
        : addressFamily === 6
          ? "ipv6"
          : address
            ? "hostname"
            : "unknown",
    address_is_mdns: address.endsWith(".local"),
    port_present: Boolean(tokens[5]),
    sdp_mid: typeof candidate?.sdpMid === "string" ? candidate.sdpMid : null,
    sdp_mline_index: Number.isInteger(candidate?.sdpMLineIndex) ? candidate.sdpMLineIndex : null,
    line_bytes: Buffer.byteLength(line),
  };
}

function readJsonRequest(req) {
  return new Promise((resolve, reject) => {
    const chunks = [];
    req.on("data", (chunk) => chunks.push(chunk));
    req.on("end", () => {
      try {
        const raw = Buffer.concat(chunks).toString("utf8");
        resolve(raw ? JSON.parse(raw) : {});
      } catch (error) {
        reject(new Error(`invalid JSON request: ${error.message}`));
      }
    });
    req.on("error", reject);
  });
}

class MinimalWebSocketClient {
  constructor(url, { basicAuth } = {}) {
    this.url = url;
    this.basicAuth = basicAuth;
    this.socket = null;
    this.buffer = Buffer.alloc(0);
    this.textHandler = () => {};
    this.errorHandler = () => {};
    this.closeHandler = () => {};
    this.closed = true;
  }

  async connect(timeoutMs) {
    const port = Number(this.url.port || (this.url.protocol === "wss:" ? 443 : 80));
    const host = this.url.hostname;
    const path = `${this.url.pathname || "/"}${this.url.search || ""}`;
    this.socket = await new Promise((resolve, reject) => {
      const connect = this.url.protocol === "wss:" ? tls.connect : net.connect;
      const socket = connect({ host, port, servername: host });
      const timer = setTimeout(() => {
        socket.destroy(new Error("Selkies WebSocket connect timed out"));
      }, timeoutMs);
      socket.once("connect", () => {
        clearTimeout(timer);
        resolve(socket);
      });
      socket.once("error", reject);
    });
    const key = crypto.randomBytes(16).toString("base64");
    const headers = [
      `GET ${path} HTTP/1.1`,
      `Host: ${host}:${port}`,
      "Upgrade: websocket",
      "Connection: Upgrade",
      `Sec-WebSocket-Key: ${key}`,
      "Sec-WebSocket-Version: 13",
    ];
    if (this.basicAuth?.user && this.basicAuth?.password) {
      const value = Buffer.from(`${this.basicAuth.user}:${this.basicAuth.password}`).toString("base64");
      headers.push(`Authorization: Basic ${value}`);
    }
    this.socket.write(`${headers.join("\r\n")}\r\n\r\n`);
    await this.readHandshake(timeoutMs);
    this.closed = false;
    this.socket.on("data", (chunk) => {
      try {
        this.handleData(chunk);
      } catch (error) {
        this.errorHandler(error);
        this.close();
      }
    });
    this.socket.on("error", (error) => {
      this.closed = true;
      this.errorHandler(error);
    });
    this.socket.on("close", () => {
      this.closed = true;
      this.closeHandler();
    });
    if (this.buffer.length > 0) {
      this.handleData(Buffer.alloc(0));
    }
  }

  readHandshake(timeoutMs) {
    return new Promise((resolve, reject) => {
      let data = Buffer.alloc(0);
      const timer = setTimeout(() => {
        cleanup();
        reject(new Error("Selkies WebSocket handshake timed out"));
      }, timeoutMs);
      const cleanup = () => {
        clearTimeout(timer);
        this.socket.off("data", onData);
        this.socket.off("error", onError);
      };
      const onError = (error) => {
        cleanup();
        reject(error);
      };
      const onData = (chunk) => {
        data = Buffer.concat([data, chunk]);
        const end = data.indexOf("\r\n\r\n");
        if (end < 0) {
          return;
        }
        const head = data.subarray(0, end).toString("utf8");
        if (!head.startsWith("HTTP/1.1 101") && !head.startsWith("HTTP/1.0 101")) {
          cleanup();
          reject(new Error(`Selkies WebSocket handshake failed: ${head.split("\r\n")[0]}`));
          return;
        }
        this.buffer = data.subarray(end + 4);
        cleanup();
        resolve();
      };
      this.socket.on("data", onData);
      this.socket.on("error", onError);
    });
  }

  onText(handler) {
    this.textHandler = handler;
  }

  onError(handler) {
    this.errorHandler = handler;
  }

  onClose(handler) {
    this.closeHandler = handler;
  }

  sendText(text) {
    this.sendFrame(0x1, Buffer.from(text));
  }

  sendFrame(opcode, payload) {
    if (!this.socket || this.socket.destroyed || this.closed) {
      throw new Error("Selkies WebSocket is closed");
    }
    const header = [];
    header.push(0x80 | opcode);
    if (payload.length < 126) {
      header.push(0x80 | payload.length);
    } else if (payload.length <= 0xffff) {
      header.push(0x80 | 126, (payload.length >> 8) & 0xff, payload.length & 0xff);
    } else {
      throw new Error("Selkies WebSocket message is too large");
    }
    const mask = crypto.randomBytes(4);
    const masked = Buffer.alloc(payload.length);
    for (let index = 0; index < payload.length; index += 1) {
      masked[index] = payload[index] ^ mask[index % 4];
    }
    this.socket.write(Buffer.concat([Buffer.from(header), mask, masked]));
  }

  close() {
    if (!this.socket || this.socket.destroyed) {
      return;
    }
    try {
      this.sendFrame(0x8, Buffer.alloc(0));
    } catch {
      // The socket may already be closing; TCP teardown below is still required.
    }
    this.closed = true;
    this.socket.end();
  }

  handleData(chunk) {
    this.buffer = Buffer.concat([this.buffer, chunk]);
    for (;;) {
      const frame = readFrame(this.buffer);
      if (!frame) {
        return;
      }
      this.buffer = this.buffer.subarray(frame.consumed);
      if (frame.opcode === 0x1) {
        this.textHandler(frame.payload.toString("utf8"));
      } else if (frame.opcode === 0x8) {
        this.close();
        return;
      } else if (frame.opcode === 0x9) {
        this.sendFrame(0xa, frame.payload);
      }
    }
  }
}

function readFrame(buffer) {
  if (buffer.length < 2) {
    return null;
  }
  const opcode = buffer[0] & 0x0f;
  const masked = (buffer[1] & 0x80) !== 0;
  let length = buffer[1] & 0x7f;
  let offset = 2;
  if (length === 126) {
    if (buffer.length < offset + 2) return null;
    length = buffer.readUInt16BE(offset);
    offset += 2;
  } else if (length === 127) {
    if (buffer.length < offset + 8) return null;
    const bigLength = buffer.readBigUInt64BE(offset);
    offset += 8;
    if (bigLength > BigInt(MAX_WEBSOCKET_FRAME_BYTES)) {
      throw new Error("Selkies WebSocket frame is too large");
    }
    length = Number(bigLength);
  }
  if (length > MAX_WEBSOCKET_FRAME_BYTES) {
    throw new Error("Selkies WebSocket frame is too large");
  }
  let mask;
  if (masked) {
    if (buffer.length < offset + 4) return null;
    mask = buffer.subarray(offset, offset + 4);
    offset += 4;
  }
  if (buffer.length < offset + length) {
    return null;
  }
  const payload = Buffer.from(buffer.subarray(offset, offset + length));
  if (mask) {
    for (let index = 0; index < payload.length; index += 1) {
      payload[index] ^= mask[index % 4];
    }
  }
  return { opcode, payload, consumed: offset + length };
}

class SelkiesPage {
  constructor(config, launchRequest, onClosed = () => {}, options = {}) {
    this.config = config;
    this.launchRequest = launchRequest;
    this.pageId = pageIdFor(launchRequest.url, launchRequest.stream_id);
    this.onClosed = onClosed;
    this.forceNewTarget = options.forceNewTarget === true;
    this.ws = null;
    this.audioWs = null;
    this.serverPeerId = null;
    this.audioServerPeerId = null;
    this.signalingEnvelope = "peer_routed";
    this.audioSignalingEnvelope = "peer_routed";
    this.messages = [];
    this.audioMessages = [];
    this.waiters = [];
    this.audioWaiters = [];
    this.remoteCandidates = [];
    this.remoteCandidateHistory = [];
    this.audioRemoteCandidates = [];
    this.audioRemoteCandidateHistory = [];
    this.webrtcMedia = { audio: false, video: false };
    this.displaySession = null;
    this.signalingStats = {
      opened_at: null,
      last_selkies_message_at: null,
      last_browser_signal_at: null,
      answer_received_at: null,
      browser_answers_received: 0,
      browser_candidates_received: 0,
      browser_end_of_candidates_received: 0,
      selkies_messages_received: 0,
      selkies_offers_received: 0,
      selkies_candidates_received: 0,
      last_browser_candidate: null,
      last_selkies_candidate: null,
    };
    this.closed = false;
    this.audioClosed = false;
    this.resetSignaling();
    this.resetAudioSignaling();
  }

  async open() {
    this.signalingStats.opened_at = nowIso();
    const wallet = normalizeWalletBridge(this.launchRequest.wallet || {});
    this.wallet = wallet;
    const displaySize = displaySizeForLaunch(this.launchRequest, this.config);
    logControlEvent("page_open_browser_page_start", { page_id: this.pageId });
    const browserPage = await openBrowserPage(
      this.config,
      this.launchRequest.url,
      wallet,
      this.launchRequest,
      { forceNewTarget: this.forceNewTarget },
    );
    browserPage.page_id = this.pageId;
    logControlEvent("page_open_browser_page_done", {
      page_id: this.pageId,
      target_id: browserPage.target_id || null,
      actual_url: browserPage.url || null,
    });
    browserPage._stopWalletBridgeWatch = startWalletBridgeWatch(
      browserPage,
      wallet,
      this.config.browserControl.timeoutMs,
    );
    this.browserPage = browserPage;
    logControlEvent("page_open_video_sdp_start", { page_id: this.pageId });
    const videoOffer = await this.openWebRtcSession(displaySize);
    logControlEvent("page_open_video_sdp_done", { page_id: this.pageId });
    logControlEvent("page_open_audio_sdp_start", { page_id: this.pageId });
    const audioOffer = await this.openAudioWebRtcSession(displaySize);
    logControlEvent("page_open_audio_sdp_done", { page_id: this.pageId });
    const result = this.supervisorResult(
      videoOffer.sdp.sdp,
      browserPage,
      wallet,
      audioOffer.sdp.sdp,
    );
    this.displaySession = result.display_session || null;
    return result;
  }

  createWebSocket() {
    const ws = new MinimalWebSocketClient(this.config.selkiesWsUrl, { basicAuth: this.config.basicAuth });
    ws.onText((message) => {
      if (this.ws === ws) {
        this.handleMessage(message);
      }
    });
    ws.onClose(() => {
      if (this.ws === ws) {
        this.markClosed();
      }
    });
    return ws;
  }

  createAudioWebSocket() {
    const ws = new MinimalWebSocketClient(this.config.selkiesWsUrl, { basicAuth: this.config.basicAuth });
    ws.onText((message) => {
      if (this.audioWs === ws) {
        this.handleAudioMessage(message);
      }
    });
    ws.onClose(() => {
      if (this.audioWs === ws) {
        this.markAudioClosed();
      }
    });
    return ws;
  }

  resetSignaling() {
    if (this.ws) {
      try {
        this.ws.close();
      } catch (error) {}
    }
    for (const waiter of this.waiters) {
      clearTimeout(waiter.timer);
    }
    this.ws = this.createWebSocket();
    this.serverPeerId = null;
    this.signalingEnvelope = "peer_routed";
    this.messages = [];
    this.waiters = [];
    this.remoteCandidates = [];
    this.remoteCandidateHistory = [];
    this.closed = false;
  }

  resetAudioSignaling() {
    if (this.audioWs) {
      try {
        this.audioWs.close();
      } catch (error) {}
    }
    for (const waiter of this.audioWaiters) {
      clearTimeout(waiter.timer);
    }
    this.audioWs = this.createAudioWebSocket();
    this.audioServerPeerId = null;
    this.audioSignalingEnvelope = "peer_routed";
    this.audioMessages = [];
    this.audioWaiters = [];
    this.audioRemoteCandidates = [];
    this.audioRemoteCandidateHistory = [];
    this.audioClosed = false;
  }

  async openWebRtcSession(displaySize) {
    if (this.config.signalingProtocol === "legacy") {
      return await this.openLegacySelkiesSession(displaySize);
    }
    if (this.config.signalingProtocol === "current") {
      return await this.openCurrentSelkiesSession();
    }
    try {
      return await this.openCurrentSelkiesSession();
    } catch (error) {
      if (!isCurrentSelkiesHelloFailure(error)) {
        throw error;
      }
      logControlEvent("selkies_legacy_handshake_retry", {
        page_id: this.pageId,
        message: error instanceof Error ? error.message : String(error),
      });
      this.resetSignaling();
      return await this.openLegacySelkiesSession(displaySize);
    }
  }

  async openAudioWebRtcSession(displaySize) {
    if (this.signalingEnvelope === "raw_json") {
      return await this.openLegacySelkiesAudioSession(displaySize);
    }
    if (this.signalingEnvelope === "peer_routed" && this.config.signalingProtocol !== "auto") {
      return await this.openCurrentSelkiesAudioSession();
    }
    if (this.config.signalingProtocol === "current") {
      return await this.openCurrentSelkiesAudioSession();
    }
    if (this.config.signalingProtocol === "auto") {
      try {
        return await this.openCurrentSelkiesAudioSession();
      } catch (error) {
        if (!isCurrentSelkiesHelloFailure(error)) {
          throw error;
        }
        this.resetAudioSignaling();
      }
    }
    return await this.openLegacySelkiesAudioSession(displaySize);
  }

  async openCurrentSelkiesAudioSession() {
    await this.audioWs.connect(this.config.connectTimeoutMs);
    this.audioWs.sendText("HELLO client " + JSON.stringify({ client_type: "controller", client_slot: 3, client_strict_viewer: false }));
    await this.waitForAudio((message) => message.kind === "hello", "Selkies audio HELLO");
    this.audioWs.sendText("SESSION server");
    const session = await this.waitForAudio((message) => message.kind === "session_ok", "Selkies audio SESSION_OK");
    this.audioServerPeerId = session.serverPeerId;
    this.audioSignalingEnvelope = "peer_routed";
    return await this.waitForAudio(
      (message) => message.sdp?.type === "offer" && typeof message.sdp.sdp === "string",
      "Selkies audio SDP offer",
    );
  }

  async openLegacySelkiesAudioSession(displaySize) {
    await this.audioWs.connect(this.config.connectTimeoutMs);
    const helloMeta = Buffer.from(JSON.stringify({
      res: `${this.config.displaySurface.stream.width}x${this.config.displaySurface.stream.height}`,
      scale: displaySize.scale || 1,
    })).toString("base64");
    this.audioWs.sendText(`HELLO 3 ${helloMeta}`);
    await this.waitForAudio((message) => message.kind === "hello", "legacy Selkies audio HELLO");
    const offer = await this.waitForAudio(
      (message) => message.sdp?.type === "offer" && typeof message.sdp.sdp === "string",
      "legacy Selkies audio SDP offer",
    );
    this.audioServerPeerId = offer.from || "2";
    this.audioSignalingEnvelope = "raw_json";
    return offer;
  }

  async openCurrentSelkiesSession() {
    await this.ws.connect(this.config.connectTimeoutMs);
    this.ws.sendText("HELLO client " + JSON.stringify({ client_type: "controller", client_slot: 1, client_strict_viewer: false }));
    await this.waitFor((message) => message.kind === "hello", "Selkies HELLO");
    this.ws.sendText("SESSION server");
    const session = await this.waitFor((message) => message.kind === "session_ok", "Selkies SESSION_OK");
    this.serverPeerId = session.serverPeerId;
    this.signalingEnvelope = "peer_routed";
    const offer = await this.waitFor(
      (message) => message.sdp?.type === "offer" && typeof message.sdp.sdp === "string",
      "Selkies SDP offer",
    );
    return offer;
  }

  async openLegacySelkiesSession(displaySize) {
    await this.ws.connect(this.config.connectTimeoutMs);
    const helloMeta = Buffer.from(JSON.stringify({
      res: `${this.config.displaySurface.stream.width}x${this.config.displaySurface.stream.height}`,
      scale: displaySize.scale || 1,
    })).toString("base64");
    this.ws.sendText(`HELLO 1 ${helloMeta}`);
    await this.waitFor((message) => message.kind === "hello", "legacy Selkies HELLO");
    const offer = await this.waitFor(
      (message) => message.sdp?.type === "offer" && typeof message.sdp.sdp === "string",
      "legacy Selkies SDP offer",
    );
    this.serverPeerId = offer.from || "1";
    this.signalingEnvelope = "raw_json";
    return offer;
  }

  supervisorResult(sdp, browserPage, wallet, audioOfferSdp) {
    const displaySize = displaySizeForLaunch(this.launchRequest, this.config);
    const audioSdp = normalizeAudioOfferSdp(audioOfferSdp);
    const media = mediaKindsForSdp(sdp);
    const audioMedia = mediaKindsForSdp(audioSdp);
    this.webrtcMedia = { audio: audioMedia.audio, video: media.video };
    return {
      schema: "elastos.browser.engine.supervisor-result/v1",
      page_id: this.pageId,
      adapter: this.launchRequest.adapter,
      engine: this.launchRequest.engine,
      stream_id: this.launchRequest.stream_id,
      actual_url: browserPage.url || this.launchRequest.url,
      title: browserPage.title || "Selkies Browser",
      network_mode: "runtime_net_only",
      direct_network: false,
      wallet_injection: false,
      wallet_bridge: {
        schema: "elastos.browser.wallet-bridge/v1",
        mode: "runtime_mediated_eip1193",
        accounts: wallet.accounts.length,
        default_chain_namespace: wallet.default_chain_namespace,
        signing: "approval_required",
      },
      view: {
        schema: "elastos.browser.view/v1",
        mode: "webrtc_remote_display",
        width: displaySize.width,
        height: displaySize.height,
      },
      display_session: {
        schema: "elastos.browser.display-session/v1",
        session_id: `display:${this.launchRequest.stream_id}`,
        mode: "webrtc_remote_display",
        width: this.config.displaySurface.stream.width,
        height: this.config.displaySurface.stream.height,
        input: "datachannel",
        input_protocol: "selkies_v1",
        offerer: "engine",
        initial_offer: {
          schema: "elastos.browser.webrtc-offer/v1",
          type: "offer",
          sdp,
          candidates: this.remoteCandidateHistory.slice(),
        },
        audio_offer: {
          schema: "elastos.browser.webrtc-offer/v1",
          type: "offer",
          sdp: audioSdp,
          candidates: this.audioRemoteCandidateHistory.slice(),
        },
        display_backend: "selkies_gstreamer_webrtc",
        backend_class: "product_compositor",
        media_transport: "runtime_relay",
        audio: audioMedia.audio,
        video: media.video,
        ice_servers: this.config.iceServers,
        network_mode: "runtime_net_only",
        direct_network: false,
        signaling_url: `/api/apps/browser/pages/${encodeURIComponent(this.pageId)}/webrtc`,
      },
    };
  }

  handleMessage(raw) {
    const parsed = parseSelkiesMessage(raw);
    if (parsed) {
      this.signalingStats.selkies_messages_received += 1;
      this.signalingStats.last_selkies_message_at = nowIso();
    }
    if (parsed?.sdp?.type === "offer") {
      this.signalingStats.selkies_offers_received += 1;
    }
    if (parsed?.ice) {
      this.remoteCandidates.push(parsed.ice);
      this.remoteCandidateHistory.push(parsed.ice);
      this.signalingStats.selkies_candidates_received += 1;
      this.signalingStats.last_selkies_candidate = summarizeIceCandidate(parsed.ice);
    }
    if (parsed) {
      this.messages.push(parsed);
      this.flushWaiters();
    }
  }

  handleAudioMessage(raw) {
    const parsed = parseSelkiesMessage(raw);
    if (parsed) {
      this.signalingStats.selkies_messages_received += 1;
      this.signalingStats.last_selkies_message_at = nowIso();
    }
    if (parsed?.sdp?.type === "offer") {
      this.signalingStats.selkies_offers_received += 1;
    }
    if (parsed?.ice) {
      this.audioRemoteCandidates.push(parsed.ice);
      this.audioRemoteCandidateHistory.push(parsed.ice);
      this.signalingStats.selkies_candidates_received += 1;
      this.signalingStats.last_selkies_candidate = summarizeIceCandidate(parsed.ice);
    }
    if (parsed) {
      this.audioMessages.push(parsed);
      this.flushAudioWaiters();
    }
  }

  flushWaiters() {
    for (const waiter of [...this.waiters]) {
      const matchIndex = this.messages.findIndex(waiter.predicate);
      if (matchIndex >= 0) {
        const [message] = this.messages.splice(matchIndex, 1);
        this.waiters = this.waiters.filter((entry) => entry !== waiter);
        clearTimeout(waiter.timer);
        waiter.resolve(message);
      } else if (this.closed) {
        this.waiters = this.waiters.filter((entry) => entry !== waiter);
        clearTimeout(waiter.timer);
        waiter.reject(new Error(`Selkies WebSocket closed while waiting for ${waiter.label}`));
      }
    }
  }

  flushAudioWaiters() {
    for (const waiter of [...this.audioWaiters]) {
      const matchIndex = this.audioMessages.findIndex(waiter.predicate);
      if (matchIndex >= 0) {
        const [message] = this.audioMessages.splice(matchIndex, 1);
        this.audioWaiters = this.audioWaiters.filter((entry) => entry !== waiter);
        clearTimeout(waiter.timer);
        waiter.resolve(message);
      } else if (this.audioClosed) {
        this.audioWaiters = this.audioWaiters.filter((entry) => entry !== waiter);
        clearTimeout(waiter.timer);
        waiter.reject(new Error(`Selkies audio WebSocket closed while waiting for ${waiter.label}`));
      }
    }
  }

  waitFor(predicate, label) {
    const matchIndex = this.messages.findIndex(predicate);
    if (matchIndex >= 0) {
      const [message] = this.messages.splice(matchIndex, 1);
      return Promise.resolve(message);
    }
    if (this.closed) {
      return Promise.reject(new Error(`Selkies WebSocket closed while waiting for ${label}`));
    }
    return new Promise((resolve, reject) => {
      const waiter = {
        predicate,
        label,
        resolve,
        reject,
        timer: setTimeout(() => {
          this.waiters = this.waiters.filter((entry) => entry !== waiter);
          reject(new Error(`timed out waiting for ${label}`));
        }, this.config.signalTimeoutMs),
      };
      this.waiters.push(waiter);
    });
  }

  waitForAudio(predicate, label) {
    const matchIndex = this.audioMessages.findIndex(predicate);
    if (matchIndex >= 0) {
      const [message] = this.audioMessages.splice(matchIndex, 1);
      return Promise.resolve(message);
    }
    if (this.audioClosed) {
      return Promise.reject(new Error(`Selkies audio WebSocket closed while waiting for ${label}`));
    }
    return new Promise((resolve, reject) => {
      const waiter = {
        predicate,
        label,
        resolve,
        reject,
        timer: setTimeout(() => {
          this.audioWaiters = this.audioWaiters.filter((entry) => entry !== waiter);
          reject(new Error(`timed out waiting for ${label}`));
        }, this.config.signalTimeoutMs),
      };
      this.audioWaiters.push(waiter);
    });
  }

  signal(signal, channel = "video") {
    if (channel === "audio") {
      return this.signalAudio(signal);
    }
    if (this.signalingEnvelope !== "raw_json" && !this.serverPeerId) {
      throw new Error("Selkies server peer is unavailable");
    }
    this.signalingStats.last_browser_signal_at = nowIso();
    if (signal.schema === "elastos.browser.webrtc-answer/v1") {
      this.signalingStats.browser_answers_received += 1;
      this.signalingStats.answer_received_at = this.signalingStats.last_browser_signal_at;
      this.sendSignal({ sdp: { type: "answer", sdp: signal.sdp } });
      return this.ack("answer");
    }
    if (signal.schema === "elastos.browser.webrtc-candidate/v1") {
      this.signalingStats.browser_candidates_received += 1;
      this.signalingStats.last_browser_candidate = summarizeIceCandidate(signal.candidate);
      this.sendSignal({ ice: signal.candidate });
      return this.ack("candidate");
    }
    if (signal.schema === "elastos.browser.webrtc-end-of-candidates/v1") {
      this.signalingStats.browser_end_of_candidates_received += 1;
      return this.ack("end_of_candidates");
    }
    throw new Error("unsupported WebRTC signal for Selkies control service");
  }

  signalAudio(signal) {
    if (this.audioSignalingEnvelope !== "raw_json" && !this.audioServerPeerId) {
      throw new Error("Selkies audio server peer is unavailable");
    }
    this.signalingStats.last_browser_signal_at = nowIso();
    if (signal.schema === "elastos.browser.webrtc-answer/v1") {
      this.signalingStats.browser_answers_received += 1;
      this.signalingStats.answer_received_at = this.signalingStats.last_browser_signal_at;
      this.sendAudioSignal({ sdp: { type: "answer", sdp: signal.sdp } });
      return this.ackAudio("answer");
    }
    if (signal.schema === "elastos.browser.webrtc-candidate/v1") {
      this.signalingStats.browser_candidates_received += 1;
      this.signalingStats.last_browser_candidate = summarizeIceCandidate(signal.candidate);
      this.sendAudioSignal({ ice: signal.candidate });
      return this.ackAudio("candidate");
    }
    if (signal.schema === "elastos.browser.webrtc-end-of-candidates/v1") {
      this.signalingStats.browser_end_of_candidates_received += 1;
      return this.ackAudio("end_of_candidates");
    }
    throw new Error("unsupported audio WebRTC signal for Selkies control service");
  }

  sendSignal(payload) {
    const serialized = JSON.stringify(payload);
    if (this.signalingEnvelope === "raw_json") {
      this.ws.sendText(serialized);
      return;
    }
    this.ws.sendText(`${this.serverPeerId} ${serialized}`);
  }

  sendAudioSignal(payload) {
    const serialized = JSON.stringify(payload);
    if (this.audioSignalingEnvelope === "raw_json") {
      this.audioWs.sendText(serialized);
      return;
    }
    this.audioWs.sendText(`${this.audioServerPeerId} ${serialized}`);
  }

  signalingDebug() {
    return {
      ...this.signalingStats,
      pending_selkies_candidates: this.remoteCandidates.length,
      websocket_closed: this.closed,
    };
  }

  ack(type) {
    const candidates = type === "answer"
      ? this.remoteCandidateHistory.slice()
      : this.remoteCandidates.splice(0);
    if (type === "answer") {
      this.remoteCandidates = [];
    }
    return {
      schema: "elastos.browser.webrtc-signal-ack/v1",
      page_id: this.pageId,
      type,
      accepted: true,
      candidates,
      end_of_candidates: false,
    };
  }

  ackAudio(type) {
    const candidates = type === "answer"
      ? this.audioRemoteCandidateHistory.slice()
      : this.audioRemoteCandidates.splice(0);
    if (type === "answer") {
      this.audioRemoteCandidates = [];
    }
    return {
      schema: "elastos.browser.webrtc-signal-ack/v1",
      page_id: this.pageId,
      type,
      accepted: true,
      candidates,
      end_of_candidates: false,
    };
  }

  close() {
    this.markClosed();
    this.ws.close();
    this.markAudioClosed();
    this.audioWs.close();
    if (this.browserPage?.target_id) {
      closeBrowserPage(this.config.browserControl, this.browserPage).catch(() => {});
    }
  }

  markClosed() {
    if (this.closed) {
      return;
    }
    this.closed = true;
    this.onClosed(this);
    this.flushWaiters();
  }

  markAudioClosed() {
    if (this.audioClosed) {
      return;
    }
    this.audioClosed = true;
    this.flushAudioWaiters();
  }
}

class CdpClient {
  constructor(webSocketUrl, defaultTimeoutMs = 15000) {
    this.ws = new MinimalWebSocketClient(new URL(webSocketUrl));
    this.defaultTimeoutMs = defaultTimeoutMs;
    this.nextId = 1;
    this.pending = new Map();
    this.events = [];
    this.waiters = [];
    this.eventHandlers = new Map();
    this.closed = true;
  }

  async connect(timeoutMs) {
    await this.ws.connect(timeoutMs);
    this.closed = false;
    this.ws.onText((message) => this.handleMessage(message));
    const rejectPending = (reason) => {
      this.closed = true;
      for (const pending of this.pending.values()) {
        clearTimeout(pending.timer);
        pending.reject(new Error(reason));
      }
      this.pending.clear();
      for (const waiter of this.waiters.splice(0)) {
        clearTimeout(waiter.timer);
        waiter.reject(new Error(`${reason} while waiting for ${waiter.label}`));
      }
    };
    this.ws.onError((error) => {
      rejectPending(`browser CDP WebSocket error: ${error instanceof Error ? error.message : String(error)}`);
    });
    this.ws.onClose(() => {
      rejectPending("browser CDP WebSocket closed");
    });
  }

  request(method, params = {}, timeoutMs = this.defaultTimeoutMs) {
    if (this.closed) {
      return Promise.reject(new Error("browser CDP WebSocket closed"));
    }
    const id = this.nextId++;
    this.ws.sendText(JSON.stringify({ id, method, params }));
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(new Error(`timed out waiting for browser CDP response ${method}`));
      }, timeoutMs);
      this.pending.set(id, { resolve, reject, timer });
    });
  }

  waitForEvent(method, timeoutMs, label = method) {
    const existingIndex = this.events.findIndex((event) => event.method === method);
    if (existingIndex >= 0) {
      const [event] = this.events.splice(existingIndex, 1);
      return Promise.resolve(event.params || {});
    }
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.waiters = this.waiters.filter((entry) => entry.timer !== timer);
        reject(new Error(`timed out waiting for browser CDP event ${label}`));
      }, timeoutMs);
      this.waiters.push({ method, label, timer, resolve, reject });
    });
  }

  clearEvents(methods) {
    const blocked = new Set(methods || []);
    this.events = this.events.filter((event) => !blocked.has(event.method));
  }

  onEvent(method, handler) {
    const handlers = this.eventHandlers.get(method) || [];
    handlers.push(handler);
    this.eventHandlers.set(method, handlers);
    return () => {
      const current = this.eventHandlers.get(method) || [];
      this.eventHandlers.set(method, current.filter((item) => item !== handler));
    };
  }

  handleMessage(raw) {
    let message;
    try {
      message = JSON.parse(raw);
    } catch {
      return;
    }
    if (Number.isInteger(message.id)) {
      const pending = this.pending.get(message.id);
      if (!pending) {
        return;
      }
      this.pending.delete(message.id);
      clearTimeout(pending.timer);
      if (message.error) {
        pending.reject(new Error(message.error.message || "browser CDP request failed"));
      } else {
        pending.resolve(message.result || {});
      }
      return;
    }
    if (typeof message.method !== "string") {
      return;
    }
    const waiter = this.waiters.find((entry) => entry.method === message.method);
    if (waiter) {
      this.waiters = this.waiters.filter((entry) => entry !== waiter);
      clearTimeout(waiter.timer);
      waiter.resolve(message.params || {});
      return;
    }
    for (const handler of this.eventHandlers.get(message.method) || []) {
      Promise.resolve()
        .then(() => handler(message.params || {}))
        .catch((error) => {
          console.error(JSON.stringify({
            schema: "elastos.browser.selkies-control.cdp-event-handler-error/v1",
            method: message.method,
            error: error instanceof Error ? error.message : String(error),
          }));
        });
    }
    this.events.push(message);
    if (this.events.length > 100) {
      this.events.splice(0, this.events.length - 100);
    }
  }

  close() {
    this.closed = true;
    this.ws.close();
  }
}

async function fetchBrowserControlJson(browserControl, path, { method = "GET" } = {}) {
  if (browserControl.kind !== "cdp_http") {
    throw new Error("unsupported browser control kind");
  }
  const target = new URL(path, browserControl.endpoint);
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), browserControl.timeoutMs);
  let response;
  try {
    response = await fetch(target, { method, signal: controller.signal });
  } catch (error) {
    throw new Error(`browser CDP control ${method} ${target.pathname} failed: ${error instanceof Error ? error.message : String(error)}`);
  } finally {
    clearTimeout(timer);
  }
  if (!response.ok) {
    throw new Error(`browser CDP control ${method} ${target.pathname} failed: HTTP ${response.status}`);
  }
  try {
    return await response.json();
  } catch (error) {
    throw new Error(`browser CDP control ${method} ${target.pathname} returned invalid JSON: ${error instanceof Error ? error.message : String(error)}`);
  }
}

async function fetchBrowserControl(browserControl, path, { method = "GET" } = {}) {
  if (browserControl.kind !== "cdp_http") {
    throw new Error("unsupported browser control kind");
  }
  const target = new URL(path, browserControl.endpoint);
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), browserControl.timeoutMs);
  try {
    await fetch(target, { method, signal: controller.signal });
  } catch {
    // Best-effort CDP browser-control endpoints such as /json/activate are not authority-critical.
  } finally {
    clearTimeout(timer);
  }
}

function usableBrowserTarget(value) {
  return (
    value &&
    value.type === "page" &&
    typeof value.id === "string" &&
    value.id.length > 0 &&
    typeof value.webSocketDebuggerUrl === "string" &&
    value.webSocketDebuggerUrl.length > 0
  );
}

async function browserPageTarget(browserControl, preferredTargetId = "", options = {}) {
  if (options.forceNew !== true) {
    const targets = await fetchBrowserControlJson(browserControl, "/json/list");
    const pages = Array.isArray(targets) ? targets.filter(usableBrowserTarget) : [];
    const preferred = pages.find((page) => page.id === preferredTargetId);
    if (preferred) {
      return preferred;
    }
    if (pages[0]) {
      return pages[0];
    }
  }
  const created = await fetchBrowserControlJson(browserControl, "/json/new?about:blank", { method: "PUT" });
  if (!usableBrowserTarget(created)) {
    throw new Error("browser CDP navigation did not return a page debugger URL");
  }
  return created;
}

async function activateBrowserTarget(browserControl, targetId) {
  if (targetId && !/[\s\0/]/.test(targetId)) {
    await fetchBrowserControl(browserControl, `/json/activate/${encodeURIComponent(targetId)}`);
  }
}

async function closeBrowserTarget(browserControl, targetId) {
  if (targetId && !/[\s\0/]/.test(targetId)) {
    await fetchBrowserControl(browserControl, `/json/close/${encodeURIComponent(targetId)}`);
  }
}

async function applyBrowserViewport(cdp, launch, config) {
  const displaySize = displaySizeForLaunch(launch, config);
  await cdp.request("Emulation.setDeviceMetricsOverride", {
    width: displaySize.width,
    height: displaySize.height,
    deviceScaleFactor: config.displaySurface.deviceScaleFactor,
    mobile: false,
    screenWidth: displaySize.width,
    screenHeight: displaySize.height,
  });
  return displaySize;
}

function walletRuntimeHttpError(message, code = 4100) {
  const error = new Error(message);
  error.code = code;
  return error;
}

function runtimeFetchProxyOrigin(proxyUrl) {
  if (!proxyUrl) {
    return null;
  }
  return `${proxyUrl.protocol}//${proxyUrl.host}`;
}

function visibleWalletBridgePayload(payload) {
  return {
    schema: "elastos.browser.wallet-bridge/v1",
    accounts: Array.isArray(payload?.accounts) ? payload.accounts : [],
    default_chain_namespace:
      typeof payload?.default_chain_namespace === "string" ? payload.default_chain_namespace : "",
    default_account_id:
      typeof payload?.default_account_id === "string" ? payload.default_account_id : "",
    signing: "approval_required",
    authority: "runtime_mediated",
    transport: "cdp_runtime_binding",
  };
}

function updateWalletRuntimeState(runtime, payload) {
  const next = normalizeWalletBridge({
    ...runtime.wallet,
    accounts: Array.isArray(payload?.accounts) ? payload.accounts : runtime.wallet.accounts,
    default_chain_namespace:
      typeof payload?.default_chain_namespace === "string"
        ? payload.default_chain_namespace
        : runtime.wallet.default_chain_namespace,
    default_account_id:
      typeof payload?.default_account_id === "string"
        ? payload.default_account_id
        : runtime.wallet.default_account_id,
    bridge_url: typeof payload?.bridge_url === "string" ? payload.bridge_url : runtime.wallet.bridge_url,
    approval_url:
      typeof payload?.approval_url === "string" ? payload.approval_url : runtime.wallet.approval_url,
    transaction_url:
      typeof payload?.transaction_url === "string" ? payload.transaction_url : runtime.wallet.transaction_url,
    read_url: typeof payload?.read_url === "string" ? payload.read_url : runtime.wallet.read_url,
    transaction_broadcast_url:
      typeof payload?.transaction_broadcast_url === "string"
        ? payload.transaction_broadcast_url
        : runtime.wallet.transaction_broadcast_url,
    approval_status_url:
      typeof payload?.approval_status_url === "string"
        ? payload.approval_status_url
        : runtime.wallet.approval_status_url,
    home_token: typeof payload?.home_token === "string" ? payload.home_token : runtime.wallet.home_token,
  });
  runtime.wallet = next;
  return next;
}

async function walletRuntimeFetchJson(runtime, url, { method = "GET", body = null } = {}) {
  if (!url || !runtime.wallet.home_token) {
    throw walletRuntimeHttpError("Runtime wallet endpoint is unavailable for this Browser session.");
  }
  const target = new URL(url);
  const timeoutMs = Math.min(runtime.timeoutMs, 30000);
  const jsonBody = body == null ? null : JSON.stringify(body);
  let status = 0;
  let text = "";
  try {
    if (!runtime.runtimeFetchProxyUrl) {
      throw new Error("Runtime wallet bridge proxy is required for Browser VM sessions.");
    }
    ({ status, text } = await walletRuntimeFetchViaProxy(runtime, target, method, jsonBody, timeoutMs));
  } catch (error) {
    throw walletRuntimeHttpError(
      `Runtime wallet endpoint request failed: ${error instanceof Error ? error.message : String(error)}`,
    );
  }
  let payload = null;
  if (text) {
    try {
      payload = JSON.parse(text);
    } catch {
      payload = text;
    }
  }
  if (status < 200 || status >= 300) {
    throw walletRuntimeHttpError(
      typeof payload === "string"
        ? payload
        : payload?.message || payload?.error || "Runtime wallet request failed.",
      status === 400 ? 4001 : 4100,
    );
  }
  return payload;
}

function walletRuntimeFetchHeaders(runtime, jsonBody) {
  const headers = {
    "content-type": "application/json",
    "x-elastos-home-token": runtime.wallet.home_token,
  };
  if (jsonBody != null) {
    headers["content-length"] = String(Buffer.byteLength(jsonBody));
  }
  return headers;
}

function walletRuntimeFetchViaProxy(runtime, target, method, jsonBody, timeoutMs) {
  if (target.protocol !== "http:") {
    throw new Error("Runtime wallet proxy supports HTTP bridge endpoints only.");
  }
  const proxyUrl = runtime.runtimeFetchProxyUrl;
  const headers = {
    ...walletRuntimeFetchHeaders(runtime, jsonBody),
    host: target.host,
    connection: "close",
  };
  return new Promise((resolve, reject) => {
    const req = http.request(
      {
        host: proxyUrl.hostname === "localhost" ? "127.0.0.1" : proxyUrl.hostname,
        port: Number(proxyUrl.port || 80),
        method,
        path: target.href,
        headers,
        timeout: timeoutMs,
      },
      (res) => {
        const chunks = [];
        res.on("data", (chunk) => chunks.push(Buffer.from(chunk)));
        res.on("end", () => resolve({
          status: res.statusCode || 0,
          text: Buffer.concat(chunks).toString("utf8"),
        }));
      },
    );
    req.on("timeout", () => req.destroy(new Error("request timed out")));
    req.on("error", reject);
    if (jsonBody != null) {
      req.write(jsonBody);
    }
    req.end();
  });
}

async function dispatchWalletRuntimeBinding(runtime, message) {
  if (message.action === "bridge") {
    const payload = await walletRuntimeFetchJson(runtime, runtime.wallet.bridge_url);
    updateWalletRuntimeState(runtime, payload);
    return visibleWalletBridgePayload(payload);
  }
  if (message.action === "post") {
    const operation = String(message.operation || "");
    const urlField = WALLET_RUNTIME_POST_URL_FIELDS[operation];
    if (!urlField) {
      throw walletRuntimeHttpError("Unsupported Runtime wallet bridge operation.", 4001);
    }
    return walletRuntimeFetchJson(runtime, runtime.wallet[urlField], {
      method: "POST",
      body: message.body || {},
    });
  }
  if (message.action === "approvalStatus") {
    const requestId = String(message.request_id || "");
    if (!safeId(requestId)) {
      throw walletRuntimeHttpError("Runtime wallet approval request id is invalid.", 4001);
    }
    const base = String(runtime.wallet.approval_status_url || "").replace(/\/+$/, "");
    return walletRuntimeFetchJson(runtime, `${base}/${encodeURIComponent(requestId)}`);
  }
  throw walletRuntimeHttpError("Unsupported Runtime wallet bridge action.", 4001);
}

async function resolveWalletRuntimeBinding(cdp, id, response) {
  const payload = JSON.stringify({ id, ...response });
  await cdp.request(
    "Runtime.evaluate",
    {
      expression:
        `globalThis[${JSON.stringify(WALLET_RUNTIME_RESULT)}] && ` +
        `globalThis[${JSON.stringify(WALLET_RUNTIME_RESULT)}](${JSON.stringify(payload)})`,
      awaitPromise: false,
    },
    5000,
  );
}

async function handleWalletRuntimeBinding(cdp, runtime, params) {
  if (params.name !== WALLET_RUNTIME_BINDING) {
    return;
  }
  let message = {};
  try {
    message = JSON.parse(String(params.payload || "{}"));
  } catch {
    message = {};
  }
  const id = typeof message.id === "string" && message.id ? message.id : "";
  if (!id) {
    return;
  }
  try {
    const result = await dispatchWalletRuntimeBinding(runtime, message);
    await resolveWalletRuntimeBinding(cdp, id, { ok: true, result });
  } catch (error) {
    await resolveWalletRuntimeBinding(cdp, id, {
      ok: false,
      error: {
        message: error instanceof Error ? error.message : String(error),
        code: Number.isInteger(error?.code) ? error.code : 4100,
      },
    });
  }
}

async function installWalletRuntimeBinding(cdp, wallet, timeoutMs) {
  const runtime = cdp.walletRuntime || {
    wallet: normalizeWalletBridge(wallet),
    timeoutMs,
    runtimeFetchProxyUrl: cdp.runtimeFetchProxyUrl || null,
  };
  runtime.wallet = normalizeWalletBridge({
    ...runtime.wallet,
    ...wallet,
  });
  runtime.timeoutMs = timeoutMs;
  runtime.runtimeFetchProxyUrl = cdp.runtimeFetchProxyUrl || null;
  cdp.walletRuntime = runtime;
  if (cdp.walletRuntimeBindingInstalled) {
    return;
  }
  await cdp.request("Runtime.addBinding", { name: WALLET_RUNTIME_BINDING });
  cdp.onEvent("Runtime.bindingCalled", (params) => handleWalletRuntimeBinding(cdp, runtime, params));
  cdp.walletRuntimeBindingInstalled = true;
}

async function installWalletBridge(cdp, wallet) {
  await installWalletRuntimeBinding(cdp, wallet, cdp.defaultTimeoutMs);
  if (cdp.walletBridgeInstalled) {
    return cdp.walletBridgeInitScriptId || "";
  }
  const source = walletInitScript(wallet);
  const initScript = await cdp.request("Page.addScriptToEvaluateOnNewDocument", { source });
  const evaluated = await cdp.request("Runtime.evaluate", { expression: source, awaitPromise: false });
  if (evaluated.exceptionDetails) {
    throw new Error("Browser wallet bridge injection failed");
  }
  cdp.walletBridgeInstalled = true;
  cdp.walletBridgeInitScriptId = typeof initScript.identifier === "string" ? initScript.identifier : "";
  return cdp.walletBridgeInitScriptId;
}

function startWalletBridgeWatch(browserPage, wallet, timeoutMs) {
  const cdp = browserPage?._cdp;
  if (!cdp) {
    return null;
  }
  const source = walletInitScript(wallet);
  let stopped = false;
  const ensure = () => {
    if (stopped) {
      return;
    }
    if (browserPage.navigationInProgress === true) {
      return;
    }
    cdp
      .request("Runtime.evaluate", { expression: source, awaitPromise: false })
      .catch(() => {
        stopped = true;
      });
  };
  const timer = setInterval(ensure, Math.max(750, Math.min(timeoutMs, 2000)));
  return () => {
    stopped = true;
    clearInterval(timer);
  };
}

async function openBrowserPage(config, url, wallet, launch, options = {}) {
  const browserControl = config.browserControl;
  logControlEvent("browser_page_target_start", {
    stream_id: launch?.stream_id || null,
    force_new_target: options.forceNewTarget === true,
  });
  const body = await browserPageTarget(browserControl, "", { forceNew: options.forceNewTarget === true });
  if (typeof body.webSocketDebuggerUrl !== "string" || !body.webSocketDebuggerUrl) {
    throw new Error("browser CDP navigation did not return a page debugger URL");
  }
  logControlEvent("browser_page_target_done", {
    stream_id: launch?.stream_id || null,
    target_id: body.id || null,
  });
  await activateBrowserTarget(browserControl, body.id);
  const cdp = new CdpClient(body.webSocketDebuggerUrl, browserControl.timeoutMs);
  cdp.runtimeFetchProxyUrl = config.runtimeFetchProxyUrl;
  const browserPage = {
    target_id: typeof body.id === "string" ? body.id : "",
    debugger_url: body.webSocketDebuggerUrl,
    init_script_id: "",
    wallet,
    url: "",
    title: "Selkies Browser",
    file_chooser: createBrowserFileChooserState(),
    runtimeFetchProxyUrl: config.runtimeFetchProxyUrl,
    launchViewport: launch?.viewport
      ? { width: launch.viewport.width, height: launch.viewport.height }
      : null,
    navigationInProgress: false,
    _cdp: cdp,
  };
  let initScriptId = "";
  let keepCdp = false;
  try {
    logControlEvent("browser_page_cdp_connect_start", { target_id: body.id || null });
    await cdp.connect(browserControl.timeoutMs);
    logControlEvent("browser_page_cdp_connect_done", { target_id: body.id || null });
    await cdp.request("Page.enable");
    await cdp.request("Runtime.enable");
    await cdp.request("Network.enable").catch(() => {});
    await cdp.request("Log.enable").catch(() => {});
    await ensureBrowserFileChooserInterception(cdp, browserPage);
    await applyBrowserViewport(cdp, launch, config);
    logControlEvent("browser_page_wallet_bridge_start", { target_id: body.id || null });
    initScriptId = await installWalletBridge(cdp, wallet);
    logControlEvent("browser_page_wallet_bridge_done", { target_id: body.id || null });
    logControlEvent("browser_page_initial_navigation_start", { target_id: body.id || null, url });
    const navigation = await navigateInitialBrowserPage(cdp, browserControl, url);
    logControlEvent("browser_page_initial_navigation_done", {
      target_id: body.id || null,
      error_text: navigation.errorText || null,
    });
    console.error(JSON.stringify({
      schema: "elastos.browser.selkies-control.navigation/v1",
      url,
      frame_id: navigation.frameId || null,
      loader_id: navigation.loaderId || null,
      error_text: navigation.errorText || null,
    }));
    const page = await readBrowserPageState(cdp, url, "Selkies Browser", browserControl.timeoutMs);
    console.error(JSON.stringify({
      schema: "elastos.browser.selkies-control.navigation-state/v1",
      url: page.url,
      title: page.title,
    }));
    assertBrowserStateDidNotLandOnErrorPage(page, "initial navigation");
    keepCdp = true;
    browserPage.init_script_id = initScriptId;
    browserPage.url = page.url;
    browserPage.title = page.title;
    return browserPage;
  } finally {
    if (!keepCdp) {
      cdp.close();
    }
  }
}

async function resizeBrowserPage(config, browserPage, event, timeoutMs) {
  if (!browserPage?.debugger_url) {
    throw new Error("browser page debugger URL is unavailable");
  }
  validateLaunchViewport({ viewport: event?.viewport });
  browserPage.runtimeFetchProxyUrl = config.runtimeFetchProxyUrl;
  return withBrowserCdp(browserPage, timeoutMs, async (cdp) => {
    await cdp.request("Page.enable");
    await cdp.request("Runtime.enable");
    await ensureBrowserFileChooserInterception(cdp, browserPage);
    const displaySize = await applyBrowserViewport(cdp, { viewport: event?.viewport }, config);
    return {
      url: browserPage.url || "",
      title: browserPage.title || "Selkies Browser",
      can_go_back: browserPage.can_go_back === true,
      can_go_forward: browserPage.can_go_forward === true,
      width: displaySize.width,
      height: displaySize.height,
      file_chooser: summarizeBrowserFileChooser(browserPage),
    };
  });
}

function validatePasteText(value) {
  const text = String(value ?? "");
  if (!text) {
    throw new Error("browser paste_text input requires non-empty text");
  }
  if (text.length > 65536) {
    throw new Error("browser paste_text input is too large");
  }
  return text;
}

function sanitizeBrowserUploadFileName(value) {
  const name = String(value || "Library item")
    .replace(/[\0\r\n]/g, "")
    .split(/[\\/]/)
    .pop()
    .trim()
    .slice(0, 255);
  return name || "Library item";
}

function validateBrowserFileUploadEvent(event) {
  const contentBase64 = String(event?.content_base64 || "");
  if (!contentBase64) {
    throw new Error("browser file_upload input requires Library file bytes");
  }
  if (contentBase64.length > Math.ceil((MAX_BROWSER_FILE_UPLOAD_BYTES * 4) / 3) + 8) {
    throw new Error("browser file_upload input is too large");
  }
  if (contentBase64.length % 4 !== 0 || !/^[A-Za-z0-9+/]+={0,2}$/.test(contentBase64)) {
    throw new Error("browser file_upload input is not valid base64");
  }
  const sizeBytes = Buffer.from(contentBase64, "base64").length;
  if (sizeBytes <= 0) {
    throw new Error("browser file_upload input is empty");
  }
  if (sizeBytes > MAX_BROWSER_FILE_UPLOAD_BYTES) {
    throw new Error("browser file_upload input is too large");
  }
  const mimeType = String(event?.mime_type || "application/octet-stream")
    .replace(/[\0\r\n]/g, "")
    .trim()
    .slice(0, 255) || "application/octet-stream";
  const objectUri = String(event?.object_uri || "")
    .replace(/[\0\r\n]/g, "")
    .trim()
    .slice(0, 2048);
  return {
    contentBase64,
    fileName: sanitizeBrowserUploadFileName(event?.file_name),
    mimeType,
    objectUri,
    sizeBytes,
  };
}

async function pasteTextIntoBrowserPage(browserPage, event, timeoutMs) {
  if (!browserPage?.debugger_url) {
    throw new Error("browser page debugger URL is unavailable");
  }
  const text = validatePasteText(event?.text);
  await withBrowserCdp(browserPage, timeoutMs, async (cdp) => {
    await cdp.request("Page.enable").catch(() => {});
    await cdp.request("Runtime.enable").catch(() => {});
    await ensureBrowserFileChooserInterception(cdp, browserPage);
    await cdp.request("Input.insertText", { text });
  });
  return {
    url: browserPage.url || "",
    title: browserPage.title || "Selkies Browser",
    can_go_back: browserPage.can_go_back === true,
    can_go_forward: browserPage.can_go_forward === true,
    file_chooser: summarizeBrowserFileChooser(browserPage),
  };
}

async function browserPageStateFromCdp(browserPage, cdp, timeoutMs) {
  const current = await cdp.request(
    "Runtime.evaluate",
    {
      expression: "JSON.stringify({ url: window.location.href, title: document.title })",
      returnByValue: true,
    },
    Math.min(timeoutMs, 5000),
  );
  const history = await cdp.request("Page.getNavigationHistory").catch(() => ({}));
  let page = {};
  try {
    page = JSON.parse(current?.result?.value || "{}");
  } catch {
    page = {};
  }
  const currentIndex = Number(history.currentIndex ?? -1);
  const entryCount = Array.isArray(history.entries) ? history.entries.length : 0;
  browserPage.url = typeof page.url === "string" ? page.url : browserPage.url;
  browserPage.title = typeof page.title === "string" ? page.title : browserPage.title;
  browserPage.can_go_back = currentIndex > 0;
  browserPage.can_go_forward = currentIndex >= 0 && currentIndex < entryCount - 1;
  return {
    url: browserPage.url,
    title: browserPage.title,
    can_go_back: browserPage.can_go_back,
    can_go_forward: browserPage.can_go_forward,
    file_chooser: summarizeBrowserFileChooser(browserPage),
  };
}

function cachedBrowserPageState(browserPage) {
  return {
    url: browserPage?.url || "",
    title: browserPage?.title || "Selkies Browser",
    can_go_back: browserPage?.can_go_back === true,
    can_go_forward: browserPage?.can_go_forward === true,
    file_chooser: summarizeBrowserFileChooser(browserPage),
  };
}

async function uploadFileIntoBrowserPage(browserPage, event, timeoutMs) {
  if (!browserPage?.debugger_url) {
    throw new Error("browser page debugger URL is unavailable");
  }
  const upload = validateBrowserFileUploadEvent(event);
  const chooser = browserPage.file_chooser?.pending || null;
  if (!Number.isInteger(chooser?.backend_node_id)) {
    throw new Error("Browser file upload requires an active Runtime Library file picker request.");
  }
  return withBrowserCdp(browserPage, timeoutMs, async (cdp) => {
    await cdp.request("Page.enable");
    await cdp.request("Runtime.enable");
    await ensureBrowserFileChooserInterception(cdp, browserPage);
    await cdp.request("DOM.enable");
    const uploadDir = fs.mkdtempSync(path.join(os.tmpdir(), "elastos-browser-upload-"));
    const uploadPath = path.join(uploadDir, upload.fileName || "Library item");
    fs.writeFileSync(uploadPath, Buffer.from(upload.contentBase64, "base64"), { mode: 0o600 });
    if (!Array.isArray(browserPage.upload_temp_files)) {
      browserPage.upload_temp_files = [];
    }
    browserPage.upload_temp_files.push(uploadPath);
    await cdp.request("DOM.setFileInputFiles", {
      backendNodeId: chooser.backend_node_id,
      files: [uploadPath],
    });
    const node = await cdp.request("DOM.resolveNode", {
      backendNodeId: chooser.backend_node_id,
    });
    const objectId = node?.object?.objectId;
    if (!objectId) {
      throw new Error("Browser file picker target is no longer available.");
    }
    const result = await cdp.request("Runtime.callFunctionOn", {
      objectId,
      awaitPromise: true,
      returnByValue: true,
      functionDeclaration: `function() {
        if (!(this instanceof HTMLInputElement) || this.type !== "file") {
          throw new Error("Browser file picker target is not a file input.");
        }
        this.dispatchEvent(new Event("input", { bubbles: true }));
        this.dispatchEvent(new Event("change", { bubbles: true }));
        const file = this.files && this.files[0];
        if (!file) {
          throw new Error("Browser file picker target has no selected file.");
        }
        return { ok: true, file_name: file.name, size: file.size, type: file.type };
      }`,
    });
    if (result?.exceptionDetails) {
      throw new Error(
        result.exceptionDetails.text || "Browser file picker injection failed.",
      );
    }
    const uploadResult = result?.result?.value || {};
    if (uploadResult.ok !== true) {
      throw new Error("Browser file picker did not accept the Library item.");
    }
    browserPage.file_chooser.pending = null;
    const state = await browserPageStateFromCdp(browserPage, cdp, timeoutMs);
    return {
      ...state,
      file_upload: {
        schema: "elastos.browser.file-upload-result/v1",
        file_name: uploadResult.file_name || upload.fileName,
        mime_type: uploadResult.type || upload.mimeType,
        size_bytes: Number(uploadResult.size || upload.sizeBytes),
        object_uri: upload.objectUri || null,
      },
    };
  });
}

async function refreshBrowserPageState(browserPage, timeoutMs) {
  if (!browserPage?.debugger_url) {
    return cachedBrowserPageState(browserPage);
  }
  return withBrowserCdp(browserPage, timeoutMs, async (cdp) => {
    await cdp.request("Page.enable");
    await cdp.request("Runtime.enable");
    await ensureBrowserFileChooserInterception(cdp, browserPage);
    return browserPageStateFromCdp(browserPage, cdp, timeoutMs);
  });
}

async function collectBrowserDiagnostics(browserPage, timeoutMs) {
  return withBrowserCdp(browserPage, timeoutMs, async (cdp) => {
    await cdp.request("Page.enable");
    await cdp.request("Runtime.enable");
    await cdp.request("Log.enable").catch(() => {});
    await ensureBrowserFileChooserInterception(cdp, browserPage);
    const expression = `(() => {
      const trim = (value, limit = 2000) => {
        const text = String(value || "");
        return text.length > limit ? text.slice(0, limit) : text;
      };
      const isVisible = (element) => {
        if (!element) return false;
        const rect = element.getBoundingClientRect();
        const style = window.getComputedStyle(element);
        return rect.width > 0 && rect.height > 0 && style.visibility !== "hidden" && style.display !== "none";
      };
      const summarizeElement = (element, limit = 240) => {
        const rect = element.getBoundingClientRect();
        const tag = String(element.tagName || "").toLowerCase();
        const role = element.getAttribute("role") || "";
        return {
          tag,
          role,
          text: trim(element.innerText || element.textContent || element.getAttribute("placeholder") || "", limit),
          aria_label: trim(element.getAttribute("aria-label") || "", limit),
          title: trim(element.getAttribute("title") || "", limit),
          test_id: trim(element.getAttribute("data-testid") || "", 120),
          visible: isVisible(element),
          rect: {
            x: Math.round(rect.x),
            y: Math.round(rect.y),
            width: Math.round(rect.width),
            height: Math.round(rect.height),
          },
        };
      };
      const summarizeResource = (entry) => ({
          name: entry.name,
          initiator_type: entry.initiatorType || "",
          duration_ms: Math.round(entry.duration || 0),
          transfer_size: Math.round(entry.transferSize || 0),
          decoded_body_size: Math.round(entry.decodedBodySize || 0),
        });
      const summarizeMediaElement = (element) => {
        const rect = element.getBoundingClientRect();
        const style = window.getComputedStyle(element);
        const buffered = [];
        try {
          for (let index = 0; index < element.buffered.length; index += 1) {
            buffered.push({
              start: Number(element.buffered.start(index).toFixed(3)),
              end: Number(element.buffered.end(index).toFixed(3)),
            });
          }
        } catch {}
        return {
          tag: String(element.tagName || "").toLowerCase(),
          src: trim(element.currentSrc || element.src || "", 1000),
          paused: element.paused === true,
          muted: element.muted === true,
          volume: Number(element.volume || 0),
          current_time: Number((element.currentTime || 0).toFixed(3)),
          duration: Number.isFinite(element.duration) ? Number(element.duration.toFixed(3)) : null,
          ready_state: Number(element.readyState || 0),
          network_state: Number(element.networkState || 0),
          ended: element.ended === true,
          autoplay: element.autoplay === true,
          controls: element.controls === true,
          plays_inline: element.playsInline === true,
          visible: rect.width > 0 && rect.height > 0 && style.visibility !== "hidden" && style.display !== "none",
          rendered_width: Math.round(rect.width || 0),
          rendered_height: Math.round(rect.height || 0),
          error: element.error ? {
            code: Number(element.error.code || 0),
            message: trim(element.error.message || "", 500),
          } : null,
          buffered,
        };
      };
      const allResources = performance.getEntriesByType("resource");
      const resources = allResources
        .slice(-80)
        .map(summarizeResource);
      const ipfsResources = allResources
        .filter((entry) => entry.name.includes("ipfs.ela.city") || entry.name.includes("/ipfs/"))
        .slice(-80)
        .map(summarizeResource);
      const images = Array.from(document.images || [])
        .slice(-80)
        .map((image) => {
          const src = image.currentSrc || image.src || "";
          const rect = image.getBoundingClientRect();
          const style = window.getComputedStyle(image);
          const resource_entries = allResources
            .filter((entry) => entry.name === src)
            .slice(-5)
            .map(summarizeResource);
          return {
            src,
            complete: image.complete === true,
            natural_width: Number(image.naturalWidth || 0),
            natural_height: Number(image.naturalHeight || 0),
            rendered_width: Math.round(rect.width || 0),
            rendered_height: Math.round(rect.height || 0),
            visible: rect.width > 0 && rect.height > 0 && style.visibility !== "hidden" && style.display !== "none",
            loading: image.loading || "",
            decoding: image.decoding || "",
            object_fit: style.objectFit || "",
            alt: trim(image.alt || "", 240),
            resource_entries,
          };
        });
      const mediaElements = Array.from(document.querySelectorAll("audio, video"))
        .slice(-20)
        .map(summarizeMediaElement);
      let clickables = [];
      try {
        clickables = Array.from(document.querySelectorAll('a[href], button, [role="button"], [role="link"]')).map((element) => {
          const rect = element.getBoundingClientRect();
          const tag = String(element.tagName || "").toLowerCase();
          const href = tag === "a" ? String(element.href || "") : "";
          const centerX = rect.x + rect.width / 2;
          const centerY = rect.y + rect.height / 2;
          const topElement = document.elementFromPoint(centerX, centerY);
          const topAction = topElement?.closest?.('a[href], button, [role="button"], [role="link"]') || null;
          const topTag = String(topElement?.tagName || "").toLowerCase();
          const topActionTag = String(topAction?.tagName || "").toLowerCase();
          const topActionHref = topActionTag === "a" ? String(topAction.href || "") : "";
          return {
            "tag": tag,
            "text": trim(element.innerText || element.textContent || "", 240),
            "aria_label": trim(element.getAttribute("aria-label") || "", 240),
            "role": element.getAttribute("role") || "",
            "href": href.indexOf("http://") === 0 || href.indexOf("https://") === 0 ? href : "",
            "disabled": Boolean(element.disabled || element.getAttribute("aria-disabled") === "true"),
            "visible": isVisible(element),
            "top_element": {
              "tag": topTag,
              "text": trim(topElement?.innerText || topElement?.textContent || "", 120),
              "action_tag": topActionTag,
              "action_text": trim(topAction?.innerText || topAction?.textContent || "", 120),
              "action_href": topActionHref.indexOf("http://") === 0 || topActionHref.indexOf("https://") === 0 ? topActionHref : "",
            },
            "rect": {
              "x": Math.round(rect.x),
              "y": Math.round(rect.y),
              "width": Math.round(rect.width),
              "height": Math.round(rect.height),
            },
          };
        });
      } catch {}
      clickables = clickables.filter((item) => item.visible).slice(0, 80);
      let visibleTextSamples = [];
      let dialogElements = [];
      try {
        const textSelectors = [
          "h1",
          "h2",
          "h3",
          "h4",
          "[role='heading']",
          "a[href]",
          "button",
          "[role='button']",
          "[role='link']",
          "[aria-label]",
          "label",
        ].join(",");
        visibleTextSamples = Array.from(document.querySelectorAll(textSelectors))
          .map((element) => summarizeElement(element))
          .filter((item) => item.visible && Boolean(item.text || item.aria_label || item.title || item.test_id))
          .slice(0, 120);
        const dialogSelectors = [
          "dialog",
          "[role='dialog']",
          "[aria-modal='true']",
          ".modal",
          ".MuiDialog-root",
          ".MuiModal-root",
        ].join(",");
        dialogElements = Array.from(document.querySelectorAll(dialogSelectors))
          .map((element) => summarizeElement(element, 1000))
          .filter((item) => item.visible)
          .slice(0, 20);
      } catch {}
        const navigation = performance.getEntriesByType("navigation")[0];
        const walletDebug = (() => {
          const ethereum = globalThis.ethereum;
          let snapshot = null;
          try {
            snapshot = typeof globalThis.__elastosWalletDebugSnapshot === "function"
              ? globalThis.__elastosWalletDebugSnapshot()
              : null;
          } catch {}
          return {
            has_ethereum: Boolean(ethereum),
            is_elastos: Boolean(ethereum?.isElastOS),
            is_metamask: Boolean(ethereum?.isMetaMask),
            selected_address: typeof ethereum?.selectedAddress === "string" ? ethereum.selectedAddress : null,
            chain_id: typeof ethereum?.chainId === "string" ? ethereum.chainId : null,
            request_available: typeof ethereum?.request === "function",
            providers_count: Array.isArray(ethereum?.providers) ? ethereum.providers.length : 0,
            runtime_binding_available: typeof globalThis.__elastosBrowserWalletRuntime === "function",
            runtime_result_available: typeof globalThis.__elastosBrowserWalletRuntimeResult === "function",
            debug: snapshot,
          };
        })();
        const storageDebug = (() => {
          const keysFor = (storage) => {
            try {
              return Array.from({ length: storage?.length || 0 }, (_, index) => storage.key(index))
                .filter((key) => typeof key === "string")
                .sort();
            } catch {
              return [];
            }
          };
          const cookieNames = (() => {
            try {
              return String(document.cookie || "")
                .split(";")
                .map((entry) => entry.trim().split("=")[0])
                .filter(Boolean)
                .sort();
            } catch {
              return [];
            }
          })();
          return {
            local_storage_keys: keysFor(globalThis.localStorage),
            session_storage_keys: keysFor(globalThis.sessionStorage),
            cookie_names: cookieNames,
          };
        })();
        return JSON.stringify({
        url: window.location.href,
        title: document.title,
        ready_state: document.readyState,
        viewport_width: window.innerWidth,
        viewport_height: window.innerHeight,
        device_pixel_ratio: window.devicePixelRatio || 1,
        body_text: trim(document.body && document.body.innerText),
        body_html: trim(document.body && document.body.innerHTML),
        body_child_count: document.body ? document.body.children.length : 0,
        root_child_count: document.querySelector("#root") ? document.querySelector("#root").children.length : null,
        root_html: trim(document.querySelector("#root") && document.querySelector("#root").innerHTML),
        root_outer_html: trim(document.querySelector("#root") && document.querySelector("#root").outerHTML),
        script_srcs: Array.from(document.scripts).map((script) => script.src || "[inline]").slice(0, 80),
        clickable_count: clickables.length,
        clickable_elements: clickables,
        visible_text_sample_count: visibleTextSamples.length,
        visible_text_samples: visibleTextSamples,
          dialog_count: dialogElements.length,
          dialog_elements: dialogElements,
          wallet_bridge: walletDebug,
          storage: storageDebug,
          image_count: document.images ? document.images.length : 0,
        media_element_count: mediaElements.length,
        media_elements: mediaElements,
        visible_image_count: images.filter((image) => image.visible).length,
        broken_image_count: images.filter((image) => image.complete && (image.natural_width <= 0 || image.natural_height <= 0)).length,
        pending_image_count: images.filter((image) => !image.complete).length,
        pending_ipfs_image_count: images.filter((image) => !image.complete && image.src.includes("ipfs.ela.city")).length,
        visible_broken_image_count: images.filter((image) => image.visible && image.complete && (image.natural_width <= 0 || image.natural_height <= 0)).length,
        visible_pending_image_count: images.filter((image) => image.visible && !image.complete).length,
        visible_pending_ipfs_image_count: images.filter((image) => image.visible && !image.complete && image.src.includes("ipfs.ela.city")).length,
        images,
        resource_count: performance.getEntriesByType("resource").length,
        resources,
        ipfs_resources: ipfsResources,
        navigation: navigation ? {
          type: navigation.type,
          duration_ms: Math.round(navigation.duration || 0),
          transfer_size: Math.round(navigation.transferSize || 0),
          decoded_body_size: Math.round(navigation.decodedBodySize || 0),
          dom_content_loaded_ms: Math.round(navigation.domContentLoadedEventEnd || 0),
          load_event_ms: Math.round(navigation.loadEventEnd || 0),
        } : null,
      });
    })()`;
    const evaluated = await cdp.request(
      "Runtime.evaluate",
      { expression, returnByValue: true },
      Math.min(timeoutMs, 10000),
    );
    if (evaluated.exceptionDetails) {
      const detail =
        evaluated.exceptionDetails.exception?.description ||
        evaluated.exceptionDetails.text ||
        "unknown exception";
      throw new Error(`Browser diagnostics evaluation failed: ${detail}`);
    }
    let diagnostics = {};
    try {
      diagnostics = JSON.parse(evaluated?.result?.value || "{}");
    } catch {
      diagnostics = {};
    }
    const trimEventText = (value, limit = 1000) => {
      const text = String(value || "");
      return text.length > limit ? text.slice(0, limit) : text;
    };
    const diagnosticEvents = cdp.events
      .filter((event) => [
        "Runtime.consoleAPICalled",
        "Runtime.exceptionThrown",
        "Log.entryAdded",
        "Page.javascriptDialogOpening",
      ].includes(event.method))
      .slice(-40)
      .map((event) => {
        const params = event.params || {};
        if (event.method === "Runtime.consoleAPICalled") {
          return {
            method: event.method,
            type: params.type || "",
            args: Array.isArray(params.args)
              ? params.args.slice(0, 6).map((arg) => trimEventText(arg.value ?? arg.description ?? arg.type, 500))
              : [],
          };
        }
        if (event.method === "Runtime.exceptionThrown") {
          const details = params.exceptionDetails || {};
          return {
            method: event.method,
            text: trimEventText(details.text, 500),
            exception: trimEventText(details.exception?.description || details.exception?.value || "", 1000),
            url: details.url || "",
            line_number: details.lineNumber ?? null,
            column_number: details.columnNumber ?? null,
          };
        }
        if (event.method === "Log.entryAdded") {
          const entry = params.entry || {};
          return {
            method: event.method,
            source: entry.source || "",
            level: entry.level || "",
            text: trimEventText(entry.text, 1000),
            url: entry.url || "",
            line_number: entry.lineNumber ?? null,
          };
        }
        return {
          method: event.method,
          message: trimEventText(params.message || "", 1000),
          url: params.url || "",
        };
      });
    return {
      schema: "elastos.browser.page-diagnostics/v1",
      direct_network: false,
      file_chooser: summarizeBrowserFileChooser(browserPage),
      cdp_events: diagnosticEvents,
      vm_log_tails: readBrowserVmLogTails(),
      ...diagnostics,
    };
  });
}

async function withBrowserCdp(browserPage, timeoutMs, action) {
  if (!browserPage?.debugger_url) {
    throw new Error("browser page debugger URL is unavailable");
  }
  const prepare = (cdp) => {
    cdp.runtimeFetchProxyUrl = browserPage.runtimeFetchProxyUrl || cdp.runtimeFetchProxyUrl || null;
    return cdp;
  };
  const cdpClosed = (error) => {
    const message = error instanceof Error ? error.message : String(error);
    return (
      message.includes("WebSocket is closed") ||
      message.includes("WebSocket closed") ||
      message.includes("browser CDP WebSocket error")
    );
  };
  if (browserPage._cdp && !browserPage._cdp.closed) {
    const cachedCdp = prepare(browserPage._cdp);
    try {
      return await action(cachedCdp);
    } catch (error) {
      if (!cdpClosed(error) && !cachedCdp.closed) {
        throw error;
      }
      cachedCdp.close();
      browserPage._cdp = null;
    }
  }
  const cdp = new CdpClient(browserPage.debugger_url, timeoutMs);
  try {
    await cdp.connect(timeoutMs);
    browserPage._cdp = prepare(cdp);
    return await action(browserPage._cdp);
  } catch (error) {
    if (cdpClosed(error) || cdp.closed) {
      cdp.close();
      if (browserPage._cdp === cdp) {
        browserPage._cdp = null;
      }
    }
    throw error;
  }
}

function closeCachedBrowserCdp(browserPage) {
  if (!browserPage?._cdp) {
    return;
  }
  try {
    browserPage._cdp.close();
  } catch {}
  browserPage._cdp = null;
}

function finiteCoordinate(value, fallback = 0) {
  const number = Number(value);
  return Number.isFinite(number) ? Math.max(0, number) : fallback;
}

function keyEventDefinition(key) {
  const map = {
    Enter: { key: "Enter", code: "Enter", windowsVirtualKeyCode: 13 },
    Backspace: { key: "Backspace", code: "Backspace", windowsVirtualKeyCode: 8 },
    Delete: { key: "Delete", code: "Delete", windowsVirtualKeyCode: 46 },
    Escape: { key: "Escape", code: "Escape", windowsVirtualKeyCode: 27 },
    Tab: { key: "Tab", code: "Tab", windowsVirtualKeyCode: 9 },
    ArrowUp: { key: "ArrowUp", code: "ArrowUp", windowsVirtualKeyCode: 38 },
    ArrowDown: { key: "ArrowDown", code: "ArrowDown", windowsVirtualKeyCode: 40 },
    ArrowLeft: { key: "ArrowLeft", code: "ArrowLeft", windowsVirtualKeyCode: 37 },
    ArrowRight: { key: "ArrowRight", code: "ArrowRight", windowsVirtualKeyCode: 39 },
    Home: { key: "Home", code: "Home", windowsVirtualKeyCode: 36 },
    End: { key: "End", code: "End", windowsVirtualKeyCode: 35 },
    PageUp: { key: "PageUp", code: "PageUp", windowsVirtualKeyCode: 33 },
    PageDown: { key: "PageDown", code: "PageDown", windowsVirtualKeyCode: 34 },
    " ": { key: " ", code: "Space", windowsVirtualKeyCode: 32 },
  };
  return map[key] || null;
}

async function dispatchBrowserInputEvent(browserPage, event, timeoutMs) {
  return withBrowserCdp(browserPage, timeoutMs, async (cdp) => {
    await cdp.request("Page.enable");
    await cdp.request("Runtime.enable");
    await ensureBrowserFileChooserInterception(cdp, browserPage);
    await cdp.request("Input.setIgnoreInputEvents", { ignore: false }).catch(() => {});
    if (event?.type === "click") {
      const x = finiteCoordinate(event.x);
      const y = finiteCoordinate(event.y);
      await cdp.request("Input.dispatchMouseEvent", {
        type: "mouseMoved",
        x,
        y,
        button: "none",
        buttons: 0,
        pointerType: "mouse",
      });
      await cdp.request("Input.dispatchMouseEvent", {
        type: "mousePressed",
        x,
        y,
        button: "left",
        buttons: 1,
        clickCount: 1,
        pointerType: "mouse",
      });
      await cdp.request("Input.dispatchMouseEvent", {
        type: "mouseReleased",
        x,
        y,
        button: "left",
        buttons: 0,
        clickCount: 1,
        pointerType: "mouse",
      });
      return;
    }
    if (event?.type === "wheel") {
      await cdp.request("Input.dispatchMouseEvent", {
        type: "mouseWheel",
        x: finiteCoordinate(event.x),
        y: finiteCoordinate(event.y),
        deltaX: Number(event.delta_x || 0),
        deltaY: Number(event.delta_y || 0),
      });
      return;
    }
    if (event?.type === "key") {
      const key = String(event.key || "");
      if (key.length === 1 && key !== "\r" && key !== "\n") {
        await cdp.request("Input.insertText", { text: key });
        return;
      }
      const def = keyEventDefinition(key);
      if (!def) {
        throw new Error("unsupported browser key input");
      }
      await cdp.request("Input.dispatchKeyEvent", { type: "keyDown", ...def });
      await cdp.request("Input.dispatchKeyEvent", { type: "keyUp", ...def });
      return;
    }
    throw new Error("unsupported browser input event");
  });
}

function validateBrowserNavigationUrl(value) {
  let parsed;
  try {
    parsed = new URL(String(value || ""));
  } catch {
    throw new Error("browser navigate command requires a valid URL");
  }
  if (!["http:", "https:"].includes(parsed.protocol)) {
    throw new Error("browser navigate command only supports http and https URLs");
  }
  return parsed.href;
}

async function applyBrowserCommand(config, browserPage, event, timeoutMs) {
  if (!browserPage?.debugger_url) {
    throw new Error("browser page debugger URL is unavailable");
  }
  const command = String(event?.command || "");
  const navigationUrl = command === "navigate"
    ? validateBrowserNavigationUrl(event?.url)
    : "";
  browserPage.runtimeFetchProxyUrl = config.runtimeFetchProxyUrl;
  if (command === "navigate") {
    browserPage.navigationInProgress = true;
  }
  try {
    await withBrowserCdp(browserPage, timeoutMs, async (cdp) => {
      await cdp.request("Page.enable");
      await cdp.request("Runtime.enable");
      await ensureBrowserFileChooserInterception(cdp, browserPage);
      const initScriptId = await installWalletBridge(cdp, normalizeWalletBridge(browserPage.wallet || {}));
      if (initScriptId) {
        browserPage.init_script_id = initScriptId;
      }
      if (command === "navigate") {
        const navigation = await navigateInitialBrowserPage(
          cdp,
          config.browserControl,
          navigationUrl,
          "command",
        );
        assertBrowserNavigationSucceeded(navigation, "navigation");
      } else if (command === "reload") {
        await cdp.request("Page.reload", { ignoreCache: false });
      } else if (command === "back" || command === "forward") {
        const history = await cdp.request("Page.getNavigationHistory");
        const currentIndex = Number(history.currentIndex ?? -1);
        const entries = Array.isArray(history.entries) ? history.entries : [];
        const nextIndex = command === "back" ? currentIndex - 1 : currentIndex + 1;
        const entry = entries[nextIndex];
        if (entry?.id != null) {
          const navigation = await cdp.request("Page.navigateToHistoryEntry", { entryId: entry.id });
          assertBrowserNavigationSucceeded(navigation, `${command} navigation`);
        }
      } else {
        throw new Error("unsupported browser command");
      }
      await cdp.waitForEvent("Page.domContentEventFired", Math.min(timeoutMs, 15000), "domcontent").catch(() => {});
    });
    const state = await refreshBrowserPageState(browserPage, timeoutMs);
    if (command === "navigate") {
      assertBrowserStateDidNotLandOnErrorPage(state, "navigation");
    }
    return state;
  } catch (error) {
    if (command !== "navigate" || !isRecoverableCommandNavigationFailure(error)) {
      throw error;
    }
    logControlEvent("browser_page_command_navigation_retarget", {
      page_id: browserPage.page_id || null,
      url: navigationUrl,
      reason: error instanceof Error ? error.message : String(error),
    });
    const state = await replaceBrowserPageTarget(config, browserPage, navigationUrl, timeoutMs);
    assertBrowserStateDidNotLandOnErrorPage(state, "navigation");
    return state;
  } finally {
    if (command === "navigate") {
      browserPage.navigationInProgress = false;
    }
  }
}

function isRecoverableCommandNavigationFailure(error) {
  const message = error instanceof Error ? error.message : String(error);
  return isRetryableBrowserNavigationException(message) || message.includes("chrome-error://");
}

async function replaceBrowserPageTarget(config, browserPage, url, timeoutMs) {
  const oldTargetId = browserPage.target_id || "";
  const oldCdp = browserPage._cdp || null;
  const oldStopWalletBridgeWatch = browserPage._stopWalletBridgeWatch || null;
  const launch = browserPage.launchViewport ? { viewport: browserPage.launchViewport } : {};
  const replacement = await openBrowserPage(
    config,
    url,
    normalizeWalletBridge(browserPage.wallet || {}),
    launch,
    { forceNewTarget: true },
  );
  if (typeof oldStopWalletBridgeWatch === "function") {
    oldStopWalletBridgeWatch();
  }
  if (oldCdp) {
    oldCdp.close();
  }
  browserPage.target_id = replacement.target_id;
  browserPage.debugger_url = replacement.debugger_url;
  browserPage.init_script_id = replacement.init_script_id;
  browserPage.wallet = replacement.wallet;
  browserPage.url = replacement.url;
  browserPage.title = replacement.title;
  browserPage.file_chooser = replacement.file_chooser;
  browserPage.runtimeFetchProxyUrl = replacement.runtimeFetchProxyUrl;
  browserPage.launchViewport = replacement.launchViewport;
  browserPage._cdp = replacement._cdp;
  browserPage._stopWalletBridgeWatch = startWalletBridgeWatch(
    browserPage,
    normalizeWalletBridge(browserPage.wallet || {}),
    timeoutMs,
  );
  if (oldTargetId && oldTargetId !== browserPage.target_id) {
    await closeBrowserTarget(config.browserControl, oldTargetId);
  }
  return await refreshBrowserPageState(browserPage, timeoutMs);
}

async function resetBrowserPageTarget(browserPage) {
  const borrowedCdp = browserPage?._cdp || null;
  const ownedCdp = !borrowedCdp && browserPage?.debugger_url
    ? new CdpClient(browserPage.debugger_url, 5000)
    : null;
  const cdp = borrowedCdp || ownedCdp;
  if (!cdp) {
    return;
  }
  try {
    if (ownedCdp) {
      await ownedCdp.connect(5000);
      await ownedCdp.request("Page.enable").catch(() => {});
    }
    await cdp.request("Page.stopLoading").catch(() => {});
    await cdp.request("Page.navigate", { url: "about:blank" }).catch(() => {});
  } finally {
    if (ownedCdp) {
      ownedCdp.close();
    }
  }
}

async function closeBrowserPage(_browserControl, browserPage) {
  cleanupBrowserUploadTempFiles(browserPage);
  if (typeof browserPage?._stopWalletBridgeWatch === "function") {
    browserPage._stopWalletBridgeWatch();
    browserPage._stopWalletBridgeWatch = null;
  }
  if (browserPage?._cdp) {
    try {
      if (browserPage.init_script_id) {
        await browserPage._cdp
          .request("Page.removeScriptToEvaluateOnNewDocument", {
            identifier: browserPage.init_script_id,
          })
          .catch(() => {});
      }
      await resetBrowserPageTarget(browserPage);
    } finally {
      browserPage._cdp.close();
      browserPage._cdp = null;
    }
    return;
  }
  await resetBrowserPageTarget(browserPage);
}

function parseSelkiesMessage(raw) {
  if (raw === "HELLO") {
    return { kind: "hello", raw };
  }
  if (raw.startsWith("SESSION_OK ")) {
    return { kind: "session_ok", raw, serverPeerId: raw.split(/\s+/)[1] };
  }
  if (raw.startsWith("ERROR")) {
    return { kind: "error", raw };
  }
  try {
    const message = JSON.parse(raw);
    return { kind: "peer_message", raw, from: null, ...message };
  } catch {
    // Session messages from current Selkies are raw JSON. Older relay-shaped
    // messages may still carry "<peer> <json>", handled below.
  }
  const separator = raw.indexOf(" ");
  if (separator < 0) {
    return { kind: "unknown", raw };
  }
  const from = raw.slice(0, separator);
  const payload = raw.slice(separator + 1);
  try {
    const message = JSON.parse(payload);
    return { kind: "peer_message", raw, from, ...message };
  } catch {
    return { kind: "unknown", raw };
  }
}

function isCurrentSelkiesHelloFailure(error) {
  const message = error instanceof Error ? error.message : String(error);
  return (
    message.includes("Selkies WebSocket closed while waiting for Selkies HELLO") ||
    message.includes("timed out waiting for Selkies HELLO") ||
    message.includes("Selkies audio WebSocket closed while waiting for Selkies audio HELLO") ||
    message.includes("timed out waiting for Selkies audio HELLO")
  );
}

function validateOpenRequest(body) {
  if (body.schema !== HOSTED_PRODUCT_OPEN_SCHEMA && body.schema !== VM_GUEST_OPEN_SCHEMA) {
    throw new Error("unsupported Selkies control open schema");
  }
  const launch = body.launch_request;
  if (!launch || launch.schema !== "elastos.browser.engine.launch-request/v1") {
    throw new Error("missing Browser Engine launch request");
  }
  if (launch.engine !== "selkies_gstreamer") {
    throw new Error("Selkies control service requires engine=selkies_gstreamer");
  }
  if (launch.display_mode !== "webrtc_remote_display") {
    throw new Error("Selkies control service requires display_mode=webrtc_remote_display");
  }
  const expectedGuarantee = body.schema === VM_GUEST_OPEN_SCHEMA ? "mechanism_microvm" : "operator_rbi";
  if (launch.guarantee_level !== expectedGuarantee) {
    throw new Error(`Selkies control service requires guarantee_level=${expectedGuarantee}`);
  }
  if (launch.network_mode !== "runtime_net_only" || launch.direct_network !== false) {
    throw new Error("Selkies control service requires runtime_net_only and direct_network=false");
  }
  if (!safeId(launch.adapter) || !safeId(launch.stream_id)) {
    throw new Error("launch request adapter and stream_id must be safe identifiers");
  }
  return launch;
}

async function main() {
  const config = readConfig();
  if (fs.existsSync(config.controlSocketPath)) {
    if (!config.replaceExistingSocket) {
      fail(`control socket already exists: ${config.controlSocketPath}`);
    }
    fs.unlinkSync(config.controlSocketPath);
  }
  const pages = new Map();
  let lastSessionClosedAt = 0;
  const markSessionClosed = () => {
    lastSessionClosedAt = Date.now();
  };
  const closeActivePages = () => {
    const pageIds = [...pages.keys()];
    for (const page of pages.values()) {
      page.close();
    }
    pages.clear();
    if (pageIds.length > 0) {
      markSessionClosed();
    }
    return pageIds;
  };
  const waitForSessionCooldown = async () => {
    const remaining = config.sessionCooldownMs - (Date.now() - lastSessionClosedAt);
    if (remaining > 0) {
      await sleep(remaining);
    }
  };
  const server = http.createServer(async (req, res) => {
    try {
      const url = new URL(req.url, "http://browser-engine");
      if (req.method === "POST" && url.pathname === "/shutdown") {
        logControlEvent("request", { method: req.method, path: url.pathname });
        closeActivePages();
        httpJson(res, 200, {
          schema: "elastos.browser.selkies-control.shutdown/v1",
          ok: true,
        });
        setTimeout(() => {
          server.close(() => {
            try {
              fs.unlinkSync(config.controlSocketPath);
            } catch {}
            process.exit(0);
          });
        }, 25);
        return;
      }
      if (req.method === "GET" && url.pathname === "/status") {
        logControlEvent("request", { method: req.method, path: url.pathname });
        httpJson(res, 200, {
          schema: "elastos.browser.selkies-control.status/v1",
          display_backend: "selkies_gstreamer_webrtc",
          backend_class: "product_compositor",
          signaling_protocol: config.signalingProtocol,
          active_pages: pages.size,
          page_ids: [...pages.keys()],
          single_session: pages.size <= 1,
          single_vm_session: true,
          direct_network: false,
          runtime_fetch_proxy_url: runtimeFetchProxyOrigin(config.runtimeFetchProxyUrl),
        });
        return;
      }
      if (req.method === "GET" && url.pathname === "/logs") {
        logControlEvent("request", { method: req.method, path: url.pathname });
        httpJson(res, 200, {
          schema: "elastos.browser.selkies-control.logs/v1",
          logs: readBrowserVmLogTails(),
        });
        return;
      }
      if (req.method === "POST" && url.pathname === "/pages") {
        logControlEvent("request", {
          method: req.method,
          path: url.pathname,
          content_length: req.headers["content-length"] || null,
        });
        const body = await readJsonRequest(req);
        logControlEvent("request_body_read", {
          method: req.method,
          path: url.pathname,
          launch_display_mode: body?.launch_request?.display_mode || null,
          launch_url: body?.launch_request?.url || null,
        });
        const launch = validateOpenRequest(body);
        if (pages.size === 0) {
          await waitForSessionCooldown();
        }
        const forceNewTarget = pages.size > 0 || lastSessionClosedAt > 0;
        const page = new SelkiesPage(config, launch, (closedPage) => {
          if (pages.get(closedPage.pageId) === closedPage) {
            pages.delete(closedPage.pageId);
            markSessionClosed();
          }
        }, { forceNewTarget });
        try {
          logControlEvent("open_wait_stack_start", { page_id: page.pageId });
          await waitForBrowserStack(config);
          logControlEvent("open_wait_stack_done", { page_id: page.pageId });
          logControlEvent("page_open_start", {
            page_id: page.pageId,
            display_mode: launch.display_mode,
            url: launch.url,
            force_new_target: forceNewTarget,
          });
          const result = await withTimeout(
            "Browser VM page open",
            config.pageOpenTimeoutMs,
            page.open(),
          );
          logControlEvent("page_open_done", {
            page_id: page.pageId,
            actual_url: result.actual_url || null,
            display_mode: result.display_session?.mode || null,
          });
          pages.set(page.pageId, page);
          httpJson(res, 200, result);
        } catch (error) {
          logControlEvent("page_open_error", {
            page_id: page.pageId,
            message: error instanceof Error ? error.message : String(error),
            stack: error instanceof Error ? error.stack || null : null,
          });
          page.close();
          markSessionClosed();
          httpJson(res, 503, {
            schema: "elastos.browser.selkies-control.error/v1",
            error: error instanceof Error ? error.message : String(error),
            logs: readBrowserVmLogTails(),
          });
        }
        return;
      }
      const pageMatch = url.pathname.match(/^\/pages\/([^/]+)\/(webrtc|input|close|status|diagnostics)$/);
      if (!pageMatch) {
        httpJson(res, 404, { error: "not found" });
        return;
      }
      const pageId = decodeURIComponent(pageMatch[1]);
      const op = pageMatch[2];
      const page = pages.get(pageId);
      if (!page) {
        httpJson(res, 404, { error: "browser page not found" });
        return;
      }
      if (req.method === "GET" && op === "status") {
        const fastStatus = url.searchParams.get("fast") === "1";
        const browserState = fastStatus
          ? cachedBrowserPageState(page.browserPage)
          : await refreshBrowserPageState(
              page.browserPage,
              config.signalTimeoutMs,
            );
        httpJson(res, 200, {
          schema: "elastos.browser.page-status/v1",
          page_id: pageId,
          display_backend: "selkies_gstreamer_webrtc",
          backend_class: "product_compositor",
          display_session: publicDisplaySession(page.displaySession),
          input_protocol: "selkies_v1",
          audio: page.webrtcMedia.audio,
          video: page.webrtcMedia.video,
          direct_network: false,
          webrtc_connection_state: page.closed ? "closed" : "signaling",
          webrtc_signaling: page.signalingDebug(),
          state_source: fastStatus ? "cache" : "cdp",
          actual_url: browserState.url,
          title: browserState.title,
          can_go_back: browserState.can_go_back === true,
          can_go_forward: browserState.can_go_forward === true,
          file_chooser: browserState.file_chooser || summarizeBrowserFileChooser(page.browserPage),
          principal_id: page.launchRequest?.principal_id || null,
          runtime_fetch_proxy_url: runtimeFetchProxyOrigin(config.runtimeFetchProxyUrl),
          wallet_bridge: {
            schema: "elastos.browser.wallet-bridge/v1",
            mode: "runtime_mediated_eip1193",
            accounts: page.wallet?.accounts?.length || 0,
            default_chain_namespace: page.wallet?.default_chain_namespace || null,
            signing: "approval_required",
          },
        });
        return;
      }
      if (req.method === "GET" && op === "diagnostics") {
        httpJson(res, 200, {
          page_id: pageId,
          direct_network: false,
          ...(await collectBrowserDiagnostics(page.browserPage, config.signalTimeoutMs)),
        });
        return;
      }
      const body = await readJsonRequest(req);
      if (req.method === "POST" && op === "webrtc") {
        httpJson(res, 200, page.signal(body.signal, body.channel || "video"));
        return;
      }
      if (req.method === "POST" && op === "input") {
        if (body?.event?.type === "browser_command") {
          const state = await applyBrowserCommand(
            config,
            page.browserPage,
            body.event,
            config.signalTimeoutMs,
          );
          httpJson(res, 200, {
            schema: "elastos.browser.input-result/v1",
            page_id: pageId,
            accepted: true,
            actual_url: state.url,
            title: state.title,
            can_go_back: state.can_go_back,
            can_go_forward: state.can_go_forward,
            file_chooser: state.file_chooser || summarizeBrowserFileChooser(page.browserPage),
            direct_network: false,
          });
          return;
        }
        if (body?.event?.type === "resize") {
          const state = await resizeBrowserPage(
            config,
            page.browserPage,
            body.event,
            config.signalTimeoutMs,
          );
          httpJson(res, 200, {
            schema: "elastos.browser.input-result/v1",
            page_id: pageId,
            accepted: true,
            actual_url: state.url,
            title: state.title,
            can_go_back: state.can_go_back,
            can_go_forward: state.can_go_forward,
            width: state.width,
            height: state.height,
            file_chooser: state.file_chooser || summarizeBrowserFileChooser(page.browserPage),
            direct_network: false,
          });
          return;
        }
        if (body?.event?.type === "paste_text") {
          const state = await pasteTextIntoBrowserPage(
            page.browserPage,
            body.event,
            config.signalTimeoutMs,
          );
          httpJson(res, 200, {
            schema: "elastos.browser.input-result/v1",
            page_id: pageId,
            accepted: true,
            actual_url: state.url,
            title: state.title,
            can_go_back: state.can_go_back,
            can_go_forward: state.can_go_forward,
            file_chooser: state.file_chooser || summarizeBrowserFileChooser(page.browserPage),
            direct_network: false,
          });
          return;
        }
        if (body?.event?.type === "file_upload") {
          const state = await uploadFileIntoBrowserPage(
            page.browserPage,
            body.event,
            config.signalTimeoutMs,
          );
          httpJson(res, 200, {
            schema: "elastos.browser.input-result/v1",
            page_id: pageId,
            accepted: true,
            actual_url: state.url,
            title: state.title,
            can_go_back: state.can_go_back,
            can_go_forward: state.can_go_forward,
            file_chooser: state.file_chooser || summarizeBrowserFileChooser(page.browserPage),
            file_upload: state.file_upload,
            direct_network: false,
          });
          return;
        }
        if (["click", "wheel", "key"].includes(body?.event?.type)) {
          await dispatchBrowserInputEvent(
            page.browserPage,
            body.event,
            config.signalTimeoutMs,
          );
          const state = await refreshBrowserPageState(
            page.browserPage,
            config.signalTimeoutMs,
          );
          httpJson(res, 200, {
            schema: "elastos.browser.input-result/v1",
            page_id: pageId,
            accepted: true,
            actual_url: state.url,
            title: state.title,
            can_go_back: state.can_go_back,
            can_go_forward: state.can_go_forward,
            width: page.browserPage?.width,
            height: page.browserPage?.height,
            file_chooser: state.file_chooser || summarizeBrowserFileChooser(page.browserPage),
            direct_network: false,
          });
          return;
        }
        httpJson(res, 200, {
          schema: "elastos.browser.input-result/v1",
          page_id: pageId,
          accepted: false,
          reason: "Selkies input is carried by the WebRTC data channel",
        });
        return;
      }
      if (req.method === "POST" && op === "close") {
        page.close();
        pages.delete(pageId);
        markSessionClosed();
        httpJson(res, 200, { schema: "elastos.browser.close-result/v1", page_id: pageId, closed: true });
        return;
      }
      httpJson(res, 405, { error: "method not allowed" });
    } catch (error) {
      logControlEvent("request_error", {
        message: error instanceof Error ? error.message : String(error),
        stack: error instanceof Error ? error.stack || null : null,
      });
      httpJson(res, 500, { error: error instanceof Error ? error.message : String(error) });
    }
  });
  server.on("clientError", (error, socket) => {
    logControlEvent("client_error", {
      message: error instanceof Error ? error.message : String(error),
      code: error?.code || null,
      bytes_parsed: Number.isInteger(error?.bytesParsed) ? error.bytesParsed : null,
      raw_packet_bytes: Buffer.isBuffer(error?.rawPacket) ? error.rawPacket.length : null,
    });
    socket.destroy();
  });
  server.listen(config.controlSocketPath, () => {
    console.log(JSON.stringify({
      ok: true,
      schema: "elastos.browser.selkies-control.ready/v1",
      control_socket: config.controlSocketPath,
      selkies_ws_url: config.selkiesWsUrl.toString(),
      signaling_protocol: config.signalingProtocol,
      display_backend: "selkies_gstreamer_webrtc",
      backend_class: "product_compositor",
      audio: false,
      video: true,
      direct_network: false,
    }));
  });
}

main().catch((error) => fail(error instanceof Error ? error.message : String(error)));
