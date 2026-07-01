#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
tmp_dir="$(mktemp -d /tmp/elastos-browser-provider-runbook-smoke-XXXXXX)"
cleanup() {
  rm -rf "$tmp_dir"
}
trap cleanup EXIT

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
export PATH="$(dirname "$node_bin"):${PATH}"

"$node_bin" "$repo_root/scripts/browser-provider-runbook.mjs" --help >"$tmp_dir/help.txt" 2>&1 || true
node -e '
  const fs = require("node:fs");
  const text = fs.readFileSync(process.argv[1], "utf8");
  const required = [
    "--hosted-bakeoff /path/to/bakeoff.json",
    "--native-preflight /path/to/native-preflight.json",
    "--manual-ux /path/to/manual-ux.json",
    "runbook regenerates the decision report",
    "Do not combine proof artifacts with --decision-report"
  ];
  const missing = required.filter((needle) => !text.includes(needle));
  if (missing.length > 0) {
    console.error(`runbook help missing artifact guidance: ${missing.join(", ")}`);
    process.exit(1);
  }
' "$tmp_dir/help.txt"

cat >"$tmp_dir/decision-report.json" <<'JSON'
{
  "schema": "elastos.browser.provider-decision-report/v1",
  "live_adapter": {
    "kind": "selkies_gstreamer",
    "display_backend": "selkies_gstreamer_webrtc",
    "control_socket": "/tmp/elastos-browser-selkies-live/selkies-control.sock",
    "control_status": {
      "active_pages": 1,
      "page_ids": ["page:selkies-test"],
      "single_session": true
    }
  },
  "selkies_service": {
    "active": "active",
    "enabled": "enabled"
  },
  "docker_is_product_architecture": false,
  "selkies_role": "managed_baseline_not_final_product",
  "native_host_capability": {
    "result": {
      "ready": {
        "product_native": false
      }
    },
    "blockers": [
      "host_audio_service detail=missing"
    ]
  },
  "goal_status": {
    "status": "blocked",
    "reason": "Browser/audio completion requires external provider/native evidence."
  },
  "candidate_readiness": [
    {
      "candidate": "kasm-workspaces",
      "ready_for_bakeoff": false,
      "blockers": [
        "env:KASM_BASE_URL detail=missing",
        "env:KASM_API_KEY detail=missing"
      ]
    },
    {
      "candidate": "browserbox",
      "ready_for_bakeoff": false,
      "blockers": [
        "command:bbx detail=missing",
        "env:BROWSERBOX_LICENSE_CONFIRMED detail=missing"
      ]
    }
  ],
  "objective_audit": {
    "prompt_to_artifact_checklist": [
      {
        "id": "best_path_determined",
        "requirement": "Determine the best Browser path instead of looping on Docker/Selkies.",
        "ok": true,
        "evidence": ["docs/BROWSER_PROVIDER_BAKEOFF.md"],
        "missing": null
      },
      {
        "id": "audio_product_proven",
        "requirement": "Enable and prove working Browser audio in an accepted product provider.",
        "ok": false,
        "evidence": ["scripts/browser-hosted-provider-bakeoff.sh"],
        "missing": "Provide accepted product audio proof."
      },
      {
        "id": "manual_user_acceptance",
        "requirement": "Confirm the browser UX manually against the artifact the user actually sees.",
        "ok": false,
        "evidence": ["scripts/browser-manual-ux-report.mjs"],
        "missing": "Record hash-bound manual UX evidence for typing, address-bar stability, scrolling/click fidelity, hosted WebRTC audio unlock where applicable, audible YouTube audio, Glide wallet connect, no raw authority, and cleanup."
      }
    ]
  },
  "blocked_by": [
    {
      "id": "audio_product_proven",
      "source": "objective_audit",
      "message": "Provide accepted product audio proof."
    },
    {
      "id": "manual_user_acceptance",
      "source": "objective_audit",
      "message": "Record hash-bound manual UX evidence for typing, address-bar stability, scrolling/click fidelity, hosted WebRTC audio unlock where applicable, audible YouTube audio, Glide wallet connect, no raw authority, and cleanup."
    }
  ],
  "recommendation": {
    "next": "Compare Kasm Workspaces first, BrowserBox if licensed."
  },
  "next_action": {
    "id": "free_or_isolate_selkies_before_bakeoff",
    "status": "blocked",
    "owner": "operator",
    "summary": "Close the active Browser page or use a separate provider instance before running more hosted bake-offs.",
    "commands": [
      "node scripts/browser-provider-decision-report.mjs",
      "node scripts/browser-provider-runbook.mjs"
    ]
  }
}
JSON

node "$repo_root/scripts/browser-provider-runbook.mjs" \
  --decision-report "$tmp_dir/decision-report.json" \
  >"$tmp_dir/runbook.md"

node -e '
  const fs = require("node:fs");
  const text = fs.readFileSync(process.argv[1], "utf8");
  const required = [
    "## Objective Checklist",
    "This runbook is read-only guidance.",
    "does not install vendors, launch",
    "preserve it and use a separate provider instance",
    "- [x] `best_path_determined`",
    "- [ ] `audio_product_proven`",
    "Provide accepted product audio proof.",
    "- [ ] `manual_user_acceptance`",
    "Record hash-bound manual UX evidence for",
    "hosted WebRTC audio unlock",
    "advertised audio",
    "user-gesture unlock",
    "unmuted/remote-audio",
    "received-audio evidence",
    "audible YouTube audio",
    "Glide wallet connect",
    "no raw authority",
    "cleanup",
    "Docker is product architecture: `false`",
    "Goal status: `blocked`",
    "Browser/audio completion requires external provider/native evidence.",
    "Warning: the live Selkies baseline is single-session",
    "Selkies session: single-session=`true`, active-pages=`1`, page-ids=`page:selkies-test`",
    "Active page ids: `page:selkies-test`",
    "## Current Host Stop Condition",
    "not accepted as product Browser proof",
    "Do not keep tuning the running Selkies baseline as product architecture.",
    "Current next action is `free_or_isolate_selkies_before_bakeoff`, owned by `operator`, status `blocked`.",
    "single-session and busy",
    "Active Browser page ids: `page:selkies-test`",
    "Operator close helper. First command is a dry run",
    "browser-selkies-close-page.mjs",
    "--confirm-close",
    "this host is not that target",
    "## Blocking Summary",
    "`audio_product_proven`",
    "`manual_user_acceptance`",
    "## Next Action",
    "ID: `free_or_isolate_selkies_before_bakeoff`",
    "Status: `blocked`",
    "Close the active Browser page or use a separate provider instance",
    "## Local Pass Checks",
    "scripts/browser-hosted-product-config-smoke.sh",
    "scripts/browser-objective-audit-smoke.sh",
    "scripts/browser-provider-decision-report-smoke.sh",
    "node scripts/browser-display-mode-smoke.mjs",
    "--artifact-out /opt/elastos/kasm-workspaces/hosted-bakeoff.json",
    "--resize-width 1000",
    "--resize-height 700",
    "--artifact-out /opt/elastos/browserbox/hosted-bakeoff.json",
    "--artifact-out /opt/elastos/native-browser/native-preflight.json",
    "## Expected-Failing Completion Audit",
    "It should exit non-zero until product audio evidence",
    "node scripts/browser-objective-audit.mjs",
    "# Edit manual-ux.json only after testing typing, scrolling, resize/page-scale, hosted WebRTC audio unlock",
    "# where applicable, including advertised audio, user-gesture unlock, unmuted/remote-audio",
    "# status, received-audio evidence, YouTube audible audio, Glide wallet connect,",
    "# no raw authority, and cleanup.",
    "Do not pass placeholder",
    "# Hosted provider completion:",
    "--hosted-bakeoff <accepted-hosted-bakeoff.json>",
    "# Native provider completion:",
    "--native-preflight <accepted-native-preflight.json>",
    "env:KASM_BASE_URL detail=missing",
    "command:bbx detail=missing"
  ];
  const missing = required.filter((needle) => !text.includes(needle));
  if (missing.length > 0) {
    console.error(`runbook smoke missing expected text: ${missing.join(", ")}`);
    process.exit(1);
  }
  if (text.includes("api-key-secret-value") || text.includes("BROWSERBOX_PRODUCT_KEY_VALUE")) {
    console.error("runbook smoke found leaked secret placeholder value");
    process.exit(1);
  }
  const forbidden = [
    "--hosted-bakeoff <hosted-bakeoff.json>",
    "--native-preflight <native-preflight.json>"
  ];
  const stale = forbidden.filter((needle) => text.includes(needle));
  if (stale.length > 0) {
    console.error(`runbook smoke found stale combined audit placeholder: ${stale.join(", ")}`);
    process.exit(1);
  }
' "$tmp_dir/runbook.md"

cat >"$tmp_dir/rejected-hosted-bakeoff.json" <<'JSON'
{
  "ok": false,
  "schema": "elastos.browser.hosted-provider-bakeoff/v1",
  "candidate": "selkies",
  "candidate_gate": {
    "ok": true,
    "status": 0,
    "result": {
      "ok": true
    }
  },
  "youtube_stress": {
    "ok": false,
    "status": 1,
    "error_tail": [
      "hosted media playback did not reach stable video+audio decode"
    ]
  },
  "partial_candidate_ok": true,
  "product_acceptance": "rejected by machine gate"
}
JSON

set +e
node "$repo_root/scripts/browser-provider-runbook.mjs" \
  --decision-report "$tmp_dir/decision-report.json" \
  --hosted-bakeoff "$tmp_dir/rejected-hosted-bakeoff.json" \
  >"$tmp_dir/combined.out" \
  2>"$tmp_dir/combined.err"
combined_status=$?
set -e
if [[ "$combined_status" -eq 0 ]]; then
  echo "runbook accepted stale decision report plus proof artifact flags" >&2
  exit 1
fi
if ! grep -q "cannot be combined with proof artifacts" "$tmp_dir/combined.err"; then
  echo "runbook did not explain decision-report/proof-artifact conflict" >&2
  cat "$tmp_dir/combined.err" >&2
  exit 1
fi

set +e
node "$repo_root/scripts/browser-provider-runbook.mjs" \
  --hosted-bakeoff "$tmp_dir/rejected-hosted-bakeoff.json" \
  >"$tmp_dir/rejected-runbook.md" \
  2>"$tmp_dir/rejected-runbook.err"
rejected_runbook_status=$?
set -e
if [[ "$rejected_runbook_status" -eq 0 ]]; then
  :
else
  echo "runbook failed to render a blocked report from a rejected hosted bake-off" >&2
  cat "$tmp_dir/rejected-runbook.err" >&2
  exit 1
fi

node -e '
  const fs = require("node:fs");
  const text = fs.readFileSync(process.argv[1], "utf8");
  const required = [
    "`hosted_bakeoff_rejected`",
    "passed candidate gates but failed YouTube",
    "Run Kasm Workspaces first",
    "## Completion Gate"
  ];
  const missing = required.filter((needle) => !text.includes(needle));
  if (missing.length > 0) {
    console.error(`runbook generated from supplied hosted bake-off is missing evidence text: ${missing.join(", ")}`);
    process.exit(1);
  }
' "$tmp_dir/rejected-runbook.md"

cat >"$tmp_dir/native-valid.json" <<'JSON'
{
  "schema": "elastos.browser.native-target-preflight/v1",
  "ok": true,
  "out_dir": "/tmp/elastos-native-smoke",
  "browser_program": "/usr/bin/chromium",
  "network_mode": "runtime_net_only",
  "direct_network": false,
  "native_audio_declared": true,
  "native_video_declared": true,
  "native_audio_proven": true,
  "native_video_proven": true,
  "native_media_required": true
}
JSON
native_valid_sha="$(sha256sum "$tmp_dir/native-valid.json" | awk '{print $1}')"
cat >"$tmp_dir/manual-native-matched.json" <<JSON
{
  "schema": "elastos.browser.manual-ux/v1",
  "ok": true,
  "reviewed_at": "2026-05-13T00:00:00Z",
  "reviewer": "runbook-smoke",
  "provider": "fake-native-valid",
  "target": "test",
  "machine_artifact": {
    "schema": "elastos.browser.native-target-preflight/v1",
    "sha256": "$native_valid_sha",
    "path": "$tmp_dir/native-valid.json"
  },
  "checks": {
    "typing_latency": true,
    "address_bar_stability": true,
    "scrolling_click_fidelity": true,
    "youtube_audible_audio": true,
    "glide_wallet_connect": true,
    "no_raw_authority": true,
    "session_cleanup": true
  },
  "evidence": {}
}
JSON

node "$repo_root/scripts/browser-provider-runbook.mjs" \
  --native-preflight "$tmp_dir/native-valid.json" \
  --manual-ux "$tmp_dir/manual-native-matched.json" \
  >"$tmp_dir/native-accepted-runbook.md"

node -e '
  const fs = require("node:fs");
  const text = fs.readFileSync(process.argv[1], "utf8");
  const required = [
    "Goal status: `accepted`",
    "Browser/audio objective has accepted product proof and manual UX evidence.",
    "Keep the accepted product provider proof and matching manual UX report",
    "## Completion Gate"
  ];
  const missing = required.filter((needle) => !text.includes(needle));
  if (missing.length > 0) {
    console.error(`runbook generated from supplied native/manual proof is missing acceptance text: ${missing.join(", ")}`);
    process.exit(1);
  }
  const forbidden = [
    "`native_host_not_product_ready`",
    "`kasm-workspaces_not_ready`"
  ];
  const stale = forbidden.filter((needle) => text.includes(needle));
  if (stale.length > 0) {
    console.error(`accepted runbook kept stale blocker text: ${stale.join(", ")}`);
    process.exit(1);
  }
' "$tmp_dir/native-accepted-runbook.md"

printf '{"schema":"elastos.browser.provider-runbook-smoke/v1","ok":true,"objective_checklist_rendered":true,"missing_audio_visible":true,"missing_manual_ux_visible":true,"local_pass_checks_rendered":true,"expected_failing_completion_audit_rendered":true,"hosted_artifact_forwarding_checked":true,"native_manual_artifact_forwarding_checked":true}\n'
