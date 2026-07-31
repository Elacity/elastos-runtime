#!/usr/bin/env node
import crypto from "node:crypto";
import fs from "node:fs";
import net from "node:net";
import path from "node:path";
import process from "node:process";
import { pathToFileURL } from "node:url";
import { spawn, spawnSync } from "node:child_process";

const OPEN_REQUEST_ENV = "ELASTOS_BROWSER_VM_OPEN_REQUEST";
const VZ_TRANSPORT_AUTHORITY_SCHEMA =
  "elastos.browser.vz-transport-authority/v1";
const VZ_TRANSPORT_SECRET_SCHEMA =
  "elastos.browser.vz-transport-secret/v1";
const VZ_LAUNCH_SETTLEMENT_SCHEMA =
  "elastos.browser.vz-launch-settlement/v1";

const sshHost = process.env.ELASTOS_BROWSER_REMOTE_VZ_SSH || "";
const configuredSshBin =
  process.env.ELASTOS_BROWSER_REMOTE_VZ_SSH_BIN || "ssh";
let sshBin = configuredSshBin;
const localRoot = process.env.ELASTOS_BROWSER_REMOTE_VZ_LOCAL_ROOT || "/tmp/evzl";
const remoteRoot = process.env.ELASTOS_BROWSER_REMOTE_VZ_ROOT || "/tmp/evzs";
const localSocketRoot =
  process.env.ELASTOS_BROWSER_REMOTE_VZ_SOCKET_ROOT || "/tmp/evzlc";
const remoteSocketRoot =
  process.env.ELASTOS_BROWSER_REMOTE_VZ_REMOTE_SOCKET_ROOT || "/tmp/evzrc";
const remoteProfileRoot = process.env.ELASTOS_BROWSER_REMOTE_VZ_PROFILE_ROOT || "";
const remoteRelayMaxSessions = Number(process.env.ELASTOS_BROWSER_REMOTE_VZ_RELAY_MAX_SESSIONS || "16");
const launchTimeoutMs = Number(process.env.ELASTOS_BROWSER_REMOTE_VZ_LAUNCH_TIMEOUT_MS || "180000");
const defaultControlProxyRequestTimeoutMs = "120000";
const socketTimeoutMs = Number(process.env.ELASTOS_BROWSER_REMOTE_VZ_SOCKET_TIMEOUT_MS || "15000");
const unixSocketPathBudget = 100;
const maxControlRequestBytes = 4 * 1024 * 1024;
const vmLauncherEnvKeys = [
  "ELASTOS_BROWSER_VM_TURN_PROGRAM",
  "ELASTOS_BROWSER_VM_TURNSERVER_BIN",
];
const legacyVzConfigurationKeys = [
  "ELASTOS_BROWSER_VM_ICE_SERVER",
  "ELASTOS_BROWSER_VM_ICE_SERVERS_JSON",
  "ELASTOS_BROWSER_VM_ICE_USERNAME",
  "ELASTOS_BROWSER_VM_ICE_CREDENTIAL",
  "ELASTOS_BROWSER_VM_ICE_TRANSPORT_POLICY",
  "ELASTOS_BROWSER_VM_MEDIA_RELAY_HOST_IPV4",
  "ELASTOS_BROWSER_VM_MEDIA_RELAY_GUEST_IPV4",
  "ELASTOS_BROWSER_VM_MEDIA_RELAY_PREFIX",
  "ELASTOS_BROWSER_VM_HIBERNATION",
  "ELASTOS_BROWSER_VM_HIBERNATION_DIR",
  "ELASTOS_BROWSER_VM_HIBERNATION_MAX_ENTRIES",
  "ELASTOS_BROWSER_VM_HIBERNATION_MAX_AGE_SECS",
  "ELASTOS_BROWSER_VM_DISABLE_EGRESS_BRIDGE",
  "ELASTOS_BROWSER_VM_RELAY_PORT",
  "ELASTOS_BROWSER_REMOTE_VZ_RELAY_ROOT",
  "ELASTOS_BROWSER_REMOTE_VZ_TURN_ENV",
  "ELASTOS_BROWSER_REMOTE_VZ_REAP_STALE_SUPERVISORS",
  "ELASTOS_BROWSER_REMOTE_VZ_REAP_STALE_RELAYS",
];

const children = new Set();
const localServers = new Set();
const cleanupCommands = [];
const remoteAbsenceChecks = [];
const localCleanupDirs = new Set();
let cleanupPromise = null;
let launchIdentity = null;
let localTurnEndpoint = null;
let firstEffectOccurred = false;
let routeAbsenceProved = false;
let nativeVmAbsenceProved = false;
let remoteSupervisorChild = null;
// These are conservative may-have-acted markers. Each is set no later than
// the first point where its acquisition may have occurred; absence is proved
// independently during cleanup and is never inferred from the marker.
const launchEffects = {
  session_directory: false,
  control_socket: false,
  ordinary_stream_bridge: false,
  media_stream_bridge: false,
  turn_process: false,
  supervisor_child: false,
  vm: false,
};

function fail(message) {
  console.error(message);
  process.exit(1);
}

function resolveExecutable(program, label) {
  if (
    typeof program !== "string" ||
    !program ||
    /[\r\n\0]/.test(program)
  ) {
    throw new Error(`${label} path is invalid`);
  }
  const candidates = program.includes("/")
    ? [program]
    : String(process.env.PATH || "")
        .split(path.delimiter)
        .filter(Boolean)
        .map((directory) => path.join(directory, program));
  for (const candidate of candidates) {
    if (!path.isAbsolute(candidate)) continue;
    try {
      const stat = fs.statSync(candidate);
      if (stat.isFile() && (stat.mode & 0o111) !== 0) {
        return fs.realpathSync(candidate);
      }
    } catch {}
  }
  throw new Error(`${label} must resolve to an executable regular file`);
}

function exactObjectKeys(value, keys) {
  return (
    value &&
    typeof value === "object" &&
    !Array.isArray(value) &&
    Object.keys(value).length === keys.length &&
    keys.every((key) => Object.hasOwn(value, key))
  );
}

function canonicalJson(value) {
  if (Array.isArray(value)) return value.map(canonicalJson);
  if (value && typeof value === "object") {
    return Object.fromEntries(
      Object.keys(value)
        .sort()
        .map((key) => [key, canonicalJson(value[key])]),
    );
  }
  return value;
}

function sha256Label(value) {
  return `sha256:${crypto.createHash("sha256").update(value).digest("hex")}`;
}

function sha256LabelIsSafe(value) {
  return typeof value === "string" && /^sha256:[0-9a-f]{64}$/.test(value);
}

function safeId(value) {
  return (
    typeof value === "string" &&
    value.length <= 512 &&
    /^[A-Za-z0-9:_-]+$/.test(value)
  );
}

function loopbackLiteral(value) {
  if (typeof value !== "string" || net.isIP(value) === 0) return false;
  return net.isIPv4(value)
    ? value.startsWith("127.")
    : value === "::1" || value === "0:0:0:0:0:0:0:1";
}

function bindingDigest(authority) {
  return authority.binding_hash.slice("sha256:".length, "sha256:".length + 32);
}

function boundSocketPaths(authority) {
  const digest = bindingDigest(authority);
  return {
    local_directory: path.join(localSocketRoot, digest),
    local_control: path.join(localSocketRoot, digest, "c.sock"),
    remote_directory: path.posix.join(remoteSocketRoot, digest),
    remote_control: path.posix.join(remoteSocketRoot, digest, "c.sock"),
    remote_session: path.posix.join(remoteRoot, `vz-${digest}`),
  };
}

function validateAbsolutePath(value, label) {
  if (typeof value !== "string" || !value.startsWith("/") || /[\r\n\0]/.test(value)) {
    throw new Error(`${label} must be an absolute path without control characters`);
  }
}

function validateUnixSocketPathBudget(value, label) {
  const bytes = Buffer.byteLength(value);
  if (bytes >= unixSocketPathBudget) {
    throw new Error(`${label} is too long for macOS Unix sockets (${bytes} bytes): ${value}`);
  }
}

function validateSafePathSegment(value, label) {
  if (typeof value !== "string" || !/^[A-Za-z0-9._-]+$/.test(value)) {
    throw new Error(`${label} must be a safe path segment`);
  }
}

function rejectLegacyVzConfiguration(env = process.env) {
  const mixed = legacyVzConfigurationKeys.filter(
    (key) => env[key] != null && env[key] !== "",
  );
  if (mixed.length > 0) {
    throw new Error(
      `remote Browser VZ launch rejects legacy or mixed transport configuration: ${mixed.join(", ")}`,
    );
  }
}

function shellQuote(value) {
  return `'${String(value).replace(/'/g, `'\\''`)}'`;
}

function optionalRemoteEnvExports(keys, env = process.env) {
  return keys.flatMap((key) => {
    const value = env[key];
    if (value == null || value === "") return [];
    if (/[\r\n\0]/.test(value)) {
      throw new Error(`${key} must not contain control characters`);
    }
    return [`export ${key}=${shellQuote(value)}`];
  });
}

function sshBaseArgs() {
  return [
    "-o",
    "BatchMode=yes",
    "-o",
    "ExitOnForwardFailure=yes",
    "-o",
    "ServerAliveInterval=15",
    "-o",
    "ServerAliveCountMax=2",
    "-o",
    "StreamLocalBindUnlink=yes",
  ];
}

function sshRemoteShellArgs(command) {
  return [...sshBaseArgs(), sshHost, `sh -lc ${shellQuote(command)}`];
}

function sshControlRemoteShellArgs(command) {
  return [
    ...sshBaseArgs(),
    "-o",
    "ControlMaster=auto",
    "-o",
    "ControlPersist=120",
    "-o",
    "ControlPath=/tmp/elastos-remote-vz-ssh-%C",
    sshHost,
    `sh -lc ${shellQuote(command)}`,
  ];
}

function spawnTracked(command, args, options = {}, effect = "") {
  const child = spawn(command, args, options);
  child.elastosEffect = effect;
  children.add(child);
  return child;
}

function trackLocalServer(server, socketPath = "", effect = "") {
  const record = {
    server,
    socketPath,
    effect,
    acquired: false,
    closed: false,
  };
  localServers.add(record);
  server.once("close", () => {
    record.closed = true;
  });
  return record;
}

function portForSuffix(suffix, salt) {
  const hash = crypto.createHash("sha256").update(`${suffix}:${salt}`).digest("hex");
  return 28000 + (Number.parseInt(hash.slice(0, 4), 16) % 20000);
}

function listen(server, ...args) {
  return new Promise((resolve, reject) => {
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
    server.listen(...args);
  });
}

function bridgeSockets(left, right) {
  const destroyBoth = () => {
    left.destroy();
    right.destroy();
  };
  left.on("error", destroyBoth);
  right.on("error", destroyBoth);
  left.pipe(right);
  right.pipe(left);
}

function runSsh(command, { input = "", timeoutMs = 15000 } = {}) {
  const result = spawnSync(sshBin, sshRemoteShellArgs(command), {
    input,
    encoding: "utf8",
    timeout: timeoutMs,
  });
  if (result.error) {
    throw result.error;
  }
  if (result.status !== 0) {
    throw new Error((result.stderr || result.stdout || `ssh exited ${result.status}`).trim());
  }
  return result.stdout || "";
}

function resolveRemoteDataDir() {
  if (process.env.ELASTOS_BROWSER_REMOTE_VZ_DATA_DIR) {
    validateAbsolutePath(process.env.ELASTOS_BROWSER_REMOTE_VZ_DATA_DIR, "ELASTOS_BROWSER_REMOTE_VZ_DATA_DIR");
    return process.env.ELASTOS_BROWSER_REMOTE_VZ_DATA_DIR;
  }
  const value = runSsh('printf "%s" "$HOME/Library/Application Support/elastos"', { timeoutMs: 5000 }).trim();
  validateAbsolutePath(value, "remote Browser VZ data dir");
  return value;
}

function remoteLaunchTurnProgramEnv() {
  const program =
    process.env.ELASTOS_BROWSER_REMOTE_VZ_TURN_PROGRAM ||
    process.env.ELASTOS_BROWSER_VM_TURN_PROGRAM ||
    "";
  validateAbsolutePath(program, "remote Browser VZ TURN program");
  return {
    ELASTOS_BROWSER_VM_TURN_PROGRAM: program,
  };
}

function readOpenRequest() {
  const stdin = fs.readFileSync(0);
  if (stdin.length > maxControlRequestBytes) {
    throw new Error(
      `Browser VM remote VZ request exceeds ${maxControlRequestBytes} bytes`,
    );
  }
  const stdinText = stdin.toString("utf8");
  const raw = stdinText.trim() ? stdinText : process.env[OPEN_REQUEST_ENV] || "";
  if (Buffer.byteLength(raw) > maxControlRequestBytes) {
    throw new Error(
      `Browser VM remote VZ request exceeds ${maxControlRequestBytes} bytes`,
    );
  }
  if (!raw.trim()) fail(`${OPEN_REQUEST_ENV} or stdin JSON is required`);
  try {
    return JSON.parse(raw);
  } catch (error) {
    fail(`Browser VM remote VZ request is invalid JSON: ${error instanceof Error ? error.message : String(error)}`);
  }
}

function validateVzTransportStream(stream, loopbackTarget) {
  if (
    !exactObjectKeys(stream, [
      "schema",
      "stream_id",
      "target",
      "runtime_socket_path",
      "vsock_port",
    ]) ||
    stream.schema !== "elastos.browser.vz-transport-stream/v1" ||
    !safeId(stream.stream_id) ||
    typeof stream.runtime_socket_path !== "string" ||
    !stream.runtime_socket_path.startsWith("/") ||
    Buffer.byteLength(stream.runtime_socket_path) >= unixSocketPathBudget ||
    /[\r\n\0]/.test(stream.runtime_socket_path) ||
    !Number.isInteger(stream.vsock_port) ||
    stream.vsock_port < 1 ||
    stream.vsock_port > 0xffffffff
  ) {
    throw new Error("invalid Browser VZ transport stream");
  }
  let target;
  try {
    target = new URL(stream.target);
  } catch {
    throw new Error("invalid Browser VZ transport target");
  }
  if (
    !["tcp:", "tls:"].includes(target.protocol) ||
    !target.port ||
    target.username ||
    target.password ||
    !["", "/"].includes(target.pathname) ||
    target.search ||
    target.hash ||
    (loopbackTarget && !loopbackLiteral(target.hostname))
  ) {
    throw new Error("invalid Browser VZ transport target");
  }
  return stream;
}

function validateVzTurnAuthority(turn, expiresAtUnixMs) {
  if (
    !exactObjectKeys(turn, [
      "schema",
      "guest_url",
      "guest_host",
      "guest_port",
      "listen_host",
      "listen_port",
      "advertised_host",
      "relay_host",
      "relay_port_min",
      "relay_port_max",
      "protocols",
      "username",
      "credential_hash",
      "auth_secret_hash",
    ]) ||
    turn.schema !== "elastos.browser.vz-turn-authority/v1" ||
    !loopbackLiteral(turn.guest_host) ||
    !loopbackLiteral(turn.listen_host) ||
    !Number.isInteger(turn.guest_port) ||
    turn.guest_port < 1 ||
    turn.guest_port > 65535 ||
    !Number.isInteger(turn.listen_port) ||
    turn.listen_port < 1 ||
    turn.listen_port > 65535 ||
    typeof turn.advertised_host !== "string" ||
    !turn.advertised_host ||
    turn.advertised_host.length > 253 ||
    /[\s\r\n\0/\\]/.test(turn.advertised_host) ||
    typeof turn.relay_host !== "string" ||
    net.isIP(turn.relay_host) === 0 ||
    !Number.isInteger(turn.relay_port_min) ||
    !Number.isInteger(turn.relay_port_max) ||
    turn.relay_port_min < 1 ||
    turn.relay_port_max > 65535 ||
    turn.relay_port_min > turn.relay_port_max ||
    turn.relay_port_max - turn.relay_port_min + 1 > 64 ||
    JSON.stringify(turn.protocols) !== JSON.stringify(["turn", "tcp"]) ||
    turn.guest_url !==
      `turn:${turn.guest_host}:${turn.guest_port}?transport=tcp` ||
    typeof turn.username !== "string" ||
    !/^[0-9]+:[A-Za-z0-9_-]+$/.test(turn.username) ||
    Number(turn.username.split(":", 1)[0]) * 1000 !== expiresAtUnixMs ||
    !sha256LabelIsSafe(turn.credential_hash) ||
    !sha256LabelIsSafe(turn.auth_secret_hash)
  ) {
    throw new Error("invalid Browser VZ TURN authority");
  }
}

function validateVzTransportAuthority(authority) {
  if (
    !exactObjectKeys(authority, [
      "schema",
      "binding_hash",
      "generation",
      "page_id",
      "vm_id",
      "principal_id",
      "egress",
      "media",
      "turn",
      "bootstrap_vsock_port",
      "expires_at_unix_ms",
    ]) ||
    authority.schema !== VZ_TRANSPORT_AUTHORITY_SCHEMA ||
    !sha256LabelIsSafe(authority.binding_hash) ||
    !sha256LabelIsSafe(authority.generation) ||
    !safeId(authority.page_id) ||
    !safeId(authority.vm_id) ||
    !safeId(authority.principal_id) ||
    !Number.isInteger(authority.bootstrap_vsock_port) ||
    authority.bootstrap_vsock_port < 1 ||
    authority.bootstrap_vsock_port > 0xffffffff ||
    !Number.isSafeInteger(authority.expires_at_unix_ms) ||
    authority.expires_at_unix_ms <= Date.now() ||
    authority.expires_at_unix_ms > Date.now() + 24 * 60 * 60 * 1000
  ) {
    throw new Error("invalid or stale Browser VZ transport authority");
  }
  const egress = validateVzTransportStream(authority.egress, false);
  const media = validateVzTransportStream(authority.media, true);
  if (
    egress.stream_id === media.stream_id ||
    egress.runtime_socket_path === media.runtime_socket_path ||
    egress.vsock_port === media.vsock_port ||
    authority.bootstrap_vsock_port === egress.vsock_port ||
    authority.bootstrap_vsock_port === media.vsock_port
  ) {
    throw new Error("Browser VZ transport bindings are not distinct");
  }
  validateVzTurnAuthority(authority.turn, authority.expires_at_unix_ms);
  const unsigned = { ...authority };
  delete unsigned.binding_hash;
  if (
    sha256Label(Buffer.from(JSON.stringify(canonicalJson(unsigned)))) !==
      authority.binding_hash ||
    Buffer.byteLength(JSON.stringify(authority)) > 32 * 1024
  ) {
    throw new Error("Browser VZ transport authority binding hash mismatch");
  }
  return authority;
}

function validateVzTransportSecret(authority, secret) {
  if (
    !exactObjectKeys(secret, [
      "schema",
      "binding_hash",
      "credential",
      "auth_secret",
    ]) ||
    secret.schema !== VZ_TRANSPORT_SECRET_SCHEMA ||
    secret.binding_hash !== authority.binding_hash ||
    typeof secret.credential !== "string" ||
    !secret.credential ||
    secret.credential.length > 512 ||
    /[\r\n\0]/.test(secret.credential) ||
    typeof secret.auth_secret !== "string" ||
    !secret.auth_secret ||
    secret.auth_secret.length > 512 ||
    /[\r\n\0]/.test(secret.auth_secret) ||
    sha256Label(Buffer.from(secret.credential)) !==
      authority.turn.credential_hash ||
    sha256Label(Buffer.from(secret.auth_secret)) !==
      authority.turn.auth_secret_hash ||
    crypto
      .createHmac("sha1", secret.auth_secret)
      .update(authority.turn.username)
      .digest("base64") !== secret.credential
  ) {
    throw new Error("invalid Browser VZ transport secret");
  }
}

function remoteProfileDiskPath(profile, remoteDataDir) {
  if (!profile || typeof profile !== "object") {
    throw new Error("remote VZ launch requires a Browser profile descriptor");
  }
  const uri = String(profile.uri || "");
  const match = uri.match(/^localhost:\/\/Users\/([^/]+)\/BrowserProfiles\/default\/profile\.ext4$/);
  if (!match) {
    throw new Error("remote VZ Browser profile uri must be an active-principal BrowserProfiles path");
  }
  const principalRoot = match[1];
  validateSafePathSegment(principalRoot, "remote VZ Browser profile principal root");
  if (profile.disk_path) {
    validateAbsolutePath(profile.disk_path, "launch_request.profile.disk_path");
  }
  const root = remoteProfileRoot || path.posix.join(remoteDataDir, "Users");
  validateAbsolutePath(root, "ELASTOS_BROWSER_REMOTE_VZ_PROFILE_ROOT");
  return path.posix.join(root, principalRoot, "BrowserProfiles/default/profile.ext4");
}

function validateOpenRequest(request) {
  launchIdentity = null;
  localTurnEndpoint = null;
  if (request.schema !== "elastos.browser.vm-engine.open/v1") {
    throw new Error("remote VZ launcher requires elastos.browser.vm-engine.open/v1");
  }
  const launch = request.launch_request || {};
  if (launch.schema !== "elastos.browser.engine.launch-request/v1") {
    throw new Error("remote VZ launcher missing Browser launch_request");
  }
  if (!safeId(launch.stream_id)) {
    throw new Error("launch_request.stream_id must be a safe identifier");
  }
  if (launch.engine !== "chromium_microvm") {
    throw new Error("remote VZ launcher accepts only chromium_microvm");
  }
  if (launch.network_mode !== "runtime_net_only" || launch.direct_network !== false || launch.wallet_injection !== false) {
    throw new Error("remote VZ launcher requires runtime_net_only with no direct network or wallet injection");
  }
  if (launch.display_mode !== "webrtc_remote_display") {
    throw new Error("remote VZ launcher requires display_mode=webrtc_remote_display");
  }
  if (launch.guarantee_level !== "mechanism_microvm") {
    throw new Error("remote VZ launcher requires guarantee_level=mechanism_microvm");
  }
  if (launch.adapter_ipc?.kind !== "unix_socket") {
    throw new Error("remote VZ launcher requires launch_request.adapter_ipc.kind=unix_socket");
  }
  validateAbsolutePath(
    launch.adapter_ipc?.runtime_stream_path,
    "launch_request.adapter_ipc.runtime_stream_path",
  );
  let stat;
  try {
    stat = fs.statSync(launch.adapter_ipc.runtime_stream_path);
  } catch (error) {
    throw new Error(`server Runtime stream socket is not reachable: ${launch.adapter_ipc.runtime_stream_path}`);
  }
  if (!stat.isSocket()) {
    throw new Error(`server Runtime stream path is not a Unix socket: ${launch.adapter_ipc.runtime_stream_path}`);
  }
  const authority = validateVzTransportAuthority(
    launch.transport_authority,
  );
  validateVzTransportSecret(authority, launch.transport_secret);
  if (
    !safeId(launch.page_id) ||
    !safeId(launch.vm_id) ||
    authority.page_id !== launch.page_id ||
    authority.vm_id !== launch.vm_id ||
    authority.generation !== launch.lifecycle_generation ||
    authority.egress.stream_id !== launch.stream_id ||
    authority.egress.runtime_socket_path !==
      launch.adapter_ipc.runtime_stream_path ||
    authority.principal_id !== launch.principal_id
  ) {
    throw new Error("remote VZ transport launch binding is invalid");
  }
  launchIdentity = {
    binding_hash: authority.binding_hash,
    generation: authority.generation,
    page_id: authority.page_id,
    vm_id: authority.vm_id,
    stream_id: authority.egress.stream_id,
    media_stream_id: authority.media.stream_id,
  };
  localTurnEndpoint = {
    host: authority.turn.listen_host,
    port: authority.turn.listen_port,
  };
  return launch;
}

function cloneForRemote(request, remoteDataDir) {
  const copy = JSON.parse(JSON.stringify(request));
  const profile = copy.profile || copy.launch_request.profile;
  const remoteDiskPath = remoteProfileDiskPath(profile, remoteDataDir);
  if (copy.profile) {
    copy.profile = {
      ...copy.profile,
      disk_path: remoteDiskPath,
    };
  }
  if (copy.launch_request.profile) {
    copy.launch_request.profile = {
      ...copy.launch_request.profile,
      disk_path: remoteDiskPath,
    };
  }
  return copy;
}

function assertOwnerOnlyDirectoryIfPresent(directory, label) {
  if (!fs.existsSync(directory)) return;
  const stat = fs.lstatSync(directory);
  if (
    !stat.isDirectory() ||
    stat.uid !== process.getuid() ||
    (stat.mode & 0o077) !== 0
  ) {
    throw new Error(`${label} must be an owner-only directory`);
  }
}

function assertAbsent(value, label) {
  if (fs.existsSync(value)) {
    throw new Error(`${label} already exists: ${value}`);
  }
}

function preflightLocalWrapper(paths, authority, localSessionDir) {
  const wrapperPath = fs.realpathSync(process.argv[1]);
  const wrapperStat = fs.statSync(wrapperPath);
  if (!wrapperStat.isFile()) {
    throw new Error("remote VZ private-stdin wrapper is not a regular file");
  }
  assertOwnerOnlyDirectoryIfPresent(localSocketRoot, "local VZ socket root");
  assertOwnerOnlyDirectoryIfPresent(localRoot, "local VZ session root");
  assertAbsent(paths.local_directory, "local VZ binding socket directory");
  assertAbsent(paths.local_control, "local VZ control socket");
  assertAbsent(localSessionDir, "local VZ binding session directory");
  validateUnixSocketPathBudget(paths.local_control, "local control socket path");
  for (const [label, socketPath] of [
    ["Runtime egress stream", authority.egress.runtime_socket_path],
    ["Runtime media stream", authority.media.runtime_socket_path],
  ]) {
    validateUnixSocketPathBudget(socketPath, label);
    const stat = fs.statSync(socketPath);
    if (!stat.isSocket()) {
      throw new Error(`${label} is not an exact Unix socket`);
    }
  }
}

async function assertTcpPortAvailable(host, port, label) {
  const server = net.createServer();
  await new Promise((resolve, reject) => {
    server.once("error", (error) =>
      reject(new Error(`${label} is unavailable at ${host}:${port}: ${error.message}`)),
    );
    server.listen(port, host, resolve);
  });
  await new Promise((resolve, reject) =>
    server.close((error) => (error ? reject(error) : resolve())),
  );
}

function remotePreflightCommand({
  remoteDataDir,
  remoteVmEnv,
  paths,
  authority,
  relayTcpPorts,
  pidPaths,
}) {
  const nativeSupervisor = path.posix.join(
    remoteDataDir,
    "bin/browser-vz-engine-supervisor",
  );
  const privateStdinWrapper = path.posix.join(
    remoteDataDir,
    "bin/browser-vm-engine-supervisor",
  );
  const privateStdinScript = path.posix.join(
    remoteDataDir,
    "bin/browser-vm-engine-supervisor.mjs",
  );
  const kernel = path.posix.join(remoteDataDir, "bin/vmlinux");
  const rootfs = path.posix.join(remoteDataDir, "browser-vm/rootfs.ext4");
  const initramfs = path.posix.join(remoteDataDir, "bin/initrd");
  const socketRoot = path.posix.dirname(paths.remote_directory);
  const supervisorMarkerPrefix = "--elastos-vz-binding=";
  const supervisorBindingHash = authority.binding_hash;
  const checkedPaths = [
    paths.remote_directory,
    paths.remote_control,
    paths.remote_session,
    authority.egress.runtime_socket_path,
    authority.media.runtime_socket_path,
    ...pidPaths,
  ];
  const checkedPorts = [
    authority.turn.listen_port,
    ...Array.from(
      {
        length:
          authority.turn.relay_port_max -
          authority.turn.relay_port_min +
          1,
      },
      (_, index) => authority.turn.relay_port_min + index,
    ),
    ...relayTcpPorts,
  ];
  for (const [value, label] of [
    [paths.remote_control, "remote control socket path"],
    [authority.egress.runtime_socket_path, "remote egress socket path"],
    [authority.media.runtime_socket_path, "remote media socket path"],
  ]) {
    validateUnixSocketPathBudget(value, label);
  }
  return [
    "set -euo pipefail",
    "umask 077",
    `native=${shellQuote(nativeSupervisor)}`,
    `private_wrapper=${shellQuote(privateStdinWrapper)}`,
    `private_script=${shellQuote(privateStdinScript)}`,
    `kernel=${shellQuote(kernel)}`,
    `rootfs=${shellQuote(rootfs)}`,
    `initramfs=${shellQuote(initramfs)}`,
    `socket_root=${shellQuote(socketRoot)}`,
    `session_root=${shellQuote(remoteRoot)}`,
    `turn_program=${shellQuote(remoteVmEnv.ELASTOS_BROWSER_VM_TURN_PROGRAM)}`,
    `supervisor_marker_prefix=${shellQuote(supervisorMarkerPrefix)}`,
    `supervisor_binding_hash=${shellQuote(supervisorBindingHash)}`,
    '[ -x "$native" ] && [ -f "$native" ]',
    '[ -x "$private_wrapper" ] && [ -f "$private_wrapper" ]',
    '[ -r "$private_script" ] && [ -f "$private_script" ]',
    '[ -r "$kernel" ] && [ -f "$kernel" ]',
    '[ -r "$rootfs" ] && [ -f "$rootfs" ]',
    '[ ! -e "$initramfs" ] || [ -f "$initramfs" ]',
    '[ -x "$turn_program" ] && [ -f "$turn_program" ]',
    'if [ -e "$socket_root" ]; then',
    '  [ -d "$socket_root" ]',
    '  [ "$(/usr/bin/stat -f %u "$socket_root")" = "$(/usr/bin/id -u)" ]',
    '  [ "$(/usr/bin/stat -f %Lp "$socket_root")" = "700" ]',
    "fi",
    'if [ -e "$session_root" ]; then',
    '  [ -d "$session_root" ]',
    '  [ "$(/usr/bin/stat -f %u "$session_root")" = "$(/usr/bin/id -u)" ]',
    '  [ "$(/usr/bin/stat -f %Lp "$session_root")" = "700" ]',
    "fi",
    "/usr/bin/codesign --verify --strict \"$native\" >/dev/null 2>&1",
    "/usr/bin/codesign -d --entitlements :- \"$native\" 2>&1 | /usr/bin/grep -q 'com.apple.security.virtualization'",
    "[ -x /bin/ps ]",
    "[ -x /usr/bin/awk ]",
    "[ -x /usr/bin/grep ]",
    "command -v python3 >/dev/null 2>&1",
    ...checkedPaths.map(
      (checkedPath) => `[ ! -e ${shellQuote(checkedPath)} ]`,
    ),
    "process_snapshot=$(/bin/ps -ww -axo command=)",
    '! printf \'%s\\n\' "$process_snapshot" | /usr/bin/grep -F -- "${supervisor_marker_prefix}${supervisor_binding_hash}" >/dev/null',
    "command -v lsof >/dev/null 2>&1",
    ...checkedPorts.flatMap((port) => [
      `! lsof -nP -iTCP:${shellQuote(String(port))} 2>/dev/null | grep -q .`,
      `! lsof -nP -iUDP:${shellQuote(String(port))} 2>/dev/null | grep -q .`,
    ]),
    "printf '%s\\n' PREFLIGHT_OK",
  ].join("\n");
}

async function preflightLaunch({
  remoteDataDir,
  remoteVmEnv,
  paths,
  authority,
  relayTcpPorts,
  pidPaths,
  localSessionDir,
}) {
  preflightLocalWrapper(paths, authority, localSessionDir);
  await assertTcpPortAvailable(
    authority.turn.listen_host,
    authority.turn.listen_port,
    "local TURN forward port",
  );
  const result = runSsh(
    remotePreflightCommand({
      remoteDataDir,
      remoteVmEnv,
      paths,
      authority,
      relayTcpPorts,
      pidPaths,
    }),
    { timeoutMs: 15_000 },
  );
  if (result.trim() !== "PREFLIGHT_OK") {
    throw new Error("remote Browser VZ preflight did not return PREFLIGHT_OK");
  }
}

function createLocalBindingDirectory(paths) {
  // mkdir/write can leave a partial exact binding behind, so ownership and
  // the conservative may-have-acted marker precede the first filesystem call.
  localCleanupDirs.add(paths.local_directory);
  launchEffects.session_directory = true;
  firstEffectOccurred = true;
  fs.mkdirSync(localSocketRoot, { recursive: true, mode: 0o700 });
  fs.chmodSync(localSocketRoot, 0o700);
  fs.mkdirSync(localRoot, { recursive: true, mode: 0o700 });
  fs.chmodSync(localRoot, 0o700);
  fs.mkdirSync(paths.local_directory, { mode: 0o700 });
  const ownerPath = path.join(paths.local_directory, "owner.json");
  fs.writeFileSync(
    ownerPath,
    `${JSON.stringify({
      schema: "elastos.browser.vz-socket-owner/v1",
      ...launchIdentity,
    })}\n`,
    { flag: "wx", mode: 0o600 },
  );
}

async function startLocalTcpToUnixBridge(localUnixPath, effect) {
  const server = net.createServer((client) => {
    const upstream = net.connect(localUnixPath);
    bridgeSockets(client, upstream);
  });
  const record = trackLocalServer(server, "", effect);
  await listen(server, 0, "127.0.0.1");
  record.acquired = true;
  const address = server.address();
  if (!address || typeof address !== "object") {
    throw new Error("local relay TCP bridge did not expose a TCP address");
  }
  return address.port;
}

function startTcpRemoteForward(localTcpPort, remoteTcpPort, effect) {
  const child = spawnTracked(sshBin, [
    ...sshBaseArgs(),
    "-N",
    "-R",
    `127.0.0.1:${remoteTcpPort}:127.0.0.1:${localTcpPort}`,
    sshHost,
  ], {
    stdio: ["ignore", "ignore", "pipe"],
  }, effect);
  child.stderr.on("data", (chunk) => {
    process.stderr.write(`[remote-vz tcp-forward] ${chunk}`);
  });
  return child;
}

function startTcpLocalForward(host, port) {
  if (!["127.0.0.1", "::1"].includes(host)) {
    throw new Error("remote VZ TURN local forward requires literal loopback");
  }
  const child = spawnTracked(sshBin, [
    ...sshBaseArgs(),
    "-N",
    "-L",
    `${host}:${port}:${host}:${port}`,
    sshHost,
  ], {
    stdio: ["ignore", "ignore", "pipe"],
  }, "turn_process");
  child.stderr.on("data", (chunk) => {
    process.stderr.write(`[remote-vz turn-forward] ${chunk}`);
  });
  launchEffects.turn_process = true;
  firstEffectOccurred = true;
  return child;
}

function waitForRemoteTcpPort(port) {
  const deadline = Date.now() + socketTimeoutMs;
  let lastError = "";
  const script = [
    "python3 - <<'PY'",
    "import socket",
    `s = socket.create_connection(('127.0.0.1', ${Number(port)}), 2)`,
    "s.close()",
    "PY",
  ].join("\n");
  while (Date.now() < deadline) {
    try {
      runSsh(script, { timeoutMs: 5000 });
      return;
    } catch (error) {
      lastError = error instanceof Error ? error.message : String(error);
      Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 150);
    }
  }
  throw new Error(`remote TCP port did not become reachable on ${sshHost}: ${port}${lastError ? `: ${lastError}` : ""}`);
}

function remoteUnixPipeScript() {
  return [
    "import os, socket, sys",
    "path = os.environ['ELASTOS_BRIDGE_UNIX']",
    "request = sys.stdin.buffer.read()",
    "upstream = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)",
    "upstream.settimeout(30)",
    "def write_all(fd, data):",
    "    view = memoryview(data)",
    "    sent = 0",
    "    while sent < len(view):",
    "        written = os.write(fd, view[sent:])",
    "        if written <= 0:",
    "            raise RuntimeError('stdout write returned no progress')",
    "        sent += written",
    "try:",
    "    upstream.connect(path)",
    "    upstream.sendall(request)",
    "    upstream.shutdown(socket.SHUT_WR)",
    "except Exception as exc:",
    "    print(f'control bridge connect failed: {exc}', file=sys.stderr, flush=True)",
    "    sys.exit(111)",
    "try:",
    "    while True:",
    "        data = upstream.recv(65536)",
    "        if not data:",
    "            break",
    "        write_all(1, data)",
    "except Exception:",
    "    pass",
  ].join("\n");
}

function completeHttpRequestLength(buffer) {
  const headerEnd = buffer.indexOf("\r\n\r\n");
  if (headerEnd < 0) return null;
  const headers = buffer.subarray(0, headerEnd).toString("latin1");
  const contentLengthLine = headers
    .split(/\r\n/)
    .find((line) => line.toLowerCase().startsWith("content-length:"));
  const contentLength = contentLengthLine
    ? Number.parseInt(contentLengthLine.split(":").slice(1).join(":").trim(), 10)
    : 0;
  if (!Number.isFinite(contentLength) || contentLength < 0) {
    throw new Error("control HTTP request Content-Length is invalid");
  }
  return headerEnd + 4 + contentLength;
}

function readHttpRequestFromClient(client) {
  return new Promise((resolve, reject) => {
    const chunks = [];
    let total = 0;
    let settled = false;
    const timer = setTimeout(() => {
      settle(new Error("control HTTP request timed out"));
    }, socketTimeoutMs);
    function settle(value) {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      client.off("data", onData);
      client.off("end", onEnd);
      client.off("error", onError);
      if (value instanceof Error) reject(value);
      else resolve(value);
    }
    function onData(chunk) {
      chunks.push(chunk);
      total += chunk.length;
      if (total > maxControlRequestBytes) {
        settle(new Error("control HTTP request is too large"));
        return;
      }
      const buffer = Buffer.concat(chunks, total);
      let expectedLength;
      try {
        expectedLength = completeHttpRequestLength(buffer);
      } catch (error) {
        settle(error);
        return;
      }
      if (expectedLength !== null && total >= expectedLength) {
        settle(buffer.subarray(0, expectedLength));
      }
    }
    function onEnd() {
      settle(new Error("control HTTP client closed before a complete request"));
    }
    function onError(error) {
      settle(error);
    }
    client.on("data", onData);
    client.on("end", onEnd);
    client.on("error", onError);
  });
}

async function startLocalUnixToRemoteUnixBridge(localUnixPath, remoteUnixPath) {
  const server = net.createServer((client) => {
    readHttpRequestFromClient(client)
      .then((request) => {
        const remoteScript = [
          "set -euo pipefail",
          `export ELASTOS_BRIDGE_UNIX=${shellQuote(remoteUnixPath)}`,
          `exec python3 -u -c ${shellQuote(remoteUnixPipeScript())}`,
        ].join("\n");
        const child = spawnTracked(
          sshBin,
          sshControlRemoteShellArgs(remoteScript),
          {
            stdio: ["pipe", "pipe", "pipe"],
          },
          "control_socket",
        );
        child.stderr.on("data", (chunk) => {
          process.stderr.write(`[remote-vz control-stdio] ${chunk}`);
        });
        child.stdin.on("error", () => {});
        child.stdout.on("error", () => {});
        child.on("error", () => {
          client.destroy();
        });
        child.on("exit", (code) => {
          if (code !== 0) client.destroy();
        });
        client.on("close", () => {
          child.kill("SIGTERM");
        });
        child.stdout.pipe(client);
        child.stdin.end(request);
      })
      .catch(() => {
        client.destroy();
      });
  });
  const record = trackLocalServer(
    server,
    localUnixPath,
    "control_socket",
  );
  await listen(server, localUnixPath);
  record.acquired = true;
  fs.chmodSync(localUnixPath, 0o600);
}

function httpGetUnixSocket(socketPath, requestPath, timeoutMs) {
  return new Promise((resolve, reject) => {
    const chunks = [];
    const socket = net.createConnection({ path: socketPath });
    const timer = setTimeout(() => {
      socket.destroy();
      reject(new Error("control HTTP probe timed out"));
    }, timeoutMs);
    socket.on("connect", () => {
      socket.write(`GET ${requestPath} HTTP/1.1\r\nHost: browser-engine\r\nConnection: close\r\n\r\n`);
    });
    socket.on("data", (chunk) => {
      chunks.push(chunk);
    });
    socket.on("end", () => {
      clearTimeout(timer);
      resolve(Buffer.concat(chunks).toString("utf8"));
    });
    socket.on("error", (error) => {
      clearTimeout(timer);
      reject(error);
    });
  });
}

async function waitForLocalControlHttp(socketPath) {
  const deadline = Date.now() + socketTimeoutMs;
  let lastError = "";
  while (Date.now() < deadline) {
    try {
      const timeoutMs = Math.max(500, Math.min(2500, deadline - Date.now()));
      const response = await httpGetUnixSocket(socketPath, "/status", timeoutMs);
      if (!response.includes("\r\n\r\n")) {
        throw new Error("control response missing HTTP headers");
      }
      if (!response.startsWith("HTTP/1.1 ")) {
        throw new Error(`control response is not HTTP: ${response.slice(0, 80)}`);
      }
      return;
    } catch (error) {
      lastError = error instanceof Error ? error.message : String(error);
      await new Promise((resolve) => setTimeout(resolve, 150));
    }
  }
  throw new Error(`local VM control socket did not return HTTP headers: ${lastError}`);
}

function startRemoteUnixToTcpBridge(
  remoteUnixPath,
  remoteTcpPort,
  remotePidPath,
  effect,
) {
  const remoteScript = [
    "set -euo pipefail",
    "ulimit -n 1024 2>/dev/null || true",
    `export ELASTOS_BRIDGE_UNIX=${shellQuote(remoteUnixPath)}`,
    `export ELASTOS_BRIDGE_PORT=${shellQuote(String(remoteTcpPort))}`,
    `export ELASTOS_BRIDGE_MAX_SESSIONS=${shellQuote(String(remoteRelayMaxSessions))}`,
    `export ELASTOS_BRIDGE_PIDFILE=${shellQuote(remotePidPath)}`,
    "mkdir -p \"$(dirname \"$ELASTOS_BRIDGE_PIDFILE\")\"",
    "bridge_pid=",
    "cleanup_bridge() {",
    "  status=$?",
    "  trap - INT TERM HUP EXIT",
    "  if [ -n \"${bridge_pid:-}\" ]; then",
    "    kill \"$bridge_pid\" 2>/dev/null || true",
    "    sleep 1",
    "    kill -KILL \"$bridge_pid\" 2>/dev/null || true",
    "    wait \"$bridge_pid\" 2>/dev/null || true",
    "  fi",
    "  rm -f \"$ELASTOS_BRIDGE_PIDFILE\" \"$ELASTOS_BRIDGE_UNIX\"",
    "  exit \"$status\"",
    "}",
    "trap cleanup_bridge INT TERM HUP EXIT",
    "python3 -u - <<'PY' &",
    "import os, pathlib, socket, sys, threading",
    "path = os.environ['ELASTOS_BRIDGE_UNIX']",
    "port = int(os.environ['ELASTOS_BRIDGE_PORT'])",
    "max_sessions = max(1, int(os.environ.get('ELASTOS_BRIDGE_MAX_SESSIONS', '32')))",
    "pathlib.Path(path).parent.mkdir(mode=0o700, parents=True, exist_ok=True)",
    "server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)",
    "server.bind(path)",
    "os.chmod(path, 0o600)",
    "server.listen(max_sessions)",
    "print('READY', flush=True)",
    "active = threading.BoundedSemaphore(max_sessions)",
    "def pump(src, dst):",
    "    try:",
    "        while True:",
    "            data = src.recv(65536)",
    "            if not data:",
    "                break",
    "            dst.sendall(data)",
    "    except Exception:",
    "        pass",
    "    try:",
    "        dst.shutdown(socket.SHUT_WR)",
    "    except Exception:",
    "        pass",
    "def close_quietly(sock):",
    "    try:",
    "        sock.close()",
    "    except Exception:",
    "        pass",
    "def handle(client):",
    "    upstream = None",
    "    try:",
    "        upstream = socket.create_connection(('127.0.0.1', port), 10)",
    "    except Exception as exc:",
    "        try:",
    "            client.close()",
    "        finally:",
    "            print(f'bridge connect failed: {exc}', file=sys.stderr, flush=True)",
    "            active.release()",
    "        return",
    "    try:",
    "        to_upstream = threading.Thread(target=pump, args=(client, upstream), daemon=True)",
    "        to_client = threading.Thread(target=pump, args=(upstream, client), daemon=True)",
    "        to_upstream.start()",
    "        to_client.start()",
    "        to_upstream.join()",
    "        to_client.join()",
    "    finally:",
    "        close_quietly(client)",
    "        close_quietly(upstream)",
    "        active.release()",
    "while True:",
    "    active.acquire()",
    "    try:",
    "        conn, _ = server.accept()",
    "    except Exception:",
    "        active.release()",
    "        raise",
    "    threading.Thread(target=handle, args=(conn,), daemon=True).start()",
    "PY",
    "bridge_pid=$!",
    "printf '%s\\n' \"$bridge_pid\" >\"$ELASTOS_BRIDGE_PIDFILE\"",
    "set +e",
    "wait \"$bridge_pid\"",
    "status=$?",
    "set -e",
    "bridge_pid=",
    "rm -f \"$ELASTOS_BRIDGE_PIDFILE\" \"$ELASTOS_BRIDGE_UNIX\"",
    "trap - INT TERM HUP EXIT",
    "exit \"$status\"",
  ].join("\n");
  const child = spawnTracked(sshBin, [
    ...sshRemoteShellArgs(remoteScript),
  ], {
    stdio: ["ignore", "pipe", "pipe"],
  }, effect);
  child.stderr.on("data", (chunk) => {
    process.stderr.write(`[remote-vz relay-shim] ${chunk}`);
  });
  return child;
}

function waitForReadyLine(child, label) {
  return new Promise((resolve, reject) => {
    let stdout = "";
    const timer = setTimeout(() => {
      reject(new Error(`${label} did not report READY within ${socketTimeoutMs}ms`));
    }, socketTimeoutMs);
    child.stdout.on("data", (chunk) => {
      stdout += chunk.toString("utf8");
      if (stdout.split(/\r?\n/).some((line) => line.trim() === "READY")) {
        clearTimeout(timer);
        resolve();
      }
    });
    child.once("exit", (code, signal) => {
      clearTimeout(timer);
      reject(new Error(`${label} exited before READY: ${code ?? signal}`));
    });
    child.once("error", (error) => {
      clearTimeout(timer);
      reject(error);
    });
  });
}

function remoteRelayCleanupCommand(pidPath, socketPath) {
  return [
    `pid_file=${shellQuote(pidPath)}`,
    `socket_path=${shellQuote(socketPath)}`,
    'if [ ! -f "$pid_file" ]; then',
    '  [ ! -e "$socket_path" ] || exit 84',
    "  exit 0",
    "fi",
    'pid=$(cat "$pid_file" 2>/dev/null || true)',
    'case "$pid" in',
    "  ''|*[!0-9]*) exit 85 ;;",
    "esac",
    'proc_command=$(/bin/ps -ww -p "$pid" -o command= 2>/dev/null || true)',
    'case "$proc_command" in',
    '  *"python3 -u -"*)',
    '    kill "$pid" 2>/dev/null || true',
    "    attempts=0",
    '    while kill -0 "$pid" 2>/dev/null; do',
    "      attempts=$((attempts + 1))",
    '      [ "$attempts" -lt 300 ] || exit 86',
    "      sleep 0.1",
    "    done",
    "    ;;",
    "  '') ;;",
    "  *) exit 87 ;;",
    "esac",
    'rm -f "$pid_file"',
    'rm -f "$socket_path"',
  ].join("\n");
}

async function startRemoteRelayTunnel(
  localRelayPath,
  remoteRelayPath,
  remoteRelayPidPath,
  suffix,
  effect,
) {
  cleanupCommands.push(remoteRelayCleanupCommand(remoteRelayPidPath, remoteRelayPath));
  launchEffects[effect] = true;
  firstEffectOccurred = true;
  const localTcpPort = await startLocalTcpToUnixBridge(localRelayPath, effect);
  const remoteTcpPort = portForSuffix(suffix, "relay");
  startTcpRemoteForward(localTcpPort, remoteTcpPort, effect);
  waitForRemoteTcpPort(remoteTcpPort);
  const shim = startRemoteUnixToTcpBridge(
    remoteRelayPath,
    remoteTcpPort,
    remoteRelayPidPath,
    effect,
  );
  await waitForReadyLine(shim, "remote relay Unix shim");
  waitForRemoteSocket(remoteRelayPath);
}

function waitForRemoteSocket(socketPath) {
  const deadline = Date.now() + socketTimeoutMs;
  let lastError = "";
  while (Date.now() < deadline) {
    const result = spawnSync(sshBin, [...sshBaseArgs(), sshHost, "test", "-S", socketPath], {
      encoding: "utf8",
      timeout: 5000,
    });
    if (result.status === 0) return;
    lastError = (result.stderr || result.stdout || "").trim();
    Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 150);
  }
  throw new Error(`remote relay socket did not appear on ${sshHost}: ${socketPath}${lastError ? `: ${lastError}` : ""}`);
}

function remoteSupervisorCleanupCommand(
  pidPath,
  bindingHash,
  nativeSupervisorPath,
) {
  if (!sha256LabelIsSafe(bindingHash)) {
    throw new Error("remote VZ supervisor cleanup requires an exact binding hash");
  }
  validateAbsolutePath(
    nativeSupervisorPath,
    "remote VZ native supervisor cleanup path",
  );
  return [
    `pid_file=${shellQuote(pidPath)}`,
    `native_supervisor=${shellQuote(nativeSupervisorPath)}`,
    `marker_prefix=${shellQuote("--elastos-vz-binding=")}`,
    `binding_hash=${shellQuote(bindingHash)}`,
    'marker="${marker_prefix}${binding_hash}"',
    "terminate_owned_supervisor() {",
    '  pid="$1"',
    '  case "$pid" in',
    "    ''|*[!0-9]*) exit 71 ;;",
    "  esac",
    '  proc_command=$(/bin/ps -ww -p "$pid" -o command= 2>/dev/null || true)',
    '  case "$proc_command" in',
    '    "$native_supervisor "*"$marker"*)',
    '      kill "$pid" 2>/dev/null || true',
    "      attempts=0",
    '      while kill -0 "$pid" 2>/dev/null; do',
    "        attempts=$((attempts + 1))",
    '        [ "$attempts" -lt 300 ] || exit 73',
    "        sleep 0.1",
    "      done",
    "      ;;",
    "    '') ;;",
    "    *) exit 72 ;;",
    "  esac",
    "}",
    'if [ -f "$pid_file" ]; then',
    '  pid=$(cat "$pid_file" 2>/dev/null || true)',
    '  terminate_owned_supervisor "$pid"',
    'fi',
    "process_snapshot=$(/bin/ps -ww -axo pid=,command=)",
    'matching_pids=$(printf \'%s\\n\' "$process_snapshot" | /usr/bin/awk -v marker="$marker" \'index($0, marker) { print $1 }\')',
    'for pid in $matching_pids; do',
    '  terminate_owned_supervisor "$pid"',
    "done",
    'rm -f "$pid_file"',
  ].join("\n");
}

function remoteTransportAbsenceChecks(
  authority,
  paths,
  pidPaths,
  relayTcpPorts,
) {
  validateAbsolutePath(
    paths.remote_session,
    "remote VZ transport session directory",
  );
  const turn = authority.turn;
  const [ordinaryPidPath, mediaPidPath, supervisorPidPath] = pidPaths;
  const [ordinaryRelayPort, mediaRelayPort] = relayTcpPorts;
  const supervisorMarkerPrefix = "--elastos-vz-binding=";
  const supervisorBindingHash = authority.binding_hash;
  const lsofPrelude = "command -v lsof >/dev/null 2>&1";
  const exactCheck = (...lines) =>
    ["set -euo pipefail", ...lines].join("\n");
  const bridgeAbsent = (field, socketPath, pidPath, relayPort) =>
    exactCheck(
      `# elastos_absence_field=${field}`,
      `[ ! -e ${shellQuote(socketPath)} ]`,
      `[ ! -e ${shellQuote(pidPath)} ]`,
      lsofPrelude,
      `! lsof -nP -iTCP:${shellQuote(String(relayPort))} 2>/dev/null | grep -q .`,
      `! lsof -nP -iUDP:${shellQuote(String(relayPort))} 2>/dev/null | grep -q .`,
    );
  return [
    {
      field: "supervisor_child_absent",
      command: exactCheck(
        "# elastos_absence_field=supervisor_child_absent",
        `[ ! -e ${shellQuote(supervisorPidPath)} ]`,
        `marker_prefix=${shellQuote(supervisorMarkerPrefix)}`,
        `binding_hash=${shellQuote(supervisorBindingHash)}`,
        "process_snapshot=$(/bin/ps -ww -axo command=)",
        '! printf \'%s\\n\' "$process_snapshot" | /usr/bin/grep -F -- "${marker_prefix}${binding_hash}" >/dev/null',
      ),
    },
    {
      field: "control_socket_absent",
      command: exactCheck(
        "# elastos_absence_field=control_socket_absent",
        `[ ! -e ${shellQuote(paths.remote_control)} ]`,
      ),
    },
    {
      field: "turn_listener_absent",
      command: exactCheck(
        "# elastos_absence_field=turn_listener_absent",
        lsofPrelude,
        `! lsof -nP -iTCP:${shellQuote(String(turn.listen_port))} -sTCP:LISTEN 2>/dev/null | grep -q .`,
      ),
    },
    {
      field: "turn_relay_ports_absent",
      command: exactCheck(
        "# elastos_absence_field=turn_relay_ports_absent",
        lsofPrelude,
        `port=${shellQuote(String(turn.relay_port_min))}`,
        `relay_max=${shellQuote(String(turn.relay_port_max))}`,
        'while [ "$port" -le "$relay_max" ]; do',
        '  ! lsof -nP -iTCP:"$port" 2>/dev/null | grep -q .',
        '  ! lsof -nP -iUDP:"$port" 2>/dev/null | grep -q .',
        '  port=$((port + 1))',
        "done",
      ),
    },
    {
      field: "ordinary_stream_bridge_absent",
      command: bridgeAbsent(
        "ordinary_stream_bridge_absent",
        authority.egress.runtime_socket_path,
        ordinaryPidPath,
        ordinaryRelayPort,
      ),
    },
    {
      field: "media_stream_bridge_absent",
      command: bridgeAbsent(
        "media_stream_bridge_absent",
        authority.media.runtime_socket_path,
        mediaPidPath,
        mediaRelayPort,
      ),
    },
    {
      field: "session_directory_absent",
      command: exactCheck(
        "# elastos_absence_field=session_directory_absent",
        `[ ! -e ${shellQuote(paths.remote_session)} ]`,
        `[ ! -e ${shellQuote(paths.remote_directory)} ]`,
      ),
    },
  ];
}

function appendBoundedStderr(child, chunk) {
  child.elastosStderrTail = `${child.elastosStderrTail || ""}${chunk.toString("utf8")}`;
  if (child.elastosStderrTail.length > 64 * 1024) {
    child.elastosStderrTail = child.elastosStderrTail.slice(-64 * 1024);
  }
}

function traceRemoteEgressEnabled() {
  return /^(1|true|TRUE|yes|YES)$/.test(String(process.env.ELASTOS_BROWSER_VM_TRACE_EGRESS || ""));
}

function filterSupervisorStderr(child, chunk) {
  const text = `${child.elastosStderrRemainder || ""}${chunk.toString("utf8")}`;
  const lines = text.split(/\r?\n/);
  child.elastosStderrRemainder = lines.pop() || "";
  const filtered = lines.filter((line) => {
    const normalized = line.replace(/^\[remote-vz supervisor\]\s*/, "");
    return !/^Browser VM host egress bridge (accepted session|session) \d+/.test(normalized);
  });
  return filtered.length ? `${filtered.join("\n")}\n` : "";
}

function errorWithSupervisorTail(message, child) {
  const tail = String(child?.elastosStderrTail || "").trim();
  const error = new Error(tail ? `${message}: ${tail.slice(-20000)}` : message);
  const settlement = parseVzLaunchSettlement(tail);
  if (settlement) {
    error.vz_launch_settlement = settlement;
    if (settlementMatchesLaunchIdentity(settlement)) {
      if (settlement.effects.vm === true) {
        launchEffects.vm = true;
      }
      if (settlement.absence.route_absent === true) {
        routeAbsenceProved = true;
      }
      if (settlement.absence.vm_absent === true) {
        nativeVmAbsenceProved = true;
      }
    }
  }
  return error;
}

function settlementMatchesLaunchIdentity(settlement) {
  const effectKeys = [
    "session_directory",
    "control_socket",
    "ordinary_stream_bridge",
    "media_stream_bridge",
    "turn_process",
    "supervisor_child",
    "vm",
  ];
  const absenceKeys = [
    "child_absent",
    "supervisor_child_absent",
    "control_socket_absent",
    "route_absent",
    "turn_listener_absent",
    "turn_relay_ports_absent",
    "ordinary_stream_bridge_absent",
    "media_stream_bridge_absent",
    "session_directory_absent",
    "vm_absent",
  ];
  const shapeAndBindingMatch =
    launchIdentity &&
    exactObjectKeys(settlement, [
      "schema",
      "state",
      "message",
      "binding_hash",
      "generation",
      "page_id",
      "vm_id",
      "stream_id",
      "media_stream_id",
      "effects",
      "absence",
    ]) &&
    settlement.schema === VZ_LAUNCH_SETTLEMENT_SCHEMA &&
    ["did_not_act", "cleanup_pending", "terminal_post_effect_cleanup"].includes(
      settlement.state,
    ) &&
    typeof settlement.message === "string" &&
    settlement.message.length <= 8_192 &&
    settlement.binding_hash === launchIdentity.binding_hash &&
    settlement.generation === launchIdentity.generation &&
    settlement.page_id === launchIdentity.page_id &&
    settlement.vm_id === launchIdentity.vm_id &&
    settlement.stream_id === launchIdentity.stream_id &&
    settlement.media_stream_id === launchIdentity.media_stream_id &&
    exactObjectKeys(settlement.effects, effectKeys) &&
    effectKeys.every((key) => typeof settlement.effects[key] === "boolean") &&
    exactObjectKeys(settlement.absence, absenceKeys) &&
    absenceKeys.every((key) => typeof settlement.absence[key] === "boolean");
  if (!shapeAndBindingMatch) return false;
  const terminalAbsence = absenceKeys.every(
    (key) => settlement.absence[key] === true,
  );
  const anyEffect = effectKeys.some(
    (key) => settlement.effects[key] === true,
  );
  return (
    (settlement.state === "did_not_act" &&
      !anyEffect &&
      terminalAbsence) ||
    (settlement.state === "terminal_post_effect_cleanup" &&
      anyEffect &&
      terminalAbsence) ||
    (settlement.state === "cleanup_pending" && !terminalAbsence)
  );
}

function parseVzLaunchSettlement(text) {
  for (const line of String(text || "").split(/\r?\n/).reverse()) {
    const candidate = line
      .replace(/^\[remote-vz supervisor\]\s*/, "")
      .trim();
    if (!candidate.startsWith("{")) continue;
    try {
      const parsed = JSON.parse(candidate);
      if (
        parsed?.schema === VZ_LAUNCH_SETTLEMENT_SCHEMA &&
        ["did_not_act", "cleanup_pending", "terminal_post_effect_cleanup"].includes(
          parsed.state,
        )
      ) {
        return parsed;
      }
    } catch {}
  }
  return null;
}

function remoteControlReadyTimeoutMs() {
  const explicit = process.env.ELASTOS_BROWSER_VM_CONTROL_READY_TIMEOUT_MS;
  if (explicit != null && explicit !== "") return explicit;
  return String(Math.max(1_000, Math.min(120_000, launchTimeoutMs - 30_000)));
}

function remoteDebugHoldOnOpenErrorMs() {
  const requested = Number(process.env.ELASTOS_BROWSER_VM_DEBUG_HOLD_ON_OPEN_ERROR_MS || "0");
  if (!Number.isFinite(requested) || requested <= 0) return "0";
  const readyTimeoutMs = Number(remoteControlReadyTimeoutMs());
  const remainingLaunchMarginMs = Number.isFinite(readyTimeoutMs)
    ? launchTimeoutMs - readyTimeoutMs - 5_000
    : 0;
  return String(Math.max(0, Math.min(requested, remainingLaunchMarginMs)));
}

async function startRemoteSupervisor(
  remoteRequest,
  remoteVmEnv,
  supervisorPidPath,
  remoteDataDir,
) {
  const serialized = `${JSON.stringify(remoteRequest)}\n`;
  if (Buffer.byteLength(serialized) > maxControlRequestBytes) {
    throw new Error(
      `remote VZ serialized request exceeds ${maxControlRequestBytes} bytes`,
    );
  }
  const remoteDataDirExpr = process.env.ELASTOS_BROWSER_REMOTE_VZ_DATA_DIR
    ? shellQuote(process.env.ELASTOS_BROWSER_REMOTE_VZ_DATA_DIR)
    : '"$HOME/Library/Application Support/elastos"';
  const bindingHash =
    remoteRequest.launch_request.transport_authority.binding_hash;
  const nativeSupervisorPath = path.posix.join(
    remoteDataDir,
    "bin/browser-vz-engine-supervisor",
  );
  cleanupCommands.push(
    remoteSupervisorCleanupCommand(
      supervisorPidPath,
      bindingHash,
      nativeSupervisorPath,
    ),
  );
  const remoteScript = [
    "set -euo pipefail",
    "umask 077",
    `DATA=${remoteDataDirExpr}`,
    `NATIVE_SUPERVISOR=${shellQuote(nativeSupervisorPath)}`,
    `PIDFILE=${shellQuote(supervisorPidPath)}`,
    `mkdir -p "$(dirname "$PIDFILE")"`,
    "REQUEST_FILE=$(mktemp \"${TMPDIR:-/tmp}/elastos-remote-vz-request.XXXXXX\")",
    "trap 'rm -f \"$PIDFILE\" \"$REQUEST_FILE\"' INT TERM HUP EXIT",
    "cat >\"$REQUEST_FILE\"",
    `export ELASTOS_BROWSER_VM_DATA_DIR="$DATA"`,
    `export ELASTOS_BROWSER_VM_ROOT=${shellQuote(remoteRoot)}`,
    `export ELASTOS_BROWSER_VM_SOCKET_ROOT=${shellQuote(remoteSocketRoot)}`,
    `export ELASTOS_BROWSER_VM_MEMORY_MIB=${shellQuote(process.env.ELASTOS_BROWSER_REMOTE_VZ_MEMORY_MIB || "2048")}`,
    `export ELASTOS_BROWSER_VM_VCPUS=${shellQuote(process.env.ELASTOS_BROWSER_REMOTE_VZ_VCPUS || "2")}`,
    `export ELASTOS_BROWSER_VM_CONTROL_READY_TIMEOUT_MS=${shellQuote(remoteControlReadyTimeoutMs())}`,
    `export ELASTOS_BROWSER_VM_CONTROL_PROXY_REQUEST_TIMEOUT_MS=${shellQuote(process.env.ELASTOS_BROWSER_VM_CONTROL_PROXY_REQUEST_TIMEOUT_MS || defaultControlProxyRequestTimeoutMs)}`,
    `export ELASTOS_BROWSER_VM_CONTROL_STATUS_PROBE_TIMEOUT_MS=${shellQuote(process.env.ELASTOS_BROWSER_VM_CONTROL_STATUS_PROBE_TIMEOUT_MS || "3000")}`,
    `export ELASTOS_BROWSER_VM_DEBUG_HOLD_ON_OPEN_ERROR_MS=${shellQuote(remoteDebugHoldOnOpenErrorMs())}`,
    `export ELASTOS_BROWSER_VM_EGRESS_MAX_SESSIONS=${shellQuote(process.env.ELASTOS_BROWSER_VM_EGRESS_MAX_SESSIONS || String(remoteRelayMaxSessions))}`,
    `export ELASTOS_BROWSER_VM_VSOCK_ATTEMPT_TIMEOUT_MS=${shellQuote(process.env.ELASTOS_BROWSER_VM_VSOCK_ATTEMPT_TIMEOUT_MS || "1000")}`,
    `export ELASTOS_BROWSER_VM_TRACE=${shellQuote(process.env.ELASTOS_BROWSER_VM_TRACE || "1")}`,
    `export ELASTOS_BROWSER_VM_TRACE_EGRESS=${shellQuote(process.env.ELASTOS_BROWSER_VM_TRACE_EGRESS || "0")}`,
    ...optionalRemoteEnvExports(
      vmLauncherEnvKeys,
      remoteVmEnv,
    ),
    "supervisor_pid=",
    "cleanup_supervisor() {",
    "  status=$?",
    "  trap - INT TERM HUP EXIT",
    "  if [ -n \"${supervisor_pid:-}\" ]; then",
    "    kill \"$supervisor_pid\" 2>/dev/null || true",
    "    attempts=0",
    "    while kill -0 \"$supervisor_pid\" 2>/dev/null; do",
    "      attempts=$((attempts + 1))",
    "      if [ \"$attempts\" -ge 300 ]; then",
    "        exit 73",
    "      fi",
    "      sleep 0.1",
    "    done",
    "    wait \"$supervisor_pid\" 2>/dev/null || true",
    "  fi",
    "  rm -f \"$PIDFILE\" \"$REQUEST_FILE\"",
    "  exit \"$status\"",
    "}",
    "trap cleanup_supervisor INT TERM HUP EXIT",
    `"$NATIVE_SUPERVISOR" ${shellQuote(`--elastos-vz-binding=${bindingHash}`)} <"$REQUEST_FILE" &`,
    "supervisor_pid=$!",
    "printf '%s\\n' \"$supervisor_pid\" >\"$PIDFILE\"",
    "rm -f \"$REQUEST_FILE\"",
    "set +e",
    "wait \"$supervisor_pid\"",
    "status=$?",
    "set -e",
    "supervisor_pid=",
    "rm -f \"$PIDFILE\"",
    "trap - INT TERM HUP EXIT",
    "exit \"$status\"",
  ].join("\n");
  const child = spawnTracked(
    sshBin,
    sshRemoteShellArgs(remoteScript),
    {
      stdio: ["pipe", "pipe", "pipe"],
      env: process.env,
    },
    "supervisor_child",
  );
  remoteSupervisorChild = child;
  child.elastosStderrTail = "";
  child.elastosStderrRemainder = "";
  child.stderr.on("data", (chunk) => {
    const output = traceRemoteEgressEnabled()
      ? chunk.toString("utf8")
      : filterSupervisorStderr(child, chunk);
    if (!output) return;
    appendBoundedStderr(child, output);
    process.stderr.write(`[remote-vz supervisor] ${output}`);
  });
  launchEffects.supervisor_child = true;
  launchEffects.vm = true;
  firstEffectOccurred = true;
  try {
    await new Promise((resolve, reject) => {
      child.stdin.once("error", reject);
      child.stdin.once("finish", resolve);
      child.stdin.end(serialized);
    });
  } catch (error) {
    await terminateAndReapChild(child);
    throw new Error(
      `remote VZ private stdin write failed: ${
        error instanceof Error ? error.message : String(error)
      }`,
    );
  }
  return child;
}

function readFirstJsonLine(child) {
  return new Promise((resolve, reject) => {
    let stdout = "";
    let settled = false;
    const timer = setTimeout(() => {
      try {
        child.kill("SIGTERM");
      } catch {}
      settle(errorWithSupervisorTail(
        `remote VZ supervisor did not return a launch result within ${launchTimeoutMs}ms`,
        child,
      ));
    }, launchTimeoutMs);
    function settle(value) {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      if (value instanceof Error) reject(value);
      else resolve(value);
    }
    child.stdout.on("data", (chunk) => {
      stdout += chunk.toString("utf8");
      const lines = stdout.split(/\r?\n/).filter((line) => line.trim());
      if (!lines.length) return;
      try {
        settle(JSON.parse(lines[0]));
      } catch (error) {
        settle(new Error(`remote VZ supervisor returned non-JSON: ${lines[0]}`));
      }
    });
    child.on("error", settle);
    child.on("exit", (code, signal) => {
      if (!settled) settle(errorWithSupervisorTail(
        `remote VZ supervisor exited before launch result: ${code ?? signal}`,
        child,
      ));
    });
  });
}

async function startControlTunnel(localControlPath, remoteControlPath) {
  validateAbsolutePath(remoteControlPath, "remote supervisor control_socket_path");
  validateUnixSocketPathBudget(remoteControlPath, "remote control socket path");
  launchEffects.control_socket = true;
  firstEffectOccurred = true;
  await startLocalUnixToRemoteUnixBridge(localControlPath, remoteControlPath);
  await waitForLocalControlHttp(localControlPath);
  cleanupCommands.push(`rm -f ${shellQuote(remoteControlPath)}`);
}

function rewriteResult(result, launch, localSessionDir, localControlPath) {
  return {
    ...result,
    adapter: launch.adapter,
    engine: launch.engine,
    stream_id: launch.stream_id,
    control_socket_path: localControlPath,
    isolated_session: true,
    isolation: {
      schema: "elastos.browser.engine.isolation/v1",
      kind: "per_launch_vm_target",
      session_dir: localSessionDir,
    },
  };
}

function validateRemoteTransportResult(result, authority) {
  const receipt = result?.transport_receipt;
  const effects = receipt?.effects;
  const effectKeys = [
    "vz_network_devices_zero",
    "guest_bootstrap_validated",
    "guest_loopback_only",
    "guest_interfaces",
    "guest_default_route_absent",
    "guest_direct_network_absent",
    "ordinary_stream_fixed_target",
    "media_stream_fixed_target",
    "turn_launch_owned",
    "turn_listener_loopback",
    "hibernation_disabled",
  ];
  if (
    result?.page_id !== authority.page_id ||
    result?.vm_id !== authority.vm_id ||
    result?.stream_id !== authority.egress.stream_id ||
    JSON.stringify(canonicalJson(result?.transport_authority)) !==
      JSON.stringify(canonicalJson(authority)) ||
    !exactObjectKeys(receipt, [
      "schema",
      "binding_hash",
      "generation",
      "page_id",
      "vm_id",
      "expires_at_unix_ms",
      "terminal",
      "effects",
    ]) ||
    receipt.schema !== "elastos.browser.vz-transport-effect-receipt/v1" ||
    receipt.binding_hash !== authority.binding_hash ||
    receipt.generation !== authority.generation ||
    receipt.page_id !== authority.page_id ||
    receipt.vm_id !== authority.vm_id ||
    receipt.expires_at_unix_ms !== authority.expires_at_unix_ms ||
    receipt.terminal !== true ||
    !exactObjectKeys(effects, effectKeys) ||
    JSON.stringify(effects.guest_interfaces) !== JSON.stringify(["lo"]) ||
    effectKeys
      .filter((key) => key !== "guest_interfaces")
      .some((key) => effects[key] !== true) ||
    valueContainsTransportSecret(result)
  ) {
    throw new Error(
      "remote VZ supervisor returned an invalid exact transport effect receipt",
    );
  }
  routeAbsenceProved = true;
  return result;
}

function valueContainsTransportSecret(value) {
  if (Array.isArray(value)) {
    return value.some(valueContainsTransportSecret);
  }
  if (!value || typeof value !== "object") return false;
  return Object.entries(value).some(
    ([key, entry]) =>
      ["credential", "auth_secret", "transport_secret"].includes(key) ||
      valueContainsTransportSecret(entry),
  );
}

function childExited(child) {
  return child.exitCode != null || child.signalCode != null;
}

function waitForChildExit(child, timeoutMs) {
  if (childExited(child)) return Promise.resolve(true);
  return new Promise((resolve) => {
    let settled = false;
    const timer = setTimeout(() => settle(false), timeoutMs);
    const onExit = () => settle(true);
    function settle(exited) {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      child.off("exit", onExit);
      resolve(exited && childExited(child));
    }
    child.once("exit", onExit);
  });
}

async function terminateAndReapChild(child) {
  if (childExited(child)) return true;
  try {
    child.kill("SIGTERM");
  } catch {}
  if (await waitForChildExit(child, 2_000)) return true;
  try {
    child.kill("SIGKILL");
  } catch {}
  return waitForChildExit(child, 5_000);
}

async function closeTrackedServer(record) {
  if (record.closed && !record.server.listening) return true;
  if (!record.acquired && !record.server.listening) {
    if (record.socketPath) {
      try {
        fs.unlinkSync(record.socketPath);
      } catch (error) {
        if (error?.code !== "ENOENT") return false;
      }
    }
    return !record.socketPath || !fs.existsSync(record.socketPath);
  }
  const closed = new Promise((resolve) => {
    try {
      record.server.close((error) => resolve(!error));
      record.server.closeAllConnections?.();
    } catch {
      resolve(false);
    }
  });
  const callbackOk = await Promise.race([
    closed,
    new Promise((resolve) => setTimeout(() => resolve(false), 5_000)),
  ]);
  if (record.socketPath) {
    try {
      fs.unlinkSync(record.socketPath);
    } catch (error) {
      if (error?.code !== "ENOENT") return false;
    }
  }
  return (
    callbackOk &&
    record.closed &&
    !record.server.listening &&
    (!record.socketPath || !fs.existsSync(record.socketPath))
  );
}

async function tcpPortIsAvailable(host, port) {
  try {
    await assertTcpPortAvailable(host, port, "cleanup port absence proof");
    return true;
  } catch {
    return false;
  }
}

function cleanupOwnedResources() {
  if (!cleanupPromise) {
    cleanupPromise = performOwnedResourceCleanup();
  }
  return cleanupPromise;
}

async function performOwnedResourceCleanup() {
  const serverAbsence = new Map();
  for (const record of localServers) {
    serverAbsence.set(record, await closeTrackedServer(record));
  }
  for (const command of [...cleanupCommands].reverse()) {
    try {
      runSsh(command, { timeoutMs: 35_000 });
    } catch (error) {
      process.stderr.write(
        `[remote-vz cleanup] exact remote cleanup failed: ${
          error instanceof Error ? error.message : String(error)
        }\n`,
      );
    }
  }
  const childAbsence = new Map();
  await Promise.all(
    Array.from(children, async (child) => {
      childAbsence.set(child, await terminateAndReapChild(child));
    }),
  );
  if (
    remoteSupervisorChild &&
    childAbsence.get(remoteSupervisorChild) === true
  ) {
    const settlement = parseVzLaunchSettlement(
      remoteSupervisorChild.elastosStderrTail,
    );
    if (settlement && settlementMatchesLaunchIdentity(settlement)) {
      routeAbsenceProved ||= settlement.absence.route_absent === true;
      nativeVmAbsenceProved ||= settlement.absence.vm_absent === true;
    } else if (remoteSupervisorChild.exitCode === 0) {
      // The bound native supervisor exits zero only after its normal cleanup
      // has returned an all-true absence result.
      routeAbsenceProved = true;
      nativeVmAbsenceProved = true;
    }
  }
  for (const directory of localCleanupDirs) {
    try {
      fs.rmSync(directory, { recursive: true, force: true });
    } catch {}
  }
  const remoteAbsence = new Map();
  for (const { field, command } of remoteAbsenceChecks) {
    try {
      runSsh(command, { timeoutMs: 35_000 });
      remoteAbsence.set(field, true);
    } catch (error) {
      remoteAbsence.set(field, false);
      process.stderr.write(
        `[remote-vz cleanup] ${field} proof failed: ${
          error instanceof Error ? error.message : String(error)
        }\n`,
      );
    }
  }
  const childrenForEffectAbsent = (effect) =>
    Array.from(children)
      .filter((child) => child.elastosEffect === effect)
      .every((child) => childAbsence.get(child) === true && childExited(child));
  const serversForEffectAbsent = (effect) =>
    Array.from(localServers)
      .filter((record) => record.effect === effect)
      .every((record) => serverAbsence.get(record) === true);
  const childAbsent = Array.from(children).every(
    (child) => childAbsence.get(child) === true && childExited(child),
  );
  const localSessionDirectoryAbsent = Array.from(localCleanupDirs).every(
    (directory) => !fs.existsSync(directory),
  );
  const supervisorChildAbsent =
    childrenForEffectAbsent("supervisor_child") &&
    remoteAbsence.get("supervisor_child_absent") === true;
  const controlSocketAbsent =
    childrenForEffectAbsent("control_socket") &&
    serversForEffectAbsent("control_socket") &&
    remoteAbsence.get("control_socket_absent") === true;
  const turnListenerAbsent =
    childrenForEffectAbsent("turn_process") &&
    remoteAbsence.get("turn_listener_absent") === true &&
    (await tcpPortIsAvailable(
      localTurnEndpoint.host,
      localTurnEndpoint.port,
    ));
  const ordinaryStreamBridgeAbsent =
    childrenForEffectAbsent("ordinary_stream_bridge") &&
    serversForEffectAbsent("ordinary_stream_bridge") &&
    remoteAbsence.get("ordinary_stream_bridge_absent") === true;
  const mediaStreamBridgeAbsent =
    childrenForEffectAbsent("media_stream_bridge") &&
    serversForEffectAbsent("media_stream_bridge") &&
    remoteAbsence.get("media_stream_bridge_absent") === true;
  const sessionDirectoryAbsent =
    localSessionDirectoryAbsent &&
    remoteAbsence.get("session_directory_absent") === true;
  const routeAbsent =
    (!launchEffects.supervisor_child || routeAbsenceProved) &&
    supervisorChildAbsent;
  const vmAbsent =
    (!launchEffects.vm || nativeVmAbsenceProved) &&
    supervisorChildAbsent;
  return {
    child_absent: childAbsent,
    supervisor_child_absent: supervisorChildAbsent,
    control_socket_absent: controlSocketAbsent,
    route_absent: routeAbsent,
    turn_listener_absent: turnListenerAbsent,
    turn_relay_ports_absent:
      remoteAbsence.get("turn_relay_ports_absent") === true,
    ordinary_stream_bridge_absent: ordinaryStreamBridgeAbsent,
    media_stream_bridge_absent: mediaStreamBridgeAbsent,
    session_directory_absent: sessionDirectoryAbsent,
    vm_absent: vmAbsent,
  };
}

function absenceIsTerminal(absence) {
  return (
    absence &&
    Object.values(absence).length > 0 &&
    Object.values(absence).every((value) => value === true)
  );
}

function launchSettlement(state, message, absence) {
  if (!launchIdentity) {
    throw new Error(
      "cannot emit an exact Browser VZ launch settlement before identity validation",
    );
  }
  return {
    schema: VZ_LAUNCH_SETTLEMENT_SCHEMA,
    state,
    message: String(message || "Browser VZ launch failed").slice(0, 8192),
    binding_hash: launchIdentity.binding_hash,
    generation: launchIdentity.generation,
    page_id: launchIdentity.page_id,
    vm_id: launchIdentity.vm_id,
    stream_id: launchIdentity.stream_id,
    media_stream_id: launchIdentity.media_stream_id,
    effects: { ...launchEffects },
    absence,
  };
}

async function cleanupAndExit(signal, error = null) {
  const preEffect = !firstEffectOccurred;
  const absence = preEffect
    ? {
        child_absent: true,
        supervisor_child_absent: true,
        control_socket_absent: true,
        route_absent: true,
        turn_listener_absent: true,
        turn_relay_ports_absent: true,
        ordinary_stream_bridge_absent: true,
        media_stream_bridge_absent: true,
        session_directory_absent: true,
        vm_absent: true,
      }
    : await cleanupOwnedResources();
  if (!error) {
    process.exit(absenceIsTerminal(absence) ? 0 : 1);
  }
  if (preEffect && !launchIdentity) {
    const message = String(
      error instanceof Error ? error.message : error,
    ).slice(0, 8192);
    process.stderr.write(
      `Browser VZ launch validation failed before exact identity validation: ${message}\n`,
      () => process.exit(1),
    );
    return;
  }
  const state = preEffect
    ? "did_not_act"
    : absenceIsTerminal(absence)
      ? "terminal_post_effect_cleanup"
      : "cleanup_pending";
  const settlement = launchSettlement(
    state,
    error instanceof Error ? error.message : String(error),
    absence,
  );
  process.stderr.write(`${JSON.stringify(settlement)}\n`, () => process.exit(1));
}

async function main() {
  const request = readOpenRequest();
  const launch = validateOpenRequest(request);
  sshBin = resolveExecutable(configuredSshBin, "remote VZ SSH client");
  if (!sshHost || /[\r\n\0]/.test(sshHost)) {
    throw new Error("ELASTOS_BROWSER_REMOTE_VZ_SSH must name the configured remote macOS Browser Engine SSH target");
  }
  rejectLegacyVzConfiguration();
  validateAbsolutePath(localRoot, "ELASTOS_BROWSER_REMOTE_VZ_LOCAL_ROOT");
  validateAbsolutePath(remoteRoot, "ELASTOS_BROWSER_REMOTE_VZ_ROOT");
  validateAbsolutePath(
    localSocketRoot,
    "ELASTOS_BROWSER_REMOTE_VZ_SOCKET_ROOT",
  );
  validateAbsolutePath(
    remoteSocketRoot,
    "ELASTOS_BROWSER_REMOTE_VZ_REMOTE_SOCKET_ROOT",
  );
  if (!Number.isInteger(remoteRelayMaxSessions) || remoteRelayMaxSessions < 1 || remoteRelayMaxSessions > 256) {
    throw new Error("ELASTOS_BROWSER_REMOTE_VZ_RELAY_MAX_SESSIONS must be an integer from 1 to 256");
  }

  const remoteDataDir = resolveRemoteDataDir();
  const transportAuthority = launch.transport_authority;
  const remoteVmEnv = remoteLaunchTurnProgramEnv();
  const suffix = `bvm-${bindingDigest(transportAuthority)}`;
  const socketPaths = boundSocketPaths(transportAuthority);
  const localSessionDir = path.join(localRoot, suffix);
  localCleanupDirs.add(localSessionDir);
  const localControlPath = socketPaths.local_control;
  const remoteRelayPath = transportAuthority.egress.runtime_socket_path;
  const remoteRelayPidPath = path.join(remoteRoot, `relay-${suffix}.pid`);
  const remoteMediaRelayPath = transportAuthority.media.runtime_socket_path;
  const remoteMediaRelayPidPath = path.join(
    remoteRoot,
    `media-relay-${suffix}.pid`,
  );
  const remoteSupervisorPidPath = path.join(remoteRoot, `supervisor-${suffix}.pid`);
  const relayTcpPorts = [
    portForSuffix(suffix, "relay"),
    portForSuffix(`${suffix}-media`, "relay"),
  ];
  const pidPaths = [
    remoteRelayPidPath,
    remoteMediaRelayPidPath,
    remoteSupervisorPidPath,
  ];
  validateUnixSocketPathBudget(localControlPath, "local control socket path");
  validateUnixSocketPathBudget(remoteRelayPath, "remote relay socket path");
  validateUnixSocketPathBudget(
    remoteMediaRelayPath,
    "remote media relay socket path",
  );

  await preflightLaunch({
    remoteDataDir,
    remoteVmEnv,
    paths: socketPaths,
    authority: transportAuthority,
    relayTcpPorts,
    pidPaths,
    localSessionDir,
  });
  remoteAbsenceChecks.push(
    ...remoteTransportAbsenceChecks(
      transportAuthority,
      socketPaths,
      pidPaths,
      relayTcpPorts,
    ),
  );

  createLocalBindingDirectory(socketPaths);
  fs.mkdirSync(localSessionDir, { mode: 0o700 });
  launchEffects.session_directory = true;
  await startRemoteRelayTunnel(
    launch.adapter_ipc.runtime_stream_path,
    remoteRelayPath,
    remoteRelayPidPath,
    suffix,
    "ordinary_stream_bridge",
  );
  await startRemoteRelayTunnel(
    transportAuthority.media.runtime_socket_path,
    remoteMediaRelayPath,
    remoteMediaRelayPidPath,
    `${suffix}-media`,
    "media_stream_bridge",
  );
  startTcpLocalForward(
    transportAuthority.turn.listen_host,
    transportAuthority.turn.listen_port,
  );

  const remoteRequest = cloneForRemote(request, remoteDataDir);
  const remoteSupervisor = await startRemoteSupervisor(
    remoteRequest,
    remoteVmEnv,
    remoteSupervisorPidPath,
    remoteDataDir,
  );
  const remoteResult = validateRemoteTransportResult(
    await readFirstJsonLine(remoteSupervisor),
    transportAuthority,
  );
  const remoteControlPath = remoteResult.control_socket_path;
  const remoteSessionDir = remoteResult.isolation?.session_dir;
  if (
    remoteControlPath !== socketPaths.remote_control ||
    remoteSessionDir !== socketPaths.remote_session
  ) {
    throw new Error(
      "remote VZ supervisor returned a substituted socket or session binding",
    );
  }
  launchEffects.session_directory = true;

  await startControlTunnel(localControlPath, remoteControlPath);

  const result = rewriteResult(
    remoteResult,
    launch,
    localSessionDir,
    localControlPath,
  );
  process.stdout.write(`${JSON.stringify(result)}\n`);
}

export {
  boundSocketPaths,
  parseVzLaunchSettlement,
  remoteRelayCleanupCommand,
  remoteSupervisorCleanupCommand,
  remoteTransportAbsenceChecks,
  remotePreflightCommand,
  settlementMatchesLaunchIdentity,
  validateOpenRequest,
  validateRemoteTransportResult,
  validateVzTransportAuthority,
  validateVzTransportSecret,
};

const invokedDirectly =
  process.argv[1] &&
  import.meta.url === pathToFileURL(fs.realpathSync(process.argv[1])).href;
if (invokedDirectly) {
  process.on("SIGTERM", () => cleanupAndExit("SIGTERM"));
  process.on("SIGINT", () => cleanupAndExit("SIGINT"));
  process.on("SIGHUP", () => cleanupAndExit("SIGHUP"));
  main().catch((error) => {
    void cleanupAndExit(null, error);
  });
}
