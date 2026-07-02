#!/usr/bin/env node
import crypto from "node:crypto";
import fs from "node:fs";
import http from "node:http";
import path from "node:path";
import process from "node:process";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";

const REQUEST_ENV = "ELASTOS_BROWSER_ENGINE_REQUEST";
const ROOT_ENV = "ELASTOS_BROWSER_PER_LAUNCH_ROOT";
const PROFILE_ROOT_ENV = "ELASTOS_BROWSER_PROFILE_ROOT";
const TARGET_IMAGE_ENV = "ELASTOS_BROWSER_SELKIES_TARGET_IMAGE";
const BROWSER_PROGRAM_ENV = "ELASTOS_BROWSER_SELKIES_BROWSER_PROGRAM";
const ICE_SERVER_ENV = "ELASTOS_BROWSER_SELKIES_ICE_SERVER";
const ICE_SERVERS_JSON_ENV = "ELASTOS_BROWSER_SELKIES_ICE_SERVERS_JSON";
const DEFAULT_STARTUP_TIMEOUT_MS = 90000;

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

function safePathSegment(value) {
  return String(value || "session").replace(/[^A-Za-z0-9_-]/g, "_").slice(0, 80);
}

function readAbsoluteDirectoryEnv(name) {
  const value = process.env[name];
  if (!value) {
    return "";
  }
  if (!path.isAbsolute(value) || /[\r\n\0]/.test(value)) {
    fail(`${name} must be an absolute path without control characters`);
  }
  return value;
}

function validateLaunchRequest(request) {
  if (request.schema !== "elastos.browser.engine.launch-request/v1") {
    fail("unsupported browser engine launch request schema");
  }
  if (!safeId(request.adapter) || !safeId(request.stream_id)) {
    fail("launch request adapter and stream_id must be safe identifiers");
  }
  if (request.engine !== "selkies_gstreamer") {
    fail(`per-launch Selkies supervisor expected selkies_gstreamer, got ${request.engine || "none"}`);
  }
  if (request.display_mode !== "webrtc_remote_display") {
    fail("per-launch Selkies supervisor requires webrtc_remote_display");
  }
  if (request.network_mode !== "runtime_net_only" || request.direct_network !== false) {
    fail("per-launch Selkies supervisor requires runtime_net_only and direct_network=false");
  }
  if (request.wallet_injection !== false) {
    fail("per-launch Selkies supervisor must not receive wallet injection authority");
  }
  if (typeof request.url !== "string" || !/^https?:\/\//.test(request.url)) {
    fail("launch request url must use http or https");
  }
}

function readIceServers() {
  const servers = [];
  if (process.env[ICE_SERVER_ENV]) {
    servers.push(process.env[ICE_SERVER_ENV]);
  }
  if (process.env[ICE_SERVERS_JSON_ENV]) {
    let parsed;
    try {
      parsed = JSON.parse(process.env[ICE_SERVERS_JSON_ENV]);
    } catch (error) {
      fail(`${ICE_SERVERS_JSON_ENV} is invalid JSON: ${error instanceof Error ? error.message : String(error)}`);
    }
    if (!Array.isArray(parsed)) {
      fail(`${ICE_SERVERS_JSON_ENV} must be a JSON array of ICE server URLs`);
    }
    for (const entry of parsed) {
      if (typeof entry !== "string") {
        fail(`${ICE_SERVERS_JSON_ENV} entries must be ICE server URL strings`);
      }
      servers.push(entry);
    }
  }
  const unique = [...new Set(servers.map((url) => String(url).trim()).filter(Boolean))];
  if (unique.length > 8) {
    fail("per-launch Selkies supervisor accepts at most 8 ICE server URLs");
  }
  for (const url of unique) {
    if (!/^(stun|turns?):/i.test(url) || /[\r\n\0]/.test(url) || url.length > 512) {
      fail("ICE server URLs must use stun:, turn:, or turns: without control characters");
    }
  }
  return unique;
}

function readBrowserProgram() {
  const value = process.env[BROWSER_PROGRAM_ENV];
  if (!value) {
    return "";
  }
  if (!value.startsWith("/") || /[\r\n\0]/.test(value)) {
    fail(`${BROWSER_PROGRAM_ENV} must be an absolute path without control characters`);
  }
  if (!fs.existsSync(value)) {
    fail(`${BROWSER_PROGRAM_ENV} does not exist: ${value}`);
  }
  try {
    fs.accessSync(value, fs.constants.X_OK);
  } catch {
    fail(`${BROWSER_PROGRAM_ENV} is not executable: ${value}`);
  }
  return value;
}

function defaultProfileRoot({ repoRoot, scriptPath }) {
  const configured = readAbsoluteDirectoryEnv(PROFILE_ROOT_ENV);
  if (configured) {
    return configured;
  }
  if (path.basename(path.dirname(scriptPath)) === "bin") {
    return path.join(repoRoot, "browser-profiles");
  }
  if (process.env.XDG_DATA_HOME) {
    return path.join(process.env.XDG_DATA_HOME, "elastos", "browser-profiles");
  }
  if (process.env.HOME) {
    return path.join(process.env.HOME, ".local", "share", "elastos", "browser-profiles");
  }
  return "/tmp/elastos-browser-profiles";
}

function profileDirForRequest(profileRoot, request) {
  const profileSubject = typeof request.principal_id === "string" && request.principal_id.trim()
    ? request.principal_id.trim()
    : request.stream_id;
  const digest = crypto.createHash("sha256").update(profileSubject).digest("hex");
  const profileDir = path.join(profileRoot, `profile-${digest}`);
  fs.mkdirSync(profileDir, { recursive: true, mode: 0o700 });
  return profileDir;
}

function tailText(file, maxBytes = 12000) {
  try {
    const stats = fs.statSync(file);
    const start = Math.max(0, stats.size - maxBytes);
    const fd = fs.openSync(file, "r");
    try {
      const buffer = Buffer.alloc(stats.size - start);
      fs.readSync(fd, buffer, 0, buffer.length, start);
      return buffer.toString("utf8").trim();
    } finally {
      fs.closeSync(fd);
    }
  } catch {
    return "";
  }
}

function readinessDiagnostics(outDir) {
  const files = [
    "target.stderr.log",
    "target.stdout.log",
    "local-exit.err",
    "local-exit.out",
    "selkies-control.log",
  ];
  const parts = [`Browser target session: ${outDir}`];
  try {
    parts.push(`Session files: ${fs.readdirSync(outDir).sort().join(", ")}`);
  } catch {}
  for (const file of files) {
    const text = tailText(path.join(outDir, file));
    if (text) {
      parts.push(`--- ${file} ---\n${text}`);
    }
  }
  return parts.join("\n");
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function controlSocketReady(controlSocket, timeoutMs = 750) {
  return new Promise((resolve) => {
    const request = http.request(
      {
        socketPath: controlSocket,
        path: "/status",
        method: "GET",
        timeout: timeoutMs,
      },
      (response) => {
        response.resume();
        response.on("end", () => resolve((response.statusCode || 500) >= 200 && (response.statusCode || 500) < 300));
      },
    );
    request.on("timeout", () => {
      request.destroy();
      resolve(false);
    });
    request.on("error", () => resolve(false));
    request.end();
  });
}

async function waitForReady({ outDir, controlSocket, child, timeoutMs }) {
  const configPath = path.join(outDir, "browser-engine-adapter.json");
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (child.exitCode != null || child.signalCode != null) {
      throw new Error(`per-launch Selkies target exited before readiness; see ${outDir}/target.stderr.log`);
    }
    if (fs.existsSync(configPath) && fs.existsSync(controlSocket) && await controlSocketReady(controlSocket)) {
      return { configPath };
    }
    await sleep(250);
  }
  throw new Error(`per-launch Selkies target did not become ready within ${timeoutMs}ms; see ${outDir}/target.stderr.log`);
}

function runHostedSupervisor({ repoRoot, request, controlSocket, timeoutMs }) {
  const supervisorPath = path.join(repoRoot, "scripts/browser-hosted-product-supervisor.mjs");
  return new Promise((resolve, reject) => {
    const child = spawn(supervisorPath, {
      cwd: repoRoot,
      env: {
        ...process.env,
        ELASTOS_BROWSER_ENGINE_REQUEST: JSON.stringify(request),
        ELASTOS_BROWSER_HOSTED_PRODUCT_CONTROL_SOCKET: controlSocket,
        ELASTOS_BROWSER_PRODUCT_DISPLAY_BACKEND: "selkies_gstreamer_webrtc",
        ELASTOS_BROWSER_HOSTED_PRODUCT_TIMEOUT_MS: String(timeoutMs),
      },
      stdio: ["ignore", "pipe", "pipe"],
    });
    let stdout = "";
    let stderr = "";
    const timer = setTimeout(() => {
      child.kill("SIGTERM");
      reject(new Error("hosted product supervisor timed out"));
    }, timeoutMs);
    child.stdout.on("data", (chunk) => {
      stdout += chunk.toString("utf8");
    });
    child.stderr.on("data", (chunk) => {
      stderr += chunk.toString("utf8");
    });
    child.on("error", (error) => {
      clearTimeout(timer);
      reject(error);
    });
    child.on("exit", (code, signal) => {
      clearTimeout(timer);
      if (code !== 0) {
        reject(new Error(stderr.trim() || `hosted product supervisor exited with ${code ?? signal}`));
        return;
      }
      try {
        resolve(JSON.parse(stdout.trim()));
      } catch (error) {
        reject(new Error(`hosted product supervisor returned invalid JSON: ${error instanceof Error ? error.message : String(error)}`));
      }
    });
  });
}

function killProcessGroup(child) {
  if (!child?.pid) {
    return;
  }
  try {
    process.kill(-child.pid, "SIGTERM");
  } catch {
    try {
      child.kill("SIGTERM");
    } catch {}
  }
}

async function main() {
  const request = parseJsonEnv(REQUEST_ENV);
  validateLaunchRequest(request);

  const scriptPath = fileURLToPath(import.meta.url);
  const repoRoot = path.resolve(path.dirname(scriptPath), "..");
  const root = process.env[ROOT_ENV] || "/tmp/elastos-browser-sessions";
  const sessionName = `${safePathSegment(request.stream_id)}-${crypto.randomBytes(6).toString("hex")}`;
  const outDir = path.join(root, sessionName);
  fs.mkdirSync(outDir, { recursive: true });
  const profileDir = profileDirForRequest(defaultProfileRoot({ repoRoot, scriptPath }), request);

  const stdoutFd = fs.openSync(path.join(outDir, "target.stdout.log"), "a");
  const stderrFd = fs.openSync(path.join(outDir, "target.stderr.log"), "a");
  const targetScript = path.join(repoRoot, "scripts/browser-selkies-runtime-exit-target.sh");
  const targetArgs = [
    "--out-dir",
    outDir,
    "--adapter-id",
    request.adapter,
    "--selkies-width",
    process.env.ELASTOS_BROWSER_SELKIES_WIDTH || "1920",
    "--selkies-height",
    process.env.ELASTOS_BROWSER_SELKIES_HEIGHT || "1080",
    "--selkies-framerate",
    process.env.ELASTOS_BROWSER_SELKIES_FRAMERATE || "30",
    "--selkies-video-bitrate",
    process.env.ELASTOS_BROWSER_SELKIES_VIDEO_BITRATE || "16",
    "--selkies-h264-crf",
    process.env.ELASTOS_BROWSER_SELKIES_H264_CRF || "23",
    "--selkies-resolution-mode",
    process.env.ELASTOS_BROWSER_SELKIES_RESOLUTION_MODE || "dynamic",
    "--timeout-seconds",
    process.env.ELASTOS_BROWSER_SELKIES_TIMEOUT_SECONDS || "300",
    "--profile-dir",
    profileDir,
  ];
  if (process.env[TARGET_IMAGE_ENV]) {
    targetArgs.push("--target-image", process.env[TARGET_IMAGE_ENV]);
  }
  const browserProgram = readBrowserProgram();
  if (browserProgram) {
    targetArgs.push("--browser-program", browserProgram);
  }
  for (const iceServer of readIceServers()) {
    targetArgs.push("--ice-server", iceServer);
  }

  const target = spawn(targetScript, targetArgs, {
    cwd: repoRoot,
    detached: true,
    stdio: ["ignore", stdoutFd, stderrFd],
    env: {
      ...process.env,
      ELASTOS_BROWSER_DUMP_DIAGNOSTICS_ON_TERM: "1",
    },
  });
  target.unref();

  const controlSocket = path.join(outDir, "selkies-control.sock");
  try {
    const startupTimeoutMs = Number(process.env.ELASTOS_BROWSER_PER_LAUNCH_STARTUP_TIMEOUT_MS || String(DEFAULT_STARTUP_TIMEOUT_MS));
    await waitForReady({ outDir, controlSocket, child: target, timeoutMs: startupTimeoutMs });
    const result = await runHostedSupervisor({
      repoRoot,
      request,
      controlSocket,
      timeoutMs: Number(process.env.ELASTOS_BROWSER_HOSTED_PRODUCT_TIMEOUT_MS || "30000"),
    });
    result.control_socket_path = controlSocket;
    result.isolated_session = true;
    result.isolation = {
      schema: "elastos.browser.engine.isolation/v1",
      kind: "per_launch_selkies_target",
      session_dir: outDir,
    };
    process.stdout.write(`${JSON.stringify(result)}\n`);
  } catch (error) {
    killProcessGroup(target);
    await sleep(750);
    const diagnostics = readinessDiagnostics(outDir);
    fail(`${error instanceof Error ? error.message : String(error)}\n${diagnostics}`);
  } finally {
    fs.closeSync(stdoutFd);
    fs.closeSync(stderrFd);
  }
}

main().catch((error) => {
  fail(error instanceof Error ? error.message : String(error));
});
