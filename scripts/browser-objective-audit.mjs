#!/usr/bin/env node
import fs from "node:fs";
import process from "node:process";

import {
  sha256File,
  validateManualUxReport,
} from "./browser-manual-ux-validation.mjs";

const repoRoot = new URL("../", import.meta.url);

function usage() {
  console.error(`Usage:
  node scripts/browser-objective-audit.mjs \\
    [--hosted-bakeoff /path/to/accepted-hosted-bakeoff.json | --native-preflight /path/to/accepted-native-preflight.json] \\
    [--manual-ux /path/to/manual-ux.json]

Audits the Browser objective against real artifacts. This is intentionally
fail-closed: architecture and smokes can pass while the product objective still
fails if audio/media/manual UX proof is missing.
Pass only the accepted product artifact for the path actually proven, plus the
matching hash-bound manual UX report. Do not pass placeholder paths for an
unproven hosted or native path.

Optional manual UX evidence format:
{
  "schema": "elastos.browser.manual-ux/v1",
  "ok": true,
  "checks": {
    "typing_latency": true,
    "address_bar_stability": true,
    "scrolling_click_fidelity": true,
    "youtube_audible_audio": true,
    "glide_wallet_connect": true,
    "no_raw_authority": true,
    "session_cleanup": true,
    "display_session_audio_advertised": true,
    "audio_unlock_gesture": true,
    "remote_audio_unmuted_status": true,
    "received_audio_evidence": true
  }
}
`);
}

function parseArgs(argv) {
  const args = {
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

function readRepo(path) {
  return fs.readFileSync(new URL(path, repoRoot), "utf8");
}

function readJsonFile(path) {
  if (!path) return null;
  return JSON.parse(fs.readFileSync(path, "utf8"));
}

function exists(path) {
  try {
    return fs.statSync(new URL(path, repoRoot)).isFile();
  } catch {
    return false;
  }
}

function criterion(id, description, ok, evidence, missing = "") {
  return {
    id,
    description,
    ok: Boolean(ok),
    evidence,
    missing: ok ? null : missing,
  };
}

function acceptedMachineArtifacts({ hostedBakeoff, nativePreflight, hostedBakeoffPath, nativePreflightPath }) {
  const artifacts = [];
  if (hostedAccepted(hostedBakeoff) && hostedBakeoffPath) {
    artifacts.push({
      schema: "elastos.browser.hosted-provider-bakeoff/v1",
      sha256: sha256File(hostedBakeoffPath),
      path: hostedBakeoffPath,
    });
  }
  if (nativeMediaAccepted(nativePreflight) && nativePreflightPath) {
    artifacts.push({
      schema: "elastos.browser.native-target-preflight/v1",
      sha256: sha256File(nativePreflightPath),
      path: nativePreflightPath,
    });
  }
  return artifacts;
}

function manualUxAccepted(manual, acceptedArtifacts = []) {
  return validateManualUxReport(manual, {
    acceptedArtifacts,
    requireAcceptedArtifact: true,
  }).ok;
}

function qualityGateAccepted(qualityGate) {
  return (
    qualityGate &&
    Number(qualityGate.decoded_frames || 0) > 0 &&
    Number(qualityGate.min_video_width || 0) >= 640 &&
    Number(qualityGate.min_video_height || 0) >= 360 &&
    Number(qualityGate.max_video_drop_ratio || 1) <= 0.25 &&
    Number(qualityGate.drop_ratio || 0) <= Number(qualityGate.max_video_drop_ratio || 0)
  );
}

function resizeGateAccepted(resizeGate) {
  return (
    resizeGate &&
    Number(resizeGate.requested_width || 0) >= 320 &&
    Number(resizeGate.requested_height || 0) >= 240 &&
    Math.abs(Number(resizeGate.css_width || 0) - Number(resizeGate.requested_width || 0)) <= 2 &&
    Math.abs(Number(resizeGate.css_height || 0) - Number(resizeGate.requested_height || 0)) <= 2 &&
    Number(resizeGate.video_width || 0) >= Number(resizeGate.css_width || 0) &&
    Number(resizeGate.video_height || 0) >= Number(resizeGate.css_height || 0)
  );
}

function hostedAccepted(bakeoff) {
  const candidate = bakeoff?.candidate_gate?.result;
  const youtube = bakeoff?.youtube_stress?.result;
  return (
    bakeoff?.schema === "elastos.browser.hosted-provider-bakeoff/v1" &&
    bakeoff.ok === true &&
    bakeoff.manual_ux_required === true &&
    bakeoff.candidate_gate?.ok === true &&
    bakeoff.candidate_gate?.status === 0 &&
    candidate?.schema === "elastos.browser.hosted-provider-candidate-smoke/v1" &&
    candidate.backend_class === "product_compositor" &&
    typeof candidate.display_backend === "string" &&
    candidate.display_backend.length > 0 &&
    candidate.audio_track === true &&
    candidate.video_track === true &&
    candidate.datachannel_input === true &&
    candidate.direct_network === false &&
    Number(candidate.held_ms || 0) >= 5000 &&
    qualityGateAccepted(candidate.quality_gate) &&
    resizeGateAccepted(candidate.resize_gate) &&
    candidate.navigation?.can_go_back_after_navigate === true &&
    candidate.navigation?.can_go_forward_after_back === true &&
    candidate.wallet_bridge &&
    typeof candidate.glide_connected_account === "string" &&
    candidate.glide_connected_account.length > 0 &&
    bakeoff.youtube_stress?.skipped !== true &&
    bakeoff.youtube_stress?.ok === true &&
    bakeoff.youtube_stress?.status === 0 &&
    youtube?.schema === "elastos.browser.hosted-product-webrtc-smoke/v1" &&
    youtube.backend_class === "product_compositor" &&
    youtube.audio_track === true &&
    youtube.video_track === true &&
    youtube.datachannel_input === true &&
    youtube.direct_network === false &&
    Number(youtube.held_ms || 0) >= 5000 &&
    qualityGateAccepted(youtube.quality_gate) &&
    resizeGateAccepted(youtube.resize_gate) &&
    Number(youtube.media?.audio_decoded_delta || 0) > 0 &&
    Number(youtube.media?.video_decoded_delta || 0) > 0
  );
}

function nativeMediaAccepted(preflight) {
  return (
    preflight?.schema === "elastos.browser.native-target-preflight/v1" &&
    preflight?.ok === true &&
    preflight.native_audio_declared === true &&
    preflight.native_video_declared === true &&
    preflight.native_audio_proven === true &&
    preflight.native_video_proven === true &&
    preflight.native_media_required === true &&
    preflight.direct_network === false &&
    preflight.network_mode === "runtime_net_only"
  );
}

function nextActions({ hostedBakeoff, nativePreflight, manualUx, acceptedArtifacts }) {
  const actions = [
    {
      id: "consult_provider_decision_report",
      purpose: "Use the live provider decision next_action before running hosted bake-offs or native media preflights.",
      commands: [
        "node scripts/browser-provider-decision-report.mjs",
        "node scripts/browser-provider-runbook.mjs",
      ],
    },
  ];
  if (!hostedAccepted(hostedBakeoff)) {
    actions.push({
      id: "run_hosted_provider_bakeoff",
      purpose: "Compare hosted product providers without adding another Browser ABI.",
      commands: [
        "node scripts/browser-hosted-product-operator-config.mjs --candidate kasm-workspaces --out-dir <dir> --supervisor-program <bridge> --control-socket <kasm-control.sock>",
        "node scripts/browser-hosted-provider-preflight.mjs --candidate kasm-workspaces --adapter-config <dir>/browser-engine-adapter.json",
        "scripts/browser-hosted-provider-bakeoff.sh --candidate kasm-workspaces --adapter-config <dir>/browser-engine-adapter.json --cdp-endpoint http://127.0.0.1:<private-cdp-port> --resize-width 1000 --resize-height 700 --artifact-out <dir>/hosted-bakeoff.json",
        "repeat with --candidate browserbox if a BrowserBox license/control socket is available, or --candidate kasmvnc if Workspaces is unavailable",
      ],
    });
  }
  if (!nativeMediaAccepted(nativePreflight)) {
    actions.push({
      id: "prove_native_product_media",
      purpose: "Prove the product-performance path on a real native target instead of treating Docker/Selkies as final.",
      commands: [
        "scripts/browser-native-target-preflight.sh --out-dir <dir> --browser-program <chromium-or-cef> --native-audio --native-video --require-native-media --artifact-out <dir>/native-preflight.json",
      ],
    });
  }
  if (!manualUxAccepted(manualUx, acceptedArtifacts)) {
    actions.push({
      id: "record_manual_ux_evidence",
      purpose: "Confirm browser-like UX and audible audio from the provider a user actually sees.",
      commands: [
        "node scripts/browser-manual-ux-report.mjs --template --machine-artifact <accepted-hosted-or-native-proof.json> > manual-ux.json",
        "edit manual-ux.json only after testing typing, scrolling, resize/page-scale, hosted WebRTC audio unlock evidence where applicable (advertised audio, user-gesture unlock, unmuted/remote-audio status, received-audio evidence), YouTube audible audio, Glide wallet connect, no raw authority, cleanup, and machine_artifact.sha256",
        "node scripts/browser-manual-ux-report.mjs --input manual-ux.json",
      ],
    });
  }
  return actions;
}

function checklistItem(id, requirement, ok, evidence, missing = "") {
  return {
    id,
    requirement,
    ok: Boolean(ok),
    evidence,
    missing: ok ? null : missing,
  };
}

function criterionOk(criteria, id) {
  return criteria.find((item) => item.id === id)?.ok === true;
}

function promptToArtifactChecklist({ criteria, hostedBakeoff, nativePreflight, manualUx, acceptedArtifacts }) {
  const architectureOk =
    criterionOk(criteria, "browser_abi_single_source") &&
    criterionOk(criteria, "selkies_is_baseline_not_product") &&
    criterionOk(criteria, "native_product_path_defined") &&
    criterionOk(criteria, "hosted_bakeoff_defined") &&
    criterionOk(criteria, "provider_decision_next_action_defined") &&
    criterionOk(criteria, "current_host_stop_condition_defined");
  const productProofOk = hostedAccepted(hostedBakeoff) || nativeMediaAccepted(nativePreflight);
  const manualOk = manualUxAccepted(manualUx, acceptedArtifacts);
  return [
    checklistItem(
      "best_path_determined",
      "Determine the best Browser path instead of looping on Docker/Selkies.",
      architectureOk,
      [
        "docs/BROWSER_PROVIDER_BAKEOFF.md",
        "ROADMAP.md",
        "TASKS.md",
        "scripts/browser-provider-decision-report.mjs",
        "scripts/browser-provider-runbook.mjs",
      ],
      "Keep Browser/Net/Exit as the ABI, native/local as the performance path, Kasm/BrowserBox/Selkies behind one hosted-provider bake-off, and a structured provider decision next_action as the machine-readable driver.",
    ),
    checklistItem(
      "audio_product_proven",
      "Enable and prove working Browser audio in an accepted product provider.",
      productProofOk,
      [
        "scripts/browser-hosted-provider-bakeoff.sh",
        "scripts/browser-native-target-preflight.sh",
      ],
      "Provide either an accepted hosted bake-off written with --artifact-out that proves product_compositor audio/video plus non-skipped YouTube audio decode, or an accepted native preflight written with --artifact-out and native_audio_proven=true plus native_video_proven=true.",
    ),
    checklistItem(
      "no_fake_fallbacks",
      "Prevent diagnostic frames, proof surfaces, or URL-only hosted sessions from masquerading as the product browser.",
      criterionOk(criteria, "native_media_not_faked") && criterionOk(criteria, "native_media_preflight_gate"),
      [
        "capsules/browser/browser/browser.js",
        "capsules/browser/browser/browser-input-surface.js",
        "capsules/browser/browser/browser-remote-display.js",
        "scripts/browser-display-mode-smoke.mjs",
        "scripts/browser-entropy-check.mjs",
        "docs/BROWSER_CAPSULE.md",
        "scripts/browser-kasm-control-service.mjs",
      ],
      "Keep diagnostic_frame debug-only, proof-surface audio rejected, native media false by default, and Kasm URL-only sessions rejected.",
    ),
    checklistItem(
      "planned_and_iterated",
      "Keep the plan and implementation gates current after each Browser iteration.",
      architectureOk && criterionOk(criteria, "native_media_preflight_gate"),
      [
        "TASKS.md",
        "ROADMAP.md",
        "docs/BROWSER_PROVIDER_BAKEOFF.md",
        "scripts/browser-provider-runbook.mjs",
        "scripts/browser-objective-audit.mjs",
      ],
      "Update plan artifacts and objective gates before claiming completion.",
    ),
    checklistItem(
      "manual_user_acceptance",
      "Confirm the browser UX manually against the artifact the user actually sees.",
      manualOk,
      ["scripts/browser-manual-ux-report.mjs"],
      "Record hash-bound manual UX evidence for typing, address-bar stability, scrolling/click fidelity, resize/page-scale, hosted WebRTC audio unlock evidence where applicable (advertised audio, user-gesture unlock, unmuted/remote-audio status, received-audio evidence), audible YouTube audio, Glide wallet connect, no raw authority, and cleanup.",
    ),
  ];
}

function main() {
  const args = parseArgs(process.argv.slice(2));
  const tasks = readRepo("TASKS.md");
  const roadmap = readRepo("ROADMAP.md");
  const browserDocs = readRepo("docs/BROWSER_CAPSULE.md");
  const bakeoffDocs = readRepo("docs/BROWSER_PROVIDER_BAKEOFF.md");
  const planningSurface = [tasks, roadmap, browserDocs, bakeoffDocs].join("\n");
  const supervisor = readRepo("elastos/tools/browser-engine-supervisor/src/main.rs");
  const nativeConfig = readRepo("scripts/browser-native-operator-config.mjs");
  const nativePreflightScript = readRepo("scripts/browser-native-target-preflight.sh");
  const decisionReport = readRepo("scripts/browser-provider-decision-report.mjs");
  const decisionReportSmoke = readRepo("scripts/browser-provider-decision-report-smoke.sh");
  const runbook = readRepo("scripts/browser-provider-runbook.mjs");
  const entropy = readRepo("scripts/browser-entropy-check.mjs");
  const browserUi = [
    readRepo("capsules/browser/browser/browser.js"),
    readRepo("capsules/browser/browser/browser-input-surface.js"),
    readRepo("capsules/browser/browser/browser-remote-display.js"),
  ].join("\n");
  const displayModeSmoke = readRepo("scripts/browser-display-mode-smoke.mjs");

  const hostedBakeoff = readJsonFile(args.hostedBakeoff);
  const nativePreflight = readJsonFile(args.nativePreflight);
  const manualUx = readJsonFile(args.manualUx);
  const acceptedArtifacts = acceptedMachineArtifacts({
    hostedBakeoff,
    nativePreflight,
    hostedBakeoffPath: args.hostedBakeoff,
    nativePreflightPath: args.nativePreflight,
  });

  const criteria = [
    criterion(
      "browser_abi_single_source",
      "Browser work uses one Browser/Net/Exit ABI instead of one-off host iframes or fallback display paths.",
      browserDocs.includes("Browser UI capsule") &&
        bakeoffDocs.includes("Runtime Browser open route") &&
        bakeoffDocs.includes("No candidate gets a new Browser ABI") &&
        tasks.includes("one Browser/Net/Exit ABI"),
      ["docs/BROWSER_CAPSULE.md", "docs/BROWSER_PROVIDER_BAKEOFF.md", "TASKS.md"],
      "Document the one Browser/Net/Exit ABI and reject candidate-specific Browser ABIs.",
    ),
    criterion(
      "selkies_is_baseline_not_product",
      "Selkies/Docker is treated as backend packaging and baseline proof, not the final browser answer.",
      bakeoffDocs.includes("Selkies remains the self-hosted baseline/proof") &&
        roadmap.includes("not the final product") &&
        (planningSurface.includes("Selkies is the current self-hosted baseline") ||
          planningSurface.includes("Selkies as the current self-hosted baseline")) &&
        planningSurface.includes("not the acceptance answer"),
      ["docs/BROWSER_PROVIDER_BAKEOFF.md", "ROADMAP.md", "TASKS.md"],
      "Make Selkies baseline-only language explicit in docs/tasks.",
    ),
    criterion(
      "native_product_path_defined",
      "Native/local adapter is the product-performance path for low latency and real audio/video surfaces.",
      bakeoffDocs.includes("performance path is native/local first") &&
        roadmap.includes("native/local browser adapters for lowest-latency") &&
        planningSurface.includes("native Chromium/CEF-style adapter"),
      ["docs/BROWSER_PROVIDER_BAKEOFF.md", "ROADMAP.md", "TASKS.md"],
      "Record native/local browser adapter as the product-performance path.",
    ),
    criterion(
      "hosted_bakeoff_defined",
      "Kasm/BrowserBox/Selkies candidates have a common hosted-provider gate.",
      exists("scripts/browser-hosted-provider-preflight.mjs") &&
        exists("scripts/browser-hosted-provider-bakeoff.sh") &&
        bakeoffDocs.includes("BrowserBox") &&
        bakeoffDocs.includes("Kasm Workspaces"),
      [
        "scripts/browser-hosted-provider-preflight.mjs",
        "scripts/browser-hosted-provider-bakeoff.sh",
        "docs/BROWSER_PROVIDER_BAKEOFF.md",
      ],
      "Add preflight/bake-off scripts and candidate decision docs.",
    ),
    criterion(
      "provider_decision_next_action_defined",
      "The Browser provider decision gate emits a structured next action instead of relying on prose or ad hoc operator memory.",
      decisionReport.includes("function nextAction(") &&
        decisionReport.includes("next_action: nextAction") &&
        decisionReport.includes("free_or_isolate_selkies_before_bakeoff") &&
        decisionReport.includes("run_native_product_preflight") &&
        decisionReport.includes("run_hosted_provider_bakeoff") &&
        decisionReport.includes("provision_kasm_workspaces_first") &&
        runbook.includes("## Next Action") &&
        runbook.includes("nextActionBlock") &&
        planningSurface.includes("structured `next_action`") &&
        bakeoffDocs.includes("structured `next_action`"),
      [
        "scripts/browser-provider-decision-report.mjs",
        "scripts/browser-provider-runbook.mjs",
        "TASKS.md",
        "docs/BROWSER_PROVIDER_BAKEOFF.md",
      ],
      "Make the decision report emit a structured next_action and render it in the runbook before candidate-specific commands.",
    ),
    criterion(
      "current_host_stop_condition_defined",
      "Current-host blockers stop local Browser provider tuning and route product proof to an operator-owned hosted or native target.",
      decisionReport.includes("free_or_isolate_selkies_before_bakeoff") &&
        decisionReport.includes('owner: "operator"') &&
        decisionReport.includes("separate provider instance") &&
        decisionReport.includes("provision_kasm_workspaces_first") &&
        decisionReportSmoke.includes("busy_selkies_next_action_exercised") &&
        decisionReportSmoke.includes("must not recommend more Selkies tuning") &&
        planningSurface.includes("Freeze new Browser provider implementation") &&
        planningSurface.includes("do not spend more branch time tuning Selkies as the product path") &&
        roadmap.includes("Browser work should stop") &&
        roadmap.includes("contract/gate layer"),
      [
        "scripts/browser-provider-decision-report.mjs",
        "scripts/browser-provider-decision-report-smoke.sh",
        "TASKS.md",
        "ROADMAP.md",
      ],
      "Make this host's stop condition explicit: do not keep tuning the running Selkies baseline when product proof requires an operator-owned hosted candidate or a native target with compositor/audio/network isolation.",
    ),
    criterion(
      "native_media_not_faked",
      "Native surface audio/video are false by default and require explicit operator capability declaration.",
      supervisor.includes("display_capabilities: DisplayCapabilities") &&
        supervisor.includes("config.display_capabilities.audio") &&
        nativeConfig.includes("nativeAudio: false") &&
        nativeConfig.includes("nativeVideo: false") &&
        nativeConfig.includes("--native-audio") &&
        nativeConfig.includes("--native-video") &&
        entropy.includes("Native Browser namespace/proxy smokes must not pretend fake browser processes prove native audio or video"),
      [
        "elastos/tools/browser-engine-supervisor/src/main.rs",
        "scripts/browser-native-operator-config.mjs",
        "scripts/browser-entropy-check.mjs",
      ],
      "Default native media off and enforce this in entropy checks.",
    ),
    criterion(
      "native_media_preflight_gate",
      "Native target preflight has an explicit product media readiness gate.",
      nativePreflightScript.includes("--require-native-media requires both --native-audio and --native-video") &&
        nativePreflightScript.includes("native media readiness requires display_capabilities audio=true and video=true") &&
        nativePreflightScript.includes("native media readiness requires the target proof to report native_audio_proven=true and native_video_proven=true") &&
        nativePreflightScript.includes("native_media_required"),
      ["scripts/browser-native-target-preflight.sh"],
      "Add --require-native-media and require actual native_audio_proven/native_video_proven output fields.",
    ),
    criterion(
      "browser_ui_audio_unlock_path",
      "Browser UI requests advertised WebRTC audio and can unlock it from an explicit user gesture.",
      browserUi.includes("const expectsAudio = displaySession.audio === true") &&
        browserUi.includes('nextPeerConnection.addTransceiver("audio", { direction: "recvonly" })') &&
        browserUi.includes("remoteVideo.volume = 1") &&
        browserUi.includes('renderPanel.addEventListener("pointerdown", unlockRemoteAudioFromGesture') &&
        browserUi.includes("Remote display ready. Click the page to enable audio.") &&
        browserUi.includes("Remote audio enabled.") &&
        displayModeSmoke.includes("remote display must unlock audio from a direct pointer gesture") &&
        displayModeSmoke.includes("remote display must reset audible volume"),
      [
        "capsules/browser/browser/browser.js",
        "capsules/browser/browser/browser-input-surface.js",
        "capsules/browser/browser/browser-remote-display.js",
        "scripts/browser-display-mode-smoke.mjs",
      ],
      "Wire Browser UI WebRTC audio receive, muted autoplay startup, volume reset, explicit user-gesture unlock, and status feedback before relying on product media proof.",
    ),
    criterion(
      "hosted_provider_product_accepted",
      "A hosted provider has passed the product-compositor machine gate including YouTube/audio stress.",
      hostedAccepted(hostedBakeoff),
      args.hostedBakeoff ? [args.hostedBakeoff] : [],
      "Run scripts/browser-hosted-provider-bakeoff.sh with --artifact-out for Kasm/BrowserBox/Selkies and provide accepted JSON that proves product_compositor, audio/video tracks, input, quality, dynamic viewport resize, navigation, wallet bridge, Glide connect, direct_network=false, and non-skipped YouTube audio/video stress.",
    ),
    criterion(
      "native_product_media_accepted",
      "A native target has passed media-ready preflight without direct network authority.",
      nativeMediaAccepted(nativePreflight),
      args.nativePreflight ? [args.nativePreflight] : [],
      "Run scripts/browser-native-target-preflight.sh --native-audio --native-video --require-native-media with --artifact-out on a real target host and provide accepted elastos.browser.native-target-preflight/v1 JSON with native_audio_proven=true and native_video_proven=true.",
    ),
    criterion(
      "manual_ux_accepted",
      "Manual UX review confirms browser-like typing, scrolling, resize/page-scale, hosted WebRTC audio unlock evidence where applicable (advertised audio, user-gesture unlock, unmuted/remote-audio status, received-audio evidence), YouTube audible audio, Glide wallet connect, no raw authority, and cleanup.",
      manualUxAccepted(manualUx, acceptedArtifacts),
      args.manualUx ? [args.manualUx] : [],
      "Record manual UX evidence with schema elastos.browser.manual-ux/v1 after real hosted or native provider testing, including resize/page-scale and hosted WebRTC audio-unlock checks when the accepted artifact is a hosted bake-off, plus machine_artifact.schema and machine_artifact.sha256 for the accepted proof artifact.",
    ),
  ];

  const productProviderAccepted =
    (hostedAccepted(hostedBakeoff) || nativeMediaAccepted(nativePreflight)) && manualUxAccepted(manualUx, acceptedArtifacts);
  const requiredNonProductCriteria = [
    "browser_abi_single_source",
    "selkies_is_baseline_not_product",
    "native_product_path_defined",
    "hosted_bakeoff_defined",
    "provider_decision_next_action_defined",
    "current_host_stop_condition_defined",
    "native_media_not_faked",
    "native_media_preflight_gate",
    "browser_ui_audio_unlock_path",
  ];
  const ok = requiredNonProductCriteria.every((id) => criterionOk(criteria, id)) && productProviderAccepted;
  const result = {
    schema: "elastos.browser.objective-audit/v1",
    ok,
    product_provider_accepted: productProviderAccepted,
    summary: ok
      ? "Browser objective has architecture, product media proof, and manual UX evidence."
      : "Browser objective is not complete; architecture is gated, but product media/manual UX proof is missing.",
    objective: {
      source: "thread goal",
      restatement:
        "Determine the best Browser architecture path, implement fail-closed provider gates, enable/prove audio through the chosen product provider, and verify with entropy/alignment/manual UX evidence.",
    },
    prompt_to_artifact_checklist: promptToArtifactChecklist({
      criteria,
      hostedBakeoff,
      nativePreflight,
      manualUx,
      acceptedArtifacts,
    }),
    criteria,
    accepted_machine_artifacts: acceptedArtifacts,
    next_actions: ok ? [] : nextActions({ hostedBakeoff, nativePreflight, manualUx, acceptedArtifacts }),
  };
  console.log(JSON.stringify(result, null, 2));
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
