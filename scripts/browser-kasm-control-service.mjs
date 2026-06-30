#!/usr/bin/env node
import fs from "node:fs";
import http from "node:http";
import process from "node:process";
import { URL } from "node:url";

const CONFIG_ENV = "ELASTOS_BROWSER_KASM_CONTROL_CONFIG";

function fail(message) {
  console.error(message);
  process.exit(1);
}

function safeId(value) {
  return typeof value === "string" && /^[A-Za-z0-9:_-]+$/.test(value);
}

function numberOr(value, fallback) {
  return Number.isInteger(value) && value > 0 ? value : fallback;
}

function readConfig() {
  const raw = process.env[CONFIG_ENV];
  if (!raw) fail(`${CONFIG_ENV} is required`);
  let config;
  try {
    config = JSON.parse(raw);
  } catch (error) {
    fail(`${CONFIG_ENV} is invalid JSON: ${error.message}`);
  }
  if (config.schema !== "elastos.browser.kasm-control.config/v1") {
    fail("unsupported Kasm control config schema");
  }
  validateSocketPath(config.control_socket_path, "control_socket_path");
  const baseUrl = new URL(config.kasm_base_url || "");
  if (baseUrl.protocol !== "https:" && !(baseUrl.protocol === "http:" && ["127.0.0.1", "::1", "localhost"].includes(baseUrl.hostname))) {
    fail("kasm_base_url must use https, except loopback http for local tests");
  }
  for (const field of ["api_key", "api_key_secret", "user_id", "image_id"]) {
    if (typeof config[field] !== "string" || config[field].length === 0 || /[\r\n\0]/.test(config[field])) {
      fail(`${field} must be a non-empty string without control characters`);
    }
  }
  const displayBridgeSocket = config.product_display_bridge_socket || "";
  if (displayBridgeSocket) {
    validateSocketPath(displayBridgeSocket, "product_display_bridge_socket");
  }
  return {
    schema: config.schema,
    controlSocketPath: config.control_socket_path,
    replaceExistingSocket: config.replace_existing_socket === true,
    baseUrl,
    apiKey: config.api_key,
    apiKeySecret: config.api_key_secret,
    userId: config.user_id,
    imageId: config.image_id,
    requestArgs: objectOr(config.request_kasm_args, {}),
    displayBridgeSocket,
    displayBackend: config.display_backend || "kasm_workspaces_webrtc",
    requestTimeoutMs: numberOr(config.request_timeout_ms, 15_000),
    readyTimeoutMs: numberOr(config.ready_timeout_ms, 60_000),
    pollIntervalMs: numberOr(config.poll_interval_ms, 1_000),
  };
}

function objectOr(value, fallback) {
  return value && typeof value === "object" && !Array.isArray(value) ? value : fallback;
}

function validateSocketPath(path, label) {
  if (typeof path !== "string" || !path.startsWith("/") || /[\s\0]/.test(path)) {
    fail(`${label} must be an absolute Unix socket path without whitespace`);
  }
}

function validateOpenRequest(body) {
  if (body.schema !== "elastos.browser.hosted-product.open/v1") {
    throw new Error("unsupported hosted product open schema");
  }
  const launch = body.launch_request;
  if (!launch || launch.schema !== "elastos.browser.engine.launch-request/v1") {
    throw new Error("missing Browser Engine launch request");
  }
  if (launch.engine !== "hosted_remote_browser") {
    throw new Error("Kasm control service requires engine=hosted_remote_browser");
  }
  if (launch.display_mode !== "webrtc_remote_display") {
    throw new Error("Kasm control service requires webrtc_remote_display");
  }
  if (launch.guarantee_level !== "operator_rbi") {
    throw new Error("Kasm control service requires guarantee_level=operator_rbi");
  }
  if (launch.network_mode !== "runtime_net_only" || launch.direct_network !== false) {
    throw new Error("Kasm control service requires runtime_net_only and direct_network=false");
  }
  if (launch.wallet_injection !== false) {
    throw new Error("Kasm control service must not receive wallet injection authority");
  }
  if (!safeId(launch.adapter) || !safeId(launch.stream_id)) {
    throw new Error("launch request adapter and stream_id must be safe identifiers");
  }
  const target = new URL(String(launch.url || ""));
  if (!["http:", "https:"].includes(target.protocol)) {
    throw new Error("launch request url must use http or https");
  }
  return launch;
}

function httpJson(res, status, body) {
  const data = Buffer.from(JSON.stringify(body));
  res.writeHead(status, {
    "content-type": "application/json",
    "content-length": data.length,
  });
  res.end(data);
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

function kasmPayload(config, extra) {
  return {
    api_key: config.apiKey,
    api_key_secret: config.apiKeySecret,
    ...extra,
  };
}

async function postKasm(config, path, body) {
  const url = new URL(path, config.baseUrl);
  const response = await fetch(url, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body),
    signal: AbortSignal.timeout(config.requestTimeoutMs),
  });
  const text = await response.text();
  let parsed = {};
  try {
    parsed = text ? JSON.parse(text) : {};
  } catch (error) {
    throw new Error(`Kasm ${path} response was not JSON: ${error.message}`);
  }
  if (!response.ok) {
    throw new Error(parsed.error_message || parsed.error || parsed.message || `Kasm ${path} returned HTTP ${response.status}`);
  }
  return parsed;
}

function extractKasmId(response) {
  const candidates = [
    response.kasm_id,
    response.kasm?.kasm_id,
    response.kasm?.id,
    response.session?.kasm_id,
    response.session?.id,
  ];
  const id = candidates.find((value) => typeof value === "string" && value.length > 0);
  if (!id) {
    throw new Error("Kasm request did not return kasm_id");
  }
  return id;
}

function extractKasmSession(status, fallbackId) {
  const kasm = status.kasm || status.session || status;
  const operationalStatus = String(kasm.operational_status || status.operational_status || "");
  const kasmUrl = kasm.kasm_url || status.kasm_url || "";
  return {
    kasm_id: String(kasm.kasm_id || kasm.id || fallbackId),
    operational_status: operationalStatus,
    kasm_url: typeof kasmUrl === "string" ? kasmUrl : "",
  };
}

async function requestKasmSession(config, launch) {
  const request = await postKasm(
    config,
    "/api/public/request_kasm",
    kasmPayload(config, {
      user_id: config.userId,
      image_id: config.imageId,
      kasm_url: launch.url,
      allow_kasm_audio: true,
      kasm_audio_default_on: true,
      ...config.requestArgs,
    }),
  );
  const kasmId = extractKasmId(request);
  const deadline = Date.now() + config.readyTimeoutMs;
  let lastSession = { kasm_id: kasmId, operational_status: "", kasm_url: "" };
  while (Date.now() <= deadline) {
    const status = await postKasm(
      config,
      "/api/public/get_kasm_status",
      kasmPayload(config, {
        user_id: config.userId,
        kasm_id: kasmId,
      }),
    );
    lastSession = extractKasmSession(status, kasmId);
    if (lastSession.operational_status === "running") {
      return lastSession;
    }
    await new Promise((resolve) => setTimeout(resolve, config.pollIntervalMs));
  }
  throw new Error(`Kasm session did not reach running state: ${lastSession.operational_status || "unknown"}`);
}

async function deleteKasmSession(config, kasmId) {
  if (!kasmId) return;
  await postKasm(
    config,
    "/api/public/delete_kasm",
    kasmPayload(config, {
      user_id: config.userId,
      kasm_id: kasmId,
    }),
  ).catch(() => {});
}

function postJsonOverUnix(socketPath, path, body, timeoutMs) {
  const bytes = Buffer.from(JSON.stringify(body));
  return new Promise((resolve, reject) => {
    const request = http.request(
      {
        socketPath,
        path,
        method: "POST",
        timeout: timeoutMs,
        headers: {
          "content-type": "application/json",
          "content-length": bytes.length,
        },
      },
      (response) => {
        const chunks = [];
        response.on("data", (chunk) => chunks.push(chunk));
        response.on("end", () => {
          const raw = Buffer.concat(chunks).toString("utf8");
          let parsed = {};
          try {
            parsed = raw ? JSON.parse(raw) : {};
          } catch (error) {
            reject(new Error(`display bridge response is not JSON: ${error.message}`));
            return;
          }
          if ((response.statusCode || 500) < 200 || (response.statusCode || 500) >= 300) {
            reject(new Error(parsed.error || parsed.message || `display bridge returned HTTP ${response.statusCode}`));
            return;
          }
          resolve(parsed);
        });
      },
    );
    request.on("timeout", () => request.destroy(new Error("display bridge request timed out")));
    request.on("error", reject);
    request.end(bytes);
  });
}

function validateDisplayBridgeResult(result, launch, config) {
  if (result.schema !== "elastos.browser.engine.supervisor-result/v1") {
    throw new Error("Kasm display bridge did not return elastos.browser.engine.supervisor-result/v1");
  }
  if ("kasm_url" in result || "session_url" in result) {
    throw new Error("Kasm display bridge must not leak raw Kasm session URLs");
  }
  if (!safeId(result.page_id)) {
    throw new Error("Kasm display bridge returned an unsafe page_id");
  }
  if (result.adapter !== launch.adapter || result.engine !== launch.engine || result.stream_id !== launch.stream_id) {
    throw new Error("Kasm display bridge returned a mismatched adapter, engine, or stream_id");
  }
  if (result.network_mode !== "runtime_net_only" || result.direct_network !== false || result.wallet_injection !== false) {
    throw new Error("Kasm display bridge must report runtime_net_only, direct_network=false, and wallet_injection=false");
  }
  const session = result.display_session || {};
  if (session.schema !== "elastos.browser.display-session/v1" || session.mode !== "webrtc_remote_display") {
    throw new Error("Kasm display bridge returned an invalid WebRTC display session");
  }
  if (session.backend_class !== "product_compositor" || session.display_backend !== config.displayBackend) {
    throw new Error(`Kasm display bridge must return product_compositor ${config.displayBackend}`);
  }
  if (session.audio !== true || session.video !== true || session.direct_network !== false) {
    throw new Error("Kasm display bridge must prove audio=true, video=true, and direct_network=false");
  }
  if (
    !Number.isInteger(session.width) ||
    !Number.isInteger(session.height) ||
    session.width < 320 ||
    session.width > 3840 ||
    session.height < 240 ||
    session.height > 2160
  ) {
    throw new Error("Kasm display bridge must expose a valid display coordinate size");
  }
  if (typeof session.signaling_url !== "string" || !session.signaling_url.startsWith("/api/apps/browser/pages/")) {
    throw new Error("Kasm display bridge must expose Runtime-scoped signaling_url");
  }
  return result;
}

async function proxyToBridge(config, page, req, res, op) {
  const method = req.method;
  const body = method === "GET" ? null : await readJsonRequest(req);
  const path = `/pages/${encodeURIComponent(page.bridge_page_id)}/${op}`;
  const response = await requestOverUnix(config.displayBridgeSocket, path, method, body, config.requestTimeoutMs);
  httpJson(res, response.status, response.body);
}

function requestOverUnix(socketPath, path, method, body, timeoutMs) {
  const bytes = body == null ? null : Buffer.from(JSON.stringify(body));
  return new Promise((resolve, reject) => {
    const request = http.request(
      {
        socketPath,
        path,
        method,
        timeout: timeoutMs,
        headers: bytes
          ? {
              "content-type": "application/json",
              "content-length": bytes.length,
            }
          : {},
      },
      (response) => {
        const chunks = [];
        response.on("data", (chunk) => chunks.push(chunk));
        response.on("end", () => {
          const raw = Buffer.concat(chunks).toString("utf8");
          let parsed = {};
          try {
            parsed = raw ? JSON.parse(raw) : {};
          } catch (error) {
            reject(new Error(`display bridge response is not JSON: ${error.message}`));
            return;
          }
          resolve({ status: response.statusCode || 500, body: parsed });
        });
      },
    );
    request.on("timeout", () => request.destroy(new Error("display bridge request timed out")));
    request.on("error", reject);
    if (bytes) request.write(bytes);
    request.end();
  });
}

async function main() {
  const config = readConfig();
  if (fs.existsSync(config.controlSocketPath)) {
    if (!config.replaceExistingSocket) fail(`control socket already exists: ${config.controlSocketPath}`);
    fs.unlinkSync(config.controlSocketPath);
  }
  const pages = new Map();
  const server = http.createServer(async (req, res) => {
    try {
      const url = new URL(req.url, "http://browser-engine");
      if (req.method === "GET" && url.pathname === "/status") {
        httpJson(res, 200, {
          schema: "elastos.browser.kasm-control.status/v1",
          display_backend: config.displayBackend,
          backend_class: "product_compositor",
          active_pages: pages.size,
          page_ids: [...pages.keys()],
          product_display_bridge_available: Boolean(config.displayBridgeSocket && fs.existsSync(config.displayBridgeSocket)),
          direct_network: false,
        });
        return;
      }
      if (req.method === "POST" && url.pathname === "/pages") {
        const body = await readJsonRequest(req);
        const launch = validateOpenRequest(body);
        if (!config.displayBridgeSocket) {
          httpJson(res, 501, {
            error: "kasm_product_display_bridge_required",
            message: "Kasm URL-only sessions are not an ElastOS Browser display adapter; configure product_display_bridge_socket.",
          });
          return;
        }
        if (!fs.existsSync(config.displayBridgeSocket)) {
          httpJson(res, 503, {
            error: "kasm_product_display_bridge_unavailable",
            message: "configured Kasm product display bridge socket is unavailable",
          });
          return;
        }
        const kasmSession = await requestKasmSession(config, launch);
        try {
          const result = validateDisplayBridgeResult(
            await postJsonOverUnix(
              config.displayBridgeSocket,
              "/pages",
              {
                schema: "elastos.browser.kasm-display-bridge.open/v1",
                launch_request: launch,
                kasm_session: kasmSession,
                requirements: body.requirements,
              },
              config.requestTimeoutMs,
            ),
            launch,
            config,
          );
          pages.set(result.page_id, {
            kasm_id: kasmSession.kasm_id,
            bridge_page_id: result.page_id,
          });
          httpJson(res, 200, result);
        } catch (error) {
          await deleteKasmSession(config, kasmSession.kasm_id);
          throw error;
        }
        return;
      }
      const pageMatch = url.pathname.match(/^\/pages\/([^/]+)\/(webrtc|input|close|status)$/);
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
      if (op === "close" && req.method === "POST") {
        await proxyToBridge(config, page, req, res, op).catch((error) => {
          httpJson(res, 500, { error: error instanceof Error ? error.message : String(error) });
        });
        pages.delete(pageId);
        await deleteKasmSession(config, page.kasm_id);
        return;
      }
      await proxyToBridge(config, page, req, res, op);
    } catch (error) {
      httpJson(res, 500, { error: error instanceof Error ? error.message : String(error) });
    }
  });
  server.listen(config.controlSocketPath, () => {
    console.log(JSON.stringify({
      ok: true,
      schema: "elastos.browser.kasm-control.ready/v1",
      control_socket: config.controlSocketPath,
      display_backend: config.displayBackend,
      backend_class: "product_compositor",
      product_display_bridge_configured: Boolean(config.displayBridgeSocket),
      direct_network: false,
    }));
  });
}

main().catch((error) => fail(error instanceof Error ? error.message : String(error)));
