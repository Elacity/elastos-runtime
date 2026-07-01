#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

socket_path="$tmp_dir/browser-vm-control.sock"
stale_socket_path="$tmp_dir/stale-browser-vm-control.sock"

python3 - "$stale_socket_path" <<'PY'
import os
import socket
import sys

path = sys.argv[1]
try:
    os.unlink(path)
except FileNotFoundError:
    pass
server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
server.bind(path)
server.close()
PY

stale_output="$(ELASTOS_BROWSER_VM_PLATFORM=linux-amd64 \
  ELASTOS_BROWSER_VM_DATA_DIR="$tmp_dir/data" \
  ELASTOS_BROWSER_VM_CONTROL_SOCKET="$stale_socket_path" \
  "$repo_root/scripts/browser-vm-engine-preflight.sh")"

OUTPUT="$stale_output" node - <<'NODE'
const result = JSON.parse(process.env.OUTPUT);
if (result.ok !== false || result.launch_ready !== false) {
  throw new Error(`stale remote control socket must fail closed: ${process.env.OUTPUT}`);
}
if (result.control_socket?.connect_ok !== false) {
  throw new Error(`stale socket must not be connect-ready: ${process.env.OUTPUT}`);
}
NODE

python3 - "$socket_path" <<'PY' &
import os
import socket
import sys

path = sys.argv[1]
try:
    os.unlink(path)
except FileNotFoundError:
    pass
server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
server.bind(path)
server.listen(1)
conn, _ = server.accept()
with conn:
    conn.recv(4096)
    conn.sendall(
        b"HTTP/1.1 200 OK\r\n"
        b"Content-Type: application/json\r\n"
        b"Connection: close\r\n"
        b"\r\n"
        b'{"schema":"elastos.browser.vm-control-service.status/v1"}'
    )
server.close()
PY
server_pid="$!"
trap 'kill "$server_pid" 2>/dev/null || true; rm -rf "$tmp_dir"' EXIT

for _ in {1..100}; do
  [[ -S "$socket_path" ]] && break
  sleep 0.05
done
[[ -S "$socket_path" ]] || {
  echo "fake Browser VM control socket did not appear" >&2
  exit 1
}

output="$(ELASTOS_BROWSER_VM_PLATFORM=linux-amd64 \
  ELASTOS_BROWSER_VM_DATA_DIR="$tmp_dir/data" \
  ELASTOS_BROWSER_VM_CONTROL_SOCKET="$socket_path" \
  "$repo_root/scripts/browser-vm-engine-preflight.sh")"

OUTPUT="$output" node - <<'NODE'
const result = JSON.parse(process.env.OUTPUT);
if (result.schema !== "elastos.browser.vm-engine-preflight/v1") throw new Error("wrong schema");
if (result.ok !== true || result.launch_ready !== true) throw new Error(`remote control socket should make launch ready: ${process.env.OUTPUT}`);
if (result.execution_mode !== "remote_vm_control_socket") throw new Error(`wrong execution mode: ${process.env.OUTPUT}`);
if (result.remote_control_supported !== true) throw new Error("remote control support not advertised");
if (!String(result.reason || "").includes("local KVM/VZ is not required")) throw new Error(`missing no-local-substrate reason: ${process.env.OUTPUT}`);
NODE

printf '%s\n' '{"schema":"elastos.browser.vm-remote-control-preflight-smoke/v1","ok":true}'
