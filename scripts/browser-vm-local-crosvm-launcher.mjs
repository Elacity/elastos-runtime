#!/usr/bin/env node
import crypto from "node:crypto";
import fs from "node:fs";
import net from "node:net";
import os from "node:os";
import path from "node:path";
import process from "node:process";
import { spawn, spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const OPEN_REQUEST_ENV = "ELASTOS_BROWSER_VM_OPEN_REQUEST";
const DATA_DIR_ENV = "ELASTOS_BROWSER_VM_DATA_DIR";
const ROOT_ENV = "ELASTOS_BROWSER_VM_ROOT";
const PROFILE_ROOT_ENV = "ELASTOS_BROWSER_PROFILE_ROOT";
const SESSION_KEEP_ENV = "ELASTOS_BROWSER_VM_KEEP_SESSIONS";
const ROOTFS_POOL_DIR_ENV = "ELASTOS_BROWSER_VM_ROOTFS_POOL_DIR";
const ROOTFS_COPY_MODE_ENV = "ELASTOS_BROWSER_VM_ROOTFS_COPY_MODE";
const ROOTFS_POOL_REFILL_COUNT_ENV = "ELASTOS_BROWSER_VM_ROOTFS_POOL_REFILL_COUNT";
const ROOTFS_POOL_REFILL_MIN_FREE_MIB_ENV = "ELASTOS_BROWSER_VM_ROOTFS_POOL_REFILL_MIN_FREE_MIB";
const ROOTFS_POOL_REFILL_SCRIPT_ENV = "ELASTOS_BROWSER_VM_ROOTFS_POOL_REFILL_SCRIPT";
const SESSION_KEEP_ROOTFS_ENV = "ELASTOS_BROWSER_VM_KEEP_SESSION_ROOTFS";
const TURNSERVER_BIN_ENV = "ELASTOS_BROWSER_VM_TURNSERVER_BIN";
const TURN_RELAY_MIN_PORT_ENV = "ELASTOS_BROWSER_VM_TURN_RELAY_MIN_PORT";
const TURN_RELAY_MAX_PORT_ENV = "ELASTOS_BROWSER_VM_TURN_RELAY_MAX_PORT";
const VM_ICE_ENV_KEYS = [
  "ELASTOS_BROWSER_VM_ICE_SERVER",
  "ELASTOS_BROWSER_VM_ICE_SERVERS_JSON",
  "ELASTOS_BROWSER_VM_ICE_USERNAME",
  "ELASTOS_BROWSER_VM_ICE_CREDENTIAL",
  "ELASTOS_BROWSER_VM_ICE_TRANSPORT_POLICY",
  "ELASTOS_BROWSER_VM_MEDIA_RELAY_HOST_IPV4",
  "ELASTOS_BROWSER_VM_MEDIA_RELAY_GUEST_IPV4",
  "ELASTOS_BROWSER_VM_MEDIA_RELAY_PREFIX",
];

const children = new Set();
const servers = new Set();
const cleanupFns = [];
let exiting = false;
let launchSucceeded = false;
const launchedAtMs = Date.now();

function logPhase(message) {
  process.stderr.write(`[local-crosvm +${Date.now() - launchedAtMs}ms] ${message}\n`);
}

function fail(message) {
  console.error(message);
  process.exit(1);
}

function safeId(value) {
  return typeof value === "string" && /^[A-Za-z0-9:_-]+$/.test(value);
}

function validateAbsolutePath(value, label) {
  if (typeof value !== "string" || !value.startsWith("/") || /[\r\n\0]/.test(value)) {
    throw new Error(`${label} must be an absolute path without control characters`);
  }
}

function readOpenRequest() {
  const stdin = fs.readFileSync(0, "utf8");
  const raw = stdin.trim() ? stdin : process.env[OPEN_REQUEST_ENV] || "";
  if (!raw.trim()) fail(`${OPEN_REQUEST_ENV} or stdin JSON is required`);
  try {
    return JSON.parse(raw);
  } catch (error) {
    fail(`Browser VM local crosvm request is invalid JSON: ${error instanceof Error ? error.message : String(error)}`);
  }
}

function validateOpenRequest(request) {
  if (request.schema !== "elastos.browser.vm-engine.open/v1") {
    throw new Error("local crosvm launcher requires elastos.browser.vm-engine.open/v1");
  }
  const launch = request.launch_request || {};
  if (launch.schema !== "elastos.browser.engine.launch-request/v1") {
    throw new Error("local crosvm launcher missing Browser launch_request");
  }
  if (!safeId(launch.stream_id) || !safeId(launch.adapter)) {
    throw new Error("launch_request adapter and stream_id must be safe identifiers");
  }
  if (launch.engine !== "chromium_microvm") {
    throw new Error("local crosvm launcher accepts only chromium_microvm");
  }
  if (launch.display_mode !== "webrtc_remote_display") {
    throw new Error("local crosvm launcher requires webrtc_remote_display");
  }
  if (launch.network_mode !== "runtime_net_only" || launch.direct_network !== false || launch.wallet_injection !== false) {
    throw new Error("local crosvm launcher requires runtime_net_only with no direct network or wallet injection");
  }
  if (launch.relay_ipc?.kind !== "unix_socket") {
    throw new Error("local crosvm launcher requires launch_request.relay_ipc.kind=unix_socket");
  }
  validateAbsolutePath(launch.relay_ipc.path, "launch_request.relay_ipc.path");
  const stat = fs.statSync(launch.relay_ipc.path);
  if (!stat.isSocket()) {
    throw new Error(`server Runtime relay path is not a Unix socket: ${launch.relay_ipc.path}`);
  }
  return launch;
}

function defaultDataDir(scriptPath) {
  if (process.env[DATA_DIR_ENV]) {
    validateAbsolutePath(process.env[DATA_DIR_ENV], DATA_DIR_ENV);
    return process.env[DATA_DIR_ENV];
  }
  if (path.basename(path.dirname(scriptPath)) === "bin") {
    return path.dirname(path.dirname(scriptPath));
  }
  if (process.env.XDG_DATA_HOME) return path.join(process.env.XDG_DATA_HOME, "elastos");
  if (process.env.HOME) return path.join(process.env.HOME, ".local/share/elastos");
  return "/var/lib/elastos";
}

function defaultVmRoot(dataDir) {
  if (process.platform === "linux") {
    return path.join(dataDir, "bvm");
  }
  return "/tmp/evzs";
}

function defaultRootfsPoolDir(dataDir) {
  return path.join(dataDir, "browser-vm/rootfs-pool");
}

function defaultVmVcpus() {
  return os.arch() === "arm64" ? "4" : "2";
}

function isIpv4(value) {
  const parts = String(value || "").split(".");
  return parts.length === 4 && parts.every((part) => {
    if (!/^(?:0|[1-9][0-9]{0,2})$/.test(part)) return false;
    const value = Number(part);
    return value >= 0 && value <= 255;
  });
}

function resolveNetworkPlaceholders(value, network) {
  return String(value || "")
    .replaceAll("{host_ip}", network.hostIp)
    .replaceAll("{guest_ip}", network.guestIp)
    .replaceAll("{turn_port}", String(network.turnPort));
}

function vmIceEnv(network) {
  const config = {};
  for (const key of VM_ICE_ENV_KEYS) {
    const value = process.env[key];
    if (value == null || value === "") continue;
    const resolved = resolveNetworkPlaceholders(value, network);
    if (/[\r\n\0]/.test(resolved)) {
      throw new Error(`${key} must not contain control characters`);
    }
    config[key] = resolved;
  }
  return config;
}

function collectIceUrls(config) {
  return collectIceServerEntries(config).flatMap((entry) => entry.urls);
}

function collectIceServerEntries(config) {
  const urls = [];
  const entries = [];
  const defaultUsername = config.ELASTOS_BROWSER_VM_ICE_USERNAME || "";
  const defaultCredential = config.ELASTOS_BROWSER_VM_ICE_CREDENTIAL || "";
  if (config.ELASTOS_BROWSER_VM_ICE_SERVER) {
    entries.push({
      urls: [config.ELASTOS_BROWSER_VM_ICE_SERVER],
      username: defaultUsername,
      credential: defaultCredential,
    });
  }
  if (config.ELASTOS_BROWSER_VM_ICE_SERVERS_JSON) {
    let parsed = null;
    try {
      parsed = JSON.parse(config.ELASTOS_BROWSER_VM_ICE_SERVERS_JSON);
    } catch (error) {
      throw new Error(`ELASTOS_BROWSER_VM_ICE_SERVERS_JSON is invalid JSON: ${error.message}`);
    }
    if (!Array.isArray(parsed)) {
      throw new Error("ELASTOS_BROWSER_VM_ICE_SERVERS_JSON must be an array");
    }
    for (const entry of parsed) {
      if (typeof entry === "string") {
        entries.push({
          urls: [entry],
          username: defaultUsername,
          credential: defaultCredential,
        });
        continue;
      }
      if (!entry || typeof entry !== "object" || Array.isArray(entry)) {
        throw new Error("ICE server entries must be URL strings or RTCIceServer objects");
      }
      const values = Array.isArray(entry.urls) ? entry.urls : [entry.urls];
      const entryUrls = [];
      for (const url of values) {
        if (typeof url === "string") entryUrls.push(url);
      }
      if (entryUrls.length > 0) {
        entries.push({
          urls: entryUrls,
          username: typeof entry.username === "string" && entry.username ? entry.username : defaultUsername,
          credential: typeof entry.credential === "string" && entry.credential ? entry.credential : defaultCredential,
        });
      }
    }
  }
  return entries;
}

function hasTurnServer(config) {
  return collectIceUrls(config).some((url) => /^turns?:/i.test(String(url || "").trim()));
}

function iceBootConfigHex(config) {
  const raw = Buffer.from(JSON.stringify(config), "utf8");
  if (raw.length === 0 || raw.toString("utf8") === "{}") {
    return "";
  }
  if (raw.length > 4096) {
    throw new Error("Browser VM ICE boot config is too large for kernel boot args");
  }
  return raw.toString("hex");
}

function turnServerPorts(config, network) {
  const ports = new Set();
  for (const turn of localTurnServers(config, network)) {
    ports.add(turn.port);
  }
  return [...ports].sort((a, b) => a - b);
}

function parseTurnUrl(value) {
  const text = String(value || "").trim();
  const match = text.match(/^(turns?):(?:\/\/)?([0-9.]+)(?::([0-9]+))?(?:\?|$)/i);
  if (!match) return null;
  const scheme = match[1].toLowerCase();
  const host = match[2];
  const port = Number(match[3] || (scheme === "turns" ? 5349 : 3478));
  if (!isIpv4(host) || !Number.isInteger(port) || port <= 0 || port > 65535) {
    return null;
  }
  return { scheme, host, port };
}

function localTurnServers(config, network) {
  const servers = [];
  for (const entry of collectIceServerEntries(config)) {
    for (const url of entry.urls) {
      const parsed = parseTurnUrl(url);
      if (!parsed || parsed.host !== network.hostIp) continue;
      servers.push({
        ...parsed,
        username: entry.username || "",
        credential: entry.credential || "",
      });
    }
  }
  return servers;
}

function turnRelayPortRange() {
  const min = Number(process.env[TURN_RELAY_MIN_PORT_ENV] || "49152");
  const max = Number(process.env[TURN_RELAY_MAX_PORT_ENV] || String(min + 63));
  if (!Number.isInteger(min) || !Number.isInteger(max) || min <= 0 || max > 65535 || min > max) {
    throw new Error(`${TURN_RELAY_MIN_PORT_ENV}/${TURN_RELAY_MAX_PORT_ENV} must define a valid port range`);
  }
  return { min, max };
}

function sessionTurnServer(config, network) {
  const servers = localTurnServers(config, network);
  if (servers.length === 0) return null;
  const server = servers.find((entry) => entry.scheme === "turn") || servers[0];
  if (!server.username || !server.credential) {
    throw new Error("session-local TURN requires ELASTOS_BROWSER_VM_ICE_USERNAME and ELASTOS_BROWSER_VM_ICE_CREDENTIAL");
  }
  return {
    ...server,
    relay: turnRelayPortRange(),
  };
}

function sessionSuffix(value) {
  const hash = crypto.createHash("sha256").update(String(value || "stream")).digest("hex");
  return `bvm-${hash.slice(0, 16)}-${crypto.randomBytes(4).toString("hex")}`;
}

function profileKey(launch) {
  const subject = typeof launch.principal_id === "string" && launch.principal_id.trim()
    ? launch.principal_id.trim()
    : launch.stream_id;
  return `principal-${crypto.createHash("sha256").update(subject).digest("hex").slice(0, 32)}`;
}

function requireFile(file, label) {
  validateAbsolutePath(file, label);
  if (!fs.existsSync(file)) throw new Error(`${label} does not exist: ${file}`);
}

function runSync(command, args, { ignoreFailure = false, timeout = 30000 } = {}) {
  const result = spawnSync(command, args, {
    encoding: "utf8",
    timeout,
  });
  if (!ignoreFailure && (result.error || result.status !== 0)) {
    const detail = result.error?.message || result.stderr || result.stdout || `${command} exited ${result.status}`;
    throw new Error(detail.trim());
  }
  return result;
}

function sudoIp(args, options = {}) {
  if (process.getuid?.() === 0) {
    return runSync("ip", args, options);
  }
  return runSync("sudo", ["-n", "ip", ...args], options);
}

function sudoIptables(args, options = {}) {
  const iptables = process.env.ELASTOS_BROWSER_VM_IPTABLES_BIN || "iptables";
  if (process.getuid?.() === 0) {
    return runSync(iptables, args, options);
  }
  return runSync("sudo", ["-n", iptables, ...args], options);
}

function privateNetworkForSuffix(suffix) {
  const digest = crypto.createHash("sha256").update(suffix).digest();
  const third = 200 + (digest[0] % 40);
  const base = (digest[1] % 50) * 4;
  const turnPort = 41000 + (((digest[6] << 8) | digest[7]) % 4096);
  return {
    hostIp: `192.168.${third}.${base + 1}`,
    guestIp: `192.168.${third}.${base + 2}`,
    prefix: 30,
    turnPort,
    tapName: `ebv${digest.toString("hex").slice(0, 10)}`,
    mac: `02:eb:${digest[2].toString(16).padStart(2, "0")}:${digest[3].toString(16).padStart(2, "0")}:${digest[4].toString(16).padStart(2, "0")}:${digest[5].toString(16).padStart(2, "0")}`,
  };
}

function prepareTap(network) {
  sudoIp(["link", "del", network.tapName], { ignoreFailure: true });
  sudoIp(["tuntap", "add", "dev", network.tapName, "mode", "tap", "user", os.userInfo().username]);
  cleanupFns.push(() => sudoIp(["link", "del", network.tapName], { ignoreFailure: true }));
  sudoIp(["addr", "add", `${network.hostIp}/${network.prefix}`, "dev", network.tapName]);
  sudoIp(["link", "set", network.tapName, "up"]);
}

function prepareFirewall(network, { mediaPorts = [], mediaPortRanges = [] } = {}) {
  const acceptRule = [
    "INPUT",
    "-i",
    network.tapName,
    "-s",
    network.guestIp,
    "-d",
    network.hostIp,
    "-p",
    "tcp",
    "--dport",
    "19091",
    "-j",
    "ACCEPT",
  ];
  const establishedRule = [
    "INPUT",
    "-i",
    network.tapName,
    "-m",
    "conntrack",
    "--ctstate",
    "ESTABLISHED,RELATED",
    "-j",
    "ACCEPT",
  ];
  const dropRule = ["INPUT", "-i", network.tapName, "-j", "DROP"];
  sudoIptables(["-I", ...dropRule]);
  cleanupFns.push(() => sudoIptables(["-D", ...dropRule], { ignoreFailure: true }));
  sudoIptables(["-I", ...acceptRule]);
  cleanupFns.push(() => sudoIptables(["-D", ...acceptRule], { ignoreFailure: true }));
  for (const port of mediaPorts) {
    for (const protocol of ["tcp", "udp"]) {
      const mediaRule = [
        "INPUT",
        "-i",
        network.tapName,
        "-s",
        network.guestIp,
        "-d",
        network.hostIp,
        "-p",
        protocol,
        "--dport",
        String(port),
        "-j",
        "ACCEPT",
      ];
      sudoIptables(["-I", ...mediaRule]);
      cleanupFns.push(() => sudoIptables(["-D", ...mediaRule], { ignoreFailure: true }));
    }
  }
  for (const range of mediaPortRanges) {
    for (const protocol of ["tcp", "udp"]) {
      const mediaRule = [
        "INPUT",
        "-i",
        network.tapName,
        "-s",
        network.guestIp,
        "-d",
        network.hostIp,
        "-p",
        protocol,
        "--dport",
        `${range.min}:${range.max}`,
        "-j",
        "ACCEPT",
      ];
      sudoIptables(["-I", ...mediaRule]);
      cleanupFns.push(() => sudoIptables(["-D", ...mediaRule], { ignoreFailure: true }));
    }
  }
  sudoIptables(["-I", ...establishedRule]);
  cleanupFns.push(() => sudoIptables(["-D", ...establishedRule], { ignoreFailure: true }));
}

function assertFirewallToolAvailable() {
  const iptables = process.env.ELASTOS_BROWSER_VM_IPTABLES_BIN || "iptables";
  if (process.getuid?.() === 0) {
    runSync(iptables, ["--version"]);
    return;
  }
  runSync("sudo", ["-n", iptables, "--version"]);
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

function trackServer(server, socketPath = "") {
  const record = { server, socketPath };
  servers.add(record);
  server.once("close", () => servers.delete(record));
  return record;
}

function bridgeSockets(left, right) {
  const destroyBoth = () => {
    left.destroy();
    right.destroy();
  };
  left.on("error", destroyBoth);
  right.on("error", destroyBoth);
  left.on("close", destroyBoth);
  right.on("close", destroyBoth);
  left.on("end", destroyBoth);
  right.on("end", destroyBoth);
  left.pipe(right);
  right.pipe(left);
}

async function startTcpToUnixBridge(host, port, unixPath) {
  const server = net.createServer((client) => {
    const upstream = net.createConnection({ path: unixPath });
    bridgeSockets(client, upstream);
  });
  trackServer(server);
  await listen(server, port, host);
}

async function startUnixToTcpBridge(unixPath, host, port) {
  try {
    fs.unlinkSync(unixPath);
  } catch (error) {
    if (error?.code !== "ENOENT") throw error;
  }
  const server = net.createServer((client) => {
    const upstream = net.connect({ host, port });
    bridgeSockets(client, upstream);
  });
  trackServer(server, unixPath);
  await listen(server, unixPath);
  fs.chmodSync(unixPath, 0o600);
}

function turnserverBin() {
  const configured = process.env[TURNSERVER_BIN_ENV];
  if (configured) {
    validateAbsolutePath(configured, TURNSERVER_BIN_ENV);
    requireFile(configured, TURNSERVER_BIN_ENV);
    return configured;
  }
  const result = runSync("sh", ["-lc", "command -v turnserver"], { ignoreFailure: true });
  return result.status === 0 && result.stdout.trim() ? result.stdout.trim() : "turnserver";
}

function tcpListenerVisible(host, port) {
  if (process.platform !== "linux") return null;
  const result = spawnSync("ss", ["-H", "-ltn"], {
    encoding: "utf8",
    timeout: 1000,
  });
  if (result.error || result.status !== 0) return null;
  const endpoint = `${host}:${port}`;
  return result.stdout.split(/\r?\n/).some((line) => line.includes(endpoint));
}

function probeTcpConnect(host, port) {
  return new Promise((resolve) => {
    const socket = net.createConnection({ host, port });
    let settled = false;
    const finish = (ready) => {
      if (settled) return;
      settled = true;
      socket.destroy();
      resolve(ready);
    };
    socket.setTimeout(1000, () => finish(false));
    socket.once("connect", () => finish(true));
    socket.once("error", () => finish(false));
  });
}

async function waitForTcpListener(host, port, child, logPath) {
  const deadline = Date.now() + Number(process.env.ELASTOS_BROWSER_VM_TURNSERVER_READY_TIMEOUT_MS || "10000");
  let usedKernelListenerProbe = false;
  while (Date.now() < deadline) {
    if (child.exitCode !== null || child.signalCode !== null) {
      throw new Error(`session TURN server exited before becoming ready: ${child.exitCode ?? child.signalCode}\n${tailFile(logPath, 12 * 1024)}`);
    }
    const visible = tcpListenerVisible(host, port);
    if (visible === true) return;
    if (visible === false) {
      usedKernelListenerProbe = true;
    } else if (await probeTcpConnect(host, port)) {
      return;
    }
    await new Promise((resolve) => setTimeout(resolve, 150));
  }
  const source = usedKernelListenerProbe ? " according to ss -ltn" : "";
  throw new Error(`session TURN server did not become visible on ${host}:${port}${source}\n${tailFile(logPath, 12 * 1024)}`);
}

async function startSessionTurnServer({ network, sessionDir, turn }) {
  if (!turn) return null;
  const logPath = path.join(sessionDir, "turnserver.log");
  const pidPath = path.join(sessionDir, "turnserver.pid");
  const logFd = fs.openSync(logPath, "a");
  const args = [
    "-n",
    "--listening-ip",
    network.hostIp,
    "--relay-ip",
    network.hostIp,
    "--listening-port",
    String(turn.port),
    "--min-port",
    String(turn.relay.min),
    "--max-port",
    String(turn.relay.max),
    "--fingerprint",
    "--lt-cred-mech",
    "--realm",
    "elastos-browser-vm.local",
    "--user",
    `${turn.username}:${turn.credential}`,
    "--no-cli",
    "--no-tls",
    "--no-dtls",
    "--no-multicast-peers",
    "--no-software-attribute",
    `--pidfile=${pidPath}`,
    `--log-file=${logPath}`,
    "--simple-log",
  ];
  const child = spawnTracked(turnserverBin(), args, {
    stdio: ["ignore", logFd, logFd],
  });
  child.once("exit", () => {
    try {
      fs.closeSync(logFd);
    } catch {}
  });
  child.once("error", () => {
    try {
      fs.closeSync(logFd);
    } catch {}
  });
  await waitForTcpListener(network.hostIp, turn.port, child, logPath);
  logPhase(`session TURN listening on ${network.hostIp}:${turn.port} relay ports ${turn.relay.min}-${turn.relay.max}`);
  return { child, logPath };
}

function spawnTracked(command, args, options = {}) {
  const child = spawn(command, args, options);
  children.add(child);
  child.once("exit", () => children.delete(child));
  return child;
}

function copyRootfs(rootfs, launchRootfs) {
  fs.mkdirSync(path.dirname(launchRootfs), { recursive: true, mode: 0o700 });
  const start = Date.now();
  runSync("cp", ["--reflink=auto", "--sparse=always", rootfs, launchRootfs], {
    timeout: Number(process.env.ELASTOS_BROWSER_VM_ROOTFS_COPY_TIMEOUT_MS || "600000"),
  });
  return Date.now() - start;
}

function acquirePreparedRootfs({ poolDir, launchRootfs }) {
  validateAbsolutePath(poolDir, ROOTFS_POOL_DIR_ENV);
  fs.mkdirSync(path.dirname(launchRootfs), { recursive: true, mode: 0o700 });
  fs.mkdirSync(poolDir, { recursive: true, mode: 0o700 });
  const candidates = fs.readdirSync(poolDir)
    .filter((name) => /^rootfs-[A-Za-z0-9._-]+\.ext4$/.test(name))
    .sort();
  for (const name of candidates) {
    const candidate = path.join(poolDir, name);
    try {
      fs.renameSync(candidate, launchRootfs);
      fs.chmodSync(launchRootfs, 0o600);
      return candidate;
    } catch (error) {
      if (error?.code !== "ENOENT") {
        logPhase(`could not acquire prepared rootfs ${candidate}: ${error.message}`);
      }
    }
  }
  return "";
}

function readyRootfsFiles(poolDir) {
  try {
    return fs.readdirSync(poolDir).filter((name) => /^rootfs-[A-Za-z0-9._-]+\.ext4$/.test(name));
  } catch (error) {
    if (error?.code === "ENOENT") return [];
    throw error;
  }
}

function rootfsPoolRefillScript(dataDir, scriptPath) {
  const configured = process.env[ROOTFS_POOL_REFILL_SCRIPT_ENV];
  const candidates = [
    configured,
    path.join(dataDir, "scripts/browser-vm-prepare-rootfs-pool.mjs"),
    path.join(path.dirname(scriptPath), "browser-vm-prepare-rootfs-pool.mjs"),
  ].filter(Boolean);
  return candidates.find((candidate) => fs.existsSync(candidate)) || "";
}

function rootfsPoolRefillCommand(refillScript) {
  if (/\.(mjs|cjs|js)$/.test(refillScript)) {
    return { command: process.execPath, args: [refillScript] };
  }
  return { command: refillScript, args: [] };
}

function availableBytesForPath(targetPath) {
  let current = targetPath;
  while (!fs.existsSync(current)) {
    const parent = path.dirname(current);
    if (parent === current) return null;
    current = parent;
  }
  const stats = fs.statfsSync(current);
  return Number(stats.bavail) * Number(stats.bsize);
}

function refillMinFreeBytes() {
  const configured = Number(process.env[ROOTFS_POOL_REFILL_MIN_FREE_MIB_ENV] || "4096");
  if (!Number.isFinite(configured) || configured < 0) {
    return 4096 * 1024 * 1024;
  }
  return configured * 1024 * 1024;
}

function maybeRefillPreparedRootfsPool({ dataDir, poolDir, rootfs, sessionDir, scriptPath }) {
  const targetCount = Number(process.env[ROOTFS_POOL_REFILL_COUNT_ENV] || "2");
  if (!Number.isInteger(targetCount) || targetCount < 1) {
    return;
  }
  const missingCount = targetCount - readyRootfsFiles(poolDir).length;
  if (missingCount <= 0) {
    return;
  }
  const refillScript = rootfsPoolRefillScript(dataDir, scriptPath);
  if (!refillScript) {
    logPhase("prepared rootfs pool refill skipped: browser-vm-prepare-rootfs-pool.mjs not found");
    return;
  }
  const availableBytes = availableBytesForPath(poolDir);
  const requiredBytes = (fs.statSync(rootfs).size * missingCount) + refillMinFreeBytes();
  if (availableBytes !== null && availableBytes < requiredBytes) {
    logPhase(`prepared rootfs pool refill skipped: free space ${Math.floor(availableBytes / 1048576)}MiB below required ${Math.ceil(requiredBytes / 1048576)}MiB`);
    return;
  }
  const logPath = path.join(sessionDir, "rootfs-pool-refill.log");
  const logFd = fs.openSync(logPath, "a");
  const refillCommand = rootfsPoolRefillCommand(refillScript);
  const child = spawn(refillCommand.command, [
    ...refillCommand.args,
    "--data-dir",
    dataDir,
    "--rootfs",
    rootfs,
    "--pool-dir",
    poolDir,
    "--count",
    String(targetCount),
  ], {
    detached: true,
    stdio: ["ignore", logFd, logFd],
    env: { ...process.env, ELASTOS_BROWSER_VM_DATA_DIR: dataDir },
  });
  child.unref();
  fs.closeSync(logFd);
  logPhase(`prepared rootfs pool refill started pid=${child.pid} target=${targetCount} log=${logPath}`);
}

function refillPreparedRootfsPoolSync({ dataDir, poolDir, rootfs, sessionDir, scriptPath }) {
  const refillScript = rootfsPoolRefillScript(dataDir, scriptPath);
  if (!refillScript) {
    logPhase("prepared rootfs pool synchronous refill skipped: browser-vm-prepare-rootfs-pool.mjs not found");
    return;
  }
  const logPath = path.join(sessionDir, "rootfs-pool-refill-sync.log");
  const logFd = fs.openSync(logPath, "a");
  try {
    const refillCommand = rootfsPoolRefillCommand(refillScript);
    const result = spawnSync(refillCommand.command, [
      ...refillCommand.args,
      "--data-dir",
      dataDir,
      "--rootfs",
      rootfs,
      "--pool-dir",
      poolDir,
      "--count",
      "1",
    ], {
      stdio: ["ignore", logFd, logFd],
      env: { ...process.env, ELASTOS_BROWSER_VM_DATA_DIR: dataDir },
      timeout: Number(process.env.ELASTOS_BROWSER_VM_ROOTFS_POOL_REFILL_TIMEOUT_MS || "900000"),
    });
    if (result.error || result.status !== 0) {
      logPhase(`prepared rootfs pool synchronous refill failed: ${result.error?.message || `exit ${result.status}`} log=${logPath}`);
      return;
    }
    logPhase(`prepared rootfs pool synchronous refill completed log=${logPath}`);
  } finally {
    fs.closeSync(logFd);
  }
}

function discardLaunchRootfs(launchRootfs) {
  if (process.env[SESSION_KEEP_ROOTFS_ENV] === "1") {
    return;
  }
  try {
    if (fs.existsSync(launchRootfs)) {
      fs.unlinkSync(launchRootfs);
      logPhase(`discarded disposable launch rootfs ${launchRootfs}`);
    }
  } catch (error) {
    logPhase(`could not discard disposable launch rootfs ${launchRootfs}: ${error.message}`);
  }
}

function prepareLaunchRootfs({ rootfs, launchRootfs, dataDir, sessionDir, scriptPath }) {
  const copyMode = process.env[ROOTFS_COPY_MODE_ENV] || "pool-or-copy";
  const poolDir = process.env[ROOTFS_POOL_DIR_ENV] || defaultRootfsPoolDir(dataDir);
  const startedAt = Date.now();
  if (copyMode === "pool-required" || copyMode === "pool-or-copy") {
    const acquired = acquirePreparedRootfs({ poolDir, launchRootfs });
    if (acquired) {
      return {
        mode: "prepared_pool",
        source: acquired,
        poolDir,
        elapsed_ms: Date.now() - startedAt,
      };
    }
    if (copyMode === "pool-required") {
      refillPreparedRootfsPoolSync({ dataDir, poolDir, rootfs, sessionDir, scriptPath });
      const refilled = acquirePreparedRootfs({ poolDir, launchRootfs });
      if (refilled) {
        return {
          mode: "prepared_pool",
          source: refilled,
          poolDir,
          elapsed_ms: Date.now() - startedAt,
        };
      }
      throw new Error(`Browser VM prepared rootfs pool is empty: ${poolDir}. Run browser-vm-prepare-rootfs-pool before launching Browser.`);
    }
  }
  return {
    mode: "copy",
    source: rootfs,
    poolDir,
    elapsed_ms: copyRootfs(rootfs, launchRootfs),
  };
}

function startCrosvm({ crosvm, kernel, initrd, rootfs, sessionDir, network, launch, profile, iceConfig }) {
  const serialLog = path.join(sessionDir, "serial.log");
  const crosvmLog = path.join(sessionDir, "crosvm.log");
  const crosvmFd = fs.openSync(crosvmLog, "a");
  if (!hasTurnServer(iceConfig)) {
    throw new Error("local crosvm webrtc_remote_display requires ELASTOS_BROWSER_VM_ICE_SERVER or JSON with at least one turn:/turns: URL for media_transport=runtime_relay");
  }
  const iceConfigHex = iceBootConfigHex(iceConfig);
  const bootArgs = [
    "console=ttyS0",
    "reboot=k",
    "panic=1",
    "root=/dev/vda",
    "rootfstype=ext4",
    "rw",
    "init=/opt/elastos/bin/browser-vm-init",
    "random.trust_cpu=on",
    `elastos.browser_epoch=${Math.floor(Date.now() / 1000)}`,
    `elastos.browser_profile=${profile}`,
    `elastos.browser_display_mode=${launch.display_mode}`,
    "elastos.browser_transport=private_tcp",
    `elastos.browser_host_ip=${network.hostIp}`,
    `elastos.browser_guest_ip=${network.guestIp}`,
    `elastos.browser_net_prefix=${network.prefix}`,
    "elastos.browser_relay_port=19091",
    "elastos.browser_control_port=19092",
    ...(iceConfigHex ? [`elastos.browser_ice_config_hex=${iceConfigHex}`] : []),
  ].join(" ");
  const args = [
    "run",
    "--mem",
    process.env.ELASTOS_BROWSER_VM_MEMORY_MIB || (os.arch() === "arm64" ? "2048" : "3072"),
    "--cpus",
    process.env.ELASTOS_BROWSER_VM_VCPUS || defaultVmVcpus(),
    "--serial",
    `type=file,path=${serialLog},hardware=serial,num=1`,
    "--block",
    `path=${rootfs},root=true`,
    "--net",
    `tap-name=${network.tapName},mac=${network.mac}`,
    "--pivot-root",
    process.env.ELASTOS_BROWSER_VM_CROSVM_PIVOT_ROOT || "/tmp/elastos/crosvm-empty",
    "--initrd",
    initrd,
    "-p",
    bootArgs,
    kernel,
  ];
  fs.mkdirSync(process.env.ELASTOS_BROWSER_VM_CROSVM_PIVOT_ROOT || "/tmp/elastos/crosvm-empty", { recursive: true });
  const child = spawnTracked(crosvm, args, {
    stdio: ["ignore", crosvmFd, crosvmFd],
  });
  child.once("exit", () => {
    try {
      fs.closeSync(crosvmFd);
    } catch {}
  });
  child.once("error", () => {
    try {
      fs.closeSync(crosvmFd);
    } catch {}
  });
  return { child, serialLog, crosvmLog };
}

function tailFile(file, maxBytes = 24 * 1024) {
  try {
    const data = fs.readFileSync(file);
    return data.subarray(Math.max(0, data.length - maxBytes)).toString("utf8");
  } catch {
    return "";
  }
}

function headerLines(headers) {
  return Object.entries(headers)
    .map(([name, value]) => `${name}: ${value}`)
    .join("\r\n");
}

function parseHttpJsonResponse(raw) {
  const headerEnd = raw.indexOf("\r\n\r\n");
  if (headerEnd < 0) {
    throw new Error("Browser VM control response missing HTTP headers");
  }
  const head = raw.slice(0, headerEnd);
  const bodyText = raw.slice(headerEnd + 4);
  const [statusLine, ...lines] = head.split("\r\n");
  const match = statusLine.match(/^HTTP\/\d(?:\.\d)?\s+(\d{3})/);
  if (!match) {
    throw new Error(`Browser VM control response is not HTTP: ${statusLine.slice(0, 120)}`);
  }
  const statusCode = Number(match[1]);
  let contentLength = null;
  for (const line of lines) {
    const separator = line.indexOf(":");
    if (separator < 0) continue;
    const name = line.slice(0, separator).trim().toLowerCase();
    if (name === "content-length") {
      const value = Number(line.slice(separator + 1).trim());
      if (Number.isFinite(value) && value >= 0) {
        contentLength = value;
      }
    }
  }
  if (contentLength != null && Buffer.byteLength(bodyText) < contentLength) {
    throw new Error("Browser VM control response body is incomplete");
  }
  const bodySlice = contentLength == null ? bodyText : bodyText.slice(0, contentLength);
  let parsed = {};
  try {
    parsed = bodySlice ? JSON.parse(bodySlice) : {};
  } catch (error) {
    throw new Error(`Browser VM control response is not JSON: ${error.message}: ${bodySlice.slice(0, 200)}`);
  }
  if (statusCode < 200 || statusCode >= 300) {
    const error = new Error(parsed.error || parsed.message || `Browser VM control returned ${statusCode}`);
    error.statusCode = statusCode;
    error.responseBody = parsed;
    throw error;
  }
  return parsed;
}

function redactSensitiveText(value) {
  return String(value)
    .replace(/(elastos\.browser_ice_config_hex=)[0-9a-fA-F]+/g, "$1[redacted]")
    .replace(/((?:credential|password|secret|token|authorization)[\"']?\s*[:=]\s*)[\"']?[^,\s}\"']+/gi, "$1[redacted]")
    .replace(/(turns?:\/\/[^:\s\"']+):([^@\s\"']+)@/gi, "$1:[redacted]@");
}

function redactSensitiveValue(value) {
  if (Array.isArray(value)) {
    return value.map((entry) => redactSensitiveValue(entry));
  }
  if (value && typeof value === "object") {
    const result = {};
    for (const [key, entry] of Object.entries(value)) {
      result[key] = /(credential|password|secret|token|authorization)/i.test(key)
        ? "[redacted]"
        : redactSensitiveValue(entry);
    }
    return result;
  }
  if (typeof value === "string") {
    return redactSensitiveText(value);
  }
  return value;
}

function writeControlDiagnostics(sessionDir, error) {
  if (!error?.responseBody || typeof error.responseBody !== "object") {
    return null;
  }
  const file = path.join(sessionDir, "guest-control-error.json");
  fs.writeFileSync(file, `${JSON.stringify(redactSensitiveValue(error.responseBody), null, 2)}\n`, { mode: 0o600 });
  return file;
}

function summarizeControlLogs(error) {
  const logs = error?.responseBody?.logs;
  if (!logs || typeof logs !== "object") {
    return "";
  }
  const lines = [];
  for (const [name, entry] of Object.entries(logs)) {
    if (!entry?.present) {
      lines.push(`${name}: absent${entry?.error ? ` (${entry.error})` : ""}`);
      continue;
    }
    const tail = redactSensitiveText(entry.tail || "")
      .split("\n")
      .slice(-20)
      .join("\n")
      .trim();
    lines.push(`${name}: ${entry.bytes || 0} bytes${tail ? `\n${tail}` : ""}`);
  }
  return lines.length > 0 ? `[local-crosvm] guest control log tails (redacted):\n${lines.join("\n---\n")}` : "";
}

function httpJsonUnix(socketPath, requestPath, { method = "GET", body = null, timeoutMs = 5000 } = {}) {
  const bytes = body ? Buffer.from(JSON.stringify(body)) : Buffer.alloc(0);
  return new Promise((resolve, reject) => {
    const request = [
      `${method} ${requestPath} HTTP/1.1`,
      headerLines({
        Host: "browser-engine",
        Connection: "close",
        ...(body
          ? {
              "Content-Type": "application/json",
              "Content-Length": bytes.length,
            }
          : {}),
      }),
      "",
      "",
    ].join("\r\n");
    let settled = false;
    const finish = (fn, value) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      socket.destroy();
      fn(value);
    };
    const timer = setTimeout(() => {
      finish(reject, new Error("Browser VM control request timed out"));
    }, timeoutMs);
    const socket = net.createConnection({ path: socketPath });
    const chunks = [];
    socket.setTimeout(timeoutMs, () => {
      finish(reject, new Error("Browser VM control request timed out"));
    });
    socket.on("connect", () => {
      socket.write(request);
      if (bytes.length > 0) {
        socket.write(bytes);
      }
    });
    socket.on("data", (chunk) => {
      chunks.push(chunk);
    });
    socket.on("end", () => {
      try {
        finish(resolve, parseHttpJsonResponse(Buffer.concat(chunks).toString("utf8")));
      } catch (error) {
        finish(reject, error);
      }
    });
    socket.on("error", (error) => finish(reject, error));
  });
}

async function waitForGuestControl({ controlSocketPath, crosvmChild, serialLog, crosvmLog }) {
  const timeoutMs = Number(process.env.ELASTOS_BROWSER_VM_CONTROL_READY_TIMEOUT_MS || "180000");
  const deadline = Date.now() + timeoutMs;
  let lastError = "";
  while (Date.now() < deadline) {
    if (crosvmChild.exitCode !== null || crosvmChild.signalCode !== null) {
      throw new Error(`crosvm exited before Browser control became ready: ${crosvmChild.exitCode ?? crosvmChild.signalCode}\nserial:\n${tailFile(serialLog)}\ncrosvm:\n${tailFile(crosvmLog)}`);
    }
    try {
      const status = await httpJsonUnix(controlSocketPath, "/status", { timeoutMs: 1500 });
      if (status?.schema === "elastos.browser.selkies-control.status/v1") {
        return;
      }
      lastError = "control returned an unexpected status schema";
    } catch (error) {
      lastError = error instanceof Error ? error.message : String(error);
    }
    await new Promise((resolve) => setTimeout(resolve, 250));
  }
  throw new Error(`Browser VM control did not become ready: ${lastError}\nserial:\n${tailFile(serialLog)}\ncrosvm:\n${tailFile(crosvmLog)}`);
}

function guestControlOpenRequest(request) {
  return {
    ...request,
    schema: "elastos.browser.vm-guest.open/v1",
    launch_request: {
      ...request.launch_request,
      engine: "selkies_gstreamer",
      guarantee_level: "mechanism_microvm",
    },
  };
}

function rewriteResult(result, launch, sessionDir, controlSocketPath) {
  return {
    ...result,
    adapter: launch.adapter,
    engine: launch.engine,
    stream_id: launch.stream_id,
    control_socket_path: controlSocketPath,
    isolated_session: true,
    display_session: {
      ...result.display_session,
      media_transport: result.display_session?.media_transport || "runtime_relay",
    },
    isolation: {
      schema: "elastos.browser.engine.isolation/v1",
      kind: "per_launch_vm_target",
      session_dir: sessionDir,
    },
  };
}

async function cleanupAndExit(signal = null) {
  if (exiting) return;
  exiting = true;
  for (const { server, socketPath } of Array.from(servers)) {
    try {
      server.close();
    } catch {}
    if (socketPath) {
      try {
        fs.unlinkSync(socketPath);
      } catch {}
    }
  }
  for (const child of Array.from(children)) {
    try {
      child.kill("SIGTERM");
    } catch {}
  }
  for (const cleanup of cleanupFns.reverse()) {
    try {
      cleanup();
    } catch {}
  }
  if (globalThis.__elastosBrowserVmSessionDir) {
    discardLaunchRootfs(path.join(globalThis.__elastosBrowserVmSessionDir, "rootfs.ext4"));
  }
  if (process.env[SESSION_KEEP_ENV] !== "1" && globalThis.__elastosBrowserVmSessionDir) {
    try {
      fs.rmSync(globalThis.__elastosBrowserVmSessionDir || "", { recursive: true, force: true });
    } catch {}
  }
  setTimeout(() => {
    for (const child of Array.from(children)) {
      try {
        child.kill("SIGKILL");
      } catch {}
    }
    process.exit(launchSucceeded ? 0 : 1);
  }, 500);
}

async function main() {
  const request = readOpenRequest();
  const launch = validateOpenRequest(request);
  const scriptPath = fileURLToPath(import.meta.url);
  const dataDir = defaultDataDir(scriptPath);
  const root = process.env[ROOT_ENV] || defaultVmRoot(dataDir);
  validateAbsolutePath(root, ROOT_ENV);
  const suffix = sessionSuffix(launch.stream_id);
  const sessionDir = path.join(root, suffix);
  globalThis.__elastosBrowserVmSessionDir = sessionDir;
  fs.mkdirSync(sessionDir, { recursive: true, mode: 0o700 });

  const rootfs = process.env.ELASTOS_BROWSER_VM_ROOTFS || path.join(dataDir, "browser-vm/rootfs.ext4");
  const kernel = process.env.ELASTOS_BROWSER_VM_KERNEL || path.join(dataDir, "bin/vmlinux");
  const initrd = process.env.ELASTOS_BROWSER_VM_INITRD || path.join(dataDir, "browser-vm/initrd");
  const crosvm = process.env.ELASTOS_BROWSER_VM_CROSVM_BIN || path.join(dataDir, "bin/crosvm");
  requireFile(rootfs, "Browser VM rootfs");
  requireFile(kernel, "Browser VM kernel");
  requireFile(initrd, "Browser VM initrd");
  requireFile(crosvm, "crosvm");
  requireFile("/dev/kvm", "/dev/kvm");
  requireFile("/dev/net/tun", "/dev/net/tun");
  assertFirewallToolAvailable();

  const network = privateNetworkForSuffix(suffix);
  const iceConfig = vmIceEnv(network);
  const mediaPorts = turnServerPorts(iceConfig, network);
  const turn = sessionTurnServer(iceConfig, network);
  const mediaPortRanges = turn ? [turn.relay] : [];
  const controlSocketPath = path.join(sessionDir, "control.sock");
  validateAbsolutePath(controlSocketPath, "control socket path");
  const launchRootfs = path.join(sessionDir, "rootfs.ext4");

  logPhase(`session dir ${sessionDir}`);
  logPhase(`preparing launch rootfs from ${rootfs} to ${launchRootfs}`);
  prepareTap(network);
  await startSessionTurnServer({ network, sessionDir, turn });
  prepareFirewall(network, { mediaPorts, mediaPortRanges });
  if (mediaPorts.length > 0) {
    logPhase(`allowed guest access to host media relay ports ${mediaPorts.join(",")}`);
  }
  if (mediaPortRanges.length > 0) {
    logPhase(`allowed guest access to host media relay port ranges ${mediaPortRanges.map((range) => `${range.min}-${range.max}`).join(",")}`);
  }
  await startTcpToUnixBridge(network.hostIp, 19091, launch.relay_ipc.path);
  await startUnixToTcpBridge(controlSocketPath, network.guestIp, 19092);
  const rootfsPrep = prepareLaunchRootfs({ rootfs, launchRootfs, dataDir, sessionDir, scriptPath });
  logPhase(`prepared rootfs via ${rootfsPrep.mode} in ${rootfsPrep.elapsed_ms}ms`);
  let deferredRootfsPoolRefill = null;
  if (rootfsPrep.mode === "prepared_pool") {
    deferredRootfsPoolRefill = {
      dataDir,
      poolDir: rootfsPrep.poolDir,
      rootfs,
      sessionDir,
      scriptPath,
    };
    logPhase("prepared rootfs pool refill deferred until VM exit");
  }

  const memoryMiB = process.env.ELASTOS_BROWSER_VM_MEMORY_MIB || (os.arch() === "arm64" ? "2048" : "3072");
  const vcpus = process.env.ELASTOS_BROWSER_VM_VCPUS || defaultVmVcpus();
  logPhase(`starting crosvm with ${memoryMiB}MiB and ${vcpus} vCPUs`);
  const vm = startCrosvm({
    crosvm,
    kernel,
    initrd,
    rootfs: launchRootfs,
    sessionDir,
    network,
    launch,
    profile: profileKey(launch),
    iceConfig,
  });
  let vmExited = false;

  try {
    logPhase("waiting for guest control");
    await waitForGuestControl({
      controlSocketPath,
      crosvmChild: vm.child,
      serialLog: vm.serialLog,
      crosvmLog: vm.crosvmLog,
    });
    logPhase("guest control ready");

    logPhase("opening Browser page");
    let opened;
    try {
      opened = await httpJsonUnix(controlSocketPath, "/pages", {
        method: "POST",
        body: guestControlOpenRequest(request),
        timeoutMs: Number(process.env.ELASTOS_BROWSER_VM_PAGE_OPEN_TIMEOUT_MS || "120000"),
      });
    } catch (error) {
      const diagnosticsFile = writeControlDiagnostics(sessionDir, error);
      const logSummary = summarizeControlLogs(error);
      throw new Error([
        error instanceof Error ? error.message : String(error),
        diagnosticsFile ? `[local-crosvm] guest control diagnostics: ${diagnosticsFile}` : "",
        logSummary,
      ].filter(Boolean).join("\n"));
    }
    logPhase("Browser page open returned");
    const result = rewriteResult(opened, launch, sessionDir, controlSocketPath);
    launchSucceeded = true;
    process.stdout.write(`${JSON.stringify(result)}\n`);

    await new Promise((resolve) => {
      vm.child.once("exit", resolve);
    });
    vmExited = true;
  } finally {
    if (vmExited) {
      discardLaunchRootfs(launchRootfs);
    }
    if (deferredRootfsPoolRefill && vmExited) {
      maybeRefillPreparedRootfsPool(deferredRootfsPoolRefill);
    } else if (deferredRootfsPoolRefill) {
      logPhase("prepared rootfs pool refill skipped: VM did not exit cleanly");
    }
  }
}

process.on("SIGTERM", () => cleanupAndExit("SIGTERM"));
process.on("SIGINT", () => cleanupAndExit("SIGINT"));
process.on("SIGHUP", () => cleanupAndExit("SIGHUP"));

main()
  .then(() => cleanupAndExit(null))
  .catch((error) => {
    console.error(error instanceof Error ? error.message : String(error));
    cleanupAndExit(null);
  });
