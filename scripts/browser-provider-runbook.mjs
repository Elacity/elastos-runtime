#!/usr/bin/env node
import { spawnSync } from "node:child_process";
import process from "node:process";

const nodeBinary = process.execPath;

function usage() {
  console.error(`Usage:
  node scripts/browser-provider-runbook.mjs \\
    [--decision-report /path/to/report.json] \\
    [--hosted-bakeoff /path/to/bakeoff.json] \\
    [--native-preflight /path/to/native-preflight.json] \\
    [--manual-ux /path/to/manual-ux.json]

Prints a redacted operator runbook for the next Browser provider proof. It does
not install vendors, launch browsers, or stop services.

When proof artifacts are provided, the runbook regenerates the decision report
with those artifacts so the visible blockers and next action match the actual
evidence. Do not combine proof artifacts with --decision-report; a precomputed
report is already the source of truth.
`);
}

function parseArgs(argv) {
  const args = {
    decisionReport: "",
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
    } else if (arg === "--decision-report") {
      args.decisionReport = next();
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
  if (
    args.decisionReport &&
    (args.hostedBakeoff || args.nativePreflight || args.manualUx)
  ) {
    throw new Error(
      "--decision-report cannot be combined with proof artifacts; regenerate the decision report instead",
    );
  }
  return args;
}

function run(command, args) {
  const result = spawnSync(command, args, {
    encoding: "utf8",
    maxBuffer: 16 * 1024 * 1024,
    stdio: ["ignore", "pipe", "pipe"],
  });
  if (result.error) {
    throw result.error;
  }
  return {
    status: result.status,
    stdout: result.stdout,
    stderr: result.stderr,
  };
}

function loadDecisionReport(args) {
  if (args.decisionReport) {
    const result = run(nodeBinary, ["-e", `const fs=require("fs"); process.stdout.write(fs.readFileSync(process.argv[1],"utf8"))`, args.decisionReport]);
    if (result.status !== 0) {
      throw new Error(result.stderr.trim() || `failed to read decision report: ${args.decisionReport}`);
    }
    return JSON.parse(result.stdout);
  }
  const decisionArgs = ["scripts/browser-provider-decision-report.mjs"];
  if (args.hostedBakeoff) decisionArgs.push("--hosted-bakeoff", args.hostedBakeoff);
  if (args.nativePreflight) decisionArgs.push("--native-preflight", args.nativePreflight);
  if (args.manualUx) decisionArgs.push("--manual-ux", args.manualUx);
  const result = run(nodeBinary, decisionArgs);
  const parsed = parseFirstJson(result.stdout);
  if (!parsed) {
    throw new Error(result.stderr.trim() || "browser-provider-decision-report.mjs did not produce JSON");
  }
  return parsed;
}

function parseFirstJson(raw) {
  try {
    return JSON.parse(raw);
  } catch {
    return null;
  }
}

function blockerLine(candidate, name) {
  const entry = candidateReadiness(candidate, name);
  if (!entry) return "- No readiness entry.";
  if (entry.ready_for_bakeoff) return "- Preflight ready.";
  if (!entry.blockers?.length) return "- Not ready; no blocker details were reported.";
  return entry.blockers.map((blocker) => `- ${blocker}`).join("\n");
}

function nativeHostBlockerLine(report) {
  const native = report.native_host_capability;
  if (!native) return "- No native host capability probe was reported.";
  if (native.result?.ready?.product_native === true) return "- Native host capability probe is product-ready.";
  if (!native.blockers?.length) return "- Native host is not product-ready; no blocker details were reported.";
  return native.blockers.map((blocker) => `- ${blocker}`).join("\n");
}

function candidateReadiness(report, name) {
  return (report.candidate_readiness || []).find((entry) => entry.candidate === name);
}

function shellBlock(lines) {
  return ["```bash", ...lines, "```"].join("\n");
}

function objectiveChecklistBlock(report) {
  const checklist = report.objective_audit?.prompt_to_artifact_checklist;
  if (!Array.isArray(checklist) || checklist.length === 0) {
    return "- Objective audit checklist unavailable; run `node scripts/browser-objective-audit.mjs` directly.";
  }
  return checklist
    .map((item) => {
      const marker = item.ok === true ? "[x]" : "[ ]";
      const evidence = Array.isArray(item.evidence) && item.evidence.length > 0
        ? ` Evidence: ${item.evidence.map((entry) => `\`${entry}\``).join(", ")}.`
        : "";
      const missing = item.ok === true || !item.missing ? "" : ` Missing: ${item.missing}`;
      return `- ${marker} \`${item.id}\`: ${item.requirement}${evidence}${missing}`;
    })
    .join("\n");
}

function blockingSummaryBlock(report) {
  const blockers = report.blocked_by;
  if (!Array.isArray(blockers) || blockers.length === 0) {
    return "- No top-level blockers were reported.";
  }
  return blockers
    .map((blocker) => `- \`${blocker.id || "unknown"}\` (${blocker.source || "unknown"}): ${blocker.message || "blocked"}`)
    .join("\n");
}

function nextActionBlock(report) {
  const action = report.next_action;
  if (!action || typeof action !== "object") {
    return "- No structured next action was reported. Run `node scripts/browser-provider-decision-report.mjs` directly.";
  }
  const lines = [
    `- ID: \`${action.id || "unknown"}\``,
    `- Status: \`${action.status || "unknown"}\``,
    `- Owner: \`${action.owner || "unknown"}\``,
    `- Summary: ${action.summary || "missing"}`,
  ];
  if (action.candidate) {
    lines.push(`- Candidate: \`${action.candidate}\``);
  }
  if (Array.isArray(action.blockers) && action.blockers.length > 0) {
    lines.push("- Blockers:");
    lines.push(...action.blockers.map((blocker) => `- ${blocker}`));
  }
  if (Array.isArray(action.commands) && action.commands.length > 0) {
    lines.push("");
    lines.push(shellBlock(action.commands));
  }
  const pageIds = activePageIds(report);
  const controlSocket = report.live_adapter?.control_socket;
  if (action.id === "free_or_isolate_selkies_before_bakeoff" && controlSocket && pageIds.length === 1) {
    lines.push("");
    lines.push("Operator close helper. First command is a dry run; second command mutates the live Selkies session only by explicit operator choice:");
    lines.push("");
    lines.push(shellBlock([
      `node scripts/browser-selkies-close-page.mjs --control-socket ${shellQuote(controlSocket)} --page-id ${shellQuote(pageIds[0])}`,
      `node scripts/browser-selkies-close-page.mjs --control-socket ${shellQuote(controlSocket)} --page-id ${shellQuote(pageIds[0])} --confirm-close`,
    ]));
  }
  return lines.join("\n");
}

function shellQuote(value) {
  return `'${String(value).replaceAll("'", "'\\''")}'`;
}

function currentHostStopConditionBlock(report) {
  if (report.goal_status?.status === "accepted") {
    return "- Accepted product proof is present. Preserve the accepted artifacts and manual UX evidence.";
  }
  const action = report.next_action;
  const lines = [
    "- This host is not accepted as product Browser proof until `audio_product_proven` and `manual_user_acceptance` pass.",
    "- Do not keep tuning the running Selkies baseline as product architecture.",
  ];
  if (action?.id) {
    lines.push(`- Current next action is \`${action.id}\`, owned by \`${action.owner || "unknown"}\`, status \`${action.status || "unknown"}\`.`);
  }
  if (
    report.live_adapter?.control_status?.single_session === true &&
    report.live_adapter?.control_status?.single_vm_session !== true &&
    Number(report.live_adapter?.control_status?.active_pages || 0) > 0
  ) {
    lines.push("- The live Selkies target is single-session and busy; close the active page only by operator choice, or provision a separate provider instance for bake-offs.");
    const pageIds = activePageIds(report);
    if (pageIds.length > 0) {
      lines.push(`- Active Browser page ids: ${pageIds.map((id) => `\`${id}\``).join(", ")}.`);
    }
  }
  if (report.native_host_capability?.result?.ready?.product_native !== true) {
    lines.push("- Native product proof must run on a target with browser/compositor/audio/network isolation; this host is not that target.");
  }
  return lines.join("\n");
}

function activePageIds(report) {
  const ids = report.live_adapter?.control_status?.page_ids;
  return Array.isArray(ids)
    ? ids
        .filter((id) => typeof id === "string" && id.trim().length > 0)
        .map((id) => id.trim())
    : [];
}

function markdown(report) {
  const activePages = Number(report.live_adapter?.control_status?.active_pages || 0);
  const singleSession =
    report.live_adapter?.control_status?.single_session === true &&
    report.live_adapter?.control_status?.single_vm_session !== true;
  const pageIds = activePageIds(report);
  return `# Browser Provider Operator Runbook

Generated from \`elastos.browser.provider-decision-report/v1\`.

This runbook is read-only guidance. It does not install vendors, launch
browsers, stop services, close active sessions, or change Runtime state. If the
live Selkies baseline is busy, preserve it and use a separate provider instance
for bake-offs.

## Current State

- Live adapter: \`${report.live_adapter?.kind || "unknown"}\` / \`${report.live_adapter?.display_backend || "unknown"}\`
- Selkies service: \`${report.selkies_service?.active || "unknown"}\` / \`${report.selkies_service?.enabled || "unknown"}\`
- Selkies role: \`${report.selkies_role || "unknown"}\`
- Selkies session: single-session=\`${String(singleSession)}\`, active-pages=\`${String(activePages)}\`${pageIds.length > 0 ? `, page-ids=${pageIds.map((id) => `\`${id}\``).join(", ")}` : ""}
- Docker is product architecture: \`${String(report.docker_is_product_architecture)}\`
- Native host product-ready: \`${String(report.native_host_capability?.result?.ready?.product_native === true)}\`
- Goal status: \`${report.goal_status?.status || "unknown"}\` — ${report.goal_status?.reason || "missing"}
- Current recommendation: ${report.recommendation?.next || "missing"}

${singleSession && activePages > 0 ? `Warning: the live Selkies baseline is single-session and currently has ${activePages} active page(s). Do not run Selkies bake-offs against this instance until the page is closed, or use a separate provider instance.${pageIds.length > 0 ? ` Active page ids: ${pageIds.map((id) => `\`${id}\``).join(", ")}.` : ""}` : ""}

## Current Host Stop Condition

${currentHostStopConditionBlock(report)}

## Objective Checklist

${objectiveChecklistBlock(report)}

## Blocking Summary

${blockingSummaryBlock(report)}

## Next Action

${nextActionBlock(report)}

## Local Pass Checks

Run these before and after changing Browser provider config. They do not accept
the product browser by themselves; they prove the ABI and no-fallback guardrails
still hold.

${shellBlock([
  "scripts/browser-hosted-product-config-smoke.sh",
  "scripts/browser-objective-audit-smoke.sh",
  "scripts/browser-provider-decision-report-smoke.sh",
  "scripts/browser-provider-runbook-smoke.sh",
  "node scripts/browser-display-mode-smoke.mjs",
  "node scripts/home-entropy-check.mjs",
  "scripts/check-wci-alignment.sh",
])}

## Expected-Failing Completion Audit

Run this to confirm the remaining Browser/audio blockers are still explicit.
It should exit non-zero until product audio evidence and hash-bound manual UX
evidence are provided.

${shellBlock([
  "node scripts/browser-objective-audit.mjs",
])}

## Kasm Workspaces Path

Run Kasm Workspaces first for the hosted product comparison unless a higher
priority accepted provider proof already exists.

Current blockers:

${blockerLine(report, "kasm-workspaces")}

Operator steps:

${shellBlock([
  "# Provision Kasm Workspaces outside the Runtime tree.",
  "# Create a Developer API key with the permissions required to create and inspect sessions.",
  "# Keep these values in operator secret storage; do not commit them.",
  "# Enable Kasm audio in the session/casting policy; the ElastOS gate must still prove decoded audio.",
  "export KASM_BASE_URL=https://kasm.example.invalid",
  "export KASM_API_KEY=<operator-api-key>",
  "export KASM_API_KEY_SECRET=<operator-api-key-secret>",
  "",
  "# Start or point to an operator control service that calls request_kasm, waits",
  "# for get_kasm_status=running, and exposes only an ElastOS product_compositor receipt.",
  "# The bundled control service owns Kasm lifecycle but still requires a separate",
  "# product display bridge; it rejects URL-only Kasm sessions before calling Kasm.",
  "export KASM_CONTROL_SOCKET=/run/elastos/kasm-workspaces-control.sock",
  "export KASM_PRODUCT_DISPLAY_BRIDGE_SOCKET=/run/elastos/kasm-display-bridge.sock",
  "",
  "ELASTOS_BROWSER_KASM_CONTROL_CONFIG='{\"schema\":\"elastos.browser.kasm-control.config/v1\",\"control_socket_path\":\"'\"$KASM_CONTROL_SOCKET\"'\",\"kasm_base_url\":\"'\"$KASM_BASE_URL\"'\",\"api_key\":\"'\"$KASM_API_KEY\"'\",\"api_key_secret\":\"'\"$KASM_API_KEY_SECRET\"'\",\"user_id\":\"<kasm-user-id>\",\"image_id\":\"<kasm-image-id>\",\"product_display_bridge_socket\":\"'\"$KASM_PRODUCT_DISPLAY_BRIDGE_SOCKET\"'\"}' \\",
  "  node scripts/browser-kasm-control-service.mjs",
  "",
  "node scripts/browser-hosted-product-operator-config.mjs \\",
  "  --candidate kasm-workspaces \\",
  "  --out-dir /opt/elastos/kasm-workspaces \\",
  "  --supervisor-program \"$PWD/scripts/browser-hosted-product-supervisor.mjs\" \\",
  "  --control-socket \"$KASM_CONTROL_SOCKET\"",
  "",
  "node scripts/browser-hosted-provider-preflight.mjs \\",
  "  --candidate kasm-workspaces \\",
  "  --adapter-config /opt/elastos/kasm-workspaces/browser-engine-adapter.json",
  "",
  "scripts/browser-hosted-provider-bakeoff.sh \\",
  "  --candidate kasm-workspaces \\",
  "  --adapter-config /opt/elastos/kasm-workspaces/browser-engine-adapter.json \\",
  "  --cdp-endpoint http://127.0.0.1:<private-cdp-port> \\",
  "  --resize-width 1000 \\",
  "  --resize-height 700 \\",
  "  --artifact-out /opt/elastos/kasm-workspaces/hosted-bakeoff.json",
])}

## BrowserBox Path

Current blockers:

${blockerLine(report, "browserbox")}

Operator steps:

${shellBlock([
  "# Install/license BrowserBox outside the Runtime tree using official BrowserBox instructions.",
  "# Do not commit product keys or generated secrets.",
  "bbx install",
  "bbx certify <BROWSERBOX_PRODUCT_KEY>",
  "bbx setup",
  "bbx run",
  "",
  "# After licensing is confirmed by the operator:",
  "export BROWSERBOX_LICENSE_CONFIRMED=1",
  "",
  "# Start or point to an operator control service that adapts BrowserBox to",
  "# elastos.browser.engine.supervisor-result/v1 over this Unix socket:",
  "export BROWSERBOX_CONTROL_SOCKET=/run/elastos/browserbox-control.sock",
  "",
  "node scripts/browser-hosted-product-operator-config.mjs \\",
  "  --candidate browserbox \\",
  "  --out-dir /opt/elastos/browserbox \\",
  "  --supervisor-program \"$PWD/scripts/browser-hosted-product-supervisor.mjs\" \\",
  "  --control-socket \"$BROWSERBOX_CONTROL_SOCKET\"",
  "",
  "node scripts/browser-hosted-provider-preflight.mjs \\",
  "  --candidate browserbox \\",
  "  --adapter-config /opt/elastos/browserbox/browser-engine-adapter.json",
  "",
  "scripts/browser-hosted-provider-bakeoff.sh \\",
  "  --candidate browserbox \\",
  "  --adapter-config /opt/elastos/browserbox/browser-engine-adapter.json \\",
  "  --cdp-endpoint http://127.0.0.1:<private-cdp-port> \\",
  "  --resize-width 1000 \\",
  "  --resize-height 700 \\",
  "  --artifact-out /opt/elastos/browserbox/hosted-bakeoff.json",
])}

## Native Product Path

Use this when testing local launcher, Jetson, or mobile-style hosts where native
compositor/audio is the product-performance path. The command is accepted only
when the target proof reports \`native_audio_proven=true\` and
\`native_video_proven=true\`; declaration-only config is not enough.

Current native host blockers:

${nativeHostBlockerLine(report)}

${shellBlock([
  "node scripts/browser-native-host-capability.mjs \\",
  "  --browser-program <chromium-or-cef> \\",
  "  --require-product-native",
  "",
  "scripts/browser-native-target-preflight.sh \\",
  "  --out-dir /opt/elastos/native-browser \\",
  "  --browser-program <chromium-or-cef> \\",
  "  --native-audio \\",
  "  --native-video \\",
  "  --require-native-media \\",
  "  --artifact-out /opt/elastos/native-browser/native-preflight.json",
])}

## Completion Gate

After a hosted or native product path passes, record manual UX evidence and run
the objective audit for the same accepted artifact. Do not pass placeholder
paths for the path you did not prove.

${shellBlock([
  "node scripts/browser-manual-ux-report.mjs \\",
  "  --template \\",
  "  --machine-artifact <accepted-hosted-or-native-proof.json> \\",
  "  > manual-ux.json",
  "# Edit manual-ux.json only after testing typing, scrolling, resize/page-scale, hosted WebRTC audio unlock",
  "# where applicable, including advertised audio, user-gesture unlock, unmuted/remote-audio",
  "# status, received-audio evidence, YouTube audible audio, Glide wallet connect,",
  "# no raw authority, and cleanup.",
  "node scripts/browser-manual-ux-report.mjs --input manual-ux.json",
  "",
  "# Hosted provider completion:",
  "node scripts/browser-objective-audit.mjs \\",
  "  --hosted-bakeoff <accepted-hosted-bakeoff.json> \\",
  "  --manual-ux manual-ux.json",
  "",
  "# Native provider completion:",
  "node scripts/browser-objective-audit.mjs \\",
  "  --native-preflight <accepted-native-preflight.json> \\",
  "  --manual-ux manual-ux.json",
])}
`;
}

function main() {
  const args = parseArgs(process.argv.slice(2));
  const report = loadDecisionReport(args);
  if (report?.schema !== "elastos.browser.provider-decision-report/v1") {
    throw new Error("decision report must use schema elastos.browser.provider-decision-report/v1");
  }
  process.stdout.write(markdown(report));
}

try {
  main();
} catch (error) {
  console.error(error instanceof Error ? error.message : String(error));
  usage();
  process.exit(1);
}
