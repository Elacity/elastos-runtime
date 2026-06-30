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

tmp_dir="$(mktemp -d /tmp/elastos-browser-mac-vm-auth-profile-setup-smoke-XXXXXX)"
cleanup() {
  rm -rf "$tmp_dir"
}
trap cleanup EXIT

dry_run="$tmp_dir/dry-run.json"
receipt="$tmp_dir/setup-receipt.json"

"$repo_root/scripts/browser-mac-vm-auth-profile-setup.sh" \
  --dry-run \
  --auth-profile "$tmp_dir/profile" \
  --base-url http://localhost:61180 \
  --url https://ela.city/channels \
  --hold-ms 12345 \
  --receipt-out "$receipt" \
  >"$dry_run"

"$node_bin" - "$dry_run" "$tmp_dir/profile" "$receipt" <<'NODE'
const fs = require("node:fs");
const [dryRunPath, profilePath, receiptPath] = process.argv.slice(2);
const dryRun = JSON.parse(fs.readFileSync(dryRunPath, "utf8"));
const receipt = JSON.parse(fs.readFileSync(receiptPath, "utf8"));
if (dryRun.schema !== "elastos.browser.mac-vm-auth-profile-setup.dry-run/v1" || dryRun.ok !== true) {
  throw new Error("dry-run schema was not emitted");
}
if (!dryRun.command?.[1]?.endsWith("scripts/home-passkey-virtual-auth-smoke.mjs")) {
  throw new Error("setup must launch the virtual-auth smoke harness");
}
if (dryRun.receipt_out !== receiptPath) {
  throw new Error("dry-run did not report receipt path");
}
if (!dryRun.follow_up?.acceptance_handoff?.includes("--auth-setup-receipt")) {
  throw new Error("dry-run did not emit the authenticated handoff receipt flag");
}
if (receipt.schema !== "elastos.browser.mac-vm-auth-profile-setup.dry-run/v1" || receipt.ok !== true) {
  throw new Error("dry-run receipt schema was not emitted");
}
if (!receipt.follow_up?.acceptance_handoff?.includes(receiptPath)) {
  throw new Error("dry-run receipt did not include the handoff receipt path");
}
const resolvedProfilePath = fs.realpathSync(profilePath);
const env = dryRun.env || {};
const expected = {
  ELASTOS_BASE_URL: "http://localhost:61180",
  HOME_VIRTUAL_AUTH_PROFILE: resolvedProfilePath,
  HOME_VIRTUAL_AUTH_PRESERVE_PROFILE: "1",
  HOME_VIRTUAL_AUTH_CLEANUP: "0",
  HOME_VIRTUAL_AUTH_HEADED: "1",
  HOME_VIRTUAL_AUTH_BROWSER: "1",
  HOME_VIRTUAL_AUTH_BROWSER_UI_SETUP: "1",
  HOME_VIRTUAL_AUTH_BROWSER_OPEN_URLS: "https://ela.city/channels",
  HOME_VIRTUAL_AUTH_BROWSER_OPEN_HOLD_MS: "12345",
};
for (const [key, value] of Object.entries(expected)) {
  if (env[key] !== value) {
    throw new Error(`unexpected ${key}: ${env[key]}`);
  }
}
if (receipt.auth_profile?.path !== resolvedProfilePath) {
  throw new Error("receipt profile path does not match the resolved setup profile");
}
if (env.HOME_VIRTUAL_AUTH_BROWSER_OPEN === "1") {
  throw new Error("auth setup must use the visible Browser UI setup path, not hidden API Browser open");
}
if (
  receipt.setup?.open_url !== "https://ela.city/channels" ||
  receipt.setup?.hold_ms !== 12345 ||
  receipt.setup?.headed !== true ||
  receipt.setup?.preserve_profile !== true ||
  receipt.setup?.cleanup_passkey !== false ||
  receipt.setup?.authentication_claim !== "setup_only_not_authentication_proof" ||
  receipt.setup?.authentication_proof !== "deferred_to_machine_diagnostics_and_manual_ux"
) {
  throw new Error("receipt does not preserve the auth setup contract");
}
NODE

set +e
"$repo_root/scripts/browser-mac-vm-auth-profile-setup.sh" \
  --dry-run \
  --url file:///tmp/nope \
  >"$tmp_dir/bad-url.out" 2>"$tmp_dir/bad-url.err"
bad_url_status=$?
set -e

if [[ "$bad_url_status" -eq 0 ]]; then
  echo "auth profile setup accepted a non-http URL" >&2
  cat "$tmp_dir/bad-url.out" >&2
  exit 1
fi
if ! grep -q "must be an http(s) URL" "$tmp_dir/bad-url.err"; then
  echo "auth profile setup did not explain URL validation failure" >&2
  cat "$tmp_dir/bad-url.err" >&2
  exit 1
fi

set +e
"$repo_root/scripts/browser-mac-vm-auth-profile-setup.sh" \
  --dry-run \
  --hold-ms 300001 \
  >"$tmp_dir/bad-hold.out" 2>"$tmp_dir/bad-hold.err"
bad_hold_status=$?
set -e

if [[ "$bad_hold_status" -eq 0 ]]; then
  echo "auth profile setup accepted a hold duration above the virtual-auth harness bound" >&2
  cat "$tmp_dir/bad-hold.out" >&2
  exit 1
fi
if ! grep -q "between 1000 and 300000" "$tmp_dir/bad-hold.err"; then
  echo "auth profile setup did not explain hold validation failure" >&2
  cat "$tmp_dir/bad-hold.err" >&2
  exit 1
fi

printf '{"schema":"elastos.browser.mac-vm-auth-profile-setup-smoke/v1","ok":true,"dry_run_checked":true,"receipt_checked":true,"invalid_url_rejected":true,"invalid_hold_rejected":true}\n'
