import crypto from "node:crypto";
import fs from "node:fs";

import {
  HOSTED_WEBRTC_MANUAL_CHECKS,
  MAC_VM_MANUAL_CHECKS,
  requiredManualChecksForSchema,
} from "./browser-manual-ux-checks.mjs";

export const MACHINE_ARTIFACT_SCHEMAS = [
  "elastos.browser.hosted-provider-bakeoff/v1",
  "elastos.browser.native-target-preflight/v1",
  "elastos.browser.mac-vm-proof/v1",
];

function number(value, fallback = 0) {
  const parsed = Number(value);
  return Number.isFinite(parsed) ? parsed : fallback;
}

const REQUIRED_MAC_VM_PERFORMANCE_CHECKS = [
  "remote_video_ready_ms_within_threshold",
  "embedded_navigation_ms_within_threshold",
  "diagnostics_navigation_ms_within_threshold",
  "decoded_frame_delta_ok",
  "dropped_frame_delta_ok",
];

const REQUIRED_MAC_VM_ZOOM_CHECKS = [
  "device_pixel_ratio_ok",
  "viewport_width_ok",
  "viewport_height_ok",
  "panel_aspect_matches_viewport",
  "initial_video_matches_panel",
  "after_navigation_video_matches_panel",
  "source_video_matches_panel",
];

const MAC_VM_VISUAL_REVIEW_ARTIFACT_KINDS = new Set([
  "screen_recording",
]);

export function profileResetReceiptLeaksAuthority(receipt) {
  const profile = receipt?.profile || {};
  if (
    profile.profile_key != null ||
    profile.principal_id != null ||
    profile.disk_path != null
  ) {
    return true;
  }
  if (
    profile.uri != null &&
    profile.uri !== "localhost://Users/self/BrowserProfiles/default/profile.ext4"
  ) {
    return true;
  }
  const scrubbed = {
    ...(receipt || {}),
    profile: {
      ...profile,
      uri: undefined,
    },
  };
  const text = JSON.stringify(scrubbed);
  return /browser-vm\/profiles|profile-[a-f0-9]|person:local|did:key|\/Users\/|\/tmp\//i
    .test(text);
}

function textCorpus(values) {
  return values.filter(Boolean).join("\n");
}

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

function macVmArtifactAccepted(artifact) {
  const afterControl = artifact?.vm_control?.after || {};
  const videoInput = artifact?.embedded_video_input || {};
  const displaySession = videoInput.display_session || {};
  const vmIsolation = videoInput.vm_isolation || {};
  const clickNavigation = videoInput.click_navigation || {};
  const pageDiagnostics = artifact?.page_diagnostics || {};
  const qualityGates = artifact?.quality_gates || {};
  const performanceChecks = qualityGates.performance?.checks || {};
  const zoom = qualityGates.zoom || {};
  const zoomChecks = zoom.checks || {};
  const maxDroppedFrames = number(qualityGates.thresholds?.max_dropped_frames, 1);
  const expectedViewportWidth = number(qualityGates.thresholds?.expected_viewport_width, 1280);
  const expectedViewportHeight = number(qualityGates.thresholds?.expected_viewport_height, 720);
  const profileReset = artifact?.profile_reset || {};
  const profileResetReceipt = profileReset.receipt || null;
  const resetReceiptLeaksAuthority =
    profileResetReceiptLeaksAuthority(profileResetReceipt);
  const diagnosticClickActions = Array.isArray(pageDiagnostics.diagnostic_click_actions)
    ? pageDiagnostics.diagnostic_click_actions
    : [];
  const editProfileActionPattern = /\b(edit profile|account settings)\b/i;
  const editProfileDialogPattern = /\b(edit profile|profile|account settings)\b/i;
  const hasEditProfileDiagnosticClick = diagnosticClickActions.some((action) => {
    const actionDialogs = Array.isArray(action?.diagnostics?.dialog_elements)
      ? action.diagnostics.dialog_elements
      : [];
    const actionTargetCorpus = textCorpus([
      action?.target?.text,
      action?.target?.aria_label,
      action?.target?.title,
      action?.target?.test_id,
    ]);
    return action?.ok === true
      && action?.input?.accepted === true
      && editProfileActionPattern.test(actionTargetCorpus)
      && actionDialogs.some((item) =>
        editProfileDialogPattern.test(
          textCorpus([item.text, item.aria_label, item.title, item.test_id]),
        ),
      );
  });
  const controlStartedAt = parseTimestamp(afterControl.started_at);
  const proofGeneratedAt = parseTimestamp(artifact?.generated_at);
  const controlRestart = artifact?.vm_control?.restart || {};
  const controlUptimeMs = number(afterControl.uptime_ms, Infinity);
  const controlMaxUptimeMs = number(controlRestart.max_uptime_ms, 0);
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
  const runtimeMediaRelayOk =
    videoInput.display_mode === "webrtc_remote_display" &&
    displaySession.mode === "webrtc_remote_display" &&
    displaySession.media_transport === "runtime_relay" &&
    number(displaySession.turn_ice_server_count) > 0 &&
    number(displaySession.credentialed_turn_ice_server_count) > 0;
  return (
    artifact?.schema === "elastos.browser.mac-vm-proof/v1" &&
    artifact.ok === true &&
    (proofGeneratedAt == null || (controlStartedAt != null && controlStartedAt <= proofGeneratedAt)) &&
    artifact.home?.http_code === 200 &&
    artifact.home?.hash_parity === true &&
    afterControl.ok === true &&
    controlStartedAt != null &&
    controlUptimeMs > 0 &&
    controlRestart.schema === "elastos.browser.mac-vm-control-restart/v1" &&
    controlRestart.fresh_after_restart === true &&
    controlMaxUptimeMs > 0 &&
    controlUptimeMs <= controlMaxUptimeMs &&
    afterControl.network_mode === "runtime_net_only" &&
    afterControl.direct_network === false &&
    number(afterControl.active_pages) === 0 &&
    number(afterControl.pending_launches) === 0 &&
    videoInput.ok === true &&
    videoInput.display_mode === "webrtc_remote_display" &&
    vmIsolation.schema === "elastos.browser.engine.identity/v1" &&
    vmIsolation.adapter === "browser-vm-product" &&
    vmIsolation.engine === "chromium_microvm" &&
    vmIsolation.display_mode === "webrtc_remote_display" &&
    vmIsolation.guarantee_level === "mechanism_microvm" &&
    vmIsolation.engine_control === "page_scoped" &&
    vmIsolation.isolated_engine_session === true &&
    vmIsolation.isolation_kind === "per_launch_vm_target" &&
    runtimeMediaRelayOk &&
    number(videoInput.remote_video_ready_ms) > 0 &&
    number(videoInput.decoded_frame_delta) > 0 &&
    number(videoInput.dropped_frame_delta) <= maxDroppedFrames &&
    clickNavigation.ok === true &&
    clickNavigation.skipped !== true &&
    clickExpectedUrlMatches &&
    clickChangedFromStartingUrl &&
    /^https:\/\/ela\.city\//.test(clickAddressValue) &&
    /^https:\/\/ela\.city\//.test(clickActualUrl) &&
    pageDiagnostics.ok === true &&
    number(pageDiagnostics.visible_image_count) > 0 &&
    number(pageDiagnostics.visible_broken_image_count) === 0 &&
    number(pageDiagnostics.visible_pending_image_count) === 0 &&
    qualityGates.ok === true &&
    qualityGates.performance?.ok === true &&
    zoom.ok === true &&
    requiredChecksOk(performanceChecks, REQUIRED_MAC_VM_PERFORMANCE_CHECKS) &&
    requiredChecksOk(zoomChecks, REQUIRED_MAC_VM_ZOOM_CHECKS) &&
    number(zoom.device_pixel_ratio) === 1 &&
    number(zoom.viewport_width) === expectedViewportWidth &&
    number(zoom.viewport_height) === expectedViewportHeight &&
    profileReset.requested === true &&
    profileReset.ok === true &&
    profileResetReceipt?.schema === "elastos.browser.profile-reset/v1" &&
    profileResetReceipt?.status === "ok" &&
    profileResetReceipt?.profile?.scope === "active_principal" &&
    profileResetReceipt?.profile?.storage === "principal_owned_profile_disk" &&
    profileResetReceipt?.profile?.storage_posture === "principal_owned_reset_scoped_unprotected" &&
    profileResetReceipt?.profile?.protected_storage === false &&
    profileResetReceipt?.profile?.encrypted === false &&
    profileResetReceipt?.profile?.recoverable === false &&
    profileResetReceipt?.profile?.recovery === "not_recovery_kit_packaged" &&
    profileResetReceipt?.profile?.reset === "whole_profile" &&
    profileResetReceipt?.profile?.profile_key == null &&
    profileResetReceipt?.profile?.principal_id == null &&
    profileResetReceipt?.removed_profile_disk === true &&
    !resetReceiptLeaksAuthority &&
    hasEditProfileDiagnosticClick
  );
}

export function sha256File(path) {
  if (!path) return "";
  return crypto.createHash("sha256").update(fs.readFileSync(path)).digest("hex");
}

function realpath(path) {
  try {
    return fs.realpathSync(path);
  } catch {
    return "";
  }
}

function parseTimestamp(value) {
  if (typeof value !== "string" || value.trim().length === 0) {
    return null;
  }
  const millis = Date.parse(value);
  return Number.isNaN(millis) ? null : millis;
}

function reviewArtifactTextIsRedacted(path) {
  const text = fs.readFileSync(path, "utf8");
  return !/connect_ticket|relay_ipc|adapter_ipc|runtime_stream_path|home_token|authorization|cookie|set-cookie|person:local|did:key|profile-[a-f0-9]|browser-vm\/profiles|\.ext4/i
    .test(text);
}

function validateReviewArtifacts(report, errors) {
  const artifacts = report?.review_artifacts;
  if (artifacts == null) {
    return [];
  }
  if (!Array.isArray(artifacts)) {
    errors.push("review_artifacts must be an array when present");
    return [];
  }
  const accepted = [];
  for (const [index, artifact] of artifacts.entries()) {
    if (typeof artifact?.kind !== "string" || artifact.kind.trim().length === 0) {
      errors.push(`review_artifacts[${index}].kind must be a non-empty string`);
    }
    if (typeof artifact?.description !== "string" || artifact.description.trim().length === 0) {
      errors.push(`review_artifacts[${index}].description must be a non-empty string`);
    }
    if (artifact?.redacted !== true) {
      errors.push(`review_artifacts[${index}].redacted must be true after redaction review`);
    }
    if (typeof artifact?.sha256 !== "string" || !/^[a-f0-9]{64}$/i.test(artifact.sha256)) {
      errors.push(`review_artifacts[${index}].sha256 must be a 64-character hex SHA-256 digest`);
    }
    if (typeof artifact?.path !== "string" || artifact.path.trim().length === 0) {
      errors.push(`review_artifacts[${index}].path must be a non-empty string`);
      continue;
    }
    if (!fs.existsSync(artifact.path)) {
      errors.push(`review_artifacts[${index}].path must point to the reviewed local artifact`);
      continue;
    }
    if (/person:local|did:key|profile-[a-f0-9]|connect_ticket|relay_ipc|adapter_ipc|runtime_stream_path|home_token|authorization|cookie|set-cookie/i.test(artifact.path)) {
      errors.push(`review_artifacts[${index}].path must not contain raw authority identifiers`);
    }
    if (!reviewArtifactTextIsRedacted(artifact.path)) {
      errors.push(`review_artifacts[${index}].path must point to a redacted artifact without raw authority text`);
    }
    if (
      typeof artifact?.sha256 === "string" &&
      /^[a-f0-9]{64}$/i.test(artifact.sha256) &&
      sha256File(artifact.path).toLowerCase() !== artifact.sha256.toLowerCase()
    ) {
      errors.push(`review_artifacts[${index}].sha256 must match review_artifacts[${index}].path`);
      continue;
    }
    accepted.push(artifact);
  }
  return accepted;
}

export function validateManualUxReport(
  report,
  { acceptedArtifacts = [], requireAcceptedArtifact = false } = {},
) {
  const errors = [];
  if (report?.schema !== "elastos.browser.manual-ux/v1") {
    errors.push("schema must be elastos.browser.manual-ux/v1");
  }
  if (report?.ok !== true) {
    errors.push("ok must be true after manual review passes");
  }
  const requiredChecks = requiredManualChecksForSchema(report?.machine_artifact?.schema);
  const checkKeys =
    report?.checks && typeof report.checks === "object" && !Array.isArray(report.checks)
      ? Object.keys(report.checks)
      : [];
  const evidenceKeys =
    report?.evidence && typeof report.evidence === "object" && !Array.isArray(report.evidence)
      ? Object.keys(report.evidence)
      : [];
  for (const name of checkKeys) {
    if (!requiredChecks.includes(name)) {
      errors.push(`checks.${name} is not valid for machine_artifact.schema`);
    }
  }
  for (const name of evidenceKeys) {
    if (!requiredChecks.includes(name)) {
      errors.push(`evidence.${name} is not valid for machine_artifact.schema`);
    }
  }
  for (const name of requiredChecks) {
    if (report?.checks?.[name] !== true) {
      errors.push(`checks.${name} must be true`);
    }
  }
  if (report?.machine_artifact?.schema === "elastos.browser.hosted-provider-bakeoff/v1") {
    for (const name of HOSTED_WEBRTC_MANUAL_CHECKS) {
      if (typeof report.evidence?.[name] !== "string" || report.evidence[name].trim().length === 0) {
        errors.push(`evidence.${name} must describe the observed hosted WebRTC audio proof`);
      }
    }
  }
  if (report?.machine_artifact?.schema === "elastos.browser.mac-vm-proof/v1") {
    for (const name of MAC_VM_MANUAL_CHECKS) {
      if (typeof report.evidence?.[name] !== "string" || report.evidence[name].trim().length === 0) {
        errors.push(`evidence.${name} must describe the observed Mac VM proof`);
      }
    }
    if (
      typeof report.evidence?.ela_city_edit_profile_modal === "string" &&
      report.evidence.ela_city_edit_profile_modal.trim().length > 0 &&
      !/\b(edit profile|account settings)\b/i.test(report.evidence.ela_city_edit_profile_modal)
    ) {
      errors.push("evidence.ela_city_edit_profile_modal must cite Edit Profile or Account Settings");
    }
  }
  const acceptedReviewArtifacts = validateReviewArtifacts(report, errors);
  if (
    report?.machine_artifact?.schema === "elastos.browser.mac-vm-proof/v1" &&
    !acceptedReviewArtifacts.some((artifact) =>
      MAC_VM_VISUAL_REVIEW_ARTIFACT_KINDS.has(artifact.kind),
    )
  ) {
    errors.push("review_artifacts must include at least one hash-bound redacted Mac VM screen recording artifact");
  }
  for (const field of ["reviewed_at", "reviewer", "provider", "target"]) {
    if (typeof report?.[field] !== "string" || report[field].trim().length === 0) {
      errors.push(`${field} must be a non-empty string`);
    }
  }
  const reviewedAt = parseTimestamp(report?.reviewed_at);
  if (typeof report?.reviewed_at === "string" && report.reviewed_at.trim().length > 0 && reviewedAt == null) {
    errors.push("reviewed_at must be an ISO timestamp");
  }

  const artifact = report?.machine_artifact;
  let artifactPath = "";
  if (!MACHINE_ARTIFACT_SCHEMAS.includes(artifact?.schema)) {
    errors.push("machine_artifact.schema must identify the accepted hosted bake-off, native preflight, or Mac VM proof schema");
  }
  if (typeof artifact?.sha256 !== "string" || !/^[a-f0-9]{64}$/i.test(artifact.sha256)) {
    errors.push("machine_artifact.sha256 must be a 64-character hex SHA-256 digest");
  }
  if (typeof artifact?.path !== "string" || artifact.path.trim().length === 0) {
    errors.push("machine_artifact.path must be a non-empty string");
  } else if (!fs.existsSync(artifact.path)) {
    errors.push("machine_artifact.path must point to the reviewed machine artifact JSON");
  } else if (sha256File(artifact.path).toLowerCase() !== String(artifact.sha256 || "").toLowerCase()) {
    errors.push("machine_artifact.sha256 must match machine_artifact.path");
  } else {
    try {
      const artifactFile = JSON.parse(fs.readFileSync(artifact.path, "utf8"));
      if (artifactFile.schema !== artifact.schema) {
        errors.push("machine_artifact.schema must match machine_artifact.path");
      }
      if (artifactFile.ok !== true) {
        errors.push("machine_artifact.path must point to a successful machine artifact");
      }
      if (artifact?.schema === "elastos.browser.mac-vm-proof/v1" && !macVmArtifactAccepted(artifactFile)) {
        errors.push("machine_artifact.path must point to a successful Mac VM proof with Runtime-only networking, Runtime media relay proof, WebRTC video/input, changed click URL sync, settled visible images, quality gates, fresh restart evidence, safe profile reset proof, edit-profile diagnostic click proof, and clean shutdown");
      }
      const generatedAt = parseTimestamp(artifactFile.generated_at);
      if (typeof artifactFile.generated_at === "string" && artifactFile.generated_at.trim().length > 0 && generatedAt == null) {
        errors.push("machine_artifact.generated_at must be an ISO timestamp when present");
      } else if (reviewedAt != null && generatedAt != null && reviewedAt < generatedAt) {
        errors.push("reviewed_at must be at or after machine_artifact.generated_at");
      }
      artifactPath = realpath(artifact.path);
    } catch {
      errors.push("machine_artifact.path must be valid JSON");
    }
  }

  if (
    requireAcceptedArtifact &&
    !acceptedArtifacts.some(
      (accepted) =>
        accepted.schema === artifact?.schema &&
        accepted.sha256.toLowerCase() === String(artifact?.sha256 || "").toLowerCase() &&
        realpath(accepted.path) === artifactPath,
    )
  ) {
    errors.push("machine_artifact must match an accepted machine artifact passed to the objective audit");
  }

  if (errors.length > 0) {
    return {
      ok: false,
      schema: "elastos.browser.manual-ux.validation/v1",
      errors,
    };
  }
  return {
    ok: true,
    schema: "elastos.browser.manual-ux.validation/v1",
    provider: report.provider,
    target: report.target,
    machine_artifact: artifact,
  };
}
