#!/usr/bin/env node
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import process from "node:process";

function usage() {
  console.error(`Usage:
  node scripts/browser-native-host-capability.mjs \\
    --browser-program /absolute/path/to/chromium-or-cef \\
    [--require-native-media] [--require-network-isolation] [--require-product-native]

Checks whether this target host can plausibly run the native Browser product
path: native browser binary, real host compositor/display, real audio service,
and Linux network namespace isolation. It does not install anything, launch a
browser UI, or use Docker.
`);
}

function parseArgs(argv) {
  const args = {
    browserProgram: "",
    requireNativeMedia: false,
    requireNetworkIsolation: false,
  };
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    const next = () => {
      index += 1;
      if (index >= argv.length || argv[index].startsWith("--")) {
        throw new Error(`${arg} requires a value`);
      }
      return argv[index];
    };
    if (arg === "--help" || arg === "-h") {
      usage();
      process.exit(0);
    } else if (arg === "--browser-program") {
      args.browserProgram = next();
    } else if (arg === "--require-native-media") {
      args.requireNativeMedia = true;
    } else if (arg === "--require-network-isolation") {
      args.requireNetworkIsolation = true;
    } else if (arg === "--require-product-native") {
      args.requireNativeMedia = true;
      args.requireNetworkIsolation = true;
    } else {
      throw new Error(`unknown option: ${arg}`);
    }
  }
  if (!args.browserProgram) {
    throw new Error("--browser-program is required");
  }
  if (!path.isAbsolute(args.browserProgram)) {
    throw new Error("--browser-program must be absolute");
  }
  return args;
}

function commandExists(command) {
  const result = spawnSync("sh", ["-lc", `command -v ${shellQuote(command)}`], {
    encoding: "utf8",
    timeout: 1000,
  });
  return result.status === 0 ? result.stdout.trim() : "";
}

function shellQuote(value) {
  return `'${String(value).replaceAll("'", "'\\''")}'`;
}

function run(command, args, timeout = 1500) {
  const result = spawnSync(command, args, {
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
    timeout,
  });
  return {
    status: result.status,
    signal: result.signal,
    stdout: result.stdout || "",
    stderr: result.stderr || "",
    ok: result.status === 0,
  };
}

function fileExists(file) {
  try {
    return fs.existsSync(file);
  } catch {
    return false;
  }
}

function statExecutable(file) {
  try {
    const stat = fs.statSync(file);
    return stat.isFile() && (stat.mode & 0o111) !== 0;
  } catch {
    return false;
  }
}

function x11Socket(display) {
  if (!display) return "";
  const match = /^:([0-9]+)/.exec(display) || /^[^:]+:([0-9]+)/.exec(display);
  if (!match) return "";
  return `/tmp/.X11-unix/X${match[1]}`;
}

function checkBrowser(program) {
  const executable = statExecutable(program);
  const version = executable ? run(program, ["--version"], 2500) : null;
  const identity = (version?.stdout || version?.stderr || "").split(/\r?\n/)[0]?.trim() || "";
  const looksCompatible = /\b(chromium|chrome|google-chrome|cefclient|cefsimple|cef|brave|msedge)\b/i.test(
    `${path.basename(program)} ${identity}`,
  );
  return {
    executable,
    identity,
    looksCompatible,
  };
}

function checkDisplay() {
  const display = process.env.DISPLAY || "";
  const waylandDisplay = process.env.WAYLAND_DISPLAY || "";
  const runtimeDir = process.env.XDG_RUNTIME_DIR || "";
  const waylandSocket = runtimeDir && waylandDisplay ? path.join(runtimeDir, waylandDisplay) : "";
  const x11 = x11Socket(display);
  return {
    display,
    wayland_display: waylandDisplay,
    xdg_runtime_dir: runtimeDir,
    x11_socket: x11,
    x11_socket_exists: Boolean(x11 && fileExists(x11)),
    wayland_socket: waylandSocket,
    wayland_socket_exists: Boolean(waylandSocket && fileExists(waylandSocket)),
    ok: Boolean((x11 && fileExists(x11)) || (waylandSocket && fileExists(waylandSocket))),
  };
}

function checkAudio() {
  const runtimeDir = process.env.XDG_RUNTIME_DIR || "";
  const pulseSocket = runtimeDir ? path.join(runtimeDir, "pulse", "native") : "";
  const pipewireSocket = runtimeDir ? path.join(runtimeDir, "pipewire-0") : "";
  const pactl = commandExists("pactl");
  const pwCli = commandExists("pw-cli");
  const pactlInfo = pactl ? run(pactl, ["info"], 1500) : null;
  const pwInfo = pwCli ? run(pwCli, ["info", "0"], 1500) : null;
  return {
    pulse_socket: pulseSocket,
    pulse_socket_exists: Boolean(pulseSocket && fileExists(pulseSocket)),
    pipewire_socket: pipewireSocket,
    pipewire_socket_exists: Boolean(pipewireSocket && fileExists(pipewireSocket)),
    pactl: pactl || "",
    pactl_ok: pactlInfo?.ok === true,
    pw_cli: pwCli || "",
    pw_cli_ok: pwInfo?.ok === true,
    ok: Boolean(
      (pulseSocket && fileExists(pulseSocket)) ||
        (pipewireSocket && fileExists(pipewireSocket)) ||
        pactlInfo?.ok === true ||
        pwInfo?.ok === true,
    ),
  };
}

function checkNetworkIsolation() {
  const unshare = commandExists("unshare");
  const result = unshare ? run(unshare, ["--net", "--", "true"], 1500) : null;
  return {
    unshare: unshare || "",
    ok: result?.ok === true,
    status: result?.status ?? null,
    error: result && !result.ok ? (result.stderr || result.stdout).trim().slice(0, 240) : "",
  };
}

function checkGpu() {
  return {
    dev_dri_exists: fileExists("/dev/dri"),
    render_nodes: fileExists("/dev/dri") ? fs.readdirSync("/dev/dri").filter((name) => name.startsWith("renderD")) : [],
  };
}

function main() {
  const args = parseArgs(process.argv.slice(2));
  const browser = checkBrowser(args.browserProgram);
  const display = checkDisplay();
  const audio = checkAudio();
  const networkIsolation = checkNetworkIsolation();
  const gpu = checkGpu();

  const checks = [
    {
      name: "platform_linux",
      ok: os.platform() === "linux",
      detail: os.platform(),
    },
    {
      name: "browser_program_executable",
      ok: browser.executable,
      detail: args.browserProgram,
    },
    {
      name: "browser_program_chromium_or_cef",
      ok: browser.looksCompatible,
      detail: browser.identity || path.basename(args.browserProgram),
    },
    {
      name: "host_compositor_display",
      ok: display.ok,
      detail: display.wayland_socket_exists ? display.wayland_socket : display.x11_socket,
    },
    {
      name: "host_audio_service",
      ok: audio.ok,
      detail: audio.pipewire_socket_exists
        ? audio.pipewire_socket
        : audio.pulse_socket_exists
          ? audio.pulse_socket
          : audio.pactl_ok
            ? "pactl info"
            : audio.pw_cli_ok
              ? "pw-cli info 0"
              : "missing",
    },
    {
      name: "linux_network_namespace",
      ok: networkIsolation.ok,
      detail: networkIsolation.ok ? networkIsolation.unshare : networkIsolation.error || "missing unshare",
    },
  ];

  const nativeMediaReady =
    checks.find((check) => check.name === "platform_linux")?.ok === true &&
    checks.find((check) => check.name === "browser_program_executable")?.ok === true &&
    checks.find((check) => check.name === "browser_program_chromium_or_cef")?.ok === true &&
    checks.find((check) => check.name === "host_compositor_display")?.ok === true &&
    checks.find((check) => check.name === "host_audio_service")?.ok === true;
  const networkIsolationReady = checks.find((check) => check.name === "linux_network_namespace")?.ok === true;
  const productNativeReady = nativeMediaReady && networkIsolationReady;
  const missing = [];
  if (args.requireNativeMedia && !nativeMediaReady) {
    missing.push("native media requires Linux, a Chromium/CEF browser, host display/compositor, and host audio service");
  }
  if (args.requireNetworkIsolation && !networkIsolationReady) {
    missing.push("network isolation requires a working Linux network namespace probe");
  }
  const ok = missing.length === 0;

  const report = {
    schema: "elastos.browser.native-host-capability/v1",
    ok,
    browser_program: args.browserProgram,
    ready: {
      native_media: nativeMediaReady,
      network_isolation: networkIsolationReady,
      product_native: productNativeReady,
    },
    checks,
    details: {
      browser,
      display,
      audio,
      network_isolation: networkIsolation,
      gpu,
    },
    missing,
  };
  console.log(JSON.stringify(report, null, 2));
  if (!ok) {
    process.exit(1);
  }
}

try {
  main();
} catch (error) {
  console.error(error instanceof Error ? error.message : String(error));
  usage();
  process.exit(2);
}
