#!/usr/bin/env node
import crypto from "node:crypto";
import fs from "node:fs";
import net from "node:net";
import path from "node:path";
import process from "node:process";
import { isDeepStrictEqual } from "node:util";
import { pathToFileURL } from "node:url";

const CONFIG_ENV = "ELASTOS_BROWSER_VM_VZ_TRANSPORT_BOOTSTRAP_CONFIG";
const AUTHORITY_SCHEMA = "elastos.browser.vz-transport-authority/v1";
const SECRET_SCHEMA = "elastos.browser.vz-transport-secret/v1";
const REQUEST_SCHEMA = "elastos.browser.vz-transport-bootstrap/v1";
const RECEIPT_SCHEMA = "elastos.browser.vz-transport-bootstrap-receipt/v1";
const MAX_DESCRIPTOR_BYTES = 64 * 1024;

function exactObjectKeys(value, keys) {
  return (
    value &&
    typeof value === "object" &&
    !Array.isArray(value) &&
    Object.keys(value).length === keys.length &&
    keys.every((key) => Object.hasOwn(value, key))
  );
}

function safeId(value) {
  return (
    typeof value === "string" &&
    value.length > 0 &&
    value.length <= 512 &&
    /^[A-Za-z0-9:_-]+$/.test(value)
  );
}

function sha256Label(value) {
  return `sha256:${crypto.createHash("sha256").update(value).digest("hex")}`;
}

function sha256LabelIsSafe(value) {
  return typeof value === "string" && /^sha256:[0-9a-f]{64}$/.test(value);
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

function loopbackLiteral(value) {
  if (typeof value !== "string" || net.isIP(value) === 0) return false;
  if (net.isIPv4(value)) return value.startsWith("127.");
  return value === "::1" || value === "0:0:0:0:0:0:0:1";
}

function port(value, label, max = 65535) {
  if (!Number.isInteger(value) || value < 1 || value > max) {
    throw new Error(`${label} is invalid`);
  }
  return value;
}

function validateStream(stream, loopbackTarget) {
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
    stream.runtime_socket_path.length > 103 ||
    /[\r\n\0]/.test(stream.runtime_socket_path)
  ) {
    throw new Error("Browser VZ transport stream is invalid");
  }
  let target;
  try {
    target = new URL(stream.target);
  } catch {
    throw new Error("Browser VZ transport target is invalid");
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
    throw new Error("Browser VZ transport target is invalid");
  }
  port(stream.vsock_port, "Browser VZ transport vsock port", 0xffffffff);
  return stream;
}

function validateTurn(turn, expiresAtUnixMs) {
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
    port(turn.guest_port, "Browser VZ guest TURN port") < 1 ||
    port(turn.listen_port, "Browser VZ TURN listener port") < 1 ||
    typeof turn.advertised_host !== "string" ||
    !turn.advertised_host ||
    turn.advertised_host.length > 253 ||
    /[\s\r\n\0/\\]/.test(turn.advertised_host) ||
    typeof turn.relay_host !== "string" ||
    net.isIP(turn.relay_host) === 0 ||
    port(turn.relay_port_min, "Browser VZ TURN relay minimum") >
      port(turn.relay_port_max, "Browser VZ TURN relay maximum") ||
    turn.relay_port_max - turn.relay_port_min + 1 > 64 ||
    !isDeepStrictEqual(turn.protocols, ["turn", "tcp"]) ||
    turn.guest_url !==
      `turn:${turn.guest_host}:${turn.guest_port}?transport=tcp` ||
    typeof turn.username !== "string" ||
    !/^[0-9]+:[A-Za-z0-9_-]+$/.test(turn.username) ||
    Number(turn.username.split(":", 1)[0]) * 1000 !== expiresAtUnixMs ||
    !sha256LabelIsSafe(turn.credential_hash) ||
    !sha256LabelIsSafe(turn.auth_secret_hash)
  ) {
    throw new Error("Browser VZ TURN authority is invalid");
  }
}

function validateAuthority(authority) {
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
    authority.schema !== AUTHORITY_SCHEMA ||
    !sha256LabelIsSafe(authority.binding_hash) ||
    !sha256LabelIsSafe(authority.generation) ||
    !safeId(authority.page_id) ||
    !safeId(authority.vm_id) ||
    !safeId(authority.principal_id) ||
    !Number.isSafeInteger(authority.expires_at_unix_ms) ||
    authority.expires_at_unix_ms <= Date.now() ||
    authority.expires_at_unix_ms > Date.now() + 24 * 60 * 60 * 1000
  ) {
    throw new Error("Browser VZ transport authority is invalid");
  }
  const egress = validateStream(authority.egress, false);
  const media = validateStream(authority.media, true);
  port(
    authority.bootstrap_vsock_port,
    "Browser VZ bootstrap vsock port",
    0xffffffff,
  );
  if (
    egress.stream_id === media.stream_id ||
    egress.runtime_socket_path === media.runtime_socket_path ||
    egress.vsock_port === media.vsock_port ||
    authority.bootstrap_vsock_port === egress.vsock_port ||
    authority.bootstrap_vsock_port === media.vsock_port
  ) {
    throw new Error("Browser VZ transport bindings are not distinct");
  }
  validateTurn(authority.turn, authority.expires_at_unix_ms);
  const unsigned = { ...authority };
  delete unsigned.binding_hash;
  if (
    sha256Label(Buffer.from(JSON.stringify(canonicalJson(unsigned)))) !==
      authority.binding_hash ||
    Buffer.byteLength(JSON.stringify(authority)) > 32 * 1024
  ) {
    throw new Error("Browser VZ transport authority hash mismatch");
  }
  return authority;
}

function validateSecret(authority, secret) {
  if (
    !exactObjectKeys(secret, [
      "schema",
      "binding_hash",
      "credential",
      "auth_secret",
    ]) ||
    secret.schema !== SECRET_SCHEMA ||
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
      authority.turn.auth_secret_hash
  ) {
    throw new Error("Browser VZ transport secret is invalid");
  }
  const expected = crypto
    .createHmac("sha1", secret.auth_secret)
    .update(authority.turn.username)
    .digest("base64");
  if (expected !== secret.credential) {
    throw new Error("Browser VZ TURN credential mismatch");
  }
  return secret;
}

function readConfig() {
  const raw = process.env[CONFIG_ENV];
  if (!raw) throw new Error(`${CONFIG_ENV} is required`);
  const config = JSON.parse(raw);
  if (
    !exactObjectKeys(config, [
      "schema",
      "relay_socket_path",
      "authority_path",
      "ice_servers_path",
    ]) ||
    config.schema !==
      "elastos.browser.vz-transport-bootstrap.config/v1"
  ) {
    throw new Error("Browser VZ transport bootstrap config is invalid");
  }
  for (const field of [
    "relay_socket_path",
    "authority_path",
    "ice_servers_path",
  ]) {
    if (
      typeof config[field] !== "string" ||
      !config[field].startsWith("/") ||
      /[\r\n\0]/.test(config[field])
    ) {
      throw new Error(`Browser VZ transport bootstrap ${field} is invalid`);
    }
  }
  return config;
}

function writeOwnerOnlyAtomic(filePath, value) {
  const bytes = Buffer.from(`${JSON.stringify(value)}\n`);
  const temporaryPath = `${filePath}.tmp.${process.pid}.${crypto
    .randomBytes(8)
    .toString("hex")}`;
  fs.mkdirSync(path.dirname(filePath), { recursive: true, mode: 0o700 });
  let fd;
  try {
    fd = fs.openSync(temporaryPath, "wx", 0o600);
    fs.writeFileSync(fd, bytes);
    fs.fchmodSync(fd, 0o600);
    fs.fsyncSync(fd);
    fs.closeSync(fd);
    fd = undefined;
    fs.renameSync(temporaryPath, filePath);
  } finally {
    if (fd !== undefined) fs.closeSync(fd);
    try {
      fs.unlinkSync(temporaryPath);
    } catch (error) {
      if (error?.code !== "ENOENT") throw error;
    }
  }
  const stat = fs.lstatSync(filePath);
  if (!stat.isFile() || (stat.mode & 0o077) !== 0) {
    throw new Error("Browser VZ bootstrap output is not owner-only");
  }
}

function guestNetworkState({
  netClassPath = "/sys/class/net",
  ipv4RoutePath = "/proc/net/route",
  ipv6RoutePath = "/proc/net/ipv6_route",
} = {}) {
  const interfaces = fs
    .readdirSync(netClassPath, { withFileTypes: true })
    .filter((entry) => entry.isDirectory() || entry.isSymbolicLink())
    .map((entry) => entry.name)
    .sort();
  if (!isDeepStrictEqual(interfaces, ["lo"])) {
    throw new Error(
      `Browser VZ guest has unexpected network interfaces: ${interfaces.join(",")}`,
    );
  }
  const ipv4Routes = fs.existsSync(ipv4RoutePath)
    ? fs.readFileSync(ipv4RoutePath, "utf8")
    : "";
  const ipv6Routes = fs.existsSync(ipv6RoutePath)
    ? fs.readFileSync(ipv6RoutePath, "utf8")
    : "";
  const defaultIpv4 = ipv4Routes
    .split(/\n/)
    .slice(1)
    .some((line) => line.trim().split(/\s+/)[1] === "00000000");
  const defaultIpv6 = ipv6Routes
    .split(/\n/)
    .some((line) => line.trim().split(/\s+/)[0] === "0".repeat(32));
  if (defaultIpv4 || defaultIpv6) {
    throw new Error("Browser VZ guest unexpectedly has a default route");
  }
  return { interfaces, default_route_absent: true };
}

function directNetworkErrorProvesUnavailable(error) {
  return [
    "EAI_AGAIN",
    "EHOSTUNREACH",
    "ENETDOWN",
    "ENETUNREACH",
    "ENONET",
    "ENOTFOUND",
  ].includes(error?.code);
}

function proveDirectNetworkUnavailable(authority, timeoutMs = 1_500) {
  const target = new URL(authority.egress.target);
  return new Promise((resolve, reject) => {
    const socket = net.createConnection({
      host: target.hostname,
      port: Number(target.port),
    });
    let settled = false;
    const finish = (result, error = null) => {
      if (settled) return;
      settled = true;
      socket.removeAllListeners();
      socket.destroy();
      if (result === "connected") {
        reject(
          new Error(
            "Browser VZ guest unexpectedly reached its Runtime egress target directly",
          ),
        );
      } else if (result === "absent") {
        resolve(true);
      } else {
        reject(
          new Error(
            `Browser VZ direct-network probe was indeterminate${
              error?.code ? ` (${error.code})` : ""
            }`,
          ),
        );
      }
    };
    socket.setTimeout(timeoutMs);
    socket.once("connect", () => finish("connected"));
    socket.once("timeout", () => finish("indeterminate"));
    socket.once("error", (error) => {
      const provesAbsent = directNetworkErrorProvesUnavailable(error);
      finish(provesAbsent ? "absent" : "indeterminate", error);
    });
  });
}

function readDescriptor(socketPath) {
  return new Promise((resolve, reject) => {
    const socket = net.createConnection({ path: socketPath });
    const chunks = [];
    let bytes = 0;
    const timeout = setTimeout(() => {
      socket.destroy();
      reject(new Error("Browser VZ transport descriptor timed out"));
    }, 30_000);
    socket.on("data", (chunk) => {
      bytes += chunk.length;
      if (bytes > MAX_DESCRIPTOR_BYTES) {
        socket.destroy();
        reject(new Error("Browser VZ transport descriptor is too large"));
        return;
      }
      chunks.push(chunk);
      if (chunk.includes(0x0a)) {
        socket.pause();
        try {
          const payload = Buffer.concat(chunks);
          const line = payload
            .subarray(0, payload.indexOf(0x0a))
            .toString("utf8");
          resolve({ socket, request: JSON.parse(line) });
        } catch (error) {
          socket.destroy();
          reject(error);
        }
      }
    });
    socket.once("error", reject);
    socket.once("close", () => clearTimeout(timeout));
  });
}

async function main() {
  const config = readConfig();
  const { socket, request } = await readDescriptor(config.relay_socket_path);
  if (
    !exactObjectKeys(request, ["schema", "authority", "secret"]) ||
    request.schema !== REQUEST_SCHEMA
  ) {
    throw new Error("Browser VZ transport descriptor envelope is invalid");
  }
  const authority = validateAuthority(request.authority);
  const secret = validateSecret(authority, request.secret);
  const network = guestNetworkState();
  await proveDirectNetworkUnavailable(authority);
  writeOwnerOnlyAtomic(config.authority_path, authority);
  writeOwnerOnlyAtomic(config.ice_servers_path, [
    {
      urls: [authority.turn.guest_url],
      username: authority.turn.username,
      credential: secret.credential,
    },
  ]);
  const receipt = {
    schema: RECEIPT_SCHEMA,
    binding_hash: authority.binding_hash,
    generation: authority.generation,
    page_id: authority.page_id,
    vm_id: authority.vm_id,
    expires_at_unix_ms: authority.expires_at_unix_ms,
    terminal: true,
    effects: {
      descriptor_validated: true,
      authority_owner_only: true,
      ice_config_owner_only: true,
      loopback_only: true,
      interfaces: network.interfaces,
      default_route_absent: network.default_route_absent,
      direct_network_probe_failed: true,
    },
  };
  socket.end(`${JSON.stringify(receipt)}\n`);
}

export {
  directNetworkErrorProvesUnavailable,
  guestNetworkState,
  proveDirectNetworkUnavailable,
  validateAuthority,
  validateSecret,
  validateTurn,
  writeOwnerOnlyAtomic,
};

if (
  process.argv[1] &&
  import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href
) {
  main().catch((error) => {
    process.stderr.write(
      `Browser VZ transport bootstrap failed: ${
        error instanceof Error ? error.message : String(error)
      }\n`,
    );
    process.exit(1);
  });
}
