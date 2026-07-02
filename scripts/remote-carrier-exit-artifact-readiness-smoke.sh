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

tmp_dir="$(mktemp -d /tmp/elastos-remote-carrier-exit-artifact-readiness-smoke-XXXXXX)"
cleanup() {
  rm -rf "$tmp_dir"
}
trap cleanup EXIT

valid_gateway="$tmp_dir/elastos-valid"
stale_gateway="$tmp_dir/elastos-stale"
valid_exit_provider="$tmp_dir/exit-provider-valid"
stale_exit_provider="$tmp_dir/exit-provider-stale"
valid_out="$tmp_dir/valid.json"
stale_gateway_out="$tmp_dir/stale-gateway.json"
stale_provider_out="$tmp_dir/stale-provider.json"

cat >"$valid_gateway" <<'EOF_GATEWAY'
binary text
browser_exit_stream
elastos.browser.carrier-stream/v1
elastos://exit/open_stream
EOF_GATEWAY

cat >"$stale_gateway" <<'EOF_GATEWAY'
binary text
elastos://exit/open_stream
EOF_GATEWAY

cat >"$valid_exit_provider" <<'EOF_PROVIDER'
binary text
remote_carrier_exits
elastos.exit.remote-carrier.discovery/v1
elastos.exit.remote-carrier.quote/v1
elastos.exit.remote-carrier-session/v1
elastos.exit.relay-ipc/v1
max_active_streams_per_principal
EOF_PROVIDER

cat >"$stale_exit_provider" <<'EOF_PROVIDER'
binary text
elastos.exit.relay-ipc/v1
EOF_PROVIDER

"$node_bin" "$repo_root/scripts/remote-carrier-exit-artifact-readiness.mjs" \
  --gateway-bin "$valid_gateway" \
  --exit-provider-bin "$valid_exit_provider" \
  >"$valid_out"

"$node_bin" - "$valid_out" <<'NODE'
const fs = require("node:fs");
const result = JSON.parse(fs.readFileSync(process.argv[2], "utf8"));
if (result.schema !== "elastos.remote-carrier-exit.artifact-readiness/v1" ||
    result.ok !== true ||
    result.artifacts?.gateway?.required_strings?.some((item) => item.present !== true) ||
    result.artifacts?.exit_provider?.required_strings?.some((item) => item.present !== true)) {
  throw new Error("valid artifact readiness did not pass with all required strings present");
}
for (const artifact of [result.artifacts.gateway, result.artifacts.exit_provider]) {
  if (!/^[a-f0-9]{64}$/.test(artifact.sha256) || artifact.size_bytes <= 0) {
    throw new Error("artifact readiness did not report stable hash and size");
  }
}
NODE

set +e
"$node_bin" "$repo_root/scripts/remote-carrier-exit-artifact-readiness.mjs" \
  --gateway-bin "$stale_gateway" \
  --exit-provider-bin "$valid_exit_provider" \
  >"$stale_gateway_out"
stale_gateway_status=$?
set -e
if [[ "$stale_gateway_status" -eq 0 ]]; then
  echo "artifact readiness accepted a gateway without Browser Carrier stream support" >&2
  cat "$stale_gateway_out" >&2
  exit 1
fi

"$node_bin" - "$stale_gateway_out" <<'NODE'
const fs = require("node:fs");
const result = JSON.parse(fs.readFileSync(process.argv[2], "utf8"));
if (result.ok !== false ||
    !result.failures.includes("gateway_missing_browser_exit_stream") ||
    !result.failures.includes("gateway_missing_elastos.browser.carrier-stream/v1")) {
  throw new Error("stale gateway failure did not name missing Browser Carrier stream capability");
}
NODE

set +e
"$node_bin" "$repo_root/scripts/remote-carrier-exit-artifact-readiness.mjs" \
  --gateway-bin "$valid_gateway" \
  --exit-provider-bin "$stale_exit_provider" \
  >"$stale_provider_out"
stale_provider_status=$?
set -e
if [[ "$stale_provider_status" -eq 0 ]]; then
  echo "artifact readiness accepted an exit-provider without remote Carrier policy support" >&2
  cat "$stale_provider_out" >&2
  exit 1
fi

"$node_bin" - "$stale_provider_out" <<'NODE'
const fs = require("node:fs");
const result = JSON.parse(fs.readFileSync(process.argv[2], "utf8"));
for (const id of [
  "exit_provider_missing_remote_carrier_exits",
  "exit_provider_missing_elastos.exit.remote-carrier-session/v1",
  "exit_provider_missing_max_active_streams_per_principal",
]) {
  if (!result.failures.includes(id)) {
    throw new Error(`stale exit-provider failure missing ${id}`);
  }
}
NODE

printf '{"schema":"elastos.remote-carrier-exit.artifact-readiness-smoke/v1","ok":true,"valid_artifacts_accepted":true,"stale_gateway_rejected":true,"stale_exit_provider_rejected":true}\n'
