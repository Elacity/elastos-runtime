#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
tmp_dir="$(mktemp -d /tmp/elastos-browser-objective-audit-smoke-XXXXXX)"
cleanup() {
  rm -rf "$tmp_dir"
}
trap cleanup EXIT

node "$repo_root/scripts/browser-objective-audit.mjs" --help >"$tmp_dir/help.txt" 2>&1 || true

node -e '
  const fs = require("node:fs");
  const text = fs.readFileSync(process.argv[1], "utf8");
  const required = [
    "--hosted-bakeoff /path/to/accepted-hosted-bakeoff.json | --native-preflight /path/to/accepted-native-preflight.json",
    "Pass only the accepted product artifact for the path actually proven",
    "Do not pass placeholder paths"
  ];
  const missing = required.filter((needle) => !text.includes(needle));
  if (missing.length > 0) {
    console.error(`objective audit help missing expected completion guidance: ${missing.join(", ")}`);
    process.exit(1);
  }
' "$tmp_dir/help.txt"

cat >"$tmp_dir/native-declared-only.json" <<'JSON'
{
  "schema": "elastos.browser.native-target-preflight/v1",
  "ok": true,
  "network_mode": "runtime_net_only",
  "direct_network": false,
  "native_audio_declared": true,
  "native_video_declared": true,
  "native_audio_proven": false,
  "native_video_proven": false,
  "native_media_required": true
}
JSON

cat >"$tmp_dir/manual-passed.json" <<'JSON'
{
  "schema": "elastos.browser.manual-ux/v1",
  "ok": true,
  "reviewed_at": "2026-05-13T00:00:00Z",
  "reviewer": "objective-audit-smoke",
  "provider": "fake-native-declared-only",
  "target": "test",
  "machine_artifact": {
    "schema": "elastos.browser.native-target-preflight/v1",
    "sha256": "0000000000000000000000000000000000000000000000000000000000000000",
    "path": "detached-test-artifact.json"
  },
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
JSON

set +e
node "$repo_root/scripts/browser-objective-audit.mjs" \
  --native-preflight "$tmp_dir/native-declared-only.json" \
  --manual-ux "$tmp_dir/manual-passed.json" \
  >"$tmp_dir/audit.json" \
  2>"$tmp_dir/audit.err"
status=$?
set -e

if [[ "$status" -eq 0 ]]; then
  echo "objective audit accepted declaration-only native media" >&2
  cat "$tmp_dir/audit.json" >&2
  exit 1
fi

node -e '
  const fs = require("node:fs");
  const audit = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  if (!audit.objective || !String(audit.objective.restatement || "").includes("enable/prove audio")) {
    console.error("objective audit must restate the Browser/audio objective");
    process.exit(1);
  }
  if (!Array.isArray(audit.prompt_to_artifact_checklist)) {
    console.error("objective audit must emit prompt_to_artifact_checklist");
    process.exit(1);
  }
  const audioItem = audit.prompt_to_artifact_checklist.find((item) => item.id === "audio_product_proven");
  if (!audioItem || audioItem.ok !== false || !String(audioItem.missing || "").includes("native_audio_proven=true")) {
    console.error("prompt checklist must fail closed on missing product audio proof");
    process.exit(1);
  }
  if (!String(audioItem.missing || "").includes("--artifact-out")) {
    console.error("prompt checklist missing text must require artifact-producing product proof");
    process.exit(1);
  }
  const pathItem = audit.prompt_to_artifact_checklist.find((item) => item.id === "best_path_determined");
  if (!pathItem || pathItem.ok !== true || !Array.isArray(pathItem.evidence) || !pathItem.evidence.includes("scripts/browser-provider-runbook.mjs")) {
    console.error("best_path_determined must require the structured provider decision next_action contract");
    process.exit(1);
  }
  const plannedItem = audit.prompt_to_artifact_checklist.find((item) => item.id === "planned_and_iterated");
  if (!plannedItem || !Array.isArray(plannedItem.evidence) || plannedItem.evidence.includes("TODAY.md")) {
    console.error("planned_and_iterated evidence must use durable docs/scripts, not ignored TODAY.md");
    process.exit(1);
  }
  const nextActionCriterion = audit.criteria.find((item) => item.id === "provider_decision_next_action_defined");
  if (!nextActionCriterion || nextActionCriterion.ok !== true || !String(nextActionCriterion.description || "").includes("structured next action")) {
    console.error("provider_decision_next_action_defined criterion must pass in the objective audit");
    process.exit(1);
  }
  const stopConditionCriterion = audit.criteria.find((item) => item.id === "current_host_stop_condition_defined");
  if (!stopConditionCriterion || stopConditionCriterion.ok !== true) {
    console.error("current_host_stop_condition_defined criterion must pass in the objective audit");
    process.exit(1);
  }
  if (!String(stopConditionCriterion.description || "").includes("stop local Browser provider tuning")) {
    console.error("current_host_stop_condition_defined must make the local stop condition explicit");
    process.exit(1);
  }
  if (!stopConditionCriterion.evidence?.includes("scripts/browser-provider-decision-report-smoke.sh")) {
    console.error("current_host_stop_condition_defined must cite the provider decision report smoke");
    process.exit(1);
  }
  const browserUiAudioCriterion = audit.criteria.find((item) => item.id === "browser_ui_audio_unlock_path");
  if (!browserUiAudioCriterion || browserUiAudioCriterion.ok !== true || !browserUiAudioCriterion.evidence?.includes("capsules/browser/browser.js")) {
    console.error("browser_ui_audio_unlock_path criterion must pass and cite Browser UI source");
    process.exit(1);
  }
  if (!String(browserUiAudioCriterion.description || "").includes("explicit user gesture")) {
    console.error("browser_ui_audio_unlock_path must require explicit user-gesture audio unlock");
    process.exit(1);
  }
  const firstNextAction = audit.next_actions?.[0];
  if (!firstNextAction || firstNextAction.id !== "consult_provider_decision_report") {
    console.error("objective audit next_actions must start with the live provider decision report");
    process.exit(1);
  }
  const hostedAction = audit.next_actions?.find((item) => item.id === "run_hosted_provider_bakeoff");
  if (!hostedAction || !hostedAction.commands?.some((command) => command.includes("--artifact-out <dir>/hosted-bakeoff.json"))) {
    console.error("objective audit hosted next action must write a hosted bake-off artifact");
    process.exit(1);
  }
  if (!hostedAction.commands?.some((command) => command.includes("--resize-width 1000 --resize-height 700"))) {
    console.error("objective audit hosted next action must include the remote viewport resize gate");
    process.exit(1);
  }
  const nativeAction = audit.next_actions?.find((item) => item.id === "prove_native_product_media");
  if (!nativeAction || !nativeAction.commands?.some((command) => command.includes("--artifact-out <dir>/native-preflight.json"))) {
    console.error("objective audit native next action must write a native preflight artifact");
    process.exit(1);
  }
  const nativeCriterion = audit.criteria.find((item) => item.id === "native_product_media_accepted");
  if (!nativeCriterion || nativeCriterion.ok !== false) {
    console.error("native_product_media_accepted must fail for declaration-only native media");
    process.exit(1);
  }
  if (!String(nativeCriterion.missing || "").includes("native_audio_proven=true")) {
    console.error("native_product_media_accepted missing text must require native_audio_proven=true");
    process.exit(1);
  }
  if (!String(nativeCriterion.missing || "").includes("--artifact-out")) {
    console.error("native_product_media_accepted missing text must require --artifact-out");
    process.exit(1);
  }
  const hostedCriterion = audit.criteria.find((item) => item.id === "hosted_provider_product_accepted");
  if (!hostedCriterion || !String(hostedCriterion.missing || "").includes("--artifact-out")) {
    console.error("hosted_provider_product_accepted missing text must require --artifact-out");
    process.exit(1);
  }
  if (!String(hostedCriterion.missing || "").includes("dynamic viewport resize")) {
    console.error("hosted_provider_product_accepted missing text must require dynamic viewport resize proof");
    process.exit(1);
  }
  const manualCriterion = audit.criteria.find((item) => item.id === "manual_ux_accepted");
  if (!manualCriterion || !String(manualCriterion.description || "").includes("hosted WebRTC audio unlock")) {
    console.error("manual_ux_accepted must explicitly require hosted WebRTC audio-unlock evidence where applicable");
    process.exit(1);
  }
' "$tmp_dir/audit.json"

cat >"$tmp_dir/hosted-shallow-ok.json" <<'JSON'
{
  "schema": "elastos.browser.hosted-provider-bakeoff/v1",
  "ok": true
}
JSON

set +e
node "$repo_root/scripts/browser-objective-audit.mjs" \
  --hosted-bakeoff "$tmp_dir/hosted-shallow-ok.json" \
  --manual-ux "$tmp_dir/manual-passed.json" \
  >"$tmp_dir/hosted-shallow-audit.json" \
  2>"$tmp_dir/hosted-shallow-audit.err"
hosted_shallow_status=$?
set -e

if [[ "$hosted_shallow_status" -eq 0 ]]; then
  echo "objective audit accepted shallow hosted ok=true artifact" >&2
  cat "$tmp_dir/hosted-shallow-audit.json" >&2
  exit 1
fi

node -e '
  const fs = require("node:fs");
  const audit = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  const hostedCriterion = audit.criteria.find((item) => item.id === "hosted_provider_product_accepted");
  if (!hostedCriterion || hostedCriterion.ok !== false) {
    console.error("hosted_provider_product_accepted must fail for shallow ok=true artifacts");
    process.exit(1);
  }
  if (!String(hostedCriterion.missing || "").includes("non-skipped YouTube")) {
    console.error("hosted_provider_product_accepted missing text must require non-skipped YouTube stress");
    process.exit(1);
  }
' "$tmp_dir/hosted-shallow-audit.json"

cat >"$tmp_dir/hosted-skipped-youtube.json" <<'JSON'
{
  "schema": "elastos.browser.hosted-provider-bakeoff/v1",
  "ok": true,
  "manual_ux_required": true,
  "candidate_gate": {
    "ok": true,
    "status": 0,
    "result": {
      "schema": "elastos.browser.hosted-provider-candidate-smoke/v1",
      "display_backend": "browserbox_webrtc",
      "backend_class": "product_compositor",
      "audio_track": true,
      "video_track": true,
      "datachannel_input": true,
      "held_ms": 10000,
      "quality_gate": {
        "decoded_frames": 10,
        "dropped_frames": 0,
        "drop_ratio": 0,
        "min_video_width": 1280,
        "min_video_height": 720,
        "min_video_fps": 24,
        "max_video_drop_ratio": 0.1
      },
      "resize_gate": {
        "requested_width": 1000,
        "requested_height": 700,
        "css_width": 1000,
        "css_height": 700,
        "video_width": 1920,
        "video_height": 1080
      },
      "navigation": {
        "can_go_back_after_navigate": true,
        "can_go_forward_after_back": true
      },
      "wallet_bridge": { "available": true },
      "glide_connected_account": "0x1111111111111111111111111111111111111111",
      "direct_network": false
    }
  },
  "youtube_stress": {
    "skipped": true,
    "reason": "operator skipped product-compositor YouTube stress gate"
  }
}
JSON

set +e
node "$repo_root/scripts/browser-objective-audit.mjs" \
  --hosted-bakeoff "$tmp_dir/hosted-skipped-youtube.json" \
  --manual-ux "$tmp_dir/manual-passed.json" \
  >"$tmp_dir/hosted-skipped-youtube-audit.json" \
  2>"$tmp_dir/hosted-skipped-youtube-audit.err"
hosted_skipped_youtube_status=$?
set -e

if [[ "$hosted_skipped_youtube_status" -eq 0 ]]; then
  echo "objective audit accepted hosted artifact with skipped YouTube stress" >&2
  cat "$tmp_dir/hosted-skipped-youtube-audit.json" >&2
  exit 1
fi

cat >"$tmp_dir/hosted-valid.json" <<'JSON'
{
  "schema": "elastos.browser.hosted-provider-bakeoff/v1",
  "ok": true,
  "manual_ux_required": true,
  "candidate_gate": {
    "ok": true,
    "status": 0,
    "result": {
      "schema": "elastos.browser.hosted-provider-candidate-smoke/v1",
      "display_backend": "browserbox_webrtc",
      "backend_class": "product_compositor",
      "audio_track": true,
      "video_track": true,
      "datachannel_input": true,
      "held_ms": 10000,
      "quality_gate": {
        "decoded_frames": 10,
        "dropped_frames": 0,
        "drop_ratio": 0,
        "min_video_width": 1280,
        "min_video_height": 720,
        "min_video_fps": 24,
        "max_video_drop_ratio": 0.1
      },
      "resize_gate": {
        "requested_width": 1000,
        "requested_height": 700,
        "css_width": 1000,
        "css_height": 700,
        "video_width": 1920,
        "video_height": 1080
      },
      "navigation": {
        "can_go_back_after_navigate": true,
        "can_go_forward_after_back": true
      },
      "wallet_bridge": { "available": true },
      "glide_connected_account": "0x1111111111111111111111111111111111111111",
      "direct_network": false
    }
  },
  "youtube_stress": {
    "ok": true,
    "status": 0,
    "result": {
      "schema": "elastos.browser.hosted-product-webrtc-smoke/v1",
      "display_backend": "browserbox_webrtc",
      "backend_class": "product_compositor",
      "audio_track": true,
      "video_track": true,
      "datachannel_input": true,
      "held_ms": 10000,
      "quality_gate": {
        "decoded_frames": 10,
        "dropped_frames": 0,
        "drop_ratio": 0,
        "min_video_width": 1280,
        "min_video_height": 720,
        "min_video_fps": 24,
        "max_video_drop_ratio": 0.1
      },
      "resize_gate": {
        "requested_width": 1000,
        "requested_height": 700,
        "css_width": 1000,
        "css_height": 700,
        "video_width": 1920,
        "video_height": 1080
      },
      "media": {
        "audio_decoded_delta": 1024,
        "video_decoded_delta": 2048
      },
      "direct_network": false
    }
  }
}
JSON

node -e '
  const fs = require("node:fs");
  const artifact = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  delete artifact.candidate_gate.result.resize_gate;
  delete artifact.youtube_stress.result.resize_gate;
  fs.writeFileSync(process.argv[2], `${JSON.stringify(artifact, null, 2)}\n`);
' "$tmp_dir/hosted-valid.json" "$tmp_dir/hosted-missing-resize.json"

set +e
node "$repo_root/scripts/browser-objective-audit.mjs" \
  --hosted-bakeoff "$tmp_dir/hosted-missing-resize.json" \
  --manual-ux "$tmp_dir/manual-passed.json" \
  >"$tmp_dir/hosted-missing-resize-audit.json" \
  2>"$tmp_dir/hosted-missing-resize-audit.err"
hosted_missing_resize_status=$?
set -e

if [[ "$hosted_missing_resize_status" -eq 0 ]]; then
  echo "objective audit accepted hosted artifact without resize_gate" >&2
  cat "$tmp_dir/hosted-missing-resize-audit.json" >&2
  exit 1
fi

node -e '
  const fs = require("node:fs");
  const audit = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  const hostedCriterion = audit.criteria.find((item) => item.id === "hosted_provider_product_accepted");
  if (!hostedCriterion || hostedCriterion.ok !== false) {
    console.error("hosted_provider_product_accepted must fail without resize_gate");
    process.exit(1);
  }
  if (!String(hostedCriterion.missing || "").includes("dynamic viewport resize")) {
    console.error("missing resize_gate rejection must point to dynamic viewport resize proof");
    process.exit(1);
  }
' "$tmp_dir/hosted-missing-resize-audit.json"

hosted_valid_sha="$(sha256sum "$tmp_dir/hosted-valid.json" | awk '{print $1}')"
node "$repo_root/scripts/browser-manual-ux-report.mjs" \
  --template \
  --machine-artifact "$tmp_dir/hosted-valid.json" \
  >"$tmp_dir/manual-template-hosted.json"
node -e '
  const fs = require("node:fs");
  const template = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  const expectedSha = process.argv[2];
  const expectedPath = process.argv[3];
  if (template.provider !== "browserbox_webrtc" && template.provider !== "browserbox") {
    console.error(`manual UX template must prefill hosted provider, got ${template.provider}`);
    process.exit(1);
  }
  if (template.target !== "browserbox_webrtc") {
    console.error(`manual UX template must prefill hosted target, got ${template.target}`);
    process.exit(1);
  }
  if (template.machine_artifact?.schema !== "elastos.browser.hosted-provider-bakeoff/v1") {
    console.error("manual UX template must prefill hosted machine artifact schema");
    process.exit(1);
  }
  if (template.machine_artifact?.sha256 !== expectedSha) {
    console.error("manual UX template must prefill hosted machine artifact sha256");
    process.exit(1);
  }
  if (template.machine_artifact?.path !== expectedPath) {
    console.error("manual UX template must prefill hosted machine artifact path");
    process.exit(1);
  }
  const hostedAudioChecks = [
    "display_session_audio_advertised",
    "audio_unlock_gesture",
    "remote_audio_unmuted_status",
    "received_audio_evidence"
  ];
  for (const check of hostedAudioChecks) {
    if (template.checks?.[check] !== false) {
      console.error(`manual UX template must include hosted WebRTC audio check ${check}`);
      process.exit(1);
    }
    if (template.evidence?.[check] !== "") {
      console.error(`manual UX template must include empty hosted WebRTC audio evidence field ${check}`);
      process.exit(1);
    }
  }
' "$tmp_dir/manual-template-hosted.json" "$hosted_valid_sha" "$tmp_dir/hosted-valid.json"

cat >"$tmp_dir/manual-hosted-detached.json" <<JSON
{
  "schema": "elastos.browser.manual-ux/v1",
  "ok": true,
  "reviewed_at": "2026-05-13T00:00:00Z",
  "reviewer": "objective-audit-smoke",
  "provider": "fake-hosted-valid",
  "target": "test",
  "machine_artifact": {
    "schema": "elastos.browser.hosted-provider-bakeoff/v1",
    "sha256": "1111111111111111111111111111111111111111111111111111111111111111",
    "path": "$tmp_dir/hosted-valid.json"
  },
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
  },
  "evidence": {
    "display_session_audio_advertised": "display session reported audio=true",
    "audio_unlock_gesture": "clicked render panel and playback unlocked",
    "remote_audio_unmuted_status": "remote video was unmuted with volume=1",
    "received_audio_evidence": "WebRTC stats showed received audio bytes"
  }
}
JSON

set +e
node "$repo_root/scripts/browser-manual-ux-report.mjs" \
  --input "$tmp_dir/manual-hosted-detached.json" \
  >"$tmp_dir/manual-detached-validation.json" \
  2>"$tmp_dir/manual-detached-validation.err"
manual_detached_validation_status=$?
set -e

if [[ "$manual_detached_validation_status" -eq 0 ]]; then
  echo "manual UX validator accepted a machine artifact hash that does not match the referenced file" >&2
  cat "$tmp_dir/manual-detached-validation.json" >&2
  exit 1
fi

node -e '
  const fs = require("node:fs");
  const validation = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  if (!Array.isArray(validation.errors) || !validation.errors.includes("machine_artifact.sha256 must match machine_artifact.path")) {
    console.error("manual UX validator must report machine artifact hash mismatch");
    process.exit(1);
  }
' "$tmp_dir/manual-detached-validation.json"

cat >"$tmp_dir/manual-hosted-schema-mismatch.json" <<JSON
{
  "schema": "elastos.browser.manual-ux/v1",
  "ok": true,
  "reviewed_at": "2026-05-13T00:00:00Z",
  "reviewer": "objective-audit-smoke",
  "provider": "fake-hosted-valid",
  "target": "test",
  "machine_artifact": {
    "schema": "elastos.browser.native-target-preflight/v1",
    "sha256": "$hosted_valid_sha",
    "path": "$tmp_dir/hosted-valid.json"
  },
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
  },
  "evidence": {
    "display_session_audio_advertised": "display session reported audio=true",
    "audio_unlock_gesture": "clicked render panel and playback unlocked",
    "remote_audio_unmuted_status": "remote video was unmuted with volume=1",
    "received_audio_evidence": "WebRTC stats showed received audio bytes"
  }
}
JSON

set +e
node "$repo_root/scripts/browser-manual-ux-report.mjs" \
  --input "$tmp_dir/manual-hosted-schema-mismatch.json" \
  >"$tmp_dir/manual-schema-mismatch-validation.json" \
  2>"$tmp_dir/manual-schema-mismatch-validation.err"
manual_schema_mismatch_status=$?
set -e

if [[ "$manual_schema_mismatch_status" -eq 0 ]]; then
  echo "manual UX validator accepted a machine artifact schema that does not match the referenced file" >&2
  cat "$tmp_dir/manual-schema-mismatch-validation.json" >&2
  exit 1
fi

node -e '
  const fs = require("node:fs");
  const validation = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  if (!Array.isArray(validation.errors) || !validation.errors.includes("machine_artifact.schema must match machine_artifact.path")) {
    console.error("manual UX validator must report machine artifact schema mismatch");
    process.exit(1);
  }
' "$tmp_dir/manual-schema-mismatch-validation.json"

cp "$tmp_dir/hosted-valid.json" "$tmp_dir/hosted-valid-copy.json"
cat >"$tmp_dir/manual-hosted-copy-path.json" <<JSON
{
  "schema": "elastos.browser.manual-ux/v1",
  "ok": true,
  "reviewed_at": "2026-05-13T00:00:00Z",
  "reviewer": "objective-audit-smoke",
  "provider": "fake-hosted-valid",
  "target": "test",
  "machine_artifact": {
    "schema": "elastos.browser.hosted-provider-bakeoff/v1",
    "sha256": "$hosted_valid_sha",
    "path": "$tmp_dir/hosted-valid-copy.json"
  },
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
  },
  "evidence": {
    "display_session_audio_advertised": "display session reported audio=true",
    "audio_unlock_gesture": "clicked render panel and playback unlocked",
    "remote_audio_unmuted_status": "remote video was unmuted with volume=1",
    "received_audio_evidence": "WebRTC stats showed received audio bytes"
  }
}
JSON

node "$repo_root/scripts/browser-manual-ux-report.mjs" --input "$tmp_dir/manual-hosted-copy-path.json" >/dev/null

set +e
node "$repo_root/scripts/browser-objective-audit.mjs" \
  --hosted-bakeoff "$tmp_dir/hosted-valid.json" \
  --manual-ux "$tmp_dir/manual-hosted-copy-path.json" \
  >"$tmp_dir/manual-copy-path-audit.json" \
  2>"$tmp_dir/manual-copy-path-audit.err"
manual_copy_path_status=$?
set -e

if [[ "$manual_copy_path_status" -eq 0 ]]; then
  echo "objective audit accepted manual UX evidence pointing to a copied machine artifact instead of the accepted artifact path" >&2
  cat "$tmp_dir/manual-copy-path-audit.json" >&2
  exit 1
fi

set +e
node "$repo_root/scripts/browser-objective-audit.mjs" \
  --hosted-bakeoff "$tmp_dir/hosted-valid.json" \
  --manual-ux "$tmp_dir/manual-hosted-detached.json" \
  >"$tmp_dir/manual-detached-audit.json" \
  2>"$tmp_dir/manual-detached-audit.err"
manual_detached_status=$?
set -e

if [[ "$manual_detached_status" -eq 0 ]]; then
  echo "objective audit accepted manual UX evidence with a detached machine artifact hash" >&2
  cat "$tmp_dir/manual-detached-audit.json" >&2
  exit 1
fi

cat >"$tmp_dir/manual-hosted-missing-audio-unlock.json" <<JSON
{
  "schema": "elastos.browser.manual-ux/v1",
  "ok": true,
  "reviewed_at": "2026-05-13T00:00:00Z",
  "reviewer": "objective-audit-smoke",
  "provider": "fake-hosted-valid",
  "target": "test",
  "machine_artifact": {
    "schema": "elastos.browser.hosted-provider-bakeoff/v1",
    "sha256": "$hosted_valid_sha",
    "path": "$tmp_dir/hosted-valid.json"
  },
  "checks": {
    "typing_latency": true,
    "address_bar_stability": true,
    "scrolling_click_fidelity": true,
    "youtube_audible_audio": true,
    "glide_wallet_connect": true,
    "no_raw_authority": true,
    "session_cleanup": true
  }
}
JSON

set +e
node "$repo_root/scripts/browser-manual-ux-report.mjs" \
  --input "$tmp_dir/manual-hosted-missing-audio-unlock.json" \
  >"$tmp_dir/manual-missing-audio-unlock-validation.json" \
  2>"$tmp_dir/manual-missing-audio-unlock-validation.err"
manual_missing_audio_unlock_status=$?
set -e

if [[ "$manual_missing_audio_unlock_status" -eq 0 ]]; then
  echo "manual UX validator accepted hosted WebRTC evidence without explicit audio-unlock checks" >&2
  cat "$tmp_dir/manual-missing-audio-unlock-validation.json" >&2
  exit 1
fi

node -e '
  const fs = require("node:fs");
  const validation = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  const required = [
    "checks.display_session_audio_advertised must be true",
    "checks.audio_unlock_gesture must be true",
    "checks.remote_audio_unmuted_status must be true",
    "checks.received_audio_evidence must be true"
  ];
  for (const message of required) {
    if (!validation.errors?.includes(message)) {
      console.error(`manual UX validator must require hosted WebRTC audio evidence: ${message}`);
      process.exit(1);
    }
  }
' "$tmp_dir/manual-missing-audio-unlock-validation.json"

cat >"$tmp_dir/manual-hosted-missing-audio-evidence.json" <<JSON
{
  "schema": "elastos.browser.manual-ux/v1",
  "ok": true,
  "reviewed_at": "2026-05-13T00:00:00Z",
  "reviewer": "objective-audit-smoke",
  "provider": "fake-hosted-valid",
  "target": "test",
  "machine_artifact": {
    "schema": "elastos.browser.hosted-provider-bakeoff/v1",
    "sha256": "$hosted_valid_sha",
    "path": "$tmp_dir/hosted-valid.json"
  },
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
JSON

set +e
node "$repo_root/scripts/browser-objective-audit.mjs" \
  --hosted-bakeoff "$tmp_dir/hosted-valid.json" \
  --manual-ux "$tmp_dir/manual-hosted-missing-audio-evidence.json" \
  >"$tmp_dir/manual-missing-audio-evidence-audit.json" \
  2>"$tmp_dir/manual-missing-audio-evidence-audit.err"
manual_missing_audio_evidence_status=$?
set -e

if [[ "$manual_missing_audio_evidence_status" -eq 0 ]]; then
  echo "objective audit accepted hosted manual UX checkmarks without text-backed audio evidence" >&2
  cat "$tmp_dir/manual-missing-audio-evidence-audit.json" >&2
  exit 1
fi

cat >"$tmp_dir/manual-hosted-stale-check.json" <<JSON
{
  "schema": "elastos.browser.manual-ux/v1",
  "ok": true,
  "reviewed_at": "2026-05-13T00:00:00Z",
  "reviewer": "objective-audit-smoke",
  "provider": "fake-hosted-valid",
  "target": "test",
  "machine_artifact": {
    "schema": "elastos.browser.hosted-provider-bakeoff/v1",
    "sha256": "$hosted_valid_sha",
    "path": "$tmp_dir/hosted-valid.json"
  },
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
    "received_audio_evidence": true,
    "legacy_frame_preview_audio": true
  },
  "evidence": {
    "display_session_audio_advertised": "display session reported audio=true",
    "audio_unlock_gesture": "clicked render panel and playback unlocked",
    "remote_audio_unmuted_status": "remote video was unmuted with volume=1",
    "received_audio_evidence": "WebRTC stats showed received audio bytes",
    "legacy_frame_preview_audio": "stale fallback field"
  }
}
JSON

set +e
node "$repo_root/scripts/browser-manual-ux-report.mjs" \
  --input "$tmp_dir/manual-hosted-stale-check.json" \
  >"$tmp_dir/manual-stale-check-validation.json" \
  2>"$tmp_dir/manual-stale-check-validation.err"
manual_stale_check_status=$?
set -e

if [[ "$manual_stale_check_status" -eq 0 ]]; then
  echo "manual UX validator accepted stale hosted check/evidence fields" >&2
  cat "$tmp_dir/manual-stale-check-validation.json" >&2
  exit 1
fi

node -e '
  const fs = require("node:fs");
  const validation = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  const required = [
    "checks.legacy_frame_preview_audio is not valid for machine_artifact.schema",
    "evidence.legacy_frame_preview_audio is not valid for machine_artifact.schema"
  ];
  for (const message of required) {
    if (!validation.errors?.includes(message)) {
      console.error(`manual UX validator must reject stale manual UX fields: ${message}`);
      process.exit(1);
    }
  }
' "$tmp_dir/manual-stale-check-validation.json"

set +e
node "$repo_root/scripts/browser-objective-audit.mjs" \
  --hosted-bakeoff "$tmp_dir/hosted-valid.json" \
  --manual-ux "$tmp_dir/manual-hosted-stale-check.json" \
  >"$tmp_dir/manual-stale-check-audit.json" \
  2>"$tmp_dir/manual-stale-check-audit.err"
manual_stale_check_audit_status=$?
set -e

if [[ "$manual_stale_check_audit_status" -eq 0 ]]; then
  echo "objective audit accepted stale hosted check/evidence fields" >&2
  cat "$tmp_dir/manual-stale-check-audit.json" >&2
  exit 1
fi

cat >"$tmp_dir/manual-hosted-matched.json" <<JSON
{
  "schema": "elastos.browser.manual-ux/v1",
  "ok": true,
  "reviewed_at": "2026-05-13T00:00:00Z",
  "reviewer": "objective-audit-smoke",
  "provider": "fake-hosted-valid",
  "target": "test",
  "machine_artifact": {
    "schema": "elastos.browser.hosted-provider-bakeoff/v1",
    "sha256": "$hosted_valid_sha",
    "path": "$tmp_dir/hosted-valid.json"
  },
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
  },
  "evidence": {
    "display_session_audio_advertised": "display session reported audio=true",
    "audio_unlock_gesture": "clicked render panel and playback unlocked",
    "remote_audio_unmuted_status": "remote video was unmuted with volume=1",
    "received_audio_evidence": "WebRTC stats showed received audio bytes"
  }
}
JSON

node "$repo_root/scripts/browser-manual-ux-report.mjs" --input "$tmp_dir/manual-hosted-matched.json" >/dev/null
node "$repo_root/scripts/browser-objective-audit.mjs" \
  --hosted-bakeoff "$tmp_dir/hosted-valid.json" \
  --manual-ux "$tmp_dir/manual-hosted-matched.json" \
  >"$tmp_dir/manual-matched-audit.json"

node -e '
  const fs = require("node:fs");
  const audit = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  if (audit.ok !== true || audit.product_provider_accepted !== true) {
    console.error("objective audit must accept a strict hosted artifact with matching manual UX hash");
    process.exit(1);
  }
  const checklist = audit.prompt_to_artifact_checklist || [];
  for (const id of ["best_path_determined", "audio_product_proven", "no_fake_fallbacks", "planned_and_iterated", "manual_user_acceptance"]) {
    const item = checklist.find((candidate) => candidate.id === id);
    if (!item || item.ok !== true) {
      console.error(`strict accepted proof must satisfy checklist item ${id}`);
      process.exit(1);
    }
  }
' "$tmp_dir/manual-matched-audit.json"

printf '{"schema":"elastos.browser.objective-audit-smoke/v1","ok":true,"declared_only_native_media_rejected":true,"shallow_hosted_ok_rejected":true,"skipped_youtube_rejected":true,"missing_resize_gate_rejected":true,"planned_evidence_is_durable":true,"manual_template_prefilled":true,"manual_hash_mismatch_rejected":true,"manual_schema_mismatch_rejected":true,"manual_artifact_path_mismatch_rejected":true,"detached_manual_ux_rejected":true,"hosted_manual_audio_unlock_required":true,"hosted_manual_audio_evidence_required":true,"stale_manual_fields_rejected":true,"matched_manual_ux_accepted":true}\n'
