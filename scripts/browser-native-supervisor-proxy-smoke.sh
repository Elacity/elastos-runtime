#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
prebuilt=0
require_isolation=0
for arg in "$@"; do
  case "$arg" in
    --prebuilt) prebuilt=1 ;;
    --require-isolation) require_isolation=1 ;;
    *) echo "unknown argument: $arg" >&2; exit 2 ;;
  esac
done
tmp_dir="$(mktemp -d)"
relay_pid=""
controller_pid=""

cleanup() {
  if [[ -n "$controller_pid" ]]; then
    kill "$controller_pid" >/dev/null 2>&1 || true
    wait "$controller_pid" >/dev/null 2>&1 || true
  fi
  if [[ -n "$relay_pid" ]]; then
    kill "$relay_pid" >/dev/null 2>&1 || true
    wait "$relay_pid" >/dev/null 2>&1 || true
  fi
  rm -rf "$tmp_dir"
}
trap cleanup EXIT
trap 'exit 143' TERM
trap 'exit 130' INT

if [[ "$(uname -s)" != "Linux" ]]; then
  if [[ "$require_isolation" -eq 1 ]]; then
    echo "required native isolation proof needs Linux CLONE_NEWNET" >&2
    exit 1
  fi
  echo '{"skipped":true,"reason":"browser native supervisor proxy smoke requires Linux CLONE_NEWNET"}'
  exit 0
fi

cd "$repo_root"

if [[ "$prebuilt" -eq 0 ]]; then
  cargo build --quiet --manifest-path elastos/tools/browser-engine-supervisor/Cargo.toml
  cargo build --quiet --manifest-path elastos/tools/browser-native-proxy-engine/Cargo.toml
fi

supervisor_bin="$repo_root/elastos/tools/browser-engine-supervisor/target/debug/browser-engine-supervisor"
proxy_engine_bin="$repo_root/elastos/tools/browser-native-proxy-engine/target/debug/browser-native-proxy-engine"
relay_socket="$tmp_dir/relay.sock"
adapter_socket="$tmp_dir/adapter.sock"
browser_proof="$tmp_dir/browser-proof.json"
relay_proof="$tmp_dir/relay-proof.json"

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
server.settimeout(10)
server.bind(path)
server.listen(1)
conn, _ = server.accept()
conn.settimeout(10)
with conn:
    line = b""
    while not line.endswith(b"\n"):
        chunk = conn.recv(1)
        if not chunk:
            raise SystemExit("Runtime relay open request ended early")
        line += chunk
    request_head = b""
    while not request_head.endswith(b"\r\n\r\n"):
        chunk = conn.recv(1)
        if not chunk:
            break
        request_head += chunk
    conn.sendall(
        b"HTTP/1.1 200 OK\r\n"
        b"Content-Type: text/plain\r\n"
        b"Content-Length: 27\r\n"
        b"Connection: close\r\n\r\n"
        b"native-supervisor-proxy-ok\n"
    )

with open(proof_path, "w", encoding="utf-8") as handle:
    json.dump({
        "open": json.loads(line.decode("utf-8")),
        "request_head": request_head.decode("utf-8"),
    }, handle)
server.close()
PY

cat >"$tmp_dir/fake_browser.py" <<'PY'
#!/usr/bin/env python3
import json
import os
import pathlib
import socket
import sys
import urllib.parse

proof_path = sys.argv[1]
network_namespace = os.readlink("/proc/self/ns/net")
network_namespace_isolated = network_namespace != os.environ["SMOKE_HOST_NET_NAMESPACE"]

direct_tcp_blocked = False
direct_tcp_error = None
sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
sock.settimeout(0.75)
try:
    sock.connect(("1.1.1.1", 443))
except OSError as exc:
    direct_tcp_blocked = True
    direct_tcp_error = str(exc)
finally:
    sock.close()

direct_dns_blocked = False
direct_dns_error = None
try:
    socket.getaddrinfo("example.com", 80)
except OSError as exc:
    direct_dns_blocked = True
    direct_dns_error = str(exc)

proxy = urllib.parse.urlparse(os.environ["ELASTOS_BROWSER_PROXY_URL"])
# Inspect the kernel listener, so binding to 0.0.0.0 fails even when loopback connects.
proxy_listeners = []
for table in ("/proc/net/tcp", "/proc/net/tcp6"):
    for line in pathlib.Path(table).read_text().splitlines()[1:]:
        fields = line.split()
        address, port = fields[1].split(":")
        if int(port, 16) == proxy.port and fields[3] == "0A":
            proxy_listeners.append(address)
proxy_loopback_only = proxy.hostname == "127.0.0.1" and proxy_listeners == ["0100007F"]
proxy_response = b""
proxy_error = None
try:
    stream = socket.create_connection((proxy.hostname, proxy.port), timeout=3)
    with stream:
        stream.sendall(
            b"GET http://example.test/native-supervisor-proxy HTTP/1.1\r\n"
            b"Host: example.test\r\n"
            b"Proxy-Connection: keep-alive\r\n"
            b"Connection: close\r\n\r\n"
        )
        while True:
            chunk = stream.recv(4096)
            if not chunk:
                break
            proxy_response += chunk
except OSError as exc:
    proxy_error = str(exc)

with open(proof_path + ".tmp", "w", encoding="utf-8") as handle:
    json.dump({
        "network_namespace_isolated": network_namespace_isolated,
        "proxy_loopback_only": proxy_loopback_only,
        "direct_tcp_blocked": direct_tcp_blocked,
        "direct_tcp_error": direct_tcp_error,
        "direct_dns_blocked": direct_dns_blocked,
        "direct_dns_error": direct_dns_error,
        "proxy_response": proxy_response.decode("utf-8"),
        "proxy_error": proxy_error,
    }, handle)
os.replace(proof_path + ".tmp", proof_path)

if not network_namespace_isolated:
    raise SystemExit("native browser retained the host network namespace")
if not proxy_loopback_only:
    raise SystemExit("native proxy listener is outside IPv4 loopback")
if not direct_tcp_blocked:
    raise SystemExit("direct TCP unexpectedly succeeded inside native proxy engine namespace")
if not direct_dns_blocked:
    raise SystemExit("direct DNS unexpectedly succeeded inside native proxy engine namespace")
if b"native-supervisor-proxy-ok" not in proxy_response:
    raise SystemExit(f"Runtime Exit proxy path failed: {proxy_error}")
PY
chmod +x "$tmp_dir/fake_relay.py" "$tmp_dir/fake_browser.py"

"$tmp_dir/fake_relay.py" "$relay_socket" "$relay_proof" >"$tmp_dir/relay.out" 2>"$tmp_dir/relay.err" &
relay_pid="$!"

for _ in {1..100}; do
  [[ -S "$relay_socket" ]] && break
  sleep 0.05
done
if [[ ! -S "$relay_socket" ]]; then
  echo "fake Runtime Exit relay did not create $relay_socket" >&2
  cat "$tmp_dir/relay.err" >&2 || true
  exit 1
fi

native_config_json="$(python3 - <<PY
import json
print(json.dumps({
    "schema": "elastos.browser.native-proxy-engine.config/v1",
    "browser_program": "$tmp_dir/fake_browser.py",
    "browser_args": ["$browser_proof"],
    "startup_grace_ms": 0,
}))
PY
)"

request_json="$(python3 - <<PY
import json
print(json.dumps({
    "schema": "elastos.browser.engine.launch-request/v1",
    "adapter": "linux-native-proxy-smoke",
    "engine": "cef",
    "url": "https://example.test/native-supervisor-proxy",
    "stream_id": "stream:native-supervisor-proxy-smoke",
    "target": "tls://example.test:443",
    "network_mode": "runtime_net_only",
    "direct_network": False,
    "wallet_injection": False,
    "adapter_ipc": {
        "schema": "elastos.adapter-ipc/v1",
        "kind": "unix_socket",
        "path": "$adapter_socket",
        "stream_id": "stream:native-supervisor-proxy-smoke",
    },
    "relay_ipc": {
        "schema": "elastos.exit.relay-ipc/v1",
        "kind": "unix_socket",
        "path": "$relay_socket",
        "stream_id": "stream:native-supervisor-proxy-smoke",
    },
    "display_mode": "native_surface",
    "wallet": {},
    "viewport": {"width": 1280, "height": 720},
}))
PY
)"

config_json="$(NATIVE_CONFIG="$native_config_json" python3 - <<PY
import json
import os
print(json.dumps({
    "schema": "elastos.browser.engine.supervisor-config/v1",
    "adapter": "linux-native-proxy-smoke",
    "engine": "cef",
    "program": "$proxy_engine_bin",
    "args": [],
    "env": {
        "ELASTOS_BROWSER_NATIVE_PROXY_ENGINE_CONFIG": os.environ["NATIVE_CONFIG"],
        "SMOKE_HOST_NET_NAMESPACE": os.readlink("/proc/self/ns/net"),
    },
    "network_sandbox": "linux_new_netns",
    "startup_grace_ms": 1000,
}))
PY
)"

set +e
ELASTOS_BROWSER_ENGINE_REQUEST="$request_json" \
ELASTOS_BROWSER_ENGINE_SUPERVISOR_CONFIG="$config_json" \
python3 - "$supervisor_bin" "$tmp_dir/supervisor.out" "$tmp_dir/supervisor.err" "$browser_proof" <<'PY' &
import ctypes
import os
import pathlib
import signal
import subprocess
import sys
import time

# Adopt the fixture's grandchildren when the supervisor or proxy exits.
libc = ctypes.CDLL(None, use_errno=True)
if libc.prctl(36, 1, 0, 0, 0) != 0:  # PR_SET_CHILD_SUBREAPER
    raise OSError(ctypes.get_errno(), "native fixture child adoption failed")

def reap_children():
    while True:
        try:
            child, _ = os.waitpid(-1, os.WNOHANG)
        except ChildProcessError:
            return True
        if child == 0:
            return False

with open(sys.argv[2], "w") as output, open(sys.argv[3], "w") as errors:
    supervisor = subprocess.Popen([sys.argv[1]], stdout=output, stderr=errors, start_new_session=True)
    try:
        signal.signal(signal.SIGTERM, lambda *_: sys.exit(143))
        status = supervisor.wait(timeout=20)
        if status == 0:
            deadline = time.monotonic() + 10
            while not pathlib.Path(sys.argv[4]).exists():
                if time.monotonic() >= deadline:
                    raise SystemExit("native browser proof did not arrive")
                time.sleep(0.05)
    finally:
        # Complete cleanup if the shell forwards another termination signal.
        signal.signal(signal.SIGTERM, signal.SIG_IGN)
        signal.signal(signal.SIGINT, signal.SIG_IGN)
        # The successful supervisor hands its engine PID back and exits. The
        # fixture still owns that engine and every browser child in its session.
        for stop_signal in (signal.SIGTERM, signal.SIGKILL):
            try:
                os.killpg(supervisor.pid, stop_signal)
            except ProcessLookupError:
                pass
            try:
                supervisor.wait(timeout=2)
            except subprocess.TimeoutExpired:
                continue
            deadline = time.monotonic() + 2
            while not reap_children() and time.monotonic() < deadline:
                # Subreaping also owns children that started a new session.
                # Their parents may exit during this loop and expose more children.
                children = pathlib.Path(f"/proc/self/task/{os.getpid()}/children")
                for child in children.read_text().split():
                    try:
                        os.kill(int(child), stop_signal)
                    except ProcessLookupError:
                        pass
                time.sleep(0.05)
            if reap_children():
                break
        if not reap_children():
            raise SystemExit("native fixture child cleanup failed")
    raise SystemExit(status)
PY
controller_pid="$!"
wait "$controller_pid"
supervisor_status=$?
controller_pid=""
set -e
supervisor_output="$(cat "$tmp_dir/supervisor.out")"

if [[ "$supervisor_status" -ne 0 ]]; then
  if [[ "$require_isolation" -eq 0 ]] && grep -qi "Operation not permitted" "$tmp_dir/supervisor.err"; then
    echo '{"skipped":true,"reason":"host does not permit CLONE_NEWNET or loopback setup for browser native supervisor proxy proof"}'
    exit 0
  fi
  cat "$tmp_dir/supervisor.err" >&2 || true
  exit "$supervisor_status"
fi

wait "$relay_pid"
relay_pid=""

SUPERVISOR_OUTPUT="$supervisor_output" BROWSER_PROOF="$browser_proof" RELAY_PROOF="$relay_proof" python3 - <<'PY'
import json
import os
import pathlib

result = json.loads(os.environ["SUPERVISOR_OUTPUT"])
browser = json.loads(pathlib.Path(os.environ["BROWSER_PROOF"]).read_text(encoding="utf-8"))
relay = json.loads(pathlib.Path(os.environ["RELAY_PROOF"]).read_text(encoding="utf-8"))

assert result["schema"] == "elastos.browser.engine.supervisor-result/v1"
assert result["network_mode"] == "runtime_net_only"
assert result["direct_network"] is False
assert result["wallet_injection"] is False
assert result["display_session"]["mode"] == "native_surface"
assert result["display_session"]["direct_network"] is False
assert result["display_session"]["audio"] is False
assert result["display_session"]["video"] is False
assert result["process"]["network_sandbox"] == "linux_new_netns"
assert browser["network_namespace_isolated"] is True
assert browser["proxy_loopback_only"] is True
assert browser["direct_tcp_blocked"] is True
assert browser["direct_dns_blocked"] is True
assert "native-supervisor-proxy-ok" in browser["proxy_response"]
assert relay["open"]["schema"] == "elastos.exit.relay-open/v1"
assert relay["open"]["target"] == "tcp://example.test:80"
assert relay["open"]["scheme"] == "tcp"
assert "GET /native-supervisor-proxy HTTP/1.1" in relay["request_head"]
assert "Proxy-Connection" not in relay["request_head"]

print(json.dumps({
    "ok": True,
    "page_id": result["page_id"],
    "surface_id": result["display_session"]["surface_id"],
    "network_namespace_isolated": browser["network_namespace_isolated"],
    "proxy_loopback_only": browser["proxy_loopback_only"],
    "children_stopped": True,
    "direct_tcp_blocked": browser["direct_tcp_blocked"],
    "direct_dns_blocked": browser["direct_dns_blocked"],
    "native_audio_proven": result["display_session"]["audio"],
    "native_video_proven": result["display_session"]["video"],
    "relay_target": relay["open"]["target"],
}))
PY
