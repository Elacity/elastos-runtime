#!/usr/bin/env bash
set -euo pipefail

# Regression proof for the browser-local-exit orphan leak.
#
# The Runtime keeps browser-local-exit alive for the lifetime of the API server
# and stops it from `HostHelperProcess::drop`. Drop never runs when the server is
# SIGKILLed, panics under `panic = "abort"`, or leaves through `std::process::exit`
# (the installed-binary supersession watch does exactly that on every rebuild).
# Each of those paths used to strand a helper at PPID=1 holding an unlinked relay
# socket, so helpers accumulated one per dev-server launch.
#
# The helper now watches the inherited stdin pipe: when the launching Runtime
# disappears the pipe reaches EOF and the helper tears itself down. Teardown is
# scoped to the socket the helper actually bound, so a stranded helper can never
# unlink a successor's relay socket at the same path.

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# macOS Unix-domain socket paths are short. Keep relay paths under /tmp rather
# than the much longer per-user TMPDIR path.
tmp_dir="$(mktemp -d /tmp/elastos-local-exit.XXXXXX)"

cleanup() {
  local pid
  for pid in ${spawned_pids:-}; do
    kill -9 "$pid" 2>/dev/null || true
  done
  rm -rf "$tmp_dir"
}
trap cleanup EXIT

spawned_pids=""

cd "$repo_root"

cargo build --quiet --manifest-path elastos/tools/browser-local-exit/Cargo.toml

exit_bin="$repo_root/elastos/tools/browser-local-exit/target/debug/browser-local-exit"
[[ -x "$exit_bin" ]] || {
  echo "[local-exit-orphan] browser-local-exit binary was not built" >&2
  exit 1
}

# Mirrors `spawn_browser_local_exit`: own process group, piped stdin held open by
# the launcher, and the typed parent-EOF opt-in.
cat >"$tmp_dir/fake_runtime.py" <<'PY'
#!/usr/bin/env python3
"""Stand-in for the ElastOS API server that owns a browser-local-exit helper."""
import json
import os
import subprocess
import sys
import time

exit_bin, relay_path, pid_path, replace_existing = sys.argv[1:5]

os.setpgid(0, 0)

config = {
    "schema": "elastos.browser.local-exit.config/v1",
    "relay_ipc_path": relay_path,
    "allowed_hosts": ["*"],
    "allowed_ports": [443],
    "replace_existing_socket": replace_existing == "true",
}

child = subprocess.Popen(
    [exit_bin],
    stdin=subprocess.PIPE,
    stdout=subprocess.PIPE,
    stderr=subprocess.DEVNULL,
    env={
        **os.environ,
        "ELASTOS_BROWSER_LOCAL_EXIT_CONFIG": json.dumps(config),
        "ELASTOS_BROWSER_LOCAL_EXIT_PARENT_EOF": "1",
    },
)

# Same readiness handshake the Runtime uses: the helper announces itself only
# after it has bound the relay socket.
ready = json.loads(child.stdout.readline())
assert ready["schema"] == "elastos.browser.local-exit.ready/v1", ready

with open(pid_path, "w", encoding="utf-8") as handle:
    json.dump({"launcher": os.getpid(), "helper": child.pid}, handle)

time.sleep(600)
PY
chmod +x "$tmp_dir/fake_runtime.py"

python_bin="$(command -v python3)"

start_runtime() {
  local relay_path="$1" pid_path="$2" replace_existing="${3:-true}"
  "$python_bin" "$tmp_dir/fake_runtime.py" \
    "$exit_bin" "$relay_path" "$pid_path" "$replace_existing" \
    >/dev/null 2>&1 &
  spawned_pids="$spawned_pids $!"

  local _
  for _ in {1..200}; do
    [[ -s "$pid_path" ]] && [[ -S "$relay_path" ]] && return 0
    sleep 0.05
  done
  echo "[local-exit-orphan] helper never reported ready for $relay_path" >&2
  return 1
}

pid_from() {
  "$python_bin" -c "import json,sys;print(json.load(open(sys.argv[1]))[sys.argv[2]])" "$1" "$2"
}

pid_alive() {
  kill -0 "$1" 2>/dev/null
}

await_exit() {
  local pid="$1" label="$2" _
  for _ in {1..200}; do
    pid_alive "$pid" || return 0
    sleep 0.05
  done
  echo "[local-exit-orphan] $label (pid $pid) survived its launcher" >&2
  return 1
}

# ── Case 1: a hard-killed launcher must not strand its helper ─────────────────

relay_one="$tmp_dir/relay-one.sock"
start_runtime "$relay_one" "$tmp_dir/pids-one.json"
launcher_one="$(pid_from "$tmp_dir/pids-one.json" launcher)"
helper_one="$(pid_from "$tmp_dir/pids-one.json" helper)"

pid_alive "$helper_one" || {
  echo "[local-exit-orphan] helper was not running before the kill" >&2
  exit 1
}

kill -9 "$launcher_one"
await_exit "$helper_one" "helper"

[[ -e "$relay_one" ]] && {
  echo "[local-exit-orphan] relay socket $relay_one outlived the helper" >&2
  exit 1
}

echo "[local-exit-orphan] SIGKILLed launcher leaves no helper and no relay socket"

# ── Case 2: teardown is scoped to the socket the helper bound ─────────────────
#
# A successor helper replaces the relay socket at the same path while the first
# helper is still alive. Killing the first launcher must reap only that helper —
# never the successor's freshly bound socket.

relay_shared="$tmp_dir/relay-shared.sock"
start_runtime "$relay_shared" "$tmp_dir/pids-old.json"
launcher_old="$(pid_from "$tmp_dir/pids-old.json" launcher)"
helper_old="$(pid_from "$tmp_dir/pids-old.json" helper)"

start_runtime "$relay_shared" "$tmp_dir/pids-new.json"
launcher_new="$(pid_from "$tmp_dir/pids-new.json" launcher)"
helper_new="$(pid_from "$tmp_dir/pids-new.json" helper)"

pid_alive "$helper_new" || {
  echo "[local-exit-orphan] successor helper failed to start" >&2
  exit 1
}

kill -9 "$launcher_old"
await_exit "$helper_old" "superseded helper"

pid_alive "$helper_new" || {
  echo "[local-exit-orphan] successor helper died with the superseded launcher" >&2
  exit 1
}

[[ -S "$relay_shared" ]] || {
  echo "[local-exit-orphan] superseded helper unlinked the successor's relay socket" >&2
  exit 1
}

kill -9 "$launcher_new"
await_exit "$helper_new" "successor helper"

echo "[local-exit-orphan] superseded helper reaps itself without touching the successor socket"

# ── Case 3: standalone runs keep their existing lifetime ──────────────────────
#
# Operators and the other smoke scripts run the helper directly with stdin at
# /dev/null. Without the typed opt-in the helper must ignore stdin entirely,
# otherwise an immediate EOF would kill every standalone launch.

relay_standalone="$tmp_dir/relay-standalone.sock"
ELASTOS_BROWSER_LOCAL_EXIT_CONFIG="$(
  "$python_bin" -c 'import json,sys;print(json.dumps({
    "schema": "elastos.browser.local-exit.config/v1",
    "relay_ipc_path": sys.argv[1],
    "allowed_hosts": ["*"],
    "allowed_ports": [443],
  }))' "$relay_standalone"
)" "$exit_bin" </dev/null >/dev/null 2>&1 &
standalone_pid="$!"
spawned_pids="$spawned_pids $standalone_pid"

for _ in {1..200}; do
  [[ -S "$relay_standalone" ]] && break
  sleep 0.05
done

sleep 1

pid_alive "$standalone_pid" || {
  echo "[local-exit-orphan] standalone helper exited on /dev/null stdin" >&2
  exit 1
}

kill -9 "$standalone_pid"

echo "[local-exit-orphan] standalone helper is unaffected by the parent-EOF watch"
echo "[local-exit-orphan] OK"
