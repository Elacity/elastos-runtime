#!/usr/bin/env node
import fs from "node:fs";
import path from "node:path";
import process from "node:process";

const SUPPORTED_PLATFORMS = new Set(["linux-amd64", "linux-arm64", "darwin-arm64"]);
const VM_ICE_ENV_KEYS = [
  "ELASTOS_BROWSER_VM_ICE_SERVER",
  "ELASTOS_BROWSER_VM_ICE_SERVERS_JSON",
  "ELASTOS_BROWSER_VM_ICE_USERNAME",
  "ELASTOS_BROWSER_VM_ICE_CREDENTIAL",
  "ELASTOS_BROWSER_VM_ICE_TRANSPORT_POLICY",
];
const VM_MEDIA_RELAY_ENV_KEYS = [
  "ELASTOS_BROWSER_VM_MEDIA_RELAY_HOST_IPV4",
  "ELASTOS_BROWSER_VM_MEDIA_RELAY_GUEST_IPV4",
  "ELASTOS_BROWSER_VM_MEDIA_RELAY_PREFIX",
];
const VM_TURN_ENV_KEYS = [...VM_ICE_ENV_KEYS, ...VM_MEDIA_RELAY_ENV_KEYS];
const VM_DIAGNOSTIC_ENV_KEYS = [
  "ELASTOS_BROWSER_VM_TRACE",
  "ELASTOS_BROWSER_VM_CONTROL_STATUS_PROBE_TIMEOUT_MS",
  "ELASTOS_BROWSER_VM_DEBUG_HOLD_ON_OPEN_ERROR_MS",
];
const VM_LAUNCHER_ENV_KEYS = ["ELASTOS_BROWSER_VM_TURNSERVER_BIN"];
const REMOTE_VZ_PATH_ENV_KEYS = [
  "ELASTOS_BROWSER_REMOTE_VZ_DATA_DIR",
  "ELASTOS_BROWSER_REMOTE_VZ_TURN_ENV",
  "ELASTOS_BROWSER_REMOTE_VZ_PROFILE_ROOT",
];
const VM_GUEST_READY_TIMEOUT_MS = "120000";
const VM_REMOTE_VZ_LAUNCH_TIMEOUT_MS = "180000";
const VM_CONTROL_TIMEOUT_MS = "210000";
const VM_CONTROL_PROXY_REQUEST_TIMEOUT_MS = "120000";
const VM_ENGINE_TIMEOUT_MS = "225000";
const VM_SUPERVISOR_TIMEOUT_MS = 240000;
const VM_EGRESS_MAX_SESSIONS = "16";
const VM_ADAPTER_MAX_ACTIVE_SESSIONS = "4";
const VM_CONTROL_MAX_ACTIVE_PAGES = "1";
const VM_IDLE_KEEPALIVE_MS = "300000";
const VM_LINUX_IDLE_KEEPALIVE_MS = "0";
const VM_REUSE_IDLE_VMS = "1";
const VM_LINUX_REUSE_IDLE_VMS = "0";
const VM_HIBERNATION_MAX_ENTRIES = "4";
const VM_HIBERNATION_MAX_AGE_SECS = "604800";

function usage() {
  return `Usage:
  node scripts/browser-source-home-config.mjs \\
    --data-dir /absolute/elastos/data-dir \\
    --platform linux-amd64|linux-arm64|darwin-arm64

Options:
  --out-dir <dir>                 Default: <data-dir>/config
  --vm-supervisor <path>          Default: <data-dir>/bin/browser-vm-engine-supervisor
  --vm-control-socket <path>      Default: /tmp/elastos-browser-vm-control-<platform>.sock
  --vm-control-launcher <path>    Optional launcher used to auto-start the VM control socket.
                                  Linux default: <data-dir>/bin/browser-vm-local-crosvm-launcher
                                  Darwin default: <data-dir>/bin/browser-vz-engine-supervisor
  --vm-rootfs <path>              Optional Browser VM rootfs path
  --allow-private-targets         Allow Browser Exit to resolve private targets. Disabled by default.
`;
}

function parseArgs(argv) {
  const args = {};
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    const next = () => {
      i += 1;
      if (i >= argv.length || argv[i].startsWith("--")) {
        throw new Error(`${arg} requires a value`);
      }
      return argv[i];
    };
    switch (arg) {
      case "--help":
      case "-h":
        console.log(usage());
        process.exit(0);
      case "--data-dir":
        args.dataDir = next();
        break;
      case "--platform":
        args.platform = next();
        break;
      case "--out-dir":
        args.outDir = next();
        break;
      case "--vm-supervisor":
        args.vmSupervisor = next();
        break;
      case "--vm-control-socket":
        args.vmControlSocket = next();
        break;
      case "--vm-control-launcher":
        args.vmControlLauncher = next();
        break;
      case "--vm-rootfs":
        args.vmRootfs = next();
        break;
      case "--allow-private-targets":
        args.allowPrivateTargets = true;
        break;
      default:
        throw new Error(`unknown option: ${arg}`);
    }
  }
  return args;
}

function validateAbsolute(label, value) {
  if (typeof value !== "string" || !value.startsWith("/") || /[\r\n\0]/.test(value)) {
    throw new Error(`${label} must be an absolute path without control characters`);
  }
}

function hasVmIceEnv(env) {
  return Boolean(env.ELASTOS_BROWSER_VM_ICE_SERVER || env.ELASTOS_BROWSER_VM_ICE_SERVERS_JSON);
}

function isRemoteVzControlLauncher(launcher) {
  return path.basename(String(launcher || "")).startsWith("browser-vm-remote-vz-launcher");
}

function runtimeTurnEnvCandidates(args, env = process.env) {
  const candidates = [];
  if (env.ELASTOS_BROWSER_RUNTIME_TURN_ENV) {
    candidates.push(env.ELASTOS_BROWSER_RUNTIME_TURN_ENV);
  }
  if (env.HOME) {
    candidates.push(path.join(env.HOME, "runtime-turn", "turn-credentials.env"));
  }
  candidates.push(path.join(args.dataDir, "runtime-turn", "turn-credentials.env"));
  return [...new Set(candidates)];
}

function readRuntimeTurnEnvFile(file) {
  const values = {};
  const raw = fs.readFileSync(file, "utf8");
  for (const line of raw.split(/\n/)) {
    const trimmed = line.trim();
    if (!trimmed || trimmed.startsWith("#")) continue;
    const separator = trimmed.indexOf("=");
    if (separator <= 0) {
      throw new Error(`${file} contains a malformed TURN env line`);
    }
    const key = trimmed.slice(0, separator);
    const value = trimmed.slice(separator + 1);
    if (!VM_TURN_ENV_KEYS.includes(key)) continue;
    if (/[\r\n\0]/.test(value)) {
      throw new Error(`${key} in ${file} must not contain control characters`);
    }
    values[key] = value;
  }
  return values;
}

function runtimeTurnEnv(args, env = process.env) {
  const merged = { ...env };
  if (hasVmIceEnv(merged)) return merged;
  for (const file of runtimeTurnEnvCandidates(args, env)) {
    if (!fs.existsSync(file)) continue;
    const loaded = readRuntimeTurnEnvFile(file);
    if (!hasVmIceEnv(loaded)) continue;
    for (const key of VM_TURN_ENV_KEYS) {
      if (loaded[key] != null && loaded[key] !== "" && !merged[key]) {
        merged[key] = loaded[key];
      }
    }
    return merged;
  }
  return merged;
}

function copyVmIceEnv(env, sourceEnv = process.env) {
  for (const key of VM_ICE_ENV_KEYS) {
    const value = sourceEnv[key];
    if (value == null || value === "") continue;
    if (/[\r\n\0]/.test(value)) {
      throw new Error(`${key} must not contain control characters`);
    }
    env[key] = value;
  }
}

function copyVmDiagnosticEnv(env, sourceEnv = process.env) {
  for (const key of VM_DIAGNOSTIC_ENV_KEYS) {
    const value = sourceEnv[key];
    if (value == null || value === "") continue;
    if (/[\r\n\0]/.test(value)) {
      throw new Error(`${key} must not contain control characters`);
    }
    env[key] = value;
  }
}

function copyVmLauncherEnv(env, sourceEnv = process.env) {
  for (const key of VM_LAUNCHER_ENV_KEYS) {
    const value = sourceEnv[key];
    if (value == null || value === "") continue;
    if (/[\r\n\0]/.test(value)) {
      throw new Error(`${key} must not contain control characters`);
    }
    if (!path.isAbsolute(value)) {
      throw new Error(`${key} must be an absolute path`);
    }
    env[key] = value;
  }
}

function copyRemoteVzPathEnv(env, sourceEnv = process.env) {
  for (const key of REMOTE_VZ_PATH_ENV_KEYS) {
    const value = sourceEnv[key];
    if (value == null || value === "") continue;
    if (/[\r\n\0]/.test(value)) {
      throw new Error(`${key} must not contain control characters`);
    }
    if (!path.isAbsolute(value)) {
      throw new Error(`${key} must be an absolute path`);
    }
    env[key] = value;
  }
}

function parseIpv4(value) {
  const parts = String(value || "").split(".");
  if (parts.length !== 4) return null;
  const octets = parts.map((part) => {
    if (!/^(?:0|[1-9][0-9]{0,2})$/.test(part)) return NaN;
    return Number(part);
  });
  if (octets.some((octet) => !Number.isInteger(octet) || octet < 0 || octet > 255)) {
    return null;
  }
  return octets;
}

function isIpv4(value) {
  return parseIpv4(value) != null;
}

function turnIpv4HostFromUrl(value) {
  const match = String(value || "").trim().match(/^turns?:([0-9.]+)(?::|\?|$)/i);
  if (!match || !isIpv4(match[1])) return "";
  return match[1];
}

function collectIceUrlsFromEnv(env) {
  const urls = [];
  if (env.ELASTOS_BROWSER_VM_ICE_SERVER) {
    urls.push(env.ELASTOS_BROWSER_VM_ICE_SERVER);
  }
  if (env.ELASTOS_BROWSER_VM_ICE_SERVERS_JSON) {
    let parsed = null;
    try {
      parsed = JSON.parse(env.ELASTOS_BROWSER_VM_ICE_SERVERS_JSON);
    } catch {
      return urls;
    }
    if (Array.isArray(parsed)) {
      for (const entry of parsed) {
        if (typeof entry === "string") {
          urls.push(entry);
          continue;
        }
        if (entry && typeof entry === "object") {
          const values = Array.isArray(entry.urls) ? entry.urls : [entry.urls];
          for (const url of values) {
            if (typeof url === "string") urls.push(url);
          }
        }
      }
    }
  }
  return urls;
}

function deriveGuestIpv4(hostIpv4) {
  const octets = parseIpv4(hostIpv4);
  if (!octets) return "";
  octets[3] = octets[3] === 2 ? 3 : 2;
  return octets.join(".");
}

function copyVmMediaRelayEnv(env, platform, sourceEnv = process.env) {
  for (const key of VM_MEDIA_RELAY_ENV_KEYS) {
    const value = sourceEnv[key];
    if (value == null || value === "") continue;
    if (/[\r\n\0]/.test(value)) {
      throw new Error(`${key} must not contain control characters`);
    }
    env[key] = value;
  }
  if (env.ELASTOS_BROWSER_VM_MEDIA_RELAY_HOST_IPV4 && !isIpv4(env.ELASTOS_BROWSER_VM_MEDIA_RELAY_HOST_IPV4)) {
    throw new Error("ELASTOS_BROWSER_VM_MEDIA_RELAY_HOST_IPV4 must be an IPv4 address");
  }
  if (env.ELASTOS_BROWSER_VM_MEDIA_RELAY_GUEST_IPV4 && !isIpv4(env.ELASTOS_BROWSER_VM_MEDIA_RELAY_GUEST_IPV4)) {
    throw new Error("ELASTOS_BROWSER_VM_MEDIA_RELAY_GUEST_IPV4 must be an IPv4 address");
  }
  if (
    env.ELASTOS_BROWSER_VM_MEDIA_RELAY_PREFIX &&
    !/^(?:[1-9]|[12][0-9]|3[0-2])$/.test(env.ELASTOS_BROWSER_VM_MEDIA_RELAY_PREFIX)
  ) {
    throw new Error("ELASTOS_BROWSER_VM_MEDIA_RELAY_PREFIX must be 1..32");
  }
  if (platform !== "darwin-arm64") return;
  if (!env.ELASTOS_BROWSER_VM_MEDIA_RELAY_HOST_IPV4) {
    for (const url of collectIceUrlsFromEnv(env)) {
      const host = turnIpv4HostFromUrl(url);
      if (host) {
        env.ELASTOS_BROWSER_VM_MEDIA_RELAY_HOST_IPV4 = host;
        break;
      }
    }
  }
  if (env.ELASTOS_BROWSER_VM_MEDIA_RELAY_HOST_IPV4 && !env.ELASTOS_BROWSER_VM_MEDIA_RELAY_GUEST_IPV4) {
    env.ELASTOS_BROWSER_VM_MEDIA_RELAY_GUEST_IPV4 = deriveGuestIpv4(env.ELASTOS_BROWSER_VM_MEDIA_RELAY_HOST_IPV4);
  }
  if (
    env.ELASTOS_BROWSER_VM_MEDIA_RELAY_HOST_IPV4 &&
    env.ELASTOS_BROWSER_VM_MEDIA_RELAY_GUEST_IPV4 &&
    !env.ELASTOS_BROWSER_VM_MEDIA_RELAY_PREFIX
  ) {
    env.ELASTOS_BROWSER_VM_MEDIA_RELAY_PREFIX = "24";
  }
}

function writeJson(file, value) {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, `${JSON.stringify(value, null, 2)}\n`, "utf8");
}

function runtimeGatewayPrivateTargets(env = process.env) {
  const raw = env.ELASTOS_BROWSER_RUNTIME_GATEWAY_PORTS || env.ELASTOS_BROWSER_RUNTIME_GATEWAY_PORT || "8090,61180";
  const ports = [...new Set(String(raw)
    .split(",")
    .map((value) => Number(value.trim()))
    .filter((value) => Number.isInteger(value) && value > 0 && value <= 65535))];
  if (ports.length === 0) {
    throw new Error("ELASTOS_BROWSER_RUNTIME_GATEWAY_PORTS must include at least one TCP port");
  }
  return [
    {
      host: "localhost",
      schemes: ["tcp"],
      ports,
    },
  ];
}

function commonExitProvider(adapterSocket, relaySocket = null, { allowPrivateTargets = false } = {}) {
  const backend = {
    id: "source-home-browser-exit",
    kind: "stream_relay",
    allowed_hosts: ["*"],
    allowed_schemes: ["tcp", "tls"],
    allowed_ports: [80, 443],
    allow_private_targets: allowPrivateTargets,
    allowed_private_targets: runtimeGatewayPrivateTargets(),
    adapter_ipc: {
      kind: "unix_socket",
      path: adapterSocket,
    },
  };
  if (relaySocket) {
    validateAbsolute("relay socket", relaySocket);
    backend.relay_ipc = {
      kind: "unix_socket",
      path: relaySocket,
    };
  }
  return {
    timeout_secs: 10,
    backends: [backend],
  };
}

function existingRemoteCarrierExits(outDir) {
  const file = path.join(outDir, "exit-provider.json");
  try {
    const parsed = JSON.parse(fs.readFileSync(file, "utf8"));
    return Array.isArray(parsed.remote_carrier_exits)
      ? parsed.remote_carrier_exits
      : [];
  } catch (error) {
    if (error?.code === "ENOENT") {
      return [];
    }
    throw new Error(`failed to read existing remote Carrier exits from ${file}: ${error.message}`);
  }
}

function browserLocalExit(relaySocket, { allowPrivateTargets = false } = {}) {
  validateAbsolute("relay socket", relaySocket);
  return {
    schema: "elastos.browser.local-exit.config/v1",
    relay_ipc_path: relaySocket,
    allowed_hosts: ["*"],
    allowed_schemes: ["tcp", "tls"],
    allowed_ports: [80, 443],
    allow_private_targets: allowPrivateTargets,
    allowed_private_targets: runtimeGatewayPrivateTargets(),
    replace_existing_socket: true,
    connect_timeout_ms: 20000,
    buffer_bytes: 65536,
  };
}

function vmBrowserEngineAdapter(args, sourceEnv = process.env) {
  const supervisor = args.vmSupervisor || path.join(args.dataDir, "bin/browser-vm-engine-supervisor");
  validateAbsolute("--vm-supervisor", supervisor);
  const controlSocket = args.vmControlSocket || `/tmp/elastos-browser-vm-control-${args.platform}.sock`;
  validateAbsolute("--vm-control-socket", controlSocket);
  const controlLauncher = args.vmControlLauncher ||
    (args.platform === "darwin-arm64"
      ? path.join(args.dataDir, "bin/browser-vz-engine-supervisor")
      : args.platform.startsWith("linux-")
        ? path.join(args.dataDir, "bin/browser-vm-local-crosvm-launcher")
        : "");
  const remoteVzControlLauncher = isRemoteVzControlLauncher(controlLauncher);
  const supervisorConfig = {
    program: supervisor,
    timeout_ms: VM_SUPERVISOR_TIMEOUT_MS,
    control_socket_path: controlSocket,
  };
  const vmRoot = remoteVzControlLauncher
    ? "/tmp/evzs"
    : args.platform.startsWith("linux-")
      ? path.join(args.dataDir, "bvm")
      : "/tmp/evzs";
  const env = {
    ELASTOS_BROWSER_VM_ROOT: vmRoot,
    ELASTOS_BROWSER_VM_DATA_DIR: args.dataDir,
    ELASTOS_BROWSER_VM_PLATFORM: args.platform,
    ELASTOS_BROWSER_VM_CONTROL_SOCKET: controlSocket,
    ELASTOS_BROWSER_VM_ENGINE_TIMEOUT_MS: VM_ENGINE_TIMEOUT_MS,
    ELASTOS_BROWSER_VM_CONTROL_LAUNCH_TIMEOUT_MS: VM_CONTROL_TIMEOUT_MS,
    ELASTOS_BROWSER_VM_CONTROL_PROXY_REQUEST_TIMEOUT_MS: VM_CONTROL_PROXY_REQUEST_TIMEOUT_MS,
    ELASTOS_BROWSER_VM_EGRESS_MAX_SESSIONS: VM_EGRESS_MAX_SESSIONS,
    ELASTOS_BROWSER_VM_CONTROL_MAX_ACTIVE_PAGES: VM_CONTROL_MAX_ACTIVE_PAGES,
    ELASTOS_BROWSER_VM_IDLE_KEEPALIVE_MS: args.platform.startsWith("linux-")
      ? VM_LINUX_IDLE_KEEPALIVE_MS
      : VM_IDLE_KEEPALIVE_MS,
    ELASTOS_BROWSER_VM_REUSE_IDLE_VMS: args.platform.startsWith("linux-")
      ? VM_LINUX_REUSE_IDLE_VMS
      : VM_REUSE_IDLE_VMS,
    ELASTOS_BROWSER_VM_HIBERNATION: args.platform === "darwin-arm64" ? "1" : "0",
    ELASTOS_BROWSER_VM_HIBERNATION_DIR: path.join(args.dataDir, "browser-vm/hibernation"),
    ELASTOS_BROWSER_VM_HIBERNATION_MAX_ENTRIES: VM_HIBERNATION_MAX_ENTRIES,
    ELASTOS_BROWSER_VM_HIBERNATION_MAX_AGE_SECS: VM_HIBERNATION_MAX_AGE_SECS,
    ELASTOS_BROWSER_VM_PREWARM_CONTROL_SERVICE: "1",
  };
  if (!remoteVzControlLauncher) {
    copyVmIceEnv(env, sourceEnv);
    copyVmMediaRelayEnv(env, args.platform, sourceEnv);
  }
  copyVmDiagnosticEnv(env, sourceEnv);
  if (!remoteVzControlLauncher) {
    copyVmLauncherEnv(env, sourceEnv);
  }
  if (remoteVzControlLauncher) {
    env.ELASTOS_BROWSER_REMOTE_VZ_LAUNCH_TIMEOUT_MS = VM_REMOTE_VZ_LAUNCH_TIMEOUT_MS;
    copyRemoteVzPathEnv(env, sourceEnv);
  } else {
    env.ELASTOS_BROWSER_VM_CONTROL_READY_TIMEOUT_MS = VM_GUEST_READY_TIMEOUT_MS;
  }
  if (controlLauncher) {
    validateAbsolute("--vm-control-launcher", controlLauncher);
    env.ELASTOS_BROWSER_VM_CONTROL_LAUNCHER = controlLauncher;
  }
  if (args.vmRootfs) {
    validateAbsolute("--vm-rootfs", args.vmRootfs);
    env.ELASTOS_BROWSER_VM_ROOTFS = args.vmRootfs;
  }
  if (args.platform.startsWith("linux-") && !remoteVzControlLauncher) {
    env.ELASTOS_BROWSER_VM_ROOTFS_POOL_DIR = path.join(args.dataDir, "browser-vm/rootfs-pool");
    env.ELASTOS_BROWSER_VM_ROOTFS_COPY_MODE = "pool-required";
    env.ELASTOS_BROWSER_VM_ROOTFS_POOL_REFILL_COUNT = "2";
    env.ELASTOS_BROWSER_VM_ROOTFS_POOL_REFILL_SCRIPT = path.join(args.dataDir, "bin/browser-vm-prepare-rootfs-pool");
  }
  supervisorConfig.env = env;
  return {
    max_active_sessions: Number(VM_ADAPTER_MAX_ACTIVE_SESSIONS),
    adapters: [
      {
        id: "browser-vm-product",
        kind: "chromium_microvm",
        network_mode: "runtime_net_only",
        display_modes: ["webrtc_remote_display"],
        supervisor: supervisorConfig,
      },
    ],
  };
}

function main() {
  try {
    const args = parseArgs(process.argv.slice(2));
    if (!args.dataDir) throw new Error("--data-dir is required");
    if (!args.platform) throw new Error("--platform is required");
    if (!SUPPORTED_PLATFORMS.has(args.platform)) {
      throw new Error("--platform must be linux-amd64, linux-arm64, or darwin-arm64");
    }
    validateAbsolute("--data-dir", args.dataDir);
    const outDir = args.outDir || path.join(args.dataDir, "config");
    validateAbsolute("--out-dir", outDir);
    const adapterSocket = `/tmp/elastos-browser-source-home-${args.platform}.sock`;
    const relaySocket = `/tmp/elastos-browser-source-home-${args.platform}-relay.sock`;
    const sourceEnv = runtimeTurnEnv(args);
    const browserEngineAdapter = vmBrowserEngineAdapter(args, sourceEnv);
    const exitProvider = commonExitProvider(adapterSocket, relaySocket, {
      allowPrivateTargets: args.allowPrivateTargets === true,
    });
    const remoteCarrierExits = existingRemoteCarrierExits(outDir);
    if (remoteCarrierExits.length > 0) {
      exitProvider.remote_carrier_exits = remoteCarrierExits;
    }
    const localExit = browserLocalExit(relaySocket, {
      allowPrivateTargets: args.allowPrivateTargets === true,
    });

    writeJson(path.join(outDir, "browser-engine-adapter.json"), browserEngineAdapter);
    writeJson(path.join(outDir, "exit-provider.json"), exitProvider);
    writeJson(path.join(outDir, "browser-local-exit.json"), localExit);
    const files = ["browser-engine-adapter.json", "exit-provider.json", "browser-local-exit.json"];
    console.log(JSON.stringify({
      ok: true,
      schema: "elastos.browser.source-home-config/v1",
      platform: args.platform,
      out_dir: outDir,
      adapter_id: browserEngineAdapter.adapters[0].id,
      engine: browserEngineAdapter.adapters[0].kind,
      engine_mode: "vm",
      display_modes: browserEngineAdapter.adapters[0].display_modes,
      network_mode: "runtime_net_only",
      direct_network: false,
      relay_ipc: true,
      wallet_injection: false,
      remote_carrier_exit_count: remoteCarrierExits.length,
      files,
    }));
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error));
    console.error(usage());
    process.exit(1);
  }
}

main();
