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

tmp_dir="$(mktemp -d /tmp/elastos-browser-mac-vm-acceptance-handoff-smoke-XXXXXX)"
cleanup() {
  rm -rf "$tmp_dir"
}
trap cleanup EXIT

proof="$tmp_dir/mac-proof.json"
manual="$tmp_dir/manual-ux.json"
audit="$tmp_dir/audit.json"
summary="$tmp_dir/summary.json"

cat >"$proof" <<'JSON'
{
  "schema": "elastos.browser.mac-vm-proof/v1",
  "ok": true,
  "target": "mac-source-home",
  "generated_at": "2026-06-19T00:00:00.000Z",
  "home": {
    "http_code": 200,
    "hash_parity": true,
    "installed_index_sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    "source_index_sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
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
  },
  "manual_acceptance": {
    "status": "not_recorded"
  }
}
JSON

restart_receipt="$tmp_dir/source-home-restart.json"
"$node_bin" - "$proof" "$restart_receipt" <<'NODE'
const fs = require("node:fs");
const crypto = require("node:crypto");
const [proofPath, receiptPath] = process.argv.slice(2);
const proof = JSON.parse(fs.readFileSync(proofPath, "utf8"));
const receipt = {
  schema: "elastos.mac-source-home-restart/v1",
  ok: true,
  dry_run: false,
  generated_at: "2026-06-19T00:00:00.000Z",
  repo: "/Users/anders/Code/elastos-runtime",
  test_home: "/Users/anders/elastos-mac-test-home",
  data_dir: "/Users/anders/elastos-mac-test-home/Library/Application Support/elastos",
  addr: "localhost:61180",
  home_url: "http://localhost:61180/apps/home/",
  gateway_bin: "/Users/anders/Code/elastos-runtime/elastos/target/release/elastos",
  gateway_log: "/Users/anders/elastos-mac-test-home/logs/gateway-smoke.log",
  http_code: 200,
  served_index_sha256: proof.home.installed_index_sha256,
  installed_index_sha256: proof.home.installed_index_sha256,
  source_index_sha256: proof.home.source_index_sha256,
  browser_helper_source_sha256: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
  browser_helper_installed_sha256: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
  browser_helper_initrd_sha256: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
  browser_helper_rootfs_sha256: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
};
fs.writeFileSync(receiptPath, `${JSON.stringify(receipt, null, 2)}\n`);
crypto.createHash("sha256").update(fs.readFileSync(receiptPath)).digest("hex");
NODE

set +e
"$repo_root/scripts/browser-mac-vm-acceptance-handoff.sh" \
  --machine-proof "$proof" \
  --source-home-restart-receipt "$restart_receipt" \
  --manual-out "$manual" \
  --audit-out "$audit" \
  --summary-out "$summary" \
  >/dev/null
unauth_handoff_status=$?
set -e

if [[ "$unauth_handoff_status" -eq 0 ]]; then
  echo "handoff accepted an unauthenticated disposable proof without auth setup receipt" >&2
  cat "$summary" >&2
  exit 1
fi

"$node_bin" - "$proof" "$manual" "$audit" "$summary" <<'NODE'
const fs = require("node:fs");
const [proofPath, manualPath, auditPath, summaryPath] = process.argv.slice(2);
const manual = JSON.parse(fs.readFileSync(manualPath, "utf8"));
const audit = JSON.parse(fs.readFileSync(auditPath, "utf8"));
const summary = JSON.parse(fs.readFileSync(summaryPath, "utf8"));
if (manual.schema !== "elastos.browser.manual-ux/v1" || manual.provider !== "mac-vm") {
  throw new Error("handoff did not generate a Mac VM manual UX template");
}
if (manual.machine_artifact?.path !== proofPath || !manual.machine_artifact?.sha256) {
  throw new Error("manual template is not bound to the machine proof artifact");
}
if (audit.ok !== false || !audit.criteria?.some((item) => item.id === "manual_ux_hash_bound" && item.ok === false)) {
  throw new Error("handoff audit must fail closed while manual evidence is missing");
}
if (summary.schema !== "elastos.browser.mac-vm-acceptance-handoff/v1" || summary.machine_proof?.path !== proofPath) {
  throw new Error("handoff summary has the wrong schema or proof path");
}
if (summary.ok !== false || summary.handoff_ready !== false || summary.machine_ready !== true ||
    summary.machine_proof?.ok !== true || summary.machine_proof?.machine_ready !== true ||
    summary.acceptance_audit?.machine_ready !== true) {
  throw new Error("handoff summary must keep a machine-ready unauthenticated proof handoff-failed until auth setup is bound");
}
if (!/^[a-f0-9]{64}$/i.test(String(summary.machine_proof?.sha256 || "")) ||
    !/^[a-f0-9]{64}$/i.test(String(summary.manual_template?.sha256 || "")) ||
    !/^[a-f0-9]{64}$/i.test(String(summary.acceptance_audit?.sha256 || ""))) {
  throw new Error("handoff summary must hash-bind the machine proof, manual template, and acceptance audit");
}
if (summary.acceptance_ready !== false ||
    !summary.remaining_acceptance_gaps?.includes("manual_ux_hash_bound") ||
    !summary.remaining_acceptance_gaps?.includes("auth_setup_receipt_chain") ||
    !summary.remaining_acceptance_gaps?.includes("ela_city_authenticated_surface")) {
  throw new Error("handoff summary must expose that product acceptance is still blocked by manual/auth gaps");
}
if (!summary.failing_criteria?.includes("manual_ux_hash_bound") ||
    !summary.failing_criteria?.includes("auth_setup_receipt_chain") ||
    !summary.auth_failing?.includes("auth_setup_receipt_chain") ||
    !summary.manual_failing?.includes("manual_ux_hash_bound") ||
    !summary.acceptance_audit?.failing?.includes("manual_ux_hash_bound") ||
    !summary.acceptance_audit?.failing?.includes("auth_setup_receipt_chain") ||
    !summary.acceptance_audit?.failing?.includes("ela_city_authenticated_surface")) {
  throw new Error("handoff summary must expose the remaining manual/authenticated gaps");
}
if (summary.authenticated_profile?.persistent_virtual_auth_profile !== false ||
    summary.authenticated_profile?.auth_profile_requested !== false) {
  throw new Error("handoff summary must expose that this fixture was not collected with a persistent auth profile");
}
if (summary.vm_control_restart?.fresh_after_restart !== true ||
    summary.vm_control_restart?.schema !== "elastos.browser.mac-vm-control-restart/v1" ||
    summary.vm_control_restart?.actual_uptime_ms !== 61000) {
  throw new Error("handoff summary must expose the fresh VM control-service restart receipt");
}
if (summary.source_home_restart?.ok !== true ||
    summary.source_home_restart?.schema !== "elastos.mac-source-home-restart/v1" ||
    summary.source_home_restart?.path == null ||
    !summary.source_home_restart?.sha256 ||
    summary.source_home_restart?.browser_helper_source_sha256 !== summary.source_home_restart?.browser_helper_rootfs_sha256) {
  throw new Error("handoff summary must hash-bind the source-home restart and Browser helper freshness receipt");
}
if (summary.profile_reset?.requested !== true ||
    summary.profile_reset?.ok !== true ||
    summary.profile_reset?.receipt_schema !== "elastos.browser.profile-reset/v1") {
  throw new Error("handoff summary must expose the Browser profile reset receipt");
}
if (!summary.next_steps?.some((step) =>
    step.includes("browser-mac-vm-auth-profile-setup.sh") &&
    step.includes("--auth-profile") &&
    step.includes("--receipt-out"))) {
  throw new Error("handoff summary must include the persistent auth-profile setup receipt step");
}
if (!summary.next_steps?.some((step) =>
    step.includes("browser-mac-vm-acceptance-handoff.sh") &&
    step.includes("--auth-profile") &&
    step.includes("--auth-setup-receipt"))) {
  throw new Error("handoff summary must include the authenticated handoff receipt step");
}
if (!summary.next_steps?.some((step) => step.includes("Fresh VM control-service restart proof"))) {
  throw new Error("handoff summary must include the fresh VM control-service restart proof status");
}
if (!summary.next_steps?.some((step) => step.includes("Edit Profile diagnostic-click"))) {
  throw new Error("handoff summary must expose the edit-profile diagnostic-click proof step");
}
if (!summary.next_steps?.some((step) => step.includes("browser-mac-vm-acceptance-audit.mjs"))) {
  throw new Error("handoff summary must include the final audit command");
}
if (!summary.next_steps?.some((step) => step.includes("browser-mac-vm-manual-review-packet.mjs") && step.includes("--handoff-summary"))) {
  throw new Error("handoff summary must include the manual review packet command");
}
if (!summary.next_steps?.some((step) => step.includes("--handoff-summary") && step.includes(summaryPath))) {
  throw new Error("handoff summary must pass itself to the final audit command");
}
NODE

auth_profile="$tmp_dir/auth-profile"
mkdir -p "$auth_profile"
auth_profile_real="$(cd "$auth_profile" && pwd -P)"
auth_receipt="$tmp_dir/auth-setup-receipt.json"
auth_proof="$tmp_dir/mac-proof-auth.json"
auth_manual="$tmp_dir/manual-ux-auth.json"
auth_audit="$tmp_dir/audit-auth.json"
auth_summary="$tmp_dir/summary-auth.json"

"$node_bin" - "$proof" "$auth_proof" <<'NODE'
const fs = require("node:fs");
const [proofPath, authProofPath] = process.argv.slice(2);
const proof = JSON.parse(fs.readFileSync(proofPath, "utf8"));
proof.virtual_auth = {
  persistent_profile: true,
  cleanup_passkey: false,
};
fs.writeFileSync(authProofPath, `${JSON.stringify(proof, null, 2)}\n`);
NODE

"$node_bin" - "$auth_receipt" "$auth_profile_real" <<'NODE'
const fs = require("node:fs");
const [receiptPath, profilePath] = process.argv.slice(2);
const receipt = {
  schema: "elastos.browser.mac-vm-auth-profile-setup/v1",
  ok: true,
  generated_at: "2026-06-19T00:00:00.000Z",
  auth_profile: {
    path: profilePath,
    persistent_virtual_auth_profile: true,
  },
  setup: {
    base_url: "http://localhost:61180",
    open_url: "https://ela.city/channels",
    hold_ms: 12345,
    headed: true,
    preserve_profile: true,
    cleanup_passkey: false,
    authentication_claim: "setup_only_not_authentication_proof",
    authentication_proof: "deferred_to_machine_diagnostics_and_manual_ux",
  },
  follow_up: {
    acceptance_handoff: [
      "scripts/browser-mac-vm-acceptance-handoff.sh",
      "--restart-source-home",
      "--auth-profile",
      profilePath,
      "--auth-setup-receipt",
      receiptPath,
    ],
  },
};
fs.writeFileSync(receiptPath, `${JSON.stringify(receipt, null, 2)}\n`);
NODE

"$repo_root/scripts/browser-mac-vm-acceptance-handoff.sh" \
  --machine-proof "$auth_proof" \
  --auth-profile "$auth_profile" \
  --auth-setup-receipt "$auth_receipt" \
  --source-home-restart-receipt "$restart_receipt" \
  --manual-out "$auth_manual" \
  --audit-out "$auth_audit" \
  --summary-out "$auth_summary" \
  >/dev/null

"$node_bin" - "$auth_summary" "$auth_receipt" <<'NODE'
const fs = require("node:fs");
const [summaryPath, receiptPath] = process.argv.slice(2);
const summary = JSON.parse(fs.readFileSync(summaryPath, "utf8"));
if (summary.ok !== true || summary.handoff_ready !== true || summary.machine_ready !== true ||
    summary.machine_proof?.machine_ready !== true ||
    summary.acceptance_audit?.machine_ready !== true) {
  throw new Error("matched auth setup receipt should keep a machine-ready handoff green");
}
if (summary.acceptance_ready !== false ||
    !summary.remaining_acceptance_gaps?.includes("manual_ux_hash_bound")) {
  throw new Error("matched auth setup without manual UX must not look accepted");
}
if (summary.remaining_acceptance_gaps?.includes("auth_setup_receipt_chain") ||
    summary.failing_criteria?.includes("auth_setup_receipt_chain") ||
    summary.auth_failing?.includes("auth_setup_receipt_chain") ||
    !summary.passing_criteria?.includes("auth_setup_receipt_chain") ||
    !summary.acceptance_audit?.passing?.includes("auth_setup_receipt_chain")) {
  throw new Error("matched auth setup receipt must satisfy the final audit receipt chain");
}
if (!summary.acceptance_audit?.ela_city?.auth_setup_receipt?.ok) {
  throw new Error("handoff final audit must expose the verified auth setup receipt");
}
const receipt = summary.authenticated_profile?.auth_setup_receipt;
if (!receipt?.ok || receipt.path !== receiptPath || !receipt.sha256) {
  throw new Error("handoff summary must hash-bind the auth setup receipt");
}
if (receipt.schema !== "elastos.browser.mac-vm-auth-profile-setup/v1") {
  throw new Error("handoff summary must expose the auth setup receipt schema");
}
if (receipt.profile_matches_auth_profile !== true || receipt.proof_used_persistent_profile !== true) {
  throw new Error("handoff summary must prove the receipt profile and machine proof persistent profile match");
}
NODE

bad_receipt="$tmp_dir/auth-setup-receipt-bad-profile.json"
bad_summary="$tmp_dir/summary-auth-bad.json"
"$node_bin" - "$auth_receipt" "$bad_receipt" "$tmp_dir/other-profile" <<'NODE'
const fs = require("node:fs");
const [receiptPath, badReceiptPath, otherProfile] = process.argv.slice(2);
const receipt = JSON.parse(fs.readFileSync(receiptPath, "utf8"));
receipt.auth_profile.path = otherProfile;
fs.writeFileSync(badReceiptPath, `${JSON.stringify(receipt, null, 2)}\n`);
NODE

set +e
"$repo_root/scripts/browser-mac-vm-acceptance-handoff.sh" \
  --machine-proof "$auth_proof" \
  --auth-profile "$auth_profile" \
  --auth-setup-receipt "$bad_receipt" \
  --source-home-restart-receipt "$restart_receipt" \
  --manual-out "$tmp_dir/manual-ux-auth-bad.json" \
  --audit-out "$tmp_dir/audit-auth-bad.json" \
  --summary-out "$bad_summary" \
  >"$tmp_dir/handoff-bad.out" 2>"$tmp_dir/handoff-bad.err"
bad_receipt_status=$?
set -e

if [[ "$bad_receipt_status" -eq 0 ]]; then
  echo "handoff accepted an auth setup receipt for a different profile" >&2
  cat "$tmp_dir/handoff-bad.out" >&2
  exit 1
fi

"$node_bin" - "$bad_summary" <<'NODE'
const fs = require("node:fs");
const summary = JSON.parse(fs.readFileSync(process.argv[2], "utf8"));
const receipt = summary.authenticated_profile?.auth_setup_receipt;
if (summary.ok !== false || receipt?.ok !== false || receipt?.profile_matches_auth_profile !== false) {
  throw new Error("handoff summary must fail closed on mismatched auth setup receipt");
}
NODE

shallow_proof="$tmp_dir/mac-proof-shallow.json"
shallow_manual="$tmp_dir/manual-ux-shallow.json"
shallow_audit="$tmp_dir/audit-shallow.json"
shallow_summary="$tmp_dir/summary-shallow.json"
"$node_bin" - "$proof" "$shallow_proof" <<'NODE'
const fs = require("node:fs");
const [proofPath, shallowPath] = process.argv.slice(2);
const proof = JSON.parse(fs.readFileSync(proofPath, "utf8"));
delete proof.quality_gates.performance.checks;
delete proof.quality_gates.zoom.checks;
fs.writeFileSync(shallowPath, `${JSON.stringify(proof, null, 2)}\n`);
NODE

set +e
"$repo_root/scripts/browser-mac-vm-acceptance-handoff.sh" \
  --machine-proof "$shallow_proof" \
  --source-home-restart-receipt "$restart_receipt" \
  --manual-out "$shallow_manual" \
  --audit-out "$shallow_audit" \
  --summary-out "$shallow_summary" \
  >/dev/null
shallow_handoff_status=$?
set -e

if [[ "$shallow_handoff_status" -eq 0 ]]; then
  echo "handoff accepted a shallow unauthenticated machine proof" >&2
  cat "$shallow_summary" >&2
  exit 1
fi

"$node_bin" - "$shallow_summary" <<'NODE'
const fs = require("node:fs");
const summary = JSON.parse(fs.readFileSync(process.argv[2], "utf8"));
if (summary.ok !== false || summary.handoff_ready !== false || summary.machine_ready !== false ||
    summary.machine_proof?.machine_ready !== false ||
    summary.acceptance_audit?.machine_ready !== false) {
  throw new Error("handoff summary must not mark shallow machine proof as ready");
}
if (summary.acceptance_ready !== false ||
    !summary.remaining_acceptance_gaps?.includes("performance_zoom")) {
  throw new Error("shallow machine proof must keep acceptance_ready false and expose performance_zoom");
}
if (!summary.acceptance_audit?.machine_failing?.includes("performance_zoom")) {
  throw new Error("handoff summary must expose the failed machine-only criteria");
}
NODE

stale_restart_proof="$tmp_dir/mac-proof-stale-restart.json"
stale_restart_manual="$tmp_dir/manual-ux-stale-restart.json"
stale_restart_audit="$tmp_dir/audit-stale-restart.json"
stale_restart_summary="$tmp_dir/summary-stale-restart.json"
"$node_bin" - "$proof" "$stale_restart_proof" <<'NODE'
const fs = require("node:fs");
const [proofPath, stalePath] = process.argv.slice(2);
const proof = JSON.parse(fs.readFileSync(proofPath, "utf8"));
proof.vm_control.after.uptime_ms = 900000;
proof.vm_control.restart.fresh_after_restart = false;
proof.vm_control.restart.actual_uptime_ms = 900000;
fs.writeFileSync(stalePath, `${JSON.stringify(proof, null, 2)}\n`);
NODE

set +e
"$repo_root/scripts/browser-mac-vm-acceptance-handoff.sh" \
  --machine-proof "$stale_restart_proof" \
  --source-home-restart-receipt "$restart_receipt" \
  --manual-out "$stale_restart_manual" \
  --audit-out "$stale_restart_audit" \
  --summary-out "$stale_restart_summary" \
  >/dev/null
stale_restart_handoff_status=$?
set -e

if [[ "$stale_restart_handoff_status" -eq 0 ]]; then
  echo "handoff accepted a stale unauthenticated machine proof" >&2
  cat "$stale_restart_summary" >&2
  exit 1
fi

"$node_bin" - "$stale_restart_summary" <<'NODE'
const fs = require("node:fs");
const summary = JSON.parse(fs.readFileSync(process.argv[2], "utf8"));
if (summary.ok !== false || summary.handoff_ready !== false || summary.machine_ready !== false ||
    summary.machine_proof?.machine_ready !== false ||
    summary.acceptance_audit?.machine_ready !== false) {
  throw new Error("handoff summary must not mark stale-restart machine proof as ready");
}
if (summary.acceptance_ready !== false ||
    !summary.remaining_acceptance_gaps?.includes("vm_control_restart_proof")) {
  throw new Error("stale restart proof must keep acceptance_ready false and expose vm_control_restart_proof");
}
if (!summary.acceptance_audit?.machine_failing?.includes("vm_control_restart_proof")) {
  throw new Error("handoff summary must expose stale restart as a machine-only criterion failure");
}
if (!summary.next_steps?.some((step) => step.includes("--restart-source-home") && step.includes("mac-source-home-restart.sh"))) {
  throw new Error("handoff summary must point stale restart failures at the restart helper");
}
NODE

printf '{"schema":"elastos.browser.mac-vm-acceptance-handoff-smoke/v1","ok":true,"manual_template_bound":true,"missing_manual_visible":true,"acceptance_ready_false_visible":true,"auth_gap_visible":true,"auth_setup_receipt_checked":true,"source_home_restart_receipt_checked":true,"auth_setup_receipt_mismatch_rejected":true,"edit_profile_probe_visible":true,"shallow_machine_not_ready":true,"stale_restart_machine_not_ready":true}\n'
