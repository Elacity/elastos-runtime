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

tmp_dir="$(mktemp -d /tmp/elastos-remote-carrier-exit-public-live-plan-smoke-XXXXXX)"
cleanup() {
  rm -rf "$tmp_dir"
}
trap cleanup EXIT

valid_gateway="$tmp_dir/elastos-valid"
stale_gateway="$tmp_dir/elastos-stale"
mach_o_gateway="$tmp_dir/elastos-mach-o"
valid_exit_provider="$tmp_dir/exit-provider-valid"
stale_installed_readiness="$tmp_dir/stale-installed-readiness.json"
invalid_installed_readiness="$tmp_dir/invalid-installed-readiness.json"
valid_plan="$tmp_dir/valid-plan.json"
stale_plan="$tmp_dir/stale-plan.json"
mach_o_plan="$tmp_dir/mach-o-plan.json"

"$node_bin" - "$valid_gateway" "$stale_gateway" "$mach_o_gateway" "$valid_exit_provider" <<'NODE'
const fs = require("node:fs");
const [validGateway, staleGateway, machOGateway, validExitProvider] = process.argv.slice(2);
function linuxElf(strings) {
  const header = Buffer.alloc(64);
  header[0] = 0x7f;
  header.write("ELF", 1, "ascii");
  header[4] = 2;
  header[5] = 1;
  header.writeUInt16LE(0x3e, 18);
  return Buffer.concat([header, Buffer.from(`\n${strings.join("\n")}\n`, "utf8")]);
}
function machO(strings) {
  return Buffer.concat([
    Buffer.from([0xcf, 0xfa, 0xed, 0xfe]),
    Buffer.from(`\n${strings.join("\n")}\n`, "utf8"),
  ]);
}
const gatewayStrings = [
  "browser_exit_stream",
  "elastos.browser.carrier-stream/v1",
  "elastos://exit/open_stream",
];
const exitProviderStrings = [
  "remote_carrier_exits",
  "elastos.exit.remote-carrier.discovery/v1",
  "elastos.exit.remote-carrier.quote/v1",
  "elastos.exit.remote-carrier-session/v1",
  "elastos.exit.relay-ipc/v1",
  "max_active_streams_per_principal",
];
fs.writeFileSync(validGateway, linuxElf(gatewayStrings));
fs.writeFileSync(staleGateway, linuxElf(["elastos://exit/open_stream"]));
fs.writeFileSync(machOGateway, machO(gatewayStrings));
fs.writeFileSync(validExitProvider, linuxElf(exitProviderStrings));
NODE

"$node_bin" "$repo_root/scripts/remote-carrier-exit-artifact-readiness.mjs" \
  --gateway-bin "$stale_gateway" \
  --exit-provider-bin "$valid_exit_provider" \
  >"$stale_installed_readiness" || true

printf '{"schema":"not-artifact-readiness","ok":true}\n' >"$invalid_installed_readiness"

"$node_bin" "$repo_root/scripts/remote-carrier-exit-public-live-plan.mjs" \
  --candidate-gateway-bin "$valid_gateway" \
  --candidate-exit-provider-bin "$valid_exit_provider" \
  --installed-artifact-readiness "$stale_installed_readiness" \
  --commit "0123456789abcdef0123456789abcdef01234567" \
  --ssh-host "elastos-server" \
  --live-root "/srv/elastos-live" \
  --data-root "/srv/elastos-live/data" \
  >"$valid_plan"

"$node_bin" - "$valid_plan" "$valid_gateway" "$valid_exit_provider" <<'NODE'
const fs = require("node:fs");
const crypto = require("node:crypto");
const [planPath, gatewayPath, exitProviderPath] = process.argv.slice(2);
const plan = JSON.parse(fs.readFileSync(planPath, "utf8"));
const gatewaySha = crypto.createHash("sha256").update(fs.readFileSync(gatewayPath)).digest("hex");
const exitProviderSha = crypto.createHash("sha256").update(fs.readFileSync(exitProviderPath)).digest("hex");
if (plan.schema !== "elastos.remote-carrier-exit.public-live-update-plan/v1" ||
    plan.ok !== true ||
    plan.dry_run_only !== true ||
    plan.mutation_allowed !== false ||
    plan.private_material_redacted !== true) {
  throw new Error("valid public-live plan must be a dry-run-only accepted plan");
}
if (plan.candidate_readiness?.artifacts?.gateway?.sha256 !== gatewaySha ||
    plan.candidate_readiness?.artifacts?.exit_provider?.sha256 !== exitProviderSha) {
  throw new Error("public-live plan must hash-bind candidate artifacts");
}
if (plan.candidate_public_live_executables?.ok !== true ||
    plan.candidate_public_live_executables?.target?.os !== "linux" ||
    plan.candidate_public_live_executables?.target?.arch !== "x86_64" ||
    plan.candidate_public_live_executables?.artifacts?.gateway?.format !== "elf" ||
    plan.candidate_public_live_executables?.artifacts?.gateway?.arch !== "x86_64" ||
    plan.candidate_public_live_executables?.artifacts?.exit_provider?.format !== "elf" ||
    plan.candidate_public_live_executables?.artifacts?.exit_provider?.arch !== "x86_64") {
  throw new Error("public-live plan must require Linux x86_64 ELF candidate executables");
}
if (plan.installed_artifact_readiness?.ok !== false ||
    plan.stale_installed_artifacts_observed !== true) {
  throw new Error("public-live plan must surface stale installed artifact readiness");
}
for (const group of ["local_preflight", "public_live_backup", "operator_workstation_stage_candidates", "public_live_install_after_explicit_approval", "post_restart_verify", "rollback"]) {
  if (!Array.isArray(plan.commands?.[group]) || plan.commands[group].length === 0) {
    throw new Error(`public-live plan missing command group ${group}`);
  }
}
if (!plan.commands.local_preflight.some((command) => command.includes("carrier-only-authority-check.sh")) ||
    !plan.commands.public_live_backup.some((command) => command.includes("components.json")) ||
    !plan.commands.operator_workstation_stage_candidates.some((command) => command.includes("scp") && command.includes("candidates/elastos")) ||
    !plan.commands.public_live_install_after_explicit_approval.some((command) => command.includes("Restart the existing public-live gateway")) ||
    !plan.commands.post_restart_verify.some((command) => command.includes("remote-carrier-exit-artifact-readiness.mjs")) ||
    !plan.commands.rollback.some((command) => command.includes("backup"))) {
  throw new Error("public-live plan does not contain the expected review/install/verify/rollback commands");
}
if (plan.command_contexts?.local_preflight !== "operator_workstation_before_approval" ||
    plan.command_contexts?.operator_workstation_stage_candidates !== "operator_workstation_after_explicit_approval" ||
    plan.command_contexts?.public_live_backup !== "public_server_after_explicit_approval" ||
    plan.command_contexts?.public_live_install_after_explicit_approval !== "public_server_after_explicit_approval" ||
    plan.command_contexts?.post_restart_verify !== "public_server_after_explicit_approval" ||
    plan.command_contexts?.rollback !== "public_server_if_post_restart_verify_fails") {
  throw new Error("public-live plan must label operator-workstation and public-server command contexts");
}
if (!plan.install_targets?.candidate_staging_dir?.endsWith("/candidates") ||
    !plan.install_targets?.staged_gateway?.endsWith("/candidates/elastos") ||
    !plan.install_targets?.staged_exit_provider?.endsWith("/candidates/exit-provider")) {
  throw new Error("public-live plan must name server-side candidate staging paths");
}
if (plan.commands.public_live_install_after_explicit_approval.some((command) =>
  command.includes(gatewayPath) || command.includes(exitProviderPath)
)) {
  throw new Error("public-live server install commands must use staged server artifacts, not local workstation paths");
}
const text = JSON.stringify(plan);
if (/connect_ticket|adapter_ipc|runtime_stream_path|home_token|authorization|cookie|set-cookie/i.test(text)) {
  throw new Error("public-live plan leaked private route material");
}
NODE

set +e
"$node_bin" "$repo_root/scripts/remote-carrier-exit-public-live-plan.mjs" \
  --candidate-gateway-bin "$stale_gateway" \
  --candidate-exit-provider-bin "$valid_exit_provider" \
  --commit "0123456789abcdef0123456789abcdef01234567" \
  --ssh-host "elastos-server" \
  --live-root "/srv/elastos-live" \
  --data-root "/srv/elastos-live/data" \
  >"$stale_plan"
stale_status=$?
set -e

if [[ "$stale_status" -eq 0 ]]; then
  echo "public-live plan accepted a stale candidate gateway" >&2
  cat "$stale_plan" >&2
  exit 1
fi

"$node_bin" - "$stale_plan" <<'NODE'
const fs = require("node:fs");
const plan = JSON.parse(fs.readFileSync(process.argv[2], "utf8"));
if (plan.ok !== false ||
    !plan.candidate_readiness?.failures?.includes("gateway_missing_browser_exit_stream") ||
    plan.mutation_allowed !== false ||
    !plan.next_steps?.some((step) => step.includes("Linux x86_64 ELF"))) {
  throw new Error("stale candidate plan must fail closed with rebuild guidance");
}
NODE

set +e
"$node_bin" "$repo_root/scripts/remote-carrier-exit-public-live-plan.mjs" \
  --candidate-gateway-bin "$mach_o_gateway" \
  --candidate-exit-provider-bin "$valid_exit_provider" \
  --commit "0123456789abcdef0123456789abcdef01234567" \
  --ssh-host "elastos-server" \
  --live-root "/srv/elastos-live" \
  --data-root "/srv/elastos-live/data" \
  >"$mach_o_plan"
mach_o_status=$?
set -e

if [[ "$mach_o_status" -eq 0 ]]; then
  echo "public-live plan accepted a non-Linux Mach-O candidate gateway" >&2
  cat "$mach_o_plan" >&2
  exit 1
fi

"$node_bin" - "$mach_o_plan" <<'NODE'
const fs = require("node:fs");
const plan = JSON.parse(fs.readFileSync(process.argv[2], "utf8"));
if (plan.ok !== false ||
    plan.candidate_readiness?.ok !== true ||
    plan.candidate_public_live_executables?.ok !== false ||
    !plan.candidate_public_live_executables?.failures?.includes("gateway_not_linux_x86_64_elf") ||
    plan.candidate_public_live_executables?.artifacts?.gateway?.format !== "mach-o" ||
    plan.mutation_allowed !== false ||
    !plan.next_steps?.some((step) => step.includes("Linux x86_64 ELF"))) {
  throw new Error("Mach-O candidate plan must fail closed with Linux ELF rebuild guidance");
}
NODE

set +e
"$node_bin" "$repo_root/scripts/remote-carrier-exit-public-live-plan.mjs" \
  --candidate-gateway-bin "$valid_gateway" \
  --candidate-exit-provider-bin "$valid_exit_provider" \
  --installed-artifact-readiness "$invalid_installed_readiness" \
  --commit "0123456789abcdef0123456789abcdef01234567" \
  --ssh-host "elastos-server" \
  --live-root "/srv/elastos-live" \
  --data-root "/srv/elastos-live/data" \
  >"$tmp_dir/invalid-installed-readiness-plan.json" \
  2>"$tmp_dir/invalid-installed-readiness-plan.err"
invalid_installed_status=$?
set -e

if [[ "$invalid_installed_status" -eq 0 ]] ||
   ! grep -q "must point to an elastos.remote-carrier-exit.artifact-readiness/v1 report" "$tmp_dir/invalid-installed-readiness-plan.err"; then
  echo "public-live plan accepted invalid installed artifact readiness input" >&2
  cat "$tmp_dir/invalid-installed-readiness-plan.json" >&2 || true
  cat "$tmp_dir/invalid-installed-readiness-plan.err" >&2 || true
  exit 1
fi

printf '{"schema":"elastos.remote-carrier-exit.public-live-plan-smoke/v1","ok":true,"valid_plan_accepted":true,"stale_candidate_rejected":true,"mach_o_candidate_rejected":true,"linux_x86_64_elf_required":true,"stale_installed_readiness_visible":true,"invalid_installed_readiness_rejected":true,"server_candidate_staging_required":true,"command_contexts_required":true,"dry_run_only":true}\n'
