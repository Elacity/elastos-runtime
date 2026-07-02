#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

find_node() {
  if [[ -n "${ELASTOS_NODE_BIN:-}" && -x "${ELASTOS_NODE_BIN}" ]]; then
    printf '%s\n' "${ELASTOS_NODE_BIN}"
    return 0
  fi
  if command -v node >/dev/null 2>&1; then
    command -v node
    return 0
  fi
  local bundled="${HOME}/.elastos/node/node-v22.13.1-darwin-arm64/bin/node"
  if [[ -x "$bundled" ]]; then
    printf '%s\n' "$bundled"
    return 0
  fi
  return 1
}

node_bin="$(find_node || true)"
if [[ -z "$node_bin" ]]; then
  echo "node not found. Install Node or set ELASTOS_NODE_BIN to an executable node binary." >&2
  exit 2
fi
tmp_dir="$(mktemp -d /tmp/elastos-browser-mac-vm-manual-ux-smoke-XXXXXX)"
cleanup() {
  rm -rf "$tmp_dir"
}
trap cleanup EXIT

cat >"$tmp_dir/mac-valid.json" <<'JSON'
{
  "schema": "elastos.browser.mac-vm-proof/v1",
  "ok": true,
  "target": "mac-source-home",
  "generated_at": "2026-06-19T00:00:00.000Z",
  "home": {
    "url": "http://localhost:61180/apps/home/",
    "http_code": 200,
    "hash_parity": true
  },
  "profile_reset": {
    "requested": true,
    "ok": true,
    "receipt": {
      "schema": "elastos.browser.profile-reset/v1",
      "status": "ok",
      "profile": {
        "scope": "active_principal",
        "storage": "principal_owned_profile_disk",
        "storage_posture": "principal_owned_reset_scoped_unprotected",
        "protected_storage": false,
        "encrypted": false,
        "recoverable": false,
        "recovery": "not_recovery_kit_packaged",
        "reset": "whole_profile",
        "uri": "localhost://Users/self/BrowserProfiles/default/profile.ext4"
      },
      "removed_profile_disk": true
    }
  },
  "vm_control": {
    "restart": {
      "schema": "elastos.browser.mac-vm-control-restart/v1",
      "fresh_after_restart": true,
      "max_uptime_ms": 300000,
      "actual_uptime_ms": 61000
    },
    "after": {
      "ok": true,
      "started_at": "2026-06-18T23:59:00.000Z",
      "uptime_ms": 61000,
      "active_pages": 0,
      "pending_launches": 0,
      "network_mode": "runtime_net_only",
      "direct_network": false
    }
  },
  "embedded_video_input": {
    "ok": true,
    "display_mode": "webrtc_remote_display",
    "display_session": {
      "schema": "elastos.browser.display-session/v1",
      "mode": "webrtc_remote_display",
      "media_transport": "runtime_relay",
      "display_backend": "browser-vm",
      "backend_class": "vm_compositor",
      "offerer": "runtime",
      "turn_ice_server_count": 1,
      "credentialed_turn_ice_server_count": 1
    },
    "vm_isolation": {
      "schema": "elastos.browser.engine.identity/v1",
      "adapter": "browser-vm-product",
      "engine": "chromium_microvm",
      "display_mode": "webrtc_remote_display",
      "guarantee_level": "mechanism_microvm",
      "engine_control": "page_scoped",
      "isolated_engine_session": true,
      "isolation_kind": "per_launch_vm_target"
    },
    "remote_video_ready_ms": 4500,
    "decoded_frame_delta": 24,
    "dropped_frame_delta": 0,
    "navigation": {
      "requested_url": "https://ela.city/channels",
      "actual_url": "https://ela.city/channels",
      "address_value": "https://ela.city/channels"
    },
    "click_navigation": {
      "ok": true,
      "skipped": false,
      "expected_url_re": "https://ela[.]city/explore",
      "address_value": "https://ela.city/explore",
      "status": {
        "actual_url": "https://ela.city/explore"
      }
    }
  },
  "page_diagnostics": {
    "ok": true,
    "url": "https://ela.city/channels",
    "visible_image_count": 49,
    "visible_broken_image_count": 0,
    "visible_pending_image_count": 0,
    "diagnostic_click_actions": [{
      "ok": true,
      "target": {
        "text": "Edit Profile",
        "aria_label": "",
        "title": "",
        "test_id": ""
      },
      "input": {
        "accepted": true
      },
      "diagnostics": {
        "dialog_elements": [{
          "text": "Edit Profile",
          "aria_label": "",
          "title": "",
          "test_id": ""
        }]
      }
    }]
  },
  "quality_gates": {
    "ok": true,
    "performance": {
      "ok": true,
      "checks": {
        "remote_video_ready_ms_within_threshold": true,
        "embedded_navigation_ms_within_threshold": true,
        "diagnostics_navigation_ms_within_threshold": true,
        "decoded_frame_delta_ok": true,
        "dropped_frame_delta_ok": true
      }
    },
    "zoom": {
      "ok": true,
      "checks": {
        "device_pixel_ratio_ok": true,
        "viewport_width_ok": true,
        "viewport_height_ok": true,
        "panel_aspect_matches_viewport": true,
        "initial_video_matches_panel": true,
        "after_navigation_video_matches_panel": true,
        "source_video_matches_panel": true
      },
      "device_pixel_ratio": 1,
      "viewport_width": 1280,
      "viewport_height": 720
    }
  },
  "manual_acceptance": {
    "status": "not_recorded"
  }
}
JSON

"$node_bin" -e '
  const fs = require("node:fs");
  const proof = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  proof.quality_gates.thresholds = {
    ...(proof.quality_gates.thresholds || {}),
    expected_viewport_width: 1000,
    expected_viewport_height: 700,
  };
  proof.quality_gates.zoom.viewport_width = 1000;
  proof.quality_gates.zoom.viewport_height = 700;
  fs.writeFileSync(process.argv[2], `${JSON.stringify(proof, null, 2)}\n`);
' "$tmp_dir/mac-valid.json" "$tmp_dir/mac-resized.json"

"$node_bin" "$repo_root/scripts/browser-manual-ux-report.mjs" \
  --template \
  --machine-artifact "$tmp_dir/mac-valid.json" \
  >"$tmp_dir/manual-template-mac.json"
"$node_bin" "$repo_root/scripts/browser-manual-ux-report.mjs" \
  --template \
  --machine-artifact "$tmp_dir/mac-resized.json" \
  >"$tmp_dir/manual-template-mac-resized.json"

"$node_bin" -e '
  const fs = require("node:fs");
  const crypto = require("node:crypto");
  const template = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  const artifactPath = process.argv[2];
  const expectedSha = crypto.createHash("sha256").update(fs.readFileSync(artifactPath)).digest("hex");
  if (template.provider !== "mac-vm") {
    console.error(`Mac VM manual UX template must prefill provider=mac-vm, got ${template.provider}`);
    process.exit(1);
  }
  if (template.target !== "mac-source-home") {
    console.error(`Mac VM manual UX template must prefill target=mac-source-home, got ${template.target}`);
    process.exit(1);
  }
  if (template.machine_artifact?.schema !== "elastos.browser.mac-vm-proof/v1") {
    console.error("Mac VM manual UX template must prefill the Mac VM proof schema");
    process.exit(1);
  }
  if (template.machine_artifact?.sha256 !== expectedSha) {
    console.error("Mac VM manual UX template must prefill the machine artifact sha256");
    process.exit(1);
  }
  for (const key of ["mac_gateway_restarted", "remote_video_visible", "performance_thresholds_reviewed", "zoom_geometry_reviewed", "ela_city_url_sync", "ela_city_images_loaded", "ela_city_edit_profile_modal", "no_raw_authority", "session_cleanup"]) {
    if (template.checks?.[key] !== false) {
      console.error(`Mac VM manual UX template missing check ${key}`);
      process.exit(1);
    }
    if (template.evidence?.[key] !== "") {
      console.error(`Mac VM manual UX template must start ${key} evidence empty`);
      process.exit(1);
    }
  }
  if ("youtube_audible_audio" in (template.checks || {})) {
    console.error("Mac VM manual UX template must not inherit hosted/native product-audio checks");
    process.exit(1);
  }
  if (!Array.isArray(template.review_artifacts) || template.review_artifacts.length !== 0) {
    console.error("Mac VM manual UX template must start with an empty review_artifacts array");
    process.exit(1);
  }
' "$tmp_dir/manual-template-mac.json" "$tmp_dir/mac-valid.json"

cat >"$tmp_dir/mac-shallow.json" <<'JSON'
{
  "schema": "elastos.browser.mac-vm-proof/v1",
  "ok": true
}
JSON

shallow_sha="$("$node_bin" -e 'const fs = require("node:fs"); const crypto = require("node:crypto"); process.stdout.write(crypto.createHash("sha256").update(fs.readFileSync(process.argv[1])).digest("hex"));' "$tmp_dir/mac-shallow.json")"
"$node_bin" -e '
  const fs = require("node:fs");
  const proof = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  delete proof.quality_gates.performance.checks;
  delete proof.quality_gates.zoom.checks;
  fs.writeFileSync(process.argv[2], `${JSON.stringify(proof, null, 2)}\n`);
' "$tmp_dir/mac-valid.json" "$tmp_dir/mac-aggregate-only.json"
aggregate_only_sha="$("$node_bin" -e 'const fs = require("node:fs"); const crypto = require("node:crypto"); process.stdout.write(crypto.createHash("sha256").update(fs.readFileSync(process.argv[1])).digest("hex"));' "$tmp_dir/mac-aggregate-only.json")"
"$node_bin" -e '
  const fs = require("node:fs");
  const proof = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  proof.embedded_video_input.display_session.media_transport = "direct";
  proof.embedded_video_input.display_session.credentialed_turn_ice_server_count = 0;
  fs.writeFileSync(process.argv[2], `${JSON.stringify(proof, null, 2)}\n`);
' "$tmp_dir/mac-valid.json" "$tmp_dir/mac-missing-runtime-media-relay.json"
missing_runtime_media_relay_sha="$("$node_bin" -e 'const fs = require("node:fs"); const crypto = require("node:crypto"); process.stdout.write(crypto.createHash("sha256").update(fs.readFileSync(process.argv[1])).digest("hex"));' "$tmp_dir/mac-missing-runtime-media-relay.json")"
"$node_bin" -e '
  const fs = require("node:fs");
  const proof = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  proof.vm_control.after.uptime_ms = 900000;
  proof.vm_control.restart.fresh_after_restart = false;
  proof.vm_control.restart.actual_uptime_ms = 900000;
  fs.writeFileSync(process.argv[2], `${JSON.stringify(proof, null, 2)}\n`);
' "$tmp_dir/mac-valid.json" "$tmp_dir/mac-stale-restart.json"
stale_restart_sha="$("$node_bin" -e 'const fs = require("node:fs"); const crypto = require("node:crypto"); process.stdout.write(crypto.createHash("sha256").update(fs.readFileSync(process.argv[1])).digest("hex"));' "$tmp_dir/mac-stale-restart.json")"
"$node_bin" -e '
  const fs = require("node:fs");
  const proof = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  proof.profile_reset = {
    requested: false,
    ok: false,
    receipt: null,
  };
  fs.writeFileSync(process.argv[2], `${JSON.stringify(proof, null, 2)}\n`);
' "$tmp_dir/mac-valid.json" "$tmp_dir/mac-missing-profile-reset.json"
missing_profile_reset_sha="$("$node_bin" -e 'const fs = require("node:fs"); const crypto = require("node:crypto"); process.stdout.write(crypto.createHash("sha256").update(fs.readFileSync(process.argv[1])).digest("hex"));' "$tmp_dir/mac-missing-profile-reset.json")"
"$node_bin" -e '
  const fs = require("node:fs");
  const proof = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  proof.profile_reset.receipt.profile.profile_key = "profile-secret";
  proof.profile_reset.receipt.profile.principal_id = "person:local:secret";
  proof.profile_reset.receipt.profile.disk_path = "/Users/operator/elastos/Users/0123456789ab/BrowserProfiles/default/profile.ext4";
  fs.writeFileSync(process.argv[2], `${JSON.stringify(proof, null, 2)}\n`);
' "$tmp_dir/mac-valid.json" "$tmp_dir/mac-leaky-profile-reset.json"
leaky_profile_reset_sha="$("$node_bin" -e 'const fs = require("node:fs"); const crypto = require("node:crypto"); process.stdout.write(crypto.createHash("sha256").update(fs.readFileSync(process.argv[1])).digest("hex"));' "$tmp_dir/mac-leaky-profile-reset.json")"
"$node_bin" -e '
  const fs = require("node:fs");
  const proof = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  proof.profile_reset.receipt.removed_profile_disk = false;
  fs.writeFileSync(process.argv[2], `${JSON.stringify(proof, null, 2)}\n`);
' "$tmp_dir/mac-valid.json" "$tmp_dir/mac-reset-without-removal.json"
reset_without_removal_sha="$("$node_bin" -e 'const fs = require("node:fs"); const crypto = require("node:crypto"); process.stdout.write(crypto.createHash("sha256").update(fs.readFileSync(process.argv[1])).digest("hex"));' "$tmp_dir/mac-reset-without-removal.json")"
"$node_bin" -e '
  const fs = require("node:fs");
  const proof = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  proof.embedded_video_input.click_navigation.expected_url_re = "https://ela[.]city/channels";
  proof.embedded_video_input.click_navigation.address_value = "https://ela.city/channels";
  proof.embedded_video_input.click_navigation.status.actual_url = "https://ela.city/channels";
  fs.writeFileSync(process.argv[2], `${JSON.stringify(proof, null, 2)}\n`);
' "$tmp_dir/mac-valid.json" "$tmp_dir/mac-url-unchanged.json"
url_unchanged_sha="$("$node_bin" -e 'const fs = require("node:fs"); const crypto = require("node:crypto"); process.stdout.write(crypto.createHash("sha256").update(fs.readFileSync(process.argv[1])).digest("hex"));' "$tmp_dir/mac-url-unchanged.json")"
"$node_bin" -e '
  const fs = require("node:fs");
  const proof = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  proof.page_diagnostics.diagnostic_click_actions = [];
  fs.writeFileSync(process.argv[2], `${JSON.stringify(proof, null, 2)}\n`);
' "$tmp_dir/mac-valid.json" "$tmp_dir/mac-missing-edit-profile-diagnostic.json"
missing_edit_profile_diagnostic_sha="$("$node_bin" -e 'const fs = require("node:fs"); const crypto = require("node:crypto"); process.stdout.write(crypto.createHash("sha256").update(fs.readFileSync(process.argv[1])).digest("hex"));' "$tmp_dir/mac-missing-edit-profile-diagnostic.json")"
cat >"$tmp_dir/manual-mac-shallow.json" <<JSON
{
  "schema": "elastos.browser.manual-ux/v1",
  "ok": true,
  "reviewed_at": "2026-06-19T00:00:00Z",
  "reviewer": "mac-vm-manual-ux-smoke",
  "provider": "mac-vm",
  "target": "mac-source-home",
  "machine_artifact": {
    "schema": "elastos.browser.mac-vm-proof/v1",
    "sha256": "$shallow_sha",
    "path": "$tmp_dir/mac-shallow.json"
  },
  "checks": {
    "mac_gateway_restarted": true,
    "remote_video_visible": true,
    "typing_latency": true,
    "address_bar_stability": true,
    "scrolling_click_fidelity": true,
    "performance_thresholds_reviewed": true,
    "zoom_geometry_reviewed": true,
    "ela_city_url_sync": true,
    "ela_city_images_loaded": true,
    "ela_city_edit_profile_modal": true,
    "no_raw_authority": true,
    "session_cleanup": true
  },
  "evidence": {
    "mac_gateway_restarted": "gateway was restarted before the proof",
    "remote_video_visible": "remote WebRTC video was visible",
    "typing_latency": "typing felt responsive",
    "address_bar_stability": "address bar stayed stable",
    "scrolling_click_fidelity": "scrolling and clicks matched the page",
    "performance_thresholds_reviewed": "machine quality_gates.performance.ok was reviewed",
    "zoom_geometry_reviewed": "machine quality_gates.zoom.ok was reviewed",
    "ela_city_url_sync": "ela.city address/status URL sync was observed",
    "ela_city_images_loaded": "visible ela.city images were settled",
    "ela_city_edit_profile_modal": "edit profile modal opened",
    "no_raw_authority": "no raw authority was exposed",
    "session_cleanup": "session cleanup was observed"
  }
}
JSON

cat >"$tmp_dir/manual-mac-aggregate-only.json" <<JSON
{
  "schema": "elastos.browser.manual-ux/v1",
  "ok": true,
  "reviewed_at": "2026-06-19T00:00:00Z",
  "reviewer": "mac-vm-manual-ux-smoke",
  "provider": "mac-vm",
  "target": "mac-source-home",
  "machine_artifact": {
    "schema": "elastos.browser.mac-vm-proof/v1",
    "sha256": "$aggregate_only_sha",
    "path": "$tmp_dir/mac-aggregate-only.json"
  },
  "checks": {
    "mac_gateway_restarted": true,
    "remote_video_visible": true,
    "typing_latency": true,
    "address_bar_stability": true,
    "scrolling_click_fidelity": true,
    "performance_thresholds_reviewed": true,
    "zoom_geometry_reviewed": true,
    "ela_city_url_sync": true,
    "ela_city_images_loaded": true,
    "ela_city_edit_profile_modal": true,
    "no_raw_authority": true,
    "session_cleanup": true
  },
  "evidence": {
    "mac_gateway_restarted": "gateway was restarted before the proof",
    "remote_video_visible": "remote WebRTC video was visible",
    "typing_latency": "typing felt responsive",
    "address_bar_stability": "address bar stayed stable",
    "scrolling_click_fidelity": "scrolling and clicks matched the page",
    "performance_thresholds_reviewed": "machine quality_gates.performance.checks were reviewed",
    "zoom_geometry_reviewed": "machine quality_gates.zoom.checks were reviewed",
    "ela_city_url_sync": "ela.city address/status URL sync was observed",
    "ela_city_images_loaded": "visible ela.city images were settled",
    "ela_city_edit_profile_modal": "edit profile modal opened",
    "no_raw_authority": "no raw authority was exposed",
    "session_cleanup": "session cleanup was observed"
  }
}
JSON

"$node_bin" -e '
  const fs = require("node:fs");
  const [
    baseReportPath,
    missingRelayOut,
    missingRelaySha,
    missingRelayArtifactPath,
    missingOut,
    missingSha,
    missingArtifactPath,
    resetWithoutRemovalOut,
    resetWithoutRemovalSha,
    resetWithoutRemovalArtifactPath,
    leakyOut,
    leakySha,
    leakyArtifactPath,
    staleOut,
    staleSha,
    staleArtifactPath,
  ] = process.argv.slice(1);
  const base = JSON.parse(fs.readFileSync(baseReportPath, "utf8"));
  for (const [out, sha256, path] of [
    [missingRelayOut, missingRelaySha, missingRelayArtifactPath],
    [missingOut, missingSha, missingArtifactPath],
    [resetWithoutRemovalOut, resetWithoutRemovalSha, resetWithoutRemovalArtifactPath],
    [leakyOut, leakySha, leakyArtifactPath],
    [staleOut, staleSha, staleArtifactPath],
  ]) {
    const report = structuredClone(base);
    report.machine_artifact.sha256 = sha256;
    report.machine_artifact.path = path;
    fs.writeFileSync(out, `${JSON.stringify(report, null, 2)}\n`);
  }
' "$tmp_dir/manual-mac-aggregate-only.json" \
  "$tmp_dir/manual-mac-missing-runtime-media-relay.json" \
  "$missing_runtime_media_relay_sha" \
  "$tmp_dir/mac-missing-runtime-media-relay.json" \
  "$tmp_dir/manual-mac-missing-profile-reset.json" \
  "$missing_profile_reset_sha" \
  "$tmp_dir/mac-missing-profile-reset.json" \
  "$tmp_dir/manual-mac-reset-without-removal.json" \
  "$reset_without_removal_sha" \
  "$tmp_dir/mac-reset-without-removal.json" \
  "$tmp_dir/manual-mac-leaky-profile-reset.json" \
  "$leaky_profile_reset_sha" \
  "$tmp_dir/mac-leaky-profile-reset.json" \
  "$tmp_dir/manual-mac-stale-restart.json" \
  "$stale_restart_sha" \
  "$tmp_dir/mac-stale-restart.json"

"$node_bin" -e '
  const fs = require("node:fs");
  const [baseReportPath, outPath, sha256, artifactPath] = process.argv.slice(1);
  const report = JSON.parse(fs.readFileSync(baseReportPath, "utf8"));
  report.machine_artifact.sha256 = sha256;
  report.machine_artifact.path = artifactPath;
  fs.writeFileSync(outPath, `${JSON.stringify(report, null, 2)}\n`);
' "$tmp_dir/manual-mac-aggregate-only.json" \
  "$tmp_dir/manual-mac-url-unchanged.json" \
  "$url_unchanged_sha" \
  "$tmp_dir/mac-url-unchanged.json"

"$node_bin" -e '
  const fs = require("node:fs");
  const [baseReportPath, outPath, sha256, artifactPath] = process.argv.slice(1);
  const report = JSON.parse(fs.readFileSync(baseReportPath, "utf8"));
  report.machine_artifact.sha256 = sha256;
  report.machine_artifact.path = artifactPath;
  fs.writeFileSync(outPath, `${JSON.stringify(report, null, 2)}\n`);
' "$tmp_dir/manual-mac-aggregate-only.json" \
  "$tmp_dir/manual-mac-missing-edit-profile-diagnostic.json" \
  "$missing_edit_profile_diagnostic_sha" \
  "$tmp_dir/mac-missing-edit-profile-diagnostic.json"

set +e
"$node_bin" "$repo_root/scripts/browser-manual-ux-report.mjs" \
  --input "$tmp_dir/manual-mac-shallow.json" \
  >"$tmp_dir/manual-mac-shallow-validation.json" \
  2>"$tmp_dir/manual-mac-shallow-validation.err"
shallow_status=$?
set -e
if [[ "$shallow_status" -eq 0 ]]; then
  echo "manual UX validator accepted shallow Mac VM proof" >&2
  cat "$tmp_dir/manual-mac-shallow-validation.json" >&2
  exit 1
fi

"$node_bin" -e '
  const fs = require("node:fs");
  const validation = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  if (!validation.errors?.some((error) => error.includes("successful Mac VM proof"))) {
    console.error("manual UX validator must reject shallow Mac VM artifacts");
    process.exit(1);
  }
' "$tmp_dir/manual-mac-shallow-validation.json"

set +e
"$node_bin" "$repo_root/scripts/browser-manual-ux-report.mjs" \
  --input "$tmp_dir/manual-mac-aggregate-only.json" \
  >"$tmp_dir/manual-mac-aggregate-only-validation.json" \
  2>"$tmp_dir/manual-mac-aggregate-only-validation.err"
aggregate_only_status=$?
set -e
if [[ "$aggregate_only_status" -eq 0 ]]; then
  echo "manual UX validator accepted aggregate-only Mac VM proof" >&2
  cat "$tmp_dir/manual-mac-aggregate-only-validation.json" >&2
  exit 1
fi

"$node_bin" -e '
  const fs = require("node:fs");
  const validation = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  if (!validation.errors?.some((error) => error.includes("successful Mac VM proof"))) {
    console.error("manual UX validator must reject aggregate-only Mac VM artifacts");
    process.exit(1);
  }
' "$tmp_dir/manual-mac-aggregate-only-validation.json"

set +e
"$node_bin" "$repo_root/scripts/browser-manual-ux-report.mjs" \
  --input "$tmp_dir/manual-mac-url-unchanged.json" \
  >"$tmp_dir/manual-mac-url-unchanged-validation.json" \
  2>"$tmp_dir/manual-mac-url-unchanged-validation.err"
url_unchanged_status=$?
set -e
if [[ "$url_unchanged_status" -eq 0 ]]; then
  echo "manual UX validator accepted Mac VM proof without changed ela.city URL-sync evidence" >&2
  cat "$tmp_dir/manual-mac-url-unchanged-validation.json" >&2
  exit 1
fi

"$node_bin" -e '
  const fs = require("node:fs");
  const validation = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  if (!validation.errors?.some((error) => error.includes("changed click URL sync"))) {
    console.error("manual UX validator must reject Mac VM artifacts whose click URL sync did not change from the starting URL");
    process.exit(1);
  }
' "$tmp_dir/manual-mac-url-unchanged-validation.json"

set +e
"$node_bin" "$repo_root/scripts/browser-manual-ux-report.mjs" \
  --input "$tmp_dir/manual-mac-missing-runtime-media-relay.json" \
  >"$tmp_dir/manual-mac-missing-runtime-media-relay-validation.json" \
  2>"$tmp_dir/manual-mac-missing-runtime-media-relay-validation.err"
missing_runtime_media_relay_status=$?
set -e
if [[ "$missing_runtime_media_relay_status" -eq 0 ]]; then
  echo "manual UX validator accepted Mac VM proof without Runtime media relay evidence" >&2
  cat "$tmp_dir/manual-mac-missing-runtime-media-relay-validation.json" >&2
  exit 1
fi

"$node_bin" -e '
  const fs = require("node:fs");
  const validation = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  if (!validation.errors?.some((error) => error.includes("Runtime media relay proof"))) {
    console.error("manual UX validator must reject Mac VM artifacts without Runtime media relay evidence");
    process.exit(1);
  }
' "$tmp_dir/manual-mac-missing-runtime-media-relay-validation.json"

set +e
"$node_bin" "$repo_root/scripts/browser-manual-ux-report.mjs" \
  --input "$tmp_dir/manual-mac-missing-edit-profile-diagnostic.json" \
  >"$tmp_dir/manual-mac-missing-edit-profile-diagnostic-validation.json" \
  2>"$tmp_dir/manual-mac-missing-edit-profile-diagnostic-validation.err"
missing_edit_profile_diagnostic_status=$?
set -e
if [[ "$missing_edit_profile_diagnostic_status" -eq 0 ]]; then
  echo "manual UX validator accepted Mac VM proof without edit-profile diagnostic click evidence" >&2
  cat "$tmp_dir/manual-mac-missing-edit-profile-diagnostic-validation.json" >&2
  exit 1
fi

"$node_bin" -e '
  const fs = require("node:fs");
  const validation = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  if (!validation.errors?.some((error) => error.includes("edit-profile diagnostic click proof"))) {
    console.error("manual UX validator must reject Mac VM artifacts without edit-profile diagnostic click evidence");
    process.exit(1);
  }
' "$tmp_dir/manual-mac-missing-edit-profile-diagnostic-validation.json"

set +e
"$node_bin" "$repo_root/scripts/browser-manual-ux-report.mjs" \
  --input "$tmp_dir/manual-mac-missing-profile-reset.json" \
  >"$tmp_dir/manual-mac-missing-profile-reset-validation.json" \
  2>"$tmp_dir/manual-mac-missing-profile-reset-validation.err"
missing_profile_reset_status=$?
set -e
if [[ "$missing_profile_reset_status" -eq 0 ]]; then
  echo "manual UX validator accepted Mac VM proof without profile reset evidence" >&2
  cat "$tmp_dir/manual-mac-missing-profile-reset-validation.json" >&2
  exit 1
fi

"$node_bin" -e '
  const fs = require("node:fs");
  const validation = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  if (!validation.errors?.some((error) => error.includes("safe profile reset proof"))) {
    console.error("manual UX validator must reject Mac VM artifacts without profile reset evidence");
    process.exit(1);
  }
' "$tmp_dir/manual-mac-missing-profile-reset-validation.json"

set +e
"$node_bin" "$repo_root/scripts/browser-manual-ux-report.mjs" \
  --input "$tmp_dir/manual-mac-reset-without-removal.json" \
  >"$tmp_dir/manual-mac-reset-without-removal-validation.json" \
  2>"$tmp_dir/manual-mac-reset-without-removal-validation.err"
reset_without_removal_status=$?
set -e
if [[ "$reset_without_removal_status" -eq 0 ]]; then
  echo "manual UX validator accepted Mac VM proof whose profile reset did not remove a disk" >&2
  cat "$tmp_dir/manual-mac-reset-without-removal-validation.json" >&2
  exit 1
fi

"$node_bin" -e '
  const fs = require("node:fs");
  const validation = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  if (!validation.errors?.some((error) => error.includes("safe profile reset proof"))) {
    console.error("manual UX validator must reject Mac VM artifacts whose profile reset did not remove a disk");
    process.exit(1);
  }
' "$tmp_dir/manual-mac-reset-without-removal-validation.json"

set +e
"$node_bin" "$repo_root/scripts/browser-manual-ux-report.mjs" \
  --input "$tmp_dir/manual-mac-leaky-profile-reset.json" \
  >"$tmp_dir/manual-mac-leaky-profile-reset-validation.json" \
  2>"$tmp_dir/manual-mac-leaky-profile-reset-validation.err"
leaky_profile_reset_status=$?
set -e
if [[ "$leaky_profile_reset_status" -eq 0 ]]; then
  echo "manual UX validator accepted Mac VM proof with leaky profile reset evidence" >&2
  cat "$tmp_dir/manual-mac-leaky-profile-reset-validation.json" >&2
  exit 1
fi

"$node_bin" -e '
  const fs = require("node:fs");
  const validation = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  if (!validation.errors?.some((error) => error.includes("safe profile reset proof"))) {
    console.error("manual UX validator must reject Mac VM artifacts with leaky profile reset evidence");
    process.exit(1);
  }
' "$tmp_dir/manual-mac-leaky-profile-reset-validation.json"

set +e
"$node_bin" "$repo_root/scripts/browser-manual-ux-report.mjs" \
  --input "$tmp_dir/manual-mac-stale-restart.json" \
  >"$tmp_dir/manual-mac-stale-restart-validation.json" \
  2>"$tmp_dir/manual-mac-stale-restart-validation.err"
stale_restart_status=$?
set -e
if [[ "$stale_restart_status" -eq 0 ]]; then
  echo "manual UX validator accepted Mac VM proof with stale restart evidence" >&2
  cat "$tmp_dir/manual-mac-stale-restart-validation.json" >&2
  exit 1
fi

"$node_bin" -e '
  const fs = require("node:fs");
  const validation = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  if (!validation.errors?.some((error) => error.includes("fresh restart evidence"))) {
    console.error("manual UX validator must reject stale Mac VM control restart evidence");
    process.exit(1);
  }
' "$tmp_dir/manual-mac-stale-restart-validation.json"

"$node_bin" -e '
  const fs = require("node:fs");
  const template = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  template.ok = true;
  template.reviewed_at = "2026-06-19T00:00:00Z";
  template.reviewer = "mac-vm-manual-ux-smoke";
  for (const key of Object.keys(template.checks)) {
    template.checks[key] = true;
    template.evidence[key] = `observed ${key}`;
  }
  template.evidence.ela_city_edit_profile_modal = "Edit Profile modal opened in the Mac VM Browser";
  fs.writeFileSync(process.argv[2], `${JSON.stringify(template, null, 2)}\n`);
  template.evidence.ela_city_edit_profile_modal = "profile looked good";
  fs.writeFileSync(process.argv[4], `${JSON.stringify(template, null, 2)}\n`);
  template.evidence.ela_city_edit_profile_modal = "";
  fs.writeFileSync(process.argv[3], `${JSON.stringify(template, null, 2)}\n`);
' "$tmp_dir/manual-template-mac.json" "$tmp_dir/manual-mac-matched.json" "$tmp_dir/manual-mac-missing-edit-profile-evidence.json" "$tmp_dir/manual-mac-generic-edit-profile-evidence.json"

printf 'redacted Mac Browser VM manual screen recording fixture\n' >"$tmp_dir/mac-review-screen-recording.txt"
review_artifact_sha="$("$node_bin" -e 'const fs = require("node:fs"); const crypto = require("node:crypto"); process.stdout.write(crypto.createHash("sha256").update(fs.readFileSync(process.argv[1])).digest("hex"));' "$tmp_dir/mac-review-screen-recording.txt")"
printf 'leaky Mac Browser VM manual artifact with home_token=secret and connect_ticket ticket:secret\n' >"$tmp_dir/mac-review-leaky-screen-recording.txt"
leaky_review_artifact_sha="$("$node_bin" -e 'const fs = require("node:fs"); const crypto = require("node:crypto"); process.stdout.write(crypto.createHash("sha256").update(fs.readFileSync(process.argv[1])).digest("hex"));' "$tmp_dir/mac-review-leaky-screen-recording.txt")"

"$node_bin" -e '
  const fs = require("node:fs");
  const [
    matchedPath,
    missingArtifactPath,
    mismatchedArtifactPath,
    unredactedArtifactPath,
    leakyArtifactReportPath,
    artifactPath,
    artifactSha,
    leakyArtifactPath,
    leakyArtifactSha,
  ] = process.argv.slice(1);
  const matched = JSON.parse(fs.readFileSync(matchedPath, "utf8"));
  fs.writeFileSync(missingArtifactPath, `${JSON.stringify(matched, null, 2)}\n`);
  matched.review_artifacts = [{
    kind: "screen_recording",
    path: artifactPath,
    sha256: artifactSha,
    description: "Redacted Mac Browser VM manual review recording covering video, input, zoom, ela.city edit profile, and cleanup.",
    redacted: true,
  }];
  fs.writeFileSync(matchedPath, `${JSON.stringify(matched, null, 2)}\n`);
  const mismatched = structuredClone(matched);
  mismatched.review_artifacts[0].sha256 = "0".repeat(64);
  fs.writeFileSync(mismatchedArtifactPath, `${JSON.stringify(mismatched, null, 2)}\n`);
  const unredacted = structuredClone(matched);
  unredacted.review_artifacts[0].redacted = false;
  fs.writeFileSync(unredactedArtifactPath, `${JSON.stringify(unredacted, null, 2)}\n`);
  const leaky = structuredClone(matched);
  leaky.review_artifacts[0].path = leakyArtifactPath;
  leaky.review_artifacts[0].sha256 = leakyArtifactSha;
  fs.writeFileSync(leakyArtifactReportPath, `${JSON.stringify(leaky, null, 2)}\n`);
' "$tmp_dir/manual-mac-matched.json" \
  "$tmp_dir/manual-mac-missing-review-artifact.json" \
  "$tmp_dir/manual-mac-mismatched-review-artifact.json" \
  "$tmp_dir/manual-mac-unredacted-review-artifact.json" \
  "$tmp_dir/manual-mac-leaky-review-artifact.json" \
  "$tmp_dir/mac-review-screen-recording.txt" \
  "$review_artifact_sha" \
  "$tmp_dir/mac-review-leaky-screen-recording.txt" \
  "$leaky_review_artifact_sha"

"$node_bin" -e '
  const fs = require("node:fs");
  const [templatePath, outPath, artifactPath, artifactSha] = process.argv.slice(1);
  const report = JSON.parse(fs.readFileSync(templatePath, "utf8"));
  report.ok = true;
  report.reviewed_at = "2026-06-19T00:00:00Z";
  report.reviewer = "mac-vm-manual-ux-smoke";
  for (const key of Object.keys(report.checks)) {
    report.checks[key] = true;
    report.evidence[key] = `observed ${key}`;
  }
  report.evidence.ela_city_edit_profile_modal = "Edit Profile modal opened in the resized Mac VM Browser";
  report.review_artifacts = [{
    kind: "screen_recording",
    path: artifactPath,
    sha256: artifactSha,
    description: "Redacted resized Mac Browser VM manual review recording covering video, input, zoom, ela.city edit profile, and cleanup.",
    redacted: true,
  }];
  fs.writeFileSync(outPath, `${JSON.stringify(report, null, 2)}\n`);
' "$tmp_dir/manual-template-mac-resized.json" \
  "$tmp_dir/manual-mac-resized.json" \
  "$tmp_dir/mac-review-screen-recording.txt" \
  "$review_artifact_sha"

"$node_bin" -e '
  const fs = require("node:fs");
  const report = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  report.reviewed_at = "2026-06-18T23:59:59Z";
  fs.writeFileSync(process.argv[2], `${JSON.stringify(report, null, 2)}\n`);
' "$tmp_dir/manual-mac-matched.json" "$tmp_dir/manual-mac-stale-review.json"

set +e
"$node_bin" "$repo_root/scripts/browser-manual-ux-report.mjs" \
  --input "$tmp_dir/manual-mac-stale-review.json" \
  >"$tmp_dir/manual-mac-stale-review-validation.json" \
  2>"$tmp_dir/manual-mac-stale-review-validation.err"
stale_review_status=$?
set -e
if [[ "$stale_review_status" -eq 0 ]]; then
  echo "manual UX validator accepted a review timestamp before the Mac VM proof timestamp" >&2
  cat "$tmp_dir/manual-mac-stale-review-validation.json" >&2
  exit 1
fi

"$node_bin" -e '
  const fs = require("node:fs");
  const validation = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  if (!validation.errors?.includes("reviewed_at must be at or after machine_artifact.generated_at")) {
    console.error("manual UX validator must reject review timestamps before machine artifact generation");
    process.exit(1);
  }
' "$tmp_dir/manual-mac-stale-review-validation.json"

set +e
"$node_bin" "$repo_root/scripts/browser-manual-ux-report.mjs" \
  --input "$tmp_dir/manual-mac-missing-edit-profile-evidence.json" \
  >"$tmp_dir/manual-mac-missing-edit-profile-validation.json" \
  2>"$tmp_dir/manual-mac-missing-edit-profile-validation.err"
missing_edit_profile_status=$?
set -e
if [[ "$missing_edit_profile_status" -eq 0 ]]; then
  echo "manual UX validator accepted Mac VM evidence without edit-profile modal evidence text" >&2
  cat "$tmp_dir/manual-mac-missing-edit-profile-validation.json" >&2
  exit 1
fi

"$node_bin" -e '
  const fs = require("node:fs");
  const validation = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  if (!validation.errors?.includes("evidence.ela_city_edit_profile_modal must describe the observed Mac VM proof")) {
    console.error("manual UX validator must require edit-profile modal evidence for Mac VM proof");
    process.exit(1);
  }
' "$tmp_dir/manual-mac-missing-edit-profile-validation.json"

set +e
"$node_bin" "$repo_root/scripts/browser-manual-ux-report.mjs" \
  --input "$tmp_dir/manual-mac-generic-edit-profile-evidence.json" \
  >"$tmp_dir/manual-mac-generic-edit-profile-validation.json" \
  2>"$tmp_dir/manual-mac-generic-edit-profile-validation.err"
generic_edit_profile_status=$?
set -e
if [[ "$generic_edit_profile_status" -eq 0 ]]; then
  echo "manual UX validator accepted generic profile evidence for Mac VM edit-profile proof" >&2
  cat "$tmp_dir/manual-mac-generic-edit-profile-validation.json" >&2
  exit 1
fi

"$node_bin" -e '
  const fs = require("node:fs");
  const validation = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  if (!validation.errors?.includes("evidence.ela_city_edit_profile_modal must cite Edit Profile or Account Settings")) {
    console.error("manual UX validator must reject generic Mac VM profile evidence");
    process.exit(1);
  }
' "$tmp_dir/manual-mac-generic-edit-profile-validation.json"

set +e
"$node_bin" "$repo_root/scripts/browser-manual-ux-report.mjs" \
  --input "$tmp_dir/manual-mac-missing-review-artifact.json" \
  >"$tmp_dir/manual-mac-missing-review-artifact-validation.json" \
  2>"$tmp_dir/manual-mac-missing-review-artifact-validation.err"
missing_review_artifact_status=$?
set -e
if [[ "$missing_review_artifact_status" -eq 0 ]]; then
  echo "manual UX validator accepted Mac VM manual evidence without a hash-bound review artifact" >&2
  cat "$tmp_dir/manual-mac-missing-review-artifact-validation.json" >&2
  exit 1
fi

"$node_bin" -e '
  const fs = require("node:fs");
  const validation = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  if (!validation.errors?.includes("review_artifacts must include at least one hash-bound redacted Mac VM screen recording artifact")) {
    console.error("manual UX validator must require a hash-bound Mac VM screen recording artifact");
    process.exit(1);
  }
' "$tmp_dir/manual-mac-missing-review-artifact-validation.json"

set +e
"$node_bin" "$repo_root/scripts/browser-manual-ux-report.mjs" \
  --input "$tmp_dir/manual-mac-mismatched-review-artifact.json" \
  >"$tmp_dir/manual-mac-mismatched-review-artifact-validation.json" \
  2>"$tmp_dir/manual-mac-mismatched-review-artifact-validation.err"
mismatched_review_artifact_status=$?
set -e
if [[ "$mismatched_review_artifact_status" -eq 0 ]]; then
  echo "manual UX validator accepted a Mac VM manual review artifact with a mismatched digest" >&2
  cat "$tmp_dir/manual-mac-mismatched-review-artifact-validation.json" >&2
  exit 1
fi

"$node_bin" -e '
  const fs = require("node:fs");
  const validation = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  if (!validation.errors?.some((error) => error.includes("review_artifacts[0].sha256 must match"))) {
    console.error("manual UX validator must reject review artifact hash mismatches");
    process.exit(1);
  }
' "$tmp_dir/manual-mac-mismatched-review-artifact-validation.json"

set +e
"$node_bin" "$repo_root/scripts/browser-manual-ux-report.mjs" \
  --input "$tmp_dir/manual-mac-unredacted-review-artifact.json" \
  >"$tmp_dir/manual-mac-unredacted-review-artifact-validation.json" \
  2>"$tmp_dir/manual-mac-unredacted-review-artifact-validation.err"
unredacted_review_artifact_status=$?
set -e
if [[ "$unredacted_review_artifact_status" -eq 0 ]]; then
  echo "manual UX validator accepted a Mac VM manual review artifact without redacted=true" >&2
  cat "$tmp_dir/manual-mac-unredacted-review-artifact-validation.json" >&2
  exit 1
fi

"$node_bin" -e '
  const fs = require("node:fs");
  const validation = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  if (!validation.errors?.some((error) => error.includes("review_artifacts[0].redacted must be true"))) {
    console.error("manual UX validator must reject review artifacts without redacted=true");
    process.exit(1);
  }
' "$tmp_dir/manual-mac-unredacted-review-artifact-validation.json"

set +e
"$node_bin" "$repo_root/scripts/browser-manual-ux-report.mjs" \
  --input "$tmp_dir/manual-mac-leaky-review-artifact.json" \
  >"$tmp_dir/manual-mac-leaky-review-artifact-validation.json" \
  2>"$tmp_dir/manual-mac-leaky-review-artifact-validation.err"
leaky_review_artifact_status=$?
set -e
if [[ "$leaky_review_artifact_status" -eq 0 ]]; then
  echo "manual UX validator accepted a Mac VM manual review artifact containing raw authority text" >&2
  cat "$tmp_dir/manual-mac-leaky-review-artifact-validation.json" >&2
  exit 1
fi

"$node_bin" -e '
  const fs = require("node:fs");
  const validation = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  if (!validation.errors?.some((error) => error.includes("review_artifacts[0].path must point to a redacted artifact without raw authority text"))) {
    console.error("manual UX validator must reject review artifacts containing raw authority text");
    process.exit(1);
  }
' "$tmp_dir/manual-mac-leaky-review-artifact-validation.json"

"$node_bin" "$repo_root/scripts/browser-manual-ux-report.mjs" \
  --input "$tmp_dir/manual-mac-matched.json" \
  >"$tmp_dir/manual-mac-matched-validation.json"

"$node_bin" "$repo_root/scripts/browser-manual-ux-report.mjs" \
  --input "$tmp_dir/manual-mac-resized.json" \
  >"$tmp_dir/manual-mac-resized-validation.json"

"$node_bin" -e '
  const fs = require("node:fs");
  const validation = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  if (validation.ok !== true || validation.machine_artifact?.schema !== "elastos.browser.mac-vm-proof/v1") {
    console.error("manual UX validator must accept matched Mac VM machine + manual evidence");
    process.exit(1);
  }
' "$tmp_dir/manual-mac-matched-validation.json"

"$node_bin" -e '
  const fs = require("node:fs");
  const validation = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  const proof = JSON.parse(fs.readFileSync(process.argv[2], "utf8"));
  if (validation.ok !== true || validation.machine_artifact?.path !== process.argv[2]) {
    console.error("manual UX validator must accept matched resized Mac VM proof evidence");
    process.exit(1);
  }
  if (
    proof.quality_gates?.thresholds?.expected_viewport_width !== 1000 ||
    proof.quality_gates?.thresholds?.expected_viewport_height !== 700 ||
    proof.quality_gates?.zoom?.viewport_width !== 1000 ||
    proof.quality_gates?.zoom?.viewport_height !== 700
  ) {
    console.error("resized Mac VM manual UX smoke must exercise the requested viewport thresholds");
    process.exit(1);
  }
' "$tmp_dir/manual-mac-resized-validation.json" "$tmp_dir/mac-resized.json"

set +e
"$node_bin" "$repo_root/scripts/browser-objective-audit.mjs" \
  --manual-ux "$tmp_dir/manual-mac-matched.json" \
  >"$tmp_dir/mac-manual-product-audit.json" \
  2>"$tmp_dir/mac-manual-product-audit.err"
product_audit_status=$?
set -e
if [[ "$product_audit_status" -eq 0 ]]; then
  echo "product Browser objective audit accepted Mac VM manual evidence as hosted/native product evidence" >&2
  cat "$tmp_dir/mac-manual-product-audit.json" >&2
  exit 1
fi

printf '{"schema":"elastos.browser.mac-vm-manual-ux-smoke/v1","ok":true,"mac_template_prefilled":true,"shallow_mac_artifact_rejected":true,"aggregate_only_mac_artifact_rejected":true,"url_unchanged_rejected":true,"missing_runtime_media_relay_rejected":true,"missing_edit_profile_diagnostic_rejected":true,"missing_profile_reset_rejected":true,"reset_without_removal_rejected":true,"leaky_profile_reset_rejected":true,"stale_restart_rejected":true,"stale_review_rejected":true,"edit_profile_evidence_required":true,"generic_edit_profile_evidence_rejected":true,"review_artifact_required":true,"review_artifact_hash_mismatch_rejected":true,"review_artifact_redaction_required":true,"review_artifact_secret_leak_rejected":true,"matched_mac_manual_ux_accepted":true,"resized_mac_artifact_accepted":true,"mac_manual_does_not_satisfy_product_audio_audit":true}\n'
