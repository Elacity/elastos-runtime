#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
tmp_dir="$(mktemp -d)"
runtime_echo_pid=""

cleanup() {
  if [[ -n "$runtime_echo_pid" ]]; then
    kill "$runtime_echo_pid" >/dev/null 2>&1 || true
  fi
  rm -rf "$tmp_dir"
}
trap cleanup EXIT

if [[ "$(uname -s)" != "Linux" ]]; then
  echo '{"skipped":true,"reason":"browser native supervisor smoke requires Linux CLONE_NEWNET"}'
  exit 0
fi

cd "$repo_root"

cargo build --quiet --manifest-path elastos/tools/browser-engine-supervisor/Cargo.toml
cargo build --quiet --manifest-path elastos/tools/browser-stream-bridge/Cargo.toml

supervisor_bin="$repo_root/elastos/tools/browser-engine-supervisor/target/debug/browser-engine-supervisor"
stream_bridge_bin="$repo_root/elastos/tools/browser-stream-bridge/target/debug/browser-stream-bridge"
runtime_socket="$tmp_dir/runtime.sock"
adapter_socket="$tmp_dir/adapter.sock"
proof_file="$adapter_socket.proof.json"

cat >"$tmp_dir/runtime_echo.py" <<'PY'
#!/usr/bin/env python3
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
    while True:
        data = conn.recv(4096)
        if not data:
            break
        conn.sendall(data)
server.close()
PY
chmod +x "$tmp_dir/runtime_echo.py"
"$tmp_dir/runtime_echo.py" "$runtime_socket" >"$tmp_dir/runtime_echo.out" 2>"$tmp_dir/runtime_echo.err" &
runtime_echo_pid="$!"

for _ in {1..100}; do
  [[ -S "$runtime_socket" ]] && break
  sleep 0.05
done
if [[ ! -S "$runtime_socket" ]]; then
  echo "runtime echo did not create $runtime_socket" >&2
  cat "$tmp_dir/runtime_echo.err" >&2 || true
  exit 1
fi

cat >"$tmp_dir/native_probe.py" <<'PY'
#!/usr/bin/env python3
import json
import os
import pathlib
import socket
import sys

ipc_path = os.environ["ELASTOS_BROWSER_ENGINE_IPC"]
proof_path = pathlib.Path(ipc_path + ".proof.json")

direct_error = None
direct_tcp_blocked = False
sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
sock.settimeout(0.75)
try:
    sock.connect(("1.1.1.1", 443))
except OSError as exc:
    direct_tcp_blocked = True
    direct_error = str(exc)
finally:
    sock.close()

direct_dns_error = None
direct_dns_blocked = False
try:
    socket.getaddrinfo("example.com", 80)
except OSError as exc:
    direct_dns_blocked = True
    direct_dns_error = str(exc)

direct_http_error = None
direct_http_blocked = False
http_sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
http_sock.settimeout(0.75)
try:
    http_sock.connect(("93.184.216.34", 80))
    http_sock.sendall(b"GET / HTTP/1.1\r\nHost: example.com\r\nConnection: close\r\n\r\n")
    response = http_sock.recv(16)
    direct_http_blocked = not bool(response)
except OSError as exc:
    direct_http_blocked = True
    direct_http_error = str(exc)
finally:
    http_sock.close()

stream_bridge_echo = False
bridge_error = None
unix_sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
unix_sock.settimeout(2)
try:
    unix_sock.connect(ipc_path)
    payload = b"elastos-native-supervisor-smoke\n"
    unix_sock.sendall(payload)
    received = unix_sock.recv(len(payload))
    stream_bridge_echo = received == payload
except OSError as exc:
    bridge_error = str(exc)
finally:
    unix_sock.close()

proof = {
    "schema": "elastos.browser.native-supervisor-proof/v1",
    "stream_id": os.environ["ELASTOS_BROWSER_ENGINE_STREAM_ID"],
    "target": os.environ["ELASTOS_BROWSER_ENGINE_TARGET"],
    "url": os.environ["ELASTOS_BROWSER_ENGINE_URL"],
    "direct_tcp_blocked": direct_tcp_blocked,
    "direct_tcp_error": direct_error,
    "direct_dns_blocked": direct_dns_blocked,
    "direct_dns_error": direct_dns_error,
    "direct_http_blocked": direct_http_blocked,
    "direct_http_error": direct_http_error,
    "stream_bridge_echo": stream_bridge_echo,
    "stream_bridge_error": bridge_error,
}
proof_path.write_text(json.dumps(proof), encoding="utf-8")

if not direct_tcp_blocked:
    raise SystemExit("direct TCP unexpectedly succeeded inside native browser namespace")
if not direct_dns_blocked:
    raise SystemExit("direct DNS unexpectedly succeeded inside native browser namespace")
if not direct_http_blocked:
    raise SystemExit("direct HTTP unexpectedly succeeded inside native browser namespace")
if not stream_bridge_echo:
    raise SystemExit("Runtime stream bridge echo failed inside native browser namespace")
PY
chmod +x "$tmp_dir/native_probe.py"

request_json="$(python3 - <<PY
import json
print(json.dumps({
    "schema": "elastos.browser.engine.launch-request/v1",
    "adapter": "linux-native-smoke",
    "engine": "cef",
    "url": "https://example.com/",
    "stream_id": "stream:native-supervisor-smoke",
    "target": "tls://example.com:443",
    "network_mode": "runtime_net_only",
    "direct_network": False,
    "wallet_injection": False,
    "adapter_ipc": {
        "schema": "elastos.adapter-ipc/v1",
        "kind": "unix_socket",
        "path": "$adapter_socket",
        "stream_id": "stream:native-supervisor-smoke",
        "runtime_stream_path": "$runtime_socket",
    },
    "display_mode": "native_surface",
    "guarantee_level": "policy_webview",
    "wallet": {},
    "viewport": {"width": 1280, "height": 720},
}))
PY
)"

config_json="$(python3 - <<PY
import json
print(json.dumps({
    "schema": "elastos.browser.engine.supervisor-config/v1",
    "adapter": "linux-native-smoke",
    "engine": "cef",
    "program": "$tmp_dir/native_probe.py",
    "args": [],
    "network_sandbox": "linux_new_netns",
    "startup_grace_ms": 1000,
    "stream_bridge": {
        "program": "$stream_bridge_bin",
        "args": [],
        "replace_existing_socket": True,
        "startup_wait_ms": 5000,
    },
}))
PY
)"

set +e
supervisor_output="$(
  ELASTOS_BROWSER_ENGINE_REQUEST="$request_json" \
  ELASTOS_BROWSER_ENGINE_SUPERVISOR_CONFIG="$config_json" \
  "$supervisor_bin" 2>"$tmp_dir/supervisor.err"
)"
supervisor_status=$?
set -e

if [[ "$supervisor_status" -ne 0 ]]; then
  if grep -qi "Operation not permitted" "$tmp_dir/supervisor.err"; then
    echo '{"skipped":true,"reason":"host does not permit CLONE_NEWNET for browser native supervisor proof"}'
    exit 0
  fi
  cat "$tmp_dir/supervisor.err" >&2 || true
  exit "$supervisor_status"
fi

SUPERVISOR_OUTPUT="$supervisor_output" PROOF_FILE="$proof_file" python3 - <<'PY'
import json
import os
import pathlib

result = json.loads(os.environ["SUPERVISOR_OUTPUT"])
proof = json.loads(pathlib.Path(os.environ["PROOF_FILE"]).read_text(encoding="utf-8"))

assert result["schema"] == "elastos.browser.engine.supervisor-result/v1"
assert result["network_mode"] == "runtime_net_only"
assert result["direct_network"] is False
assert result["wallet_injection"] is False
assert result["display_session"]["mode"] == "native_surface"
assert result["display_session"]["direct_network"] is False
assert result["display_session"]["audio"] is False
assert result["display_session"]["video"] is False
assert result["process"]["network_sandbox"] == "linux_new_netns"
assert proof["schema"] == "elastos.browser.native-supervisor-proof/v1"
assert proof["direct_tcp_blocked"] is True
assert proof["direct_dns_blocked"] is True
assert proof["direct_http_blocked"] is True
assert proof["stream_bridge_echo"] is True

print(json.dumps({
    "ok": True,
    "page_id": result["page_id"],
    "surface_id": result["display_session"]["surface_id"],
    "network_sandbox": result["process"]["network_sandbox"],
    "direct_tcp_blocked": proof["direct_tcp_blocked"],
    "direct_dns_blocked": proof["direct_dns_blocked"],
    "direct_http_blocked": proof["direct_http_blocked"],
    "native_audio_proven": result["display_session"]["audio"],
    "native_video_proven": result["display_session"]["video"],
    "stream_bridge_echo": proof["stream_bridge_echo"],
}))
PY
