#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

tmp_dir="$(mktemp -d /tmp/elastos-linux-source-home-restart-smoke-XXXXXX)"
cleanup() {
  for pid_file in "$tmp_dir"/run-*/gateway.pid "$tmp_dir"/run/gateway.pid; do
    [[ -f "$pid_file" ]] || continue
    pid="$(tr -dc '0-9' <"$pid_file" || true)"
    if [[ -n "${pid:-}" ]]; then
      kill "$pid" >/dev/null 2>&1 || true
    fi
  done
  if [[ -n "${unrelated_listener_pid:-}" ]]; then
    kill "$unrelated_listener_pid" >/dev/null 2>&1 || true
  fi
  rm -rf "$tmp_dir"
}
trap cleanup EXIT

free_port() {
  python3 - <<'PY'
import socket
with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
    sock.bind(("127.0.0.1", 0))
    print(sock.getsockname()[1])
PY
}

prepare_installed_assets() {
  local xdg_data_home="$1"
  mkdir -p \
    "$xdg_data_home/elastos/capsules/home/browser" \
    "$xdg_data_home/elastos/capsules/services/browser"
  cp "$repo_root/capsules/home/browser/index.html" \
    "$xdg_data_home/elastos/capsules/home/browser/index.html"
  cp "$repo_root/capsules/services/browser/index.html" \
    "$xdg_data_home/elastos/capsules/services/browser/index.html"
}

dry_run_json="$tmp_dir/restart-dry-run.json"
"$repo_root/scripts/linux-source-home-restart.sh" \
  --dry-run \
  --home "$tmp_dir/source home" \
  --xdg-data-home "$tmp_dir/xdg data" \
  --addr 127.0.0.1:18090 \
  --gateway-bin "$repo_root/elastos/target/release/elastos" \
  --log-dir "$tmp_dir/logs" \
  --pid-file "$tmp_dir/run/gateway.pid" \
  --json-out "$dry_run_json" \
  >"$tmp_dir/stdout.json"

python3 - "$repo_root" "$dry_run_json" "$tmp_dir/stdout.json" "$tmp_dir/source home" "$tmp_dir/xdg data" <<'PY'
import json
import pathlib
import sys

repo, json_path, stdout_path, source_home, xdg_data_home = sys.argv[1:]
from_file = json.loads(pathlib.Path(json_path).read_text())
from_stdout = json.loads(pathlib.Path(stdout_path).read_text())
if from_file != from_stdout:
    raise SystemExit("dry-run stdout and --json-out differ")
if from_file.get("schema") != "elastos.linux-source-home-restart/v1":
    raise SystemExit("unexpected restart schema")
if from_file.get("ok") is not True or from_file.get("dry_run") is not True:
    raise SystemExit("dry-run restart plan must be ok=true and dry_run=true")
if not isinstance(from_file.get("generated_at"), str) or not from_file["generated_at"].endswith("Z"):
    raise SystemExit("dry-run restart plan generated_at mismatch")
if from_file.get("repo") != repo:
    raise SystemExit("restart plan repo mismatch")
if from_file.get("home") != source_home:
    raise SystemExit("restart plan home mismatch")
if from_file.get("xdg_data_home") != xdg_data_home:
    raise SystemExit("restart plan xdg_data_home mismatch")
if from_file.get("data_dir") != f"{xdg_data_home}/elastos":
    raise SystemExit("restart plan data_dir mismatch")
if from_file.get("home_url") != "http://127.0.0.1:18090/apps/home/":
    raise SystemExit("restart plan home URL mismatch")
if from_file.get("services_url") != "http://127.0.0.1:18090/apps/services/":
    raise SystemExit("restart plan Services URL mismatch")
if not from_file.get("gateway_log", "").startswith(f"{pathlib.Path(source_home).parent}/logs/gateway-18090-"):
    raise SystemExit("restart plan gateway log mismatch")
if from_file.get("pid_file") != f"{pathlib.Path(source_home).parent}/run/gateway.pid":
    raise SystemExit("restart plan pid file mismatch")
PY

python3 - "$repo_root/scripts/linux-source-home-restart.sh" <<'PY'
import pathlib
import sys

source = pathlib.Path(sys.argv[1]).read_text()
required = [
    "elastos.linux-source-home-restart/v1",
    "home_served_index_sha256",
    "services_served_index_sha256",
    "Linux source-home restart verification failed",
    "OK=0",
    "http_status",
    "process_matches_gateway_listener",
    "refusing to kill unrelated listener",
    "pkill -TERM -f",
    "kill_port_listeners",
]
missing = [needle for needle in required if needle not in source]
if missing:
    raise SystemExit(f"restart helper lost expected restart/proof gates: {missing}")
PY

set +e
"$repo_root/scripts/linux-source-home-restart.sh" --dry-run --addr not-a-host-port \
  >"$tmp_dir/invalid-addr.out" \
  2>"$tmp_dir/invalid-addr.err"
invalid_addr_status=$?
set -e
if [[ "$invalid_addr_status" -eq 0 ]]; then
  echo "linux-source-home-restart accepted an invalid address" >&2
  exit 1
fi

failing_gateway="$tmp_dir/failing-gateway"
cat >"$failing_gateway" <<'SH'
#!/usr/bin/env bash
exit 0
SH
chmod +x "$failing_gateway"

fail_port="$(free_port)"
fail_home="$tmp_dir/fail-home"
fail_xdg="$tmp_dir/fail-xdg"
fail_json="$tmp_dir/fail-receipt.json"
prepare_installed_assets "$fail_xdg"
set +e
"$repo_root/scripts/linux-source-home-restart.sh" \
  --home "$fail_home" \
  --xdg-data-home "$fail_xdg" \
  --addr "127.0.0.1:${fail_port}" \
  --gateway-bin "$failing_gateway" \
  --wait-seconds 1 \
  --log-dir "$tmp_dir/fail-logs" \
  --pid-file "$tmp_dir/run-fail/gateway.pid" \
  --json-out "$fail_json" \
  >"$tmp_dir/fail.out" \
  2>"$tmp_dir/fail.err"
fail_status=$?
set -e
if [[ "$fail_status" -eq 0 ]]; then
  echo "linux-source-home-restart succeeded even though the fake gateway never started" >&2
  exit 1
fi
python3 - "$fail_json" <<'PY'
import json
import pathlib
import sys

payload = json.loads(pathlib.Path(sys.argv[1]).read_text())
if payload.get("ok") is not False:
    raise SystemExit("failed restart receipt must set ok=false")
if payload.get("home_http_code") != 0:
    raise SystemExit(f"expected failed Home probe code 000, got {payload.get('home_http_code')!r}")
if payload.get("error") != "Linux source-home restart verification failed":
    raise SystemExit("failed restart receipt missing stable error")
PY

mismatch_gateway="$tmp_dir/mismatch-gateway"
cat >"$mismatch_gateway" <<'SH'
#!/usr/bin/env bash
addr="127.0.0.1:0"
while [[ $# -gt 0 ]]; do
  case "$1" in
    --addr)
      addr="${2:-$addr}"
      shift 2
      ;;
    *)
      shift
      ;;
  esac
done
port="${addr##*:}"
python3 - "$port" <<'PY'
from http.server import BaseHTTPRequestHandler, HTTPServer
import sys

port = int(sys.argv[1])

class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200)
        self.send_header("content-type", "text/html")
        self.end_headers()
        self.wfile.write(b"wrong source-home asset")

    def log_message(self, _format, *_args):
        pass

HTTPServer(("127.0.0.1", port), Handler).serve_forever()
PY
SH
chmod +x "$mismatch_gateway"

mismatch_port="$(free_port)"
mismatch_home="$tmp_dir/mismatch-home"
mismatch_xdg="$tmp_dir/mismatch-xdg"
mismatch_json="$tmp_dir/mismatch-receipt.json"
prepare_installed_assets "$mismatch_xdg"
set +e
"$repo_root/scripts/linux-source-home-restart.sh" \
  --home "$mismatch_home" \
  --xdg-data-home "$mismatch_xdg" \
  --addr "127.0.0.1:${mismatch_port}" \
  --gateway-bin "$mismatch_gateway" \
  --wait-seconds 3 \
  --log-dir "$tmp_dir/mismatch-logs" \
  --pid-file "$tmp_dir/run-mismatch/gateway.pid" \
  --json-out "$mismatch_json" \
  >"$tmp_dir/mismatch.out" \
  2>"$tmp_dir/mismatch.err"
mismatch_status=$?
set -e
if [[ "$mismatch_status" -eq 0 ]]; then
  echo "linux-source-home-restart succeeded with mismatched served assets" >&2
  exit 1
fi
python3 - "$mismatch_json" <<'PY'
import json
import pathlib
import sys

payload = json.loads(pathlib.Path(sys.argv[1]).read_text())
if payload.get("ok") is not False:
    raise SystemExit("mismatch receipt must set ok=false")
if payload.get("home_http_code") != 200 or payload.get("services_http_code") != 200:
    raise SystemExit("mismatch fixture should prove hash failure after HTTP 200")
if payload.get("home_served_index_sha256") == payload.get("home_source_index_sha256"):
    raise SystemExit("mismatch receipt did not capture served/source hash mismatch")
PY

unrelated_port="$(free_port)"
python3 - "$unrelated_port" <<'PY' &
from http.server import BaseHTTPRequestHandler, HTTPServer
import sys

port = int(sys.argv[1])

class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200)
        self.end_headers()
        self.wfile.write(b"unrelated service")

    def log_message(self, _format, *_args):
        pass

HTTPServer(("127.0.0.1", port), Handler).serve_forever()
PY
unrelated_listener_pid="$!"
for _ in {1..100}; do
  if curl -sS -o /dev/null "http://127.0.0.1:${unrelated_port}/" 2>/dev/null; then
    break
  fi
  sleep 0.05
done

unrelated_home="$tmp_dir/unrelated-home"
unrelated_xdg="$tmp_dir/unrelated-xdg"
prepare_installed_assets "$unrelated_xdg"
set +e
"$repo_root/scripts/linux-source-home-restart.sh" \
  --home "$unrelated_home" \
  --xdg-data-home "$unrelated_xdg" \
  --addr "127.0.0.1:${unrelated_port}" \
  --gateway-bin "$failing_gateway" \
  --wait-seconds 1 \
  --log-dir "$tmp_dir/unrelated-logs" \
  --pid-file "$tmp_dir/run-unrelated/gateway.pid" \
  >"$tmp_dir/unrelated.out" \
  2>"$tmp_dir/unrelated.err"
unrelated_status=$?
set -e
if [[ "$unrelated_status" -eq 0 ]]; then
  echo "linux-source-home-restart succeeded while an unrelated listener owned the port" >&2
  exit 1
fi
if ! kill -0 "$unrelated_listener_pid" 2>/dev/null; then
  echo "linux-source-home-restart killed an unrelated listener" >&2
  exit 1
fi
if ! grep -q "refusing to kill unrelated listener" "$tmp_dir/unrelated.err"; then
  echo "linux-source-home-restart did not explain unrelated listener refusal" >&2
  exit 1
fi

printf '{"schema":"elastos.linux-source-home-restart-smoke/v1","ok":true,"dry_run_contract":true,"hash_proof_gate_present":true,"invalid_addr_rejected":true,"live_failure_receipt":true,"hash_mismatch_receipt":true,"unrelated_listener_protected":true}\n'
