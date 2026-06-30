#!/usr/bin/env node
import crypto from "node:crypto";
import fs from "node:fs";
import net from "node:net";
import path from "node:path";
import process from "node:process";
import { spawn, spawnSync } from "node:child_process";

const OPEN_REQUEST_ENV = "ELASTOS_BROWSER_VM_OPEN_REQUEST";

const sshHost = process.env.ELASTOS_BROWSER_REMOTE_VZ_SSH || "elastos-mac-staging";
const sshBin = process.env.ELASTOS_BROWSER_REMOTE_VZ_SSH_BIN || "ssh";
const localRoot = process.env.ELASTOS_BROWSER_REMOTE_VZ_LOCAL_ROOT || "/tmp/evzl";
const remoteRoot = process.env.ELASTOS_BROWSER_REMOTE_VZ_ROOT || "/tmp/evzs";
const remoteRelayRoot = process.env.ELASTOS_BROWSER_REMOTE_VZ_RELAY_ROOT || "/tmp";
const remoteProfileRoot = process.env.ELASTOS_BROWSER_REMOTE_VZ_PROFILE_ROOT || "";
const remoteRelayMaxSessions = Number(process.env.ELASTOS_BROWSER_REMOTE_VZ_RELAY_MAX_SESSIONS || "16");
const launchTimeoutMs = Number(process.env.ELASTOS_BROWSER_REMOTE_VZ_LAUNCH_TIMEOUT_MS || "180000");
const defaultControlProxyRequestTimeoutMs = "120000";
const socketTimeoutMs = Number(process.env.ELASTOS_BROWSER_REMOTE_VZ_SOCKET_TIMEOUT_MS || "15000");
const unixSocketPathBudget = 100;
const maxControlRequestBytes = 4 * 1024 * 1024;
const vmIceServerEnvKeys = [
  "ELASTOS_BROWSER_VM_ICE_SERVER",
  "ELASTOS_BROWSER_VM_ICE_SERVERS_JSON",
  "ELASTOS_BROWSER_VM_ICE_USERNAME",
  "ELASTOS_BROWSER_VM_ICE_CREDENTIAL",
  "ELASTOS_BROWSER_VM_ICE_TRANSPORT_POLICY",
];
const vmMediaRelayEnvKeys = [
  "ELASTOS_BROWSER_VM_MEDIA_RELAY_HOST_IPV4",
  "ELASTOS_BROWSER_VM_MEDIA_RELAY_GUEST_IPV4",
  "ELASTOS_BROWSER_VM_MEDIA_RELAY_PREFIX",
];
const vmIceEnvKeys = [...vmIceServerEnvKeys, ...vmMediaRelayEnvKeys];
const vmTurnEnvKeys = new Set(vmIceEnvKeys);

const children = new Set();
const localServers = new Set();
const cleanupCommands = [];

function fail(message) {
  console.error(message);
  process.exit(1);
}

function sessionSuffix(value) {
  const hash = crypto.createHash("sha256").update(String(value || "stream")).digest("hex").slice(0, 16);
  return `bvm-${hash}-${crypto.randomBytes(4).toString("hex")}`;
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

function spawnTracked(command, args, options = {}) {
  const child = spawn(command, args, options);
  children.add(child);
  child.once("exit", () => children.delete(child));
  return child;
}

function trackLocalServer(server, socketPath = "") {
  const record = { server, socketPath };
  localServers.add(record);
  server.once("close", () => localServers.delete(record));
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

function resolveRemoteHome() {
  const value = runSsh('printf "%s" "$HOME"', { timeoutMs: 5000 }).trim();
  validateAbsolutePath(value, "remote Browser VZ HOME");
  return value;
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

function hasVmIceEnv(env) {
  return Boolean(env.ELASTOS_BROWSER_VM_ICE_SERVER || env.ELASTOS_BROWSER_VM_ICE_SERVERS_JSON);
}

function hasVmMediaRelayEnv(env) {
  return Boolean(
    env.ELASTOS_BROWSER_VM_MEDIA_RELAY_HOST_IPV4 &&
    env.ELASTOS_BROWSER_VM_MEDIA_RELAY_GUEST_IPV4 &&
    env.ELASTOS_BROWSER_VM_MEDIA_RELAY_PREFIX,
  );
}

function parseRemoteTurnEnv(raw, label) {
  const values = {};
  for (const line of String(raw || "").split(/\n/)) {
    const trimmed = line.trim();
    if (!trimmed || trimmed.startsWith("#")) continue;
    const separator = trimmed.indexOf("=");
    if (separator <= 0) {
      throw new Error(`${label} contains a malformed TURN env line`);
    }
    const key = trimmed.slice(0, separator);
    const value = trimmed.slice(separator + 1);
    if (!vmTurnEnvKeys.has(key)) continue;
    if (/[\r\n\0]/.test(value)) {
      throw new Error(`${key} in ${label} must not contain control characters`);
    }
    values[key] = value;
  }
  return values;
}

function remoteRuntimeTurnEnv(remoteDataDir) {
  const env = {};
  for (const key of vmIceServerEnvKeys) {
    const value = process.env[key];
    if (value == null || value === "") continue;
    if (/[\r\n\0]/.test(value)) {
      throw new Error(`${key} must not contain control characters`);
    }
    env[key] = value;
  }
  const hasViewerIceEnv = hasVmIceEnv(env);

  const remoteHome = resolveRemoteHome();
  const candidates = [
    process.env.ELASTOS_BROWSER_REMOTE_VZ_TURN_ENV || "",
    path.posix.join(remoteHome, "runtime-turn", "turn-credentials.env"),
    path.posix.join(remoteDataDir, "runtime-turn", "turn-credentials.env"),
  ].filter(Boolean);
  for (const candidate of [...new Set(candidates)]) {
    validateAbsolutePath(candidate, "remote Browser VZ TURN env path");
    const raw = runSsh(
      `[ -f ${shellQuote(candidate)} ] && cat ${shellQuote(candidate)} || true`,
      { timeoutMs: 5000 },
    );
    const loaded = parseRemoteTurnEnv(raw, candidate);
    if (!hasVmIceEnv(loaded)) continue;
    const keys = hasViewerIceEnv ? vmMediaRelayEnvKeys : vmIceEnvKeys;
    for (const key of keys) {
      if (loaded[key] != null && loaded[key] !== "" && !env[key]) {
        env[key] = loaded[key];
      }
    }
    if (hasViewerIceEnv && !hasVmMediaRelayEnv(env)) {
      throw new Error(
        `remote Browser VZ TURN env is missing VM media relay settings: ${candidate}`,
      );
    }
    return env;
  }
  if (hasViewerIceEnv) {
    throw new Error(
      "remote Browser VZ launch has viewer ICE credentials but no remote VM media relay settings; run setup-source-home on the Mac provider",
    );
  }
  return env;
}

function readOpenRequest() {
  const stdin = fs.readFileSync(0, "utf8");
  const raw = stdin.trim() ? stdin : process.env[OPEN_REQUEST_ENV] || "";
  if (!raw.trim()) fail(`${OPEN_REQUEST_ENV} or stdin JSON is required`);
  try {
    return JSON.parse(raw);
  } catch (error) {
    fail(`Browser VM remote VZ request is invalid JSON: ${error instanceof Error ? error.message : String(error)}`);
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
  if (request.schema !== "elastos.browser.vm-engine.open/v1") {
    throw new Error("remote VZ launcher requires elastos.browser.vm-engine.open/v1");
  }
  const launch = request.launch_request || {};
  if (launch.schema !== "elastos.browser.engine.launch-request/v1") {
    throw new Error("remote VZ launcher missing Browser launch_request");
  }
  if (!/^[A-Za-z0-9:_-]+$/.test(launch.stream_id || "")) {
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
  return launch;
}

function cloneForRemote(request, remoteRelayPath, remoteDataDir) {
  const copy = JSON.parse(JSON.stringify(request));
  copy.launch_request.adapter_ipc = {
    ...copy.launch_request.adapter_ipc,
    runtime_stream_path: remoteRelayPath,
  };
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

async function startLocalTcpToUnixBridge(localUnixPath) {
  const server = net.createServer((client) => {
    const upstream = net.connect(localUnixPath);
    bridgeSockets(client, upstream);
  });
  trackLocalServer(server);
  await listen(server, 0, "127.0.0.1");
  const address = server.address();
  if (!address || typeof address !== "object") {
    throw new Error("local relay TCP bridge did not expose a TCP address");
  }
  return address.port;
}

function startTcpRemoteForward(localTcpPort, remoteTcpPort) {
  const child = spawnTracked(sshBin, [
    ...sshBaseArgs(),
    "-N",
    "-R",
    `127.0.0.1:${remoteTcpPort}:127.0.0.1:${localTcpPort}`,
    sshHost,
  ], {
    stdio: ["ignore", "ignore", "pipe"],
  });
  child.stderr.on("data", (chunk) => {
    process.stderr.write(`[remote-vz tcp-forward] ${chunk}`);
  });
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
  try {
    fs.unlinkSync(localUnixPath);
  } catch (error) {
    if (error?.code !== "ENOENT") throw error;
  }
  const server = net.createServer((client) => {
    readHttpRequestFromClient(client)
      .then((request) => {
        const remoteScript = [
          "set -euo pipefail",
          `export ELASTOS_BRIDGE_UNIX=${shellQuote(remoteUnixPath)}`,
          `exec python3 -u -c ${shellQuote(remoteUnixPipeScript())}`,
        ].join("\n");
        const child = spawnTracked(sshBin, sshControlRemoteShellArgs(remoteScript), {
          stdio: ["pipe", "pipe", "pipe"],
        });
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
  trackLocalServer(server, localUnixPath);
  await listen(server, localUnixPath);
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

function startRemoteUnixToTcpBridge(remoteUnixPath, remoteTcpPort, remotePidPath) {
  runSsh(`rm -f ${shellQuote(remoteUnixPath)} ${shellQuote(remotePidPath)}`);
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
    "import os, socket, sys, threading",
    "path = os.environ['ELASTOS_BRIDGE_UNIX']",
    "port = int(os.environ['ELASTOS_BRIDGE_PORT'])",
    "max_sessions = max(1, int(os.environ.get('ELASTOS_BRIDGE_MAX_SESSIONS', '32')))",
    "try:",
    "    os.unlink(path)",
    "except FileNotFoundError:",
    "    pass",
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
  });
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
    'if [ -f "$pid_file" ]; then',
    '  pid=$(cat "$pid_file" 2>/dev/null || true)',
    '  rm -f "$pid_file"',
    '  case "$pid" in',
    "    ''|*[!0-9]*) ;;",
    '    *) kill "$pid" 2>/dev/null || true; sleep 1; kill -KILL "$pid" 2>/dev/null || true ;;',
    '  esac',
    'fi',
    'rm -f "$socket_path"',
  ].join("\n");
}

async function startRemoteRelayTunnel(localRelayPath, remoteRelayPath, remoteRelayPidPath, suffix) {
  runSsh(`rm -f ${shellQuote(remoteRelayPath)} ${shellQuote(remoteRelayPidPath)}`);
  cleanupCommands.push(remoteRelayCleanupCommand(remoteRelayPidPath, remoteRelayPath));
  const localTcpPort = await startLocalTcpToUnixBridge(localRelayPath);
  const remoteTcpPort = portForSuffix(suffix, "relay");
  startTcpRemoteForward(localTcpPort, remoteTcpPort);
  waitForRemoteTcpPort(remoteTcpPort);
  const shim = startRemoteUnixToTcpBridge(remoteRelayPath, remoteTcpPort, remoteRelayPidPath);
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

function remoteSupervisorCleanupCommand(pidPath) {
  return [
    `pid_file=${shellQuote(pidPath)}`,
    'if [ -f "$pid_file" ]; then',
    '  pid=$(cat "$pid_file" 2>/dev/null || true)',
    '  rm -f "$pid_file"',
    '  case "$pid" in',
    "    ''|*[!0-9]*) ;;",
    '    *)',
    '      proc_command=$(ps -p "$pid" -o command= 2>/dev/null || true)',
    '      case "$proc_command" in',
    '        *browser-vz-engine-supervisor*) kill "$pid" 2>/dev/null || true; sleep 1; kill -KILL "$pid" 2>/dev/null || true ;;',
    '      esac',
    '      ;;',
    '  esac',
    'fi',
  ].join("\n");
}

function remoteStaleSupervisorCleanupCommand(root) {
  return [
    `supervisor_root=${shellQuote(root)}`,
    'if [ -d "$supervisor_root" ]; then',
    '  for pid_file in "$supervisor_root"/supervisor-*.pid; do',
    '    [ -e "$pid_file" ] || continue',
    '    pid=$(cat "$pid_file" 2>/dev/null || true)',
    '    rm -f "$pid_file"',
    '    case "$pid" in',
    "      ''|*[!0-9]*) continue ;;",
    '    esac',
    '    proc_command=$(ps -p "$pid" -o command= 2>/dev/null || true)',
    '    case "$proc_command" in',
    '      *browser-vz-engine-supervisor*)',
    '        printf "%s\\n" "[remote-vz cleanup] reaping stale browser-vz-engine-supervisor pid=$pid" >&2',
    '        kill "$pid" 2>/dev/null || true',
    '        sleep 1',
    '        kill -KILL "$pid" 2>/dev/null || true',
    '        ;;',
    '    esac',
    '  done',
    'fi',
  ].join("\n");
}

function remoteStaleRelayCleanupCommand(root) {
  return [
    `relay_root=${shellQuote(root)}`,
    'if [ -d "$relay_root" ]; then',
    '  for pid_file in "$relay_root"/relay-bvm-*.pid; do',
    '    [ -e "$pid_file" ] || continue',
    '    pid=$(cat "$pid_file" 2>/dev/null || true)',
    '    rm -f "$pid_file"',
    '    case "$pid" in',
    "      ''|*[!0-9]*) continue ;;",
    '    esac',
    '    proc_command=$(ps -p "$pid" -o command= 2>/dev/null || true)',
    '    case "$proc_command" in',
    '      *python*ELASTOS_BRIDGE_UNIX*|*python*)',
    '        printf "%s\\n" "[remote-vz cleanup] reaping stale relay bridge pid=$pid" >&2',
    '        kill "$pid" 2>/dev/null || true',
    '        sleep 1',
    '        kill -KILL "$pid" 2>/dev/null || true',
    '        ;;',
    '    esac',
    '  done',
    '  ps -axo pid=,command= 2>/dev/null | while IFS= read -r line; do',
    '    case "$line" in',
    '      *"ELASTOS_BRIDGE_PIDFILE="*"$relay_root"/relay-bvm-*.pid*)',
    '        pid=$(printf "%s\\n" "$line" | awk \'{print $1}\')',
    '        case "$pid" in',
    "          ''|*[!0-9]*) continue ;;",
    '        esac',
    '        printf "%s\\n" "[remote-vz cleanup] reaping stale relay shim pid=$pid" >&2',
    '        kill "$pid" 2>/dev/null || true',
    '        sleep 1',
    '        kill -KILL "$pid" 2>/dev/null || true',
    '        ;;',
    '    esac',
    '  done',
    '  rm -f "$relay_root"/relay-bvm-*.pid',
    'fi',
  ].join("\n");
}

function reapStaleRemoteSupervisorsEnabled() {
  return !/^(0|false|FALSE|no|NO)$/.test(String(process.env.ELASTOS_BROWSER_REMOTE_VZ_REAP_STALE_SUPERVISORS || "1"));
}

function reapStaleRemoteRelaysEnabled() {
  return !/^(0|false|FALSE|no|NO)$/.test(String(process.env.ELASTOS_BROWSER_REMOTE_VZ_REAP_STALE_RELAYS || "1"));
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
  if (!tail) return new Error(message);
  return new Error(`${message}: ${tail.slice(-20000)}`);
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

function startRemoteSupervisor(remoteRequest, remoteVmEnv, supervisorPidPath) {
  const remoteDataDirExpr = process.env.ELASTOS_BROWSER_REMOTE_VZ_DATA_DIR
    ? shellQuote(process.env.ELASTOS_BROWSER_REMOTE_VZ_DATA_DIR)
    : '"$HOME/Library/Application Support/elastos"';
  cleanupCommands.push(remoteSupervisorCleanupCommand(supervisorPidPath));
  const remoteScript = [
    "set -euo pipefail",
    `DATA=${remoteDataDirExpr}`,
    `PIDFILE=${shellQuote(supervisorPidPath)}`,
    `mkdir -p "$(dirname "$PIDFILE")"`,
    "REQUEST_FILE=$(mktemp \"${TMPDIR:-/tmp}/elastos-remote-vz-request.XXXXXX\")",
    "trap 'rm -f \"$PIDFILE\" \"$REQUEST_FILE\"' INT TERM HUP EXIT",
    "cat >\"$REQUEST_FILE\"",
    `export ELASTOS_BROWSER_VM_DATA_DIR="$DATA"`,
    `export ELASTOS_BROWSER_VM_ROOT=${shellQuote(remoteRoot)}`,
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
    ...optionalRemoteEnvExports(vmIceEnvKeys, remoteVmEnv),
    "supervisor_pid=",
    "cleanup_supervisor() {",
    "  status=$?",
    "  trap - INT TERM HUP EXIT",
    "  if [ -n \"${supervisor_pid:-}\" ]; then",
    "    kill \"$supervisor_pid\" 2>/dev/null || true",
    "    sleep 1",
    "    kill -KILL \"$supervisor_pid\" 2>/dev/null || true",
    "    wait \"$supervisor_pid\" 2>/dev/null || true",
    "  fi",
    "  rm -f \"$PIDFILE\" \"$REQUEST_FILE\"",
    "  exit \"$status\"",
    "}",
    "trap cleanup_supervisor INT TERM HUP EXIT",
    `"$DATA/bin/browser-vz-engine-supervisor" <"$REQUEST_FILE" &`,
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
  const child = spawnTracked(sshBin, sshRemoteShellArgs(remoteScript), {
    stdio: ["pipe", "pipe", "pipe"],
    env: process.env,
  });
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
  child.stdin.end(`${JSON.stringify(remoteRequest)}\n`);
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
  await startLocalUnixToRemoteUnixBridge(localControlPath, remoteControlPath);
  await waitForLocalControlHttp(localControlPath);
  cleanupCommands.push(`rm -f ${shellQuote(remoteControlPath)}`);
}

function rewriteResult(result, launch, localSessionDir, localControlPath, remoteControlPath, remoteRelayPath) {
  validateAbsolutePath(remoteControlPath, "remote supervisor control_socket_path");
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

async function cleanupAndExit(signal) {
  for (const { server, socketPath } of Array.from(localServers)) {
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
  for (const command of cleanupCommands.reverse()) {
    try {
      runSsh(command, { timeoutMs: 5000 });
    } catch {}
  }
  setTimeout(() => {
    for (const child of Array.from(children)) {
      try {
        child.kill("SIGKILL");
      } catch {}
    }
    process.exit(signal ? 0 : 1);
  }, 500).unref();
}

async function main() {
  validateAbsolutePath(localRoot, "ELASTOS_BROWSER_REMOTE_VZ_LOCAL_ROOT");
  validateAbsolutePath(remoteRoot, "ELASTOS_BROWSER_REMOTE_VZ_ROOT");
  validateAbsolutePath(remoteRelayRoot, "ELASTOS_BROWSER_REMOTE_VZ_RELAY_ROOT");
  if (!Number.isInteger(remoteRelayMaxSessions) || remoteRelayMaxSessions < 1 || remoteRelayMaxSessions > 256) {
    throw new Error("ELASTOS_BROWSER_REMOTE_VZ_RELAY_MAX_SESSIONS must be an integer from 1 to 256");
  }

  const request = readOpenRequest();
  const launch = validateOpenRequest(request);
  const remoteDataDir = resolveRemoteDataDir();
  if (reapStaleRemoteSupervisorsEnabled()) {
    runSsh(remoteStaleSupervisorCleanupCommand(remoteRoot), { timeoutMs: 10000 });
  }
  if (reapStaleRemoteRelaysEnabled()) {
    runSsh(remoteStaleRelayCleanupCommand(remoteRoot), { timeoutMs: 10000 });
  }
  const remoteVmEnv = remoteRuntimeTurnEnv(remoteDataDir);
  const suffix = sessionSuffix(launch.stream_id);
  const localSessionDir = path.join(localRoot, suffix);
  const localControlPath = path.join(localSessionDir, "control.sock");
  const remoteRelayPath = path.join(remoteRelayRoot, `ebv-${suffix}.sock`);
  const remoteRelayPidPath = path.join(remoteRoot, `relay-${suffix}.pid`);
  const remoteSupervisorPidPath = path.join(remoteRoot, `supervisor-${suffix}.pid`);
  validateUnixSocketPathBudget(localControlPath, "local control socket path");
  validateUnixSocketPathBudget(remoteRelayPath, "remote relay socket path");

  fs.mkdirSync(localSessionDir, { recursive: true, mode: 0o700 });
  await startRemoteRelayTunnel(launch.adapter_ipc.runtime_stream_path, remoteRelayPath, remoteRelayPidPath, suffix);

  const remoteRequest = cloneForRemote(request, remoteRelayPath, remoteDataDir);
  const remoteSupervisor = startRemoteSupervisor(remoteRequest, remoteVmEnv, remoteSupervisorPidPath);
  const remoteResult = await readFirstJsonLine(remoteSupervisor);
  const remoteControlPath = remoteResult.control_socket_path;

  await startControlTunnel(localControlPath, remoteControlPath);

  const result = rewriteResult(
    remoteResult,
    launch,
    localSessionDir,
    localControlPath,
    remoteControlPath,
    remoteRelayPath,
  );
  process.stdout.write(`${JSON.stringify(result)}\n`);
}

process.on("SIGTERM", () => cleanupAndExit("SIGTERM"));
process.on("SIGINT", () => cleanupAndExit("SIGINT"));
process.on("SIGHUP", () => cleanupAndExit("SIGHUP"));

main().catch((error) => {
  console.error(error instanceof Error ? error.message : String(error));
  cleanupAndExit(null);
});
