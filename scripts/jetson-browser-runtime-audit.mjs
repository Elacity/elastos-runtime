#!/usr/bin/env node
import { spawnSync } from "node:child_process";
import fs from "node:fs";

const schema = "elastos.jetson-browser-runtime-audit/v1";

function envDefault(name, fallback) {
  return process.env[name] || fallback;
}

function envFlag(name, fallback = false) {
  const value = process.env[name];
  if (value == null || value === "") return fallback;
  if (["1", "true", "yes"].includes(value.toLowerCase())) return true;
  if (["0", "false", "no"].includes(value.toLowerCase())) return false;
  throw new Error(`${name} must be 1 or 0`);
}

function envInteger(name, fallback = 0) {
  const value = process.env[name];
  if (value == null || value === "") return fallback;
  const parsed = Number(value);
  if (!Number.isInteger(parsed) || parsed < 0) {
    throw new Error(`${name} must be a non-negative integer`);
  }
  return parsed;
}

const defaults = {
  host: envDefault("ELASTOS_050_JETSON_HOST", ""),
  port: envDefault("ELASTOS_050_JETSON_PORT", "22"),
  user: envDefault("ELASTOS_050_JETSON_USER", ""),
  key: envDefault("ELASTOS_050_JETSON_KEY", ""),
  proxyJump: envDefault("ELASTOS_050_JETSON_PROXY_JUMP", ""),
  dataDir: envDefault("ELASTOS_050_JETSON_DATA_DIR", ""),
  sourceDir: envDefault("ELASTOS_050_JETSON_SOURCE_DIR", ""),
  requireParity: envFlag("ELASTOS_050_REQUIRE_JETSON_PARITY", false),
  minActiveCrosvmSeconds: envInteger("ELASTOS_050_JETSON_MIN_ACTIVE_CROSVM_SECONDS", 0),
};

function usage() {
  process.stdout.write(`Usage:
  scripts/jetson-browser-runtime-audit.mjs [options]

Read-only Jetson Browser VM runtime audit for the 0.5.0 closeout.

Options:
  --host HOST             Jetson SSH host. Required unless ELASTOS_050_JETSON_HOST is set.
  --port PORT             Jetson SSH port. Default: ${defaults.port}
  --user USER             Jetson SSH user. Required unless ELASTOS_050_JETSON_USER is set.
  --key PATH              Optional Jetson SSH key. Default: ${defaults.key || "<ssh default>"}
  --proxy-jump HOST       Optional SSH ProxyJump host. Default: ${defaults.proxyJump || "<none>"}
  --no-proxy-jump         Connect directly without SSH ProxyJump.
  --data-dir PATH         Jetson ElastOS data dir. Required unless ELASTOS_050_JETSON_DATA_DIR is set.
  --source-dir PATH       Jetson source checkout. Required unless ELASTOS_050_JETSON_SOURCE_DIR is set.
  --require-parity        Fail when Jetson source/install/artifact helper hashes or source
                          git state do not match this branch. Default: ${defaults.requireParity ? "on" : "off"}
  --min-active-crosvm-seconds SECONDS
                          Fail when the active crosvm has been running for less than SECONDS.
                          Default: ${defaults.minActiveCrosvmSeconds}
  --help, -h              Show this help.

This audit does not mutate the Jetson. It checks the installed Browser adapter
contract, VM preflight state, rootfs pool, active crosvm launch shape, and recent
guest-control logs.
`);
}

function parseArgs(argv) {
  const args = { ...defaults };
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    const needValue = () => {
      const value = argv[index + 1];
      if (!value) throw new Error(`${arg} requires a value`);
      index += 1;
      return value;
    };
    switch (arg) {
      case "--help":
      case "-h":
        usage();
        process.exit(0);
        break;
      case "--host":
        args.host = needValue();
        break;
      case "--port":
        args.port = needValue();
        break;
      case "--user":
        args.user = needValue();
        break;
      case "--key":
        args.key = needValue();
        break;
      case "--proxy-jump":
        args.proxyJump = needValue();
        break;
      case "--no-proxy-jump":
        args.proxyJump = "";
        break;
      case "--data-dir":
        args.dataDir = needValue();
        break;
      case "--source-dir":
        args.sourceDir = needValue();
        break;
      case "--require-parity":
        args.requireParity = true;
        break;
      case "--min-active-crosvm-seconds":
        args.minActiveCrosvmSeconds = Number(needValue());
        if (!Number.isInteger(args.minActiveCrosvmSeconds) || args.minActiveCrosvmSeconds < 0) {
          throw new Error("--min-active-crosvm-seconds must be a non-negative integer");
        }
        break;
      default:
        throw new Error(`unknown option: ${arg}`);
    }
  }
  return args;
}

function validateConfig(args) {
  const missing = [];
  if (!args.host) missing.push("--host or ELASTOS_050_JETSON_HOST");
  if (!args.user) missing.push("--user or ELASTOS_050_JETSON_USER");
  if (!args.dataDir) missing.push("--data-dir or ELASTOS_050_JETSON_DATA_DIR");
  if (!args.sourceDir) missing.push("--source-dir or ELASTOS_050_JETSON_SOURCE_DIR");
  if (missing.length > 0) {
    throw new Error(`missing required target configuration: ${missing.join(", ")}`);
  }
}

function run(command, args, options = {}) {
  const result = spawnSync(command, args, {
    encoding: "utf8",
    timeout: options.timeoutMs ?? 20_000,
    maxBuffer: options.maxBuffer ?? 2 * 1024 * 1024,
  });
  return {
    ok: result.status === 0,
    status: result.status,
    stdout: result.stdout ?? "",
    stderr: result.stderr ?? "",
    error: result.error ? String(result.error.message ?? result.error) : null,
  };
}

function sshNode(config, remoteScript) {
  const args = [
    "-o",
    "BatchMode=yes",
    "-o",
    "ConnectTimeout=8",
    "-o",
    "StrictHostKeyChecking=accept-new",
  ];
  if (config.key && fs.existsSync(config.key)) args.push("-i", config.key);
  if (config.proxyJump) args.push("-J", config.proxyJump);
  args.push("-p", String(config.port), `${config.user}@${config.host}`, "node", "-");
  const result = spawnSync("ssh", args, {
    encoding: "utf8",
    input: remoteScript,
    timeout: 30_000,
    maxBuffer: 2 * 1024 * 1024,
  });
  return {
    ok: result.status === 0,
    status: result.status,
    stdout: result.stdout ?? "",
    stderr: result.stderr ?? "",
    error: result.error ? String(result.error.message ?? result.error) : null,
  };
}

function localSha256(paths) {
  const result = run("sha256sum", paths, { timeoutMs: 10_000 });
  if (!result.ok) return {};
  const hashes = {};
  for (const line of result.stdout.trim().split(/\r?\n/).filter(Boolean)) {
    const [sha, path] = line.trim().split(/\s+/, 2);
    hashes[path.replace(/^scripts\//, "")] = sha;
  }
  return hashes;
}

function remoteAuditScript(dataDir, sourceDir, minActiveCrosvmSeconds) {
  return `
const fs = require("node:fs");
const crypto = require("node:crypto");
const cp = require("node:child_process");
const os = require("node:os");
const dataDir = ${JSON.stringify(dataDir)};
const sourceDir = ${JSON.stringify(sourceDir)};
const minActiveCrosvmSeconds = ${JSON.stringify(minActiveCrosvmSeconds)};

function parentDir(path) {
  const parts = path.replace(/\\/+$/, "").split("/");
  parts.pop();
  return parts.join("/") || "/";
}

function inferHomeFromDataDir(path) {
  const suffix = "/xdg-data/elastos";
  if (path.endsWith(suffix)) {
    return path.slice(0, -suffix.length) || "/";
  }
  return process.env.HOME || "";
}

function inferXdgFromDataDir(path) {
  const suffix = "/elastos";
  if (path.endsWith(suffix)) {
    return parentDir(path);
  }
  return process.env.XDG_DATA_HOME || "";
}

function readJson(path) {
  return JSON.parse(fs.readFileSync(path, "utf8"));
}
function exists(path) {
  try { fs.accessSync(path); return true; } catch { return false; }
}
function sha(path) {
  try {
    return crypto.createHash("sha256").update(fs.readFileSync(path)).digest("hex");
  } catch {
    return null;
  }
}
function run(command, args, options = {}) {
  const result = cp.spawnSync(command, args, { encoding: "utf8", maxBuffer: 2 * 1024 * 1024, ...options });
  return { ok: result.status === 0, status: result.status, stdout: result.stdout || "", stderr: result.stderr || "" };
}
function getUnixHttpJson(socketPath, requestPath) {
  if (!socketPath || !exists(socketPath)) {
    return { ok: false, status: null, body: null, error: "control socket is unavailable" };
  }
  const result = run("curl", [
    "--silent",
    "--show-error",
    "--max-time",
    "5",
    "--unix-socket",
    socketPath,
    "http://localhost" + requestPath,
  ], { maxBuffer: 1024 * 1024 });
  if (!result.ok) {
    return {
      ok: false,
      status: result.status,
      body: null,
      error: (result.stderr || result.stdout || "curl request failed").trim(),
    };
  }
  try {
    return { ok: true, status: 200, body: JSON.parse(result.stdout || "{}"), error: null };
  } catch (error) {
    return {
      ok: false,
      status: 200,
      body: null,
      error: error instanceof Error ? error.message : String(error),
    };
  }
}
function crosvmProcessInfos() {
  return run("ps", ["-eo", "pid=,ppid=,pgid=,etimes=,command="]).stdout
    .split(/\\r?\\n/)
    .map((line) => {
      const match = line.match(/^\\s*(\\d+)\\s+(\\d+)\\s+(\\d+)\\s+(\\d+)\\s+(.*)$/);
      if (!match) return null;
      return {
        pid: Number(match[1]),
        ppid: Number(match[2]),
        pgid: Number(match[3]),
        uptime_seconds: Number(match[4]),
        command: match[5],
      };
    })
    .filter((info) => info && info.command.includes("/crosvm run"));
}
function tail(path, lines = 120) {
  const result = run("tail", ["-" + String(lines), path]);
  return result.ok ? result.stdout : "";
}
function extractFlagPath(commandLine, flag) {
  const parts = commandLine.trim().split(/\\s+/).filter(Boolean);
  for (let index = 0; index < parts.length - 1; index += 1) {
    if (parts[index] === flag) return parts[index + 1];
  }
  return null;
}
function inspectInitrdHelper(initrdPath) {
  const info = {
    path: initrdPath,
    exists: exists(initrdPath),
    helper_path: "bin/browser-selkies-control-service.mjs",
    sha256: null,
    error: null,
  };
  if (!info.exists) return info;
  const workDir = fs.mkdtempSync(os.tmpdir() + "/elastos-initrd-");
  try {
    const extract = run("sh", [
      "-lc",
      "command -v gzip >/dev/null 2>&1 && command -v cpio >/dev/null 2>&1 && gzip -dc \\"$1\\" | cpio -id --quiet bin/browser-selkies-control-service.mjs",
      "sh",
      initrdPath,
    ], { cwd: workDir });
    if (!extract.ok) {
      info.error = (extract.stderr || extract.stdout || "initrd helper extraction failed").trim();
      return info;
    }
    info.sha256 = sha(workDir + "/bin/browser-selkies-control-service.mjs");
    if (!info.sha256) info.error = "initrd helper was not found after extraction";
    return info;
  } finally {
    fs.rmSync(workDir, { recursive: true, force: true });
  }
}
function inspectRootfsHelper(rootfsPath) {
  const info = {
    path: rootfsPath,
    exists: exists(rootfsPath),
    helper_path: "/opt/elastos/bin/browser-selkies-control-service.mjs",
    sha256: null,
    error: null,
  };
  if (!info.exists) return info;
  const result = run("sh", [
    "-lc",
    "command -v debugfs >/dev/null 2>&1 && tmp=$(mktemp) && trap 'rm -f \\"$tmp\\"' EXIT && debugfs -R 'cat /opt/elastos/bin/browser-selkies-control-service.mjs' \\"$1\\" >\\"$tmp\\" 2>/dev/null && sha256sum \\"$tmp\\"",
    "sh",
    rootfsPath,
  ]);
  if (!result.ok) {
    info.error = (result.stderr || result.stdout || "rootfs helper inspection failed").trim();
    return info;
  }
  const match = result.stdout.trim().match(/^([a-f0-9]{64})\\b/);
  if (!match) {
    info.error = "rootfs helper hash was not produced";
    return info;
  }
  info.sha256 = match[1];
  return info;
}
function sourceGitState(path) {
  let gitFile = null;
  try {
    const gitPath = path + "/.git";
    if (fs.statSync(gitPath).isFile()) {
      gitFile = fs.readFileSync(gitPath, "utf8").trim();
    }
  } catch {}
  const head = run("git", ["-C", path, "rev-parse", "HEAD"]);
  const branch = run("git", ["-C", path, "branch", "--show-current"]);
  const status = run("git", ["-C", path, "status", "--short"]);
  const toplevel = run("git", ["-C", path, "rev-parse", "--show-toplevel"]);
  return {
    path,
    ok: head.ok && branch.ok && status.ok && toplevel.ok,
    head: head.ok ? head.stdout.trim() : null,
    branch: branch.ok ? branch.stdout.trim() : null,
    dirty: status.ok ? status.stdout.trim() !== "" : null,
    status: status.ok ? status.stdout.trim().split(/\\r?\\n/).filter(Boolean) : [],
    toplevel: toplevel.ok ? toplevel.stdout.trim() : null,
    git_file: gitFile,
    error: head.ok && branch.ok && status.ok && toplevel.ok
      ? null
      : [head.stderr, branch.stderr, status.stderr, toplevel.stderr].find((value) => value && value.trim())?.trim() || "git source state unavailable",
  };
}
function targetRefreshVerify() {
  const targetHome = inferHomeFromDataDir(dataDir);
  const targetXdgDataHome = inferXdgFromDataDir(dataDir);
  const result = run("bash", [
    sourceDir + "/scripts/browser-vm-target-refresh.sh",
    "--source-dir",
    sourceDir,
    "--data-dir",
    dataDir,
    "--verify-only",
  ], {
    env: {
      ...process.env,
      ...(targetHome ? { HOME: targetHome } : {}),
      ...(targetXdgDataHome ? { XDG_DATA_HOME: targetXdgDataHome } : {}),
    },
  });
  return {
    ok: result.ok,
    status: result.status,
    stdout_tail: result.stdout.trim().split(/\\r?\\n/).slice(-40),
    stderr_tail: result.stderr.trim().split(/\\r?\\n/).filter(Boolean).slice(-20),
  };
}

const adapterPath = dataDir + "/config/browser-engine-adapter.json";
const adapterConfig = readJson(adapterPath);
const adapter = adapterConfig.adapters?.[0] || {};
const env = adapter.supervisor?.env || {};
const preflight = JSON.parse(run("bash", [
  sourceDir + "/scripts/browser-vm-engine-preflight.sh",
], {
  env: {
    ...process.env,
    ELASTOS_BROWSER_VM_DATA_DIR: dataDir,
    ELASTOS_BROWSER_VM_PLATFORM: "linux-arm64",
    ELASTOS_BROWSER_VM_CONTROL_SOCKET: env.ELASTOS_BROWSER_VM_CONTROL_SOCKET || "",
  },
}).stdout || "{}");
const poolDir = dataDir + "/browser-vm/rootfs-pool";
let poolFiles = [];
try {
  poolFiles = fs.readdirSync(poolDir)
    .filter((name) => name.endsWith(".ext4"))
    .map((name) => {
      const path = poolDir + "/" + name;
      const st = fs.statSync(path);
      return { name, size: st.size };
    });
} catch {}
const crosvmInfos = crosvmProcessInfos();
const crosvmLines = crosvmInfos.map((info) => info.command);
const activeCrosvmInfo = crosvmInfos[0] || null;
const activeCrosvm = activeCrosvmInfo?.command || "";
const bvmMatch = activeCrosvm.match(/\\/bvm\\/(bvm-[^/\\s]+)/);
const sessionId = bvmMatch?.[1] || null;
const sessionDir = sessionId ? dataDir + "/bvm/" + sessionId : null;
const serialTail = sessionDir ? tail(sessionDir + "/serial.log", 140) : "";
const crosvmTail = sessionDir ? tail(sessionDir + "/crosvm.log", 80) : "";
const activeInitrdPath = extractFlagPath(activeCrosvm, "--initrd") || dataDir + "/browser-vm/initrd";
const baseRootfsPath = dataDir + "/browser-vm/rootfs.ext4";
const activeRootfsPath =
  extractFlagPath(activeCrosvm, "--rwdisk") ||
  extractFlagPath(activeCrosvm, "--rw-root") ||
  null;
const helperNames = [
  "browser-vm-local-crosvm-launcher.mjs",
  "browser-vm-control-service.mjs",
  "browser-source-home-config.mjs",
  "browser-vm-prepare-rootfs-pool.mjs",
  "browser-selkies-control-service.mjs",
];
const installedHelperPaths = {
  "browser-vm-local-crosvm-launcher.mjs": dataDir + "/bin/browser-vm-local-crosvm-launcher.mjs",
  "browser-vm-control-service.mjs": dataDir + "/bin/browser-vm-control-service.mjs",
  "browser-source-home-config.mjs": dataDir + "/scripts/browser-source-home-config.mjs",
  "browser-vm-prepare-rootfs-pool.mjs": dataDir + "/scripts/browser-vm-prepare-rootfs-pool.mjs",
  "browser-selkies-control-service.mjs": dataDir + "/scripts/browser-selkies-control-service.mjs",
};
const helperHashes = {};
for (const name of helperNames) {
  const installed = installedHelperPaths[name];
  const source = sourceDir + "/scripts/" + name;
  helperHashes[name] = {
    installed: sha(installed),
    bin_mirror: sha(dataDir + "/bin/" + name),
    source: sha(source),
    installed_path: exists(installed) ? installed : null,
    bin_mirror_path: exists(dataDir + "/bin/" + name) ? dataDir + "/bin/" + name : null,
    source_path: exists(source) ? source : null,
  };
}
const guestHelperHashes = {
  active_initrd: inspectInitrdHelper(activeInitrdPath),
  base_rootfs: inspectRootfsHelper(baseRootfsPath),
};
if (activeRootfsPath && activeRootfsPath !== baseRootfsPath) {
  guestHelperHashes.active_rootfs = inspectRootfsHelper(activeRootfsPath);
}
const controlSocket = env.ELASTOS_BROWSER_VM_CONTROL_SOCKET || "";
const controlStatus = getUnixHttpJson(controlSocket, "/status");
const activePageId = Array.isArray(controlStatus.body?.page_ids) ? controlStatus.body.page_ids[0] || "" : "";
const activePageStatus = activePageId
  ? getUnixHttpJson(controlSocket, "/pages/" + encodeURIComponent(activePageId) + "/status")
  : { ok: false, status: null, body: null, error: "no active page id" };
const activePageUrl = String(activePageStatus.body?.actual_url || activePageStatus.body?.url || "");
const activePageTitle = String(activePageStatus.body?.title || "");
const activePageDisplay = activePageStatus.body?.display_session || {};
const activePageOpened = activePageStatus.ok === true &&
  activePageStatus.body?.video === true &&
  activePageDisplay.backend_class === "product_compositor" &&
  activePageDisplay.media_transport === "runtime_relay" &&
  (activePageUrl.includes("ela.city") || activePageTitle.includes("ela.city"));
const checks = {
  adapter_webrtc_only: JSON.stringify(adapter.display_modes || []) === JSON.stringify(["webrtc_remote_display"]) && !("preferred_display_mode" in adapter),
  adapter_runtime_net_only: adapter.network_mode === "runtime_net_only" && adapter.direct_network !== true && adapter.wallet_injection !== true,
  control_socket_configured: Boolean(env.ELASTOS_BROWSER_VM_CONTROL_SOCKET),
  control_socket_exists: exists(controlSocket),
  rootfs_pool_required: env.ELASTOS_BROWSER_VM_ROOTFS_COPY_MODE === "pool-required",
  local_substrate_ready: preflight.local_substrate_ready === true,
  launch_ready: preflight.launch_ready === true,
  crosvm_running: crosvmLines.length > 0,
  crosvm_min_uptime: minActiveCrosvmSeconds === 0 ||
    Number(activeCrosvmInfo?.uptime_seconds || 0) >= minActiveCrosvmSeconds,
  crosvm_four_vcpus: activeCrosvm.includes("--cpus 4"),
  crosvm_webrtc_display: activeCrosvm.includes("elastos.browser_display_mode=webrtc_remote_display"),
  guest_opened_page: serialTail.includes("page_open_done") || activePageOpened,
  guest_webrtc_only_open: serialTail.includes('"display_mode":"webrtc_remote_display"') || activeCrosvm.includes("elastos.browser_display_mode=webrtc_remote_display"),
};
console.log(JSON.stringify({
  hostname: run("hostname", []).stdout.trim(),
  uname: run("uname", ["-a"]).stdout.trim(),
  data_dir: dataDir,
  source_git: sourceGitState(sourceDir),
  target_refresh_verify: targetRefreshVerify(),
  adapter: {
    max_active_sessions: adapterConfig.max_active_sessions,
    id: adapter.id,
    kind: adapter.kind,
    display_modes: adapter.display_modes,
    network_mode: adapter.network_mode,
    control_socket: env.ELASTOS_BROWSER_VM_CONTROL_SOCKET || null,
    launcher: env.ELASTOS_BROWSER_VM_CONTROL_LAUNCHER || null,
    rootfs_pool_dir: env.ELASTOS_BROWSER_VM_ROOTFS_POOL_DIR || null,
    rootfs_copy_mode: env.ELASTOS_BROWSER_VM_ROOTFS_COPY_MODE || null,
  },
  preflight,
  control_status: controlStatus,
  active_page_status: activePageStatus,
  rootfs_pool: poolFiles,
  active_crosvm: activeCrosvm,
  active_crosvm_process: activeCrosvmInfo ? {
    pid: activeCrosvmInfo.pid,
    ppid: activeCrosvmInfo.ppid,
    pgid: activeCrosvmInfo.pgid,
    uptime_seconds: activeCrosvmInfo.uptime_seconds,
    min_required_uptime_seconds: minActiveCrosvmSeconds,
  } : null,
  active_session_dir: sessionDir,
  helper_hashes: helperHashes,
  guest_helper_hashes: guestHelperHashes,
  checks,
  log_summary: {
    serial_has_page_open_done: serialTail.includes("page_open_done"),
    serial_has_ela_city: serialTail.includes("https://ela.city/"),
    control_status_active_pages: Number(controlStatus.body?.active_pages || 0),
    active_page_id: activePageId || null,
    active_page_url: activePageUrl || null,
    active_page_title: activePageTitle || null,
    active_page_video: activePageStatus.body?.video === true,
    active_page_audio: activePageStatus.body?.audio === true,
    crosvm_log_has_multiprocess: crosvmTail.includes("multiprocess"),
  },
}, null, 2));
`;
}

function main() {
  let config;
  try {
    config = parseArgs(process.argv.slice(2));
    validateConfig(config);
  } catch (error) {
    process.stderr.write(`${error.message}\n`);
    usage();
    process.exit(2);
  }

  const localHashes = localSha256([
    "scripts/browser-vm-local-crosvm-launcher.mjs",
    "scripts/browser-vm-control-service.mjs",
    "scripts/browser-source-home-config.mjs",
    "scripts/browser-vm-prepare-rootfs-pool.mjs",
    "scripts/browser-selkies-control-service.mjs",
  ]);
  const result = sshNode(
    config,
    remoteAuditScript(config.dataDir, config.sourceDir, config.minActiveCrosvmSeconds),
  );
  if (!result.ok) {
    process.stdout.write(JSON.stringify({
      schema,
      ok: false,
      error: result.error || result.stderr.trim() || result.stdout.trim() || "ssh/node audit failed",
    }, null, 2) + "\n");
    process.exit(1);
  }
  let remote;
  try {
    remote = JSON.parse(result.stdout);
  } catch (error) {
    process.stdout.write(JSON.stringify({
      schema,
      ok: false,
      error: `invalid remote audit JSON: ${error.message}`,
      stdout: result.stdout,
      stderr: result.stderr,
    }, null, 2) + "\n");
    process.exit(1);
  }

  const helperParity = {};
  for (const [name, hashes] of Object.entries(remote.helper_hashes || {})) {
    const expectedInstalled = hashes.installed !== null;
    helperParity[name] = {
      expected_installed: expectedInstalled,
      installed_matches_branch: hashes.installed === localHashes[name],
      bin_mirror_matches_branch: hashes.bin_mirror === localHashes[name],
      source_matches_branch: hashes.source === localHashes[name],
      installed_matches_source: hashes.installed === hashes.source,
      branch_sha256: localHashes[name] || null,
      installed_sha256: hashes.installed,
      bin_mirror_sha256: hashes.bin_mirror,
      source_sha256: hashes.source,
    };
  }
  const requiredChecks = [
    "adapter_webrtc_only",
    "adapter_runtime_net_only",
    "control_socket_configured",
    "control_socket_exists",
    "rootfs_pool_required",
    "local_substrate_ready",
    "launch_ready",
    "crosvm_running",
    "crosvm_four_vcpus",
    "crosvm_webrtc_display",
    "guest_opened_page",
    "guest_webrtc_only_open",
  ];
  if (config.minActiveCrosvmSeconds > 0) {
    requiredChecks.push("crosvm_min_uptime");
  }
  const failedChecks = requiredChecks.filter((name) => remote.checks?.[name] !== true);
  const sourceDrift = Object.entries(helperParity)
    .filter(([, value]) => !value.source_matches_branch)
    .map(([name]) => name);
  const installedRuntimeDrift = Object.entries(helperParity)
    .filter(([, value]) => value.expected_installed && !value.installed_matches_branch)
    .map(([name]) => name);
  const branchGuestHelperHash = localHashes["browser-selkies-control-service.mjs"] || null;
  const guestArtifactParity = {};
  for (const [name, hashes] of Object.entries(remote.guest_helper_hashes || {})) {
    if (!hashes) continue;
    guestArtifactParity[name] = {
      expected_present: hashes.exists === true,
      matches_branch: hashes.exists === true && hashes.sha256 === branchGuestHelperHash,
      branch_sha256: branchGuestHelperHash,
      artifact_sha256: hashes.sha256,
      path: hashes.path,
      helper_path: hashes.helper_path,
      error: hashes.error || null,
    };
  }
  const guestArtifactDrift = Object.entries(guestArtifactParity)
    .filter(([, value]) => value.expected_present && !value.matches_branch)
    .map(([name]) => name);
  const sourceGitOk = remote.source_git?.ok === true && remote.source_git?.dirty === false;
  const targetRefreshVerifyOk = remote.target_refresh_verify?.ok === true;
  const parityFailures = [];
  if (config.requireParity && !sourceGitOk) {
    parityFailures.push("jetson_source_git_not_clean_or_unavailable");
  }
  if (config.requireParity && sourceDrift.length > 0) {
    parityFailures.push("jetson_source_helper_drift");
  }
  if (config.requireParity && installedRuntimeDrift.length > 0) {
    parityFailures.push("jetson_installed_helper_drift");
  }
  if (config.requireParity && guestArtifactDrift.length > 0) {
    parityFailures.push("jetson_guest_artifact_helper_drift");
  }
  if (config.requireParity && !targetRefreshVerifyOk) {
    parityFailures.push("jetson_target_refresh_verify_drift");
  }
  const report = {
    schema,
    ok: failedChecks.length === 0 && parityFailures.length === 0,
    target: {
      host: config.host,
      port: Number(config.port),
      user: config.user,
      proxy_jump: config.proxyJump || null,
      source_dir: config.sourceDir,
      require_parity: config.requireParity,
      min_active_crosvm_seconds: config.minActiveCrosvmSeconds,
    },
    remote,
    helper_parity: helperParity,
    guest_artifact_parity: guestArtifactParity,
    failed_checks: failedChecks,
    parity_failures: parityFailures,
    source_git_ok: sourceGitOk,
    target_refresh_verify_ok: targetRefreshVerifyOk,
    source_drift: sourceDrift,
    installed_runtime_drift: installedRuntimeDrift,
    guest_artifact_drift: guestArtifactDrift,
    drift_is_blocking: config.requireParity && (
      !sourceGitOk ||
      sourceDrift.length > 0 ||
      installedRuntimeDrift.length > 0 ||
      guestArtifactDrift.length > 0 ||
      !targetRefreshVerifyOk
    ),
    next_actions:
      !sourceGitOk || sourceDrift.length > 0 || installedRuntimeDrift.length > 0 || guestArtifactDrift.length > 0 || !targetRefreshVerifyOk
        ? [
            "Jetson source/install/artifact parity or target refresh verify is incomplete; do not claim target source parity until the source checkout, installed helpers, VM artifacts, and active runtime are updated or explicitly reconciled.",
          ]
        : [],
  };
  process.stdout.write(`${JSON.stringify(report, null, 2)}\n`);
  process.exit(report.ok ? 0 : 1);
}

main();
