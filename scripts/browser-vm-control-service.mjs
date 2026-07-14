#!/usr/bin/env node
import crypto from "node:crypto";
import fs from "node:fs";
import http from "node:http";
import path from "node:path";
import process from "node:process";
import { spawn } from "node:child_process";

const CONFIG_ENV = "ELASTOS_BROWSER_VM_CONTROL_SERVICE_CONFIG";
const OPEN_REQUEST_ENV = "ELASTOS_BROWSER_VM_OPEN_REQUEST";
const MAX_BROWSER_FILE_UPLOAD_BYTES = 16 * 1024 * 1024;
const MAX_BROWSER_INPUT_BODY_BYTES =
  Math.ceil((MAX_BROWSER_FILE_UPLOAD_BYTES * 4) / 3) + 64 * 1024;

function fail(message) {
  console.error(message);
  process.exit(1);
}

function parseConfig() {
  const raw = process.env[CONFIG_ENV];
  if (!raw) fail(`${CONFIG_ENV} is required`);
  let config;
  try {
    config = JSON.parse(raw);
  } catch (error) {
    fail(`${CONFIG_ENV} is invalid JSON: ${error instanceof Error ? error.message : String(error)}`);
  }
  validateConfig(config);
  return config;
}

function safeId(value) {
  return typeof value === "string" && /^[A-Za-z0-9:_-]+$/.test(value);
}

function validateAbsolutePath(value, label) {
  if (typeof value !== "string" || !value.startsWith("/") || /[\r\n\0]/.test(value)) {
    throw new Error(`${label} must be an absolute path without control characters`);
  }
}

function validateConfig(config) {
  if (config.schema !== "elastos.browser.vm-control-service.config/v1") {
    throw new Error("unsupported Browser VM control service config schema");
  }
  validateAbsolutePath(config.control_socket_path, "control_socket_path");
  validateAbsolutePath(config.launcher_program, "launcher_program");
  if (!fs.existsSync(config.launcher_program)) {
    throw new Error(`launcher_program does not exist: ${config.launcher_program}`);
  }
  if (!Array.isArray(config.launcher_args || [])) {
    throw new Error("launcher_args must be an array");
  }
  for (const arg of config.launcher_args || []) {
    if (typeof arg !== "string" || /[\r\n\0]/.test(arg)) {
      throw new Error("launcher_args entries must be strings without control characters");
    }
  }
  if (config.shutdown_program !== undefined) {
    validateAbsolutePath(config.shutdown_program, "shutdown_program");
  }
  if (!Array.isArray(config.shutdown_args || [])) {
    throw new Error("shutdown_args must be an array");
  }
  if (
    config.config_fingerprint !== undefined &&
    (typeof config.config_fingerprint !== "string" ||
      !/^[0-9a-f]{64}$/.test(config.config_fingerprint))
  ) {
    throw new Error("config_fingerprint must be a sha256 hex string");
  }
  if (config.persistent_launcher !== undefined && typeof config.persistent_launcher !== "boolean") {
    throw new Error("persistent_launcher must be a boolean");
  }
  if (config.reuse_idle_vms !== undefined && typeof config.reuse_idle_vms !== "boolean") {
    throw new Error("reuse_idle_vms must be a boolean");
  }
  const maxActivePages = Number(config.max_active_pages ?? 1);
  if (!Number.isInteger(maxActivePages) || maxActivePages < 1 || maxActivePages > 32) {
    throw new Error("max_active_pages must be 1..32");
  }
  const idleVmKeepaliveMs = Number(config.idle_vm_keepalive_ms ?? 0);
  if (!Number.isInteger(idleVmKeepaliveMs) || idleVmKeepaliveMs < 0 || idleVmKeepaliveMs > 3600000) {
    throw new Error("idle_vm_keepalive_ms must be 0..3600000");
  }
  if (
    config.hibernation_mode !== undefined &&
    !["off", "vz_save_restore"].includes(config.hibernation_mode)
  ) {
    throw new Error("hibernation_mode must be off or vz_save_restore");
  }
  const timeout = Number(config.launch_timeout_ms ?? 120000);
  if (!Number.isInteger(timeout) || timeout < 1000 || timeout > 600000) {
    throw new Error("launch_timeout_ms must be 1000..600000");
  }
}

function jsonResponse(res, status, body) {
  const bytes = Buffer.from(JSON.stringify(body));
  res.writeHead(status, {
    "content-type": "application/json",
    "content-length": bytes.length,
  });
  res.end(bytes);
}

function logEvent(event, fields = {}) {
  process.stderr.write(`${JSON.stringify({
    schema: "elastos.browser.vm-control-service.event/v1",
    event,
    ts: new Date().toISOString(),
    ...fields,
  })}\n`);
}

function readJsonBody(req, maxBytes = 1024 * 1024) {
  return new Promise((resolve, reject) => {
    const chunks = [];
    let size = 0;
    req.on("data", (chunk) => {
      size += chunk.length;
      if (size > maxBytes) {
        reject(new Error("request body too large"));
        req.destroy();
        return;
      }
      chunks.push(chunk);
    });
    req.on("end", () => {
      try {
        const raw = Buffer.concat(chunks).toString("utf8");
        resolve(raw ? JSON.parse(raw) : {});
      } catch (error) {
        reject(new Error(`request body is not JSON: ${error instanceof Error ? error.message : String(error)}`));
      }
    });
    req.on("error", reject);
  });
}

function validateVmOpenRequest(body) {
  if (body.schema !== "elastos.browser.vm-engine.open/v1") {
    throw new Error("unsupported Browser VM open request schema");
  }
  const launch = body.launch_request || {};
  if (launch.schema !== "elastos.browser.engine.launch-request/v1") {
    throw new Error("missing Browser engine launch request");
  }
  if (!safeId(launch.adapter) || !safeId(launch.stream_id)) {
    throw new Error("launch request adapter and stream_id must be safe identifiers");
  }
  if (launch.engine !== "chromium_microvm") {
    throw new Error("Browser VM control service accepts only chromium_microvm");
  }
  if (launch.display_mode !== "webrtc_remote_display") {
    throw new Error("Browser VM control service requires webrtc_remote_display");
  }
  if (launch.guarantee_level !== "mechanism_microvm") {
    throw new Error("Browser VM control service requires guarantee_level=mechanism_microvm");
  }
  if (launch.network_mode !== "runtime_net_only" || launch.direct_network !== false) {
    throw new Error("Browser VM control service requires runtime_net_only and direct_network=false");
  }
  if (launch.wallet_injection !== false) {
    throw new Error("Browser VM control service must not receive wallet injection authority");
  }
  if (typeof launch.url !== "string" || !/^https?:\/\//.test(launch.url)) {
    throw new Error("launch request url must use http or https");
  }
  if (body.requirements?.substrate !== "microvm") {
    throw new Error("Browser VM control service requires substrate=microvm");
  }
  if (body.requirements?.network_mode !== "runtime_net_only" || body.requirements?.direct_network !== false) {
    throw new Error("Browser VM control service requires runtime_net_only VM requirements");
  }
  return launch;
}

function validateSupervisorResult(result, launch) {
  if (result.schema !== "elastos.browser.engine.supervisor-result/v1") {
    throw new Error("launcher did not return elastos.browser.engine.supervisor-result/v1");
  }
  if (!safeId(result.page_id)) {
    throw new Error("launcher returned unsafe page_id");
  }
  if (result.adapter !== launch.adapter || result.engine !== launch.engine || result.stream_id !== launch.stream_id) {
    throw new Error("launcher returned mismatched adapter, engine, or stream_id");
  }
  if (result.network_mode !== "runtime_net_only" || result.direct_network !== false || result.wallet_injection !== false) {
    throw new Error("launcher must report runtime_net_only with no direct network or wallet injection");
  }
  if (result.display_session?.schema !== "elastos.browser.display-session/v1") {
    throw new Error("launcher returned invalid display session schema");
  }
  if (result.display_session?.mode !== launch.display_mode) {
    throw new Error("launcher returned a mismatched display mode");
  }
  const expectedBackendClass = "product_compositor";
  if (result.display_session?.backend_class !== expectedBackendClass) {
    throw new Error(`launcher display session must be ${expectedBackendClass}`);
  }
  const expectedMediaTransport = "runtime_relay";
  if (result.display_session?.media_transport !== expectedMediaTransport) {
    throw new Error(`Browser VM display sessions must report media_transport=${expectedMediaTransport}`);
  }
  if (result.display_session?.audio !== true || result.display_session?.video !== true) {
    throw new Error("Browser VM product display sessions must advertise audio=true and video=true");
  }
  if (
    result.display_session?.audio_offer?.schema !== "elastos.browser.webrtc-offer/v1" ||
    result.display_session?.audio_offer?.type !== "offer" ||
    typeof result.display_session?.audio_offer?.sdp !== "string" ||
    !sdpHasMediaKind(result.display_session.audio_offer.sdp, "audio")
  ) {
    throw new Error("Browser VM product display sessions must include an audio WebRTC offer");
  }
  if (result.display_session?.network_mode !== "runtime_net_only" || result.display_session?.direct_network !== false) {
    throw new Error("launcher display session must report runtime_net_only and direct_network=false");
  }
  if (result.isolation?.kind !== "per_launch_vm_target") {
    throw new Error("launcher must report per_launch_vm_target isolation");
  }
  try {
    validateAbsolutePath(result.isolation?.session_dir, "launcher isolation session_dir");
  } catch {
    throw new Error("launcher returned invalid Browser VM session directory");
  }
}

function sameLaunchIdentity(a, b) {
  return Boolean(a && b) &&
    a.adapter === b.adapter &&
    a.engine === b.engine &&
    a.stream_id === b.stream_id &&
    a.url === b.url &&
    a.display_mode === b.display_mode &&
    a.guarantee_level === b.guarantee_level &&
    a.network_mode === b.network_mode &&
    a.direct_network === b.direct_network &&
    a.wallet_injection === b.wallet_injection;
}

function sameVmStream(a, b) {
  return Boolean(a && b) &&
    a.adapter === b.adapter &&
    a.engine === b.engine &&
    a.stream_id === b.stream_id;
}

function idleVmKeepaliveMs(config) {
  return Number(config.idle_vm_keepalive_ms ?? 0);
}

function idleVmReuseEnabled(config) {
  return config.reuse_idle_vms === true;
}

function vmKeyHash(vmKey) {
  return crypto.createHash("sha256").update(String(vmKey || "")).digest("hex").slice(0, 16);
}

const LIFECYCLE_PHASES = [
  "CONTROL_READY",
  "ACQUIRING_SLOT",
  "PREPARING_IMAGE",
  "STARTING_VM",
  "GUEST_READY",
  "ACTIVE_SESSION",
  "NAVIGATING",
  "QUIESCING_PAGE",
  "WARM_IDLE",
  "HIBERNATING",
  "HIBERNATED",
  "RETIRING",
  "FAILED",
];

function lifecycleHash(value) {
  const text = String(value || "").trim();
  if (!text) return null;
  return `sha256:${crypto.createHash("sha256").update(text).digest("hex").slice(0, 16)}`;
}

function lifecycleUrl(value) {
  try {
    const parsed = new URL(String(value || ""));
    parsed.username = "";
    parsed.password = "";
    parsed.hash = "";
    return parsed.href;
  } catch {
    return "";
  }
}

function startedAtAgeMs(startedAt, nowMs) {
  const startedMs = Date.parse(startedAt || "");
  return Number.isFinite(startedMs) ? Math.max(0, nowMs - startedMs) : null;
}

function lifecycleExitId(launch) {
  const authority = streamAuthorityFromStreamId(launch?.stream_id);
  if (!authority || authority === "local-runtime") {
    return "local-runtime";
  }
  return `remote-carrier:${lifecycleHash(authority)}`;
}

function lifecycleProfileKeyHash(launch) {
  return lifecycleHash(launch?.profile?.profile_key);
}

function lifecycleActivePageRecord(pageId, record, nowMs) {
  return {
    session_id: lifecycleHash(record?.launch?.stream_id || pageId),
    page_id: lifecycleHash(pageId),
    principal_id: lifecycleHash(record?.launch?.principal_id),
    profile_key_hash: lifecycleProfileKeyHash(record?.launch),
    exit_id: lifecycleExitId(record?.launch),
    url: lifecycleUrl(record?.page?.actual_url || record?.page?.url || record?.launch?.url),
    phase: "ACTIVE_SESSION",
    started_at: record?.started_at || null,
    age_ms: startedAtAgeMs(record?.started_at, nowMs),
    last_navigation_at: null,
    last_frame_at: null,
    pending_launch_age_ms: null,
    vm_key_hash: record?.vm_key ? lifecycleHash(record.vm_key) : null,
    warm_vm: false,
    capacity_available: false,
    failure_reason: null,
  };
}

function lifecyclePendingLaunchRecord(requestId, launch, nowMs) {
  return {
    session_id: lifecycleHash(requestId),
    page_id: null,
    principal_id: lifecycleHash(launch?.principal_id),
    profile_key_hash: lifecycleHash(launch?.profile_key),
    exit_id: launch?.exit_id || "local-runtime",
    url: lifecycleUrl(launch?.url),
    phase: launch?.phase || "STARTING_VM",
    started_at: launch?.started_at || null,
    age_ms: startedAtAgeMs(launch?.started_at, nowMs),
    last_navigation_at: null,
    last_frame_at: null,
    pending_launch_age_ms: startedAtAgeMs(launch?.started_at, nowMs),
    vm_key_hash: launch?.vm_key_hash || null,
    warm_vm: false,
    capacity_available: false,
    failure_reason: launch?.failure_reason || null,
  };
}

function lifecycleWarmVmRecord(vmKey, vmRecord, nowMs, config) {
  return {
    session_id: lifecycleHash(vmKey),
    page_id: null,
    principal_id: null,
    profile_key_hash: lifecycleHash(vmRecord?.profile_lease_key),
    exit_id: "redacted",
    url: "",
    phase: config.hibernation_mode === "vz_save_restore" ? "HIBERNATED" : "WARM_IDLE",
    started_at: vmRecord?.started_at || null,
    age_ms: startedAtAgeMs(vmRecord?.started_at, nowMs),
    last_navigation_at: null,
    last_frame_at: null,
    pending_launch_age_ms: null,
    vm_key_hash: lifecycleHash(vmKey),
    warm_vm: true,
    capacity_available: true,
    failure_reason: null,
  };
}

function lifecycleStatus(config, activePages, activeVms, pendingLaunches) {
  const nowMs = Date.now();
  const maxActivePages = Number(config.max_active_pages ?? 1);
  const sessions = [
    ...[...pendingLaunches.entries()].map(([requestId, launch]) =>
      lifecyclePendingLaunchRecord(requestId, launch, nowMs),
    ),
    ...[...activePages.entries()].map(([pageId, record]) =>
      lifecycleActivePageRecord(pageId, record, nowMs),
    ),
    ...[...activeVms.entries()]
      .filter(([, vmRecord]) => vmRecord.pages.size === 0)
      .map(([vmKey, vmRecord]) => lifecycleWarmVmRecord(vmKey, vmRecord, nowMs, config)),
  ];
  return {
    schema: "elastos.browser.lifecycle-status/v1",
    owner: "vm_control_service",
    phases: LIFECYCLE_PHASES,
    capacity_available: activePages.size < maxActivePages,
    sessions,
    redaction: {
      principal_id: "sha256-16",
      session_id: "sha256-16",
      page_id: "sha256-16",
      profile_key: "sha256-16",
      exit_id: "local-or-sha256-16",
      vm_key: "sha256-16",
    },
  };
}

function clearIdleVmShutdown(vmRecord) {
  if (vmRecord?.idle_shutdown_timer) {
    clearTimeout(vmRecord.idle_shutdown_timer);
    vmRecord.idle_shutdown_timer = null;
  }
  if (vmRecord) {
    vmRecord.idle_expires_at = null;
  }
}

function retainIdleVm(config, vmKey, vmRecord, activeVms) {
  const keepaliveMs = idleVmKeepaliveMs(config);
  if (!idleVmReuseEnabled(config) || !vmRecord?.launcher_child || keepaliveMs <= 0) {
    return false;
  }
  clearIdleVmShutdown(vmRecord);
  const expiresAt = Date.now() + keepaliveMs;
  vmRecord.idle_expires_at = new Date(expiresAt).toISOString();
  vmRecord.idle_shutdown_timer = setTimeout(async () => {
    if (activeVms.get(vmKey) !== vmRecord || vmRecord.pages.size > 0) {
      return;
    }
    activeVms.delete(vmKey);
    logEvent("idle_vm_shutdown", {
      vm_key_hash: vmKeyHash(vmKey),
      idle_keepalive_ms: keepaliveMs,
    });
    await terminatePersistentLauncher(vmRecord.launcher_child, Number(config.shutdown_timeout_ms ?? 30000));
  }, keepaliveMs);
  vmRecord.idle_shutdown_timer.unref?.();
  logEvent("idle_vm_retained", {
    vm_key_hash: vmKeyHash(vmKey),
    idle_keepalive_ms: keepaliveMs,
    idle_expires_at: vmRecord.idle_expires_at,
  });
  return true;
}

function runProgram(program, args, env, stdin, timeoutMs) {
  return new Promise((resolve, reject) => {
    const child = spawn(program, args || [], {
      env,
      stdio: ["pipe", "pipe", "pipe"],
    });
    let stdout = "";
    let stderr = "";
    const timer = setTimeout(() => {
      child.kill("SIGTERM");
      reject(new Error("Browser VM launcher timed out"));
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
        reject(new Error(stderr.trim() || `Browser VM launcher exited with ${code ?? signal}`));
        return;
      }
      resolve({ stdout, stderr });
    });
    child.stdin.end(stdin);
  });
}

function forceTerminateChild(child, graceMs = 5000) {
  if (!child || child.exitCode != null || child.signalCode != null) return;
  try {
    child.kill("SIGTERM");
  } catch {}
  setTimeout(() => {
    if (child.exitCode == null && child.signalCode == null) {
      try {
        child.kill("SIGKILL");
      } catch {}
    }
  }, graceMs).unref();
}

function launcherError(message, stderr) {
  const detail = String(stderr || "").trim();
  if (!detail) return new Error(message);
  const bounded = detail.length > 8192 ? detail.slice(-8192) : detail;
  return new Error(`${message}: ${bounded}`);
}

function requestJsonOverUnix(socketPath, method, requestPath, body, timeoutMs) {
  validateAbsolutePath(socketPath, "Browser VM guest control socket");
  const bytes = body == null ? Buffer.alloc(0) : Buffer.from(JSON.stringify(body));
  return new Promise((resolve, reject) => {
    const req = http.request(
      {
        socketPath,
        path: requestPath,
        method,
        timeout: timeoutMs,
        headers: {
          accept: "application/json",
          "content-type": "application/json",
          "content-length": bytes.length,
        },
      },
      (res) => {
        const chunks = [];
        res.on("data", (chunk) => chunks.push(chunk));
        res.on("end", () => {
          const text = Buffer.concat(chunks).toString("utf8");
          let parsed = {};
          if (text) {
            try {
              parsed = JSON.parse(text);
            } catch (error) {
              reject(new Error(`Browser VM guest control returned non-JSON: ${error instanceof Error ? error.message : String(error)}`));
              return;
            }
          }
          if (res.statusCode < 200 || res.statusCode >= 300) {
            reject(new Error(parsed.error || `Browser VM guest control ${method} ${requestPath} failed: HTTP ${res.statusCode}`));
            return;
          }
          resolve(parsed);
        });
      },
    );
    req.on("timeout", () => {
      req.destroy(new Error(`Browser VM guest control ${method} ${requestPath} timed out`));
    });
    req.on("error", reject);
    req.end(bytes);
  });
}

function postJsonOverUnix(socketPath, requestPath, body, timeoutMs) {
  return requestJsonOverUnix(socketPath, "POST", requestPath, body, timeoutMs);
}

function getJsonOverUnix(socketPath, requestPath, timeoutMs) {
  return requestJsonOverUnix(socketPath, "GET", requestPath, null, timeoutMs);
}

function sdpHasMediaKind(sdp, kind) {
  const prefix = `m=${kind} `;
  return String(sdp || "")
    .split(/\r?\n/)
    .some((line) => line.startsWith(prefix));
}

function normalizeDisplayMediaFromOffer(display) {
  const videoSdp = display?.initial_offer?.sdp;
  const audioSdp = display?.audio_offer?.sdp || videoSdp;
  if (typeof videoSdp === "string") {
    display.video = sdpHasMediaKind(videoSdp, "video");
  }
  if (typeof audioSdp === "string") {
    display.audio = sdpHasMediaKind(audioSdp, "audio");
  }
}

function browserProfileDescriptor(body, launch) {
  return launch?.profile || body?.profile || null;
}

function launchProfileLeaseKey(body, launch) {
  const profile = browserProfileDescriptor(body, launch);
  if (
    !profile ||
    typeof profile.profile_key !== "string" ||
    !/^profile-[0-9a-fA-F]{64}$/.test(profile.profile_key) ||
    typeof profile.disk_path !== "string" ||
    !profile.disk_path.startsWith("/") ||
    /[\r\n\0]/.test(profile.disk_path)
  ) {
    return null;
  }
  const principalId = typeof launch?.principal_id === "string" ? launch.principal_id : "";
  if (!safeId(principalId)) {
    return null;
  }
  return JSON.stringify({
    principal_id: principalId,
    profile_key: profile.profile_key.toLowerCase(),
    profile_disk_path: profile.disk_path,
  });
}

function streamAuthorityFromStreamId(streamId) {
  if (typeof streamId !== "string") {
    return "";
  }
  const remote = streamId.match(/^remote-carrier:([^:]+):/);
  if (remote?.[1] && safeId(remote[1])) {
    return `remote-carrier:${remote[1]}`;
  }
  if (streamId.startsWith("stream:")) {
    return "local-runtime";
  }
  return "";
}

function sameProfileVmKey(body, launch) {
  const profileLease = launchProfileLeaseKey(body, launch);
  if (!profileLease) {
    return null;
  }
  const parsedProfileLease = JSON.parse(profileLease);
  return JSON.stringify({
    adapter: launch.adapter,
    engine: launch.engine,
    principal_id: parsedProfileLease.principal_id,
    profile_key: parsedProfileLease.profile_key,
    profile_disk_path: parsedProfileLease.profile_disk_path,
    guarantee_level: launch.guarantee_level,
    network_mode: launch.network_mode,
    direct_network: launch.direct_network,
    wallet_injection: launch.wallet_injection,
  });
}

async function retireConflictingIdleVmsForProfile(config, activeVms, vmKey, profileLeaseKey) {
  if (!profileLeaseKey) {
    return;
  }
  for (const [activeVmKey, vmRecord] of [...activeVms.entries()]) {
    if (activeVmKey === vmKey || vmRecord.profile_lease_key !== profileLeaseKey) {
      continue;
    }
    if (vmRecord.pages.size > 0) {
      throw new Error("Browser profile is already attached to another active Browser VM; close that page before changing exit node.");
    }
    clearIdleVmShutdown(vmRecord);
    activeVms.delete(activeVmKey);
    logEvent("profile_idle_vm_retired", {
      vm_key_hash: vmKeyHash(activeVmKey),
      next_vm_key_hash: vmKey ? vmKeyHash(vmKey) : null,
      reason: "profile_single_writer_conflict",
    });
    await terminatePersistentLauncher(
      vmRecord.launcher_child,
      Number(config.shutdown_timeout_ms ?? 30000),
    );
  }
}

async function retireNonReusableIdleVmsForSinglePageRuntime(config, activeVms, vmKey) {
  if (Number(config.max_active_pages ?? 1) !== 1) {
    return;
  }
  for (const [activeVmKey, vmRecord] of [...activeVms.entries()]) {
    if (activeVmKey === vmKey || vmRecord.pages.size > 0) {
      continue;
    }
    clearIdleVmShutdown(vmRecord);
    activeVms.delete(activeVmKey);
    logEvent("idle_vm_retired_for_new_profile", {
      vm_key_hash: vmKeyHash(activeVmKey),
      next_vm_key_hash: vmKey ? vmKeyHash(vmKey) : null,
      reason: "single_active_page_non_reusable_profile",
    });
    terminatePersistentLauncher(
      vmRecord.launcher_child,
      Number(config.shutdown_timeout_ms ?? 30000),
    ).then(() => {
      logEvent("idle_vm_retire_complete", {
        vm_key_hash: vmKeyHash(activeVmKey),
        reason: "single_active_page_non_reusable_profile",
      });
    });
  }
}

function guestControlOpenRequest(body) {
  const guestRequest = JSON.parse(JSON.stringify(body));
  guestRequest.schema = "elastos.browser.vm-guest.open/v1";
  guestRequest.launch_request.engine = "selkies_gstreamer";
  return guestRequest;
}

function vmSupervisorResultFromGuest(result, launch, vmRecord) {
  const normalized = JSON.parse(JSON.stringify(result));
  normalized.engine = "chromium_microvm";
  normalized.control_socket_path = vmRecord.control_socket_path;
  normalized.isolated_session = true;
  normalized.isolation = vmRecord.isolation;
  normalized.network_mode = "runtime_net_only";
  normalized.direct_network = false;
  normalized.wallet_injection = false;
  const display = normalized.display_session;
  if (display && typeof display === "object") {
    display.display_backend = "vm_selkies_gstreamer_webrtc";
    display.media_transport = "runtime_relay";
    normalizeDisplayMediaFromOffer(display);
    display.network_mode = "runtime_net_only";
    display.direct_network = false;
  }
  return normalized;
}

function runPersistentProgram(program, args, env, stdin, timeoutMs, signal) {
  return new Promise((resolve, reject) => {
    const child = spawn(program, args || [], {
      env,
      stdio: ["pipe", "pipe", "pipe"],
    });
    let stdout = "";
    let stderr = "";
    let settled = false;
    const timer = setTimeout(() => {
      forceTerminateChild(child);
      settleError(launcherError("Browser VM persistent launcher timed out", stderr));
    }, timeoutMs);
    const abortLaunch = () => {
      forceTerminateChild(child);
      settleError(launcherError("Browser VM persistent launcher canceled", stderr));
    };
    const settleError = (error) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      signal?.removeEventListener?.("abort", abortLaunch);
      reject(error);
    };
    const settleOk = (line) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      signal?.removeEventListener?.("abort", abortLaunch);
      resolve({ stdout: line, stderr, child });
    };
    if (signal?.aborted) {
      abortLaunch();
      return;
    }
    signal?.addEventListener?.("abort", abortLaunch, { once: true });
    child.stdout.on("data", (chunk) => {
      stdout += chunk.toString("utf8");
      const lines = stdout.split(/\r?\n/).filter(Boolean);
      if (lines.length > 0) {
        settleOk(lines[0]);
      }
    });
    child.stderr.on("data", (chunk) => {
      stderr += chunk.toString("utf8");
      if (stderr.length > 64 * 1024) {
        stderr = stderr.slice(-64 * 1024);
      }
    });
    child.on("error", settleError);
    child.on("exit", (code, signal) => {
      if (!settled) {
        settleError(launcherError(
          `Browser VM persistent launcher exited before readiness with ${code ?? signal}`,
          stderr,
        ));
      }
    });
    child.stdin.end(stdin);
  });
}

async function openPageInActiveVm(config, body, launch, vmRecord) {
  const result = await postJsonOverUnix(
    vmRecord.control_socket_path,
    "/pages",
    guestControlOpenRequest(body),
    Number(config.launch_timeout_ms ?? 120000),
  );
  const normalized = vmSupervisorResultFromGuest(result, launch, vmRecord);
  validateSupervisorResult(normalized, launch);
  return normalized;
}

async function openPage(config, body, activePages, activeVms, pendingLaunches, signal) {
  const launch = validateVmOpenRequest(body);
  const vmKey = sameProfileVmKey(body, launch);
  const profileLeaseKey = launchProfileLeaseKey(body, launch);
  if (pendingLaunches.size > 0) {
    const busyStreamId = [...pendingLaunches.values()][0]?.stream_id || "";
    throw new Error(`Browser VM launch already in progress${busyStreamId ? ` for ${busyStreamId}` : ""}`);
  }
  for (const activeRecord of activePages.values()) {
    if (sameLaunchIdentity(activeRecord.launch, launch)) {
      logEvent("launch_reused", {
        page_id: activeRecord.page?.page_id,
        stream_id: launch.stream_id,
        url: launch.url,
      });
      return activeRecord.page;
    }
  }
  const maxActivePages = Number(config.max_active_pages ?? 1);
  if (maxActivePages === 1 && activePages.size > 0) {
    const activeRecord = activePages.values().next().value;
    if (safeId(activeRecord?.page?.page_id)) {
      logEvent("launch_replacing", {
        page_id: activeRecord.page.page_id,
        stream_id: launch.stream_id,
        reason: "single_active_page",
        previous_url: activeRecord.launch?.url || activeRecord.page?.actual_url || "",
        next_url: launch.url,
      });
      await shutdownPage(
        config,
        { page_id: activeRecord.page.page_id },
        activePages,
        activeVms,
        { keep_vm_alive: vmKey && activeRecord.vm_key === vmKey },
      );
    }
  }
  for (const activeRecord of activePages.values()) {
    if (sameVmStream(activeRecord.launch, launch) && safeId(activeRecord.page?.page_id)) {
      logEvent("launch_replacing", {
        page_id: activeRecord.page.page_id,
        stream_id: launch.stream_id,
        reason: "same_stream",
        previous_url: activeRecord.launch?.url || activeRecord.page?.actual_url || "",
        next_url: launch.url,
      });
      await shutdownPage(config, { page_id: activeRecord.page.page_id }, activePages, activeVms);
      break;
    }
  }
  await retireConflictingIdleVmsForProfile(config, activeVms, vmKey, profileLeaseKey);
  await retireNonReusableIdleVmsForSinglePageRuntime(config, activeVms, vmKey);
  if (activePages.size >= maxActivePages) {
    throw new Error(`Browser VM active page capacity reached (${activePages.size}/${maxActivePages}); close a page before launching another page`);
  }
  let activeVm = vmKey ? activeVms.get(vmKey) : null;
  if (
    activeVm?.control_socket_path &&
    activeVm.pages.size === 0 &&
    !idleVmReuseEnabled(config)
  ) {
    clearIdleVmShutdown(activeVm);
    activeVms.delete(vmKey);
    logEvent("idle_vm_reuse_disabled_retired", {
      vm_key_hash: vmKeyHash(vmKey),
      stream_id: launch.stream_id,
    });
    await terminatePersistentLauncher(
      activeVm.launcher_child,
      Number(config.shutdown_timeout_ms ?? 30000),
    );
    activeVm = null;
  }
  if (activeVm?.control_socket_path) {
    clearIdleVmShutdown(activeVm);
    const startedAt = Date.now();
    const requestId = `browser-vm:${crypto.randomBytes(8).toString("hex")}`;
    pendingLaunches.set(requestId, {
      stream_id: launch.stream_id,
      url: launch.url,
      started_at: new Date(startedAt).toISOString(),
      phase: "GUEST_READY",
      principal_id: launch.principal_id || "",
      profile_key: browserProfileDescriptor(body, launch)?.profile_key ?? null,
      exit_id: lifecycleExitId(launch),
      vm_key_hash: vmKey ? lifecycleHash(vmKey) : null,
    });
    logEvent("launch_reused_vm", {
      request_id: requestId,
      stream_id: launch.stream_id,
      url: launch.url,
      active_vm_pages: activeVm.pages.size,
    });
    try {
      const result = await openPageInActiveVm(config, body, launch, activeVm);
      activePages.set(result.page_id, {
        page: result,
        launch,
        vm_key: vmKey,
        launcher_child: null,
        started_at: new Date(startedAt).toISOString(),
      });
      activeVm.pages.add(result.page_id);
      logEvent("launch_ready", {
        request_id: requestId,
        page_id: result.page_id,
        stream_id: launch.stream_id,
        reused_vm: true,
        latency_ms: Date.now() - startedAt,
      });
      return result;
    } catch (error) {
      if (activeVm.pages.size === 0) {
        clearIdleVmShutdown(activeVm);
        activeVms.delete(vmKey);
        logEvent("warm_vm_retired_after_reuse_failure", {
          request_id: requestId,
          stream_id: launch.stream_id,
          vm_key_hash: vmKeyHash(vmKey),
        });
        await terminatePersistentLauncher(
          activeVm.launcher_child,
          Number(config.shutdown_timeout_ms ?? 30000),
        );
      }
      logEvent("launch_failed", {
        request_id: requestId,
        stream_id: launch.stream_id,
        reused_vm: true,
        latency_ms: Date.now() - startedAt,
        error: error instanceof Error ? error.message : String(error),
      });
      throw error;
    } finally {
      pendingLaunches.delete(requestId);
    }
  }
  const request = {
    ...body,
    control_plane: {
      schema: "elastos.browser.vm-control-service.launch/v1",
      request_id: `browser-vm:${crypto.randomBytes(8).toString("hex")}`,
    },
  };
  const serialized = JSON.stringify(request);
  const timeoutMs = Number(config.launch_timeout_ms ?? 120000);
  const startedAt = Date.now();
  const requestId = request.control_plane.request_id;
  let launcher;
  pendingLaunches.set(requestId, {
    stream_id: launch.stream_id,
    url: launch.url,
    started_at: new Date(startedAt).toISOString(),
    phase: "STARTING_VM",
    principal_id: launch.principal_id || "",
    profile_key: browserProfileDescriptor(body, launch)?.profile_key ?? null,
    exit_id: lifecycleExitId(launch),
    vm_key_hash: vmKey ? lifecycleHash(vmKey) : null,
  });
  logEvent("launch_start", {
    request_id: requestId,
    stream_id: launch.stream_id,
    url: launch.url,
    principal_id: launch.principal_id ?? null,
    profile_key: browserProfileDescriptor(body, launch)?.profile_key ?? null,
    profile_disk_path: browserProfileDescriptor(body, launch)?.disk_path ?? null,
  });
  try {
    launcher = config.persistent_launcher === true
      ? await runPersistentProgram(
          config.launcher_program,
          config.launcher_args || [],
          {
            ...process.env,
            [OPEN_REQUEST_ENV]: serialized,
          },
          `${serialized}\n`,
          timeoutMs,
          signal,
        )
      : await runProgram(
          config.launcher_program,
          config.launcher_args || [],
          {
            ...process.env,
            [OPEN_REQUEST_ENV]: serialized,
          },
          `${serialized}\n`,
          timeoutMs,
        );
    const result = JSON.parse(launcher.stdout.trim().split(/\r?\n/).filter(Boolean).at(-1) || "");
    validateSupervisorResult(result, launch);
    activePages.set(result.page_id, {
      page: result,
      launch,
      vm_key: vmKey,
      launcher_child: launcher.child || null,
      started_at: new Date(startedAt).toISOString(),
    });
    if (vmKey && result.control_socket_path) {
      activeVms.set(vmKey, {
        control_socket_path: result.control_socket_path,
        isolation: result.isolation,
        launcher_child: launcher.child || null,
        pages: new Set([result.page_id]),
        profile_lease_key: profileLeaseKey,
        started_at: new Date(startedAt).toISOString(),
        idle_shutdown_timer: null,
        idle_expires_at: null,
      });
    }
    if (launcher.child) {
      launcher.child.once("exit", (code, exitSignal) => {
        if (vmKey) {
          const currentVm = activeVms.get(vmKey);
          if (currentVm?.launcher_child === launcher.child) {
            for (const pageId of currentVm.pages) {
              activePages.delete(pageId);
            }
            activeVms.delete(vmKey);
          }
        } else {
          const current = activePages.get(result.page_id);
          if (current?.launcher_child === launcher.child) {
            activePages.delete(result.page_id);
          }
        }
        logEvent("launcher_exit", {
          request_id: requestId,
          page_id: result.page_id,
          code,
          signal: exitSignal,
        });
      });
    }
    logEvent("launch_ready", {
      request_id: requestId,
      page_id: result.page_id,
      latency_ms: Date.now() - startedAt,
    });
    return result;
  } catch (error) {
    if (launcher?.child) {
      await terminatePersistentLauncher(launcher.child, Number(config.shutdown_timeout_ms ?? 30000));
    }
    logEvent("launch_failed", {
      request_id: requestId,
      stream_id: launch.stream_id,
      latency_ms: Date.now() - startedAt,
      error: error instanceof Error ? error.message : String(error),
    });
    if (error instanceof SyntaxError) {
      throw new Error(`Browser VM launcher output is not JSON: ${error.message}`);
    }
    throw error;
  } finally {
    pendingLaunches.delete(requestId);
  }
}

async function shutdownPage(config, body, activePages, activeVms, options = {}) {
  const pageId = body?.page_id;
  if (!safeId(pageId)) {
    throw new Error("page_id must be a safe identifier");
  }
  const record = activePages.get(pageId);
  activePages.delete(pageId);
  const vmRecord = record?.vm_key ? activeVms.get(record.vm_key) : null;
  if (vmRecord?.control_socket_path) {
    try {
      await postJsonOverUnix(
        vmRecord.control_socket_path,
        `/pages/${encodeURIComponent(pageId)}/close`,
        {},
        Number(config.shutdown_timeout_ms ?? 30000),
      );
    } catch (error) {
      logEvent("page_close_failed", {
        page_id: pageId,
        error: error instanceof Error ? error.message : String(error),
      });
    }
    vmRecord.pages.delete(pageId);
    if (vmRecord.pages.size > 0) {
      return {
        schema: "elastos.browser.vm-engine.shutdown/v1",
        ok: true,
        page_id: pageId,
        isolated_session: true,
      };
    }
    const idleRetained = retainIdleVm(config, record.vm_key, vmRecord, activeVms);
    if ((options.keep_vm_alive === true && idleVmReuseEnabled(config)) || idleRetained) {
      return {
        schema: "elastos.browser.vm-engine.shutdown/v1",
        ok: true,
        page_id: pageId,
        isolated_session: true,
        warm_vm_retained: true,
        idle_keepalive_ms: idleVmKeepaliveMs(config),
      };
    }
    clearIdleVmShutdown(vmRecord);
    activeVms.delete(record.vm_key);
  }
  if (config.shutdown_program) {
    const page = record?.page || null;
    const request = JSON.stringify({
      schema: "elastos.browser.vm-control-service.shutdown/v1",
      page_id: pageId,
      page,
      principal_id: body?.principal_id,
    });
    await runProgram(
      config.shutdown_program,
      config.shutdown_args || [],
      {
        ...process.env,
        ELASTOS_BROWSER_VM_SHUTDOWN_REQUEST: request,
      },
      `${request}\n`,
      Number(config.shutdown_timeout_ms ?? 30000),
    );
  }
  const launcherChild = vmRecord?.launcher_child || record?.launcher_child;
  if (launcherChild) {
    await terminatePersistentLauncher(launcherChild, Number(config.shutdown_timeout_ms ?? 30000));
  }
  return {
    schema: "elastos.browser.vm-engine.shutdown/v1",
    ok: true,
    page_id: pageId,
    isolated_session: true,
  };
}

function activePageGuestControl(activePages, activeVms, pageId) {
  if (!safeId(pageId)) {
    throw new Error("page_id must be a safe identifier");
  }
  const record = activePages.get(pageId);
  if (!record) {
    throw new Error("browser page not found");
  }
  const vmRecord = record.vm_key ? activeVms.get(record.vm_key) : null;
  const controlSocketPath = vmRecord?.control_socket_path || record.page?.control_socket_path || "";
  if (!controlSocketPath) {
    throw new Error("browser page guest control socket is unavailable");
  }
  return { record, controlSocketPath };
}

async function proxyGuestPageRead(config, activePages, activeVms, pageId, op) {
  if (op !== "status" && op !== "diagnostics" && op !== "logs") {
    throw new Error("unsupported browser page read operation");
  }
  const { controlSocketPath } = activePageGuestControl(activePages, activeVms, pageId);
  const guestPath = op === "logs"
    ? "/logs"
    : `/pages/${encodeURIComponent(pageId)}/${op}`;
  return getJsonOverUnix(
    controlSocketPath,
    guestPath,
    Number(config.signal_timeout_ms ?? config.launch_timeout_ms ?? 30000),
  );
}

async function proxyGuestPageInput(config, activePages, activeVms, pageId, body) {
  const { controlSocketPath } = activePageGuestControl(activePages, activeVms, pageId);
  return postJsonOverUnix(
    controlSocketPath,
    `/pages/${encodeURIComponent(pageId)}/input`,
    body,
    Number(config.signal_timeout_ms ?? config.launch_timeout_ms ?? 30000),
  );
}

async function proxyGuestPageWebrtc(config, activePages, activeVms, pageId, body) {
  const { controlSocketPath } = activePageGuestControl(activePages, activeVms, pageId);
  return postJsonOverUnix(
    controlSocketPath,
    `/pages/${encodeURIComponent(pageId)}/webrtc`,
    body,
    Number(config.signal_timeout_ms ?? config.launch_timeout_ms ?? 30000),
  );
}

function terminatePersistentLauncher(child, timeoutMs) {
  return new Promise((resolve) => {
    if (!child || child.exitCode != null || child.signalCode != null) {
      resolve();
      return;
    }
    const timer = setTimeout(() => {
      try {
        child.kill("SIGKILL");
      } catch {}
      resolve();
    }, timeoutMs);
    child.once("exit", () => {
      clearTimeout(timer);
      resolve();
    });
    try {
      child.kill("SIGTERM");
    } catch {
      clearTimeout(timer);
      resolve();
    }
  });
}

function prepareControlSocket(socketPath, replaceExistingSocket) {
  try {
    const stat = fs.lstatSync(socketPath);
    if (!replaceExistingSocket) {
      throw new Error(`control socket already exists: ${socketPath}`);
    }
    if (!stat.isSocket()) {
      throw new Error(`control socket path exists and is not a socket: ${socketPath}`);
    }
    fs.unlinkSync(socketPath);
  } catch (error) {
    if (error?.code !== "ENOENT") throw error;
  }
  fs.mkdirSync(path.dirname(socketPath), { recursive: true, mode: 0o700 });
}

function main() {
  const config = parseConfig();
  const serviceStartedAtMs = Date.now();
  const serviceStartedAt = new Date(serviceStartedAtMs).toISOString();
  const activePages = new Map();
  const activeVms = new Map();
  const pendingLaunches = new Map();
  prepareControlSocket(config.control_socket_path, config.replace_existing_socket === true);
  const server = http.createServer(async (req, res) => {
    let responseStarted = false;
    const sendJson = (status, body) => {
      responseStarted = true;
      jsonResponse(res, status, body);
    };
    try {
      const url = new URL(req.url || "/", "http://browser-vm-control");
      if (req.method === "GET" && url.pathname === "/status") {
        sendJson(200, {
          schema: "elastos.browser.vm-control-service.status/v1",
          ok: true,
          pid: process.pid,
          started_at: serviceStartedAt,
          uptime_ms: Math.max(0, Date.now() - serviceStartedAtMs),
          config_fingerprint: config.config_fingerprint || null,
          active_pages: activePages.size,
          active_vms: activeVms.size,
          warm_vms: [...activeVms.values()].filter((record) => record.pages.size === 0).length,
          max_active_pages: Number(config.max_active_pages ?? 1),
          idle_vm_keepalive_ms: idleVmKeepaliveMs(config),
          reuse_idle_vms: idleVmReuseEnabled(config),
          hibernation_mode: config.hibernation_mode || "off",
          capacity_available: activePages.size < Number(config.max_active_pages ?? 1),
          page_ids: [...activePages.keys()],
          active_stream_ids: [...activePages.values()].map((record) => record.page?.stream_id || record.launch?.stream_id || ""),
          pending_launches: pendingLaunches.size,
          pending_stream_ids: [...pendingLaunches.values()].map((launch) => launch.stream_id),
          lifecycle: lifecycleStatus(config, activePages, activeVms, pendingLaunches),
          network_mode: "runtime_net_only",
          direct_network: false,
        });
        return;
      }
      const pageReadMatch = url.pathname.match(/^\/pages\/([^/]+)\/(status|diagnostics|logs)$/);
      if (req.method === "GET" && pageReadMatch) {
        try {
          sendJson(
            200,
            await proxyGuestPageRead(
              config,
              activePages,
              activeVms,
              decodeURIComponent(pageReadMatch[1]),
              pageReadMatch[2],
            ),
          );
        } catch (error) {
          sendJson(404, { error: error instanceof Error ? error.message : String(error) });
        }
        return;
      }
      const pageInputMatch = url.pathname.match(/^\/pages\/([^/]+)\/input$/);
      if (req.method === "POST" && pageInputMatch) {
        try {
          sendJson(
            200,
            await proxyGuestPageInput(
              config,
              activePages,
              activeVms,
              decodeURIComponent(pageInputMatch[1]),
              await readJsonBody(req, MAX_BROWSER_INPUT_BODY_BYTES),
            ),
          );
        } catch (error) {
          sendJson(404, { error: error instanceof Error ? error.message : String(error) });
        }
        return;
      }
      const pageWebrtcMatch = url.pathname.match(/^\/pages\/([^/]+)\/webrtc$/);
      if (req.method === "POST" && pageWebrtcMatch) {
        try {
          sendJson(
            200,
            await proxyGuestPageWebrtc(
              config,
              activePages,
              activeVms,
              decodeURIComponent(pageWebrtcMatch[1]),
              await readJsonBody(req),
            ),
          );
        } catch (error) {
          sendJson(404, { error: error instanceof Error ? error.message : String(error) });
        }
        return;
      }
      if (req.method === "POST" && url.pathname === "/pages") {
        const abortController = new AbortController();
        res.on("close", () => {
          if (!responseStarted) abortController.abort();
        });
        const body = await readJsonBody(req);
        sendJson(200, await openPage(config, body, activePages, activeVms, pendingLaunches, abortController.signal));
        return;
      }
      if (req.method === "POST" && url.pathname === "/shutdown") {
        const body = await readJsonBody(req);
        sendJson(200, await shutdownPage(config, body, activePages, activeVms));
        return;
      }
      sendJson(404, { error: "not found" });
    } catch (error) {
      sendJson(400, { error: error instanceof Error ? error.message : String(error) });
    }
  });
  server.listen(config.control_socket_path, () => {
    process.stdout.write(`${JSON.stringify({
      schema: "elastos.browser.vm-control-service.ready/v1",
      control_socket_path: config.control_socket_path,
      pid: process.pid,
      started_at: serviceStartedAt,
      config_fingerprint: config.config_fingerprint || null,
      hibernation_mode: config.hibernation_mode || "off",
      network_mode: "runtime_net_only",
      direct_network: false,
    })}\n`);
  });
}

main();
