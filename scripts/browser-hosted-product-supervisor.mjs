#!/usr/bin/env node
import http from "node:http";
import process from "node:process";

const REQUEST_ENV = "ELASTOS_BROWSER_ENGINE_REQUEST";
const CONTROL_SOCKET_ENV = "ELASTOS_BROWSER_HOSTED_PRODUCT_CONTROL_SOCKET";
const PRODUCT_ENGINE_ENV = "ELASTOS_BROWSER_PRODUCT_ENGINE";
const PRODUCT_DISPLAY_BACKEND_ENV = "ELASTOS_BROWSER_PRODUCT_DISPLAY_BACKEND";

function fail(message) {
  console.error(message);
  process.exit(1);
}

function parseJsonEnv(name) {
  const raw = process.env[name];
  if (!raw) {
    fail(`${name} is required`);
  }
  try {
    return JSON.parse(raw);
  } catch (error) {
    fail(`${name} is invalid JSON: ${error instanceof Error ? error.message : String(error)}`);
  }
}

function safeId(value) {
  return typeof value === "string" && /^[A-Za-z0-9:_-]+$/.test(value);
}

function validateSocketPath(path) {
  if (typeof path !== "string" || !path.startsWith("/") || /[\s\0]/.test(path)) {
    fail(`${CONTROL_SOCKET_ENV} must be an absolute Unix socket path without whitespace`);
  }
}

function validateLaunchRequest(request) {
  if (request.schema !== "elastos.browser.engine.launch-request/v1") {
    fail("unsupported browser engine launch request schema");
  }
  if (!safeId(request.adapter) || !safeId(request.stream_id)) {
    fail("launch request adapter and stream_id must be safe identifiers");
  }
  const expectedEngine = process.env[PRODUCT_ENGINE_ENV] || "selkies_gstreamer";
  if (request.engine !== expectedEngine) {
    fail(`hosted product supervisor expected ${expectedEngine}, got ${request.engine || "none"}`);
  }
  if (request.display_mode !== "webrtc_remote_display") {
    fail("hosted product supervisor requires webrtc_remote_display");
  }
  if (request.network_mode !== "runtime_net_only" || request.direct_network !== false) {
    fail("hosted product supervisor requires runtime_net_only and direct_network=false");
  }
  if (request.wallet_injection !== false) {
    fail("hosted product supervisor must not receive wallet injection authority");
  }
  if (typeof request.url !== "string" || !/^https?:\/\//.test(request.url)) {
    fail("launch request url must use http or https");
  }
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
          let parsed;
          try {
            parsed = raw ? JSON.parse(raw) : {};
          } catch (error) {
            reject(new Error(`hosted product control response is not JSON: ${error.message}`));
            return;
          }
          if ((response.statusCode || 500) < 200 || (response.statusCode || 500) >= 300) {
            reject(new Error(parsed.error || parsed.message || `hosted product control returned ${response.statusCode}`));
            return;
          }
          resolve(parsed);
        });
      },
    );
    request.on("timeout", () => {
      request.destroy(new Error("hosted product control request timed out"));
    });
    request.on("error", reject);
    request.end(bytes);
  });
}

function validateSupervisorResult(result, request) {
  if (result.schema !== "elastos.browser.engine.supervisor-result/v1") {
    fail("hosted product control did not return elastos.browser.engine.supervisor-result/v1");
  }
  if (!safeId(result.page_id)) {
    fail("hosted product control returned an unsafe page_id");
  }
  if (result.adapter !== request.adapter || result.engine !== request.engine || result.stream_id !== request.stream_id) {
    fail("hosted product control returned a mismatched adapter, engine, or stream_id");
  }
  if (result.network_mode !== "runtime_net_only" || result.direct_network !== false) {
    fail("hosted product control must report runtime_net_only and direct_network=false");
  }
  if (result.wallet_injection !== false) {
    fail("hosted product control must not report wallet injection authority");
  }
  const session = result.display_session || {};
  if (session.schema !== "elastos.browser.display-session/v1") {
    fail("hosted product control returned an invalid display session schema");
  }
  if (session.mode !== "webrtc_remote_display") {
    fail("hosted product control must return webrtc_remote_display");
  }
  if (session.network_mode !== "runtime_net_only" || session.direct_network !== false) {
    fail("hosted product display session must report runtime_net_only and direct_network=false");
  }
  if (session.backend_class !== "product_compositor") {
    fail("hosted product display session must be product_compositor");
  }
  if (session.display_backend === "cdp_screencast_i420" || !session.display_backend) {
    fail("hosted product display session must not use the CDP screencast proof backend");
  }
  const expectedDisplayBackend = process.env[PRODUCT_DISPLAY_BACKEND_ENV];
  if (expectedDisplayBackend && session.display_backend !== expectedDisplayBackend) {
    fail(`hosted product display session expected ${expectedDisplayBackend}, got ${session.display_backend || "none"}`);
  }
  if (session.audio !== true || session.video !== true) {
    fail("hosted product display session must advertise audio=true and video=true");
  }
  if (
    !Number.isInteger(session.width) ||
    !Number.isInteger(session.height) ||
    session.width < 320 ||
    session.width > 3840 ||
    session.height < 240 ||
    session.height > 2160
  ) {
    fail("hosted product display session must expose a valid display coordinate size");
  }
  if (session.offerer !== "browser" && session.offerer !== "engine") {
    fail("hosted product display sessions must use offerer=browser or offerer=engine");
  }
  if (session.offerer === "engine") {
    const initialOffer = session.initial_offer || {};
    if (initialOffer.schema !== "elastos.browser.webrtc-offer/v1" || initialOffer.type !== "offer" || typeof initialOffer.sdp !== "string") {
      fail("engine-offer hosted product display sessions must include an initial WebRTC offer");
    }
    if (!initialOffer.sdp.includes("m=video") || !initialOffer.sdp.includes("m=audio")) {
      fail("hosted product initial offer must include video and audio media sections");
    }
  }
  if (typeof session.signaling_url !== "string" || !session.signaling_url.startsWith("/api/apps/browser/pages/")) {
    fail("hosted product display session must expose a Runtime-scoped signaling_url");
  }
}

async function main() {
  const request = parseJsonEnv(REQUEST_ENV);
  const controlSocket = process.env[CONTROL_SOCKET_ENV];
  validateSocketPath(controlSocket);
  validateLaunchRequest(request);

  const result = await postJsonOverUnix(
    controlSocket,
    "/pages",
    {
      schema: "elastos.browser.hosted-product.open/v1",
      launch_request: request,
      requirements: {
        display_mode: "webrtc_remote_display",
        backend_class: "product_compositor",
        audio: true,
        video: true,
        network_mode: "runtime_net_only",
        direct_network: false,
      },
    },
    Number(process.env.ELASTOS_BROWSER_HOSTED_PRODUCT_TIMEOUT_MS || "30000"),
  );
  validateSupervisorResult(result, request);
  process.stdout.write(`${JSON.stringify(result)}\n`);
}

main().catch((error) => {
  fail(error instanceof Error ? error.message : String(error));
});
