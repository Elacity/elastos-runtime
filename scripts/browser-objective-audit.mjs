#!/usr/bin/env node
import fs from "node:fs";
import process from "node:process";
import { macVmArtifactAccepted, sha256File, validateManualUxReport } from "./browser-manual-ux-validation.mjs";

const root = new URL("../", import.meta.url);
const read = file => fs.readFileSync(new URL(file, root), "utf8");

function parseArgs(argv) {
  const args = {};
  for (let i = 0; i < argv.length; i++) {
    if (argv[i] === "--help" || argv[i] === "-h") {
      console.log("Usage: node scripts/browser-objective-audit.mjs --vm-proof <machine.json> --manual-ux <human.json>\nThe shared VM Engine requires accepted target media and matching hash-bound human evidence. Jetson hardware qualification remains separate.");
      process.exit(0);
    }
    if (!["--vm-proof", "--manual-ux"].includes(argv[i]) || !argv[i + 1] || argv[i + 1].startsWith("--")) {
      throw new Error(`Unsupported or incomplete objective input: ${argv[i]}`);
    }
    if (args[argv[i]]) throw new Error(`Duplicate objective input: ${argv[i]}`);
    const key = argv[i];
    args[key] = argv[++i];
  }
  return args;
}

function main() {
  const args = parseArgs(process.argv.slice(2));
  const docs = read("docs/BROWSER_CAPSULE.md");
  const supervisor = read("scripts/browser-vm-engine-supervisor.mjs");
  const display = read("capsules/browser/browser/browser-remote-display.js");
  const input = read("capsules/browser/browser/browser-input-surface.js");
  const item = (id, description, ok, evidence) => ({ id, description, ok: Boolean(ok), evidence });
  const criteria = [
    item("browser_abi_single_source", "Runtime owns the Browser Engine/Net/Exit contract.",
      docs.includes("Browser UI capsule") && docs.includes("Browser Engine Adapter"), ["docs/BROWSER_CAPSULE.md"]),
    item("single_vm_engine", "Mac and Linux ARM64 use one per-launch Linux guest recipe.",
      docs.includes("One immutable ARM64 image") && docs.includes("per-launch Linux guest") &&
      supervisor.includes("chromium_microvm"), ["docs/BROWSER_CAPSULE.md", "scripts/browser-vm-engine-supervisor.mjs"]),
    item("retired_native_engine_absent", "Host-native namespace execution is retired.",
      !fs.existsSync(new URL("elastos/tools/browser-engine-supervisor", root)), ["elastos/tools"]),
    item("browser_ui_audio_unlock_path", "WebRTC audio is received and unlocked by a user gesture.",
      display.includes('addTransceiver("audio", { direction: "recvonly" })') &&
      display.includes("remoteVideo.volume = 1") && display.includes("unlockAudioFromGesture") && input.includes('renderPanel.addEventListener("pointerdown", unlockRemoteAudioFromGesture'),
      ["capsules/browser/browser/browser-remote-display.js"]),
  ];
  const machine = args["--vm-proof"] ? JSON.parse(fs.readFileSync(args["--vm-proof"], "utf8")) : null;
  const machineAccepted = macVmArtifactAccepted(machine);
  const accepted = machineAccepted ? [{ schema: machine.schema, path: args["--vm-proof"], sha256: sha256File(args["--vm-proof"]) }] : [];
  const manual = args["--manual-ux"] ? JSON.parse(fs.readFileSync(args["--manual-ux"], "utf8")) : null;
  const validation = validateManualUxReport(manual, { acceptedArtifacts: accepted, requireAcceptedArtifact: true });
  criteria.push(item("vm_product_media_accepted", "Target VM evidence proves decoded frames, input, profile and clean lifecycle.", machineAccepted, args["--vm-proof"] ? [args["--vm-proof"]] : []));
  criteria.push(item("manual_ux_accepted", "Human evidence binds the same machine artifact and proves the required media and UX checks.", validation.ok, args["--manual-ux"] ? [args["--manual-ux"]] : []));
  const ok = criteria.every(c => c.ok);
  console.log(JSON.stringify({ schema: "elastos.browser.objective-audit/v1", ok,
    product_provider_accepted: machineAccepted && validation.ok,
    target: machineAccepted ? machine.target : null,
    summary: ok ? "The supplied VM target and matching manual UX evidence pass." : "Target VM proof and matching human UX evidence are required.",
    criteria, accepted_machine_artifacts: accepted, manual_validation: validation,
    next_actions: ok ? [] : [{ id: "prove_published_vm_target", owner: "operator", purpose: "Run the published release on its target, then record matching human evidence.",
      commands: ["node scripts/browser-objective-audit.mjs --vm-proof <machine.json> --manual-ux <human.json>"] }],
  }, null, 2));
  process.exitCode = ok ? 0 : 1;
}

try { main(); } catch (error) {
  console.error(error.message);
  process.exitCode = 2;
}
