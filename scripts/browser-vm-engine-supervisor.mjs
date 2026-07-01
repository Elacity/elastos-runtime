#!/usr/bin/env node
import crypto from "node:crypto";
import fs from "node:fs";
import http from "node:http";
import os from "node:os";
import path from "node:path";
import process from "node:process";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";

const REQUEST_ENV = "ELASTOS_BROWSER_ENGINE_REQUEST";
const CONTROL_SOCKET_ENV = "ELASTOS_BROWSER_VM_CONTROL_SOCKET";
const CONTROL_SERVICE_ENV = "ELASTOS_BROWSER_VM_CONTROL_SERVICE";
const CONTROL_SERVICE_CONFIG_ENV = "ELASTOS_BROWSER_VM_CONTROL_SERVICE_CONFIG";
const CONTROL_SERVICE_LOG_ENV = "ELASTOS_BROWSER_VM_CONTROL_LOG";
const CONTROL_LAUNCHER_ENV = "ELASTOS_BROWSER_VM_CONTROL_LAUNCHER";
const ROOT_ENV = "ELASTOS_BROWSER_VM_ROOT";
const DATA_DIR_ENV = "ELASTOS_BROWSER_VM_DATA_DIR";
const PLATFORM_ENV = "ELASTOS_BROWSER_VM_PLATFORM";

function fail(message) {
  console.error(message);
  process.exit(1);
}

function parseJsonEnv(name) {
  const raw = process.env[name];
  if (!raw) fail(`${name} is required`);
  try {
    return JSON.parse(raw);
  } catch (error) {
    fail(`${name} is invalid JSON: ${error instanceof Error ? error.message : String(error)}`);
  }
}

function safeId(value) {
  return typeof value === "string" && /^[A-Za-z0-9:_-]+$/.test(value);
}

function sessionSuffix(value) {
  const digest = crypto.createHash("sha256").update(String(value || "session")).digest("hex").slice(0, 16);
  return `bvm-${digest}-${crypto.randomBytes(4).toString("hex")}`;
}

function validateAbsolutePath(value, label) {
  if (typeof value !== "string" || !value.startsWith("/") || /[\r\n\0]/.test(value)) {
    fail(`${label} must be an absolute path without control characters`);
  }
}

function validateLaunchRequest(request) {
  if (request.schema !== "elastos.browser.engine.launch-request/v1") {
    fail("unsupported browser engine launch request schema");
  }
  if (!safeId(request.adapter) || !safeId(request.stream_id)) {
    fail("launch request adapter and stream_id must be safe identifiers");
  }
  if (request.engine !== "chromium_microvm") {
    fail(`Browser VM supervisor expected chromium_microvm, got ${request.engine || "none"}`);
  }
  if (request.display_mode !== "webrtc_remote_display") {
    fail("Browser VM supervisor requires webrtc_remote_display");
  }
  if (request.guarantee_level !== "mechanism_microvm") {
    fail("Browser VM supervisor requires guarantee_level=mechanism_microvm");
  }
  if (request.network_mode !== "runtime_net_only" || request.direct_network !== false) {
    fail("Browser VM supervisor requires runtime_net_only and direct_network=false");
  }
  if (request.wallet_injection !== false) {
    fail("Browser VM supervisor must not receive wallet injection authority");
  }
  if (typeof request.url !== "string" || !/^https?:\/\//.test(request.url)) {
    fail("launch request url must use http or https");
  }
  validateBrowserProfileDescriptor(request.profile);
}

function validateBrowserProfileDescriptor(profile) {
  if (!profile || typeof profile !== "object") {
    fail("Browser VM supervisor requires an explicit Browser profile descriptor");
  }
  if (
    profile.schema !== "elastos.browser.profile/v1" ||
    profile.scope !== "active_principal" ||
    profile.storage !== "principal_owned_profile_disk" ||
    profile.storage_posture !== "principal_owned_reset_scoped_unprotected" ||
    profile.protected_storage !== false ||
    profile.encrypted !== false ||
    profile.recoverable !== false ||
    profile.recovery !== "not_recovery_kit_packaged" ||
    profile.reset !== "whole_profile"
  ) {
    fail("Browser VM supervisor received an unsupported Browser profile descriptor");
  }
  if (profile.public_uri !== "localhost://Users/self/BrowserProfiles/default/profile.ext4") {
    fail("Browser VM profile public_uri must use the Users/self alias");
  }
  if (
    typeof profile.uri !== "string" ||
    !profile.uri.startsWith("localhost://Users/") ||
    !profile.uri.endsWith("/BrowserProfiles/default/profile.ext4") ||
    /[\r\n\0]/.test(profile.uri) ||
    profile.uri.includes("/../") ||
    profile.uri.endsWith("/..")
  ) {
    fail("Browser VM profile uri must be under the active principal BrowserProfiles root");
  }
  if (typeof profile.profile_key !== "string" || !/^profile-[0-9a-fA-F]{64}$/.test(profile.profile_key)) {
    fail("Browser VM profile_key must be a safe non-reversible profile id");
  }
  if (
    typeof profile.disk_path !== "string" ||
    !profile.disk_path.startsWith("/") ||
    !profile.disk_path.endsWith("/BrowserProfiles/default/profile.ext4") ||
    /[\r\n\0]/.test(profile.disk_path) ||
    profile.disk_path.includes("/../") ||
    profile.disk_path.endsWith("/..")
  ) {
    fail("Browser VM profile disk_path must be an absolute active-principal profile disk path");
  }
}

function platformId() {
  if (process.env[PLATFORM_ENV]) return process.env[PLATFORM_ENV];
  const arch = os.arch() === "x64" ? "amd64" : os.arch();
  if (process.platform === "darwin") return `darwin-${arch}`;
  if (process.platform === "linux") return `linux-${arch}`;
  return `${process.platform}-${arch}`;
}

function defaultDataDir(scriptPath) {
  const configured = process.env[DATA_DIR_ENV];
  if (configured) {
    validateAbsolutePath(configured, DATA_DIR_ENV);
    return configured;
  }
  if (path.basename(path.dirname(scriptPath)) === "bin") {
    return path.dirname(path.dirname(scriptPath));
  }
  if (process.platform === "darwin" && process.env.HOME) {
    return path.join(process.env.HOME, "Library/Application Support/elastos");
  }
  if (process.env.XDG_DATA_HOME) {
    return path.join(process.env.XDG_DATA_HOME, "elastos");
  }
  if (process.env.HOME) {
    return path.join(process.env.HOME, ".local/share/elastos");
  }
  return "/var/lib/elastos";
}

function defaultVmRoot(dataDir, platform) {
  if (platform.startsWith("linux-")) {
    return path.join(dataDir, "bvm");
  }
  return "/tmp/evzs";
}

function pathStatus(file) {
  if (!file) return { ok: false, path: "" };
  return { ok: fs.existsSync(file), path: file };
}

function canonicalJson(value) {
  if (Array.isArray(value)) {
    return value.map(canonicalJson);
  }
  if (value && typeof value === "object") {
    return Object.keys(value).sort().reduce((out, key) => {
      out[key] = canonicalJson(value[key]);
      return out;
    }, {});
  }
  return value;
}

function sha256Json(value) {
  return crypto
    .createHash("sha256")
    .update(JSON.stringify(canonicalJson(value)))
    .digest("hex");
}

function fileFingerprint(file) {
  if (!file) return null;
  try {
    const stat = fs.statSync(file);
    if (!stat.isFile()) {
      return { path: file, ok: false, reason: "not_file" };
    }
    return {
      path: file,
      ok: true,
      sha256: crypto.createHash("sha256").update(fs.readFileSync(file)).digest("hex"),
    };
  } catch (error) {
    return { path: file, ok: false, reason: error?.code || String(error) };
  }
}

function controlServiceArtifactFingerprints(controlService) {
  const paths = [controlService];
  if (controlService && !controlService.endsWith(".mjs")) {
    paths.push(`${controlService}.mjs`);
  }
  return paths.map(fileFingerprint).filter(Boolean);
}

function vmControlEnvFingerprintFields({ dataDir, platform, root }) {
  const env = {
    [DATA_DIR_ENV]: dataDir,
    [ROOT_ENV]: root,
    [PLATFORM_ENV]: platform,
  };
  for (const [key, value] of Object.entries(process.env)) {
    if (
      !key.startsWith("ELASTOS_BROWSER_VM_") &&
      !key.startsWith("ELASTOS_BROWSER_REMOTE_VZ_")
    ) {
      continue;
    }
    if (key === CONTROL_SERVICE_CONFIG_ENV) continue;
    env[key] = value;
  }
  return env;
}

function vmControlConfigFingerprint({ config, controlService, dataDir, platform, root }) {
  return sha256Json({
    config,
    env: vmControlEnvFingerprintFields({ dataDir, platform, root }),
    artifacts: {
      control_service: controlServiceArtifactFingerprints(controlService),
      launcher: controlServiceArtifactFingerprints(config.launcher_program),
    },
  });
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function vmControlMaxActivePages() {
  const value = Number(process.env.ELASTOS_BROWSER_VM_CONTROL_MAX_ACTIVE_PAGES || "1");
  if (!Number.isInteger(value) || value < 1 || value > 32) {
    fail("ELASTOS_BROWSER_VM_CONTROL_MAX_ACTIVE_PAGES must be 1..32");
  }
  return value;
}

function isRemoteVzControlLauncher(launcher) {
  return path.basename(String(launcher || "")).startsWith("browser-vm-remote-vz-launcher");
}

function remoteVzLaunchTimeoutMs(config) {
  const configured = Number(process.env.ELASTOS_BROWSER_REMOTE_VZ_LAUNCH_TIMEOUT_MS || "");
  if (Number.isFinite(configured) && configured >= 1000) return configured;
  const launchTimeoutMs = Number(config.launch_timeout_ms ?? 120000);
  return Math.max(1000, launchTimeoutMs - 30000);
}

function applyRemoteVzControlDefaults(config) {
  if (!isRemoteVzControlLauncher(config.launcher_program)) return config;
  config.remote_vz_launch_timeout_ms = remoteVzLaunchTimeoutMs(config);
  config.remote_vz_trace_egress = process.env.ELASTOS_BROWSER_VM_TRACE_EGRESS === "1";
  return config;
}

function collectPreflight({ dataDir, platform }) {
  const rootfs = process.env.ELASTOS_BROWSER_VM_ROOTFS || path.join(dataDir, "browser-vm/rootfs.ext4");
  const controlSocket = process.env[CONTROL_SOCKET_ENV] || "";
  const common = {
    schema: "elastos.browser.vm-engine-preflight/v1",
    platform,
    data_dir: dataDir,
    control_socket: { ok: !!controlSocket && fs.existsSync(controlSocket), path: controlSocket },
    rootfs: pathStatus(rootfs),
    remote_control_supported: true,
    remote_control_hint: `Set ${CONTROL_SOCKET_ENV} to a local Unix socket served by a Browser VM control plane. That socket may be backed by a local VM substrate or by a remote VM provider reached through Runtime/Carrier/SSH tunnel plumbing.`,
  };
  if (platform.startsWith("linux-")) {
    const crosvm = process.env.ELASTOS_BROWSER_VM_CROSVM_BIN || path.join(dataDir, "bin/crosvm");
    const kernel = process.env.ELASTOS_BROWSER_VM_KERNEL || path.join(dataDir, "bin/vmlinux");
    const local = {
      kvm: { ok: fs.existsSync("/dev/kvm"), path: "/dev/kvm" },
      crosvm: pathStatus(crosvm),
      kernel: pathStatus(kernel),
      rootfs: common.rootfs,
    };
    const localMissing = Object.entries(local).filter(([, value]) => !value.ok).map(([name]) => name);
    const remoteReady = common.control_socket.ok;
    return {
      ...common,
      substrate: "crosvm",
      ok: remoteReady,
      launch_ready: remoteReady,
      local_substrate_ready: localMissing.length === 0,
      missing_for_local_substrate: localMissing,
      execution_mode: remoteReady
        ? "remote_vm_control_socket"
        : localMissing.length === 0
          ? "local_substrate_ready_needs_vm_control_service"
          : "unavailable",
      reason: remoteReady
        ? "Browser VM control socket is available; local KVM/VZ is not required on this host."
        : localMissing.includes("kvm")
          ? `Local crosvm Browser VM is unavailable because /dev/kvm is missing. This is acceptable for a gateway host if ${CONTROL_SOCKET_ENV} points at a remote/operator Browser VM provider.`
          : "Browser VM launch is unavailable until a control socket is configured or the local substrate is provisioned.",
      kvm: local.kvm,
      crosvm: local.crosvm,
      kernel: local.kernel,
    };
  }
  if (platform.startsWith("darwin-")) {
    const vzSupervisor = process.env.ELASTOS_BROWSER_VM_VZ_SUPERVISOR || path.join(dataDir, "bin/browser-vz-engine-supervisor");
    const kernel = process.env.ELASTOS_BROWSER_VM_KERNEL || path.join(dataDir, "bin/vmlinux");
    const local = {
      vz_supervisor: pathStatus(vzSupervisor),
      kernel: pathStatus(kernel),
      rootfs: common.rootfs,
    };
    const localMissing = Object.entries(local).filter(([, value]) => !value.ok).map(([name]) => name);
    const remoteReady = common.control_socket.ok;
    return {
      ...common,
      substrate: "apple_virtualization_framework",
      ok: remoteReady,
      launch_ready: remoteReady,
      local_substrate_ready: localMissing.length === 0,
      missing_for_local_substrate: localMissing,
      execution_mode: remoteReady
        ? "remote_vm_control_socket"
        : localMissing.length === 0
          ? "local_substrate_ready_needs_vm_control_service"
          : "unavailable",
      reason: remoteReady
        ? "Browser VM control socket is available; local KVM/VZ is not required on this host."
        : "Browser VM launch is unavailable until a control socket is configured or the Apple VZ substrate is provisioned.",
      vz_supervisor: local.vz_supervisor,
      kernel: local.kernel,
    };
  }
  return {
    ...common,
    substrate: "unsupported",
    ok: false,
    launch_ready: false,
    local_substrate_ready: false,
    missing_for_local_substrate: ["supported_platform"],
    execution_mode: "unavailable",
    reason: "Browser VM launch is unavailable on this unsupported platform.",
  };
}

function postJsonOverUnix(socketPath, requestPath, body, timeoutMs) {
  validateAbsolutePath(socketPath, CONTROL_SOCKET_ENV);
  const bytes = Buffer.from(JSON.stringify(body));
  return new Promise((resolve, reject) => {
    const request = http.request(
      {
        socketPath,
        path: requestPath,
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
            reject(new Error(`Browser VM control response is not JSON: ${error.message}`));
            return;
          }
          if ((response.statusCode || 500) < 200 || (response.statusCode || 500) >= 300) {
            reject(new Error(parsed.error || parsed.message || `Browser VM control returned ${response.statusCode}`));
            return;
          }
          resolve(parsed);
        });
      },
    );
    request.on("timeout", () => request.destroy(new Error("Browser VM control request timed out")));
    request.on("error", reject);
    request.end(bytes);
  });
}

function getJsonOverUnix(socketPath, requestPath, timeoutMs) {
  validateAbsolutePath(socketPath, CONTROL_SOCKET_ENV);
  return new Promise((resolve, reject) => {
    const request = http.request(
      {
        socketPath,
        path: requestPath,
        method: "GET",
        timeout: timeoutMs,
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
            reject(new Error(`Browser VM control status is not JSON: ${error.message}`));
            return;
          }
          if ((response.statusCode || 500) < 200 || (response.statusCode || 500) >= 300) {
            reject(new Error(parsed.error || parsed.message || `Browser VM control status returned ${response.statusCode}`));
            return;
          }
          resolve(parsed);
        });
      },
    );
    request.on("timeout", () => request.destroy(new Error("Browser VM control status timed out")));
    request.on("error", reject);
    request.end();
  });
}

async function waitForVmControlStatus(controlSocket, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  let lastError = null;
  while (Date.now() < deadline) {
    try {
      const status = await getJsonOverUnix(controlSocket, "/status", 1000);
      if (status?.schema === "elastos.browser.vm-control-service.status/v1" && status.ok === true) {
        return status;
      }
      lastError = new Error("Browser VM control status returned an invalid schema");
    } catch (error) {
      lastError = error;
    }
    await sleep(100);
  }
  throw lastError || new Error("Browser VM control status did not become ready");
}

function localControlLauncherForPlatform({ dataDir, platform }) {
  if (process.env[CONTROL_LAUNCHER_ENV]) {
    return process.env[CONTROL_LAUNCHER_ENV];
  }
  if (platform.startsWith("darwin-")) {
    return process.env.ELASTOS_BROWSER_VM_VZ_SUPERVISOR || path.join(dataDir, "bin/browser-vz-engine-supervisor");
  }
  if (platform.startsWith("linux-")) {
    return path.join(dataDir, "bin/browser-vm-local-crosvm-launcher");
  }
  return "";
}

function localVmControlServiceAvailable({ dataDir, platform }) {
  if (process.env.ELASTOS_BROWSER_VM_AUTO_START_CONTROL_SERVICE === "0") {
    return false;
  }
  const controlService = process.env[CONTROL_SERVICE_ENV] || path.join(dataDir, "bin/browser-vm-control-service");
  const launcher = localControlLauncherForPlatform({ dataDir, platform });
  if (!controlService || !fs.existsSync(controlService) || !launcher || !fs.existsSync(launcher)) {
    return false;
  }
  return true;
}

function sanitizedVmControlServiceEnv({ config, dataDir, root, platform }) {
  const env = { ...process.env };
  delete env[REQUEST_ENV];
  delete env.ELASTOS_BROWSER_VM_OPEN_REQUEST;
  const serviceEnv = {
    ...env,
    [CONTROL_SERVICE_CONFIG_ENV]: JSON.stringify(config),
    [DATA_DIR_ENV]: dataDir,
    [ROOT_ENV]: root,
    [PLATFORM_ENV]: platform,
  };
  if (
    isRemoteVzControlLauncher(config.launcher_program) &&
    !serviceEnv.ELASTOS_BROWSER_REMOTE_VZ_LAUNCH_TIMEOUT_MS
  ) {
    serviceEnv.ELASTOS_BROWSER_REMOTE_VZ_LAUNCH_TIMEOUT_MS = String(
      config.remote_vz_launch_timeout_ms ?? remoteVzLaunchTimeoutMs(config),
    );
  }
  if (
    isRemoteVzControlLauncher(config.launcher_program) &&
    !serviceEnv.ELASTOS_BROWSER_VM_TRACE_EGRESS
  ) {
    serviceEnv.ELASTOS_BROWSER_VM_TRACE_EGRESS =
      config.remote_vz_trace_egress === true ? "1" : "0";
  }
  return serviceEnv;
}

function startLocalVmControlService({ controlSocket, dataDir, platform, root, expectedFingerprint }) {
  if (!localVmControlServiceAvailable({ dataDir, platform })) return false;
  const controlService = process.env[CONTROL_SERVICE_ENV] || path.join(dataDir, "bin/browser-vm-control-service");
  const launcher = localControlLauncherForPlatform({ dataDir, platform });

  const config = applyRemoteVzControlDefaults({
    schema: "elastos.browser.vm-control-service.config/v1",
    control_socket_path: controlSocket,
    launcher_program: launcher,
    launcher_args: [],
    persistent_launcher: true,
    replace_existing_socket: true,
    max_active_pages: vmControlMaxActivePages(),
    idle_vm_keepalive_ms: Number(process.env.ELASTOS_BROWSER_VM_IDLE_KEEPALIVE_MS || "0"),
    reuse_idle_vms: process.env.ELASTOS_BROWSER_VM_REUSE_IDLE_VMS === "1",
    hibernation_mode: process.env.ELASTOS_BROWSER_VM_HIBERNATION === "1" && platform.startsWith("darwin-")
      ? "vz_save_restore"
      : "off",
    launch_timeout_ms: Number(process.env.ELASTOS_BROWSER_VM_CONTROL_LAUNCH_TIMEOUT_MS || "150000"),
    shutdown_timeout_ms: Number(process.env.ELASTOS_BROWSER_VM_CONTROL_SHUTDOWN_TIMEOUT_MS || "30000"),
  });
  config.config_fingerprint = expectedFingerprint ||
    vmControlConfigFingerprint({ config, controlService, dataDir, platform, root });
  const logPath = process.env[CONTROL_SERVICE_LOG_ENV] || path.join(dataDir, "logs/browser-vm-control-service.log");
  fs.mkdirSync(path.dirname(logPath), { recursive: true, mode: 0o700 });
  const logFd = fs.openSync(logPath, "a");
  try {
    const child = spawn(controlService, [], {
      detached: true,
      env: sanitizedVmControlServiceEnv({ config, dataDir, root, platform }),
      stdio: ["ignore", logFd, logFd],
    });
    child.on("error", (error) => {
      process.stderr.write(`Browser VM control service auto-start failed: ${error.message}\n`);
    });
    child.unref();
  } finally {
    fs.closeSync(logFd);
  }
  return true;
}

async function vmControlStatusOrNull(controlSocket, timeoutMs = 1000) {
  try {
    const status = await getJsonOverUnix(controlSocket, "/status", timeoutMs);
    return status?.schema === "elastos.browser.vm-control-service.status/v1" && status.ok === true
      ? status
      : null;
  } catch {
    return null;
  }
}

function vmControlStatusMatches(status, expectedFingerprint) {
  return status?.config_fingerprint === expectedFingerprint;
}

async function stopStaleVmControlService(status, controlSocket) {
  const pid = Number(status?.pid || 0);
  if (Number.isInteger(pid) && pid > 1 && pid !== process.pid) {
    try {
      process.kill(pid, "SIGTERM");
    } catch {}
    for (let attempt = 0; attempt < 50; attempt += 1) {
      const current = await vmControlStatusOrNull(controlSocket, 100);
      if (!current || Number(current.pid || 0) !== pid) {
        return;
      }
      await sleep(100);
    }
  }
  try {
    fs.unlinkSync(controlSocket);
  } catch (error) {
    if (error?.code !== "ENOENT") throw error;
  }
}

async function waitForMatchingVmControlStatus(controlSocket, expectedFingerprint, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  let last = null;
  while (Date.now() < deadline) {
    last = await vmControlStatusOrNull(controlSocket, 1000);
    if (vmControlStatusMatches(last, expectedFingerprint)) return last;
    await sleep(100);
  }
  throw new Error(
    last
      ? "Browser VM control status did not match requested config fingerprint"
      : "Browser VM control status did not become ready",
  );
}

async function ensureVmControlAvailable({ controlSocket, dataDir, platform, root }) {
  const launcher = localControlLauncherForPlatform({ dataDir, platform });
  const controlService = process.env[CONTROL_SERVICE_ENV] || path.join(dataDir, "bin/browser-vm-control-service");
  const controlConfig = applyRemoteVzControlDefaults({
    schema: "elastos.browser.vm-control-service.config/v1",
    control_socket_path: controlSocket,
    launcher_program: launcher,
    launcher_args: [],
    persistent_launcher: true,
    replace_existing_socket: true,
    max_active_pages: vmControlMaxActivePages(),
    idle_vm_keepalive_ms: Number(process.env.ELASTOS_BROWSER_VM_IDLE_KEEPALIVE_MS || "0"),
    reuse_idle_vms: process.env.ELASTOS_BROWSER_VM_REUSE_IDLE_VMS === "1",
    hibernation_mode: process.env.ELASTOS_BROWSER_VM_HIBERNATION === "1" && platform.startsWith("darwin-")
      ? "vz_save_restore"
      : "off",
    launch_timeout_ms: Number(process.env.ELASTOS_BROWSER_VM_CONTROL_LAUNCH_TIMEOUT_MS || "150000"),
    shutdown_timeout_ms: Number(process.env.ELASTOS_BROWSER_VM_CONTROL_SHUTDOWN_TIMEOUT_MS || "30000"),
  });
  const expectedFingerprint = vmControlConfigFingerprint({
    config: controlConfig,
    controlService,
    dataDir,
    platform,
    root,
  });

  const existing = await vmControlStatusOrNull(controlSocket, 1200);
  if (vmControlStatusMatches(existing, expectedFingerprint)) {
    return;
  }
  if (existing && !localVmControlServiceAvailable({ dataDir, platform })) {
    return;
  }

  const lockDir = `${controlSocket}.start.lock`;
  let lockOwned = false;
  try {
    fs.mkdirSync(lockDir, { recursive: false, mode: 0o700 });
    lockOwned = true;
    const lockedExisting = await vmControlStatusOrNull(controlSocket, 1000);
    if (vmControlStatusMatches(lockedExisting, expectedFingerprint)) {
      return;
    }
    if (lockedExisting) {
      await stopStaleVmControlService(lockedExisting, controlSocket);
    }
    if (!startLocalVmControlService({
      controlSocket,
      dataDir,
      platform,
      root,
      expectedFingerprint,
    })) {
      throw new Error("Browser VM control socket is unavailable");
    }
  } catch (error) {
    if (error?.code !== "EEXIST") {
      throw error;
    }
  } finally {
    if (lockOwned) {
      fs.rmSync(lockDir, { recursive: true, force: true });
    }
  }
  try {
    await waitForMatchingVmControlStatus(controlSocket, expectedFingerprint, 7000);
  } catch (error) {
    throw new Error(`Browser VM control service did not become ready: ${error.message}`);
  }
}

function validateSupervisorResult(result, request) {
  if (result.schema !== "elastos.browser.engine.supervisor-result/v1") {
    throw new Error("Browser VM control did not return elastos.browser.engine.supervisor-result/v1");
  }
  if (!safeId(result.page_id)) {
    throw new Error("Browser VM control returned an unsafe page_id");
  }
  if (result.adapter !== request.adapter || result.engine !== request.engine || result.stream_id !== request.stream_id) {
    throw new Error("Browser VM control returned a mismatched adapter, engine, or stream_id");
  }
  if (result.network_mode !== "runtime_net_only" || result.direct_network !== false || result.wallet_injection !== false) {
    throw new Error("Browser VM control must report runtime_net_only with no direct network or wallet injection");
  }
  if (result.display_session?.mode !== request.display_mode) {
    throw new Error("Browser VM control returned a mismatched display mode");
  }
  if (result.display_session?.media_transport !== "runtime_relay") {
    throw new Error("Browser VM display sessions must report media_transport=runtime_relay");
  }
}

async function delegateToVmControl({ controlSocket, request, timeoutMs, platform, sessionDir }) {
  const result = await postJsonOverUnix(
    controlSocket,
    "/pages",
    {
      schema: "elastos.browser.vm-engine.open/v1",
      launch_request: request,
      requirements: {
        substrate: "microvm",
        platform,
        display_mode: request.display_mode,
        guarantee_level: request.guarantee_level,
        backend_class: "product_compositor",
        network_mode: "runtime_net_only",
        direct_network: false,
      },
      profile: request.profile,
    },
    timeoutMs,
  );
  validateSupervisorResult(result, request);
  if (!result.control_socket_path) result.control_socket_path = controlSocket;
  if (!result.isolated_session) result.isolated_session = true;
  if (!result.isolation) {
    result.isolation = {
      schema: "elastos.browser.engine.isolation/v1",
      kind: "per_launch_vm_target",
      session_dir: sessionDir,
    };
  }
  process.stdout.write(`${JSON.stringify(result)}\n`);
}

async function main() {
  const scriptPath = fileURLToPath(import.meta.url);
  const platform = platformId();
  const dataDir = defaultDataDir(scriptPath);
  const root = process.env[ROOT_ENV] || defaultVmRoot(dataDir, platform);
  validateAbsolutePath(root, ROOT_ENV);
  const hasLaunchRequest =
    typeof process.env[REQUEST_ENV] === "string" &&
    process.env[REQUEST_ENV].trim() !== "";

  if (process.env.ELASTOS_BROWSER_VM_PREWARM_CONTROL_SERVICE === "1" && !hasLaunchRequest) {
    const controlSocket = process.env[CONTROL_SOCKET_ENV] || "";
    if (!controlSocket) {
      fail(`${CONTROL_SOCKET_ENV} is required for Browser VM prewarm`);
    }
    await ensureVmControlAvailable({ controlSocket, dataDir, platform, root });
    const status = await getJsonOverUnix(controlSocket, "/status", 1000);
    process.stdout.write(`${JSON.stringify({
      schema: "elastos.browser.vm-engine-prewarm/v1",
      ok: true,
      control_socket_path: controlSocket,
      control_status: {
        schema: status.schema,
        pid: status.pid,
        started_at: status.started_at,
        active_pages: status.active_pages,
        warm_vms: status.warm_vms,
        max_active_pages: status.max_active_pages,
        idle_vm_keepalive_ms: status.idle_vm_keepalive_ms,
        reuse_idle_vms: status.reuse_idle_vms === true,
        hibernation_mode: status.hibernation_mode || "off",
        network_mode: status.network_mode,
        direct_network: status.direct_network,
      },
      network_mode: "runtime_net_only",
      direct_network: false,
    })}\n`);
    return;
  }

  const request = parseJsonEnv(REQUEST_ENV);
  validateLaunchRequest(request);

  const sessionDir = path.join(root, sessionSuffix(request.stream_id));
  fs.mkdirSync(sessionDir, { recursive: true, mode: 0o700 });

  const controlSocket = process.env[CONTROL_SOCKET_ENV] || "";
  if (controlSocket) {
    await ensureVmControlAvailable({ controlSocket, dataDir, platform, root });
    await delegateToVmControl({
      controlSocket,
      request,
      timeoutMs: Number(process.env.ELASTOS_BROWSER_VM_ENGINE_TIMEOUT_MS || "165000"),
      platform,
      sessionDir,
    });
    return;
  }

  const preflight = collectPreflight({ dataDir, platform });
  fail(`Browser VM engine target is not launch-ready. ${preflight.reason} Set ${CONTROL_SOCKET_ENV} to a Browser VM control service; on no-KVM gateway hosts this should point at a remote/operator VM provider instead of requiring local KVM. Preflight: ${JSON.stringify(preflight)}`);
}

main().catch((error) => {
  fail(error instanceof Error ? error.message : String(error));
});
