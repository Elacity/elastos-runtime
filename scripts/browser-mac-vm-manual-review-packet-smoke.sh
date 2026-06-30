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

tmp_dir="$(mktemp -d /tmp/elastos-browser-mac-vm-review-packet-smoke-XXXXXX)"
cleanup() {
  rm -rf "$tmp_dir"
}
trap cleanup EXIT

proof="$tmp_dir/mac-proof.json"
summary="$tmp_dir/handoff-summary.json"
out_dir="$tmp_dir/packet"
packet_out="$tmp_dir/packet-output.json"

cat >"$proof" <<'JSON'
{
  "schema": "elastos.browser.mac-vm-proof/v1",
  "ok": true,
  "target": "mac-source-home",
  "generated_at": "2026-06-19T00:00:00.000Z",
  "home": {
    "url": "http://localhost:61180/apps/home/",
    "http_code": 200,
    "hash_parity": true,
    "installed_index_sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    "source_index_sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
  },
  "virtual_auth": {
    "persistent_profile": false,
    "cleanup_passkey": true
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
        "reset": "whole_profile"
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
    "decoded_frame_delta": 20,
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
    "title": "ela.city",
    "body_text": "Log In Home Channels",
    "visible_image_count": 3,
    "visible_broken_image_count": 0,
    "visible_pending_image_count": 0,
    "visible_text_samples": [{
      "text": "Log In",
      "visible": true
    }],
    "dialog_elements": [],
    "diagnostic_click_actions": []
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
  }
}
JSON

"$node_bin" - "$proof" "$summary" <<'NODE'
const fs = require("node:fs");
const crypto = require("node:crypto");
const [proofPath, summaryPath] = process.argv.slice(2);
const proof = JSON.parse(fs.readFileSync(proofPath, "utf8"));
const sha256 = crypto.createHash("sha256").update(fs.readFileSync(proofPath)).digest("hex");
const summary = {
  schema: "elastos.browser.mac-vm-acceptance-handoff/v1",
  ok: false,
  handoff_ready: false,
  machine_ready: true,
  acceptance_ready: false,
  generated_at: "2026-06-19T00:01:00.000Z",
  machine_proof: {
    path: proofPath,
    schema: proof.schema,
    ok: true,
    machine_ready: true,
    sha256,
    generated_at: proof.generated_at,
  },
  source_home_restart: {
    ok: true,
    schema: "elastos.mac-source-home-restart/v1",
    browser_helper_source_sha256: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
    browser_helper_installed_sha256: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
    browser_helper_initrd_sha256: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
    browser_helper_rootfs_sha256: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
  },
  machine_failing: [],
  auth_failing: [
    "ela_city_auth_profile_persistence",
    "auth_setup_receipt_chain",
    "ela_city_authenticated_surface",
    "ela_city_edit_profile_modal"
  ],
  manual_failing: [
    "manual_ux_hash_bound",
    "ela_city_edit_profile_modal"
  ],
  remaining_acceptance_gaps: [
    "manual_ux_hash_bound",
    "auth_setup_receipt_chain",
    "ela_city_authenticated_surface",
    "ela_city_edit_profile_modal"
  ],
  next_steps: [
    "For authenticated ela.city proof, first run scripts/browser-mac-vm-auth-profile-setup.sh --auth-profile /tmp/mac-browser-vm-proof-auth --receipt-out /tmp/mac-browser-vm-auth-setup.json, sign into ela.city inside that persistent Browser VM profile, then rerun this handoff with the same profile and receipt.",
    "Collect the authenticated proof with scripts/browser-mac-vm-acceptance-handoff.sh --restart-source-home --auth-profile /tmp/mac-browser-vm-proof-auth --auth-setup-receipt /tmp/mac-browser-vm-auth-setup.json --summary-out <handoff-summary.json>.",
    "Run final audit: node scripts/browser-mac-vm-acceptance-audit.mjs --machine-proof /tmp/mac-proof.json --manual-ux /tmp/manual.json --handoff-summary /tmp/handoff-summary.json",
    "Validate the manual report: node scripts/browser-manual-ux-report.mjs --input /tmp/manual.json"
  ],
};
fs.writeFileSync(summaryPath, `${JSON.stringify(summary, null, 2)}\n`);
NODE

"$node_bin" "$repo_root/scripts/browser-mac-vm-manual-review-packet.mjs" \
  --machine-proof "$proof" \
  --handoff-summary "$summary" \
  --out-dir "$out_dir" \
  >"$packet_out"

"$node_bin" - "$packet_out" "$out_dir" "$proof" "$summary" <<'NODE'
const fs = require("node:fs");
const crypto = require("node:crypto");
const [packetOut, outDir, proofPath, summaryPath] = process.argv.slice(2);
const packet = JSON.parse(fs.readFileSync(packetOut, "utf8"));
const manualPath = `${outDir}/manual-ux-draft.json`;
const checklistPath = `${outDir}/operator-checklist.md`;
const manual = JSON.parse(fs.readFileSync(manualPath, "utf8"));
const checklist = fs.readFileSync(checklistPath, "utf8");
const proofSha = crypto.createHash("sha256").update(fs.readFileSync(proofPath)).digest("hex");
const summarySha = crypto.createHash("sha256").update(fs.readFileSync(summaryPath)).digest("hex");
if (packet.schema !== "elastos.browser.mac-vm-manual-review-packet/v1" || packet.ok !== true) {
  throw new Error("review packet schema mismatch");
}
if (packet.machine_artifact?.sha256 !== proofSha || manual.machine_artifact?.sha256 !== proofSha) {
  throw new Error("review packet/manual draft is not bound to the machine proof");
}
if (packet.handoff_summary?.sha256 !== summarySha || packet.handoff_summary?.source_home_restart_ok !== true) {
  throw new Error("review packet is not bound to the handoff/source-home restart summary");
}
if (packet.handoff_summary?.machine_failing?.length !== 0 ||
    !packet.handoff_summary?.auth_failing?.includes("auth_setup_receipt_chain") ||
    !packet.handoff_summary?.manual_failing?.includes("manual_ux_hash_bound") ||
    !packet.handoff_summary?.next_steps?.some((step) => step.includes("browser-mac-vm-auth-profile-setup.sh")) ||
    !packet.handoff_summary?.next_steps?.some((step) => step.includes("browser-mac-vm-acceptance-audit.mjs"))) {
  throw new Error("review packet must preserve grouped remaining gaps and auth/final audit guidance");
}
if (manual.ok !== false || manual.reviewed_at !== "" || manual.reviewer !== "") {
  throw new Error("manual draft must remain unaccepted until real review");
}
for (const check of ["remote_video_visible", "typing_latency", "ela_city_edit_profile_modal", "session_cleanup"]) {
  if (manual.checks?.[check] !== false || manual.evidence?.[check] !== "") {
    throw new Error(`manual draft must leave ${check} for the reviewer`);
  }
}
const artifact = manual.review_artifacts?.[0];
if (artifact?.kind !== "checklist" || artifact?.redacted !== true || artifact?.path !== checklistPath) {
  throw new Error("manual draft must include the generated checklist as a redacted non-visual artifact");
}
if (!checklist.includes("Add at least one separate redacted screen recording") ||
    !checklist.includes("ela_city_edit_profile_modal") ||
    !checklist.includes("source-home restart freshness") ||
    !checklist.includes("Authenticated Setup And Final Audit") ||
    !checklist.includes("browser-mac-vm-auth-profile-setup.sh") ||
    !checklist.includes("browser-mac-vm-acceptance-audit.mjs") ||
    !checklist.includes("authenticated ela.city failing") ||
    !checklist.includes("machine failing | none")) {
  throw new Error("operator checklist is missing Mac VM review guidance");
}
if (/connect_ticket|relay_ipc|adapter_ipc|runtime_stream_path|home_token|authorization|cookie|set-cookie|did:key|person:local/i.test(checklist)) {
  throw new Error("operator checklist leaked raw authority text");
}
NODE

set +e
"$node_bin" "$repo_root/scripts/browser-manual-ux-report.mjs" \
  --input "$out_dir/manual-ux-draft.json" \
  >"$tmp_dir/manual-validation.json" \
  2>"$tmp_dir/manual-validation.err"
manual_status=$?
set -e
if [[ "$manual_status" -eq 0 ]]; then
  echo "generated manual review packet unexpectedly satisfied manual UX acceptance" >&2
  cat "$tmp_dir/manual-validation.json" >&2
  exit 1
fi

"$node_bin" - "$tmp_dir/manual-validation.json" <<'NODE'
const fs = require("node:fs");
const validation = JSON.parse(fs.readFileSync(process.argv[2], "utf8"));
if (!validation.errors?.some((error) => error.includes("ok must be true")) ||
    !validation.errors?.some((error) => error.includes("review_artifacts must include at least one hash-bound redacted Mac VM screen recording artifact"))) {
  throw new Error("manual packet draft must fail closed until human review and visual evidence are added");
}
NODE

bad_summary="$tmp_dir/bad-summary.json"
"$node_bin" - "$summary" "$bad_summary" <<'NODE'
const fs = require("node:fs");
const [summaryPath, badSummaryPath] = process.argv.slice(2);
const summary = JSON.parse(fs.readFileSync(summaryPath, "utf8"));
summary.machine_proof.sha256 = "0".repeat(64);
fs.writeFileSync(badSummaryPath, `${JSON.stringify(summary, null, 2)}\n`);
NODE

set +e
"$node_bin" "$repo_root/scripts/browser-mac-vm-manual-review-packet.mjs" \
  --machine-proof "$proof" \
  --handoff-summary "$bad_summary" \
  --out-dir "$tmp_dir/bad-packet" \
  >"$tmp_dir/bad-packet.out" \
  2>"$tmp_dir/bad-packet.err"
bad_status=$?
set -e
if [[ "$bad_status" -eq 0 ]]; then
  echo "manual review packet accepted a handoff summary for a different machine proof" >&2
  exit 1
fi
if ! grep -q "machine_proof.sha256 must match" "$tmp_dir/bad-packet.err"; then
  echo "manual review packet did not explain handoff hash mismatch" >&2
  cat "$tmp_dir/bad-packet.err" >&2
  exit 1
fi

printf '{"schema":"elastos.browser.mac-vm-manual-review-packet-smoke/v1","ok":true,"packet_bound":true,"draft_fails_closed":true,"mismatched_handoff_rejected":true}\n'
