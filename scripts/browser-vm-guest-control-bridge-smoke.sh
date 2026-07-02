#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
tmp_dir="$(mktemp -d)"
bridge_pid=""
guest_pid=""
request_pid=""

cleanup() {
  for pid in "$request_pid" "$bridge_pid" "$guest_pid"; do
    [[ -n "$pid" ]] || continue
    kill "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
  done
  rm -rf "$tmp_dir"
}
trap cleanup EXIT

cd "$repo_root"

cargo build --quiet --manifest-path elastos/tools/browser-vm-guest-control-bridge/Cargo.toml

bridge_bin="$repo_root/elastos/tools/browser-vm-guest-control-bridge/target/debug/browser-vm-guest-control-bridge"
guest_socket="$tmp_dir/guest-control.sock"
host_socket="$tmp_dir/host-control.sock"
proof_path="$tmp_dir/proof.json"

cat >"$tmp_dir/fake_guest_control.mjs" <<'NODE'
import fs from "node:fs";
import http from "node:http";

const [socketPath, proofPath] = process.argv.slice(2);
try {
  fs.unlinkSync(socketPath);
} catch {}

const server = http.createServer((req, res) => {
  if (req.url !== "/status") {
    res.writeHead(404).end();
    return;
  }
  fs.writeFileSync(proofPath, JSON.stringify({method: req.method, url: req.url}));
  const body = Buffer.from(JSON.stringify({
    schema: "elastos.browser.selkies-control.status/v1",
    ok: true,
  }));
  res.writeHead(200, {
    "content-type": "application/json",
    "content-length": body.length,
    "connection": "close",
  });
  res.end(body);
});
server.listen(socketPath);
NODE

config_json="$(python3 - <<PY
import json
print(json.dumps({
    "schema": "elastos.browser.vm-guest-control-bridge.config/v1",
    "guest_control_socket_path": "$guest_socket",
    "network_mode": "runtime_net_only",
    "direct_network": False,
    "transport": {
        "kind": "unix_listen",
        "path": "$host_socket",
    },
    "replace_existing_socket": True,
    "max_sessions": 1,
    "control_socket_ready_timeout_ms": 5000,
    "control_request_timeout_ms": 5000,
}))
PY
)"

ELASTOS_BROWSER_VM_CONTROL_BRIDGE_CONFIG="$config_json" \
  "$bridge_bin" >"$tmp_dir/bridge.out" 2>"$tmp_dir/bridge.err" &
bridge_pid="$!"

for _ in {1..100}; do
  [[ -S "$host_socket" ]] && break
  sleep 0.05
done
if [[ ! -S "$host_socket" ]]; then
  cat "$tmp_dir/bridge.err" >&2 || true
  exit 1
fi

HOST_SOCKET="$host_socket" node - <<'NODE' >"$tmp_dir/request.out" 2>"$tmp_dir/request.err" &
import http from "node:http";

const socketPath = process.env.HOST_SOCKET;
const body = await new Promise((resolve, reject) => {
  const req = http.request({socketPath, path: "/status", method: "GET"}, (res) => {
    let chunks = "";
    res.setEncoding("utf8");
    res.on("data", (chunk) => chunks += chunk);
    res.on("end", () => resolve(chunks));
  });
  req.on("error", reject);
  req.end();
});
const parsed = JSON.parse(body);
if (parsed.schema !== "elastos.browser.selkies-control.status/v1") {
  throw new Error(`unexpected status payload: ${body}`);
}
NODE
request_pid="$!"

sleep 0.2
node "$tmp_dir/fake_guest_control.mjs" "$guest_socket" "$proof_path" >"$tmp_dir/guest.out" 2>"$tmp_dir/guest.err" &
guest_pid="$!"

for _ in {1..100}; do
  [[ -S "$guest_socket" ]] && break
  sleep 0.05
done
if [[ ! -S "$guest_socket" ]]; then
  cat "$tmp_dir/guest.err" >&2 || true
  exit 1
fi

if ! wait "$request_pid"; then
  cat "$tmp_dir/request.err" >&2 || true
  cat "$tmp_dir/bridge.err" >&2 || true
  exit 1
fi

wait "$bridge_pid"
if [[ -n "$guest_pid" ]]; then
  kill "$guest_pid" 2>/dev/null || true
  wait "$guest_pid" 2>/dev/null || true
fi

BRIDGE_OUT="$tmp_dir/bridge.out" PROOF_PATH="$proof_path" python3 - <<'PY'
import json
import os
import pathlib

ready = json.loads(pathlib.Path(os.environ["BRIDGE_OUT"]).read_text(encoding="utf-8").splitlines()[0])
proof = json.loads(pathlib.Path(os.environ["PROOF_PATH"]).read_text(encoding="utf-8"))
assert ready["schema"] == "elastos.browser.vm-guest-control-bridge.ready/v1"
assert ready["network_mode"] == "runtime_net_only"
assert ready["direct_network"] is False
assert ready["transport"] == "unix_listen"
assert ready["control_socket_ready_timeout_ms"] == 5000
assert ready["control_request_timeout_ms"] == 5000
assert proof == {"method": "GET", "url": "/status"}

print(json.dumps({
    "schema": "elastos.browser.vm-guest-control-bridge-smoke/v1",
    "ok": True,
    "transport": ready["transport"],
}))
PY
