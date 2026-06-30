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

tmp_dir="$(mktemp -d /tmp/elastos-browser-mac-vm-acceptance-audit-smoke-XXXXXX)"
cleanup() {
  rm -rf "$tmp_dir"
}
trap cleanup EXIT

write_machine_proof() {
  local out="$1"
  local body_text="$2"
  local visible_text="$3"
  local dialog_text="$4"
  cat >"$out" <<JSON
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
    "persistent_profile": true,
    "cleanup_passkey": false
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
    "decoded_frame_delta": 42,
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
    "url": "https://ela.city/profile",
    "title": "ela.city - Profile",
    "body_text": "$body_text",
    "visible_text_sample_count": 1,
    "visible_text_samples": [{
      "tag": "button",
      "role": "button",
      "text": "$visible_text",
      "aria_label": "",
      "title": "",
      "test_id": "",
      "visible": true,
      "rect": { "x": 20, "y": 20, "width": 120, "height": 32 }
    }],
    "clickable_count": 1,
    "clickable_elements": [{
      "tag": "button",
      "role": "button",
      "text": "$visible_text",
      "aria_label": "",
      "title": "",
      "test_id": "",
      "visible": true,
      "rect": { "x": 20, "y": 20, "width": 120, "height": 32 }
    }],
    "dialog_count": 0,
    "dialog_elements": [],
    "diagnostic_click_actions": [{
      "ok": true,
      "expected_text_re": "Edit Profile",
      "click": { "x": 80, "y": 36 },
      "target": {
        "tag": "button",
        "role": "button",
        "text": "$visible_text",
        "aria_label": "",
        "title": "",
        "test_id": "",
        "visible": true,
        "rect": { "x": 20, "y": 20, "width": 120, "height": 32 }
      },
      "input": {
        "accepted": true,
        "actual_url": "https://ela.city/profile",
        "title": "ela.city - Profile"
      },
      "diagnostics": {
        "body_text": "$body_text $dialog_text",
        "visible_text_samples": [{
          "tag": "button",
          "role": "button",
          "text": "$visible_text",
          "aria_label": "",
          "title": "",
          "test_id": "",
          "visible": true,
          "rect": { "x": 20, "y": 20, "width": 120, "height": 32 }
        }],
        "dialog_count": 1,
        "dialog_elements": [{
          "tag": "div",
          "role": "dialog",
          "text": "$dialog_text",
          "aria_label": "",
          "title": "",
          "test_id": "",
          "visible": true,
          "rect": { "x": 100, "y": 100, "width": 400, "height": 300 }
        }]
      }
    }],
    "visible_image_count": 12,
    "visible_broken_image_count": 0,
    "visible_pending_image_count": 0
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
}

write_machine_proof "$tmp_dir/mac-authenticated.json" "Home Settings" "Edit Profile" "Edit Profile modal"
write_machine_proof "$tmp_dir/mac-unauthenticated.json" "Log In Home Marketplace Channels" "Log In" ""
write_machine_proof "$tmp_dir/mac-generic-profile.json" "Log In Profile directory" "Profile" "Profile"

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
' "$tmp_dir/mac-authenticated.json" "$tmp_dir/mac-resized-authenticated.json"

"$node_bin" -e '
  const fs = require("node:fs");
  const proof = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  proof.embedded_video_input.click_navigation.expected_url_re = "https://ela[.]city/channels";
  proof.embedded_video_input.click_navigation.address_value = "https://ela.city/channels";
  proof.embedded_video_input.click_navigation.status.actual_url = "https://ela.city/channels";
  fs.writeFileSync(process.argv[2], `${JSON.stringify(proof, null, 2)}\n`);
' "$tmp_dir/mac-authenticated.json" "$tmp_dir/mac-url-unchanged.json"

"$node_bin" -e '
  const fs = require("node:fs");
  const proof = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  proof.page_diagnostics.body_html = "<main>raw profile DOM</main>";
  proof.page_diagnostics.diagnostic_click_actions[0].diagnostics.root_html = "<div>raw modal DOM</div>";
  proof.page_diagnostics.diagnostic_click_actions[0].diagnostics.cdp_event_samples = [
    { method: "Runtime.consoleAPICalled" },
  ];
  fs.writeFileSync(process.argv[2], `${JSON.stringify(proof, null, 2)}\n`);
' "$tmp_dir/mac-authenticated.json" "$tmp_dir/mac-raw-diagnostics.json"

"$node_bin" -e '
  const fs = require("node:fs");
  const proof = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  proof.virtual_auth = {
    persistent_profile: false,
    cleanup_passkey: true,
  };
  fs.writeFileSync(process.argv[2], `${JSON.stringify(proof, null, 2)}\n`);
' "$tmp_dir/mac-authenticated.json" "$tmp_dir/mac-disposable-authenticated.json"

"$node_bin" -e '
  const fs = require("node:fs");
  const proof = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  proof.page_diagnostics.diagnostic_click_actions = [];
  fs.writeFileSync(process.argv[2], `${JSON.stringify(proof, null, 2)}\n`);
' "$tmp_dir/mac-authenticated.json" "$tmp_dir/mac-manual-only-edit-profile.json"

"$node_bin" -e '
  const fs = require("node:fs");
  const proof = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  proof.embedded_video_input.display_session.media_transport = "direct";
  proof.embedded_video_input.display_session.credentialed_turn_ice_server_count = 0;
  fs.writeFileSync(process.argv[2], `${JSON.stringify(proof, null, 2)}\n`);
' "$tmp_dir/mac-authenticated.json" "$tmp_dir/mac-missing-runtime-media-relay.json"

"$node_bin" -e '
  const fs = require("node:fs");
  const proof = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  delete proof.embedded_video_input.vm_isolation;
  fs.writeFileSync(process.argv[2], `${JSON.stringify(proof, null, 2)}\n`);
' "$tmp_dir/mac-authenticated.json" "$tmp_dir/mac-missing-vm-isolation.json"

"$node_bin" -e '
  const fs = require("node:fs");
  const proof = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  delete proof.vm_control.after.started_at;
  proof.vm_control.after.uptime_ms = null;
  proof.vm_control.restart.fresh_after_restart = false;
  proof.vm_control.restart.actual_uptime_ms = null;
  fs.writeFileSync(process.argv[2], `${JSON.stringify(proof, null, 2)}\n`);
' "$tmp_dir/mac-authenticated.json" "$tmp_dir/mac-missing-restart-proof.json"

"$node_bin" -e '
  const fs = require("node:fs");
  const proof = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  proof.vm_control.after.uptime_ms = 900000;
  proof.vm_control.restart.fresh_after_restart = false;
  proof.vm_control.restart.actual_uptime_ms = 900000;
  fs.writeFileSync(process.argv[2], `${JSON.stringify(proof, null, 2)}\n`);
' "$tmp_dir/mac-authenticated.json" "$tmp_dir/mac-stale-restart-proof.json"

"$node_bin" -e '
  const fs = require("node:fs");
  const proof = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  delete proof.quality_gates.performance.checks;
  delete proof.quality_gates.zoom.checks;
  fs.writeFileSync(process.argv[2], `${JSON.stringify(proof, null, 2)}\n`);
' "$tmp_dir/mac-authenticated.json" "$tmp_dir/mac-aggregate-only-zoom.json"

"$node_bin" -e '
  const fs = require("node:fs");
  const proof = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  proof.profile_reset = {
    requested: false,
    ok: false,
    receipt: null,
  };
  fs.writeFileSync(process.argv[2], `${JSON.stringify(proof, null, 2)}\n`);
' "$tmp_dir/mac-authenticated.json" "$tmp_dir/mac-missing-profile-reset.json"

"$node_bin" -e '
  const fs = require("node:fs");
  const proof = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  proof.profile_reset.receipt.removed_profile_disk = false;
  fs.writeFileSync(process.argv[2], `${JSON.stringify(proof, null, 2)}\n`);
' "$tmp_dir/mac-authenticated.json" "$tmp_dir/mac-reset-without-removal.json"

"$node_bin" -e '
  const fs = require("node:fs");
  const proof = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  proof.profile_reset.receipt.profile.profile_key = "profile-secret";
  proof.profile_reset.receipt.profile.principal_id = "person:local:secret";
  proof.profile_reset.receipt.profile.disk_path = "/Users/operator/elastos/Users/0123456789ab/BrowserProfiles/default/profile.ext4";
  fs.writeFileSync(process.argv[2], `${JSON.stringify(proof, null, 2)}\n`);
' "$tmp_dir/mac-authenticated.json" "$tmp_dir/mac-leaky-profile-reset.json"

write_handoff_summary() {
  local proof_path="$1"
  local summary_path="$2"
  "$node_bin" - "$proof_path" "$summary_path" <<'NODE'
const fs = require("node:fs");
const crypto = require("node:crypto");
const [proofPath, summaryPath] = process.argv.slice(2);
const proof = JSON.parse(fs.readFileSync(proofPath, "utf8"));
const sha256 = crypto.createHash("sha256").update(fs.readFileSync(proofPath)).digest("hex");
const receiptPath = `${proofPath}.auth-setup-receipt.json`;
const receipt = {
  schema: "elastos.browser.mac-vm-auth-profile-setup/v1",
  ok: true,
  generated_at: "2026-06-19T00:00:00.000Z",
  auth_profile: {
    path: `${proofPath}.auth-profile`,
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
      "--auth-setup-receipt",
      receiptPath,
    ],
  },
};
fs.writeFileSync(receiptPath, `${JSON.stringify(receipt, null, 2)}\n`);
const receiptSha256 = crypto.createHash("sha256").update(fs.readFileSync(receiptPath)).digest("hex");
const sourceHomeRestartPath = `${proofPath}.source-home-restart.json`;
const sourceHomeRestart = {
  schema: "elastos.mac-source-home-restart/v1",
  ok: true,
  dry_run: false,
  generated_at: "2026-06-19T00:00:00.000Z",
  repo: "/Users/operator/Code/elastos-runtime",
  test_home: "/Users/operator/elastos-mac-test-home",
  data_dir: "/Users/operator/elastos-mac-test-home/Library/Application Support/elastos",
  addr: "localhost:61180",
  home_url: "http://localhost:61180/apps/home/",
  gateway_bin: "/Users/operator/Code/elastos-runtime/elastos/target/release/elastos",
  gateway_log: "/Users/operator/elastos-mac-test-home/logs/gateway-smoke.log",
  http_code: 200,
  served_index_sha256: proof.home.installed_index_sha256,
  installed_index_sha256: proof.home.installed_index_sha256,
  source_index_sha256: proof.home.source_index_sha256,
  browser_helper_source_sha256: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
  browser_helper_installed_sha256: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
  browser_helper_initrd_sha256: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
  browser_helper_rootfs_sha256: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
};
fs.writeFileSync(sourceHomeRestartPath, `${JSON.stringify(sourceHomeRestart, null, 2)}\n`);
const sourceHomeRestartSha256 = crypto.createHash("sha256").update(fs.readFileSync(sourceHomeRestartPath)).digest("hex");
const summary = {
  schema: "elastos.browser.mac-vm-acceptance-handoff/v1",
  ok: true,
  generated_at: "2026-06-19T00:00:00.000Z",
  machine_proof: {
    path: proofPath,
    schema: proof.schema,
    sha256,
    generated_at: proof.generated_at || null,
  },
  manual_template: {
    path: `${proofPath}.manual-template.json`,
    schema: "elastos.browser.manual-ux/v1",
    provider: "mac-vm",
    target: proof.target || "mac-source-home",
    ok: false,
    check_count: 12,
  },
  acceptance_audit: {
    path: `${proofPath}.acceptance-audit.json`,
    exit_status: 1,
    ok: false,
    machine_ready: true,
    machine_failing: [],
    passing: [],
    failing: ["manual_ux_hash_bound"],
  },
  source_home_restart: {
    path: sourceHomeRestartPath,
    sha256: sourceHomeRestartSha256,
    ok: true,
    schema: "elastos.mac-source-home-restart/v1",
    error: null,
    generated_at: sourceHomeRestart.generated_at,
    home_url: sourceHomeRestart.home_url,
    http_code: sourceHomeRestart.http_code,
    served_index_sha256: sourceHomeRestart.served_index_sha256,
    installed_index_sha256: sourceHomeRestart.installed_index_sha256,
    source_index_sha256: sourceHomeRestart.source_index_sha256,
    browser_helper_source_sha256: sourceHomeRestart.browser_helper_source_sha256,
    browser_helper_installed_sha256: sourceHomeRestart.browser_helper_installed_sha256,
    browser_helper_initrd_sha256: sourceHomeRestart.browser_helper_initrd_sha256,
    browser_helper_rootfs_sha256: sourceHomeRestart.browser_helper_rootfs_sha256,
  },
  authenticated_profile: {
    persistent_virtual_auth_profile: proof.virtual_auth?.persistent_profile === true,
    cleanup_passkey: proof.virtual_auth?.cleanup_passkey ?? null,
    auth_profile_requested: true,
    auth_setup_receipt: {
      path: receiptPath,
      sha256: receiptSha256,
      ok: true,
      proof_used_persistent_profile: proof.virtual_auth?.persistent_profile === true,
      schema: "elastos.browser.mac-vm-auth-profile-setup/v1",
      error: null,
      open_url: "https://ela.city/channels",
      profile_matches_auth_profile: true,
    },
  },
  vm_control_restart: {
    schema: proof.vm_control?.restart?.schema || null,
    fresh_after_restart: proof.vm_control?.restart?.fresh_after_restart === true,
    max_uptime_ms: proof.vm_control?.restart?.max_uptime_ms ?? null,
    actual_uptime_ms: proof.vm_control?.restart?.actual_uptime_ms ?? null,
  },
  profile_reset: {
    requested: proof.profile_reset?.requested === true,
    ok: proof.profile_reset?.ok === true,
    receipt_schema: proof.profile_reset?.receipt?.schema || null,
    receipt_status: proof.profile_reset?.receipt?.status || null,
    removed_profile_disk: proof.profile_reset?.receipt?.removed_profile_disk ?? null,
  },
  next_steps: [],
};
fs.writeFileSync(summaryPath, `${JSON.stringify(summary, null, 2)}\n`);
NODE
}

for proof_name in \
  mac-authenticated \
  mac-resized-authenticated \
  mac-unauthenticated \
  mac-generic-profile \
  mac-url-unchanged \
  mac-raw-diagnostics \
  mac-disposable-authenticated \
  mac-manual-only-edit-profile \
  mac-missing-runtime-media-relay \
  mac-missing-vm-isolation \
  mac-missing-restart-proof \
  mac-stale-restart-proof \
  mac-aggregate-only-zoom \
  mac-missing-profile-reset \
  mac-reset-without-removal \
  mac-leaky-profile-reset
do
  write_handoff_summary "$tmp_dir/${proof_name}.json" "$tmp_dir/${proof_name}-handoff-summary.json"
done

"$node_bin" -e '
  const fs = require("node:fs");
  const summary = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  delete summary.source_home_restart;
  fs.writeFileSync(process.argv[2], `${JSON.stringify(summary, null, 2)}\n`);
' "$tmp_dir/mac-authenticated-handoff-summary.json" "$tmp_dir/mac-missing-source-home-restart-handoff-summary.json"

set +e
"$node_bin" "$repo_root/scripts/browser-mac-vm-acceptance-audit.mjs" \
  --machine-proof "$tmp_dir/mac-authenticated.json" \
  --handoff-summary "$tmp_dir/mac-authenticated-handoff-summary.json" \
  >"$tmp_dir/no-manual-audit.json" \
  2>"$tmp_dir/no-manual-audit.err"
no_manual_status=$?
set -e
if [[ "$no_manual_status" -eq 0 ]]; then
  echo "Mac VM acceptance audit accepted machine proof without manual UX evidence" >&2
  cat "$tmp_dir/no-manual-audit.json" >&2
  exit 1
fi

"$node_bin" -e '
  const fs = require("node:fs");
  const audit = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  const manual = audit.criteria.find((item) => item.id === "manual_ux_hash_bound");
  if (!manual || manual.ok !== false || !String(manual.missing || "").includes("browser-manual-ux-report.mjs --template")) {
    console.error("Mac VM acceptance audit must fail closed without manual UX evidence");
    process.exit(1);
  }
' "$tmp_dir/no-manual-audit.json"

set +e
"$node_bin" "$repo_root/scripts/browser-mac-vm-acceptance-audit.mjs" \
  --machine-proof "$tmp_dir/mac-authenticated.json" \
  --handoff-summary "$tmp_dir/mac-missing-source-home-restart-handoff-summary.json" \
  >"$tmp_dir/missing-source-home-restart-audit.json" \
  2>"$tmp_dir/missing-source-home-restart-audit.err"
missing_source_home_restart_status=$?
set -e
if [[ "$missing_source_home_restart_status" -eq 0 ]]; then
  echo "Mac VM acceptance audit accepted a handoff summary without source-home restart freshness evidence" >&2
  cat "$tmp_dir/missing-source-home-restart-audit.json" >&2
  exit 1
fi

"$node_bin" -e '
  const fs = require("node:fs");
  const audit = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  const restart = audit.criteria.find((item) => item.id === "source_home_restart_freshness");
  if (!restart || restart.ok !== false || !String(restart.missing || "").includes("source-home-restart")) {
    console.error("Mac VM acceptance audit must reject handoffs without source-home restart freshness evidence");
    process.exit(1);
  }
' "$tmp_dir/missing-source-home-restart-audit.json"

set +e
"$node_bin" "$repo_root/scripts/browser-mac-vm-acceptance-audit.mjs" \
  --machine-proof "$tmp_dir/mac-missing-restart-proof.json" \
  --handoff-summary "$tmp_dir/mac-missing-restart-proof-handoff-summary.json" \
  >"$tmp_dir/missing-restart-proof-audit.json" \
  2>"$tmp_dir/missing-restart-proof-audit.err"
missing_restart_status=$?
set -e
if [[ "$missing_restart_status" -eq 0 ]]; then
  echo "Mac VM acceptance audit accepted a proof without VM control-service start evidence" >&2
  cat "$tmp_dir/missing-restart-proof-audit.json" >&2
  exit 1
fi

"$node_bin" -e '
  const fs = require("node:fs");
  const audit = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  const restart = audit.criteria.find((item) => item.id === "vm_control_restart_proof");
  if (!restart || restart.ok !== false || !String(restart.missing || "").includes("vm_control.after.started_at")) {
    console.error("Mac VM acceptance audit must reject machine proofs without VM control-service start evidence");
    process.exit(1);
  }
' "$tmp_dir/missing-restart-proof-audit.json"

set +e
"$node_bin" "$repo_root/scripts/browser-mac-vm-acceptance-audit.mjs" \
  --machine-proof "$tmp_dir/mac-stale-restart-proof.json" \
  --handoff-summary "$tmp_dir/mac-stale-restart-proof-handoff-summary.json" \
  >"$tmp_dir/stale-restart-proof-audit.json" \
  2>"$tmp_dir/stale-restart-proof-audit.err"
stale_restart_status=$?
set -e
if [[ "$stale_restart_status" -eq 0 ]]; then
  echo "Mac VM acceptance audit accepted a proof from a stale VM control-service run" >&2
  cat "$tmp_dir/stale-restart-proof-audit.json" >&2
  exit 1
fi

"$node_bin" -e '
  const fs = require("node:fs");
  const audit = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  const restart = audit.criteria.find((item) => item.id === "vm_control_restart_proof");
  if (!restart || restart.ok !== false || !String(restart.missing || "").includes("fresh_after_restart")) {
    console.error("Mac VM acceptance audit must reject stale VM control-service restart evidence");
    process.exit(1);
  }
' "$tmp_dir/stale-restart-proof-audit.json"

set +e
"$node_bin" "$repo_root/scripts/browser-mac-vm-acceptance-audit.mjs" \
  --machine-proof "$tmp_dir/mac-aggregate-only-zoom.json" \
  --handoff-summary "$tmp_dir/mac-aggregate-only-zoom-handoff-summary.json" \
  >"$tmp_dir/aggregate-only-zoom-audit.json" \
  2>"$tmp_dir/aggregate-only-zoom-audit.err"
aggregate_only_zoom_status=$?
set -e
if [[ "$aggregate_only_zoom_status" -eq 0 ]]; then
  echo "Mac VM acceptance audit accepted aggregate-only performance/zoom evidence" >&2
  cat "$tmp_dir/aggregate-only-zoom-audit.json" >&2
  exit 1
fi

"$node_bin" -e '
  const fs = require("node:fs");
  const audit = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  const zoom = audit.criteria.find((item) => item.id === "performance_zoom");
  if (!zoom || zoom.ok !== false || !String(zoom.missing || "").includes("named performance/zoom sub-check")) {
    console.error("Mac VM acceptance audit must reject aggregate-only performance/zoom evidence");
    process.exit(1);
  }
' "$tmp_dir/aggregate-only-zoom-audit.json"

set +e
"$node_bin" "$repo_root/scripts/browser-mac-vm-acceptance-audit.mjs" \
  --machine-proof "$tmp_dir/mac-missing-runtime-media-relay.json" \
  --handoff-summary "$tmp_dir/mac-missing-runtime-media-relay-handoff-summary.json" \
  >"$tmp_dir/missing-runtime-media-relay-audit.json" \
  2>"$tmp_dir/missing-runtime-media-relay-audit.err"
missing_runtime_media_relay_status=$?
set -e
if [[ "$missing_runtime_media_relay_status" -eq 0 ]]; then
  echo "Mac VM acceptance audit accepted a proof without Runtime media relay evidence" >&2
  cat "$tmp_dir/missing-runtime-media-relay-audit.json" >&2
  exit 1
fi

"$node_bin" -e '
  const fs = require("node:fs");
  const audit = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  const relay = audit.criteria.find((item) => item.id === "runtime_media_relay");
  if (!relay || relay.ok !== false || !String(relay.missing || "").includes("media_transport=runtime_relay")) {
    console.error("Mac VM acceptance audit must reject proofs without Runtime media relay evidence");
    process.exit(1);
  }
' "$tmp_dir/missing-runtime-media-relay-audit.json"

set +e
"$node_bin" "$repo_root/scripts/browser-mac-vm-acceptance-audit.mjs" \
  --machine-proof "$tmp_dir/mac-missing-vm-isolation.json" \
  --handoff-summary "$tmp_dir/mac-missing-vm-isolation-handoff-summary.json" \
  >"$tmp_dir/missing-vm-isolation-audit.json" \
  2>"$tmp_dir/missing-vm-isolation-audit.err"
missing_vm_isolation_status=$?
set -e
if [[ "$missing_vm_isolation_status" -eq 0 ]]; then
  echo "Mac VM acceptance audit accepted a proof without Browser VM isolation identity" >&2
  cat "$tmp_dir/missing-vm-isolation-audit.json" >&2
  exit 1
fi

"$node_bin" -e '
  const fs = require("node:fs");
  const audit = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  const vm = audit.criteria.find((item) => item.id === "browser_vm_isolation");
  if (!vm || vm.ok !== false || !String(vm.missing || "").includes("per_launch_vm_target")) {
    console.error("Mac VM acceptance audit must reject proofs without Browser VM isolation identity");
    process.exit(1);
  }
' "$tmp_dir/missing-vm-isolation-audit.json"

"$node_bin" "$repo_root/scripts/browser-manual-ux-report.mjs" \
  --template \
  --machine-artifact "$tmp_dir/mac-authenticated.json" \
  >"$tmp_dir/manual-auth-template.json"
"$node_bin" "$repo_root/scripts/browser-manual-ux-report.mjs" \
  --template \
  --machine-artifact "$tmp_dir/mac-resized-authenticated.json" \
  >"$tmp_dir/manual-resized-template.json"
"$node_bin" "$repo_root/scripts/browser-manual-ux-report.mjs" \
  --template \
  --machine-artifact "$tmp_dir/mac-unauthenticated.json" \
  >"$tmp_dir/manual-unauth-template.json"
"$node_bin" "$repo_root/scripts/browser-manual-ux-report.mjs" \
  --template \
  --machine-artifact "$tmp_dir/mac-generic-profile.json" \
  >"$tmp_dir/manual-generic-profile-template.json"
"$node_bin" "$repo_root/scripts/browser-manual-ux-report.mjs" \
  --template \
  --machine-artifact "$tmp_dir/mac-url-unchanged.json" \
  >"$tmp_dir/manual-url-unchanged-template.json"
"$node_bin" "$repo_root/scripts/browser-manual-ux-report.mjs" \
  --template \
  --machine-artifact "$tmp_dir/mac-raw-diagnostics.json" \
  >"$tmp_dir/manual-raw-template.json"
"$node_bin" "$repo_root/scripts/browser-manual-ux-report.mjs" \
  --template \
  --machine-artifact "$tmp_dir/mac-disposable-authenticated.json" \
  >"$tmp_dir/manual-disposable-template.json"
"$node_bin" "$repo_root/scripts/browser-manual-ux-report.mjs" \
  --template \
  --machine-artifact "$tmp_dir/mac-manual-only-edit-profile.json" \
  >"$tmp_dir/manual-manual-only-template.json"
"$node_bin" "$repo_root/scripts/browser-manual-ux-report.mjs" \
  --template \
  --machine-artifact "$tmp_dir/mac-missing-profile-reset.json" \
  >"$tmp_dir/manual-missing-profile-reset-template.json"
"$node_bin" "$repo_root/scripts/browser-manual-ux-report.mjs" \
  --template \
  --machine-artifact "$tmp_dir/mac-reset-without-removal.json" \
  >"$tmp_dir/manual-reset-without-removal-template.json"
"$node_bin" "$repo_root/scripts/browser-manual-ux-report.mjs" \
  --template \
  --machine-artifact "$tmp_dir/mac-leaky-profile-reset.json" \
  >"$tmp_dir/manual-leaky-profile-reset-template.json"

printf 'redacted Mac Browser VM acceptance review recording fixture\n' >"$tmp_dir/mac-acceptance-review-recording.txt"
review_artifact_sha="$("$node_bin" -e 'const fs = require("node:fs"); const crypto = require("node:crypto"); process.stdout.write(crypto.createHash("sha256").update(fs.readFileSync(process.argv[1])).digest("hex"));' "$tmp_dir/mac-acceptance-review-recording.txt")"

"$node_bin" -e '
  const fs = require("node:fs");
  const [artifactPath, artifactSha, ...paths] = process.argv.slice(1);
  for (const path of paths) {
    const report = JSON.parse(fs.readFileSync(path, "utf8"));
    report.ok = true;
    report.reviewed_at = "2026-06-19T00:00:00Z";
    report.reviewer = "mac-vm-acceptance-audit-smoke";
    for (const key of Object.keys(report.checks)) {
      report.checks[key] = true;
      report.evidence[key] = `observed ${key}`;
    }
    report.evidence.ela_city_edit_profile_modal = "Edit Profile modal opened in the Mac VM Browser";
    report.review_artifacts = [{
      kind: "screen_recording",
      path: artifactPath,
      sha256: artifactSha,
      description: "Redacted Mac Browser VM manual review artifact for acceptance audit smoke.",
      redacted: true,
    }];
    fs.writeFileSync(path.replace("-template.json", "-manual.json"), `${JSON.stringify(report, null, 2)}\n`);
  }
' "$tmp_dir/mac-acceptance-review-recording.txt" "$review_artifact_sha" "$tmp_dir/manual-auth-template.json" "$tmp_dir/manual-resized-template.json" "$tmp_dir/manual-unauth-template.json" "$tmp_dir/manual-generic-profile-template.json" "$tmp_dir/manual-url-unchanged-template.json" "$tmp_dir/manual-raw-template.json" "$tmp_dir/manual-disposable-template.json" "$tmp_dir/manual-manual-only-template.json" "$tmp_dir/manual-missing-profile-reset-template.json" "$tmp_dir/manual-reset-without-removal-template.json" "$tmp_dir/manual-leaky-profile-reset-template.json"

set +e
"$node_bin" "$repo_root/scripts/browser-mac-vm-acceptance-audit.mjs" \
  --machine-proof "$tmp_dir/mac-unauthenticated.json" \
  --manual-ux "$tmp_dir/manual-unauth-manual.json" \
  --handoff-summary "$tmp_dir/mac-unauthenticated-handoff-summary.json" \
  >"$tmp_dir/unauth-audit.json" \
  2>"$tmp_dir/unauth-audit.err"
unauth_status=$?
set -e
if [[ "$unauth_status" -eq 0 ]]; then
  echo "Mac VM acceptance audit accepted unauthenticated ela.city diagnostics for edit-profile acceptance" >&2
  cat "$tmp_dir/unauth-audit.json" >&2
  exit 1
fi

"$node_bin" -e '
  const fs = require("node:fs");
  const audit = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  const auth = audit.criteria.find((item) => item.id === "ela_city_authenticated_surface");
  if (!auth || auth.ok !== false || audit.ela_city_diagnostics?.looks_unauthenticated !== true) {
    console.error("Mac VM acceptance audit must reject unauthenticated ela.city diagnostics");
    process.exit(1);
  }
' "$tmp_dir/unauth-audit.json"

set +e
"$node_bin" "$repo_root/scripts/browser-mac-vm-acceptance-audit.mjs" \
  --machine-proof "$tmp_dir/mac-generic-profile.json" \
  --manual-ux "$tmp_dir/manual-generic-profile-manual.json" \
  --handoff-summary "$tmp_dir/mac-generic-profile-handoff-summary.json" \
  >"$tmp_dir/generic-profile-audit.json" \
  2>"$tmp_dir/generic-profile-audit.err"
generic_profile_status=$?
set -e
if [[ "$generic_profile_status" -eq 0 ]]; then
  echo "Mac VM acceptance audit accepted generic Profile text as edit-profile proof" >&2
  cat "$tmp_dir/generic-profile-audit.json" >&2
  exit 1
fi

"$node_bin" -e '
  const fs = require("node:fs");
  const audit = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  const auth = audit.criteria.find((item) => item.id === "ela_city_authenticated_surface");
  const edit = audit.criteria.find((item) => item.id === "ela_city_edit_profile_modal");
  if (!auth || auth.ok !== false || audit.ela_city_diagnostics?.looks_unauthenticated !== true) {
    console.error("Mac VM acceptance audit must not treat generic Profile text as authenticated ela.city proof");
    process.exit(1);
  }
  if (!edit || edit.ok !== false || audit.ela_city_diagnostics?.has_edit_profile_diagnostic_click !== false) {
    console.error("Mac VM acceptance audit must not treat generic Profile text as edit-profile modal proof");
    process.exit(1);
  }
' "$tmp_dir/generic-profile-audit.json"

set +e
"$node_bin" "$repo_root/scripts/browser-mac-vm-acceptance-audit.mjs" \
  --machine-proof "$tmp_dir/mac-url-unchanged.json" \
  --manual-ux "$tmp_dir/manual-url-unchanged-manual.json" \
  --handoff-summary "$tmp_dir/mac-url-unchanged-handoff-summary.json" \
  >"$tmp_dir/url-unchanged-audit.json" \
  2>"$tmp_dir/url-unchanged-audit.err"
url_unchanged_status=$?
set -e
if [[ "$url_unchanged_status" -eq 0 ]]; then
  echo "Mac VM acceptance audit accepted ela.city URL-sync evidence that did not navigate away from the starting URL" >&2
  cat "$tmp_dir/url-unchanged-audit.json" >&2
  exit 1
fi

"$node_bin" -e '
  const fs = require("node:fs");
  const audit = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  const urlSync = audit.criteria.find((item) => item.id === "ela_city_url_sync");
  if (
    !urlSync ||
    urlSync.ok !== false ||
    audit.ela_city_diagnostics?.click_changed_from_starting_url !== false ||
    !String(urlSync.missing || "").includes("changed URL matching the recorded expected_url_re")
  ) {
    console.error("Mac VM acceptance audit must reject URL-sync evidence that stays on the starting ela.city URL");
    process.exit(1);
  }
' "$tmp_dir/url-unchanged-audit.json"

set +e
"$node_bin" "$repo_root/scripts/browser-mac-vm-acceptance-audit.mjs" \
  --machine-proof "$tmp_dir/mac-raw-diagnostics.json" \
  --manual-ux "$tmp_dir/manual-raw-manual.json" \
  --handoff-summary "$tmp_dir/mac-raw-diagnostics-handoff-summary.json" \
  >"$tmp_dir/raw-diagnostics-audit.json" \
  2>"$tmp_dir/raw-diagnostics-audit.err"
raw_diagnostics_status=$?
set -e
if [[ "$raw_diagnostics_status" -eq 0 ]]; then
  echo "Mac VM acceptance audit accepted raw DOM/CDP diagnostics" >&2
  cat "$tmp_dir/raw-diagnostics-audit.json" >&2
  exit 1
fi

"$node_bin" -e '
  const fs = require("node:fs");
  const audit = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  const sanitized = audit.criteria.find((item) => item.id === "sanitized_diagnostics");
  const rawFields = audit.ela_city_diagnostics?.raw_diagnostic_fields || [];
  if (!sanitized || sanitized.ok !== false) {
    console.error("Mac VM acceptance audit must reject raw DOM/CDP diagnostic fields");
    process.exit(1);
  }
  for (const field of ["page_diagnostics.body_html", "diagnostic_click_actions[0].diagnostics.root_html", "diagnostic_click_actions[0].diagnostics.cdp_event_samples"]) {
    if (!rawFields.includes(field)) {
      console.error(`Mac VM acceptance audit did not report raw field ${field}`);
      process.exit(1);
    }
  }
' "$tmp_dir/raw-diagnostics-audit.json"

set +e
"$node_bin" "$repo_root/scripts/browser-mac-vm-acceptance-audit.mjs" \
  --machine-proof "$tmp_dir/mac-disposable-authenticated.json" \
  --manual-ux "$tmp_dir/manual-disposable-manual.json" \
  --handoff-summary "$tmp_dir/mac-disposable-authenticated-handoff-summary.json" \
  >"$tmp_dir/disposable-auth-audit.json" \
  2>"$tmp_dir/disposable-auth-audit.err"
disposable_auth_status=$?
set -e
if [[ "$disposable_auth_status" -eq 0 ]]; then
  echo "Mac VM acceptance audit accepted authenticated ela.city evidence from a disposable virtual profile" >&2
  cat "$tmp_dir/disposable-auth-audit.json" >&2
  exit 1
fi

"$node_bin" -e '
  const fs = require("node:fs");
  const audit = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  const profile = audit.criteria.find((item) => item.id === "ela_city_auth_profile_persistence");
  if (!profile || profile.ok !== false || audit.ela_city_diagnostics?.persistent_virtual_auth_profile !== false) {
    console.error("Mac VM acceptance audit must reject authenticated ela.city evidence from disposable virtual auth profiles");
    process.exit(1);
  }
' "$tmp_dir/disposable-auth-audit.json"

set +e
"$node_bin" "$repo_root/scripts/browser-mac-vm-acceptance-audit.mjs" \
  --machine-proof "$tmp_dir/mac-missing-profile-reset.json" \
  --manual-ux "$tmp_dir/manual-missing-profile-reset-manual.json" \
  --handoff-summary "$tmp_dir/mac-missing-profile-reset-handoff-summary.json" \
  >"$tmp_dir/missing-profile-reset-audit.json" \
  2>"$tmp_dir/missing-profile-reset-audit.err"
missing_profile_reset_status=$?
set -e
if [[ "$missing_profile_reset_status" -eq 0 ]]; then
  echo "Mac VM acceptance audit accepted Browser storage acceptance without profile reset proof" >&2
  cat "$tmp_dir/missing-profile-reset-audit.json" >&2
  exit 1
fi

"$node_bin" -e '
  const fs = require("node:fs");
  const audit = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  const reset = audit.criteria.find((item) => item.id === "profile_reset_safety");
  if (!reset || reset.ok !== false || audit.ela_city_diagnostics?.profile_reset?.requested !== false) {
    console.error("Mac VM acceptance audit must reject Browser storage acceptance without profile reset proof");
    process.exit(1);
  }
' "$tmp_dir/missing-profile-reset-audit.json"

set +e
"$node_bin" "$repo_root/scripts/browser-mac-vm-acceptance-audit.mjs" \
  --machine-proof "$tmp_dir/mac-reset-without-removal.json" \
  --manual-ux "$tmp_dir/manual-reset-without-removal-manual.json" \
  --handoff-summary "$tmp_dir/mac-reset-without-removal-handoff-summary.json" \
  >"$tmp_dir/reset-without-removal-audit.json" \
  2>"$tmp_dir/reset-without-removal-audit.err"
reset_without_removal_status=$?
set -e
if [[ "$reset_without_removal_status" -eq 0 ]]; then
  echo "Mac VM acceptance audit accepted a Browser profile reset proof that did not remove a profile disk" >&2
  cat "$tmp_dir/reset-without-removal-audit.json" >&2
  exit 1
fi

"$node_bin" -e '
  const fs = require("node:fs");
  const audit = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  const reset = audit.criteria.find((item) => item.id === "profile_reset_safety");
  if (!reset || reset.ok !== false || audit.ela_city_diagnostics?.profile_reset?.removed_profile_disk !== false) {
    console.error("Mac VM acceptance audit must reject profile reset proofs that did not remove a profile disk");
    process.exit(1);
  }
' "$tmp_dir/reset-without-removal-audit.json"

set +e
"$node_bin" "$repo_root/scripts/browser-mac-vm-acceptance-audit.mjs" \
  --machine-proof "$tmp_dir/mac-leaky-profile-reset.json" \
  --manual-ux "$tmp_dir/manual-leaky-profile-reset-manual.json" \
  --handoff-summary "$tmp_dir/mac-leaky-profile-reset-handoff-summary.json" \
  >"$tmp_dir/leaky-profile-reset-audit.json" \
  2>"$tmp_dir/leaky-profile-reset-audit.err"
leaky_profile_reset_status=$?
set -e
if [[ "$leaky_profile_reset_status" -eq 0 ]]; then
  echo "Mac VM acceptance audit accepted a Browser profile reset receipt that leaks raw authority" >&2
  cat "$tmp_dir/leaky-profile-reset-audit.json" >&2
  exit 1
fi

"$node_bin" -e '
  const fs = require("node:fs");
  const audit = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  const reset = audit.criteria.find((item) => item.id === "profile_reset_safety");
  if (!reset || reset.ok !== false || audit.ela_city_diagnostics?.profile_reset?.leaked_authority !== true) {
    console.error("Mac VM acceptance audit must reject profile reset receipts that leak profile keys, principals, or host paths");
    process.exit(1);
  }
' "$tmp_dir/leaky-profile-reset-audit.json"

set +e
"$node_bin" "$repo_root/scripts/browser-mac-vm-acceptance-audit.mjs" \
  --machine-proof "$tmp_dir/mac-manual-only-edit-profile.json" \
  --manual-ux "$tmp_dir/manual-manual-only-manual.json" \
  --handoff-summary "$tmp_dir/mac-manual-only-edit-profile-handoff-summary.json" \
  >"$tmp_dir/manual-only-edit-profile-audit.json" \
  2>"$tmp_dir/manual-only-edit-profile-audit.err"
manual_only_edit_profile_status=$?
set -e
if [[ "$manual_only_edit_profile_status" -eq 0 ]]; then
  echo "Mac VM acceptance audit accepted edit-profile modal evidence without machine diagnostic-click proof" >&2
  cat "$tmp_dir/manual-only-edit-profile-audit.json" >&2
  exit 1
fi

"$node_bin" -e '
  const fs = require("node:fs");
  const audit = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  const modal = audit.criteria.find((item) => item.id === "ela_city_edit_profile_modal");
  if (!modal || modal.ok !== false || audit.ela_city_diagnostics?.has_edit_profile_diagnostic_click !== false) {
    console.error("Mac VM acceptance audit must reject manual-only edit-profile modal evidence");
    process.exit(1);
  }
' "$tmp_dir/manual-only-edit-profile-audit.json"

set +e
"$node_bin" "$repo_root/scripts/browser-mac-vm-acceptance-audit.mjs" \
  --machine-proof "$tmp_dir/mac-authenticated.json" \
  --manual-ux "$tmp_dir/manual-auth-manual.json" \
  >"$tmp_dir/missing-handoff-summary-audit.json" \
  2>"$tmp_dir/missing-handoff-summary-audit.err"
missing_handoff_summary_status=$?
set -e
if [[ "$missing_handoff_summary_status" -eq 0 ]]; then
  echo "Mac VM acceptance audit accepted authenticated evidence without a handoff summary" >&2
  cat "$tmp_dir/missing-handoff-summary-audit.json" >&2
  exit 1
fi

"$node_bin" -e '
  const fs = require("node:fs");
  const audit = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  const handoff = audit.criteria.find((item) => item.id === "auth_setup_receipt_chain");
  if (!handoff || handoff.ok !== false || !String(handoff.missing || "").includes("--handoff-summary")) {
    console.error("Mac VM acceptance audit must reject authenticated evidence without a handoff summary");
    process.exit(1);
  }
' "$tmp_dir/missing-handoff-summary-audit.json"

"$node_bin" -e '
  const fs = require("node:fs");
  const [sourcePath, outPath] = process.argv.slice(1);
  const summary = JSON.parse(fs.readFileSync(sourcePath, "utf8"));
  summary.machine_proof.sha256 = "0".repeat(64);
  fs.writeFileSync(outPath, `${JSON.stringify(summary, null, 2)}\n`);
' "$tmp_dir/mac-authenticated-handoff-summary.json" "$tmp_dir/mac-mismatched-handoff-summary.json"

set +e
"$node_bin" "$repo_root/scripts/browser-mac-vm-acceptance-audit.mjs" \
  --machine-proof "$tmp_dir/mac-authenticated.json" \
  --manual-ux "$tmp_dir/manual-auth-manual.json" \
  --handoff-summary "$tmp_dir/mac-mismatched-handoff-summary.json" \
  >"$tmp_dir/mismatched-handoff-summary-audit.json" \
  2>"$tmp_dir/mismatched-handoff-summary-audit.err"
mismatched_handoff_summary_status=$?
set -e
if [[ "$mismatched_handoff_summary_status" -eq 0 ]]; then
  echo "Mac VM acceptance audit accepted a handoff summary for a different machine proof" >&2
  cat "$tmp_dir/mismatched-handoff-summary-audit.json" >&2
  exit 1
fi

"$node_bin" -e '
  const fs = require("node:fs");
  const audit = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  const handoff = audit.criteria.find((item) => item.id === "auth_setup_receipt_chain");
  if (!handoff || handoff.ok !== false || !audit.handoff_summary?.validation?.errors?.some((error) => error.includes("machine_proof.sha256"))) {
    console.error("Mac VM acceptance audit must reject mismatched handoff summary machine proof hashes");
    process.exit(1);
  }
' "$tmp_dir/mismatched-handoff-summary-audit.json"

"$node_bin" -e '
  const fs = require("node:fs");
  const [sourcePath, outPath] = process.argv.slice(1);
  const summary = JSON.parse(fs.readFileSync(sourcePath, "utf8"));
  summary.authenticated_profile.auth_setup_receipt.sha256 = "0".repeat(64);
  fs.writeFileSync(outPath, `${JSON.stringify(summary, null, 2)}\n`);
' "$tmp_dir/mac-authenticated-handoff-summary.json" "$tmp_dir/mac-mismatched-auth-receipt-summary.json"

set +e
"$node_bin" "$repo_root/scripts/browser-mac-vm-acceptance-audit.mjs" \
  --machine-proof "$tmp_dir/mac-authenticated.json" \
  --manual-ux "$tmp_dir/manual-auth-manual.json" \
  --handoff-summary "$tmp_dir/mac-mismatched-auth-receipt-summary.json" \
  >"$tmp_dir/mismatched-auth-receipt-audit.json" \
  2>"$tmp_dir/mismatched-auth-receipt-audit.err"
mismatched_auth_receipt_status=$?
set -e
if [[ "$mismatched_auth_receipt_status" -eq 0 ]]; then
  echo "Mac VM acceptance audit accepted a handoff summary with a mismatched auth setup receipt digest" >&2
  cat "$tmp_dir/mismatched-auth-receipt-audit.json" >&2
  exit 1
fi

"$node_bin" -e '
  const fs = require("node:fs");
  const audit = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  const handoff = audit.criteria.find((item) => item.id === "auth_setup_receipt_chain");
  if (!handoff || handoff.ok !== false || !audit.handoff_summary?.validation?.errors?.some((error) => error.includes("auth setup receipt sha256 must match"))) {
    console.error("Mac VM acceptance audit must reject mismatched auth setup receipt hashes");
    process.exit(1);
  }
' "$tmp_dir/mismatched-auth-receipt-audit.json"

"$node_bin" -e '
  const fs = require("node:fs");
  const crypto = require("node:crypto");
  const [sourcePath, outPath, receiptPath] = process.argv.slice(1);
  const summary = JSON.parse(fs.readFileSync(sourcePath, "utf8"));
  const receipt = JSON.parse(fs.readFileSync(summary.authenticated_profile.auth_setup_receipt.path, "utf8"));
  delete receipt.setup.authentication_claim;
  delete receipt.setup.authentication_proof;
  fs.writeFileSync(receiptPath, `${JSON.stringify(receipt, null, 2)}\n`);
  summary.authenticated_profile.auth_setup_receipt.path = receiptPath;
  summary.authenticated_profile.auth_setup_receipt.sha256 = crypto.createHash("sha256").update(fs.readFileSync(receiptPath)).digest("hex");
  fs.writeFileSync(outPath, `${JSON.stringify(summary, null, 2)}\n`);
' "$tmp_dir/mac-authenticated-handoff-summary.json" "$tmp_dir/mac-ambiguous-auth-receipt-summary.json" "$tmp_dir/mac-ambiguous-auth-receipt.json"

set +e
"$node_bin" "$repo_root/scripts/browser-mac-vm-acceptance-audit.mjs" \
  --machine-proof "$tmp_dir/mac-authenticated.json" \
  --manual-ux "$tmp_dir/manual-auth-manual.json" \
  --handoff-summary "$tmp_dir/mac-ambiguous-auth-receipt-summary.json" \
  >"$tmp_dir/ambiguous-auth-receipt-audit.json" \
  2>"$tmp_dir/ambiguous-auth-receipt-audit.err"
ambiguous_auth_receipt_status=$?
set -e
if [[ "$ambiguous_auth_receipt_status" -eq 0 ]]; then
  echo "Mac VM acceptance audit accepted an ambiguous auth setup receipt that could be read as authentication proof" >&2
  cat "$tmp_dir/ambiguous-auth-receipt-audit.json" >&2
  exit 1
fi

"$node_bin" -e '
  const fs = require("node:fs");
  const audit = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  const handoff = audit.criteria.find((item) => item.id === "auth_setup_receipt_chain");
  const errors = audit.handoff_summary?.validation?.errors || [];
  if (
    !handoff ||
    handoff.ok !== false ||
    !errors.some((error) => error.includes("must not claim ela.city authentication by itself")) ||
    !errors.some((error) => error.includes("must defer authentication proof"))
  ) {
    console.error("Mac VM acceptance audit must reject ambiguous auth setup receipts");
    process.exit(1);
  }
' "$tmp_dir/ambiguous-auth-receipt-audit.json"

"$node_bin" -e '
  const fs = require("node:fs");
  const crypto = require("node:crypto");
  const [sourcePath, outPath, receiptPath] = process.argv.slice(1);
  const summary = JSON.parse(fs.readFileSync(sourcePath, "utf8"));
  const receipt = JSON.parse(fs.readFileSync(summary.authenticated_profile.auth_setup_receipt.path, "utf8"));
  receipt.generated_at = "2026-06-20T00:00:00.000Z";
  fs.writeFileSync(receiptPath, `${JSON.stringify(receipt, null, 2)}\n`);
  summary.authenticated_profile.auth_setup_receipt.path = receiptPath;
  summary.authenticated_profile.auth_setup_receipt.sha256 = crypto.createHash("sha256").update(fs.readFileSync(receiptPath)).digest("hex");
  fs.writeFileSync(outPath, `${JSON.stringify(summary, null, 2)}\n`);
' "$tmp_dir/mac-authenticated-handoff-summary.json" "$tmp_dir/mac-receipt-after-proof-summary.json" "$tmp_dir/mac-receipt-after-proof.json"

set +e
"$node_bin" "$repo_root/scripts/browser-mac-vm-acceptance-audit.mjs" \
  --machine-proof "$tmp_dir/mac-authenticated.json" \
  --manual-ux "$tmp_dir/manual-auth-manual.json" \
  --handoff-summary "$tmp_dir/mac-receipt-after-proof-summary.json" \
  >"$tmp_dir/receipt-after-proof-audit.json" \
  2>"$tmp_dir/receipt-after-proof-audit.err"
receipt_after_proof_status=$?
set -e
if [[ "$receipt_after_proof_status" -eq 0 ]]; then
  echo "Mac VM acceptance audit accepted auth setup receipt generated after the machine proof" >&2
  cat "$tmp_dir/receipt-after-proof-audit.json" >&2
  exit 1
fi

"$node_bin" -e '
  const fs = require("node:fs");
  const audit = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  const handoff = audit.criteria.find((item) => item.id === "auth_setup_receipt_chain");
  if (!handoff || handoff.ok !== false || !audit.handoff_summary?.validation?.errors?.some((error) => error.includes("auth setup receipt generated_at must be at or before machine proof generated_at"))) {
    console.error("Mac VM acceptance audit must reject auth setup receipts generated after the machine proof");
    process.exit(1);
  }
' "$tmp_dir/receipt-after-proof-audit.json"

"$node_bin" -e '
  const fs = require("node:fs");
  const [sourcePath, outPath] = process.argv.slice(1);
  const summary = JSON.parse(fs.readFileSync(sourcePath, "utf8"));
  summary.generated_at = "2026-06-18T23:59:59.000Z";
  fs.writeFileSync(outPath, `${JSON.stringify(summary, null, 2)}\n`);
' "$tmp_dir/mac-authenticated-handoff-summary.json" "$tmp_dir/mac-summary-before-proof.json"

set +e
"$node_bin" "$repo_root/scripts/browser-mac-vm-acceptance-audit.mjs" \
  --machine-proof "$tmp_dir/mac-authenticated.json" \
  --manual-ux "$tmp_dir/manual-auth-manual.json" \
  --handoff-summary "$tmp_dir/mac-summary-before-proof.json" \
  >"$tmp_dir/summary-before-proof-audit.json" \
  2>"$tmp_dir/summary-before-proof-audit.err"
summary_before_proof_status=$?
set -e
if [[ "$summary_before_proof_status" -eq 0 ]]; then
  echo "Mac VM acceptance audit accepted a handoff summary generated before the machine proof" >&2
  cat "$tmp_dir/summary-before-proof-audit.json" >&2
  exit 1
fi

"$node_bin" -e '
  const fs = require("node:fs");
  const audit = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  const handoff = audit.criteria.find((item) => item.id === "auth_setup_receipt_chain");
  if (!handoff || handoff.ok !== false || !audit.handoff_summary?.validation?.errors?.some((error) => error.includes("handoff summary generated_at must be at or after machine proof generated_at"))) {
    console.error("Mac VM acceptance audit must reject handoff summaries generated before the machine proof");
    process.exit(1);
  }
' "$tmp_dir/summary-before-proof-audit.json"

"$node_bin" "$repo_root/scripts/browser-mac-vm-acceptance-audit.mjs" \
  --machine-proof "$tmp_dir/mac-authenticated.json" \
  --manual-ux "$tmp_dir/manual-auth-manual.json" \
  --handoff-summary "$tmp_dir/mac-authenticated-handoff-summary.json" \
  >"$tmp_dir/auth-audit.json"

"$node_bin" -e '
  const fs = require("node:fs");
  const audit = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  if (audit.ok !== true) {
    console.error("Mac VM acceptance audit must accept authenticated machine proof plus hash-bound manual UX evidence");
    process.exit(1);
  }
  for (const id of ["runtime_only_network", "vm_control_restart_proof", "source_home_restart_freshness", "remote_video_input", "browser_vm_isolation", "runtime_media_relay", "performance_zoom", "ela_city_url_sync", "ela_city_images", "sanitized_diagnostics", "ela_city_auth_profile_persistence", "auth_setup_receipt_chain", "profile_reset_safety", "manual_ux_hash_bound", "manual_video_input_performance", "ela_city_authenticated_surface", "ela_city_edit_profile_modal", "authority_and_cleanup"]) {
    const item = audit.criteria.find((candidate) => candidate.id === id);
    if (!item || item.ok !== true) {
      console.error(`Mac VM acceptance audit missing passing criterion ${id}`);
      process.exit(1);
    }
  }
' "$tmp_dir/auth-audit.json"

"$node_bin" "$repo_root/scripts/browser-mac-vm-acceptance-audit.mjs" \
  --machine-proof "$tmp_dir/mac-resized-authenticated.json" \
  --manual-ux "$tmp_dir/manual-resized-manual.json" \
  --handoff-summary "$tmp_dir/mac-resized-authenticated-handoff-summary.json" \
  >"$tmp_dir/resized-auth-audit.json"

"$node_bin" -e '
  const fs = require("node:fs");
  const audit = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  const proof = JSON.parse(fs.readFileSync(process.argv[2], "utf8"));
  const zoom = audit.criteria.find((candidate) => candidate.id === "performance_zoom");
  if (audit.ok !== true || !zoom || zoom.ok !== true) {
    console.error("Mac VM acceptance audit must accept authenticated resized viewport proof");
    process.exit(1);
  }
  if (
    proof.quality_gates?.thresholds?.expected_viewport_width !== 1000 ||
    proof.quality_gates?.thresholds?.expected_viewport_height !== 700 ||
    proof.quality_gates?.zoom?.viewport_width !== 1000 ||
    proof.quality_gates?.zoom?.viewport_height !== 700
  ) {
    console.error("resized Mac VM acceptance smoke must exercise requested viewport thresholds");
    process.exit(1);
  }
' "$tmp_dir/resized-auth-audit.json" "$tmp_dir/mac-resized-authenticated.json"

printf '{"schema":"elastos.browser.mac-vm-acceptance-audit-smoke/v1","ok":true,"missing_manual_rejected":true,"missing_source_home_restart_rejected":true,"missing_restart_proof_rejected":true,"stale_restart_proof_rejected":true,"aggregate_only_zoom_rejected":true,"missing_runtime_media_relay_rejected":true,"missing_vm_isolation_rejected":true,"unauthenticated_ela_city_rejected":true,"generic_profile_text_rejected":true,"url_unchanged_rejected":true,"raw_diagnostics_rejected":true,"disposable_auth_profile_rejected":true,"missing_profile_reset_rejected":true,"reset_without_removal_rejected":true,"leaky_profile_reset_rejected":true,"manual_only_edit_profile_rejected":true,"missing_handoff_summary_rejected":true,"mismatched_handoff_summary_rejected":true,"mismatched_auth_receipt_rejected":true,"ambiguous_auth_receipt_rejected":true,"receipt_after_proof_rejected":true,"summary_before_proof_rejected":true,"matched_authenticated_manual_accepted":true,"resized_authenticated_manual_accepted":true}\n'
