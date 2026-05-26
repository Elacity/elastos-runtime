#!/usr/bin/env node
import fs from "node:fs";
import path from "node:path";
import process from "node:process";

function usage() {
  return `Usage:
  node scripts/browser-hosted-product-operator-config.mjs \\
    --out-dir /path/to/config-dir \\
    --supervisor-program /absolute/path/to/browser-hosted-product-supervisor.mjs \\
    --control-socket /absolute/path/to/control.sock

Options:
  --adapter-id <id>                 Default: hosted-product
  --candidate <id>                  selkies|browserbox|kasm-workspaces|kasmvnc. Sets engine/display backend.
  --engine-kind <kind>              Default: selkies_gstreamer. Use hosted_remote_browser for KasmVNC/BrowserBox-style spikes.
  --display-backend <backend>       Default: selkies_gstreamer_webrtc
  --supervisor-arg <arg>            May be repeated. Passed as-is to supervisor.
  --env <KEY=VALUE>                 May be repeated. Added to supervisor env.
  --timeout-ms <ms>                 Default: 30000

The supervisor program must launch a real hosted Browser product display
adapter, such as a Selkies/GStreamer-style Chromium session or another proven
remote-browser/compositor backend, and return an
elastos.browser.engine.supervisor-result/v1 receipt with:
  engine = selected --engine-kind
  display_session.mode = webrtc_remote_display
  display_session.backend_class = product_compositor
  display_session.display_backend = selected --display-backend
  display_session.audio = true
  display_session.video = true
  direct_network = false

Use scripts/browser-hosted-product-display-smoke.sh against the generated
browser-engine-adapter.json before deploying it.

The bundled scripts/browser-hosted-product-supervisor.mjs is a strict bridge to
an operator-run compositor control service on --control-socket. It does not
fake media or fall back to the Playwright/CDP proof.
`;
}

function parseArgs(argv) {
  const args = {
    adapterId: "hosted-product",
    adapterIdExplicit: false,
    candidate: "",
    engineKind: "selkies_gstreamer",
    engineKindExplicit: false,
    displayBackend: "selkies_gstreamer_webrtc",
    displayBackendExplicit: false,
    supervisorArgs: [],
    env: {},
    timeoutMs: 30000,
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
    switch (arg) {
      case "--help":
      case "-h":
        console.log(usage());
        process.exit(0);
      case "--out-dir":
        args.outDir = next();
        break;
      case "--adapter-id":
        args.adapterId = next();
        args.adapterIdExplicit = true;
        break;
      case "--candidate":
        args.candidate = next();
        break;
      case "--engine-kind":
        args.engineKind = next();
        args.engineKindExplicit = true;
        break;
      case "--display-backend":
        args.displayBackend = next();
        args.displayBackendExplicit = true;
        break;
      case "--supervisor-program":
        args.supervisorProgram = next();
        break;
      case "--control-socket":
        args.controlSocket = next();
        break;
      case "--supervisor-arg":
        args.supervisorArgs.push(next());
        break;
      case "--env": {
        const entry = next();
        const separator = entry.indexOf("=");
        if (separator <= 0) {
          throw new Error("--env must use KEY=VALUE");
        }
        const key = entry.slice(0, separator);
        const value = entry.slice(separator + 1);
        if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(key)) {
          throw new Error(`invalid environment key: ${key}`);
        }
        args.env[key] = value;
        break;
      }
      case "--timeout-ms":
        args.timeoutMs = Number(next());
        break;
      default:
        throw new Error(`unknown option: ${arg}`);
    }
  }
  return args;
}

function candidatePreset(candidate) {
  if (!candidate) return null;
  if (candidate === "selkies" || candidate === "selkies-baseline") {
    return {
      adapterId: "selkies-product",
      engineKind: "selkies_gstreamer",
      displayBackend: "selkies_gstreamer_webrtc",
    };
  }
  if (candidate === "browserbox") {
    return {
      adapterId: "browserbox-product",
      engineKind: "hosted_remote_browser",
      displayBackend: "browserbox_webrtc",
    };
  }
  if (candidate === "kasm-workspaces") {
    return {
      adapterId: "kasm-workspaces-product",
      engineKind: "hosted_remote_browser",
      displayBackend: "kasm_workspaces_webrtc",
    };
  }
  if (candidate === "kasmvnc") {
    return {
      adapterId: "kasmvnc-product",
      engineKind: "hosted_remote_browser",
      displayBackend: "kasmvnc_webrtc",
    };
  }
  throw new Error(`unsupported candidate: ${candidate}`);
}

function applyCandidatePreset(args) {
  const preset = candidatePreset(args.candidate);
  if (!preset) return;
  if (args.engineKindExplicit && args.engineKind !== preset.engineKind) {
    throw new Error(`--engine-kind conflicts with --candidate ${args.candidate}; expected ${preset.engineKind}`);
  }
  if (args.displayBackendExplicit && args.displayBackend !== preset.displayBackend) {
    throw new Error(`--display-backend conflicts with --candidate ${args.candidate}; expected ${preset.displayBackend}`);
  }
  if (!args.adapterIdExplicit) {
    args.adapterId = preset.adapterId;
  }
  args.engineKind = preset.engineKind;
  args.displayBackend = preset.displayBackend;
}

function validate(args) {
  applyCandidatePreset(args);
  if (!args.outDir) {
    throw new Error("--out-dir is required");
  }
  if (!args.supervisorProgram) {
    throw new Error("--supervisor-program is required");
  }
  if (!args.controlSocket) {
    throw new Error("--control-socket is required");
  }
  validateAbsolutePath("--supervisor-program", args.supervisorProgram);
  validateAbsolutePath("--control-socket", args.controlSocket);
  if (/\s|\0/.test(args.controlSocket)) {
    throw new Error("--control-socket must not contain whitespace or NUL");
  }
  if (!fs.existsSync(args.supervisorProgram)) {
    throw new Error(`--supervisor-program does not exist: ${args.supervisorProgram}`);
  }
  if (!isExecutable(args.supervisorProgram)) {
    throw new Error(`--supervisor-program is not executable: ${args.supervisorProgram}`);
  }
  if (!/^[A-Za-z0-9:_-]+$/.test(args.adapterId)) {
    throw new Error("--adapter-id must be a safe identifier");
  }
  if (!["selkies_gstreamer", "hosted_remote_browser"].includes(args.engineKind)) {
    throw new Error("--engine-kind must be selkies_gstreamer or hosted_remote_browser");
  }
  if (!/^[a-z0-9][a-z0-9_-]*$/.test(args.displayBackend)) {
    throw new Error("--display-backend must be a safe backend identifier");
  }
  if (!Number.isInteger(args.timeoutMs) || args.timeoutMs < 1000 || args.timeoutMs > 300000) {
    throw new Error("--timeout-ms must be an integer from 1000 to 300000");
  }
  for (const value of args.supervisorArgs) {
    if (/[\0]/.test(value)) {
      throw new Error("--supervisor-arg must not contain NUL");
    }
  }
  for (const [key, value] of Object.entries(args.env)) {
    if (/[\0]/.test(value)) {
      throw new Error(`environment value for ${key} must not contain NUL`);
    }
  }
}

function validateAbsolutePath(label, value) {
  if (typeof value !== "string" || !value.startsWith("/")) {
    throw new Error(`${label} must be an absolute path`);
  }
}

function isExecutable(file) {
  try {
    fs.accessSync(file, fs.constants.X_OK);
    return true;
  } catch {
    return false;
  }
}

function buildConfig(args) {
  return {
    adapters: [
      {
        id: args.adapterId,
        kind: args.engineKind,
        network_mode: "runtime_net_only",
        display_modes: ["webrtc_remote_display"],
        supervisor: {
          program: args.supervisorProgram,
          args: args.supervisorArgs,
          env: {
            ELASTOS_BROWSER_PRODUCT_ENGINE: args.engineKind,
            ELASTOS_BROWSER_PRODUCT_DISPLAY_BACKEND: args.displayBackend,
            ELASTOS_BROWSER_HOSTED_PRODUCT_CONTROL_SOCKET: args.controlSocket,
            ...args.env,
          },
          timeout_ms: args.timeoutMs,
          control_socket_path: args.controlSocket,
        },
      },
    ],
  };
}

function writeJson(file, value) {
  fs.writeFileSync(file, `${JSON.stringify(value, null, 2)}\n`, "utf8");
}

function main() {
  try {
    const args = parseArgs(process.argv.slice(2));
    validate(args);
    const config = buildConfig(args);
    const outDir = path.resolve(args.outDir);
    fs.mkdirSync(outDir, { recursive: true });
    writeJson(path.join(outDir, "browser-engine-adapter.json"), config);
    console.log(JSON.stringify({
      ok: true,
      out_dir: outDir,
      files: ["browser-engine-adapter.json"],
      adapter_id: args.adapterId,
      engine: args.engineKind,
      display_mode: "webrtc_remote_display",
      display_backend: args.displayBackend,
      backend_class: "product_compositor",
      audio_required: true,
      candidate: args.candidate || null,
      control_socket: args.controlSocket,
      direct_network: false,
    }));
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error));
    console.error(usage());
    process.exit(1);
  }
}

main();
