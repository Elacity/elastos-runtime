#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

usage() {
  cat >&2 <<'USAGE'
Usage:
  scripts/linux-source-home-restart.sh [options]

Options:
  --home <path>           Source-home root. Default: $LINUX_SOURCE_HOME or $HOME.
  --xdg-data-home <path>  XDG data root. Default: $XDG_DATA_HOME or <home>/.local/share.
  --addr <host:port>      Gateway bind address. Default: $LINUX_GATEWAY_ADDR or 127.0.0.1:8090.
  --gateway-bin <path>    Gateway binary. Default: <repo>/elastos/target/release/elastos.
  --log-dir <path>        Restart log directory. Default: <xdg-data-home>/elastos/logs.
  --pid-file <path>       PID file. Default: <home>/run/gateway-<port>.pid.
  --dry-run               Print the restart plan without stopping or starting processes.
  --json-out <path>       Also write the restart receipt JSON to this path.
  --wait-seconds <n>      Seconds to wait for Home after start. Default: 40.

Restarts a Linux source-home gateway, then verifies Home and Services are served
from the installed assets that match this source checkout.
USAGE
}

sha256_file() {
  sha256sum "$1" | awk '{print $1}'
}

emit_json() {
  local out="$1"
  python3 - "$out" <<'PY'
import json
import os
import sys

keys = [
    "schema",
    "ok",
    "dry_run",
    "generated_at",
    "repo",
    "home",
    "xdg_data_home",
    "data_dir",
    "addr",
    "home_url",
    "services_url",
    "gateway_bin",
    "gateway_log",
    "pid_file",
    "gateway_pid",
    "error",
    "home_http_code",
    "services_http_code",
    "home_served_index_sha256",
    "home_installed_index_sha256",
    "home_source_index_sha256",
    "services_served_index_sha256",
    "services_installed_index_sha256",
    "services_source_index_sha256",
]
data = {}
for key in keys:
    value = os.environ.get(key.upper())
    if value is None:
        continue
    if key in {"ok", "dry_run"}:
        data[key] = value == "1"
    elif key in {"gateway_pid", "home_http_code", "services_http_code"}:
        data[key] = int(value) if value.isdigit() else value
    else:
        data[key] = value
json.dump(data, sys.stdout, indent=2)
sys.stdout.write("\n")
if sys.argv[1]:
    with open(sys.argv[1], "w", encoding="utf-8") as handle:
        json.dump(data, handle, indent=2)
        handle.write("\n")
PY
}

stop_pid_file_process() {
  local pid_file="$1"
  if [[ ! -f "$pid_file" ]]; then
    return
  fi
  local pid
  pid="$(tr -dc '0-9' <"$pid_file" || true)"
  if [[ -z "$pid" ]]; then
    return
  fi
  if kill -0 "$pid" 2>/dev/null; then
    kill -TERM "$pid" 2>/dev/null || true
  fi
}

kill_port_listeners() {
  local port="$1"
  local expected_gateway_bin="$2"
  local expected_addr="$3"
  local pids=""
  if command -v lsof >/dev/null 2>&1; then
    pids="$(lsof -tiTCP:"$port" -sTCP:LISTEN 2>/dev/null || true)"
  elif command -v fuser >/dev/null 2>&1; then
    pids="$(fuser -n tcp "$port" 2>/dev/null || true)"
  fi
  if [[ -z "$pids" ]]; then
    return
  fi
  local pid
  local refused=0
  for pid in $pids; do
    if process_matches_gateway_listener "$pid" "$expected_gateway_bin" "$expected_addr"; then
      kill -KILL "$pid" 2>/dev/null || true
    else
      echo "refusing to kill unrelated listener on TCP port ${port}: pid=${pid} command=$(process_command "$pid")" >&2
      refused=1
    fi
  done
  return "$refused"
}

process_command() {
  ps -p "$1" -o args= 2>/dev/null || true
}

process_matches_gateway_listener() {
  local pid="$1"
  local expected_gateway_bin="$2"
  local expected_addr="$3"
  local command_line
  command_line="$(process_command "$pid")"
  if [[ -z "$command_line" ]]; then
    return 1
  fi
  if [[ "$command_line" != *"$expected_gateway_bin"* || "$command_line" != *" gateway "* ]]; then
    return 1
  fi
  [[ "$command_line" == *"--addr ${expected_addr}"* || "$command_line" == *"--addr=${expected_addr}"* ]]
}

http_status() {
  local url="$1"
  local code
  code="$(curl -sS -o /dev/null -w '%{http_code}' "$url" 2>/dev/null || true)"
  if [[ "$code" =~ ^[0-9][0-9][0-9]$ ]]; then
    printf '%s\n' "$code"
  else
    printf '000\n'
  fi
}

http_index_hash() {
  local url="$1"
  local out="$2"
  if curl -sS "$url" -o "$out" 2>/dev/null; then
    sha256_file "$out"
  else
    printf 'unavailable\n'
  fi
}

source_home="${LINUX_SOURCE_HOME:-${HOME}}"
xdg_data_home="${XDG_DATA_HOME:-}"
addr="${LINUX_GATEWAY_ADDR:-127.0.0.1:8090}"
gateway_bin="${repo_root}/elastos/target/release/elastos"
log_dir=""
pid_file=""
wait_seconds=40
dry_run=0
json_out=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --home)
      source_home="${2:-}"
      if [[ -z "$source_home" ]]; then
        echo "--home requires a path" >&2
        exit 2
      fi
      shift 2
      ;;
    --xdg-data-home)
      xdg_data_home="${2:-}"
      if [[ -z "$xdg_data_home" ]]; then
        echo "--xdg-data-home requires a path" >&2
        exit 2
      fi
      shift 2
      ;;
    --addr)
      addr="${2:-}"
      if [[ -z "$addr" ]]; then
        echo "--addr requires host:port" >&2
        exit 2
      fi
      shift 2
      ;;
    --gateway-bin)
      gateway_bin="${2:-}"
      if [[ -z "$gateway_bin" ]]; then
        echo "--gateway-bin requires a path" >&2
        exit 2
      fi
      shift 2
      ;;
    --log-dir)
      log_dir="${2:-}"
      if [[ -z "$log_dir" ]]; then
        echo "--log-dir requires a path" >&2
        exit 2
      fi
      shift 2
      ;;
    --pid-file)
      pid_file="${2:-}"
      if [[ -z "$pid_file" ]]; then
        echo "--pid-file requires a path" >&2
        exit 2
      fi
      shift 2
      ;;
    --wait-seconds)
      wait_seconds="${2:-}"
      if [[ -z "$wait_seconds" ]]; then
        echo "--wait-seconds requires a value" >&2
        exit 2
      fi
      shift 2
      ;;
    --dry-run)
      dry_run=1
      shift
      ;;
    --json-out)
      json_out="${2:-}"
      if [[ -z "$json_out" ]]; then
        echo "--json-out requires a path" >&2
        exit 2
      fi
      shift 2
      ;;
    --help|-h)
      usage
      exit 0
      ;;
    *)
      echo "unknown argument: $1" >&2
      usage
      exit 2
      ;;
  esac
done

if [[ ! "$addr" =~ ^[^[:space:]:]+:[0-9]+$ ]]; then
  echo "--addr must be host:port" >&2
  exit 2
fi
if [[ ! "$wait_seconds" =~ ^[0-9]+$ || "$wait_seconds" -lt 1 || "$wait_seconds" -gt 300 ]]; then
  echo "--wait-seconds must be an integer between 1 and 300" >&2
  exit 2
fi

if [[ -z "$xdg_data_home" ]]; then
  xdg_data_home="${source_home}/.local/share"
fi
data_dir="${xdg_data_home}/elastos"
port="${addr##*:}"
bind_host="${addr%:*}"
probe_host="$bind_host"
if [[ "$probe_host" == "0.0.0.0" || "$probe_host" == "*" ]]; then
  probe_host="127.0.0.1"
fi
home_url="http://${probe_host}:${port}/apps/home/"
services_url="http://${probe_host}:${port}/apps/services/"
log_dir="${log_dir:-${data_dir}/logs}"
pid_file="${pid_file:-${source_home}/run/gateway-${port}.pid}"
gateway_log="${log_dir}/gateway-${port}-$(date -u +%Y%m%dT%H%M%SZ).log"

if [[ "$dry_run" -eq 1 ]]; then
  SCHEMA="elastos.linux-source-home-restart/v1" \
  OK=1 \
  DRY_RUN=1 \
  GENERATED_AT="$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
  REPO="$repo_root" \
  HOME="$source_home" \
  XDG_DATA_HOME="$xdg_data_home" \
  DATA_DIR="$data_dir" \
  ADDR="$addr" \
  HOME_URL="$home_url" \
  SERVICES_URL="$services_url" \
  GATEWAY_BIN="$gateway_bin" \
  GATEWAY_LOG="$gateway_log" \
  PID_FILE="$pid_file" \
  emit_json "$json_out"
  exit 0
fi

if [[ "$(uname -s)" != "Linux" ]]; then
  echo "linux-source-home-restart is only for Linux source-home targets" >&2
  exit 2
fi
if [[ ! -x "$gateway_bin" ]]; then
  echo "gateway binary is not executable: $gateway_bin" >&2
  exit 2
fi

home_source_index="${repo_root}/capsules/home/browser/index.html"
home_installed_index="${data_dir}/capsules/home/browser/index.html"
services_source_index="${repo_root}/capsules/services/browser/index.html"
services_installed_index="${data_dir}/capsules/services/browser/index.html"
for path in "$home_source_index" "$home_installed_index" "$services_source_index" "$services_installed_index"; do
  if [[ ! -f "$path" ]]; then
    echo "required source-home asset is missing: $path" >&2
    exit 2
  fi
done

mkdir -p "$log_dir" "$(dirname "$pid_file")"

stop_pid_file_process "$pid_file"
pkill -TERM -f "${gateway_bin} gateway --addr ${addr}" 2>/dev/null || true
sleep 2
kill_port_listeners "$port" "$gateway_bin" "$addr"

python3 - "$source_home" "$xdg_data_home" "$gateway_bin" "$addr" "$gateway_log" "$pid_file" <<'PY'
import os
import pathlib
import sys

home, xdg_data_home, gateway_bin, addr, log_path, pid_file = sys.argv[1:]

pid = os.fork()
if pid != 0:
    pathlib.Path(pid_file).write_text(f"{pid}\n")
    raise SystemExit(0)

os.setsid()
os.environ["HOME"] = home
os.environ["XDG_DATA_HOME"] = xdg_data_home

stdin = os.open(os.devnull, os.O_RDONLY)
stdout = os.open(log_path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o644)
os.dup2(stdin, 0)
os.dup2(stdout, 1)
os.dup2(stdout, 2)
os.close(stdin)
os.close(stdout)

os.execv(gateway_bin, [gateway_bin, "gateway", "--addr", addr])
PY

gateway_pid="$(tr -dc '0-9' <"$pid_file" || true)"

for _ in $(seq 1 "$wait_seconds"); do
  if curl -fsS -o /dev/null "$home_url"; then
    break
  fi
  sleep 1
done

tmp_dir="$(mktemp -d)"
cleanup() {
  rm -rf "$tmp_dir"
}
trap cleanup EXIT

home_http_code="$(http_status "$home_url")"
services_http_code="$(http_status "$services_url")"
home_served_hash="$(http_index_hash "$home_url" "$tmp_dir/home.index.html")"
services_served_hash="$(http_index_hash "$services_url" "$tmp_dir/services.index.html")"
home_installed_hash="$(sha256_file "$home_installed_index")"
home_source_hash="$(sha256_file "$home_source_index")"
services_installed_hash="$(sha256_file "$services_installed_index")"
services_source_hash="$(sha256_file "$services_source_index")"

if [[ "$home_http_code" != "200" ||
      "$services_http_code" != "200" ||
      "$home_served_hash" != "$home_installed_hash" ||
      "$home_served_hash" != "$home_source_hash" ||
      "$services_served_hash" != "$services_installed_hash" ||
      "$services_served_hash" != "$services_source_hash" ]]; then
  verification_error="Linux source-home restart verification failed"
  echo "Linux source-home restart verification failed" >&2
  echo "home_http=${home_http_code}" >&2
  echo "services_http=${services_http_code}" >&2
  echo "home_served=${home_served_hash}" >&2
  echo "home_installed=${home_installed_hash}" >&2
  echo "home_source=${home_source_hash}" >&2
  echo "services_served=${services_served_hash}" >&2
  echo "services_installed=${services_installed_hash}" >&2
  echo "services_source=${services_source_hash}" >&2
  SCHEMA="elastos.linux-source-home-restart/v1" \
  OK=0 \
  DRY_RUN=0 \
  GENERATED_AT="$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
  REPO="$repo_root" \
  HOME="$source_home" \
  XDG_DATA_HOME="$xdg_data_home" \
  DATA_DIR="$data_dir" \
  ADDR="$addr" \
  HOME_URL="$home_url" \
  SERVICES_URL="$services_url" \
  GATEWAY_BIN="$gateway_bin" \
  GATEWAY_LOG="$gateway_log" \
  PID_FILE="$pid_file" \
  GATEWAY_PID="$gateway_pid" \
  ERROR="$verification_error" \
  HOME_HTTP_CODE="$home_http_code" \
  SERVICES_HTTP_CODE="$services_http_code" \
  HOME_SERVED_INDEX_SHA256="$home_served_hash" \
  HOME_INSTALLED_INDEX_SHA256="$home_installed_hash" \
  HOME_SOURCE_INDEX_SHA256="$home_source_hash" \
  SERVICES_SERVED_INDEX_SHA256="$services_served_hash" \
  SERVICES_INSTALLED_INDEX_SHA256="$services_installed_hash" \
  SERVICES_SOURCE_INDEX_SHA256="$services_source_hash" \
  emit_json "$json_out"
  exit 1
fi

SCHEMA="elastos.linux-source-home-restart/v1" \
OK=1 \
DRY_RUN=0 \
GENERATED_AT="$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
REPO="$repo_root" \
HOME="$source_home" \
XDG_DATA_HOME="$xdg_data_home" \
DATA_DIR="$data_dir" \
ADDR="$addr" \
HOME_URL="$home_url" \
SERVICES_URL="$services_url" \
GATEWAY_BIN="$gateway_bin" \
GATEWAY_LOG="$gateway_log" \
PID_FILE="$pid_file" \
GATEWAY_PID="$gateway_pid" \
HOME_HTTP_CODE="$home_http_code" \
SERVICES_HTTP_CODE="$services_http_code" \
HOME_SERVED_INDEX_SHA256="$home_served_hash" \
HOME_INSTALLED_INDEX_SHA256="$home_installed_hash" \
HOME_SOURCE_INDEX_SHA256="$home_source_hash" \
SERVICES_SERVED_INDEX_SHA256="$services_served_hash" \
SERVICES_INSTALLED_INDEX_SHA256="$services_installed_hash" \
SERVICES_SOURCE_INDEX_SHA256="$services_source_hash" \
emit_json "$json_out"
