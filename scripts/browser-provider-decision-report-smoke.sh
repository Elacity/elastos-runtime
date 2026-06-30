#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
tmp_dir="$(mktemp -d /tmp/elastos-browser-provider-decision-report-smoke-XXXXXX)"
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

set +e
"$node_bin" "$repo_root/scripts/browser-provider-decision-report.mjs" \
  >"$tmp_dir/report.json" \
  2>"$tmp_dir/report.err"
status=$?
set -e

node -e '
  const fs = require("node:fs");
  const report = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  const source = fs.readFileSync(process.argv[3], "utf8");
  const status = Number(process.argv[2]);

  function fail(message) {
    console.error(message);
    process.exit(1);
  }

  if (report.schema !== "elastos.browser.provider-decision-report/v1") {
    fail("decision report has wrong schema");
  }
  if (report.docker_is_product_architecture !== false) {
    fail("decision report must not classify Docker/Selkies as product architecture");
  }
  if (report.selkies_role !== "managed_baseline_not_final_product") {
    fail("decision report must keep Selkies as baseline, not accepted product");
  }
  if (!report.goal_status || !["accepted", "blocked", "incomplete"].includes(report.goal_status.status)) {
    fail("decision report must expose goal_status");
  }
  if (!report.next_action?.id || !report.next_action?.status || !report.next_action?.owner) {
    fail("decision report must expose structured next_action");
  }
  if (!source.includes("--out-dir <dir> --browser-program <chromium-or-cef> --native-audio --native-video --require-native-media --artifact-out <dir>/native-preflight.json")) {
    fail("native next_action command must generate a hashable native-preflight artifact");
  }
  if (!source.includes("--resize-width 1000 --resize-height 700 --artifact-out <${nextReady.candidate}-hosted-bakeoff.json>")) {
    fail("hosted next_action command must generate a hashable hosted bake-off artifact and require remote viewport resize proof");
  }
  if (!source.includes("--template --machine-artifact <dir>/native-preflight.json")) {
    fail("native manual UX command must point at the generated native artifact");
  }
  if (!source.includes("--template --machine-artifact <${nextReady.candidate}-hosted-bakeoff.json>")) {
    fail("hosted manual UX command must point at the generated hosted artifact");
  }
  if (!Array.isArray(report.blocked_by)) {
    fail("decision report must expose blocked_by");
  }
  if (!Array.isArray(report.candidate_readiness)) {
    fail("decision report must expose candidate_readiness");
  }
  for (const candidate of ["selkies", "kasm-workspaces", "browserbox", "kasmvnc"]) {
    if (!report.candidate_readiness.some((entry) => entry.candidate === candidate)) {
      fail(`decision report missing candidate readiness for ${candidate}`);
    }
  }
  for (const candidate of ["kasm-workspaces", "browserbox", "kasmvnc"]) {
    const entry = report.candidate_readiness.find((item) => item.candidate === candidate);
    if (entry?.generated_config === true) {
      const blockerText = (entry.blockers || []).join("\n");
      if (/\/tmp\/elastos-browser-/.test(blockerText)) {
        fail(`generated ${candidate} blockers must not expose temporary placeholder socket paths`);
      }
      if (!blockerText.includes("operator_control_socket not provisioned")) {
        fail(`generated ${candidate} blockers must tell the operator to provision a durable control socket`);
      }
    }
  }
  const selkies = report.candidate_readiness.find((entry) => entry.candidate === "selkies");
  const selkiesBusy = report.live_adapter?.control_status?.single_session === true
    && Number(report.live_adapter?.control_status?.active_pages || 0) > 0;
  if (selkiesBusy) {
    if (selkies?.ready_for_bakeoff === true) {
      fail("busy single-session Selkies target must not be marked ready_for_bakeoff");
    }
    if (selkies?.preflight_ready_for_bakeoff !== true) {
      fail("busy Selkies report must preserve the underlying preflight readiness separately");
    }
    if (!selkies.blockers?.some((blocker) => blocker.includes("single-session target"))) {
      fail("busy Selkies report must explain the live-session blocker in candidate_readiness");
    }
    if (report.next_action?.id !== "free_or_isolate_selkies_before_bakeoff") {
      fail("busy Selkies report must make freeing or isolating Selkies the structured next action");
    }
    if (report.next_action?.status !== "blocked" || report.next_action?.owner !== "operator") {
      fail("busy Selkies next action must be operator-owned and blocked");
    }
    if (!String(report.next_action?.summary || "").includes("separate provider instance")) {
      fail("busy Selkies next action must tell operators to use a separate provider instance");
    }
    const pageIds = report.live_adapter?.control_status?.page_ids || [];
    if (pageIds.length === 1) {
      const commands = (report.next_action?.commands || []).join("\n");
      if (!commands.includes("browser-selkies-close-page.mjs") || !commands.includes("--confirm-close")) {
        fail("busy Selkies next action must include the explicit close-page helper command when a page id is known");
      }
    }
    if (/tuning Selkies/i.test(report.next_action?.summary || "")) {
      fail("busy Selkies next action must not recommend more Selkies tuning");
    }
  }
  if (report.ok === true) {
    if (status !== 0 || report.goal_status.status !== "accepted") {
      fail("accepted decision report must exit zero and set goal_status=accepted");
    }
    process.exit(0);
  }
  if (status === 0) {
    fail("non-accepted decision report must exit non-zero");
  }
  const checklist = report.objective_audit?.prompt_to_artifact_checklist || [];
  const missing = checklist.filter((item) => item.ok !== true).map((item) => item.id);
  if (checklist.length > 0) {
    for (const id of ["audio_product_proven", "manual_user_acceptance"]) {
      if (!missing.includes(id)) {
        fail(`non-accepted decision report must keep ${id} visible in objective checklist`);
      }
      if (!report.blocked_by.some((blocker) => blocker.id === id)) {
        fail(`non-accepted decision report must keep ${id} visible in blocked_by`);
      }
    }
  }
  if (!["blocked", "incomplete"].includes(report.goal_status.status)) {
    fail("non-accepted decision report must be blocked or incomplete");
  }
' "$tmp_dir/report.json" "$status" "$repo_root/scripts/browser-provider-decision-report.mjs"

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
node "$repo_root/scripts/browser-provider-decision-report.mjs" \
  --hosted-bakeoff "$tmp_dir/rejected-hosted-bakeoff.json" \
  >"$tmp_dir/rejected-report.json" \
  2>"$tmp_dir/rejected-report.err"
rejected_status=$?
set -e

node -e '
  const fs = require("node:fs");
  const report = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  const status = Number(process.argv[2]);
  function fail(message) {
    console.error(message);
    process.exit(1);
  }
  if (status === 0 || report.ok === true) {
    fail("rejected hosted bake-off report must stay non-accepted");
  }
  if (report.hosted_bakeoff?.candidate_gate_ok !== true || report.hosted_bakeoff?.youtube_stress_ok !== false) {
    fail("decision report must summarize supplied rejected hosted bake-off state");
  }
  if (!String(report.goal_status?.reason || "").includes("passed candidate gates but failed YouTube")) {
    fail("decision report goal status must name the supplied hosted bake-off failure mode");
  }
  if (!report.blocked_by?.some((item) => item.id === "hosted_bakeoff_rejected")) {
    fail("decision report must expose rejected hosted bake-off in blocked_by");
  }
' "$tmp_dir/rejected-report.json" "$rejected_status"

cat >"$tmp_dir/rejected-native-preflight.json" <<'JSON'
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

set +e
node "$repo_root/scripts/browser-provider-decision-report.mjs" \
  --native-preflight "$tmp_dir/rejected-native-preflight.json" \
  >"$tmp_dir/rejected-native-report.json" \
  2>"$tmp_dir/rejected-native-report.err"
rejected_native_status=$?
set -e

node -e '
  const fs = require("node:fs");
  const report = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  const status = Number(process.argv[2]);
  function fail(message) {
    console.error(message);
    process.exit(1);
  }
  if (status === 0 || report.ok === true) {
    fail("rejected native preflight report must stay non-accepted");
  }
  if (report.native_preflight?.raw_ok !== true || report.native_preflight?.product_media_ok !== false) {
    fail("decision report must summarize supplied native preflight media readiness");
  }
  if (report.native_preflight?.native_audio_proven !== false || report.native_preflight?.native_video_proven !== false) {
    fail("decision report must expose missing native audio/video proof");
  }
  if (!String(report.goal_status?.reason || "").includes("native preflight did not prove")) {
    fail("decision report goal status must name the supplied native preflight failure mode");
  }
  if (!report.blocked_by?.some((item) => item.id === "native_preflight_rejected")) {
    fail("decision report must expose rejected native preflight in blocked_by");
  }
' "$tmp_dir/rejected-native-report.json" "$rejected_native_status"

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
  "reviewer": "decision-report-smoke",
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

node "$repo_root/scripts/browser-provider-decision-report.mjs" \
  --native-preflight "$tmp_dir/native-valid.json" \
  --manual-ux "$tmp_dir/manual-native-matched.json" \
  >"$tmp_dir/native-accepted-report.json"

node -e '
  const fs = require("node:fs");
  const report = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  function fail(message) {
    console.error(message);
    process.exit(1);
  }
  if (report.ok !== true || report.goal_status?.status !== "accepted") {
    fail("accepted native/manual proof must make the decision report accepted");
  }
  if (report.native_preflight?.ok !== true || report.native_preflight?.product_media_ok !== true) {
    fail("accepted native/manual proof must expose accepted native preflight summary");
  }
  if (!Array.isArray(report.blocked_by) || report.blocked_by.length !== 0) {
    fail("accepted decision report must not keep unrelated live-host/provider blockers");
  }
  if (report.next_action?.id !== "keep_accepted_browser_artifacts") {
    fail("accepted decision report must route next action to preserving accepted artifacts");
  }
' "$tmp_dir/native-accepted-report.json"

node -e '
  const fs = require("node:fs");
  const report = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  const busy = report.live_adapter?.control_status?.single_session === true
    && Number(report.live_adapter?.control_status?.active_pages || 0) > 0;
  console.log(JSON.stringify({
    schema: "elastos.browser.provider-decision-report-smoke/v1",
    ok: true,
    structured_next_action: true,
    blocked_by_visible: true,
    candidate_readiness_visible: true,
    native_preflight_rejection_visible: true,
    native_preflight_acceptance_visible: true,
    busy_selkies_next_action_exercised: busy,
  }));
' "$tmp_dir/report.json"
