#!/usr/bin/env node
import childProcess from "node:child_process";
import crypto from "node:crypto";
import dgram from "node:dgram";
import fs from "node:fs";
import net from "node:net";
import os from "node:os";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

const DEFAULT_PORT = 3478;
const DEFAULT_MIN_RELAY_PORT = 49160;
const DEFAULT_MAX_RELAY_PORT = 49200;
const DEFAULT_DARWIN_MEDIA_HOST_IPV4 = "192.168.65.1";
const DEFAULT_DARWIN_MEDIA_GUEST_IPV4 = "192.168.65.2";
const DEFAULT_MEDIA_RELAY_PREFIX = "24";
const USERNAME = "elastos-browser";
const REALM = "elastos-runtime";

function usage() {
  return `Usage:
  node scripts/browser-runtime-turn.mjs --data-dir /absolute/elastos/data-dir [options]

Options:
  --host-ip <ipv4>          Runtime-owned TURN relay IPv4. Default: detected primary IPv4.
  --media-host-ip <ipv4>    VM-reachable host IPv4. Darwin default: 192.168.65.1.
  --media-guest-ip <ipv4>   VM guest media IPv4. Darwin default: 192.168.65.2.
  --media-prefix <1..32>    VM media link prefix. Default: 24.
  --turnserver <path>       turnserver binary. Default: command lookup.
  --port <port>             TURN listen port. Default: 3481 on Linux, 3478 elsewhere.
  --udp-only                Advertise/start UDP TURN only. Default on Linux.
  --no-start                Only write config and credentials.
`;
}

function parseArgs(argv) {
  const args = {};
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    const next = () => {
      index += 1;
      if (index >= argv.length || argv[index].startsWith("--")) {
        throw new Error(`${arg} requires a value`);
      }
      return argv[index];
    };
    switch (arg) {
      case "--help":
      case "-h":
        console.log(usage());
        process.exit(0);
      case "--data-dir":
        args.dataDir = next();
        break;
      case "--host-ip":
        args.hostIp = next();
        break;
      case "--media-host-ip":
        args.mediaHostIp = next();
        break;
      case "--media-guest-ip":
        args.mediaGuestIp = next();
        break;
      case "--media-prefix":
        args.mediaPrefix = next();
        break;
      case "--turnserver":
        args.turnserver = next();
        break;
      case "--port":
        args.port = Number(next());
        break;
      case "--udp-only":
        args.udpOnly = true;
        break;
      case "--no-start":
        args.noStart = true;
        break;
      default:
        throw new Error(`unknown option: ${arg}`);
    }
  }
  return args;
}

function assertAbsolute(label, value) {
  if (typeof value !== "string" || !value.startsWith("/") || /[\r\n\0]/.test(value)) {
    throw new Error(`${label} must be an absolute path without control characters`);
  }
}

function isIpv4(value) {
  const parts = String(value || "").split(".");
  if (parts.length !== 4) return false;
  return parts.every((part) => {
    if (!/^(?:0|[1-9][0-9]{0,2})$/.test(part)) return false;
    const octet = Number(part);
    return Number.isInteger(octet) && octet >= 0 && octet <= 255;
  });
}

function isUsableHostIpv4(value) {
  return (
    isIpv4(value) &&
    value !== "0.0.0.0" &&
    !value.startsWith("127.") &&
    !value.startsWith("169.254.")
  );
}

function detectDefaultRouteIpv4() {
  if (process.platform !== "linux") return "";
  try {
    const output = childProcess.execFileSync("ip", ["-4", "route", "get", "1.1.1.1"], {
      encoding: "utf8",
      stdio: ["ignore", "pipe", "ignore"],
      timeout: 2_000,
    });
    const match = output.match(/\bsrc\s+([0-9.]+)/);
    return match && isUsableHostIpv4(match[1]) ? match[1] : "";
  } catch {
    return "";
  }
}

function detectPrimaryIpv4() {
  const routeIpv4 = detectDefaultRouteIpv4();
  if (routeIpv4) return routeIpv4;
  const interfaces = os.networkInterfaces();
  for (const name of Object.keys(interfaces).sort()) {
    for (const address of interfaces[name] || []) {
      if (address.family === "IPv4" && !address.internal && isUsableHostIpv4(address.address)) {
        return address.address;
      }
    }
  }
  throw new Error("could not detect a usable host IPv4 for Browser Runtime TURN");
}

function defaultMediaHostIp(hostIp) {
  return process.platform === "darwin" ? DEFAULT_DARWIN_MEDIA_HOST_IPV4 : hostIp;
}

function defaultMediaGuestIp(mediaHostIp) {
  if (process.platform === "darwin" && mediaHostIp === DEFAULT_DARWIN_MEDIA_HOST_IPV4) {
    return DEFAULT_DARWIN_MEDIA_GUEST_IPV4;
  }
  const parts = mediaHostIp.split(".");
  if (parts.length !== 4) return "";
  parts[3] = "2";
  return parts.join(".");
}

function validateMediaPrefix(value) {
  const trimmed = String(value || "").trim();
  if (!/^(?:[1-9]|[12][0-9]|3[0-2])$/.test(trimmed)) {
    throw new Error("--media-prefix must be 1..32");
  }
  return trimmed;
}

function envFlag(name) {
  const raw = String(process.env[name] || "").trim().toLowerCase();
  if (!raw) return null;
  if (["1", "true", "yes", "on"].includes(raw)) return true;
  if (["0", "false", "no", "off"].includes(raw)) return false;
  throw new Error(`${name} must be 1/0, true/false, yes/no, or on/off`);
}

function defaultUdpOnly() {
  const configured = envFlag("ELASTOS_BROWSER_RUNTIME_TURN_UDP_ONLY");
  if (configured !== null) return configured;
  return process.platform === "linux";
}

function defaultPort() {
  const configured = String(process.env.ELASTOS_BROWSER_RUNTIME_TURN_PORT || "").trim();
  if (configured) return Number(configured);
  return process.platform === "linux" ? 3481 : DEFAULT_PORT;
}

function pushTurnUrls(urls, hostIp, port, { udpOnly = false } = {}) {
  const transports = udpOnly ? ["udp"] : ["udp", "tcp"];
  for (const transport of transports) {
    const url = `turn:${hostIp}:${port}?transport=${transport}`;
    if (!urls.includes(url)) urls.push(url);
  }
}

function which(program) {
  const searchPath = process.env.PATH || "";
  for (const dir of searchPath.split(path.delimiter)) {
    if (!dir) continue;
    const candidate = path.join(dir, program);
    try {
      fs.accessSync(candidate, fs.constants.X_OK);
      return candidate;
    } catch {}
  }
  return "";
}

function resolveTurnserver(explicit) {
  if (explicit) {
    assertAbsolute("--turnserver", explicit);
    fs.accessSync(explicit, fs.constants.X_OK);
    return explicit;
  }
  for (const candidate of [
    which("turnserver"),
    "/opt/homebrew/bin/turnserver",
    "/usr/local/bin/turnserver",
    "/usr/bin/turnserver",
  ]) {
    if (!candidate) continue;
    try {
      fs.accessSync(candidate, fs.constants.X_OK);
      return candidate;
    } catch {}
  }
  throw new Error("turnserver was not found; install coturn or pass --turnserver");
}

function parseEnvFile(file) {
  const values = {};
  if (!fs.existsSync(file)) return values;
  const fd = fs.openSync(file, fs.constants.O_RDONLY | fs.constants.O_NOFOLLOW);
  let raw;
  try {
    const stat = fs.fstatSync(fd);
    if (!stat.isFile() || stat.nlink !== 1 || stat.uid !== process.getuid() || stat.size > 65536) {
      throw new Error("runtime TURN needs a bounded user-owned credential file");
    }
    const buffer = Buffer.alloc(65537);
    const count = fs.readSync(fd, buffer, 0, buffer.length, 0);
    if (count > 65536) throw new Error("runtime TURN credential file is too large");
    raw = buffer.subarray(0, count).toString("utf8");
  } finally { fs.closeSync(fd); }
  for (const line of raw.split(/\n/)) {
    const trimmed = line.trim();
    if (!trimmed || trimmed.startsWith("#")) continue;
    const separator = trimmed.indexOf("=");
    if (separator <= 0) continue;
    values[trimmed.slice(0, separator)] = trimmed.slice(separator + 1);
  }
  return values;
}

function existingCredential(values) {
  const raw = values.ELASTOS_BROWSER_VM_ICE_SERVERS_JSON;
  if (raw) {
    try {
      const parsed = JSON.parse(raw);
      for (const entry of Array.isArray(parsed) ? parsed : []) {
        if (typeof entry?.credential === "string" && entry.credential.length >= 16) {
          return entry.credential;
        }
      }
    } catch {}
  }
  return "";
}

function wait(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function tcpReachable(host, port, timeoutMs = 750) {
  return new Promise((resolve) => {
    const socket = net.createConnection({ host, port });
    const finish = (ok) => {
      socket.removeAllListeners();
      socket.destroy();
      resolve(ok);
    };
    socket.setTimeout(timeoutMs);
    socket.once("connect", () => finish(true));
    socket.once("timeout", () => finish(false));
    socket.once("error", () => finish(false));
  });
}

function processCommand(pid) {
  try {
    return childProcess
      .execFileSync("ps", ["-p", String(pid), "-o", "command="], { encoding: "utf8" })
      .trim();
  } catch {
    return "";
  }
}

function readPid(pidFile) {
  try {
    const pid = Number(fs.readFileSync(pidFile, "utf8").trim());
    return Number.isInteger(pid) && pid > 0 ? pid : 0;
  } catch {
    return 0;
  }
}

function ownedTurnProcess(pid, runtimeDir) {
  if (!pid) return false;
  const command = processCommand(pid);
  return command.includes("turnserver") && command.includes(runtimeDir);
}

async function stopExistingTurn(pidFile, runtimeDir) {
  const pid = readPid(pidFile);
  if (!ownedTurnProcess(pid, runtimeDir)) {
    fs.rmSync(pidFile, { force: true });
    return;
  }
  process.kill(pid, "SIGTERM");
  for (let attempt = 0; attempt < 40; attempt += 1) {
    await wait(100);
    if (!ownedTurnProcess(pid, runtimeDir)) {
      fs.rmSync(pidFile, { force: true });
      return;
    }
  }
  throw new Error(`timed out stopping existing runtime TURN pid ${pid}`);
}

async function waitForTurn(host, port) {
  for (let attempt = 0; attempt < 80; attempt += 1) {
    if (await tcpReachable(host, port)) return true;
    await wait(125);
  }
  return false;
}

// RFC 5389 long-term authentication and RFC 5766 Allocate/Refresh.
// The password stays in this process. Each exchange has a bounded UDP reply
// and a deadline; authenticated replies must carry valid MESSAGE-INTEGRITY.
function turnAttribute(type, value) {
  const bytes = Buffer.isBuffer(value) ? value : Buffer.from(value);
  const attr = Buffer.alloc(4 + Math.ceil(bytes.length / 4) * 4);
  attr.writeUInt16BE(type, 0);
  attr.writeUInt16BE(bytes.length, 2);
  bytes.copy(attr, 4);
  return attr;
}

export function turnPacket(type, transaction, attrs, key = null) {
  const body = Buffer.concat(attrs);
  const header = Buffer.alloc(20);
  header.writeUInt16BE(type, 0);
  header.writeUInt16BE(body.length + (key ? 24 : 0), 2);
  header.writeUInt32BE(0x2112a442, 4);
  transaction.copy(header, 8);
  const prefix = Buffer.concat([header, body]);
  return key ? Buffer.concat([prefix, turnAttribute(0x0008, crypto.createHmac("sha1", key).update(prefix).digest())]) : prefix;
}

export function readTurnPacket(packet, transaction, key) {
  if (packet.length < 20 || packet.length > 65507 || packet.readUInt32BE(4) !== 0x2112a442 ||
      !packet.subarray(8, 20).equals(transaction) || packet.readUInt16BE(2) + 20 !== packet.length) return null;
  const attrs = new Map();
  let authenticated = false;
  for (let offset = 20; offset < packet.length;) {
    if (offset + 4 > packet.length) return null;
    const type = packet.readUInt16BE(offset), size = packet.readUInt16BE(offset + 2);
    const end = offset + 4 + size, next = offset + 4 + Math.ceil(size / 4) * 4;
    if (next > packet.length || attrs.has(type)) return null;
    if (type === 0x0008) {
      if (!key || size !== 20) return null;
      const prefix = Buffer.from(packet.subarray(0, offset));
      prefix.writeUInt16BE(end - 20, 2);
      authenticated = crypto.timingSafeEqual(crypto.createHmac("sha1", key).update(prefix).digest(), packet.subarray(offset + 4, end));
      break; // Attributes after integrity carry no authenticated meaning.
    }
    attrs.set(type, packet.subarray(offset + 4, end));
    offset = next;
  }
  if (key && !authenticated) return null;
  return { type: packet.readUInt16BE(0), attrs };
}

export async function credentialedTurnAllocation(host, port, credential) {
  const socket = dgram.createSocket("udp4");
  const deadline = Date.now() + 2000;
  async function exchange(type, attrs, key = null) {
    const transaction = crypto.randomBytes(12);
    const packet = turnPacket(type, transaction, attrs, key);
    return new Promise((resolve, reject) => {
      const finish = (error, result) => {
        clearTimeout(timer);
        socket.off("message", message);
        socket.off("error", failure);
        error ? reject(error) : resolve(result);
      };
      const failure = () => finish(new Error("TURN probe socket failed"));
      const message = (bytes, peer) => {
        if (peer.address !== host || peer.port !== port) return;
        const result = readTurnPacket(bytes, transaction, key);
        if (result) finish(null, result);
      };
      const remaining = deadline - Date.now();
      const timer = setTimeout(() => finish(new Error("TURN probe timed out")), Math.max(0, remaining));
      socket.on("message", message);
      socket.once("error", failure);
      if (remaining <= 0) return;
      socket.send(packet, port, host, error => { if (error) failure(); });
    });
  }
  try {
    const requestedTransport = turnAttribute(0x0019, Buffer.from([17, 0, 0, 0]));
    const challenge = await exchange(0x0003, [requestedTransport]);
    const code = challenge.attrs.get(0x0009);
    const realm = challenge.attrs.get(0x0014), nonce = challenge.attrs.get(0x0015);
    if (challenge.type !== 0x0113 || code?.length < 4 || code?.[2] !== 4 || code?.[3] !== 1 ||
        !realm?.length || realm.length > 763 || !nonce?.length || nonce.length > 763) throw new Error("TURN probe challenge invalid");
    const key = crypto.createHash("md5").update(Buffer.concat([Buffer.from(`${USERNAME}:`), realm, Buffer.from(`:${credential}`)])).digest();
    const auth = [turnAttribute(0x0006, USERNAME), turnAttribute(0x0014, realm), turnAttribute(0x0015, nonce)];
    const allocation = await exchange(0x0003, [requestedTransport, ...auth], key);
    if (allocation.type !== 0x0103 || !allocation.attrs.has(0x0016)) throw new Error("TURN probe allocation refused");
    const released = await exchange(0x0004, [turnAttribute(0x000d, Buffer.alloc(4)), ...auth], key);
    if (released.type !== 0x0104) throw new Error("TURN probe release refused");
    return { ok: true, error: "" };
  } catch (error) {
    return { ok: false, error: error.message };
  } finally {
    try { socket.close(); } catch {}
  }
}

async function waitForCredentialedTurnAllocation(host, port, credential) {
  let last = { ok: false, error: "not checked" };
  for (let attempt = 0; attempt < 20; attempt += 1) {
    last = await credentialedTurnAllocation(host, port, credential);
    if (last.ok) return last;
    await wait(500);
  }
  return last;
}

function writePrivateTurnFile(file, text) {
  const fd = fs.openSync(file, fs.constants.O_WRONLY | fs.constants.O_CREAT | fs.constants.O_NOFOLLOW, 0o600);
  try {
    const stat = fs.fstatSync(fd);
    if (!stat.isFile() || stat.nlink !== 1 || stat.uid !== process.getuid()) throw new Error("runtime TURN needs user-owned private files");
    fs.fchmodSync(fd, 0o600);
    fs.ftruncateSync(fd, 0);
    fs.writeFileSync(fd, text, "utf8");
    fs.fsyncSync(fd);
  } finally { fs.closeSync(fd); }
}

function writeRuntimeTurnFiles({
  runtimeDir,
  hostIp,
  mediaHostIp,
  mediaGuestIp,
  mediaPrefix,
  port,
  credential,
  udpOnly,
}) {
  fs.mkdirSync(runtimeDir, { recursive: true, mode: 0o700 });
  const directory = fs.lstatSync(runtimeDir);
  if (!directory.isDirectory() || directory.uid !== process.getuid()) throw new Error("runtime TURN needs its user-owned directory");
  fs.chmodSync(runtimeDir, 0o700);
  const urls = [];
  pushTurnUrls(urls, hostIp, port, { udpOnly });
  if (mediaHostIp !== hostIp) {
    pushTurnUrls(urls, mediaHostIp, port, { udpOnly });
  }
  const iceServers = [{
    urls,
    username: USERNAME,
    credential,
  }];
  const envFile = path.join(runtimeDir, "turn-credentials.env");
  const configFile = path.join(runtimeDir, "turnserver.conf");
  const pidFile = path.join(runtimeDir, "turnserver.pid");
  const logFile = path.join(runtimeDir, "turnserver.log");
  writePrivateTurnFile(
    envFile,
    [
      `ELASTOS_BROWSER_VM_ICE_SERVERS_JSON=${JSON.stringify(iceServers)}`,
      "ELASTOS_BROWSER_VM_ICE_TRANSPORT_POLICY=relay",
      `ELASTOS_BROWSER_VM_MEDIA_RELAY_HOST_IPV4=${mediaHostIp}`,
      `ELASTOS_BROWSER_VM_MEDIA_RELAY_GUEST_IPV4=${mediaGuestIp}`,
      `ELASTOS_BROWSER_VM_MEDIA_RELAY_PREFIX=${mediaPrefix}`,
      "",
    ].join("\n"),
  );
  writePrivateTurnFile(
    configFile,
    [
      "fingerprint",
      "lt-cred-mech",
      `realm=${REALM}`,
      `user=${USERNAME}:${credential}`,
      ...[...new Set([hostIp, mediaHostIp])].map(ip => `listening-ip=${ip}`),
      `relay-ip=${hostIp}`,
      `external-ip=${hostIp}`,
      `listening-port=${port}`,
      `min-port=${DEFAULT_MIN_RELAY_PORT}`,
      `max-port=${DEFAULT_MAX_RELAY_PORT}`,
      ...(udpOnly ? ["no-tcp"] : []),
      "no-tcp-relay",
      "no-multicast-peers",
      `allowed-peer-ip=${hostIp}`,
      "denied-peer-ip=0.0.0.0-255.255.255.255",
      "denied-peer-ip=::-ffff:ffff:ffff:ffff:ffff:ffff:ffff:ffff",
      "no-tls",
      "no-dtls",
      "no-cli",
      `log-file=${logFile}`,
      `pidfile=${pidFile}`,
      "",
    ].join("\n"),
  );
  return { envFile, configFile, pidFile, logFile, iceServers };
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  if (!args.dataDir) throw new Error("--data-dir is required");
  assertAbsolute("--data-dir", args.dataDir);
  const hostIp = args.hostIp || process.env.ELASTOS_BROWSER_RUNTIME_TURN_HOST_IPV4 || detectPrimaryIpv4();
  if (!isUsableHostIpv4(hostIp)) {
    throw new Error("--host-ip must be a non-loopback IPv4 address");
  }
  const mediaHostIp = args.mediaHostIp || process.env.ELASTOS_BROWSER_VM_MEDIA_RELAY_HOST_IPV4 || defaultMediaHostIp(hostIp);
  if (!isUsableHostIpv4(mediaHostIp)) {
    throw new Error("--media-host-ip must be a non-loopback IPv4 address");
  }
  const mediaGuestIp = args.mediaGuestIp || process.env.ELASTOS_BROWSER_VM_MEDIA_RELAY_GUEST_IPV4 || defaultMediaGuestIp(mediaHostIp);
  if (!isUsableHostIpv4(mediaGuestIp)) {
    throw new Error("--media-guest-ip must be a non-loopback IPv4 address");
  }
  const mediaPrefix = validateMediaPrefix(args.mediaPrefix || process.env.ELASTOS_BROWSER_VM_MEDIA_RELAY_PREFIX || DEFAULT_MEDIA_RELAY_PREFIX);
  const port = args.port || defaultPort();
  if (!Number.isInteger(port) || port < 1 || port > 65535) {
    throw new Error("--port must be 1..65535");
  }
  const udpOnly = Boolean(args.udpOnly || defaultUdpOnly());
  const runtimeDir = path.join(args.dataDir, "runtime-turn");
  const existing = parseEnvFile(path.join(runtimeDir, "turn-credentials.env"));
  const credential = existingCredential(existing) || crypto.randomBytes(24).toString("base64url");
  const files = writeRuntimeTurnFiles({
    runtimeDir,
    hostIp,
    mediaHostIp,
    mediaGuestIp,
    mediaPrefix,
    port,
    credential,
    udpOnly,
  });
  let running = false;
  let turnserver = "";
  let allocationCheck = null;
  let allocationChecks = [];
  if (!args.noStart) {
    turnserver = resolveTurnserver(
      args.turnserver ||
        process.env.ELASTOS_BROWSER_VM_TURNSERVER_BIN ||
        process.env.ELASTOS_TURNSERVER_BIN ||
        "",
    );
    await stopExistingTurn(files.pidFile, runtimeDir);
    const child = childProcess.spawn(turnserver, ["-c", files.configFile, "--daemon"], {
      detached: true,
      stdio: "ignore",
    });
    child.unref();
    running = udpOnly || await waitForTurn(hostIp, port);
    if (!running) {
      throw new Error(`runtime TURN did not become reachable at ${hostIp}:${port}`);
    }
    const check = await waitForCredentialedTurnAllocation(hostIp, port, credential);
    allocationChecks.push({ host: hostIp, port, ...check });
    if (!check.ok) {
      await stopExistingTurn(files.pidFile, runtimeDir).catch(() => {});
      throw new Error(`runtime TURN credentialed allocation failed at ${hostIp}:${port}: ${check.error}`);
    }
    running = true;
    allocationCheck = allocationChecks[0] || null;
  }
  console.log(JSON.stringify({
    ok: true,
    schema: "elastos.browser.runtime-turn/v1",
    host_ip: hostIp,
    media_host_ip: mediaHostIp,
    media_guest_ip: mediaGuestIp,
    media_prefix: mediaPrefix,
    udp_only: udpOnly,
    port,
    env_file: files.envFile,
    config_file: files.configFile,
    pid_file: files.pidFile,
    log_file: files.logFile,
    ice_servers: files.iceServers,
    running,
    allocation_check: allocationCheck,
    allocation_checks: allocationChecks,
    turnserver: turnserver || null,
  }));
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) main().catch((error) => {
  console.error(error instanceof Error ? error.message : String(error));
  console.error(usage());
  process.exit(1);
});
