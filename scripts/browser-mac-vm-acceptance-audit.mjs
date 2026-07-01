#!/usr/bin/env node
import fs from "node:fs";
import process from "node:process";

import {
  profileResetReceiptLeaksAuthority,
  sha256File,
  validateManualUxReport,
} from "./browser-manual-ux-validation.mjs";

function usage() {
  console.error(`Usage:
  node scripts/browser-mac-vm-acceptance-audit.mjs \\
    --machine-proof /tmp/elastos-browser-mac-vm-proof.json \\
    [--manual-ux /tmp/elastos-browser-mac-vm-manual-ux.json] \\
    [--handoff-summary /tmp/elastos-browser-mac-vm-handoff-summary.json]

Audits Mac Browser VM acceptance evidence. The machine proof can prove VM
networking, video/input, zoom/performance, URL sync, and image settling. The
hash-bound manual UX report is still required for human video/input/performance
acceptance and authenticated ela.city edit-profile behavior. The handoff summary
binds authenticated proof collection to the headed ela.city auth setup receipt.
`);
}

function parseArgs(argv) {
  const args = {
    machineProof: "",
    manualUx: "",
    handoffSummary: "",
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
    } else if (arg === "--manual-ux") {
      args.manualUx = next();
    } else if (arg === "--handoff-summary") {
      args.handoffSummary = next();
    } else {
      throw new Error(`unknown option: ${arg}`);
    }
  }
  if (!args.machineProof) {
    throw new Error("--machine-proof is required");
  }
  return args;
}

function readJson(path) {
  return JSON.parse(fs.readFileSync(path, "utf8"));
}

function number(value, fallback = 0) {
  const parsed = Number(value);
  return Number.isFinite(parsed) ? parsed : fallback;
}

function parseTimestamp(value) {
  if (typeof value !== "string" || value.trim().length === 0) {
    return null;
  }
  const millis = Date.parse(value);
  return Number.isNaN(millis) ? null : millis;
}

function criterion(id, requirement, ok, evidence, missing = "") {
  return {
    id,
    requirement,
    ok: Boolean(ok),
    evidence,
    missing: ok ? null : missing,
  };
}

function rawDiagnosticFields(entry, prefix) {
  const fields = [];
  for (const key of ["body_html", "root_html", "root_outer_html", "cdp_event_samples"]) {
    if (entry && Object.prototype.hasOwnProperty.call(entry, key) && entry[key] != null) {
      if (!Array.isArray(entry[key]) || entry[key].length > 0) {
        fields.push(`${prefix}.${key}`);
      }
    }
  }
  return fields;
}

function manualCheck(manual, name) {
  return manual?.checks?.[name] === true
    && typeof manual?.evidence?.[name] === "string"
    && manual.evidence[name].trim().length > 0;
}

const REQUIRED_PERFORMANCE_CHECKS = [
  "remote_video_ready_ms_within_threshold",
  "embedded_navigation_ms_within_threshold",
  "diagnostics_navigation_ms_within_threshold",
  "decoded_frame_delta_ok",
  "dropped_frame_delta_ok",
];

const REQUIRED_ZOOM_CHECKS = [
  "device_pixel_ratio_ok",
  "viewport_width_ok",
  "viewport_height_ok",
  "panel_aspect_matches_viewport",
  "initial_video_matches_panel",
  "after_navigation_video_matches_panel",
  "source_video_matches_panel",
];

function requiredChecksOk(checks, required) {
  return required.every((name) => checks?.[name] === true);
}

function regexMatches(pattern, value) {
  if (typeof pattern !== "string" || pattern.trim().length === 0) {
    return false;
  }
  try {
    return new RegExp(pattern).test(String(value || ""));
  } catch {
    return false;
  }
}

function macVmManualAccepted(machineProofPath, machineProof, manualUxPath) {
  if (!manualUxPath) {
    return {
      ok: false,
      manual: null,
      validation: {
        ok: false,
        errors: ["--manual-ux is required for Mac VM acceptance"],
      },
    };
  }
  const manual = readJson(manualUxPath);
  const acceptedArtifacts = [{
    schema: "elastos.browser.mac-vm-proof/v1",
    sha256: sha256File(machineProofPath),
    path: machineProofPath,
  }];
  const validation = validateManualUxReport(manual, {
    acceptedArtifacts,
    requireAcceptedArtifact: true,
  });
  return {
    ok: validation.ok === true
      && manual.machine_artifact?.schema === machineProof.schema
      && manual.machine_artifact?.sha256 === acceptedArtifacts[0].sha256,
    manual,
    validation,
  };
}

function macVmHandoffAccepted(machineProofPath, machineProof, handoffSummaryPath) {
  if (!handoffSummaryPath) {
    return {
      ok: false,
      summary: null,
      validation: {
        ok: false,
        errors: ["--handoff-summary is required for authenticated Mac VM acceptance"],
      },
    };
  }
  const summary = readJson(handoffSummaryPath);
  const errors = [];
  const machineSha256 = sha256File(machineProofPath);
  const receipt = summary?.authenticated_profile?.auth_setup_receipt || null;
  const sourceHomeRestart = summary?.source_home_restart || null;
  const proofGeneratedAt = parseTimestamp(machineProof.generated_at);
  const summaryGeneratedAt = parseTimestamp(summary?.generated_at);
  let receiptFile = null;
  let sourceHomeRestartFile = null;
  if (summary?.schema !== "elastos.browser.mac-vm-acceptance-handoff/v1") {
    errors.push("handoff summary schema must be elastos.browser.mac-vm-acceptance-handoff/v1");
  }
  if (summary?.ok !== true) {
    errors.push("handoff summary ok must be true after machine/auth setup handoff passes");
  }
  if (typeof summary?.generated_at !== "string" || summary.generated_at.trim().length === 0) {
    errors.push("handoff summary generated_at must be an ISO timestamp");
  } else if (summaryGeneratedAt == null) {
    errors.push("handoff summary generated_at must be an ISO timestamp");
  } else if (proofGeneratedAt != null && summaryGeneratedAt < proofGeneratedAt) {
    errors.push("handoff summary generated_at must be at or after machine proof generated_at");
  }
  if (summary?.machine_proof?.schema !== machineProof.schema) {
    errors.push("handoff summary machine_proof.schema must match the machine proof");
  }
  if (summary?.machine_proof?.sha256 !== machineSha256) {
    errors.push("handoff summary machine_proof.sha256 must match the reviewed machine proof");
  }
  if (summary?.authenticated_profile?.persistent_virtual_auth_profile !== true) {
    errors.push("handoff summary must show a persistent virtual-auth profile");
  }
  if (sourceHomeRestart?.schema !== "elastos.mac-source-home-restart/v1") {
    errors.push("handoff summary must include a Mac source-home restart receipt schema");
  }
  if (sourceHomeRestart?.ok !== true) {
    errors.push("handoff summary Mac source-home restart receipt must be ok");
  }
  if (!/^[a-f0-9]{64}$/i.test(String(sourceHomeRestart?.sha256 || ""))) {
    errors.push("handoff summary Mac source-home restart receipt must include a SHA-256 digest");
  }
  if (typeof sourceHomeRestart?.path !== "string" || sourceHomeRestart.path.trim().length === 0) {
    errors.push("handoff summary Mac source-home restart receipt must include a path");
  } else if (!fs.existsSync(sourceHomeRestart.path)) {
    errors.push("handoff summary Mac source-home restart receipt path must exist");
  } else if (
    /^[a-f0-9]{64}$/i.test(String(sourceHomeRestart?.sha256 || "")) &&
    sha256File(sourceHomeRestart.path).toLowerCase() !== String(sourceHomeRestart.sha256).toLowerCase()
  ) {
    errors.push("handoff summary Mac source-home restart receipt sha256 must match its path");
  } else {
    try {
      sourceHomeRestartFile = readJson(sourceHomeRestart.path);
    } catch {
      errors.push("handoff summary Mac source-home restart receipt path must be valid JSON");
    }
  }
  if (sourceHomeRestartFile) {
    const restartGeneratedAt = parseTimestamp(sourceHomeRestartFile.generated_at);
    const helperHashes = [
      sourceHomeRestartFile.browser_helper_source_sha256,
      sourceHomeRestartFile.browser_helper_installed_sha256,
      sourceHomeRestartFile.browser_helper_initrd_sha256,
      sourceHomeRestartFile.browser_helper_rootfs_sha256,
    ];
    if (sourceHomeRestartFile.schema !== sourceHomeRestart.schema) {
      errors.push("handoff summary Mac source-home restart receipt schema must match its path");
    }
    if (sourceHomeRestartFile.ok !== true || sourceHomeRestartFile.dry_run !== false) {
      errors.push("handoff summary Mac source-home restart receipt path must be a successful non-dry-run restart");
    }
    if (sourceHomeRestartFile.http_code !== 200) {
      errors.push("handoff summary Mac source-home restart receipt must record Home HTTP 200");
    }
    if (typeof sourceHomeRestartFile.generated_at !== "string" || sourceHomeRestartFile.generated_at.trim().length === 0) {
      errors.push("handoff summary Mac source-home restart receipt generated_at must be an ISO timestamp");
    } else if (restartGeneratedAt == null) {
      errors.push("handoff summary Mac source-home restart receipt generated_at must be an ISO timestamp");
    } else if (proofGeneratedAt != null && restartGeneratedAt > proofGeneratedAt) {
      errors.push("handoff summary Mac source-home restart receipt generated_at must be at or before machine proof generated_at");
    }
    if (
      sourceHomeRestartFile.served_index_sha256 !== sourceHomeRestartFile.installed_index_sha256 ||
      sourceHomeRestartFile.served_index_sha256 !== sourceHomeRestartFile.source_index_sha256 ||
      sourceHomeRestartFile.installed_index_sha256 !== machineProof.home?.installed_index_sha256 ||
      sourceHomeRestartFile.source_index_sha256 !== machineProof.home?.source_index_sha256
    ) {
      errors.push("handoff summary Mac source-home restart receipt must match served/installed/source Home hashes and the machine proof");
    }
    if (
      helperHashes.some((value) => !/^[a-f0-9]{64}$/i.test(String(value || ""))) ||
      new Set(helperHashes.map((value) => String(value || "").toLowerCase())).size !== 1
    ) {
      errors.push("handoff summary Mac source-home restart receipt must prove matching Browser helper hashes for source, installed script, VM initrd, and VM rootfs");
    }
    for (const key of [
      "browser_helper_source_sha256",
      "browser_helper_installed_sha256",
      "browser_helper_initrd_sha256",
      "browser_helper_rootfs_sha256",
      "served_index_sha256",
      "installed_index_sha256",
      "source_index_sha256",
    ]) {
      if (sourceHomeRestart?.[key] !== sourceHomeRestartFile[key]) {
        errors.push(`handoff summary Mac source-home restart ${key} must match its receipt path`);
      }
    }
  }
  if (receipt?.schema !== "elastos.browser.mac-vm-auth-profile-setup/v1") {
    errors.push("handoff summary must include an auth setup receipt schema");
  }
  if (receipt?.ok !== true) {
    errors.push("handoff summary auth setup receipt must be ok");
  }
  if (!/^[a-f0-9]{64}$/i.test(String(receipt?.sha256 || ""))) {
    errors.push("handoff summary auth setup receipt must include a SHA-256 digest");
  }
  if (typeof receipt?.path !== "string" || receipt.path.trim().length === 0) {
    errors.push("handoff summary auth setup receipt must include a path");
  } else if (!fs.existsSync(receipt.path)) {
    errors.push("handoff summary auth setup receipt path must exist");
  } else if (
    /^[a-f0-9]{64}$/i.test(String(receipt?.sha256 || "")) &&
    sha256File(receipt.path).toLowerCase() !== String(receipt.sha256).toLowerCase()
  ) {
    errors.push("handoff summary auth setup receipt sha256 must match its path");
  } else {
    try {
      receiptFile = readJson(receipt.path);
    } catch {
      errors.push("handoff summary auth setup receipt path must be valid JSON");
    }
  }
  if (receiptFile) {
    const receiptGeneratedAt = parseTimestamp(receiptFile.generated_at);
    if (receiptFile.schema !== receipt.schema) {
      errors.push("handoff summary auth setup receipt schema must match its path");
    }
    if (receiptFile.ok !== true) {
      errors.push("handoff summary auth setup receipt path must be ok");
    }
    if (receiptFile.setup?.headed !== true) {
      errors.push("handoff summary auth setup receipt must come from headed setup");
    }
    if (receiptFile.setup?.preserve_profile !== true) {
      errors.push("handoff summary auth setup receipt must preserve the auth profile");
    }
    if (receiptFile.setup?.cleanup_passkey !== false) {
      errors.push("handoff summary auth setup receipt must keep the auth passkey/profile for proof collection");
    }
    if (receiptFile.setup?.authentication_claim !== "setup_only_not_authentication_proof") {
      errors.push("handoff summary auth setup receipt must not claim ela.city authentication by itself");
    }
    if (receiptFile.setup?.authentication_proof !== "deferred_to_machine_diagnostics_and_manual_ux") {
      errors.push("handoff summary auth setup receipt must defer authentication proof to machine diagnostics and manual UX");
    }
    if (typeof receiptFile.generated_at !== "string" || receiptFile.generated_at.trim().length === 0) {
      errors.push("handoff summary auth setup receipt generated_at must be an ISO timestamp");
    } else if (receiptGeneratedAt == null) {
      errors.push("handoff summary auth setup receipt generated_at must be an ISO timestamp");
    } else if (proofGeneratedAt != null && receiptGeneratedAt > proofGeneratedAt) {
      errors.push("handoff summary auth setup receipt generated_at must be at or before machine proof generated_at");
    }
  }
  if (receipt?.profile_matches_auth_profile !== true) {
    errors.push("handoff summary auth setup receipt must match the proof auth profile");
  }
  if (receipt?.proof_used_persistent_profile !== true) {
    errors.push("handoff summary proof must use the persistent auth profile");
  }
  return {
    ok: errors.length === 0,
    summary,
    validation: {
      ok: errors.length === 0,
      source_home_restart_ok:
        sourceHomeRestartFile != null &&
        !errors.some((error) => String(error).includes("Mac source-home restart")),
      errors,
    },
  };
}

function main() {
  const args = parseArgs(process.argv.slice(2));
  const proof = readJson(args.machineProof);
  const manualResult = macVmManualAccepted(args.machineProof, proof, args.manualUx);
  const handoffResult = macVmHandoffAccepted(args.machineProof, proof, args.handoffSummary);
  const manual = manualResult.manual;
  const controlAfter = proof.vm_control?.after || {};
  const videoInput = proof.embedded_video_input || {};
  const displaySession = videoInput.display_session || {};
  const vmIsolation = videoInput.vm_isolation || {};
  const clickNavigation = videoInput.click_navigation || {};
  const pageDiagnostics = proof.page_diagnostics || {};
  const quality = proof.quality_gates || {};
  const performanceChecks = quality.performance?.checks || {};
  const zoom = quality.zoom || {};
  const zoomChecks = zoom.checks || {};
  const maxDroppedFrames = number(quality.thresholds?.max_dropped_frames, 1);
  const expectedViewportWidth = number(quality.thresholds?.expected_viewport_width, 1280);
  const expectedViewportHeight = number(quality.thresholds?.expected_viewport_height, 720);
  const profileReset = proof.profile_reset || {};
  const profileResetReceipt = profileReset.receipt || null;
  const resetReceiptLeaksAuthority =
    profileResetReceiptLeaksAuthority(profileResetReceipt);
  const controlStartedAt = parseTimestamp(controlAfter.started_at);
  const proofGeneratedAt = parseTimestamp(proof.generated_at);
  const controlRestart = proof.vm_control?.restart || {};
  const controlMaxUptimeMs = number(controlRestart.max_uptime_ms, 0);
  const controlUptimeMs = number(controlAfter.uptime_ms, Infinity);
  const controlRestartProofOk =
    controlAfter.ok === true
    && controlStartedAt != null
    && controlUptimeMs > 0
    && (proofGeneratedAt == null || controlStartedAt <= proofGeneratedAt)
    && controlRestart.schema === "elastos.browser.mac-vm-control-restart/v1"
    && controlRestart.fresh_after_restart === true
    && controlMaxUptimeMs > 0
    && controlUptimeMs <= controlMaxUptimeMs;
  const runtimeMediaRelayOk =
    videoInput.display_mode === "webrtc_remote_display"
    && displaySession.mode === "webrtc_remote_display"
    && displaySession.media_transport === "runtime_relay"
    && number(displaySession.turn_ice_server_count) > 0
    && number(displaySession.credentialed_turn_ice_server_count) > 0;
  const vmIsolationOk =
    vmIsolation.schema === "elastos.browser.engine.identity/v1"
    && vmIsolation.adapter === "browser-vm-product"
    && vmIsolation.engine === "chromium_microvm"
    && vmIsolation.display_mode === "webrtc_remote_display"
    && vmIsolation.guarantee_level === "mechanism_microvm"
    && vmIsolation.engine_control === "page_scoped"
    && vmIsolation.isolated_engine_session === true
    && vmIsolation.isolation_kind === "per_launch_vm_target";
  const diagnosticClickActions = Array.isArray(pageDiagnostics.diagnostic_click_actions)
    ? pageDiagnostics.diagnostic_click_actions
    : [];
  const postClickDiagnostics = diagnosticClickActions
    .map((entry) => entry?.diagnostics)
    .filter(Boolean);
  const allDiagnostics = [pageDiagnostics, ...postClickDiagnostics];
  const rawFields = allDiagnostics.flatMap((entry, index) =>
    rawDiagnosticFields(
      entry,
      index === 0 ? "page_diagnostics" : `diagnostic_click_actions[${index - 1}].diagnostics`,
    ),
  );
  const bodyText = allDiagnostics.map((entry) => entry.body_text || "").filter(Boolean).join("\n");
  const visibleTextSamples = allDiagnostics.flatMap((entry) =>
    Array.isArray(entry.visible_text_samples) ? entry.visible_text_samples : [],
  );
  const dialogElements = allDiagnostics.flatMap((entry) =>
    Array.isArray(entry.dialog_elements) ? entry.dialog_elements : [],
  );
  const clickableElements = allDiagnostics.flatMap((entry) =>
    Array.isArray(entry.clickable_elements) ? entry.clickable_elements : [],
  );
  const textCorpus = [
    bodyText,
    ...visibleTextSamples.flatMap((item) => [item.text, item.aria_label, item.title, item.test_id]),
    ...dialogElements.flatMap((item) => [item.text, item.aria_label, item.title, item.test_id]),
    ...clickableElements.flatMap((item) => [
      item.text,
      item.aria_label,
      item.title,
      item.role,
      item.test_id,
      item.top_element?.action_text,
    ]),
  ].filter(Boolean).join("\n");
  const editProfileActionPattern = /\b(edit profile|account settings)\b/i;
  const editProfileDialogPattern = /\b(edit profile|profile|account settings)\b/i;
  const authenticatedProfilePattern = /\b(edit profile|my profile|account settings|log out|logout)\b/i;
  const looksUnauthenticated = /\blog in\b/i.test(textCorpus) && !authenticatedProfilePattern.test(textCorpus);
  const hasProfileSignal = authenticatedProfilePattern.test(textCorpus);
  const persistentProfile = proof.virtual_auth?.persistent_profile === true;
  const hasDialogSignal = dialogElements.some((item) =>
    editProfileDialogPattern.test(
      [item.text, item.aria_label, item.title, item.test_id].filter(Boolean).join("\n"),
    ),
  );
  const hasEditProfileDiagnosticClick = diagnosticClickActions.some((action) => {
    const actionDialogs = Array.isArray(action?.diagnostics?.dialog_elements)
      ? action.diagnostics.dialog_elements
      : [];
    const actionTargetCorpus = [
      action?.target?.text,
      action?.target?.aria_label,
      action?.target?.title,
      action?.target?.test_id,
    ].filter(Boolean).join("\n");
    return action?.ok === true
      && action?.input?.accepted === true
      && editProfileActionPattern.test(actionTargetCorpus)
      && actionDialogs.some((item) =>
        editProfileDialogPattern.test(
          [item.text, item.aria_label, item.title, item.test_id].filter(Boolean).join("\n"),
        ),
      );
  });
  const clickExpectedUrlRe = String(clickNavigation.expected_url_re || "").trim();
  const clickAddressValue = String(clickNavigation.address_value || "");
  const clickActualUrl = String(clickNavigation.status?.actual_url || "");
  const clickStartingUrl = String(
    videoInput.navigation?.actual_url
      || videoInput.navigation?.requested_url
      || pageDiagnostics.url
      || "",
  );
  const clickExpectedUrlMatches =
    regexMatches(clickExpectedUrlRe, clickAddressValue)
    && regexMatches(clickExpectedUrlRe, clickActualUrl);
  const clickChangedFromStartingUrl =
    Boolean(clickAddressValue && clickActualUrl && clickStartingUrl)
    && clickAddressValue !== clickStartingUrl
    && clickActualUrl !== clickStartingUrl;
  const profileResetReceiptOk =
    profileReset.requested === true
    && profileReset.ok === true
    && profileResetReceipt?.schema === "elastos.browser.profile-reset/v1"
    && profileResetReceipt?.status === "ok"
    && profileResetReceipt?.profile?.scope === "active_principal"
    && profileResetReceipt?.profile?.storage === "principal_owned_profile_disk"
    && profileResetReceipt?.profile?.storage_posture === "principal_owned_reset_scoped_unprotected"
    && profileResetReceipt?.profile?.protected_storage === false
    && profileResetReceipt?.profile?.encrypted === false
    && profileResetReceipt?.profile?.recoverable === false
    && profileResetReceipt?.profile?.recovery === "not_recovery_kit_packaged"
    && profileResetReceipt?.profile?.reset === "whole_profile"
    && profileResetReceipt?.profile?.profile_key == null
    && profileResetReceipt?.profile?.principal_id == null
    && profileResetReceipt?.removed_profile_disk === true
    && !resetReceiptLeaksAuthority;

  const criteria = [
    criterion(
      "machine_proof_ok",
      "Mac Browser VM machine proof is successful and hashable.",
      proof.schema === "elastos.browser.mac-vm-proof/v1" && proof.ok === true,
      [args.machineProof],
      "Run scripts/browser-mac-vm-proof.sh --artifact-out <proof.json> and provide a successful elastos.browser.mac-vm-proof/v1 artifact.",
    ),
    criterion(
      "runtime_only_network",
      "VM Browser uses Runtime-owned networking and does not expose direct network authority.",
      proof.home?.http_code === 200
        && proof.home?.hash_parity === true
        && controlAfter.ok === true
        && controlAfter.network_mode === "runtime_net_only"
        && controlAfter.direct_network === false,
      [args.machineProof],
      "Machine proof must show Home HTTP 200, installed/source hash parity, network_mode=runtime_net_only, and direct_network=false.",
    ),
    criterion(
      "vm_control_restart_proof",
      "Mac Browser VM proof is tied to a timestamped VM control-service run after restart.",
      controlRestartProofOk,
      [args.machineProof],
      "Machine proof must include vm_control.after.started_at as an ISO timestamp, positive vm_control.after.uptime_ms, a control start time not later than proof generated_at, and vm_control.restart.fresh_after_restart=true within its max_uptime_ms.",
    ),
    criterion(
      "source_home_restart_freshness",
      "Mac Browser VM handoff is bound to a fresh source-home restart receipt with matching Browser helper hashes.",
      handoffResult.validation.source_home_restart_ok === true,
      args.handoffSummary ? [args.handoffSummary, args.machineProof] : [args.machineProof],
      "Run scripts/browser-mac-vm-acceptance-handoff.sh --restart-source-home so the handoff summary includes a successful elastos.mac-source-home-restart/v1 receipt whose Home hashes match the machine proof and whose Browser helper hashes match source, installed script, VM initrd, and VM rootfs.",
    ),
    criterion(
      "remote_video_input",
      "Remote video frames and input navigation are present.",
      videoInput.ok === true
        && videoInput.display_mode === "webrtc_remote_display"
        && number(videoInput.remote_video_ready_ms) > 0
        && number(videoInput.decoded_frame_delta) > 0
        && number(videoInput.dropped_frame_delta) <= maxDroppedFrames,
      [args.machineProof],
      `Machine proof must show WebRTC remote display, decoded frames after input, and dropped_frame_delta <= ${maxDroppedFrames}.`,
    ),
    criterion(
      "browser_vm_isolation",
      "The opened Browser page is bound to the Browser VM adapter and per-launch VM isolation receipt.",
      vmIsolationOk,
      [args.machineProof],
      "Machine proof must include embedded_video_input.vm_isolation with adapter=browser-vm-product, engine=chromium_microvm, guarantee_level=mechanism_microvm, engine_control=page_scoped, isolated_engine_session=true, and isolation_kind=per_launch_vm_target.",
    ),
    criterion(
      "runtime_media_relay",
      "WebRTC media uses the Runtime-owned relay path with credentialed TURN and no direct media path.",
      runtimeMediaRelayOk,
      [args.machineProof],
      "Machine proof must include embedded_video_input.display_session with mode=webrtc_remote_display, media_transport=runtime_relay, and at least one credentialed TURN ICE server.",
    ),
    criterion(
      "performance_zoom",
      "Performance and zoom gates pass at the expected Mac viewport.",
      quality.ok === true
        && quality.performance?.ok === true
        && quality.zoom?.ok === true
        && requiredChecksOk(performanceChecks, REQUIRED_PERFORMANCE_CHECKS)
        && requiredChecksOk(zoomChecks, REQUIRED_ZOOM_CHECKS)
        && number(zoom.device_pixel_ratio) === 1
        && number(zoom.viewport_width) === expectedViewportWidth
        && number(zoom.viewport_height) === expectedViewportHeight,
      [args.machineProof],
      `Machine proof must pass each named performance/zoom sub-check with DPR 1 and the expected ${expectedViewportWidth}x${expectedViewportHeight} viewport.`,
    ),
    criterion(
      "ela_city_url_sync",
      "ela.city navigation updates Browser address/status URL after an in-page click.",
      clickNavigation.ok === true
        && clickNavigation.skipped !== true
        && clickExpectedUrlMatches
        && clickChangedFromStartingUrl
        && /^https:\/\/ela\.city\//.test(clickAddressValue)
        && /^https:\/\/ela\.city\//.test(clickActualUrl),
      [args.machineProof],
      "Run the Mac VM proof with ELASTOS_BROWSER_MAC_VM_CLICK_HREF_RE and ELASTOS_BROWSER_MAC_VM_CLICK_EXPECT_URL_RE so Browser address/status URL sync is proven by a changed URL matching the recorded expected_url_re.",
    ),
    criterion(
      "ela_city_images",
      "ela.city visible images settle without visible broken or pending images.",
      pageDiagnostics.ok === true
        && number(pageDiagnostics.visible_image_count) > 0
        && number(pageDiagnostics.visible_broken_image_count) === 0
        && number(pageDiagnostics.visible_pending_image_count) === 0,
      [args.machineProof],
      "Machine diagnostics must show visible images and zero visible broken/pending images.",
    ),
    criterion(
      "sanitized_diagnostics",
      "Mac VM proof diagnostics are sanitized and do not include raw DOM HTML or CDP event dumps.",
      rawFields.length === 0,
      [args.machineProof],
      `Remove raw diagnostic fields from the proof artifact: ${rawFields.join(", ") || "none"}.`,
    ),
    criterion(
      "ela_city_auth_profile_persistence",
      "Authenticated ela.city acceptance is collected with a persistent virtual-auth profile.",
      persistentProfile,
      [args.machineProof],
      "Rerun scripts/browser-mac-vm-proof.sh with ELASTOS_BROWSER_MAC_VM_PROOF_AUTH_PROFILE=<stable-profile-dir> so ela.city login/setup and proof collection use the same Browser VM principal/profile.",
    ),
    criterion(
      "auth_setup_receipt_chain",
      "Authenticated ela.city acceptance is bound to the headed auth setup receipt verified by the Mac VM handoff.",
      handoffResult.ok === true && persistentProfile,
      args.handoffSummary ? [args.handoffSummary, args.machineProof] : [args.machineProof],
      `Run scripts/browser-mac-vm-auth-profile-setup.sh --receipt-out <receipt.json>, then scripts/browser-mac-vm-acceptance-handoff.sh --auth-profile <same-profile> --auth-setup-receipt <receipt.json>, and pass the generated handoff summary with --handoff-summary.`,
    ),
    criterion(
      "profile_reset_safety",
      "Mac Browser VM proof records a sanitized active-principal reset receipt for cookies, local storage, history, and cache.",
      profileResetReceiptOk,
      [args.machineProof],
      "Rerun scripts/browser-mac-vm-proof.sh with ELASTOS_BROWSER_MAC_VM_PROFILE_RESET_PROOF=1; proof.profile_reset must show requested=true, ok=true, an elastos.browser.profile-reset/v1 active_principal principal_owned_profile_disk whole_profile receipt with storage_posture=principal_owned_reset_scoped_unprotected, protected_storage=false, encrypted=false, recoverable=false, removed_profile_disk=true, and no profile key, principal id, disk path, or host path.",
    ),
    criterion(
      "manual_ux_hash_bound",
      "Manual UX report is hash-bound to this exact Mac VM machine proof.",
      manualResult.ok === true,
      args.manualUx ? [args.manualUx, args.machineProof] : [args.machineProof],
      `Generate a template with node scripts/browser-manual-ux-report.mjs --template --machine-artifact ${args.machineProof}, fill it only after real review, then validate it with --input.`,
    ),
    criterion(
      "manual_video_input_performance",
      "Human review confirms visible video, typing/input, scrolling/click fidelity, performance, and zoom.",
      manualResult.ok === true
        && manualCheck(manual, "remote_video_visible")
        && manualCheck(manual, "typing_latency")
        && manualCheck(manual, "scrolling_click_fidelity")
        && manualCheck(manual, "performance_thresholds_reviewed")
        && manualCheck(manual, "zoom_geometry_reviewed"),
      args.manualUx ? [args.manualUx] : [],
      "Manual UX evidence must include remote_video_visible, typing_latency, scrolling_click_fidelity, performance_thresholds_reviewed, and zoom_geometry_reviewed.",
    ),
    criterion(
      "ela_city_authenticated_surface",
      "The reviewed ela.city surface is authenticated enough to test edit profile.",
      manualResult.ok === true && !looksUnauthenticated && hasProfileSignal,
      [args.machineProof],
      "Current machine diagnostics still look unauthenticated or lack a profile signal; rerun the Mac VM proof after ela.city login before claiming edit-profile acceptance.",
    ),
    criterion(
      "ela_city_edit_profile_modal",
      "Authenticated ela.city edit-profile modal opens in the Mac VM Browser.",
      manualResult.ok === true
        && manualCheck(manual, "ela_city_edit_profile_modal")
        && hasEditProfileDiagnosticClick,
      args.manualUx ? [args.manualUx, args.machineProof] : [args.machineProof],
      "Manual UX evidence must record the authenticated edit-profile modal opening, and the same Mac VM proof must include an accepted Edit Profile diagnostic click with a visible profile/edit dialog signal.",
    ),
    criterion(
      "authority_and_cleanup",
      "Manual review confirms no raw authority leakage and clean session cleanup.",
      manualResult.ok === true
        && manualCheck(manual, "no_raw_authority")
        && manualCheck(manual, "session_cleanup")
        && number(controlAfter.active_pages) === 0
        && number(controlAfter.pending_launches) === 0,
      args.manualUx ? [args.manualUx, args.machineProof] : [args.machineProof],
      "Manual UX evidence must include no_raw_authority and session_cleanup, and machine proof must end with no active/pending VM pages.",
    ),
  ];

  const ok = criteria.every((item) => item.ok);
  const result = {
    schema: "elastos.browser.mac-vm-acceptance-audit/v1",
    ok,
    generated_at: new Date().toISOString(),
    machine_proof: {
      path: args.machineProof,
      sha256: sha256File(args.machineProof),
      generated_at: proof.generated_at || null,
    },
    manual_ux: args.manualUx ? {
      path: args.manualUx,
      validation: manualResult.validation,
    } : {
      path: null,
      validation: manualResult.validation,
    },
    handoff_summary: args.handoffSummary ? {
      path: args.handoffSummary,
      validation: handoffResult.validation,
    } : {
      path: null,
      validation: handoffResult.validation,
    },
    ela_city_diagnostics: {
      url: pageDiagnostics.url || null,
      title: pageDiagnostics.title || null,
      looks_unauthenticated: looksUnauthenticated,
      has_profile_signal: hasProfileSignal,
      visible_image_count: number(pageDiagnostics.visible_image_count),
      visible_broken_image_count: number(pageDiagnostics.visible_broken_image_count),
      visible_pending_image_count: number(pageDiagnostics.visible_pending_image_count),
      click_expected_url_re: clickExpectedUrlRe || null,
      click_expected_url_matches: clickExpectedUrlMatches,
      click_starting_url: clickStartingUrl || null,
      click_changed_from_starting_url: clickChangedFromStartingUrl,
      click_address_value: clickAddressValue || null,
      click_actual_url: clickActualUrl || null,
      visible_text_sample_count: visibleTextSamples.length,
      dialog_count: dialogElements.length,
      diagnostic_click_action_count: diagnosticClickActions.length,
      display_session: {
        mode: displaySession.mode || null,
        media_transport: displaySession.media_transport || null,
        display_backend: displaySession.display_backend || null,
        backend_class: displaySession.backend_class || null,
        turn_ice_server_count: number(displaySession.turn_ice_server_count),
        credentialed_turn_ice_server_count: number(displaySession.credentialed_turn_ice_server_count),
      },
      vm_isolation: {
        adapter: vmIsolation.adapter || null,
        engine: vmIsolation.engine || null,
        display_mode: vmIsolation.display_mode || null,
        guarantee_level: vmIsolation.guarantee_level || null,
        engine_control: vmIsolation.engine_control || null,
        isolated_engine_session: vmIsolation.isolated_engine_session === true,
        isolation_kind: vmIsolation.isolation_kind || null,
      },
      has_edit_profile_dialog_signal: hasDialogSignal,
      has_edit_profile_diagnostic_click: hasEditProfileDiagnosticClick,
      raw_diagnostic_fields: rawFields,
      persistent_virtual_auth_profile: persistentProfile,
      cleanup_passkey: proof.virtual_auth?.cleanup_passkey ?? null,
      vm_control_started_at: controlAfter.started_at || null,
      vm_control_uptime_ms: Number.isFinite(number(controlAfter.uptime_ms, NaN)) ? number(controlAfter.uptime_ms) : null,
      vm_control_restart: {
        schema: controlRestart.schema || null,
        fresh_after_restart: controlRestart.fresh_after_restart === true,
        max_uptime_ms: controlMaxUptimeMs || null,
        actual_uptime_ms: Number.isFinite(controlUptimeMs) ? controlUptimeMs : null,
      },
      performance_checks: performanceChecks,
      zoom_checks: zoomChecks,
      profile_reset: {
        requested: profileReset.requested === true,
        ok: profileReset.ok === true,
        receipt_schema: profileResetReceipt?.schema || null,
        receipt_status: profileResetReceipt?.status || null,
        profile_scope: profileResetReceipt?.profile?.scope || null,
        profile_storage: profileResetReceipt?.profile?.storage || null,
        profile_reset: profileResetReceipt?.profile?.reset || null,
        removed_profile_disk: profileResetReceipt?.removed_profile_disk ?? null,
        leaked_authority: resetReceiptLeaksAuthority,
      },
      auth_setup_receipt: handoffResult.summary?.authenticated_profile?.auth_setup_receipt ? {
        ok: handoffResult.summary.authenticated_profile.auth_setup_receipt.ok === true,
        schema: handoffResult.summary.authenticated_profile.auth_setup_receipt.schema || null,
        has_sha256: /^[a-f0-9]{64}$/i.test(String(handoffResult.summary.authenticated_profile.auth_setup_receipt.sha256 || "")),
        profile_matches_auth_profile: handoffResult.summary.authenticated_profile.auth_setup_receipt.profile_matches_auth_profile === true,
        proof_used_persistent_profile: handoffResult.summary.authenticated_profile.auth_setup_receipt.proof_used_persistent_profile === true,
      } : null,
      source_home_restart: handoffResult.summary?.source_home_restart ? {
        ok: handoffResult.summary.source_home_restart.ok === true,
        schema: handoffResult.summary.source_home_restart.schema || null,
        has_sha256: /^[a-f0-9]{64}$/i.test(String(handoffResult.summary.source_home_restart.sha256 || "")),
        generated_at: handoffResult.summary.source_home_restart.generated_at || null,
        helper_hashes_match:
          handoffResult.summary.source_home_restart.browser_helper_source_sha256 === handoffResult.summary.source_home_restart.browser_helper_installed_sha256 &&
          handoffResult.summary.source_home_restart.browser_helper_source_sha256 === handoffResult.summary.source_home_restart.browser_helper_initrd_sha256 &&
          handoffResult.summary.source_home_restart.browser_helper_source_sha256 === handoffResult.summary.source_home_restart.browser_helper_rootfs_sha256,
      } : null,
    },
    criteria,
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
