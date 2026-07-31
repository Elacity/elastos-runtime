#!/usr/bin/env node
import crypto from "node:crypto";
import fs from "node:fs";
import http from "node:http";
import path from "node:path";
import process from "node:process";
import { spawn } from "node:child_process";
import { isDeepStrictEqual } from "node:util";

const CONFIG_ENV = "ELASTOS_BROWSER_VM_CONTROL_SERVICE_CONFIG";
const OPEN_REQUEST_ENV = "ELASTOS_BROWSER_VM_OPEN_REQUEST";
const MAX_BROWSER_FILE_UPLOAD_BYTES = 16 * 1024 * 1024;
const MAX_BROWSER_INPUT_BODY_BYTES =
  Math.ceil((MAX_BROWSER_FILE_UPLOAD_BYTES * 4) / 3) + 64 * 1024;
const MAX_LAUNCH_RECONCILIATIONS = 128;
const MAX_LAUNCH_RECONCILIATION_JOURNAL_BYTES = 256 * 1024;
const LAUNCH_RECONCILIATION_JOURNAL_SCHEMA =
  "elastos.browser.vm-control-service.launch-reconciliations/v1";
const LAUNCH_SETTLEMENT_DID_NOT_ACT = "did_not_act";
const LAUNCH_SETTLEMENT_TERMINAL = "terminal_post_effect_cleanup";
const LAUNCH_SETTLEMENT_PENDING = "cleanup_pending";
const CONTROL_SERVICE_IDENTITY_SCHEMA =
  "elastos.browser.vm-control-service.identity/v1";
const HOST_PROCESS_BINDING_SCHEMA =
  "elastos.browser.host-process-binding/v1";
const ownedLauncherChildren = new Set();

function fail(message) {
  console.error(message);
  process.exit(1);
}

function codedError(code, message) {
  const error = new Error(message);
  error.code = code;
  return error;
}

function trackOwnedLauncherChild(child) {
  ownedLauncherChildren.add(child);
  child.once("exit", () => ownedLauncherChildren.delete(child));
  return child;
}

function newHostProcessOwnershipId() {
  return `process:${crypto.randomBytes(32).toString("hex")}`;
}

function bindOwnedLauncherProcess(child, ownershipId) {
  if (
    !child ||
    !Number.isInteger(child.pid) ||
    child.pid <= 1 ||
    child.pid > 0x7fffffff ||
    !ownedLauncherChildren.has(child) ||
    !/^process:[0-9a-f]{64}$/.test(ownershipId)
  ) {
    throw new Error(
      "Browser VM control service could not bind its exact owned launcher process",
    );
  }
  return {
    schema: HOST_PROCESS_BINDING_SCHEMA,
    ownership_id: ownershipId,
    pid: child.pid,
    stream_bridge_pid: null,
  };
}

function exactOwnedLauncherProcess(binding, record, vmRecord, launcherChild) {
  const ownedBinding =
    vmRecord?.process_binding || record?.process_binding || record?.page?.process;
  if (
    !hostProcessBindingIsSafe(binding?.process) ||
    !isDeepStrictEqual(binding.process, ownedBinding) ||
    !launcherChild ||
    launcherChild.pid !== binding.process.pid
  ) {
    throw new Error(
      "Browser VM cleanup has no exact control-service-owned process handle",
    );
  }
  return launcherChild;
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

function controlServiceIdentityIsSafe(identity, controlSocketPath) {
  return (
    identity?.schema === CONTROL_SERVICE_IDENTITY_SCHEMA &&
    typeof identity.service_id === "string" &&
    /^service:[0-9a-f]{64}$/.test(identity.service_id) &&
    typeof identity.control_socket_path === "string" &&
    identity.control_socket_path.startsWith("/") &&
    !/[\r\n\0]/.test(identity.control_socket_path) &&
    identity.control_socket_path === controlSocketPath &&
    (identity.config_fingerprint === null ||
      /^[0-9a-f]{64}$/.test(identity.config_fingerprint))
  );
}

function hostProcessBindingIsSafe(binding) {
  return (
    binding?.schema === HOST_PROCESS_BINDING_SCHEMA &&
    typeof binding.ownership_id === "string" &&
    /^process:[0-9a-f]{64}$/.test(binding.ownership_id) &&
    Number.isInteger(binding.pid) &&
    binding.pid > 1 &&
    binding.pid <= 0x7fffffff &&
    binding.stream_bridge_pid === null
  );
}

function validateRuntimeCleanupBinding(binding, pageId) {
  if (
    binding?.schema !== "elastos.browser.engine-cleanup-binding/v2" ||
    binding.page_id !== pageId ||
    !safeId(binding.page_id) ||
    !safeId(binding.generation) ||
    !safeId(binding.stream_id) ||
    !safeId(binding.adapter) ||
    typeof binding.engine !== "string" ||
    binding.isolated_session !== true ||
    binding.isolation?.schema !== "elastos.browser.engine.isolation/v1" ||
    binding.isolation?.kind !== "per_launch_vm_target" ||
    !controlServiceIdentityIsSafe(
      binding.control_service,
      binding.shutdown_socket_path,
    ) ||
    !hostProcessBindingIsSafe(binding.process)
  ) {
    throw new Error("invalid Runtime Browser cleanup binding");
  }
  validateAbsolutePath(binding.control_socket_path, "runtime_cleanup.control_socket_path");
  if (binding.shutdown_socket_path) {
    validateAbsolutePath(
      binding.shutdown_socket_path,
      "runtime_cleanup.shutdown_socket_path",
    );
  }
  validateAbsolutePath(
    binding.isolation.session_dir,
    "runtime_cleanup.isolation.session_dir",
  );
  if (Buffer.byteLength(JSON.stringify(binding)) > 16 * 1024) {
    throw new Error("Runtime Browser cleanup binding is too large");
  }
  return binding;
}

function exactCleanupEffects(binding, pageId, activePages, activeVms, childAbsent) {
  const pageAbsent =
    !activePages.has(pageId) &&
    [...activeVms.values()].every((record) => !record.pages.has(pageId));
  const socketAbsent = !fs.existsSync(binding.control_socket_path);
  return {
    page_absent: pageAbsent,
    child_absent: childAbsent,
    vm_absent: pageAbsent && childAbsent,
    route_absent: pageAbsent,
    socket_absent: socketAbsent,
  };
}

function launchIdentityMatchesCleanupBinding(launch, binding) {
  return (
    launch &&
    binding.generation === launch.lifecycle_generation &&
    binding.stream_id === launch.stream_id &&
    binding.adapter === launch.adapter &&
    binding.engine === launch.engine &&
    binding.display_mode === launch.display_mode &&
    binding.guarantee_level === launch.guarantee_level &&
    (binding.principal_id || null) === (launch.principal_id || null)
  );
}

function cleanupBindingForSupervisorResult(
  config,
  controlServiceIdentity,
  launch,
  result,
) {
  const binding = {
    schema: "elastos.browser.engine-cleanup-binding/v2",
    page_id: result.page_id,
    generation: launch.lifecycle_generation,
    stream_id: launch.stream_id,
    adapter: launch.adapter,
    engine: launch.engine,
    display_mode: launch.display_mode,
    guarantee_level: launch.guarantee_level,
    principal_id: launch.principal_id || null,
    control_socket_path: result.control_socket_path,
    shutdown_socket_path: config.control_socket_path,
    isolated_session: true,
    isolation: result.isolation,
    control_service: controlServiceIdentity,
    process: result.process,
  };
  validateRuntimeCleanupBinding(binding, result.page_id);
  return binding;
}

function terminalCleanupReceipt(binding, effects, fields = {}) {
  const unresolved = Object.entries(effects)
    .filter(([, value]) => value !== true)
    .map(([key]) => key);
  if (unresolved.length > 0) {
    throw new Error(
      `Browser VM cleanup remains indeterminate: ${unresolved.join(", ")}`,
    );
  }
  return {
    schema: "elastos.browser.supervisor-cleanup-result/v2",
    page_id: binding.page_id,
    generation: binding.generation,
    binding,
    terminal: true,
    effects,
    ...fields,
  };
}

function requireExactRuntimeCleanupRecord(
  config,
  controlServiceIdentity,
  binding,
  record,
) {
  const launch = record?.launch;
  const page = record?.page;
  const exact =
    launch &&
    page &&
    binding.page_id === page.page_id &&
    binding.stream_id === launch.stream_id &&
    binding.adapter === launch.adapter &&
    binding.engine === launch.engine &&
    binding.display_mode === launch.display_mode &&
    binding.guarantee_level === launch.guarantee_level &&
    (binding.principal_id || "") === (launch.principal_id || "") &&
    binding.control_socket_path === page.control_socket_path &&
    binding.shutdown_socket_path === config.control_socket_path &&
    isDeepStrictEqual(binding.control_service, controlServiceIdentity) &&
    JSON.stringify(binding.isolation) === JSON.stringify(page.isolation) &&
    JSON.stringify(binding.process) === JSON.stringify(page.process);
  if (!exact) {
    throw new Error(
      "Runtime Browser cleanup binding does not match the active VM effect",
    );
  }
}

function requireExactDurableCleanupRecord(store, binding) {
  const record = store.records.get(
    launchReconciliationKey(binding.generation, binding.stream_id),
  );
  if (
    !record ||
    !record.cleanup_binding ||
    !record.control_service ||
    !isDeepStrictEqual(record.control_service, store.control_service) ||
    !isDeepStrictEqual(binding.control_service, store.control_service) ||
    !launchIdentityMatchesCleanupBinding(record.launch, binding) ||
    !isDeepStrictEqual(record.cleanup_binding, binding) ||
    ![
      "cleanup_pending",
      "effect_acquired",
      "terminal_post_effect_cleanup",
    ].includes(record.state)
  ) {
    throw new Error(
      "Runtime Browser cleanup binding does not match an exact durable VM effect",
    );
  }
  return record;
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
  if (config.control_service_identity_path !== undefined) {
    validateAbsolutePath(
      config.control_service_identity_path,
      "control_service_identity_path",
    );
  }
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

function controlServiceIdentityPath(config) {
  return (
    config.control_service_identity_path ||
    `${config.control_socket_path}.identity.json`
  );
}

function loadOrCreateControlServiceIdentity(config) {
  const identityPath = controlServiceIdentityPath(config);
  let persisted = null;
  try {
    const stat = fs.lstatSync(identityPath);
    requireOwnerOnlyRegularFile(
      identityPath,
      stat,
      "Browser VM control service identity",
    );
    persisted = JSON.parse(fs.readFileSync(identityPath, "utf8"));
  } catch (error) {
    if (error?.code !== "ENOENT") throw error;
  }
  if (persisted !== null) {
    if (
      persisted?.schema !== CONTROL_SERVICE_IDENTITY_SCHEMA ||
      !/^service:[0-9a-f]{64}$/.test(persisted.service_id || "") ||
      persisted.control_socket_path !== config.control_socket_path
    ) {
      throw new Error("Browser VM control service identity is invalid");
    }
  } else {
    persisted = {
      schema: CONTROL_SERVICE_IDENTITY_SCHEMA,
      service_id: `service:${crypto.randomBytes(32).toString("hex")}`,
      control_socket_path: config.control_socket_path,
    };
    fs.mkdirSync(path.dirname(identityPath), {
      recursive: true,
      mode: 0o700,
    });
    const temporaryPath = `${identityPath}.tmp.${process.pid}.${crypto
      .randomBytes(8)
      .toString("hex")}`;
    let fd;
    try {
      fd = fs.openSync(temporaryPath, "wx", 0o600);
      fs.writeFileSync(fd, JSON.stringify(persisted));
      fs.fchmodSync(fd, 0o600);
      fs.fsyncSync(fd);
      fs.closeSync(fd);
      fd = undefined;
      fs.renameSync(temporaryPath, identityPath);
    } finally {
      if (fd !== undefined) fs.closeSync(fd);
      try {
        fs.unlinkSync(temporaryPath);
      } catch (error) {
        if (error?.code !== "ENOENT") throw error;
      }
    }
  }
  return {
    ...persisted,
    config_fingerprint: config.config_fingerprint || null,
  };
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
  if (
    !safeId(launch.adapter) ||
    !safeId(launch.stream_id) ||
    !safeId(launch.lifecycle_generation)
  ) {
    throw new Error(
      "launch request adapter, stream_id, and lifecycle_generation must be safe identifiers",
    );
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

function launchReconciliationKey(generation, streamId) {
  return `${generation}\n${streamId}`;
}

function launchReconciliationIdentity(launch) {
  return {
    adapter: launch.adapter,
    engine: launch.engine,
    lifecycle_generation: launch.lifecycle_generation,
    stream_id: launch.stream_id,
    principal_id: launch.principal_id || null,
    display_mode: launch.display_mode,
    guarantee_level: launch.guarantee_level,
  };
}

function launchReconciliationJournalPath(config) {
  return `${config.control_socket_path}.launch-reconciliations.json`;
}

function durableCleanupBindingIsSafe(binding, launch) {
  if (binding === undefined) return true;
  try {
    validateRuntimeCleanupBinding(binding, binding?.page_id);
  } catch {
    return false;
  }
  return (
    safeId(binding.page_id) &&
    binding.page_id.length <= 256 &&
    launchIdentityMatchesCleanupBinding(launch, binding)
  );
}

function launchReconciliationRecordIsSafe(record) {
  const launch = record?.launch;
  const effects = record?.effects;
  const state = record?.state;
  const stateIsSafe = [
    "did_not_act",
    "cleanup_pending",
    "effect_acquired",
    "terminal_post_effect_cleanup",
  ].includes(state);
  const effectsAreSafe =
    effects &&
    typeof effects === "object" &&
    (effects.page_acquired === null ||
      typeof effects.page_acquired === "boolean") &&
    (effects.vm_acquired === null ||
      typeof effects.vm_acquired === "boolean") &&
    (effects.page_acquired !== null ||
      state === "cleanup_pending" ||
      state === "effect_acquired") &&
    (effects.vm_acquired !== null ||
      state === "cleanup_pending" ||
      state === "effect_acquired");
  return (
    record?.schema ===
      "elastos.browser.vm-control-service.launch-reconciliation/v1" &&
    stateIsSafe &&
    launch &&
    safeId(launch.adapter) &&
    launch.adapter.length <= 128 &&
    launch.engine === "chromium_microvm" &&
    safeId(launch.lifecycle_generation) &&
    launch.lifecycle_generation.length <= 256 &&
    safeId(launch.stream_id) &&
    launch.stream_id.length <= 256 &&
    (launch.principal_id === null ||
      (safeId(launch.principal_id) && launch.principal_id.length <= 512)) &&
    launch.display_mode === "webrtc_remote_display" &&
    launch.guarantee_level === "mechanism_microvm" &&
    (record.control_service === undefined ||
      controlServiceIdentityIsSafe(
        record.control_service,
        record.control_service?.control_socket_path,
      )) &&
    typeof record.updated_at === "string" &&
    record.updated_at.length <= 64 &&
    Number.isFinite(Date.parse(record.updated_at)) &&
    effectsAreSafe &&
    durableCleanupBindingIsSafe(record.cleanup_binding, launch) &&
    (state !== "did_not_act" ||
      (effects.page_acquired === false && effects.vm_acquired === false)) &&
    (state !== "terminal_post_effect_cleanup" ||
      (typeof effects.page_acquired === "boolean" &&
        typeof effects.vm_acquired === "boolean"))
  );
}

function durableLaunchReconciliationRecord(record) {
  const durable = {
    schema: record.schema,
    state: record.state,
    launch: record.launch,
    updated_at: record.updated_at,
    effects: record.effects,
  };
  if (record.cleanup_binding !== undefined) {
    durable.cleanup_binding = record.cleanup_binding;
  }
  if (record.control_service !== undefined) {
    durable.control_service = record.control_service;
  }
  return durable;
}

function requireOwnerOnlyRegularFile(filePath, stat, label) {
  if (!stat.isFile()) {
    throw new Error(`${label} is not a regular file: ${filePath}`);
  }
  if (
    typeof process.getuid === "function" &&
    stat.uid !== process.getuid()
  ) {
    throw new Error(`${label} is not owned by the current user: ${filePath}`);
  }
  if ((stat.mode & 0o077) !== 0) {
    throw new Error(`${label} is not owner-only: ${filePath}`);
  }
}

function loadLaunchReconciliations(journalPath) {
  let stat;
  try {
    stat = fs.lstatSync(journalPath);
  } catch (error) {
    if (error?.code === "ENOENT") return new Map();
    throw error;
  }
  requireOwnerOnlyRegularFile(
    journalPath,
    stat,
    "Browser VM launch reconciliation journal",
  );
  if (stat.size > MAX_LAUNCH_RECONCILIATION_JOURNAL_BYTES) {
    throw new Error("Browser VM launch reconciliation journal is too large");
  }
  const parsed = JSON.parse(fs.readFileSync(journalPath, "utf8"));
  if (
    parsed?.schema !== LAUNCH_RECONCILIATION_JOURNAL_SCHEMA ||
    !Array.isArray(parsed.records) ||
    parsed.records.length > MAX_LAUNCH_RECONCILIATIONS ||
    parsed.records.some((record) => !launchReconciliationRecordIsSafe(record))
  ) {
    throw new Error("Browser VM launch reconciliation journal is invalid");
  }
  const records = new Map();
  for (const persisted of parsed.records) {
    const record =
      persisted.state === "effect_acquired"
        ? {
            ...persisted,
            state: "cleanup_pending",
            effects: { page_acquired: null, vm_acquired: null },
          }
        : persisted;
    records.set(
      launchReconciliationKey(
        record.launch.lifecycle_generation,
        record.launch.stream_id,
      ),
      record,
    );
  }
  return records;
}

function persistLaunchReconciliations(store) {
  const records = [...store.records.values()].map(
    durableLaunchReconciliationRecord,
  );
  if (records.some((record) => !launchReconciliationRecordIsSafe(record))) {
    throw new Error("Browser VM launch reconciliation state is invalid");
  }
  const bytes = Buffer.from(
    JSON.stringify({
      schema: LAUNCH_RECONCILIATION_JOURNAL_SCHEMA,
      records,
    }),
  );
  if (bytes.length > MAX_LAUNCH_RECONCILIATION_JOURNAL_BYTES) {
    throw new Error("Browser VM launch reconciliation journal is too large");
  }
  try {
    const current = fs.lstatSync(store.journal_path);
    requireOwnerOnlyRegularFile(
      store.journal_path,
      current,
      "Browser VM launch reconciliation journal",
    );
  } catch (error) {
    if (error?.code !== "ENOENT") throw error;
  }
  const temporaryPath = `${store.journal_path}.tmp.${process.pid}.${crypto
    .randomBytes(8)
    .toString("hex")}`;
  let fd;
  try {
    fd = fs.openSync(temporaryPath, "wx", 0o600);
    fs.writeFileSync(fd, bytes);
    fs.fchmodSync(fd, 0o600);
    fs.fsyncSync(fd);
    fs.closeSync(fd);
    fd = undefined;
    fs.renameSync(temporaryPath, store.journal_path);
  } finally {
    if (fd !== undefined) fs.closeSync(fd);
    try {
      fs.unlinkSync(temporaryPath);
    } catch (error) {
      if (error?.code !== "ENOENT") throw error;
    }
  }
}

function launchReconciliationStore(config, controlServiceIdentity) {
  const journalPath = launchReconciliationJournalPath(config);
  return {
    journal_path: journalPath,
    records: loadLaunchReconciliations(journalPath),
    control_service: controlServiceIdentity,
  };
}

function recordLaunchReconciliation(
  launchReconciliationStore,
  launch,
  state,
  fields = {},
) {
  const launchReconciliations = launchReconciliationStore.records;
  const previous = new Map(launchReconciliations);
  const key = launchReconciliationKey(
    launch.lifecycle_generation,
    launch.stream_id,
  );
  const current = launchReconciliations.get(key);
  if (!current && launchReconciliations.size >= MAX_LAUNCH_RECONCILIATIONS) {
    const evictableKey = [...launchReconciliations].find(([, record]) =>
      [
        LAUNCH_SETTLEMENT_DID_NOT_ACT,
        LAUNCH_SETTLEMENT_TERMINAL,
      ].includes(record.state),
    )?.[0];
    if (!evictableKey) {
      throw codedError(
        "reconciliation_capacity_exhausted",
        `Browser VM launch reconciliation capacity is exhausted by ${MAX_LAUNCH_RECONCILIATIONS} unresolved effects`,
      );
    }
    launchReconciliations.delete(evictableKey);
  }
  launchReconciliations.delete(key);
  const next = {
    schema: "elastos.browser.vm-control-service.launch-reconciliation/v1",
    state,
    launch: launchReconciliationIdentity(launch),
    control_service: launchReconciliationStore.control_service,
    updated_at: new Date().toISOString(),
    ...fields,
  };
  if (
    next.cleanup_binding === undefined &&
    current?.cleanup_binding !== undefined
  ) {
    next.cleanup_binding = current.cleanup_binding;
  }
  launchReconciliations.set(key, next);
  try {
    persistLaunchReconciliations(launchReconciliationStore);
  } catch (error) {
    launchReconciliations.clear();
    for (const [previousKey, previousRecord] of previous) {
      launchReconciliations.set(previousKey, previousRecord);
    }
    throw error;
  }
}

function reconcileLaunch(launchReconciliationStore, activePages, body) {
  const launchReconciliations = launchReconciliationStore.records;
  if (
    body?.schema !== "elastos.browser.vm-control-service.reconcile-launch/v1" ||
    !safeId(body.lifecycle_generation) ||
    !safeId(body.stream_id)
  ) {
    throw new Error(
      "Browser VM launch reconciliation requires safe lifecycle_generation and stream_id",
    );
  }
  const exactActivePages = [...activePages.values()].filter(
    (record) =>
      record?.launch?.lifecycle_generation === body.lifecycle_generation &&
      record?.launch?.stream_id === body.stream_id,
  );
  if (exactActivePages.length === 1) {
    const record = exactActivePages[0];
    if (record.cleanup_pending !== true) {
      recordLaunchReconciliation(
        launchReconciliationStore,
        record.launch,
        "effect_acquired",
        {
          effects: { page_acquired: true, vm_acquired: true },
          supervisor_result: record.page,
        },
      );
    }
  } else if (
    exactActivePages.length > 1 ||
    [...activePages.values()].some(
      (record) =>
        record?.launch?.lifecycle_generation === body.lifecycle_generation ||
        record?.launch?.stream_id === body.stream_id,
    )
  ) {
    return {
      schema: "elastos.browser.vm-control-service.launch-reconciliation/v1",
      state: "cleanup_pending",
      responder_control_service: launchReconciliationStore.control_service,
      launch: {
        lifecycle_generation: body.lifecycle_generation,
        stream_id: body.stream_id,
      },
    };
  }
  const key = launchReconciliationKey(
    body.lifecycle_generation,
    body.stream_id,
  );
  const record = launchReconciliations.get(key);
  if (!record) {
    return {
      schema: "elastos.browser.vm-control-service.launch-reconciliation/v1",
      state: "indeterminate",
      responder_control_service: launchReconciliationStore.control_service,
      launch: {
        lifecycle_generation: body.lifecycle_generation,
        stream_id: body.stream_id,
      },
    };
  }
  return {
    ...record,
    responder_control_service: launchReconciliationStore.control_service,
  };
}

function markLaunchReconciliationTerminal(
  launchReconciliationStore,
  generation,
  streamId,
  effects,
) {
  const launchReconciliations = launchReconciliationStore.records;
  const record = launchReconciliations.get(
    launchReconciliationKey(generation, streamId),
  );
  if (record?.launch) {
    recordLaunchReconciliation(
      launchReconciliationStore,
      record.launch,
      "terminal_post_effect_cleanup",
      {
        effects,
      },
    );
  }
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
  if (
    !controlServiceIdentityIsSafe(
      result.control_service,
      result.control_service?.control_socket_path,
    ) ||
    !hostProcessBindingIsSafe(result.process)
  ) {
    throw new Error(
      "launcher result lacks an exact control-service-owned host process binding",
    );
  }
  try {
    validateAbsolutePath(result.isolation?.session_dir, "launcher isolation session_dir");
  } catch {
    throw new Error("launcher returned invalid Browser VM session directory");
  }
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

function launchSettlementError(error, settlement, cleanupError = null) {
  const settledError =
    error instanceof Error ? error : new Error(String(error));
  settledError.launch_settlement = settlement;
  if (cleanupError) {
    settledError.launch_cleanup_error =
      cleanupError instanceof Error ? cleanupError.message : String(cleanupError);
  }
  return settledError;
}

function launcherFailureWithDetail(message, stderr) {
  const detail = String(stderr || "").trim();
  if (!detail) return new Error(message);
  const bounded = detail.length > 8192 ? detail.slice(-8192) : detail;
  return new Error(`${message}: ${bounded}`);
}

function runProgram(program, args, env, stdin, timeoutMs, signal) {
  return new Promise((resolve, reject) => {
    let child;
    try {
      child = trackOwnedLauncherChild(spawn(program, args || [], {
        env,
        stdio: ["pipe", "pipe", "pipe"],
      }));
    } catch (error) {
      reject(
        launchSettlementError(error, LAUNCH_SETTLEMENT_DID_NOT_ACT),
      );
      return;
    }
    let stdout = "";
    let stderr = "";
    let phase = "running";
    let timer;
    const clearSettlementTriggers = () => {
      clearTimeout(timer);
      signal?.removeEventListener?.("abort", abortLaunch);
    };
    const settleError = (error) => {
      if (phase === "settled") return;
      phase = "settled";
      clearSettlementTriggers();
      reject(error);
    };
    const settleOk = (result) => {
      if (phase !== "running") return;
      phase = "settled";
      clearSettlementTriggers();
      resolve(result);
    };
    const terminateFor = (error) => {
      if (phase !== "running") return;
      phase = "terminating";
      clearSettlementTriggers();
      void terminatePersistentLauncher(child, 5000).then(
        () =>
          settleError(
            launchSettlementError(error, LAUNCH_SETTLEMENT_TERMINAL),
          ),
        (cleanupError) =>
          settleError(
            launchSettlementError(
              error,
              LAUNCH_SETTLEMENT_PENDING,
              cleanupError,
            ),
          ),
      );
    };
    const abortLaunch = () => {
      terminateFor(
        launcherFailureWithDetail("Browser VM launcher canceled", stderr),
      );
    };
    timer = setTimeout(() => {
      terminateFor(
        launcherFailureWithDetail("Browser VM launcher timed out", stderr),
      );
    }, timeoutMs);
    signal?.addEventListener?.("abort", abortLaunch, { once: true });
    child.stdout.on("data", (chunk) => {
      stdout += chunk.toString("utf8");
    });
    child.stderr.on("data", (chunk) => {
      stderr += chunk.toString("utf8");
      if (stderr.length > 64 * 1024) {
        stderr = stderr.slice(-64 * 1024);
      }
    });
    child.on("error", (error) => {
      if (phase !== "running") return;
      if (!Number.isInteger(child.pid) || child.pid <= 1) {
        settleError(
          launchSettlementError(error, LAUNCH_SETTLEMENT_DID_NOT_ACT),
        );
        return;
      }
      terminateFor(error);
    });
    child.on("exit", (code, signal) => {
      if (phase !== "running") return;
      if (code !== 0) {
        settleError(
          launchSettlementError(
            launcherError(
              `Browser VM launcher exited with ${code ?? signal}`,
              stderr,
            ),
            LAUNCH_SETTLEMENT_TERMINAL,
          ),
        );
        return;
      }
      settleOk({ stdout, stderr, child: null, owner_reaped: true });
    });
    child.stdin.on("error", () => {});
    try {
      child.stdin.end(stdin);
    } catch (error) {
      if (!Number.isInteger(child.pid) || child.pid <= 1) {
        settleError(
          launchSettlementError(error, LAUNCH_SETTLEMENT_DID_NOT_ACT),
        );
      } else {
        terminateFor(error);
      }
    }
    if (signal?.aborted) abortLaunch();
  });
}

function launcherError(message, stderr) {
  const detail = String(stderr || "").trim();
  if (!detail) return new Error(message);
  const bounded = detail.length > 8192 ? detail.slice(-8192) : detail;
  for (const line of bounded.split(/\r?\n/).reverse()) {
    try {
      const parsed = JSON.parse(line);
      if (
        parsed?.schema === "elastos.browser.engine.launch-error/v1" &&
        typeof parsed.code === "string" &&
        typeof parsed.message === "string"
      ) {
        return codedError(parsed.code, parsed.message);
      }
    } catch {}
  }
  return new Error(`${message}: ${bounded}`);
}

function requestJsonOverUnix(
  socketPath,
  method,
  requestPath,
  body,
  timeoutMs,
  signal,
) {
  validateAbsolutePath(socketPath, "Browser VM guest control socket");
  const bytes = body == null ? Buffer.alloc(0) : Buffer.from(JSON.stringify(body));
  return new Promise((resolve, reject) => {
    let req;
    const clearAbort = () =>
      signal?.removeEventListener?.("abort", abortRequest);
    const abortRequest = () => {
      req?.destroy(new Error(`Browser VM guest control ${method} ${requestPath} canceled`));
    };
    req = http.request(
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
          clearAbort();
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
    req.on("error", (error) => {
      clearAbort();
      reject(error);
    });
    signal?.addEventListener?.("abort", abortRequest, { once: true });
    req.end(bytes);
    if (signal?.aborted) abortRequest();
  });
}

function postJsonOverUnix(socketPath, requestPath, body, timeoutMs, signal) {
  return requestJsonOverUnix(
    socketPath,
    "POST",
    requestPath,
    body,
    timeoutMs,
    signal,
  );
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
  normalized.control_service = vmRecord.control_service;
  normalized.process = vmRecord.process_binding;
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
    let child;
    try {
      child = trackOwnedLauncherChild(spawn(program, args || [], {
        env,
        stdio: ["pipe", "pipe", "pipe"],
      }));
    } catch (error) {
      reject(
        launchSettlementError(error, LAUNCH_SETTLEMENT_DID_NOT_ACT),
      );
      return;
    }
    let stdout = "";
    let stderr = "";
    let phase = "running";
    let timer;
    const clearSettlementTriggers = () => {
      clearTimeout(timer);
      signal?.removeEventListener?.("abort", abortLaunch);
    };
    const settleError = (error) => {
      if (phase === "settled") return;
      phase = "settled";
      clearSettlementTriggers();
      reject(error);
    };
    const settleOk = (line) => {
      if (phase !== "running") return;
      phase = "settled";
      clearSettlementTriggers();
      resolve({ stdout: line, stderr, child, owner_reaped: false });
    };
    const terminateFor = (error) => {
      if (phase !== "running") return;
      phase = "terminating";
      clearSettlementTriggers();
      void terminatePersistentLauncher(child, 5000).then(
        () =>
          settleError(
            launchSettlementError(error, LAUNCH_SETTLEMENT_TERMINAL),
          ),
        (cleanupError) =>
          settleError(
            launchSettlementError(
              error,
              LAUNCH_SETTLEMENT_PENDING,
              cleanupError,
            ),
          ),
      );
    };
    const abortLaunch = () => {
      terminateFor(
        launcherFailureWithDetail(
          "Browser VM persistent launcher canceled",
          stderr,
        ),
      );
    };
    timer = setTimeout(() => {
      terminateFor(
        launcherFailureWithDetail(
          "Browser VM persistent launcher timed out",
          stderr,
        ),
      );
    }, timeoutMs);
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
    child.on("error", (error) => {
      if (phase !== "running") return;
      if (!Number.isInteger(child.pid) || child.pid <= 1) {
        settleError(
          launchSettlementError(error, LAUNCH_SETTLEMENT_DID_NOT_ACT),
        );
        return;
      }
      terminateFor(error);
    });
    child.on("exit", (code, signal) => {
      if (phase !== "running") return;
      settleError(
        launchSettlementError(
          launcherError(
            `Browser VM persistent launcher exited before readiness with ${
              code ?? signal
            }`,
            stderr,
          ),
          LAUNCH_SETTLEMENT_TERMINAL,
        ),
      );
    });
    child.stdin.on("error", () => {});
    try {
      child.stdin.end(stdin);
    } catch (error) {
      if (!Number.isInteger(child.pid) || child.pid <= 1) {
        settleError(
          launchSettlementError(error, LAUNCH_SETTLEMENT_DID_NOT_ACT),
        );
      } else {
        terminateFor(error);
      }
    }
    if (signal?.aborted) abortLaunch();
  });
}

async function openPageInActiveVm(config, body, launch, vmRecord, signal) {
  const result = await postJsonOverUnix(
    vmRecord.control_socket_path,
    "/pages",
    guestControlOpenRequest(body),
    Number(config.launch_timeout_ms ?? 120000),
    signal,
  );
  const normalized = vmSupervisorResultFromGuest(result, launch, vmRecord);
  validateSupervisorResult(normalized, launch);
  return normalized;
}

function retainLaunchReconciliationPendingInMemory(
  launchReconciliationStore,
  launch,
) {
  const key = launchReconciliationKey(
    launch.lifecycle_generation,
    launch.stream_id,
  );
  const current = launchReconciliationStore.records.get(key);
  launchReconciliationStore.records.set(key, {
    ...current,
    schema: "elastos.browser.vm-control-service.launch-reconciliation/v1",
    state: "cleanup_pending",
    launch: current?.launch || launchReconciliationIdentity(launch),
    control_service:
      current?.control_service || launchReconciliationStore.control_service,
    updated_at: new Date().toISOString(),
    effects: { page_acquired: null, vm_acquired: null },
  });
}

function recordProvenLaunchFailure(
  launchReconciliationStore,
  launch,
  error,
) {
  const settlement = error?.launch_settlement;
  if (
    settlement !== LAUNCH_SETTLEMENT_DID_NOT_ACT &&
    settlement !== LAUNCH_SETTLEMENT_TERMINAL
  ) {
    return;
  }
  try {
    recordLaunchReconciliation(
      launchReconciliationStore,
      launch,
      settlement,
      {
        effects:
          settlement === LAUNCH_SETTLEMENT_DID_NOT_ACT
            ? { page_acquired: false, vm_acquired: false }
            : { page_acquired: true, vm_acquired: true },
      },
    );
  } catch (journalError) {
    retainLaunchReconciliationPendingInMemory(
      launchReconciliationStore,
      launch,
    );
    logEvent("launch_reconciliation_persist_failed", {
      stream_id: launch.stream_id,
      intended_state: settlement,
      error:
        journalError instanceof Error
          ? journalError.message
          : String(journalError),
    });
  }
}

function markVmPendingLaunchReconciliationsTerminal(
  launchReconciliationStore,
  vmRecord,
) {
  const pending = vmRecord?.pending_reconciliations;
  if (!(pending instanceof Map)) return;
  for (const [key, launch] of pending) {
    recordProvenLaunchFailure(
      launchReconciliationStore,
      launch,
      launchSettlementError(
        new Error("Browser VM retired after a failed guest page open"),
        LAUNCH_SETTLEMENT_TERMINAL,
      ),
    );
    const record = launchReconciliationStore.records.get(key);
    if (record?.state === "terminal_post_effect_cleanup") {
      pending.delete(key);
    }
  }
}

async function settleDispatchedLaunchFailure(
  launcher,
  error,
  shutdownTimeoutMs,
) {
  if (
    error?.launch_settlement === LAUNCH_SETTLEMENT_DID_NOT_ACT ||
    error?.launch_settlement === LAUNCH_SETTLEMENT_TERMINAL ||
    error?.launch_settlement === LAUNCH_SETTLEMENT_PENDING
  ) {
    return error;
  }
  if (launcher?.child) {
    try {
      await terminatePersistentLauncher(launcher.child, shutdownTimeoutMs);
      return launchSettlementError(error, LAUNCH_SETTLEMENT_TERMINAL);
    } catch (cleanupError) {
      return launchSettlementError(
        error,
        LAUNCH_SETTLEMENT_PENDING,
        cleanupError,
      );
    }
  }
  if (launcher?.owner_reaped === true) {
    return launchSettlementError(error, LAUNCH_SETTLEMENT_TERMINAL);
  }
  return launchSettlementError(error, LAUNCH_SETTLEMENT_PENDING);
}

async function openPage(
  config,
  controlServiceIdentity,
  body,
  activePages,
  activeVms,
  pendingLaunches,
  launchReconciliations,
  signal,
) {
  const launch = validateVmOpenRequest(body);
  if (
    [...launchReconciliations.records.values()].some(
      (record) =>
        record?.launch?.lifecycle_generation ===
          launch.lifecycle_generation ||
        record?.launch?.stream_id === launch.stream_id,
    )
  ) {
    throw new Error(
      "Browser VM lifecycle generation or stream identity already exists",
    );
  }
  recordLaunchReconciliation(
    launchReconciliations,
    launch,
    "did_not_act",
    { effects: { page_acquired: false, vm_acquired: false } },
  );
  const vmKey = sameProfileVmKey(body, launch);
  const profileLeaseKey = launchProfileLeaseKey(body, launch);
  if (pendingLaunches.size > 0) {
    const busyStreamId = [...pendingLaunches.values()][0]?.stream_id || "";
    throw new Error(`Browser VM launch already in progress${busyStreamId ? ` for ${busyStreamId}` : ""}`);
  }
  const maxActivePages = Number(config.max_active_pages ?? 1);
  if (activePages.size >= maxActivePages) {
    throw new Error(`Browser VM active page capacity reached (${activePages.size}/${maxActivePages}); close a page before launching another page`);
  }
  await retireConflictingIdleVmsForProfile(config, activeVms, vmKey, profileLeaseKey);
  await retireNonReusableIdleVmsForSinglePageRuntime(config, activeVms, vmKey);
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
    let reuseMayHaveActed = false;
    try {
      if (signal?.aborted) {
        throw new Error("Browser VM launch canceled before dispatch");
      }
      recordLaunchReconciliation(
        launchReconciliations,
        launch,
        "cleanup_pending",
        { effects: { page_acquired: null, vm_acquired: null } },
      );
      reuseMayHaveActed = true;
      const result = await openPageInActiveVm(
        config,
        body,
        launch,
        activeVm,
        signal,
      );
      recordLaunchReconciliation(
        launchReconciliations,
        launch,
        "effect_acquired",
        {
          effects: { page_acquired: true, vm_acquired: true },
          cleanup_binding: cleanupBindingForSupervisorResult(
            config,
            controlServiceIdentity,
            launch,
            result,
          ),
          supervisor_result: result,
        },
      );
      activePages.set(result.page_id, {
        page: result,
        launch,
        vm_key: vmKey,
        launcher_child: null,
        process_binding: activeVm.process_binding,
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
      let cleanupProved = false;
      let cleanupError = null;
      if (
        reuseMayHaveActed &&
        activeVm.pages.size === 0 &&
        activeVm.launcher_child
      ) {
        clearIdleVmShutdown(activeVm);
        logEvent("warm_vm_retired_after_reuse_failure", {
          request_id: requestId,
          stream_id: launch.stream_id,
          vm_key_hash: vmKeyHash(vmKey),
        });
        try {
          await terminatePersistentLauncher(
            activeVm.launcher_child,
            Number(config.shutdown_timeout_ms ?? 30000),
          );
          if (activeVms.get(vmKey) === activeVm) {
            activeVms.delete(vmKey);
          }
          cleanupProved = true;
        } catch (terminateError) {
          cleanupError =
            terminateError instanceof Error
              ? terminateError.message
              : String(terminateError);
        }
      }
      if (cleanupProved) {
        recordProvenLaunchFailure(
          launchReconciliations,
          launch,
          launchSettlementError(error, LAUNCH_SETTLEMENT_TERMINAL),
        );
      } else if (reuseMayHaveActed) {
        if (!(activeVm.pending_reconciliations instanceof Map)) {
          activeVm.pending_reconciliations = new Map();
        }
        activeVm.pending_reconciliations.set(
          launchReconciliationKey(
            launch.lifecycle_generation,
            launch.stream_id,
          ),
          launch,
        );
        if (
          activeVm.launcher_child &&
          (activeVm.launcher_child.exitCode != null ||
            activeVm.launcher_child.signalCode != null)
        ) {
          markVmPendingLaunchReconciliationsTerminal(
            launchReconciliations,
            activeVm,
          );
        }
      }
      logEvent("launch_failed", {
        request_id: requestId,
        stream_id: launch.stream_id,
        reused_vm: true,
        latency_ms: Date.now() - startedAt,
        error: error instanceof Error ? error.message : String(error),
        settlement: reuseMayHaveActed
          ? cleanupProved
            ? "terminal_post_effect_cleanup"
            : "cleanup_pending"
          : "did_not_act",
        cleanup_proved: cleanupProved,
        ...(cleanupError ? { cleanup_error: cleanupError } : {}),
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
  const processOwnershipId = newHostProcessOwnershipId();
  const serialized = JSON.stringify(request);
  const timeoutMs = Number(config.launch_timeout_ms ?? 120000);
  const startedAt = Date.now();
  const requestId = request.control_plane.request_id;
  let launcher;
  let launchMayHaveActed = false;
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
    if (signal?.aborted) {
      throw new Error("Browser VM launch canceled before dispatch");
    }
    recordLaunchReconciliation(
      launchReconciliations,
      launch,
      "cleanup_pending",
      { effects: { page_acquired: null, vm_acquired: null } },
    );
    launchMayHaveActed = true;
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
          signal,
        );
    const result = JSON.parse(launcher.stdout.trim().split(/\r?\n/).filter(Boolean).at(-1) || "");
    result.control_service = controlServiceIdentity;
    result.process = bindOwnedLauncherProcess(
      launcher.child,
      processOwnershipId,
    );
    validateSupervisorResult(result, launch);
    recordLaunchReconciliation(
      launchReconciliations,
      launch,
      "effect_acquired",
      {
        effects: { page_acquired: true, vm_acquired: true },
        cleanup_binding: cleanupBindingForSupervisorResult(
          config,
          controlServiceIdentity,
          launch,
          result,
        ),
        supervisor_result: result,
      },
    );
    activePages.set(result.page_id, {
      page: result,
      launch,
      vm_key: vmKey,
      launcher_child: launcher.child || null,
      process_binding: result.process,
      started_at: new Date(startedAt).toISOString(),
    });
    if (vmKey && result.control_socket_path) {
      activeVms.set(vmKey, {
        control_socket_path: result.control_socket_path,
        isolation: result.isolation,
        launcher_child: launcher.child || null,
        control_service: controlServiceIdentity,
        process_binding: result.process,
        pages: new Set([result.page_id]),
        pending_reconciliations: new Map(),
        profile_lease_key: profileLeaseKey,
        started_at: new Date(startedAt).toISOString(),
        idle_shutdown_timer: null,
        idle_expires_at: null,
      });
    }
    if (launcher.child) {
      let launcherExitHandled = false;
      const handleLauncherExit = (code, exitSignal) => {
        if (launcherExitHandled) return;
        launcherExitHandled = true;
        const currentVm = vmKey ? activeVms.get(vmKey) : null;
        const affectedPages =
          currentVm?.launcher_child === launcher.child
            ? [...currentVm.pages]
                .map((pageId) => activePages.get(pageId))
                .filter(Boolean)
            : [activePages.get(result.page_id)].filter(
                (record) => record?.launcher_child === launcher.child,
              );
        for (const affected of affectedPages) {
          affected.cleanup_pending = true;
          try {
            recordLaunchReconciliation(
              launchReconciliations,
              affected.launch,
              LAUNCH_SETTLEMENT_PENDING,
              {
                effects: { page_acquired: true, vm_acquired: true },
              },
            );
          } catch (error) {
            retainLaunchReconciliationPendingInMemory(
              launchReconciliations,
              affected.launch,
            );
            logEvent("launch_reconciliation_persist_failed", {
              stream_id: affected.launch.stream_id,
              intended_state: LAUNCH_SETTLEMENT_PENDING,
              error: error instanceof Error ? error.message : String(error),
            });
          }
        }
        logEvent("launcher_exit", {
          request_id: requestId,
          page_id: result.page_id,
          code,
          signal: exitSignal,
          settlement: LAUNCH_SETTLEMENT_PENDING,
        });
      };
      launcher.child.once("exit", handleLauncherExit);
      if (
        launcher.child.exitCode != null ||
        launcher.child.signalCode != null
      ) {
        handleLauncherExit(
          launcher.child.exitCode,
          launcher.child.signalCode,
        );
      }
    }
    logEvent("launch_ready", {
      request_id: requestId,
      page_id: result.page_id,
      latency_ms: Date.now() - startedAt,
    });
    return result;
  } catch (error) {
    let settledError = error;
    if (launchMayHaveActed) {
      settledError = await settleDispatchedLaunchFailure(
        launcher,
        error,
        Number(config.shutdown_timeout_ms ?? 30000),
      );
      recordProvenLaunchFailure(
        launchReconciliations,
        launch,
        settledError,
      );
    }
    logEvent("launch_failed", {
      request_id: requestId,
      stream_id: launch.stream_id,
      latency_ms: Date.now() - startedAt,
      error: error instanceof Error ? error.message : String(error),
      settlement:
        settledError?.launch_settlement ||
        (launchMayHaveActed ? "cleanup_pending" : "did_not_act"),
      ...(settledError?.launch_cleanup_error
        ? { cleanup_error: settledError.launch_cleanup_error }
        : {}),
    });
    if (error instanceof SyntaxError) {
      throw new Error(`Browser VM launcher output is not JSON: ${error.message}`);
    }
    throw error;
  } finally {
    pendingLaunches.delete(requestId);
  }
}

async function shutdownPage(
  config,
  controlServiceIdentity,
  body,
  activePages,
  activeVms,
  launchReconciliations,
) {
  const pageId = body?.page_id;
  if (!safeId(pageId)) {
    throw new Error("page_id must be a safe identifier");
  }
  const runtimeCleanup = validateRuntimeCleanupBinding(
    body?.runtime_cleanup,
    pageId,
  );
  if (body?.force_retire_vm !== true) {
    throw new Error("Browser VM cleanup requires force_retire_vm=true");
  }
  const durableRecord = requireExactDurableCleanupRecord(
    launchReconciliations,
    runtimeCleanup,
  );
  const record = activePages.get(pageId);
  if (record) {
    requireExactRuntimeCleanupRecord(
      config,
      controlServiceIdentity,
      runtimeCleanup,
      record,
    );
  }
  let vmKey = record?.vm_key || null;
  let vmRecord = vmKey ? activeVms.get(vmKey) : null;
  if (!vmRecord) {
    for (const [candidateKey, candidate] of activeVms.entries()) {
      if (candidate.pages.has(pageId)) {
        vmKey = candidateKey;
        vmRecord = candidate;
        break;
      }
    }
  }

  const recordCleanupPending = () =>
    recordLaunchReconciliation(
      launchReconciliations,
      durableRecord.launch,
      LAUNCH_SETTLEMENT_PENDING,
      {
        effects: { page_acquired: true, vm_acquired: true },
        cleanup_binding: runtimeCleanup,
      },
    );
  const runShutdownProgram = async (page) => {
    if (!config.shutdown_program) return;
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
  };

  if (!record && !vmRecord) {
    if (durableRecord.state !== LAUNCH_SETTLEMENT_TERMINAL) {
      throw new Error(
        "Browser VM cleanup remains indeterminate after service restart: exact owned launcher unavailable",
      );
    }
    return terminalCleanupReceipt(
      runtimeCleanup,
      {
        page_absent: true,
        child_absent: true,
        vm_absent: true,
        route_absent: true,
        socket_absent: true,
      },
      { already_absent: true },
    );
  }
  if (
    vmRecord &&
    [...vmRecord.pages].some((ownedPageId) => ownedPageId !== pageId)
  ) {
    throw new Error(
      "Browser VM cleanup cannot retire a VM that still owns another page",
    );
  }
  const launcherChild = vmRecord?.launcher_child || record?.launcher_child;
  const ownedLauncherChild = exactOwnedLauncherProcess(
    runtimeCleanup,
    record,
    vmRecord,
    launcherChild,
  );
  recordCleanupPending();
  if (record) record.cleanup_pending = true;

  let closeError = null;
  const controlSocketPath =
    vmRecord?.control_socket_path || record?.page?.control_socket_path || "";
  if (controlSocketPath) {
    try {
      await postJsonOverUnix(
        controlSocketPath,
        `/pages/${encodeURIComponent(pageId)}/close`,
        {},
        Number(config.shutdown_timeout_ms ?? 30000),
      );
    } catch (error) {
      closeError = error instanceof Error ? error.message : String(error);
      logEvent("page_close_failed", {
        page_id: pageId,
        error: closeError,
      });
    }
    if (closeError) {
      logEvent("page_close_forced_vm_retirement", {
        page_id: pageId,
        vm_key_hash: vmKeyHash(vmKey),
        error: closeError,
      });
    }
  }
  const cleanupErrors = [];
  try {
    await runShutdownProgram(record?.page || null);
  } catch (error) {
    cleanupErrors.push(error instanceof Error ? error.message : String(error));
  }
  if (cleanupErrors.length === 0) {
    try {
      await terminatePersistentLauncher(
        ownedLauncherChild,
        Number(config.shutdown_timeout_ms ?? 30000),
      );
      if (
        ownedLauncherChild.exitCode == null &&
        ownedLauncherChild.signalCode == null
      ) {
        throw new Error(
          "Browser VM exact owned launcher did not produce an exit receipt",
        );
      }
    } catch (error) {
      cleanupErrors.push(error instanceof Error ? error.message : String(error));
    }
  }
  if (cleanupErrors.length === 0) {
    try {
      fs.unlinkSync(runtimeCleanup.control_socket_path);
    } catch (error) {
      if (error?.code !== "ENOENT") {
        cleanupErrors.push(
          `Browser VM cleanup could not remove its exact control socket: ${
            error instanceof Error ? error.message : String(error)
          }`,
        );
      }
    }
  }
  if (cleanupErrors.length > 0) {
    throw new Error(`Browser VM cleanup failed: ${cleanupErrors.join("; ")}`);
  }

  const externallyUnresolved = [];
  if (fs.existsSync(runtimeCleanup.control_socket_path)) {
    externallyUnresolved.push("socket_absent");
  }
  if (externallyUnresolved.length > 0) {
    throw new Error(
      `Browser VM cleanup remains indeterminate: ${externallyUnresolved.join(", ")}`,
    );
  }

  activePages.delete(pageId);
  if (vmRecord) {
    clearIdleVmShutdown(vmRecord);
    vmRecord.pages.delete(pageId);
    if (vmKey && activeVms.get(vmKey) === vmRecord) {
      activeVms.delete(vmKey);
    }
  }
  const receipt = terminalCleanupReceipt(
    runtimeCleanup,
    exactCleanupEffects(
      runtimeCleanup,
      pageId,
      activePages,
      activeVms,
      true,
    ),
    {
      forced_vm_retirement: Boolean(closeError),
      ...(closeError ? { control_error: closeError } : {}),
    },
  );
  markVmPendingLaunchReconciliationsTerminal(
    launchReconciliations,
    vmRecord,
  );
  markLaunchReconciliationTerminal(
    launchReconciliations,
    runtimeCleanup.generation,
    runtimeCleanup.stream_id,
    {
      page_acquired: true,
      vm_acquired: true,
    },
  );
  return receipt;
}

function activePageGuestControl(activePages, activeVms, pageId) {
  if (!safeId(pageId)) {
    throw new Error("page_id must be a safe identifier");
  }
  const record = activePages.get(pageId);
  if (!record) {
    throw new Error("browser page not found");
  }
  if (record.cleanup_pending === true) {
    throw new Error("browser page cleanup is pending");
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
  return new Promise((resolve, reject) => {
    if (!child || child.exitCode != null || child.signalCode != null) {
      resolve();
      return;
    }
    let killTimer;
    let reapTimer;
    let settled = false;
    const settle = (error = null) => {
      if (settled) return;
      settled = true;
      clearTimeout(killTimer);
      clearTimeout(reapTimer);
      child.removeListener("exit", exited);
      if (error) reject(error);
      else resolve();
    };
    const exited = () => {
      settle();
    };
    child.once("exit", exited);
    killTimer = setTimeout(() => {
      try {
        if (!child.kill("SIGKILL")) {
          settle(new Error("Browser VM launcher could not be killed"));
          return;
        }
        reapTimer = setTimeout(
          () =>
            settle(
              new Error(
                "Browser VM launcher did not exit after its exact SIGKILL",
              ),
            ),
          Math.min(Math.max(timeoutMs, 100), 5000),
        );
      } catch (error) {
        settle(error);
      }
    }, timeoutMs);
    try {
      if (!child.kill("SIGTERM")) {
        settle(new Error("Browser VM launcher could not be terminated"));
      }
    } catch (error) {
      settle(error);
    }
  });
}

function prepareControlSocket(socketPath) {
  try {
    fs.lstatSync(socketPath);
    throw new Error(`control socket already exists: ${socketPath}`);
  } catch (error) {
    if (error?.code !== "ENOENT") throw error;
  }
  fs.mkdirSync(path.dirname(socketPath), { recursive: true, mode: 0o700 });
}

function serviceHasEffects(activePages, activeVms, pendingLaunches) {
  return activePages.size > 0 || activeVms.size > 0 || pendingLaunches.size > 0;
}

function main() {
  const config = parseConfig();
  const controlServiceIdentity = loadOrCreateControlServiceIdentity(config);
  const serviceStartedAtMs = Date.now();
  const serviceStartedAt = new Date(serviceStartedAtMs).toISOString();
  const activePages = new Map();
  const activeVms = new Map();
  const pendingLaunches = new Map();
  const pendingAbortControllers = new Set();
  const pendingLaunchTasks = new Set();
  let acceptingLaunches = true;
  let shutdownPromise = null;
  let ownedControlSocketIdentity = null;
  prepareControlSocket(config.control_socket_path);
  const launchReconciliations = launchReconciliationStore(
    config,
    controlServiceIdentity,
  );
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
          control_service: controlServiceIdentity,
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
        if (!acceptingLaunches) {
          throw codedError(
            "service_shutting_down",
            "Browser VM control service is shutting down",
          );
        }
        const abortController = new AbortController();
        pendingAbortControllers.add(abortController);
        res.on("close", () => {
          if (!responseStarted) abortController.abort();
        });
        try {
          const body = await readJsonBody(req);
          const launchTask = openPage(
            config,
            controlServiceIdentity,
            body,
            activePages,
            activeVms,
            pendingLaunches,
            launchReconciliations,
            abortController.signal,
          );
          pendingLaunchTasks.add(launchTask);
          let result;
          try {
            result = await launchTask;
          } finally {
            pendingLaunchTasks.delete(launchTask);
          }
          sendJson(
            200,
            result,
          );
        } finally {
          pendingAbortControllers.delete(abortController);
        }
        return;
      }
      if (req.method === "POST" && url.pathname === "/shutdown") {
        const body = await readJsonBody(req);
        sendJson(
          200,
          await shutdownPage(
            config,
            controlServiceIdentity,
            body,
            activePages,
            activeVms,
            launchReconciliations,
          ),
        );
        return;
      }
      if (req.method === "POST" && url.pathname === "/launches/reconcile") {
        sendJson(
          200,
          reconcileLaunch(
            launchReconciliations,
            activePages,
            await readJsonBody(req),
          ),
        );
        return;
      }
      if (req.method === "POST" && url.pathname === "/service/shutdown") {
        const body = await readJsonBody(req);
        if (
          body?.schema !== "elastos.browser.vm-control-service.shutdown/v1" ||
          body.config_fingerprint !== (config.config_fingerprint || null) ||
          body.started_at !== serviceStartedAt
        ) {
          throw codedError(
            "control_service_substitution",
            "Browser VM control service shutdown identity did not match",
          );
        }
        if (serviceHasEffects(activePages, activeVms, pendingLaunches)) {
          throw codedError(
            "resources_in_use",
            "Browser VM control service owns active, warm, or pending VM effects",
          );
        }
        acceptingLaunches = false;
        sendJson(200, {
          schema: "elastos.browser.vm-control-service.shutdown-accepted/v1",
          accepted: true,
        });
        setImmediate(() => {
          void shutdownService(false);
        });
        return;
      }
      sendJson(404, { error: "not found" });
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      sendJson(
        error?.code === "resources_in_use" ? 409 : 400,
        typeof error?.code === "string"
          ? { code: error.code, message }
          : { error: message },
      );
    }
  });
  const shutdownService = (force) => {
    if (shutdownPromise) return shutdownPromise;
    shutdownPromise = (async () => {
      acceptingLaunches = false;
      for (const controller of pendingAbortControllers) controller.abort();
      for (const record of activeVms.values()) clearIdleVmShutdown(record);
      server.close();
      server.closeAllConnections?.();
      await Promise.allSettled([...pendingLaunchTasks]);
      const children = [...ownedLauncherChildren];
      const results = await Promise.allSettled(
        children.map((child) =>
          terminatePersistentLauncher(
            child,
            Number(config.shutdown_timeout_ms ?? 30000),
          ),
        ),
      );
      const failures = results
        .filter((result) => result.status === "rejected")
        .map((result) => result.reason instanceof Error ? result.reason.message : String(result.reason));
      if (failures.length > 0) {
        throw new Error(`Browser VM control service could not reap owned launchers: ${failures.join("; ")}`);
      }
      activePages.clear();
      activeVms.clear();
      pendingLaunches.clear();
      try {
        const current = fs.lstatSync(config.control_socket_path);
        if (
          !ownedControlSocketIdentity ||
          !current.isSocket() ||
          current.dev !== ownedControlSocketIdentity.dev ||
          current.ino !== ownedControlSocketIdentity.ino
        ) {
          throw new Error(
            "Browser VM control socket identity changed; refusing to unlink an unowned path",
          );
        }
        fs.unlinkSync(config.control_socket_path);
      } catch (error) {
        if (error?.code !== "ENOENT") throw error;
      }
      logEvent("service_shutdown", {
        forced: force,
        owned_launcher_children: children.length,
      });
    })();
    return shutdownPromise;
  };
  const handleSignal = () => {
    void shutdownService(true).then(
      () => process.exit(0),
      (error) => {
        console.error(error instanceof Error ? error.message : String(error));
        process.exit(1);
      },
    );
  };
  process.once("SIGTERM", handleSignal);
  process.once("SIGINT", handleSignal);
  server.listen(config.control_socket_path, () => {
    const socketStat = fs.lstatSync(config.control_socket_path);
    if (!socketStat.isSocket()) {
      throw new Error("Browser VM control service did not bind a Unix socket");
    }
    ownedControlSocketIdentity = {
      dev: socketStat.dev,
      ino: socketStat.ino,
    };
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
