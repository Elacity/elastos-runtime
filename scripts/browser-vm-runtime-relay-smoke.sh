#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
tmp_dir="$(mktemp -d)"

cleanup() {
  rm -rf "$tmp_dir"
}
trap cleanup EXIT

cd "$repo_root"

cargo build --quiet --manifest-path elastos/tools/browser-vm-runtime-relay/Cargo.toml

relay_bin="$repo_root/elastos/tools/browser-vm-runtime-relay/target/debug/browser-vm-runtime-relay"
guest_socket="$tmp_dir/guest-relay.sock"
host_socket="$tmp_dir/host-bridge.sock"
host_proof="$tmp_dir/host-proof.json"
client_proof="$tmp_dir/client-proof.json"

cat >"$tmp_dir/fake_host_bridge.py" <<'PY'
#!/usr/bin/env python3
import json
import os
import socket
import sys

path, proof_path = sys.argv[1], sys.argv[2]
try:
    os.unlink(path)
except FileNotFoundError:
    pass

server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
server.bind(path)
server.listen(1)
conn, _ = server.accept()
with conn:
    payload = conn.recv(4096)
    conn.sendall(b"vm-runtime-relay-ok")
with open(proof_path, "w", encoding="utf-8") as handle:
    json.dump({"payload": payload.decode("utf-8")}, handle)
server.close()
PY

cat >"$tmp_dir/fake_guest_client.py" <<'PY'
#!/usr/bin/env python3
import json
import socket
import sys

path, proof_path = sys.argv[1], sys.argv[2]
client = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
client.connect(path)
with client:
    client.sendall(b"guest-proxy-bytes")
    response = client.recv(4096)
with open(proof_path, "w", encoding="utf-8") as handle:
    json.dump({"response": response.decode("utf-8")}, handle)
PY
chmod +x "$tmp_dir/fake_host_bridge.py" "$tmp_dir/fake_guest_client.py"

"$tmp_dir/fake_host_bridge.py" "$host_socket" "$host_proof" >"$tmp_dir/host.out" 2>"$tmp_dir/host.err" &
host_pid="$!"

for _ in {1..100}; do
  [[ -S "$host_socket" ]] && break
  sleep 0.05
done
if [[ ! -S "$host_socket" ]]; then
  cat "$tmp_dir/host.err" >&2 || true
  exit 1
fi

config_json="$(python3 - <<PY
import json
print(json.dumps({
    "schema": "elastos.browser.vm-runtime-relay.config/v1",
    "guest_relay_ipc_path": "$guest_socket",
    "network_mode": "runtime_net_only",
    "direct_network": False,
    "transport": {
        "kind": "unix_socket",
        "path": "$host_socket",
    },
    "max_sessions": 1,
}))
PY
)"

ELASTOS_BROWSER_VM_RUNTIME_RELAY_CONFIG="$config_json" \
  "$relay_bin" >"$tmp_dir/relay.out" 2>"$tmp_dir/relay.err" &
relay_pid="$!"

for _ in {1..100}; do
  [[ -S "$guest_socket" ]] && break
  sleep 0.05
done
if [[ ! -S "$guest_socket" ]]; then
  cat "$tmp_dir/relay.err" >&2 || true
  exit 1
fi

"$tmp_dir/fake_guest_client.py" "$guest_socket" "$client_proof"
wait "$relay_pid"
wait "$host_pid"

RELAY_OUT="$tmp_dir/relay.out" HOST_PROOF="$host_proof" CLIENT_PROOF="$client_proof" python3 - <<'PY'
import json
import os
import pathlib

ready = json.loads(pathlib.Path(os.environ["RELAY_OUT"]).read_text(encoding="utf-8").splitlines()[0])
host = json.loads(pathlib.Path(os.environ["HOST_PROOF"]).read_text(encoding="utf-8"))
client = json.loads(pathlib.Path(os.environ["CLIENT_PROOF"]).read_text(encoding="utf-8"))

assert ready["schema"] == "elastos.browser.vm-runtime-relay.ready/v1"
assert ready["network_mode"] == "runtime_net_only"
assert ready["direct_network"] is False
assert ready["transport"] == "unix_socket"
assert host["payload"] == "guest-proxy-bytes"
assert client["response"] == "vm-runtime-relay-ok"

print(json.dumps({
    "schema": "elastos.browser.vm-runtime-relay-smoke/v1",
    "ok": True,
    "transport": ready["transport"],
}))
PY
