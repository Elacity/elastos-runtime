#!/usr/bin/env node
import fs from "node:fs";
import process from "node:process";

import { templateManualChecksForSchema } from "./browser-manual-ux-checks.mjs";
import { MACHINE_ARTIFACT_SCHEMAS, sha256File, validateManualUxReport } from "./browser-manual-ux-validation.mjs";

function usage() {
  console.error(`Usage:
  node scripts/browser-manual-ux-report.mjs --template
  node scripts/browser-manual-ux-report.mjs --template --machine-artifact /path/to/accepted-proof.json
  node scripts/browser-manual-ux-report.mjs --input /path/to/manual-ux.json

Creates or validates the manual UX evidence consumed by Browser completion
gates. This is not a test substitute; it records the human review gate after a
real hosted provider, native provider, or Mac Browser VM proof is tested.
`);
}

function parseArgs(argv) {
  const args = {
    template: false,
    input: "",
    machineArtifact: "",
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
    } else if (arg === "--template") {
      args.template = true;
    } else if (arg === "--input") {
      args.input = next();
    } else if (arg === "--machine-artifact") {
      args.machineArtifact = next();
    } else {
      throw new Error(`unknown option: ${arg}`);
    }
  }
  if (args.template === Boolean(args.input)) {
    throw new Error("use exactly one of --template or --input");
  }
  if (args.machineArtifact && !args.template) {
    throw new Error("--machine-artifact is only valid with --template");
  }
  return args;
}

function machineArtifactReference(file) {
  if (!file) {
    return {
      machineArtifact: {
        schema: "",
        sha256: "",
        path: "",
      },
      provider: "",
      target: "",
    };
  }
  const artifact = JSON.parse(fs.readFileSync(file, "utf8"));
  if (!MACHINE_ARTIFACT_SCHEMAS.includes(artifact.schema)) {
    throw new Error("--machine-artifact must be an accepted hosted bake-off, native preflight, or Mac VM proof artifact");
  }
  const provider =
    artifact.schema === "elastos.browser.hosted-provider-bakeoff/v1"
      ? String(artifact.candidate || artifact.candidate_gate?.result?.display_backend || "")
      : artifact.schema === "elastos.browser.mac-vm-proof/v1"
        ? "mac-vm"
      : String(artifact.browser_program || "");
  const target =
    artifact.schema === "elastos.browser.hosted-provider-bakeoff/v1"
      ? String(artifact.youtube_stress?.result?.display_backend || artifact.candidate_gate?.result?.display_backend || artifact.candidate || "")
      : artifact.schema === "elastos.browser.mac-vm-proof/v1"
        ? String(artifact.target || artifact.home?.url || "")
      : String(artifact.out_dir || artifact.browser_program || "");
  return {
    machineArtifact: {
      schema: artifact.schema,
      sha256: sha256File(file),
      path: file,
    },
    provider,
    target,
  };
}

function template(reference) {
  return {
    schema: "elastos.browser.manual-ux/v1",
    ok: false,
    reviewed_at: new Date(0).toISOString(),
    reviewer: "",
    provider: reference.provider,
    target: reference.target,
    machine_artifact: reference.machineArtifact,
    checks: Object.fromEntries(templateManualChecksForSchema(reference.machineArtifact.schema).map((name) => [name, false])),
    evidence: Object.fromEntries(templateManualChecksForSchema(reference.machineArtifact.schema).map((name) => [name, ""])),
    review_artifacts: [],
    notes: [
      "Set ok=true only after every check is true on a real hosted, native, or Mac VM Browser provider.",
      "For hosted WebRTC providers, fill evidence.display_session_audio_advertised, evidence.audio_unlock_gesture, evidence.remote_audio_unmuted_status, and evidence.received_audio_evidence separately from YouTube audible playback.",
      "For Mac VM acceptance, add at least one hash-bound review_artifacts entry with kind=screen_recording and redacted=true for the reviewed Mac VM pass.",
      "machine_artifact.sha256 must be the SHA-256 of the accepted hosted bake-off, native preflight, or Mac VM proof JSON you reviewed.",
      "reviewed_at must be at or after machine_artifact.generated_at when the machine artifact records a generation timestamp.",
      "Do not use this file to bypass machine gates.",
    ],
  };
}

function main() {
  const args = parseArgs(process.argv.slice(2));
  if (args.template) {
    console.log(JSON.stringify(template(machineArtifactReference(args.machineArtifact)), null, 2));
    return;
  }
  const report = JSON.parse(fs.readFileSync(args.input, "utf8"));
  const result = validateManualUxReport(report);
  console.log(JSON.stringify(result, null, 2));
  if (!result.ok) {
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
