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

tmp_dir="$(mktemp -d /tmp/elastos-remote-carrier-exit-source-config-smoke-XXXXXX)"
cleanup() {
  rm -rf "$tmp_dir"
}
trap cleanup EXIT

source_config="$tmp_dir/source-exit-provider.json"
source_install="$tmp_dir/source-exit-provider-install.json"
exit_ready="$tmp_dir/exit-ready.json"
exit_bad="$tmp_dir/exit-bad.json"
ticket_file="$tmp_dir/server-ticket.txt"
bootstrap_ticket_file="$tmp_dir/server-bootstrap.json"
candidate="$tmp_dir/source-candidate.json"
candidate_bootstrap="$tmp_dir/source-candidate-bootstrap.json"
candidate_bad="$tmp_dir/source-candidate-bad.json"
candidate_install="$tmp_dir/source-candidate-install.json"
candidate_keep_local="$tmp_dir/source-candidate-keep-local.json"
receipt="$tmp_dir/source-config-receipt.json"
receipt_bootstrap="$tmp_dir/source-config-bootstrap-receipt.json"
receipt_bad="$tmp_dir/source-config-bad-receipt.json"
receipt_install="$tmp_dir/source-config-install-receipt.json"
receipt_keep_local="$tmp_dir/source-config-keep-local-receipt.json"
out="$tmp_dir/source-config.out"
bootstrap_out="$tmp_dir/source-config-bootstrap.out"
bad_out="$tmp_dir/source-config-bad.out"
install_out="$tmp_dir/source-config-install.out"
keep_local_out="$tmp_dir/source-config-keep-local.out"
server_node_id="401264d32427bdfa7343cca7c895e441f1a363c93a6f975f0bb659b0f3919447"
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

cat >"$source_config" <<'JSON'
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
cp "$source_config" "$source_install"

cat >"$exit_ready" <<'JSON'
{
  "timeout_secs": 10,
  "backends": [{
    "id": "server-browser-exit",
    "kind": "stream_relay",
    "allowed_hosts": ["ela.city"],
    "allowed_schemes": ["tcp", "tls"],
    "allowed_ports": [80, 443],
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

cat >"$exit_bad" <<'JSON'
{
  "timeout_secs": 10,
  "backends": [{
    "id": "server-browser-exit",
    "kind": "stream_relay",
    "allowed_hosts": ["ela.city"],
    "allowed_schemes": ["tls"],
    "allowed_ports": [443],
    "allow_private_targets": false
  }]
}
JSON

printf '%s\n' "$server_ticket" >"$ticket_file"
cat >"$bootstrap_ticket_file" <<'JSON'
{
  "schema": "elastos.carrier.bootstrap/v1",
  "transport": "carrier",
  "ticket": "__SERVER_TICKET__",
  "node_id": "__SERVER_NODE_ID__",
  "did": "__SERVER_NODE_ID__"
}
JSON
python3 - "$bootstrap_ticket_file" "$server_node_id" "$server_ticket" <<'PY'
import pathlib, sys
path=pathlib.Path(sys.argv[1])
path.write_text(path.read_text().replace("__SERVER_NODE_ID__", sys.argv[2]).replace("__SERVER_TICKET__", sys.argv[3]))
PY

"$node_bin" "$repo_root/scripts/remote-carrier-exit-source-config.mjs" \
  --source-config "$source_config" \
  --exit-config "$exit_ready" \
  --exit-ticket-file "$ticket_file" \
  --exit-peer-did "$server_node_id" \
  --principal "person:local:alice" \
  --grant-id "operator-grant:server-exit:alice" \
  --target "tls://ela.city:443" \
  --allowed-scheme "tcp" \
  --allowed-scheme "tls" \
  --allowed-port 80 \
  --allowed-port 443 \
  --candidate-config "$candidate" \
  --receipt-out "$receipt" \
  >"$out"

"$node_bin" - "$source_config" "$candidate" "$exit_ready" "$receipt" "$out" <<'NODE'
const fs = require("node:fs");
const crypto = require("node:crypto");
const [sourcePath, candidatePath, exitPath, receiptPath, outPath] = process.argv.slice(2);
const output = fs.readFileSync(outPath, "utf8");
if (/connect_ticket"\s*:/i.test(output)) {
  throw new Error("source config receipt leaked the private connect_ticket");
}
const source = JSON.parse(fs.readFileSync(sourcePath, "utf8"));
const candidate = JSON.parse(fs.readFileSync(candidatePath, "utf8"));
const receipt = JSON.parse(fs.readFileSync(receiptPath, "utf8"));
if (source.backends?.length !== 1 || source.remote_carrier_exits) {
  throw new Error("dry-run source config helper must not modify the source config");
}
if (candidate.backends?.length !== 0 || candidate.remote_carrier_exits?.length !== 1) {
  throw new Error("candidate config must be remote-only");
}
if (candidate.remote_carrier_exits[0].connect_ticket.length < 100) {
  throw new Error("candidate config must carry the private ticket for runtime use");
}
if (JSON.stringify(candidate.remote_carrier_exits[0].allowed_schemes) !== JSON.stringify(["tcp", "tls"]) ||
    JSON.stringify(candidate.remote_carrier_exits[0].allowed_ports) !== JSON.stringify([80, 443])) {
  throw new Error("candidate config must support Browser tcp:80 and tls:443 egress when requested");
}
if (receipt.schema !== "elastos.remote-carrier-exit.source-config/v1" ||
    receipt.ok !== true ||
    receipt.installed !== false ||
    receipt.readiness?.ok !== true ||
    receipt.readiness?.source_config_sha256 !== crypto.createHash("sha256").update(fs.readFileSync(candidatePath)).digest("hex") ||
    receipt.readiness?.exit_config_sha256 !== crypto.createHash("sha256").update(fs.readFileSync(exitPath)).digest("hex") ||
    receipt.remote_exit?.connect_ticket_present !== true ||
    receipt.remote_exit?.connect_ticket_sha256 !== crypto.createHash("sha256").update(candidate.remote_carrier_exits[0].connect_ticket).digest("hex")) {
  throw new Error("receipt must be a redacted, readiness-bound dry-run receipt");
}
NODE

"$node_bin" "$repo_root/scripts/remote-carrier-exit-source-config.mjs" \
  --source-config "$source_config" \
  --exit-config "$exit_ready" \
  --exit-ticket-file "$bootstrap_ticket_file" \
  --exit-peer-did "$server_node_id" \
  --principal "person:local:alice" \
  --grant-id "operator-grant:server-exit:alice" \
  --target "tls://ela.city:443" \
  --candidate-config "$candidate_bootstrap" \
  --receipt-out "$receipt_bootstrap" \
  >"$bootstrap_out"

"$node_bin" - "$candidate_bootstrap" "$exit_ready" "$receipt_bootstrap" "$bootstrap_out" <<'NODE'
const fs = require("node:fs");
const crypto = require("node:crypto");
const [candidatePath, exitPath, receiptPath, outPath] = process.argv.slice(2);
const output = fs.readFileSync(outPath, "utf8");
if (/connect_ticket"\s*:/i.test(output)) {
  throw new Error("bootstrap JSON ticket receipt leaked the private connect_ticket");
}
const candidate = JSON.parse(fs.readFileSync(candidatePath, "utf8"));
const receipt = JSON.parse(fs.readFileSync(receiptPath, "utf8"));
if (candidate.remote_carrier_exits?.[0]?.connect_ticket.length < 100) {
  throw new Error("bootstrap JSON ticket must populate the candidate connect_ticket");
}
if (receipt.ok !== true ||
    receipt.readiness?.source_config_sha256 !== crypto.createHash("sha256").update(fs.readFileSync(candidatePath)).digest("hex") ||
    receipt.readiness?.exit_config_sha256 !== crypto.createHash("sha256").update(fs.readFileSync(exitPath)).digest("hex") ||
    receipt.remote_exit?.connect_ticket_sha256 !== crypto.createHash("sha256").update(candidate.remote_carrier_exits[0].connect_ticket).digest("hex")) {
  throw new Error("bootstrap JSON ticket receipt must be redacted and hash-bound");
}
NODE

"$node_bin" "$repo_root/scripts/remote-carrier-exit-source-config.mjs" \
  --source-config "$source_config" \
  --exit-config "$exit_ready" \
  --exit-ticket-file "$ticket_file" \
  --exit-peer-did "$server_node_id" \
  --principal "person:local:alice" \
  --grant-id "operator-grant:server-exit:mac" \
  --remote-exit-id "seed-node" \
  --target "tls://ela.city:443" \
  --allowed-scheme "tcp" \
  --allowed-scheme "tls" \
  --allowed-port 80 \
  --allowed-port 443 \
  --candidate-config "$candidate_keep_local" \
  --receipt-out "$receipt_keep_local" \
  --keep-local-backends \
  >"$keep_local_out"

"$node_bin" - "$source_config" "$candidate_keep_local" "$exit_ready" "$receipt_keep_local" "$keep_local_out" <<'NODE'
const fs = require("node:fs");
const crypto = require("node:crypto");
const [sourcePath, candidatePath, exitPath, receiptPath, outPath] = process.argv.slice(2);
const output = fs.readFileSync(outPath, "utf8");
if (/connect_ticket"\s*:/i.test(output)) {
  throw new Error("keep-local receipt leaked the private connect_ticket");
}
const source = JSON.parse(fs.readFileSync(sourcePath, "utf8"));
const candidate = JSON.parse(fs.readFileSync(candidatePath, "utf8"));
const receipt = JSON.parse(fs.readFileSync(receiptPath, "utf8"));
if (source.backends?.length !== 1 || source.remote_carrier_exits) {
  throw new Error("keep-local dry-run must not modify the source config");
}
if (candidate.backends?.length !== 1 || candidate.remote_carrier_exits?.length !== 1) {
  throw new Error("keep-local candidate must preserve local backends and add one remote exit");
}
if (candidate.remote_carrier_exits[0].id !== "seed-node") {
  throw new Error("keep-local candidate must honor --remote-exit-id");
}
if (receipt.schema !== "elastos.remote-carrier-exit.source-config/v1" ||
    receipt.ok !== true ||
    receipt.installed !== false ||
    receipt.readiness?.ok !== true ||
    receipt.readiness?.source_remote_only !== false ||
    receipt.readiness?.source_local_backends_allowed !== true ||
    receipt.readiness?.source_local_backend_count !== 1 ||
    receipt.readiness?.source_config_sha256 !== crypto.createHash("sha256").update(fs.readFileSync(candidatePath)).digest("hex") ||
    receipt.readiness?.exit_config_sha256 !== crypto.createHash("sha256").update(fs.readFileSync(exitPath)).digest("hex")) {
  throw new Error("keep-local receipt must be redacted, hash-bound, and explicit about local backend preservation");
}
NODE

set +e
"$node_bin" "$repo_root/scripts/remote-carrier-exit-source-config.mjs" \
  --source-config "$source_config" \
  --exit-config "$exit_bad" \
  --exit-ticket-file "$ticket_file" \
  --exit-peer-did "$server_node_id" \
  --principal "person:local:alice" \
  --grant-id "operator-grant:server-exit:alice" \
  --target "tls://ela.city:443" \
  --candidate-config "$candidate_bad" \
  --receipt-out "$receipt_bad" \
  >"$bad_out"
bad_status=$?
set -e
if [[ "$bad_status" -eq 0 ]]; then
  echo "source config helper accepted an exit backend without relay IPC" >&2
  cat "$bad_out" >&2
  exit 1
fi

"$node_bin" - "$receipt_bad" "$bad_out" <<'NODE'
const fs = require("node:fs");
const receipt = JSON.parse(fs.readFileSync(process.argv[2], "utf8"));
const output = fs.readFileSync(process.argv[3], "utf8");
if (/connect_ticket"\s*:/i.test(output)) {
  throw new Error("failed source config receipt leaked the private connect_ticket");
}
if (receipt.ok !== false ||
    receipt.readiness?.ok !== false ||
    !receipt.readiness?.failures?.includes("exit_backend_relay_ipc_required_for_remote_carrier_handoff")) {
  throw new Error("failed helper receipt must expose readiness failure");
}
NODE

"$node_bin" "$repo_root/scripts/remote-carrier-exit-source-config.mjs" \
  --source-config "$source_install" \
  --exit-config "$exit_ready" \
  --exit-ticket-file "$ticket_file" \
  --exit-peer-did "$server_node_id" \
  --principal "person:local:alice" \
  --grant-id "operator-grant:server-exit:alice" \
  --target "tls://ela.city:443" \
  --candidate-config "$candidate_install" \
  --receipt-out "$receipt_install" \
  --install \
  >"$install_out"

"$node_bin" - "$source_install" "$exit_ready" "$receipt_install" <<'NODE'
const fs = require("node:fs");
const crypto = require("node:crypto");
const [sourcePath, exitPath, receiptPath] = process.argv.slice(2);
const source = JSON.parse(fs.readFileSync(sourcePath, "utf8"));
const receipt = JSON.parse(fs.readFileSync(receiptPath, "utf8"));
if (source.backends?.length !== 0 || source.remote_carrier_exits?.length !== 1) {
  throw new Error("--install must replace the source config with the remote-only candidate");
}
if (receipt.ok !== true || receipt.installed !== true || !receipt.backup_config || !fs.existsSync(receipt.backup_config)) {
  throw new Error("--install receipt must include an existing backup path");
}
if (receipt.readiness?.source_config_sha256 !== crypto.createHash("sha256").update(fs.readFileSync(sourcePath)).digest("hex") ||
    receipt.readiness?.exit_config_sha256 !== crypto.createHash("sha256").update(fs.readFileSync(exitPath)).digest("hex")) {
  throw new Error("--install receipt readiness hashes must match the installed source config and exit config");
}
const backup = JSON.parse(fs.readFileSync(receipt.backup_config, "utf8"));
if (backup.backends?.length !== 1) {
  throw new Error("backup must preserve the prior local source config");
}
NODE

printf '{"schema":"elastos.remote-carrier-exit.source-config-smoke/v1","ok":true,"dry_run_redacted":true,"bootstrap_json_ticket_accepted":true,"keep_local_backends_supported":true,"readiness_failure_rejected":true,"install_backed_up":true}\n'
