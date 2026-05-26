#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
tmp_dir="$(mktemp -d)"

cleanup() {
  rm -rf "$tmp_dir"
}
trap cleanup EXIT

cd "$repo_root"

cargo build --quiet --manifest-path elastos/tools/browser-native-proxy-engine/Cargo.toml

engine_bin="$repo_root/elastos/tools/browser-native-proxy-engine/target/debug/browser-native-proxy-engine"
relay_socket="$tmp_dir/relay.sock"
proof_file="$tmp_dir/proof.json"
relay_file="$tmp_dir/relay.json"

cat >"$tmp_dir/fake_relay.py" <<'PY'
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
    line = b""
    while not line.endswith(b"\n"):
        line += conn.recv(1)
    request_head = b""
    while not request_head.endswith(b"\r\n\r\n"):
        chunk = conn.recv(1)
        if not chunk:
            break
        request_head += chunk
    conn.sendall(
        b"HTTP/1.1 200 OK\r\n"
        b"Content-Type: text/plain\r\n"
        b"Content-Length: 16\r\n"
        b"Connection: close\r\n\r\n"
        b"native-proxy-ok\n"
    )

proof = {
    "open": json.loads(line.decode("utf-8")),
    "request_head": request_head.decode("utf-8"),
}
with open(proof_path, "w", encoding="utf-8") as handle:
    json.dump(proof, handle)
server.close()
PY

cat >"$tmp_dir/fake_browser.py" <<'PY'
#!/usr/bin/env python3
import json
import os
import socket
import sys
import urllib.parse

proxy = urllib.parse.urlparse(os.environ["ELASTOS_BROWSER_PROXY_URL"])
proof_path = sys.argv[1]
sock = socket.create_connection((proxy.hostname, proxy.port), timeout=3)
with sock:
    sock.sendall(
        b"GET http://example.test/hello?x=1 HTTP/1.1\r\n"
        b"Host: example.test\r\n"
        b"Proxy-Connection: keep-alive\r\n"
        b"Connection: close\r\n\r\n"
    )
    response = b""
    while True:
        chunk = sock.recv(4096)
        if not chunk:
            break
        response += chunk
with open(proof_path, "w", encoding="utf-8") as handle:
    json.dump({"response": response.decode("utf-8")}, handle)
PY
chmod +x "$tmp_dir/fake_relay.py" "$tmp_dir/fake_browser.py"

"$tmp_dir/fake_relay.py" "$relay_socket" "$relay_file" >"$tmp_dir/relay.out" 2>"$tmp_dir/relay.err" &
relay_pid="$!"

for _ in {1..100}; do
  [[ -S "$relay_socket" ]] && break
  sleep 0.05
done
if [[ ! -S "$relay_socket" ]]; then
  cat "$tmp_dir/relay.err" >&2 || true
  exit 1
fi

config_json="$(python3 - <<PY
import json
print(json.dumps({
    "schema": "elastos.browser.native-proxy-engine.config/v1",
    "browser_program": "$tmp_dir/fake_browser.py",
    "browser_args": ["$proof_file"],
    "relay_ipc_path": "$relay_socket",
    "startup_grace_ms": 0,
}))
PY
)"

ELASTOS_BROWSER_NATIVE_PROXY_ENGINE_CONFIG="$config_json" \
ELASTOS_BROWSER_ENGINE_URL="https://example.test/hello?x=1" \
ELASTOS_BROWSER_ENGINE_STREAM_ID="stream:native-proxy-smoke" \
"$engine_bin" >"$tmp_dir/engine.out" 2>"$tmp_dir/engine.err"

wait "$relay_pid"

ENGINE_OUT="$tmp_dir/engine.out" PROOF_FILE="$proof_file" RELAY_FILE="$relay_file" python3 - <<'PY'
import json
import os
import pathlib

engine = json.loads(pathlib.Path(os.environ["ENGINE_OUT"]).read_text(encoding="utf-8").splitlines()[0])
browser = json.loads(pathlib.Path(os.environ["PROOF_FILE"]).read_text(encoding="utf-8"))
relay = json.loads(pathlib.Path(os.environ["RELAY_FILE"]).read_text(encoding="utf-8"))

assert engine["schema"] == "elastos.browser.native-proxy-engine.ready/v1"
assert engine["network_mode"] == "runtime_net_only"
assert engine["direct_network"] is False
assert relay["open"]["schema"] == "elastos.exit.relay-open/v1"
assert relay["open"]["target"] == "tcp://example.test:80"
assert relay["open"]["scheme"] == "tcp"
assert "GET /hello?x=1 HTTP/1.1" in relay["request_head"]
assert "Proxy-Connection" not in relay["request_head"]
assert "native-proxy-ok" in browser["response"]

print(json.dumps({
    "ok": True,
    "proxy_url": engine["proxy_url"],
    "target": relay["open"]["target"],
    "response": "native-proxy-ok",
}))
PY
