#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

usage() {
  cat >&2 <<'USAGE'
Usage:
  scripts/mac-source-home-restart.sh [options]

Options:
  --test-home <path>      Source-home root. Default: $MAC_TEST_HOME or ~/elastos-mac-test-home.
  --addr <host:port>      Gateway bind address. Default: $MAC_GATEWAY_ADDR or localhost:61180.
  --log-dir <path>        Restart log directory. Default: <test-home>/logs.
  --dry-run               Print the restart plan without stopping or starting processes.
  --json-out <path>       Also write the restart receipt JSON to this path.
  --wait-seconds <n>      Seconds to wait for Home after start. Default: 40.

Restarts the local Mac source-home gateway and runtime-data provider binaries,
then verifies Home HTTP 200 and served/installed/source Home hash parity.
USAGE
}

sha256_file() {
  shasum -a 256 "$1" | awk '{print $1}'
}

find_debugfs() {
  if [[ -n "${ELASTOS_DEBUGFS_BIN:-}" && -x "${ELASTOS_DEBUGFS_BIN}" ]]; then
    printf '%s\n' "${ELASTOS_DEBUGFS_BIN}"
    return 0
  fi
  if command -v debugfs >/dev/null 2>&1; then
    command -v debugfs
    return 0
  fi
  for candidate in \
    /opt/homebrew/opt/e2fsprogs/sbin/debugfs \
    /usr/local/opt/e2fsprogs/sbin/debugfs \
    /usr/sbin/debugfs \
    /sbin/debugfs
  do
    if [[ -x "$candidate" ]]; then
      printf '%s\n' "$candidate"
      return 0
    fi
  done
  return 1
}

initrd_browser_helper_sha256() {
  local initrd="$1"
  local tmp_dir
  tmp_dir="$(mktemp -d)"
  gzip -dc "$initrd" | (cd "$tmp_dir" && cpio -id --quiet bin/browser-selkies-control-service.mjs)
  sha256_file "$tmp_dir/bin/browser-selkies-control-service.mjs"
  rm -rf "$tmp_dir"
}

rootfs_browser_helper_sha256() {
  local rootfs="$1"
  local debugfs="$2"
  local tmp_file
  tmp_file="$(mktemp)"
  "$debugfs" -R "cat /opt/elastos/bin/browser-selkies-control-service.mjs" "$rootfs" >"$tmp_file" 2>/dev/null
  sha256_file "$tmp_file"
  rm -f "$tmp_file"
}

verify_browser_helper_freshness() {
  local source="${repo_root}/scripts/browser-selkies-control-service.mjs"
  local installed="${data_dir}/scripts/browser-selkies-control-service.mjs"
  local initrd="${data_dir}/bin/initrd"
  local rootfs="${data_dir}/browser-vm/rootfs.ext4"
  local debugfs

  if [[ ! -f "$source" || ! -f "$installed" || ! -f "$initrd" || ! -f "$rootfs" ]]; then
    echo "Mac source-home Browser helper verification failed: installed helper, VM initrd, or VM rootfs is missing." >&2
    echo "Run scripts/setup-source-home.sh for ${test_home} before restarting for Browser VM proof." >&2
    exit 1
  fi
  if ! command -v gzip >/dev/null 2>&1 || ! command -v cpio >/dev/null 2>&1; then
    echo "Mac source-home Browser helper verification failed: gzip and cpio are required to inspect the VM initrd." >&2
    exit 1
  fi
  debugfs="$(find_debugfs || true)"
  if [[ -z "$debugfs" ]]; then
    echo "Mac source-home Browser helper verification failed: debugfs is required to inspect the VM rootfs." >&2
    exit 1
  fi

  browser_helper_source_sha="$(sha256_file "$source")"
  browser_helper_installed_sha="$(sha256_file "$installed")"
  browser_helper_initrd_sha="$(initrd_browser_helper_sha256 "$initrd")"
  browser_helper_rootfs_sha="$(rootfs_browser_helper_sha256 "$rootfs" "$debugfs")"
  if [[ "$browser_helper_source_sha" != "$browser_helper_installed_sha" ||
        "$browser_helper_source_sha" != "$browser_helper_initrd_sha" ||
        "$browser_helper_source_sha" != "$browser_helper_rootfs_sha" ]]; then
    echo "Mac source-home Browser helper verification failed" >&2
    echo "source=${browser_helper_source_sha}" >&2
    echo "installed=${browser_helper_installed_sha}" >&2
    echo "initrd=${browser_helper_initrd_sha}" >&2
    echo "rootfs=${browser_helper_rootfs_sha}" >&2
    echo "Run scripts/setup-source-home.sh for ${test_home}, then rerun this restart." >&2
    exit 1
  fi
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
    "test_home",
    "data_dir",
    "addr",
    "home_url",
    "gateway_bin",
    "gateway_log",
    "http_code",
    "served_index_sha256",
    "installed_index_sha256",
    "source_index_sha256",
    "browser_helper_source_sha256",
    "browser_helper_installed_sha256",
    "browser_helper_initrd_sha256",
    "browser_helper_rootfs_sha256",
    "home_cli_renderer_source_sha256",
    "home_cli_renderer_installed_sha256",
]
data = {}
for key in keys:
    value = os.environ.get(key.upper())
    if value is None:
        continue
    if key in {"ok", "dry_run"}:
        data[key] = value == "1"
    elif key == "http_code":
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

test_home="${MAC_TEST_HOME:-${HOME}/elastos-mac-test-home}"
addr="${MAC_GATEWAY_ADDR:-localhost:61180}"
log_dir=""
wait_seconds=40
dry_run=0
json_out=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --test-home)
      test_home="${2:-}"
      if [[ -z "$test_home" ]]; then
        echo "--test-home requires a path" >&2
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
    --log-dir)
      log_dir="${2:-}"
      if [[ -z "$log_dir" ]]; then
        echo "--log-dir requires a path" >&2
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

data_dir="${test_home}/Library/Application Support/elastos"
log_dir="${log_dir:-${test_home}/logs}"
gateway_bin="${repo_root}/elastos/target/release/elastos"
gateway_log="${log_dir}/gateway-$(date -u +%Y%m%dT%H%M%SZ).log"
home_url="http://${addr}/apps/home/"

if [[ "$dry_run" -eq 1 ]]; then
  SCHEMA="elastos.mac-source-home-restart/v1" \
  OK=1 \
  DRY_RUN=1 \
  GENERATED_AT="$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
  REPO="$repo_root" \
  TEST_HOME="$test_home" \
  DATA_DIR="$data_dir" \
  ADDR="$addr" \
  HOME_URL="$home_url" \
  GATEWAY_BIN="$gateway_bin" \
  GATEWAY_LOG="$gateway_log" \
  emit_json "$json_out"
  exit 0
fi

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "mac-source-home-restart is only for local macOS source-home staging" >&2
  exit 2
fi
if [[ ! -x "$gateway_bin" ]]; then
  echo "gateway binary is not executable: $gateway_bin" >&2
  exit 2
fi
browser_helper_source_sha=""
browser_helper_installed_sha=""
browser_helper_initrd_sha=""
browser_helper_rootfs_sha=""
home_cli_renderer_source_sha=""
home_cli_renderer_installed_sha=""
verify_browser_helper_freshness

mkdir -p "$log_dir"

pkill -TERM -f "${gateway_bin} gateway --addr ${addr}" 2>/dev/null || true
pkill -TERM -f "${gateway_bin} serve --addr" 2>/dev/null || true
pkill -TERM -f "${gateway_bin} home$" 2>/dev/null || true
pkill -TERM -f "${data_dir}/bin/" 2>/dev/null || true
sleep 2
pkill -KILL -f "${gateway_bin} home$" 2>/dev/null || true

port="${addr##*:}"
for pid in $(lsof -tiTCP:"$port" -sTCP:LISTEN 2>/dev/null || true); do
  kill -KILL "$pid" 2>/dev/null || true
done

python3 - "$repo_root" "$test_home" "$addr" "$gateway_log" <<'PY'
import os
import sys

repo, home, addr, log_path = sys.argv[1:]

if os.fork() != 0:
    raise SystemExit(0)

os.setsid()

if os.fork() != 0:
    os._exit(0)

os.chdir(os.path.join(repo, "elastos"))
os.environ["HOME"] = home

stdin = os.open(os.devnull, os.O_RDONLY)
stdout = os.open(log_path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o644)
os.dup2(stdin, 0)
os.dup2(stdout, 1)
os.dup2(stdout, 2)
os.close(stdin)
os.close(stdout)

os.execv(
    "./target/release/elastos",
    ["./target/release/elastos", "gateway", "--addr", addr],
)
PY

for _ in $(seq 1 "$wait_seconds"); do
  if curl -fsS -o /dev/null "$home_url"; then
    break
  fi
  sleep 1
done

http_code="$(curl -fsS -o /dev/null -w '%{http_code}' "$home_url")"
served_hash="$(curl -fsS "$home_url" | shasum -a 256 | cut -d ' ' -f 1)"
installed_hash="$(shasum -a 256 "${data_dir}/capsules/home/browser/index.html" | cut -d ' ' -f 1)"
source_hash="$(shasum -a 256 "${repo_root}/capsules/home/browser/index.html" | cut -d ' ' -f 1)"
home_cli_renderer_source="${repo_root}/capsules/home-cli/target/release/home-cli"
home_cli_renderer_installed="${data_dir}/bin/home-cli"

if [[ ! -f "$home_cli_renderer_source" || ! -f "$home_cli_renderer_installed" ]]; then
  echo "Mac source-home Home CLI renderer verification failed: source or installed renderer is missing." >&2
  echo "Run scripts/setup-source-home.sh for ${test_home}, then rerun this restart." >&2
  exit 1
fi
home_cli_renderer_source_sha="$(sha256_file "$home_cli_renderer_source")"
home_cli_renderer_installed_sha="$(sha256_file "$home_cli_renderer_installed")"

if [[ "$http_code" != "200" || "$served_hash" != "$installed_hash" || "$served_hash" != "$source_hash" ]]; then
  echo "Mac source-home restart verification failed" >&2
  echo "http=${http_code}" >&2
  echo "served=${served_hash}" >&2
  echo "installed=${installed_hash}" >&2
  echo "source=${source_hash}" >&2
  exit 1
fi
if [[ "$home_cli_renderer_source_sha" != "$home_cli_renderer_installed_sha" ]]; then
  echo "Mac source-home Home CLI renderer verification failed" >&2
  echo "source=${home_cli_renderer_source_sha}" >&2
  echo "installed=${home_cli_renderer_installed_sha}" >&2
  echo "Run scripts/setup-source-home.sh for ${test_home}, then rerun this restart." >&2
  exit 1
fi

SCHEMA="elastos.mac-source-home-restart/v1" \
OK=1 \
DRY_RUN=0 \
GENERATED_AT="$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
REPO="$repo_root" \
TEST_HOME="$test_home" \
DATA_DIR="$data_dir" \
ADDR="$addr" \
HOME_URL="$home_url" \
GATEWAY_BIN="$gateway_bin" \
GATEWAY_LOG="$gateway_log" \
HTTP_CODE="$http_code" \
SERVED_INDEX_SHA256="$served_hash" \
INSTALLED_INDEX_SHA256="$installed_hash" \
SOURCE_INDEX_SHA256="$source_hash" \
BROWSER_HELPER_SOURCE_SHA256="$browser_helper_source_sha" \
BROWSER_HELPER_INSTALLED_SHA256="$browser_helper_installed_sha" \
BROWSER_HELPER_INITRD_SHA256="$browser_helper_initrd_sha" \
BROWSER_HELPER_ROOTFS_SHA256="$browser_helper_rootfs_sha" \
HOME_CLI_RENDERER_SOURCE_SHA256="$home_cli_renderer_source_sha" \
HOME_CLI_RENDERER_INSTALLED_SHA256="$home_cli_renderer_installed_sha" \
emit_json "$json_out"
