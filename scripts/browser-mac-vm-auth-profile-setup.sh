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
  scripts/browser-mac-vm-auth-profile-setup.sh [options]

Options:
  --auth-profile <path>  Persistent virtual-auth profile directory.
                         Default: $HOME/.local/share/elastos/mac-browser-vm-proof-auth
  --base-url <url>       Local Home base URL. Default: $ELASTOS_BASE_URL or http://localhost:61180
  --url <url>            Browser VM URL to open for login setup. Default: https://ela.city/channels
  --hold-ms <ms>         How long to keep the headed setup open. Default: 300000
  --receipt-out <path>   Write an operator receipt after successful setup.
  --dry-run              Print the exact command/env JSON without launching Chromium.

This opens a headed virtual-auth Home session, preserves the same passkey
profile, launches the Browser VM, and keeps ela.city open long enough for an
operator to sign in. After it closes cleanly, collect acceptance proof with:

  scripts/browser-mac-vm-acceptance-handoff.sh \
    --restart-source-home \
    --auth-profile <same path> \
    --auth-setup-receipt <receipt path>
USAGE
}

auth_profile="${ELASTOS_BROWSER_MAC_VM_PROOF_AUTH_PROFILE:-${HOME}/.local/share/elastos/mac-browser-vm-proof-auth}"
base_url="${ELASTOS_BASE_URL:-http://localhost:61180}"
open_url="${ELASTOS_BROWSER_MAC_VM_PROOF_URL:-https://ela.city/channels}"
hold_ms="${ELASTOS_BROWSER_MAC_VM_AUTH_SETUP_HOLD_MS:-300000}"
receipt_out=""
dry_run=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --auth-profile)
      auth_profile="${2:-}"
      if [[ -z "$auth_profile" ]]; then
        echo "--auth-profile requires a path" >&2
        exit 2
      fi
      shift 2
      ;;
    --base-url)
      base_url="${2:-}"
      if [[ -z "$base_url" ]]; then
        echo "--base-url requires a URL" >&2
        exit 2
      fi
      shift 2
      ;;
    --url)
      open_url="${2:-}"
      if [[ -z "$open_url" ]]; then
        echo "--url requires a URL" >&2
        exit 2
      fi
      shift 2
      ;;
    --hold-ms)
      hold_ms="${2:-}"
      if [[ -z "$hold_ms" ]]; then
        echo "--hold-ms requires a value" >&2
        exit 2
      fi
      shift 2
      ;;
    --receipt-out)
      receipt_out="${2:-}"
      if [[ -z "$receipt_out" ]]; then
        echo "--receipt-out requires a path" >&2
        exit 2
      fi
      shift 2
      ;;
    --dry-run)
      dry_run=1
      shift
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

"$node_bin" - "$base_url" "$open_url" "$hold_ms" <<'NODE'
const [baseUrl, openUrl, holdMsRaw] = process.argv.slice(2);
function requireHttpUrl(value, name) {
  let parsed;
  try {
    parsed = new URL(value);
  } catch {
    throw new Error(`${name} must be an http(s) URL`);
  }
  if (!["http:", "https:"].includes(parsed.protocol)) {
    throw new Error(`${name} must be an http(s) URL`);
  }
}
requireHttpUrl(baseUrl, "--base-url");
requireHttpUrl(openUrl, "--url");
const holdMs = Number(holdMsRaw);
if (!Number.isInteger(holdMs) || holdMs < 1000 || holdMs > 300000) {
  throw new Error("--hold-ms must be an integer between 1000 and 300000");
}
NODE

mkdir -p "$auth_profile"
auth_profile="$(cd "$auth_profile" && pwd -P)"

setup_env=(
  "ELASTOS_BASE_URL=${base_url%/}"
  "HOME_VIRTUAL_AUTH_PROFILE=$auth_profile"
  "HOME_VIRTUAL_AUTH_PRESERVE_PROFILE=1"
  "HOME_VIRTUAL_AUTH_CLEANUP=0"
  "HOME_VIRTUAL_AUTH_HEADED=1"
  "HOME_VIRTUAL_AUTH_NAME=${HOME_VIRTUAL_AUTH_NAME:-Mac Browser VM Auth Setup}"
  "HOME_VIRTUAL_AUTH_BROWSER=1"
  "HOME_VIRTUAL_AUTH_BROWSER_UI_SETUP=1"
  "HOME_VIRTUAL_AUTH_BROWSER_SUMMARY=1"
  "HOME_VIRTUAL_AUTH_BROWSER_OPEN_URLS=$open_url"
  "HOME_VIRTUAL_AUTH_BROWSER_OPEN_HOLD_MS=$hold_ms"
  "HOME_VIRTUAL_AUTH_BROWSER_REMOTE_VIDEO_TIMEOUT_MS=${HOME_VIRTUAL_AUTH_BROWSER_REMOTE_VIDEO_TIMEOUT_MS:-180000}"
)

write_receipt() {
  local schema="$1"
  local ok="$2"
  local output_path="$3"
  if [[ -z "$output_path" ]]; then
    return 0
  fi
  mkdir -p "$(dirname "$output_path")"
  "$node_bin" - "$schema" "$ok" "$base_url" "$open_url" "$hold_ms" "$auth_profile" "$output_path" "$repo_root/scripts/browser-mac-vm-acceptance-handoff.sh" <<'NODE'
const fs = require("node:fs");
const [
  schema,
  okRaw,
  baseUrl,
  openUrl,
  holdMsRaw,
  authProfile,
  receiptOut,
  handoffScript,
] = process.argv.slice(2);
const holdMs = Number(holdMsRaw);
const followUp = [
  handoffScript,
  "--restart-source-home",
  "--auth-profile",
  authProfile,
  "--auth-setup-receipt",
  receiptOut,
];
const receipt = {
  schema,
  ok: okRaw === "true",
  generated_at: new Date().toISOString(),
  auth_profile: {
    path: authProfile,
    persistent_virtual_auth_profile: true,
  },
  setup: {
    base_url: baseUrl.replace(/\/$/, ""),
    open_url: openUrl,
    hold_ms: holdMs,
    headed: true,
    preserve_profile: true,
    cleanup_passkey: false,
    authentication_claim: "setup_only_not_authentication_proof",
    authentication_proof: "deferred_to_machine_diagnostics_and_manual_ux",
  },
  follow_up: {
    acceptance_handoff: followUp,
  },
};
fs.writeFileSync(receiptOut, `${JSON.stringify(receipt, null, 2)}\n`);
NODE
}

if [[ "$dry_run" -eq 1 ]]; then
  write_receipt "elastos.browser.mac-vm-auth-profile-setup.dry-run/v1" true "$receipt_out"
  "$node_bin" - "$node_bin" "$repo_root/scripts/home-passkey-virtual-auth-smoke.mjs" "$receipt_out" "$auth_profile" "${setup_env[@]}" <<'NODE'
const [nodeBin, script, receiptOut, authProfile, ...envPairs] = process.argv.slice(2);
const env = Object.fromEntries(envPairs.map((entry) => {
  const index = entry.indexOf("=");
  return [entry.slice(0, index), entry.slice(index + 1)];
}));
console.log(JSON.stringify({
  schema: "elastos.browser.mac-vm-auth-profile-setup.dry-run/v1",
  ok: true,
  command: [nodeBin, script],
  env,
  receipt_out: receiptOut || null,
  follow_up: {
    acceptance_handoff: [
      "scripts/browser-mac-vm-acceptance-handoff.sh",
      "--restart-source-home",
      "--auth-profile",
      authProfile,
      ...(receiptOut ? ["--auth-setup-receipt", receiptOut] : []),
    ],
  },
}, null, 2));
NODE
  exit 0
fi

printf '[browser-mac-vm-auth-profile-setup] profile: %s\n' "$auth_profile" >&2
printf '[browser-mac-vm-auth-profile-setup] opening %s for %s ms\n' "$open_url" "$hold_ms" >&2
printf '[browser-mac-vm-auth-profile-setup] sign into ela.city inside the Browser VM window before it closes.\n' >&2

env "${setup_env[@]}" "$node_bin" "$repo_root/scripts/home-passkey-virtual-auth-smoke.mjs"
write_receipt "elastos.browser.mac-vm-auth-profile-setup/v1" true "$receipt_out"
if [[ -n "$receipt_out" ]]; then
  printf '[browser-mac-vm-auth-profile-setup] receipt: %s\n' "$receipt_out" >&2
fi
