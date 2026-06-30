#!/usr/bin/env node
import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import process from "node:process";

import { MAC_VM_MANUAL_CHECKS } from "./browser-manual-ux-checks.mjs";
import { sha256File } from "./browser-manual-ux-validation.mjs";

function usage() {
  console.error(`Usage:
  node scripts/browser-mac-vm-manual-review-packet.mjs \\
    --machine-proof /tmp/elastos-browser-mac-vm-proof.json \\
    [--handoff-summary /tmp/elastos-browser-mac-vm-handoff-summary.json] \\
    --out-dir /tmp/elastos-browser-mac-vm-review

Creates a redacted operator checklist plus an ok=false manual UX draft. It does
not satisfy acceptance by itself; a real reviewer must fill the evidence and add
at least one redacted screen recording review artifact.
`);
}

function parseArgs(argv) {
  const args = {
    machineProof: "",
    handoffSummary: "",
    outDir: "",
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
    } else if (arg === "--machine-proof") {
      args.machineProof = next();
    } else if (arg === "--handoff-summary") {
      args.handoffSummary = next();
    } else if (arg === "--out-dir") {
      args.outDir = next();
    } else {
      throw new Error(`unknown option: ${arg}`);
    }
  }
  if (!args.machineProof) {
    throw new Error("--machine-proof is required");
  }
  if (!args.outDir) {
    throw new Error("--out-dir is required");
  }
  return args;
}

function readJson(file) {
  return JSON.parse(fs.readFileSync(file, "utf8"));
}

function sha256Text(text) {
  return crypto.createHash("sha256").update(text).digest("hex");
}

function boolText(value) {
  return value === true ? "yes" : "no";
}

function tableRows(rows) {
  return rows
    .map(([key, value]) => `| ${key} | ${value == null || value === "" ? "n/a" : String(value)} |`)
    .join("\n");
}

function guidanceSteps(summary) {
  const steps = Array.isArray(summary?.next_steps) ? summary.next_steps : [];
  return steps.filter((step) =>
    typeof step === "string" &&
    (
      step.includes("browser-mac-vm-auth-profile-setup.sh") ||
      step.includes("browser-mac-vm-acceptance-handoff.sh") ||
      step.includes("browser-mac-vm-manual-review-packet.mjs") ||
      step.includes("browser-manual-ux-report.mjs") ||
      step.includes("browser-mac-vm-acceptance-audit.mjs")
    )
  );
}

function checklistText({ proof, proofPath, proofSha256, summary, summaryPath, summarySha256 }) {
  const diagnostics = proof.page_diagnostics || {};
  const video = proof.embedded_video_input || {};
  const display = video.display_session || {};
  const quality = proof.quality_gates || {};
  const restart = proof.vm_control?.restart || {};
  const sourceHomeRestart = summary?.source_home_restart || null;
  const remaining = Array.isArray(summary?.remaining_acceptance_gaps)
    ? summary.remaining_acceptance_gaps
    : [];
  const machineFailing = Array.isArray(summary?.machine_failing) ? summary.machine_failing : [];
  const authFailing = Array.isArray(summary?.auth_failing) ? summary.auth_failing : [];
  const manualFailing = Array.isArray(summary?.manual_failing) ? summary.manual_failing : [];
  const steps = guidanceSteps(summary);
  return `# Mac Browser VM Manual Review Checklist

This packet is redacted operator evidence guidance. It is not acceptance until
the manual UX report is filled from a real review and validated.

## Machine Binding

${tableRows([
    ["machine proof", proofPath],
    ["machine proof sha256", proofSha256],
    ["machine proof generated_at", proof.generated_at || ""],
    ["handoff summary", summaryPath || ""],
    ["handoff summary sha256", summarySha256 || ""],
  ])}

## Machine Signals To Confirm Visually

${tableRows([
    ["Runtime-only network", `${proof.vm_control?.after?.network_mode || ""}, direct_network=${boolText(proof.vm_control?.after?.direct_network === true)}`],
    ["VM restart fresh", boolText(restart.fresh_after_restart === true)],
    ["source-home restart freshness", boolText(sourceHomeRestart?.ok === true)],
    ["remote display", `${video.display_mode || ""}, media_transport=${display.media_transport || ""}`],
    ["credentialed TURN servers", display.credentialed_turn_ice_server_count ?? ""],
    ["decoded frame delta", video.decoded_frame_delta ?? ""],
    ["dropped frame delta", video.dropped_frame_delta ?? ""],
    ["performance gates", boolText(quality.performance?.ok === true)],
    ["zoom gates", boolText(quality.zoom?.ok === true)],
    ["ela.city URL after click", video.click_navigation?.status?.actual_url || ""],
    ["ela.city visible images", diagnostics.visible_image_count ?? ""],
    ["ela.city broken/pending images", `${diagnostics.visible_broken_image_count ?? ""}/${diagnostics.visible_pending_image_count ?? ""}`],
    ["profile reset removed disk", boolText(proof.profile_reset?.receipt?.removed_profile_disk === true)],
  ])}

## Manual Checks To Fill

${MAC_VM_MANUAL_CHECKS.map((name) => `- [ ] ${name}: write concrete observed evidence in manual-ux-draft.json`).join("\n")}

## Remaining Acceptance Gaps From Handoff

${remaining.length > 0 ? remaining.map((item) => `- ${item}`).join("\n") : "- none recorded"}

## Gap Groups

${tableRows([
    ["machine failing", machineFailing.length > 0 ? machineFailing.join(", ") : "none"],
    ["authenticated ela.city failing", authFailing.length > 0 ? authFailing.join(", ") : "none"],
    ["manual review failing", manualFailing.length > 0 ? manualFailing.join(", ") : "none"],
  ])}

## Authenticated Setup And Final Audit

${steps.length > 0
    ? steps.map((step) => `- ${step}`).join("\n")
    : "- Re-run browser-mac-vm-acceptance-handoff.sh with --auth-profile and --auth-setup-receipt, then validate the filled manual report and final acceptance audit."}

## Visual Artifact Requirement

Add at least one separate redacted screen recording to manual-ux-draft.json
under review_artifacts. This checklist is included only as a non-visual
redacted checklist artifact, so it cannot satisfy Mac VM review by itself.

## Validation

\`\`\`bash
node scripts/browser-manual-ux-report.mjs --input manual-ux-draft.json
node scripts/browser-mac-vm-acceptance-audit.mjs \\
  --machine-proof ${proofPath} \\
  --manual-ux manual-ux-draft.json \\
  --handoff-summary ${summaryPath || "<handoff-summary.json>"}
\`\`\`
`;
}

function main() {
  const args = parseArgs(process.argv.slice(2));
  const proofPath = path.resolve(args.machineProof);
  const proof = readJson(proofPath);
  if (proof.schema !== "elastos.browser.mac-vm-proof/v1" || proof.ok !== true) {
    throw new Error("--machine-proof must point to a successful elastos.browser.mac-vm-proof/v1 artifact");
  }
  const proofSha256 = sha256File(proofPath);
  let summary = null;
  let summaryPath = "";
  let summarySha256 = "";
  if (args.handoffSummary) {
    summaryPath = path.resolve(args.handoffSummary);
    summary = readJson(summaryPath);
    summarySha256 = sha256File(summaryPath);
    if (summary.schema !== "elastos.browser.mac-vm-acceptance-handoff/v1") {
      throw new Error("--handoff-summary must point to an elastos.browser.mac-vm-acceptance-handoff/v1 artifact");
    }
    if (summary.machine_proof?.sha256 !== proofSha256) {
      throw new Error("--handoff-summary machine_proof.sha256 must match --machine-proof");
    }
  }

  const outDir = path.resolve(args.outDir);
  fs.mkdirSync(outDir, { recursive: true });
  const checklistPath = path.join(outDir, "operator-checklist.md");
  const manualPath = path.join(outDir, "manual-ux-draft.json");
  const packetPath = path.join(outDir, "packet.json");

  const checklist = checklistText({
    proof,
    proofPath,
    proofSha256,
    summary,
    summaryPath,
    summarySha256,
  });
  fs.writeFileSync(checklistPath, checklist);
  const checklistSha256 = sha256Text(checklist);
  const manual = {
    schema: "elastos.browser.manual-ux/v1",
    ok: false,
    reviewed_at: "",
    reviewer: "",
    provider: "mac-vm",
    target: String(proof.target || proof.home?.url || "mac-source-home"),
    machine_artifact: {
      schema: proof.schema,
      sha256: proofSha256,
      path: proofPath,
    },
    checks: Object.fromEntries(MAC_VM_MANUAL_CHECKS.map((name) => [name, false])),
    evidence: Object.fromEntries(MAC_VM_MANUAL_CHECKS.map((name) => [name, ""])),
    review_artifacts: [{
      kind: "checklist",
      description: "Redacted Mac Browser VM manual review checklist; does not replace visual evidence.",
      path: checklistPath,
      sha256: checklistSha256,
      redacted: true,
    }],
    notes: [
      "Set ok=true only after every check is true from real Mac Browser VM review.",
      "Add at least one separate redacted screen_recording review artifact.",
      "This generated checklist is non-visual and intentionally cannot satisfy the screen recording requirement by itself.",
    ],
  };
  fs.writeFileSync(manualPath, `${JSON.stringify(manual, null, 2)}\n`);
  const packet = {
    schema: "elastos.browser.mac-vm-manual-review-packet/v1",
    ok: true,
    generated_at: new Date().toISOString(),
    machine_artifact: manual.machine_artifact,
    handoff_summary: summaryPath ? {
      path: summaryPath,
      sha256: summarySha256,
      acceptance_ready: summary?.acceptance_ready === true,
      remaining_acceptance_gaps: Array.isArray(summary?.remaining_acceptance_gaps)
        ? summary.remaining_acceptance_gaps
        : [],
      machine_failing: Array.isArray(summary?.machine_failing) ? summary.machine_failing : [],
      auth_failing: Array.isArray(summary?.auth_failing) ? summary.auth_failing : [],
      manual_failing: Array.isArray(summary?.manual_failing) ? summary.manual_failing : [],
      source_home_restart_ok: summary?.source_home_restart?.ok === true,
      next_steps: guidanceSteps(summary),
    } : null,
    outputs: {
      manual_ux_draft: {
        path: manualPath,
        sha256: sha256File(manualPath),
      },
      operator_checklist: {
        path: checklistPath,
        sha256: checklistSha256,
        redacted: true,
      },
    },
  };
  fs.writeFileSync(packetPath, `${JSON.stringify(packet, null, 2)}\n`);
  console.log(JSON.stringify(packet, null, 2));
}

try {
  main();
} catch (error) {
  console.error(error instanceof Error ? error.message : String(error));
  usage();
  process.exit(2);
}
