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

tmp_dir="$(mktemp -d /tmp/elastos-remote-carrier-exit-readiness-smoke-XXXXXX)"
cleanup() {
  rm -rf "$tmp_dir"
}
trap cleanup EXIT

source_local="$tmp_dir/source-local.json"
source_remote="$tmp_dir/source-remote.json"
source_remote_bad="$tmp_dir/source-remote-bad.json"
exit_local="$tmp_dir/exit-local.json"
exit_ready="$tmp_dir/exit-ready.json"
valid_out="$tmp_dir/valid-readiness.json"
local_out="$tmp_dir/local-readiness.json"
bad_out="$tmp_dir/bad-readiness.json"
server_node_id="401264d32427bdfa7343cca7c895e441f1a363c93a6f975f0bb659b0f3919447"
wrong_node_id="36457609ddadd973077afc4536ec6c2641f94195b0450f6eca0d40fac2bd69df"
server_ticket="$("$node_bin" - "$server_node_id" <<'NODE'
const alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
function base32(buf) {
  let bits = 0;
  let bitCount = 0;
  let out = "";
  for (const byte of buf) {
    bits = (bits << 8) | byte;
    bitCount += 8;
    while (bitCount >= 5) {
      bitCount -= 5;
      out += alphabet[(bits >> bitCount) & 31];
    }
  }
  if (bitCount > 0) out += alphabet[(bits << (5 - bitCount)) & 31];
  return out.toLowerCase();
}
const nodeId = process.argv[2];
const ticket = { topic: null, endpoints: [{ id: nodeId, addrs: [{ Ip: "203.0.113.10:4433" }] }] };
process.stdout.write(base32(Buffer.from(JSON.stringify(ticket))));
NODE
)"

cat >"$source_local" <<'JSON'
{
  "timeout_secs": 10,
  "backends": [{
    "id": "source-home-browser-exit",
    "kind": "stream_relay",
    "allowed_hosts": ["*"],
    "allowed_schemes": ["tcp", "tls"],
    "allowed_ports": [80, 443],
    "allow_private_targets": false
  }]
}
JSON

cat >"$exit_local" <<'JSON'
{
  "timeout_secs": 10,
  "backends": [{
    "id": "server-browser-exit",
    "kind": "stream_relay",
    "allowed_hosts": ["*"],
    "allowed_schemes": ["tcp", "tls"],
    "allowed_ports": [80, 443],
    "allow_private_targets": false
  }]
}
JSON

cat >"$source_remote" <<'JSON'
{
  "timeout_secs": 10,
  "remote_carrier_exits": [{
    "id": "server-exit",
    "grant_id": "operator-grant:server-exit:alice",
    "peer_did": "__SERVER_NODE_ID__",
    "carrier_service": "elastos://exit/open_stream",
    "connect_ticket": "__SERVER_TICKET__",
    "allowed_principals": ["person:local:alice"],
    "allowed_hosts": ["*.example.com"],
    "allowed_schemes": ["tls"],
    "allowed_ports": [443],
    "max_active_streams": 2,
    "max_active_streams_per_principal": 1
  }]
}
JSON
python3 - "$source_remote" "$server_node_id" "$server_ticket" <<'PY'
import pathlib, sys
path=pathlib.Path(sys.argv[1])
text=path.read_text().replace("__SERVER_NODE_ID__", sys.argv[2]).replace("__SERVER_TICKET__", sys.argv[3])
path.write_text(text)
PY

cat >"$source_remote_bad" <<'JSON'
{
  "timeout_secs": 10,
  "backends": [{
    "id": "fallback-local-exit",
    "kind": "stream_relay",
    "allowed_hosts": ["*"],
    "allowed_schemes": ["tcp", "tls"],
    "allowed_ports": [80, 443],
    "allow_private_targets": false
  }],
  "remote_carrier_exits": [{
    "id": "server-exit",
    "grant_id": "operator-grant:server-exit:alice",
    "peer_did": "__WRONG_NODE_ID__",
    "connect_ticket": "__SERVER_TICKET__",
    "allowed_principals": ["person:local:bob"],
    "allowed_hosts": ["example.net"],
    "allowed_schemes": ["tls"],
    "allowed_ports": [443]
  }]
}
JSON
python3 - "$source_remote_bad" "$wrong_node_id" "$server_ticket" <<'PY'
import pathlib, sys
path=pathlib.Path(sys.argv[1])
text=path.read_text().replace("__WRONG_NODE_ID__", sys.argv[2]).replace("__SERVER_TICKET__", sys.argv[3])
path.write_text(text)
PY

cat >"$exit_ready" <<'JSON'
{
  "timeout_secs": 10,
  "backends": [{
    "id": "server-browser-exit",
    "kind": "stream_relay",
    "allowed_hosts": ["*.example.com"],
    "allowed_schemes": ["tls"],
    "allowed_ports": [443],
    "allow_private_targets": false,
    "adapter_ipc": {
      "kind": "unix_socket",
      "path": "/redacted/source-runtime-stream.sock"
    },
    "relay_ipc": {
      "kind": "unix_socket",
      "path": "/redacted/exit-relay.sock"
    }
  }]
}
JSON

set +e
"$node_bin" "$repo_root/scripts/remote-carrier-exit-readiness.mjs" \
  --source-config "$source_local" \
  --exit-config "$exit_local" \
  --principal "person:local:alice" \
  --grant-id "operator-grant:server-exit:alice" \
  --target "tls://www.example.com:443" \
  --exit-did "$server_node_id" \
  >"$local_out"
local_status=$?
set -e
if [[ "$local_status" -eq 0 ]]; then
  echo "readiness accepted a local-only source config" >&2
  cat "$local_out" >&2
  exit 1
fi

"$node_bin" - "$local_out" <<'NODE'
const fs = require("node:fs");
const result = JSON.parse(fs.readFileSync(process.argv[2], "utf8"));
if (result.ok !== false ||
    !result.failures.includes("source_config_must_not_keep_local_exit_backends_for_remote_carrier_acceptance") ||
    !result.failures.includes("source_config_has_no_remote_carrier_exits") ||
    !result.failures.includes("exit_backend_relay_ipc_required_for_remote_carrier_handoff")) {
  throw new Error("local-only readiness failure did not expose the expected blockers");
}
NODE

set +e
"$node_bin" "$repo_root/scripts/remote-carrier-exit-readiness.mjs" \
  --source-config "$source_remote_bad" \
  --exit-config "$exit_ready" \
  --principal "person:local:alice" \
  --grant-id "operator-grant:server-exit:alice" \
  --target "tls://www.example.com:443" \
  --exit-did "$server_node_id" \
  >"$bad_out"
bad_status=$?
set -e
if [[ "$bad_status" -eq 0 ]]; then
  echo "readiness accepted a remote config with fallback/mismatched authority" >&2
  cat "$bad_out" >&2
  exit 1
fi

"$node_bin" - "$bad_out" <<'NODE'
const fs = require("node:fs");
const result = JSON.parse(fs.readFileSync(process.argv[2], "utf8"));
for (const id of [
  "source_config_must_not_keep_local_exit_backends_for_remote_carrier_acceptance",
  "source_remote_exit_peer_did_must_match_exit_runtime",
  "source_remote_exit_peer_did_must_match_connect_ticket",
  "source_remote_exit_principal_not_allowed",
  "source_remote_exit_target_policy_does_not_allow_target",
]) {
  if (!result.failures.includes(id)) {
    throw new Error(`missing expected remote readiness blocker: ${id}`);
  }
}
NODE

"$node_bin" "$repo_root/scripts/remote-carrier-exit-readiness.mjs" \
  --source-config "$source_remote" \
  --exit-config "$exit_ready" \
  --principal "person:local:alice" \
  --grant-id "operator-grant:server-exit:alice" \
  --target "tls://www.example.com:443" \
  --exit-did "$server_node_id" \
  >"$valid_out"

"$node_bin" - "$valid_out" "$source_remote" "$exit_ready" <<'NODE'
const fs = require("node:fs");
const crypto = require("node:crypto");
const text = fs.readFileSync(process.argv[2], "utf8");
if (/connect_ticket"\s*:/i.test(text)) {
  throw new Error("readiness output leaked private connect_ticket material");
}
const result = JSON.parse(text);
if (result.schema !== "elastos.remote-carrier-exit.readiness/v1" || result.ok !== true) {
  throw new Error("valid remote Carrier Exit config did not pass readiness");
}
if (result.source?.remote_only !== true ||
    result.source?.selected_remote_exit?.connect_ticket_present !== true ||
    result.source?.selected_remote_exit?.connect_ticket_peer_match !== true ||
    result.route?.byte_transport !== "carrier_stream" ||
    result.exit?.selected_stream_relay_backend?.adapter_ipc_present !== true ||
    result.exit?.selected_stream_relay_backend?.relay_ipc_present !== true) {
  throw new Error("valid readiness output is missing remote-only or relay evidence");
}
const sourceHash = crypto.createHash("sha256").update(fs.readFileSync(process.argv[3])).digest("hex");
const exitHash = crypto.createHash("sha256").update(fs.readFileSync(process.argv[4])).digest("hex");
if (result.source?.config_sha256 !== sourceHash || result.exit?.config_sha256 !== exitHash) {
  throw new Error("valid readiness output is not hash-bound to source and exit configs");
}
NODE

printf '{"schema":"elastos.remote-carrier-exit.readiness-smoke/v1","ok":true,"local_only_rejected":true,"bad_remote_rejected":true,"valid_remote_accepted":true,"ticket_redacted":true}\n'
