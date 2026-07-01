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

usage() {
  cat >&2 <<'USAGE'
Usage:
  scripts/browser-mac-vm-acceptance-handoff.sh [options]

Options:
  --machine-proof <path>  Use an existing elastos.browser.mac-vm-proof/v1 artifact.
  --proof-out <path>      Collect a fresh Mac VM proof at this path when --machine-proof is omitted.
  --auth-profile <path>   Collect fresh proof with a persistent virtual-auth profile.
  --auth-setup-receipt <path>
                         Receipt from browser-mac-vm-auth-profile-setup.sh.
  --source-home-restart-receipt <path>
                         Receipt from mac-source-home-restart.sh. Auto-written when
                         --restart-source-home collects a fresh proof.
  --restart-source-home   Restart local Mac source-home gateway before collecting fresh proof.
  --manual-out <path>     Manual UX template output path.
  --audit-out <path>      Acceptance audit output path.
  --summary-out <path>    Handoff summary output path.

When --machine-proof is omitted, this script runs scripts/browser-mac-vm-proof.sh
first. It always writes a hash-bound manual UX template and an expected failing
acceptance audit so the operator can see the exact remaining human/authenticated
evidence before editing the manual report.

Fresh proofs default to the current ela.city acceptance route:
  ELASTOS_BROWSER_MAC_VM_PROOF_URL=https://ela.city/channels
  ELASTOS_BROWSER_MAC_VM_CLICK_HREF_RE=/explore$
  ELASTOS_BROWSER_MAC_VM_CLICK_EXPECT_URL_RE=https://ela[.]city/explore

For authenticated edit-profile proof, pass --auth-profile and sign into ela.city
inside that persistent Browser VM profile before collecting the final proof.
USAGE
}

machine_proof=""
proof_out="/tmp/elastos-browser-mac-vm-proof-handoff.json"
auth_profile=""
auth_setup_receipt=""
restart_source_home=0
manual_out=""
audit_out=""
summary_out=""
source_home_restart_receipt=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --machine-proof)
      machine_proof="${2:-}"
      if [[ -z "$machine_proof" ]]; then
        echo "--machine-proof requires a path" >&2
        exit 2
      fi
      shift 2
      ;;
    --proof-out)
      proof_out="${2:-}"
      if [[ -z "$proof_out" ]]; then
        echo "--proof-out requires a path" >&2
        exit 2
      fi
      shift 2
      ;;
    --auth-profile)
      auth_profile="${2:-}"
      if [[ -z "$auth_profile" ]]; then
        echo "--auth-profile requires a path" >&2
        exit 2
      fi
      shift 2
      ;;
    --auth-setup-receipt)
      auth_setup_receipt="${2:-}"
      if [[ -z "$auth_setup_receipt" ]]; then
        echo "--auth-setup-receipt requires a path" >&2
        exit 2
      fi
      shift 2
      ;;
    --source-home-restart-receipt)
      source_home_restart_receipt="${2:-}"
      if [[ -z "$source_home_restart_receipt" ]]; then
        echo "--source-home-restart-receipt requires a path" >&2
        exit 2
      fi
      shift 2
      ;;
    --restart-source-home)
      restart_source_home=1
      shift
      ;;
    --manual-out)
      manual_out="${2:-}"
      if [[ -z "$manual_out" ]]; then
        echo "--manual-out requires a path" >&2
        exit 2
      fi
      shift 2
      ;;
    --audit-out)
      audit_out="${2:-}"
      if [[ -z "$audit_out" ]]; then
        echo "--audit-out requires a path" >&2
        exit 2
      fi
      shift 2
      ;;
    --summary-out)
      summary_out="${2:-}"
      if [[ -z "$summary_out" ]]; then
        echo "--summary-out requires a path" >&2
        exit 2
      fi
      shift 2
      ;;
    --help|-h)
      usage
      exit 0
      ;;
    *)
      echo "unknown argument: $1" >&2
      usage
      exit 2
      ;;
  esac
done

node_bin="$(find_node || true)"
if [[ -z "$node_bin" ]]; then
  echo "node not found. Install Node or set ELASTOS_NODE_BIN to an executable node binary." >&2
  exit 2
fi

if [[ -n "$auth_setup_receipt" && ! -f "$auth_setup_receipt" ]]; then
  echo "auth setup receipt not found: $auth_setup_receipt" >&2
  exit 2
fi
if [[ -n "$source_home_restart_receipt" && ! -f "$source_home_restart_receipt" ]]; then
  echo "source-home restart receipt not found: $source_home_restart_receipt" >&2
  exit 2
fi
if [[ -n "$auth_profile" && -d "$auth_profile" ]]; then
  auth_profile="$(cd "$auth_profile" && pwd -P)"
fi

if [[ -z "$machine_proof" ]]; then
  machine_proof="$proof_out"
  if [[ "$restart_source_home" -eq 1 ]]; then
    source_home_restart_receipt="${source_home_restart_receipt:-${proof_out%.json}-source-home-restart.json}"
    if [[ "$source_home_restart_receipt" == "$proof_out" ]]; then
      source_home_restart_receipt="${proof_out}-source-home-restart.json"
    fi
    "$repo_root/scripts/mac-source-home-restart.sh" \
      --json-out "$source_home_restart_receipt" \
      >/dev/null
  fi
  proof_env=(
    "ELASTOS_BROWSER_MAC_VM_PROOF_URL=${ELASTOS_BROWSER_MAC_VM_PROOF_URL:-https://ela.city/channels}"
    "ELASTOS_BROWSER_MAC_VM_CLICK_HREF_RE=${ELASTOS_BROWSER_MAC_VM_CLICK_HREF_RE:-/explore$}"
    "ELASTOS_BROWSER_MAC_VM_CLICK_EXPECT_URL_RE=${ELASTOS_BROWSER_MAC_VM_CLICK_EXPECT_URL_RE:-https://ela[.]city/explore}"
    "HOME_VIRTUAL_AUTH_BROWSER_DIAGNOSTIC_CLICK_TEXT_RE=${HOME_VIRTUAL_AUTH_BROWSER_DIAGNOSTIC_CLICK_TEXT_RE:-Edit Profile}"
    "HOME_VIRTUAL_AUTH_BROWSER_DIAGNOSTIC_CLICK_EXPECT_TEXT_RE=${HOME_VIRTUAL_AUTH_BROWSER_DIAGNOSTIC_CLICK_EXPECT_TEXT_RE:-Edit Profile|Profile|Account Settings}"
    "ELASTOS_BROWSER_MAC_VM_PROFILE_RESET_PROOF=${ELASTOS_BROWSER_MAC_VM_PROFILE_RESET_PROOF:-1}"
  )
  if [[ -n "$auth_profile" ]]; then
    env "${proof_env[@]}" \
      ELASTOS_BROWSER_MAC_VM_PROOF_AUTH_PROFILE="$auth_profile" \
      "$repo_root/scripts/browser-mac-vm-proof.sh" --artifact-out "$machine_proof" >/dev/null
  else
    env "${proof_env[@]}" \
      "$repo_root/scripts/browser-mac-vm-proof.sh" --artifact-out "$machine_proof" >/dev/null
  fi
fi

if [[ ! -f "$machine_proof" ]]; then
  echo "machine proof not found: $machine_proof" >&2
  exit 2
fi

base="${machine_proof%.json}"
if [[ "$base" == "$machine_proof" ]]; then
  base="$machine_proof"
fi
manual_out="${manual_out:-${base}-manual-ux.json}"
audit_out="${audit_out:-${base}-acceptance-audit.json}"
summary_out="${summary_out:-${base}-handoff-summary.json}"
audit_err="${audit_out}.err"

"$node_bin" "$repo_root/scripts/browser-manual-ux-report.mjs" \
  --template \
  --machine-artifact "$machine_proof" \
  >"$manual_out"

set +e
"$node_bin" "$repo_root/scripts/browser-mac-vm-acceptance-audit.mjs" \
  --machine-proof "$machine_proof" \
  >"$audit_out" \
  2>"$audit_err"
audit_status=$?
set -e

"$node_bin" - "$machine_proof" "$manual_out" "$audit_out" "$summary_out" "$audit_status" "$auth_profile" "$auth_setup_receipt" "$source_home_restart_receipt" <<'NODE'
const fs = require("node:fs");
const crypto = require("node:crypto");
const path = require("node:path");

const [
  proofPath,
  manualPath,
  auditPath,
  summaryPath,
  auditStatusRaw,
  authProfile,
  authSetupReceiptPath,
  sourceHomeRestartReceiptPath,
] = process.argv.slice(2);
const proof = JSON.parse(fs.readFileSync(proofPath, "utf8"));
const manual = JSON.parse(fs.readFileSync(manualPath, "utf8"));
let audit = null;
try {
  audit = JSON.parse(fs.readFileSync(auditPath, "utf8"));
} catch {
  audit = null;
}
const sha256 = crypto.createHash("sha256").update(fs.readFileSync(proofPath)).digest("hex");
const manualSha256 = crypto.createHash("sha256").update(fs.readFileSync(manualPath)).digest("hex");
const auditSha256 = fs.existsSync(auditPath)
  ? crypto.createHash("sha256").update(fs.readFileSync(auditPath)).digest("hex")
  : null;
const criteria = Array.isArray(audit?.criteria) ? audit.criteria : [];
const failing = criteria.filter((item) => item.ok !== true).map((item) => item.id);
const passing = criteria.filter((item) => item.ok === true).map((item) => item.id);
const machineCriteria = [
  "machine_proof_ok",
  "runtime_only_network",
  "vm_control_restart_proof",
  "remote_video_input",
  "browser_vm_isolation",
  "runtime_media_relay",
  "performance_zoom",
  "ela_city_url_sync",
  "ela_city_images",
  "sanitized_diagnostics",
  "profile_reset_safety",
];
const authCriteria = [
  "ela_city_auth_profile_persistence",
  "auth_setup_receipt_chain",
  "ela_city_authenticated_surface",
  "ela_city_edit_profile_modal",
];
const manualCriteria = [
  "manual_ux_hash_bound",
  "manual_video_input_performance",
  "ela_city_edit_profile_modal",
  "authority_and_cleanup",
];
const machineFailing = machineCriteria.filter((id) => !criteria.some((item) => item.id === id && item.ok === true));
const machineReady =
  proof.schema === "elastos.browser.mac-vm-proof/v1" &&
  proof.ok === true &&
  machineFailing.length === 0;
const persistentProfile = proof.virtual_auth?.persistent_profile === true;
const authProfileHint = authProfile || "$HOME/.local/share/elastos/mac-browser-vm-proof-auth";
let authSetupReceipt = null;
let authSetupReceiptError = null;
let authSetupReceiptSha256 = null;
let sourceHomeRestartReceipt = null;
let sourceHomeRestartError = null;
let sourceHomeRestartSha256 = null;
if (authSetupReceiptPath) {
  try {
    const receiptText = fs.readFileSync(authSetupReceiptPath, "utf8");
    authSetupReceipt = JSON.parse(receiptText);
    authSetupReceiptSha256 = crypto.createHash("sha256").update(receiptText).digest("hex");
  } catch (error) {
    authSetupReceiptError = error instanceof Error ? error.message : String(error);
  }
}
if (sourceHomeRestartReceiptPath) {
  try {
    const receiptText = fs.readFileSync(sourceHomeRestartReceiptPath, "utf8");
    sourceHomeRestartReceipt = JSON.parse(receiptText);
    sourceHomeRestartSha256 = crypto.createHash("sha256").update(receiptText).digest("hex");
  } catch (error) {
    sourceHomeRestartError = error instanceof Error ? error.message : String(error);
  }
}
const helperHashes = [
  sourceHomeRestartReceipt?.browser_helper_source_sha256,
  sourceHomeRestartReceipt?.browser_helper_installed_sha256,
  sourceHomeRestartReceipt?.browser_helper_initrd_sha256,
  sourceHomeRestartReceipt?.browser_helper_rootfs_sha256,
].filter(Boolean);
const helperHashesOk =
  helperHashes.length === 4 &&
  helperHashes.every((value) => /^[a-f0-9]{64}$/i.test(String(value))) &&
  new Set(helperHashes.map((value) => String(value).toLowerCase())).size === 1;
const homeHashesOk =
  sourceHomeRestartReceipt?.served_index_sha256 === sourceHomeRestartReceipt?.installed_index_sha256 &&
  sourceHomeRestartReceipt?.served_index_sha256 === sourceHomeRestartReceipt?.source_index_sha256 &&
  sourceHomeRestartReceipt?.installed_index_sha256 === proof.home?.installed_index_sha256 &&
  sourceHomeRestartReceipt?.source_index_sha256 === proof.home?.source_index_sha256;
const restartGeneratedAt = Date.parse(sourceHomeRestartReceipt?.generated_at || "");
const proofGeneratedAtMs = Date.parse(proof.generated_at || "");
const sourceHomeRestartReady =
  Boolean(sourceHomeRestartReceiptPath) &&
  sourceHomeRestartReceipt?.schema === "elastos.mac-source-home-restart/v1" &&
  sourceHomeRestartReceipt.ok === true &&
  sourceHomeRestartReceipt.dry_run === false &&
  sourceHomeRestartReceipt.http_code === 200 &&
  homeHashesOk &&
  helperHashesOk &&
  Number.isFinite(restartGeneratedAt) &&
  (!Number.isFinite(proofGeneratedAtMs) || restartGeneratedAt <= proofGeneratedAtMs) &&
  !sourceHomeRestartError;
const passingWithHandoff = sourceHomeRestartReady && !passing.includes("source_home_restart_freshness")
  ? [...passing, "source_home_restart_freshness"]
  : passing;
const failingWithHandoff = sourceHomeRestartReady
  ? failing.filter((id) => id !== "source_home_restart_freshness")
  : failing;
const authFailing = authCriteria.filter((id) => failingWithHandoff.includes(id));
const manualFailing = manualCriteria.filter((id) => failingWithHandoff.includes(id));
const authSetupReceiptProfile = authSetupReceipt?.auth_profile?.path || "";
const authSetupReceiptOk =
  Boolean(authSetupReceiptPath) &&
  (
    authSetupReceipt?.schema === "elastos.browser.mac-vm-auth-profile-setup/v1" &&
    authSetupReceipt.ok === true &&
    authSetupReceipt.setup?.open_url &&
    authSetupReceipt.setup?.headed === true &&
    authSetupReceipt.setup?.preserve_profile === true &&
    authSetupReceipt.setup?.cleanup_passkey === false &&
    authProfile &&
    path.resolve(authSetupReceiptProfile) === path.resolve(authProfile)
  );
const authSetupReady = authSetupReceiptOk && persistentProfile;
const restartThresholdMs = Number(proof.vm_control?.restart?.max_uptime_ms || 300000);
const restartProofStep = machineFailing.includes("vm_control_restart_proof")
  ? `Run scripts/browser-mac-vm-acceptance-handoff.sh --restart-source-home within the same Mac source-home checkout, or run scripts/mac-source-home-restart.sh and rerun this handoff within ${restartThresholdMs} ms so vm_control.restart.fresh_after_restart is true.`
  : `Fresh VM control-service restart proof is recorded with max_uptime_ms=${restartThresholdMs}.`;
const authSetupReceiptHint = authSetupReceiptPath || "/tmp/elastos-browser-mac-vm-auth-setup.json";
const authenticatedSetupStep = persistentProfile
  ? `If ela.city still looks logged out, run scripts/browser-mac-vm-auth-profile-setup.sh --auth-profile ${authProfileHint} --receipt-out ${authSetupReceiptHint}, sign into ela.city inside the Browser VM, then collect a fresh proof with the same profile and receipt.`
  : `For authenticated ela.city proof, first run scripts/browser-mac-vm-auth-profile-setup.sh --auth-profile ${authProfileHint} --receipt-out ${authSetupReceiptHint}, sign into ela.city inside that persistent Browser VM profile, then rerun this handoff with the same profile and receipt.`;
const authenticatedHandoffStep =
  `Collect the authenticated proof with scripts/browser-mac-vm-acceptance-handoff.sh --restart-source-home --auth-profile ${authProfileHint} --auth-setup-receipt ${authSetupReceiptHint} --summary-out <handoff-summary.json>.`;
const editProfileProofStep =
  "The fresh-proof path now sets the ela.city /channels -> /explore URL-sync proof and an Edit Profile diagnostic-click probe by default; override HOME_VIRTUAL_AUTH_BROWSER_DIAGNOSTIC_CLICK_TEXT_RE only if ela.city changes its visible label.";
const reviewPacketDir = proofPath.endsWith(".json")
  ? `${proofPath.slice(0, -".json".length)}-manual-review`
  : `${proofPath}-manual-review`;
const acceptanceReady = audit?.ok === true;
const handoffReady = machineReady && authSetupReady && sourceHomeRestartReady;
const summary = {
  schema: "elastos.browser.mac-vm-acceptance-handoff/v1",
  ok: handoffReady,
  handoff_ready: handoffReady,
  machine_ready: machineReady,
  acceptance_ready: acceptanceReady,
  generated_at: new Date().toISOString(),
  machine_proof: {
    path: proofPath,
    schema: proof.schema || null,
    ok: proof.ok === true,
    machine_ready: machineReady,
    sha256,
    generated_at: proof.generated_at || null,
  },
  manual_template: {
    path: manualPath,
    schema: manual.schema || null,
    provider: manual.provider || null,
    target: manual.target || null,
    ok: manual.ok === true,
    sha256: manualSha256,
    check_count: manual.checks ? Object.keys(manual.checks).length : 0,
  },
  acceptance_audit: {
    path: auditPath,
    exit_status: Number(auditStatusRaw),
    ok: audit?.ok === true,
    sha256: auditSha256,
    criteria_count: criteria.length,
    machine_ready: machineReady,
    machine_failing: machineFailing,
    auth_failing: authFailing,
    manual_failing: manualFailing,
    passing: passingWithHandoff,
    failing: failingWithHandoff,
    ela_city: audit?.ela_city_diagnostics || null,
  },
  source_home_restart: sourceHomeRestartReceiptPath ? {
    path: sourceHomeRestartReceiptPath,
    sha256: sourceHomeRestartSha256,
    ok: sourceHomeRestartReady,
    schema: sourceHomeRestartReceipt?.schema || null,
    error: sourceHomeRestartError,
    generated_at: sourceHomeRestartReceipt?.generated_at || null,
    home_url: sourceHomeRestartReceipt?.home_url || null,
    http_code: sourceHomeRestartReceipt?.http_code ?? null,
    served_index_sha256: sourceHomeRestartReceipt?.served_index_sha256 || null,
    installed_index_sha256: sourceHomeRestartReceipt?.installed_index_sha256 || null,
    source_index_sha256: sourceHomeRestartReceipt?.source_index_sha256 || null,
    browser_helper_source_sha256: sourceHomeRestartReceipt?.browser_helper_source_sha256 || null,
    browser_helper_installed_sha256: sourceHomeRestartReceipt?.browser_helper_installed_sha256 || null,
    browser_helper_initrd_sha256: sourceHomeRestartReceipt?.browser_helper_initrd_sha256 || null,
    browser_helper_rootfs_sha256: sourceHomeRestartReceipt?.browser_helper_rootfs_sha256 || null,
  } : null,
  authenticated_profile: {
    persistent_virtual_auth_profile: persistentProfile,
    cleanup_passkey: proof.virtual_auth?.cleanup_passkey ?? null,
    auth_profile_requested: Boolean(authProfile),
    auth_setup_receipt: authSetupReceiptPath ? {
      path: authSetupReceiptPath,
      sha256: authSetupReceiptSha256,
      ok: authSetupReceiptOk,
      proof_used_persistent_profile: persistentProfile,
      schema: authSetupReceipt?.schema || null,
      error: authSetupReceiptError,
      open_url: authSetupReceipt?.setup?.open_url || null,
      profile_matches_auth_profile: Boolean(
        authProfile &&
        authSetupReceiptProfile &&
        path.resolve(authSetupReceiptProfile) === path.resolve(authProfile),
      ),
    } : null,
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
  passing_criteria: passingWithHandoff,
  failing_criteria: failingWithHandoff,
  machine_failing: machineFailing,
  auth_failing: authFailing,
  manual_failing: manualFailing,
  remaining_acceptance_gaps: acceptanceReady ? [] : failingWithHandoff,
  next_steps: [
    restartProofStep,
    authenticatedSetupStep,
    authenticatedHandoffStep,
    editProfileProofStep,
    `Review the exact machine proof: ${proofPath}`,
    `Generate a manual review packet: node scripts/browser-mac-vm-manual-review-packet.mjs --machine-proof ${proofPath} --handoff-summary ${summaryPath} --out-dir ${reviewPacketDir}`,
    `Edit ${manualPath} only after real Mac Browser VM review; set ok=true and fill every evidence field.`,
    `Validate the manual report: node scripts/browser-manual-ux-report.mjs --input ${manualPath}`,
    `Run final audit: node scripts/browser-mac-vm-acceptance-audit.mjs --machine-proof ${proofPath} --manual-ux ${manualPath} --handoff-summary ${summaryPath}`,
  ],
};
fs.writeFileSync(summaryPath, `${JSON.stringify(summary, null, 2)}\n`);
if (!authSetupReady || !sourceHomeRestartReady) {
  console.log(JSON.stringify(summary, null, 2));
  process.exitCode = 1;
}
NODE

set +e
"$node_bin" "$repo_root/scripts/browser-mac-vm-acceptance-audit.mjs" \
  --machine-proof "$machine_proof" \
  --handoff-summary "$summary_out" \
  >"$audit_out" \
  2>"$audit_err"
audit_status=$?
set -e

"$node_bin" - "$summary_out" "$audit_out" "$audit_status" <<'NODE'
const fs = require("node:fs");
const crypto = require("node:crypto");

const [summaryPath, auditPath, auditStatusRaw] = process.argv.slice(2);
const summary = JSON.parse(fs.readFileSync(summaryPath, "utf8"));
const audit = JSON.parse(fs.readFileSync(auditPath, "utf8"));
const auditSha256 = crypto.createHash("sha256").update(fs.readFileSync(auditPath)).digest("hex");
const criteria = Array.isArray(audit.criteria) ? audit.criteria : [];
const failing = criteria.filter((item) => item.ok !== true).map((item) => item.id);
const passing = criteria.filter((item) => item.ok === true).map((item) => item.id);
const machineCriteria = [
  "machine_proof_ok",
  "runtime_only_network",
  "vm_control_restart_proof",
  "remote_video_input",
  "browser_vm_isolation",
  "runtime_media_relay",
  "performance_zoom",
  "ela_city_url_sync",
  "ela_city_images",
  "sanitized_diagnostics",
  "profile_reset_safety",
  "source_home_restart_freshness",
];
const authCriteria = [
  "ela_city_auth_profile_persistence",
  "auth_setup_receipt_chain",
  "ela_city_authenticated_surface",
  "ela_city_edit_profile_modal",
];
const manualCriteria = [
  "manual_ux_hash_bound",
  "manual_video_input_performance",
  "ela_city_edit_profile_modal",
  "authority_and_cleanup",
];
const machineFailing = machineCriteria.filter((id) => !criteria.some((item) => item.id === id && item.ok === true));
summary.acceptance_ready = audit.ok === true;
summary.acceptance_audit = {
  path: auditPath,
  exit_status: Number(auditStatusRaw),
  ok: audit.ok === true,
  sha256: auditSha256,
  criteria_count: criteria.length,
  machine_ready: summary.machine_ready === true && machineFailing.length === 0,
  machine_failing: machineFailing,
  auth_failing: authCriteria.filter((id) => failing.includes(id)),
  manual_failing: manualCriteria.filter((id) => failing.includes(id)),
  passing,
  failing,
  ela_city: audit.ela_city_diagnostics || null,
};
summary.passing_criteria = passing;
summary.failing_criteria = failing;
summary.machine_failing = summary.acceptance_audit.machine_failing;
summary.auth_failing = summary.acceptance_audit.auth_failing;
summary.manual_failing = summary.acceptance_audit.manual_failing;
summary.remaining_acceptance_gaps = audit.ok === true ? [] : failing;
fs.writeFileSync(summaryPath, `${JSON.stringify(summary, null, 2)}\n`);
console.log(JSON.stringify(summary, null, 2));
NODE

if [[ "$audit_status" -eq 0 ]]; then
  echo "warning: Mac VM acceptance audit passed without manual UX; inspect $audit_out" >&2
fi
