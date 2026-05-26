#!/usr/bin/env node
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";
import process from "node:process";

const selkiesBaselineConfig = "/tmp/elastos-browser-selkies-live/browser-engine-adapter.json";
const gatewayLiveConfig = "/home/wau/.local/share/elastos-public-gateway-live/xdg-data/elastos/config/browser-engine-adapter.json";

function defaultLiveConfig() {
  if (process.env.ELASTOS_BROWSER_ENGINE_ADAPTER_CONFIG) {
    return process.env.ELASTOS_BROWSER_ENGINE_ADAPTER_CONFIG;
  }
  if (fs.existsSync(gatewayLiveConfig)) {
    return gatewayLiveConfig;
  }
  return selkiesBaselineConfig;
}

function usage() {
  console.error(`Usage:
  node scripts/browser-provider-decision-report.mjs \\
    [--adapter-config /path/to/browser-engine-adapter.json] \\
    [--browserbox-config /path/to/browserbox/browser-engine-adapter.json] \\
    [--kasm-workspaces-config /path/to/kasm/browser-engine-adapter.json] \\
    [--kasmvnc-config /path/to/kasmvnc/browser-engine-adapter.json] \\
    [--native-browser-program /absolute/path/to/chromium-or-cef] \\
    [--hosted-bakeoff /path/to/bakeoff.json] \\
    [--native-preflight /path/to/native-preflight.json] \\
    [--manual-ux /path/to/manual-ux.json]

Reports the current Browser provider decision state without launching browsers,
installing vendors, or stopping services. This is a status/decision helper; the
acceptance gate remains scripts/browser-objective-audit.mjs.
`);
}

function parseArgs(argv) {
  const args = {
    adapterConfig: defaultLiveConfig(),
    browserboxConfig: "",
    kasmWorkspacesConfig: "",
    kasmvncConfig: "",
    nativeBrowserProgram: process.env.ELASTOS_BROWSER_NATIVE_BROWSER_PROGRAM || "",
    hostedBakeoff: "",
    nativePreflight: "",
    manualUx: "",
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
    } else if (arg === "--adapter-config") {
      args.adapterConfig = next();
    } else if (arg === "--browserbox-config") {
      args.browserboxConfig = next();
    } else if (arg === "--kasm-workspaces-config") {
      args.kasmWorkspacesConfig = next();
    } else if (arg === "--kasmvnc-config") {
      args.kasmvncConfig = next();
    } else if (arg === "--native-browser-program") {
      args.nativeBrowserProgram = next();
    } else if (arg === "--hosted-bakeoff") {
      args.hostedBakeoff = next();
    } else if (arg === "--native-preflight") {
      args.nativePreflight = next();
    } else if (arg === "--manual-ux") {
      args.manualUx = next();
    } else {
      throw new Error(`unknown option: ${arg}`);
    }
  }
  return args;
}

function readJson(file) {
  try {
    return JSON.parse(fs.readFileSync(file, "utf8"));
  } catch {
    return null;
  }
}

function isSocket(path) {
  if (!path) return false;
  try {
    return fs.statSync(path).isSocket();
  } catch {
    return false;
  }
}

function run(command, args) {
  const result = spawnSync(command, args, {
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
  });
  return {
    status: result.status,
    stdout: result.stdout,
    stderr: result.stderr,
  };
}

function commandPath(command) {
  const result = run("sh", ["-lc", `command -v '${command.replaceAll("'", "'\\''")}'`]);
  return result.status === 0 ? result.stdout.trim() : "";
}

function discoverNativeBrowserProgram(explicitProgram) {
  if (explicitProgram) return explicitProgram;
  for (const command of ["chromium", "chromium-browser", "google-chrome", "google-chrome-stable", "brave-browser"]) {
    const found = commandPath(command);
    if (found) return found;
  }
  for (const candidate of [
    "/home/wau/.cache/ms-playwright/chromium-1217/chrome-linux64/chrome",
    "/home/wau/.cache/ms-playwright/chromium-1208/chrome-linux64/chrome",
    "/home/wau/.cache/ms-playwright/chromium-1181/chrome-linux/chrome",
  ]) {
    if (fs.existsSync(candidate)) return candidate;
  }
  return "";
}

function adapterSummary(adapterConfigPath) {
  const config = readJson(adapterConfigPath);
  const adapter = Array.isArray(config?.adapters) ? config.adapters[0] : null;
  const controlSocket = adapter?.supervisor?.control_socket_path || "";
  const supervisorProgram = adapter?.supervisor?.program || "";
  return {
    path: adapterConfigPath,
    exists: fs.existsSync(adapterConfigPath),
    id: adapter?.id || null,
    kind: adapter?.kind || null,
    supervisor_program: supervisorProgram || null,
    per_launch_supervisor: supervisorProgram.includes("browser-per-launch-selkies-supervisor"),
    display_backend: adapter?.supervisor?.env?.ELASTOS_BROWSER_PRODUCT_DISPLAY_BACKEND || null,
    network_mode: adapter?.network_mode || null,
    display_modes: adapter?.display_modes || [],
    control_socket: controlSocket || null,
    control_socket_available: isSocket(controlSocket),
    control_status: controlStatus(controlSocket),
  };
}

function controlStatus(controlSocket) {
  if (!isSocket(controlSocket)) return null;
  const result = run("curl", [
    "-sS",
    "-m",
    "2",
    "--unix-socket",
    controlSocket,
    "http://browser-engine/status",
  ]);
  if (result.status !== 0) {
    return {
      ok: false,
      error: result.stderr.trim() || `curl exited ${result.status}`,
    };
  }
  const parsed = parseLastJson(result.stdout);
  return parsed
    ? {
        ok: true,
        ...parsed,
      }
    : {
        ok: false,
        error: "control status did not return JSON",
      };
}

function systemdSummary() {
  const active = run("systemctl", ["is-active", "elastos-browser-selkies"]);
  const enabled = run("systemctl", ["is-enabled", "elastos-browser-selkies"]);
  const show = run("systemctl", [
    "show",
    "elastos-browser-selkies",
    "--property=LoadState,ActiveState,SubState,FragmentPath,MainPID",
    "--no-pager",
  ]);
  const properties = {};
  for (const line of show.stdout.trim().split(/\r?\n/)) {
    const separator = line.indexOf("=");
    if (separator > 0) {
      properties[line.slice(0, separator)] = line.slice(separator + 1);
    }
  }
  return {
    unit: "elastos-browser-selkies.service",
    active: active.stdout.trim() || "unknown",
    enabled: enabled.stdout.trim() || "unknown",
    properties,
  };
}

function objectiveAudit(args) {
  const auditArgs = ["scripts/browser-objective-audit.mjs"];
  if (args.hostedBakeoff) auditArgs.push("--hosted-bakeoff", args.hostedBakeoff);
  if (args.nativePreflight) auditArgs.push("--native-preflight", args.nativePreflight);
  if (args.manualUx) auditArgs.push("--manual-ux", args.manualUx);
  const result = run("node", auditArgs);
  return {
    status: result.status,
    result: parseLastJson(result.stdout),
    stderr: result.stderr.trim(),
  };
}

function hostedBakeoffSummary(file) {
  if (!file) return null;
  const artifact = readJson(file);
  if (!artifact) {
    return {
      path: file,
      exists: fs.existsSync(file),
      ok: false,
      error: "hosted bake-off artifact is missing or invalid JSON",
    };
  }
  const youtubeTail = Array.isArray(artifact.youtube_stress?.error_tail)
    ? artifact.youtube_stress.error_tail.filter(Boolean).slice(-3)
    : [];
  return {
    path: file,
    schema: artifact.schema || null,
    candidate: artifact.candidate || null,
    ok: artifact.ok === true,
    candidate_gate_ok: artifact.candidate_gate?.ok === true,
    youtube_stress_ok: artifact.youtube_stress?.ok === true,
    youtube_stress_skipped: artifact.youtube_stress?.skipped === true,
    partial_candidate_ok: artifact.partial_candidate_ok === true,
    product_acceptance: artifact.product_acceptance || null,
    failure_tail: artifact.ok === true ? [] : youtubeTail,
  };
}

function nativePreflightSummary(file) {
  if (!file) return null;
  const artifact = readJson(file);
  if (!artifact) {
    return {
      path: file,
      exists: fs.existsSync(file),
      ok: false,
      raw_ok: false,
      product_media_ok: false,
      error: "native preflight artifact is missing or invalid JSON",
    };
  }
  const productMediaOk =
    artifact.schema === "elastos.browser.native-target-preflight/v1" &&
    artifact.ok === true &&
    artifact.native_audio_declared === true &&
    artifact.native_video_declared === true &&
    artifact.native_audio_proven === true &&
    artifact.native_video_proven === true &&
    artifact.native_media_required === true &&
    artifact.direct_network === false &&
    artifact.network_mode === "runtime_net_only";
  return {
    path: file,
    schema: artifact.schema || null,
    ok: productMediaOk,
    raw_ok: artifact.ok === true,
    product_media_ok: productMediaOk,
    native_audio_declared: artifact.native_audio_declared === true,
    native_video_declared: artifact.native_video_declared === true,
    native_audio_proven: artifact.native_audio_proven === true,
    native_video_proven: artifact.native_video_proven === true,
    native_media_required: artifact.native_media_required === true,
    direct_network: artifact.direct_network,
    network_mode: artifact.network_mode || null,
    browser_program: artifact.browser_program || null,
    out_dir: artifact.out_dir || null,
  };
}

function nativeHostCapability(args) {
  const browserProgram = discoverNativeBrowserProgram(args.nativeBrowserProgram);
  if (!browserProgram) {
    return {
      status: 1,
      result: null,
      browser_program: null,
      blockers: ["no native browser program found; set --native-browser-program or ELASTOS_BROWSER_NATIVE_BROWSER_PROGRAM"],
    };
  }
  const result = run("node", [
    "scripts/browser-native-host-capability.mjs",
    "--browser-program",
    browserProgram,
    "--require-product-native",
  ]);
  const parsed = parseLastJson(result.stdout);
  return {
    status: result.status,
    result: parsed,
    browser_program: browserProgram,
    blockers: nativeHostBlockers(parsed, result),
  };
}

function nativeHostBlockers(parsed, result) {
  if (!parsed?.checks) {
    return [result.stderr.trim() || "native host capability probe did not produce checks"];
  }
  return parsed.checks
    .filter((check) => check.ok !== true)
    .map((check) => `${check.name}${check.detail ? ` detail=${check.detail}` : ""}`);
}

function hostedPreflight(candidate, configPath) {
  const prepared = configPath
    ? {
        configPath,
        generated: false,
      }
    : generateCandidateConfig(candidate);
  if (!prepared.configPath) {
    return {
      candidate,
      ok: false,
      ready_for_bakeoff: false,
      skipped: true,
      blockers: prepared.blockers,
    };
  }
  const result = run("node", [
    "scripts/browser-hosted-provider-preflight.mjs",
    "--candidate",
    candidate,
    "--adapter-config",
    prepared.configPath,
  ]);
  const parsed = parseLastJson(result.stdout);
  const adapterConfig = readJson(prepared.configPath);
  const adapter = Array.isArray(adapterConfig?.adapters) ? adapterConfig.adapters[0] : null;
  const perLaunchSelkies =
    candidate === "selkies" &&
    adapter?.kind === "selkies_gstreamer" &&
    adapter?.display_modes?.includes("webrtc_remote_display") &&
    String(adapter?.supervisor?.program || "").includes("browser-per-launch-selkies-supervisor");
  if (prepared.cleanupDir) {
    fs.rmSync(prepared.cleanupDir, { recursive: true, force: true });
  }
  if (perLaunchSelkies && result.status !== 0) {
    return {
      candidate,
      ok: false,
      ready_for_bakeoff: false,
      preflight_ready_for_bakeoff: false,
      status: result.status,
      adapter_config: prepared.configPath,
      generated_config: prepared.generated,
      generated_config_removed: Boolean(prepared.cleanupDir),
      result: parsed,
      blockers: [
        "per-launch Selkies has no durable operator control socket for hosted-provider bake-off; use per-launch/WebRTC/navigation smokes or provision a separate bake-off target with private CDP",
      ],
    };
  }
  return {
    candidate,
    ok: result.status === 0 && parsed?.ok === true,
    ready_for_bakeoff: parsed?.ready_for_bakeoff === true,
    status: result.status,
    adapter_config: prepared.configPath,
    generated_config: prepared.generated,
    generated_config_removed: Boolean(prepared.cleanupDir),
    result: parsed,
    blockers: preflightBlockers(parsed, { generatedConfig: prepared.generated }),
  };
}

function generateCandidateConfig(candidate) {
  if (candidate === "selkies") {
    return {
      configPath: "",
      generated: false,
      blockers: ["Selkies should use the live adapter config, not a generated placeholder."],
    };
  }
  const safeCandidate = candidate.replace(/[^A-Za-z0-9_.-]/g, "-");
  const outDir = fs.mkdtempSync(path.join(os.tmpdir(), `elastos-browser-${safeCandidate}-`));
  const controlSocket = path.join(outDir, `${safeCandidate}.sock`);
  const supervisor = path.resolve("scripts/browser-hosted-product-supervisor.mjs");
  const result = run("node", [
    "scripts/browser-hosted-product-operator-config.mjs",
    "--candidate",
    candidate,
    "--out-dir",
    outDir,
    "--supervisor-program",
    supervisor,
    "--control-socket",
    controlSocket,
  ]);
  if (result.status !== 0) {
    return {
      configPath: "",
      generated: false,
      blockers: [
        `candidate config generation failed: ${result.stderr.trim() || result.stdout.trim() || `exit ${result.status}`}`,
      ],
    };
  }
  return {
    configPath: path.join(outDir, "browser-engine-adapter.json"),
    generated: true,
    cleanupDir: outDir,
    blockers: [],
  };
}

function preflightBlockers(parsed, options = {}) {
  if (!parsed?.checks) return ["preflight did not produce checks"];
  return parsed.checks
    .filter((check) => check.ok !== true)
    .map((check) => {
      if (options.generatedConfig && check.name === "operator_control_socket") {
        return "operator_control_socket not provisioned; configure a durable operator control socket for this candidate";
      }
      const expected = check.expected ? ` expected=${check.expected}` : "";
      const actual = check.actual ? ` actual=${JSON.stringify(check.actual)}` : "";
      const detail = check.detail ? ` detail=${check.detail}` : "";
      return `${check.name}${expected}${actual}${detail}`;
    });
}

function candidateReadiness(args) {
  const readiness = [
    hostedPreflight("selkies", args.adapterConfig),
    hostedPreflight("kasm-workspaces", args.kasmWorkspacesConfig),
    hostedPreflight("browserbox", args.browserboxConfig),
    hostedPreflight("kasmvnc", args.kasmvncConfig),
  ];
  return readiness;
}

function singleSessionBusy(adapter) {
  return Number(adapter?.control_status?.active_pages || 0) > 0 && adapter?.control_status?.single_session === true;
}

function activePageIds(adapter) {
  const ids = adapter?.control_status?.page_ids;
  return Array.isArray(ids)
    ? ids.filter((id) => typeof id === "string" && id.trim().length > 0).map((id) => id.trim())
    : [];
}

function shellQuote(value) {
  return `'${String(value).replaceAll("'", "'\\''")}'`;
}

function applyLiveReadinessBlockers(readiness, adapter) {
  if (!singleSessionBusy(adapter)) {
    return readiness;
  }
  return readiness.map((candidate) => {
    if (candidate.candidate !== "selkies") {
      return candidate;
    }
    const activePages = Number(adapter.control_status.active_pages || 0);
    return {
      ...candidate,
      preflight_ready_for_bakeoff: candidate.ready_for_bakeoff,
      ready_for_bakeoff: false,
      blockers: [
        ...(candidate.blockers || []),
        `single-session target has ${activePages} active page(s); close the page or provision a separate provider instance before bake-off`,
      ],
    };
  });
}

function parseLastJson(raw) {
  const lines = raw.trim().split(/\r?\n/).filter(Boolean);
  for (let index = lines.length - 1; index >= 0; index -= 1) {
    const line = lines.slice(0, index + 1).join("\n");
    try {
      return JSON.parse(line);
    } catch {
      continue;
    }
  }
  return null;
}

function recommendation(audit, readiness, adapter, nativeHost) {
  const criteria = audit?.result?.criteria || [];
  const failed = criteria.filter((item) => item.ok !== true).map((item) => item.id);
  if (audit?.result?.ok === true) {
    return {
      decision: "accepted",
      next: "Use the accepted provider and keep objective/manual UX artifacts with the release.",
    };
  }
  if (failed.includes("hosted_provider_product_accepted")) {
    if (nativeHost?.result?.ready?.product_native === true) {
      return {
        decision: "not_accepted",
        next: "This host appears ready for the native product path. Run browser-native-target-preflight.sh with explicit native audio/video and --artifact-out, then record manual UX evidence against that artifact.",
      };
    }
    if (singleSessionBusy(adapter)) {
      return {
        decision: "not_accepted",
        next: "Current Selkies baseline is single-session and has an active page. Do not run product bake-offs until the active Browser page is closed, then compare Kasm Workspaces first, BrowserBox if licensed, rather than tuning Selkies by default.",
      };
    }
    const nextReady = readiness.find((candidate) => candidate.candidate !== "selkies" && candidate.ready_for_bakeoff);
    if (nextReady) {
      return {
        decision: "not_accepted",
        next: `Run ${nextReady.candidate} through browser-hosted-provider-bakeoff.sh with --resize-width/--resize-height and --artifact-out, then record manual UX evidence against that artifact if it passes.`,
      };
    }
    return {
      decision: "not_accepted",
      next: "This host is not ready for native product media. Run Kasm Workspaces first, then BrowserBox if licensed or KasmVNC if Workspaces is unavailable, through the hosted_remote_browser bake-off. Do not keep tuning Selkies unless a measured gate says it closes the gap.",
    };
  }
  if (failed.includes("native_product_media_accepted")) {
    return {
      decision: "not_accepted",
      next: "Run the native target preflight with explicit native audio/video and --artifact-out on a real launcher/mobile/Jetson target.",
    };
  }
  return {
    decision: "not_accepted",
    next: "Record manual UX evidence for the accepted provider.",
  };
}

function goalStatus(audit, readiness, adapter, nativeHost, hostedBakeoff, nativePreflight) {
  if (audit?.result?.ok === true) {
    return {
      status: "accepted",
      reason: "Browser/audio objective has accepted product proof and manual UX evidence.",
    };
  }
  if (hostedBakeoff?.candidate_gate_ok === true && hostedBakeoff?.youtube_stress_ok !== true) {
    return {
      status: "blocked",
      reason: `Supplied ${hostedBakeoff.candidate || "hosted"} bake-off passed candidate gates but failed YouTube/product media stress; compare a better hosted provider or run native preflight on a capable target.`,
    };
  }
  if (nativePreflight && nativePreflight.ok !== true) {
    return {
      status: "blocked",
      reason: "Supplied native preflight did not prove required native audio/video media readiness; rerun native preflight on a capable target with --native-audio --native-video --require-native-media.",
    };
  }
  const hasMissingObjectiveProof = (audit?.result?.prompt_to_artifact_checklist || []).some((item) => item.ok !== true);
  const selkiesBusy = singleSessionBusy(adapter);
  const nativeBlocked = nativeHost?.result?.ready?.product_native !== true;
  const hostedCandidatesBlocked = (readiness || [])
    .filter((candidate) => candidate.candidate !== "selkies")
    .every((candidate) => candidate.ready_for_bakeoff !== true);

  if (hasMissingObjectiveProof && (selkiesBusy || nativeBlocked || hostedCandidatesBlocked)) {
    const blockers = [];
    if (selkiesBusy) {
      blockers.push("free or isolate the Selkies baseline");
    }
    if (hostedCandidatesBlocked) {
      blockers.push("provision Kasm Workspaces, BrowserBox, or KasmVNC");
    }
    if (nativeBlocked) {
      blockers.push("run native preflight on a host with compositor/audio/network isolation");
    }
    return {
      status: "blocked",
      reason: `Browser/audio completion requires external provider/native evidence: ${blockers.join("; ")}.`,
    };
  }
  return {
    status: "incomplete",
    reason: "Browser/audio objective is missing proof; run the next recommended gate.",
  };
}

function nextAction(audit, readiness, adapter, nativeHost) {
  if (audit?.result?.ok === true) {
    return {
      id: "keep_accepted_browser_artifacts",
      status: "ready",
      owner: "release",
      summary: "Keep the accepted product provider proof and matching manual UX report with the release artifacts.",
      commands: [
        "node scripts/browser-objective-audit.mjs --hosted-bakeoff <accepted-proof.json> --manual-ux <manual-ux.json>",
      ],
    };
  }

  if (nativeHost?.result?.ready?.product_native === true) {
    return {
      id: "run_native_product_preflight",
      status: "actionable",
      owner: "operator",
      summary: "Run the native Browser preflight on this host with explicit audio/video requirements, then record hash-bound manual UX evidence if it passes.",
      commands: [
        "scripts/browser-native-target-preflight.sh --out-dir <dir> --browser-program <chromium-or-cef> --native-audio --native-video --require-native-media --artifact-out <dir>/native-preflight.json",
        "node scripts/browser-manual-ux-report.mjs --template --machine-artifact <dir>/native-preflight.json",
      ],
    };
  }

  if (singleSessionBusy(adapter)) {
    const pageIds = activePageIds(adapter);
    const commands = [
      "node scripts/browser-provider-decision-report.mjs",
      "node scripts/browser-provider-runbook.mjs",
    ];
    if (adapter?.control_socket && pageIds.length === 1) {
      commands.push(
        `node scripts/browser-selkies-close-page.mjs --control-socket ${shellQuote(adapter.control_socket)} --page-id ${shellQuote(pageIds[0])}`,
        `node scripts/browser-selkies-close-page.mjs --control-socket ${shellQuote(adapter.control_socket)} --page-id ${shellQuote(pageIds[0])} --confirm-close`,
      );
    }
    return {
      id: "free_or_isolate_selkies_before_bakeoff",
      status: "blocked",
      owner: "operator",
      summary: "The current Selkies baseline is single-session and has an active page. Close that Browser page or provision a separate provider instance before running more hosted bake-offs.",
      commands,
    };
  }

  const nextReady = readiness.find((candidate) => candidate.candidate !== "selkies" && candidate.ready_for_bakeoff);
  if (nextReady) {
    return {
      id: "run_hosted_provider_bakeoff",
      status: "actionable",
      owner: "operator",
      candidate: nextReady.candidate,
      summary: `Run ${nextReady.candidate} through the hosted Browser provider bake-off, then record hash-bound manual UX evidence if it passes.`,
      commands: [
        `scripts/browser-hosted-provider-bakeoff.sh --candidate ${nextReady.candidate} --adapter-config <${nextReady.candidate}-browser-engine-adapter.json> --resize-width 1000 --resize-height 700 --artifact-out <${nextReady.candidate}-hosted-bakeoff.json>`,
        `node scripts/browser-manual-ux-report.mjs --template --machine-artifact <${nextReady.candidate}-hosted-bakeoff.json>`,
      ],
    };
  }

  const kasm = readiness.find((candidate) => candidate.candidate === "kasm-workspaces");
  return {
    id: "provision_kasm_workspaces_first",
    status: "blocked",
    owner: "operator",
    summary: "Provision Kasm Workspaces as the first non-Selkies hosted comparison, including API credentials, audio-enabled session policy, and a product display bridge. If Kasm is unavailable, provision BrowserBox with an accepted license.",
    blockers: kasm?.blockers || [],
    commands: [
      "node scripts/browser-provider-runbook.mjs",
      "node scripts/browser-hosted-provider-preflight.mjs --candidate kasm-workspaces --adapter-config <kasm-browser-engine-adapter.json>",
    ],
  };
}

function blockedBy(audit, readiness, adapter, nativeHost) {
  if (audit?.result?.ok === true) {
    return [];
  }
  const blockers = [];
  for (const item of audit?.result?.prompt_to_artifact_checklist || []) {
    if (item.ok !== true) {
      blockers.push({
        id: item.id,
        source: "objective_audit",
        message: item.missing || item.requirement || "objective checklist item is not satisfied",
      });
    }
  }
  if (singleSessionBusy(adapter)) {
    blockers.push({
      id: "selkies_single_session_busy",
      source: "live_adapter",
      message: `Selkies single-session baseline has ${Number(adapter.control_status.active_pages)} active page(s); serialize smokes or use a separate provider instance.`,
    });
  }
  if (nativeHost?.result?.ready?.product_native !== true) {
    blockers.push({
      id: "native_host_not_product_ready",
      source: "native_host_capability",
      message: (nativeHost?.blockers || []).join("; ") || "native host is not product-ready",
    });
  }
  for (const candidate of readiness || []) {
    if (candidate.candidate !== "selkies" && candidate.ready_for_bakeoff !== true) {
      blockers.push({
        id: `${candidate.candidate}_not_ready`,
        source: "candidate_readiness",
        message: (candidate.blockers || []).join("; ") || "candidate is not ready for bake-off",
      });
    }
  }
  return blockers;
}

function hostedBakeoffBlocker(summary) {
  if (!summary || summary.ok === true) return null;
  const reason = summary.candidate_gate_ok === true && summary.youtube_stress_ok !== true
    ? "candidate gate passed but YouTube/product media stress did not pass"
    : summary.error || "hosted bake-off did not pass";
  return {
    id: "hosted_bakeoff_rejected",
    source: "hosted_bakeoff",
    message: `${summary.path}: ${reason}`,
  };
}

function nativePreflightBlocker(summary) {
  if (!summary || summary.ok === true) return null;
  const reason = summary.error
    || (summary.raw_ok === true
      ? "native preflight did not prove required native audio/video media readiness"
      : "native preflight did not pass");
  return {
    id: "native_preflight_rejected",
    source: "native_preflight",
    message: `${summary.path}: ${reason}`,
  };
}

function main() {
  const args = parseArgs(process.argv.slice(2));
  const adapter = adapterSummary(args.adapterConfig);
  const systemd = systemdSummary();
  const audit = objectiveAudit(args);
  const hostedBakeoff = hostedBakeoffSummary(args.hostedBakeoff);
  const nativePreflight = nativePreflightSummary(args.nativePreflight);
  const readiness = applyLiveReadinessBlockers(candidateReadiness(args), adapter);
  const nativeHost = nativeHostCapability(args);
  const blockers = blockedBy(audit, readiness, adapter, nativeHost);
  const bakeoffBlocker = hostedBakeoffBlocker(hostedBakeoff);
  if (bakeoffBlocker) blockers.push(bakeoffBlocker);
  const preflightBlocker = nativePreflightBlocker(nativePreflight);
  if (preflightBlocker) blockers.push(preflightBlocker);
  const report = {
    schema: "elastos.browser.provider-decision-report/v1",
    ok: audit.result?.ok === true,
    live_adapter: adapter,
    selkies_service: systemd,
    docker_is_product_architecture: false,
    selkies_role: "managed_baseline_not_final_product",
    native_host_capability: nativeHost,
    candidate_readiness: readiness,
    hosted_bakeoff: hostedBakeoff,
    native_preflight: nativePreflight,
    objective_audit: audit.result,
    goal_status: goalStatus(audit, readiness, adapter, nativeHost, hostedBakeoff, nativePreflight),
    blocked_by: blockers,
    recommendation: recommendation(audit, readiness, adapter, nativeHost),
    next_action: nextAction(audit, readiness, adapter, nativeHost),
  };
  console.log(JSON.stringify(report, null, 2));
  if (!report.ok) {
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
