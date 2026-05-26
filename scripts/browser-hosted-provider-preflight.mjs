#!/usr/bin/env node
import fs from "node:fs";
import { spawnSync } from "node:child_process";
import process from "node:process";

function usage() {
  console.error(`Usage:
  node scripts/browser-hosted-provider-preflight.mjs \\
    --candidate browserbox|kasm-workspaces|selkies|<id> \\
    [--adapter-config /path/to/browser-engine-adapter.json] \\
    [--control-socket /path/to/provider-control.sock]

Checks whether a hosted Browser provider candidate is ready to run through
scripts/browser-hosted-provider-bakeoff.sh.

This script is intentionally fail-closed:
  - it does not install BrowserBox, Kasm, or Selkies,
  - it does not accept a candidate without an operator control surface,
  - it does not treat a vendor URL or CLI as an ElastOS Browser proof.
`);
}

function parseArgs(argv) {
  const args = {
    candidate: "",
    adapterConfig: "",
    controlSocket: "",
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
    } else if (arg === "--candidate") {
      args.candidate = next();
    } else if (arg === "--adapter-config") {
      args.adapterConfig = next();
    } else if (arg === "--control-socket") {
      args.controlSocket = next();
    } else {
      throw new Error(`unknown argument: ${arg}`);
    }
  }
  if (!args.candidate) {
    throw new Error("--candidate is required");
  }
  if (!/^[A-Za-z0-9_.:-]+$/.test(args.candidate)) {
    throw new Error("--candidate must be a safe identifier");
  }
  return args;
}

function commandExists(name) {
  const result = spawnSync("sh", ["-c", `command -v ${name}`], {
    encoding: "utf8",
    stdio: ["ignore", "pipe", "ignore"],
  });
  return result.status === 0 ? result.stdout.trim() : "";
}

function isSocket(path) {
  if (!path) return false;
  try {
    return fs.statSync(path).isSocket();
  } catch {
    return false;
  }
}

function readAdapterConfig(file) {
  if (!file) return null;
  const config = JSON.parse(fs.readFileSync(file, "utf8"));
  const adapter = Array.isArray(config.adapters) ? config.adapters[0] : null;
  return {
    kind: adapter?.kind || "",
    display_modes: adapter?.display_modes || [],
    control_socket_path: adapter?.supervisor?.control_socket_path || "",
    product_engine: adapter?.supervisor?.env?.ELASTOS_BROWSER_PRODUCT_ENGINE || "",
    display_backend: adapter?.supervisor?.env?.ELASTOS_BROWSER_PRODUCT_DISPLAY_BACKEND || "",
  };
}

function candidateExpectations(candidate) {
  if (candidate === "browserbox") {
    return {
      expected_kind: "hosted_remote_browser",
      expected_display_backend: "browserbox_webrtc",
      commands: ["bbx"],
      env: ["BROWSERBOX_LICENSE_CONFIRMED"],
      notes: [
        "BrowserBox is commercially licensed; BROWSERBOX_LICENSE_CONFIRMED=1 records operator intent only.",
        "The actual proof is still the bake-off gate, not CLI presence.",
      ],
    };
  }
  if (candidate === "kasm-workspaces" || candidate === "kasmvnc") {
    return {
      expected_kind: "hosted_remote_browser",
      expected_display_backend: candidate === "kasmvnc" ? "kasmvnc_webrtc" : "kasm_workspaces_webrtc",
      commands: [],
      env: ["KASM_BASE_URL", "KASM_API_KEY", "KASM_API_KEY_SECRET"],
      notes: [
        "Kasm must create a running session through the operator control service.",
        "Audio must be proven by the bake-off gate; standalone VNC is not an audio proof.",
      ],
    };
  }
  if (candidate === "selkies" || candidate === "selkies-baseline") {
    return {
      expected_kind: "selkies_gstreamer",
      expected_display_backend: "selkies_gstreamer_webrtc",
      commands: [],
      env: [],
      notes: [
        "Selkies is the current baseline, not the final browser acceptance answer.",
      ],
    };
  }
  return {
    expected_kind: "hosted_remote_browser",
    expected_display_backend: "",
    commands: [],
    env: [],
    notes: [
      "Unknown candidate: preflight can only validate generic hosted_remote_browser wiring.",
    ],
  };
}

function main() {
  const args = parseArgs(process.argv.slice(2));
  const expectations = candidateExpectations(args.candidate);
  const adapter = readAdapterConfig(args.adapterConfig);
  const checks = [];

  if (args.adapterConfig) {
    checks.push({
      name: "adapter_config_exists",
      ok: fs.existsSync(args.adapterConfig),
      detail: args.adapterConfig,
    });
  }
  if (adapter) {
    checks.push({
      name: "adapter_kind",
      ok: adapter.kind === expectations.expected_kind,
      expected: expectations.expected_kind,
      actual: adapter.kind,
    });
    checks.push({
      name: "webrtc_remote_display",
      ok: adapter.display_modes.includes("webrtc_remote_display"),
      actual: adapter.display_modes,
    });
    if (expectations.expected_display_backend) {
      checks.push({
        name: "display_backend",
        ok: adapter.display_backend === expectations.expected_display_backend,
        expected: expectations.expected_display_backend,
        actual: adapter.display_backend,
      });
    }
  }

  const controlSocket = args.controlSocket || adapter?.control_socket_path || "";
  checks.push({
    name: "operator_control_socket",
    ok: isSocket(controlSocket),
    detail: controlSocket || "missing",
  });

  for (const command of expectations.commands) {
    const path = commandExists(command);
    checks.push({
      name: `command:${command}`,
      ok: Boolean(path),
      detail: path || "missing",
    });
  }

  for (const name of expectations.env) {
    checks.push({
      name: `env:${name}`,
      ok: Boolean(process.env[name]),
      detail: process.env[name] ? "set" : "missing",
    });
  }

  const ready = checks.every((check) => check.ok);
  console.log(JSON.stringify({
    ok: ready,
    schema: "elastos.browser.hosted-provider-preflight/v1",
    candidate: args.candidate,
    ready_for_bakeoff: ready,
    checks,
    notes: expectations.notes,
    next_command: ready
      ? "scripts/browser-hosted-provider-bakeoff.sh --candidate <id> --adapter-config <config> --cdp-endpoint <loopback-cdp> --artifact-out <hosted-bakeoff.json>"
      : null,
  }, null, 2));
  if (!ready) {
    process.exit(1);
  }
}

main();
