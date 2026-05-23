#!/usr/bin/env bash
# Runtime cleanup helpers for the smoke scripts.
#
# **Phase 5 Day 2** — the original implementation used
# `/proc/<pid>` for liveness checks (Linux-only). On macOS
# `/proc` does not exist, so every kill issued through this
# helper was a silent no-op. The fix is to source the
# Phase-5 Day-1 cross-platform helper and use its
# `pid_is_running` + `kill_pid_then_group` primitives.

# Idempotent guard: callers may source us from a context that
# has already sourced cross-platform.sh.
if ! declare -f pid_is_running >/dev/null 2>&1; then
    # Resolve our own directory and source the sibling file.
    # BASH_SOURCE[0] is the path to THIS file even when sourced.
    _RUNTIME_CLEANUP_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
    # shellcheck source=cross-platform.sh
    . "${_RUNTIME_CLEANUP_DIR}/cross-platform.sh"
    unset _RUNTIME_CLEANUP_DIR
fi

stop_runtime_from_coords() {
    local coords_path="$1"
    local pid=""

    [[ -f "$coords_path" ]] || return 0

    pid="$(python3 - "$coords_path" <<'PY2'
import json
import pathlib
import sys

path = pathlib.Path(sys.argv[1])
if not path.is_file():
    raise SystemExit(0)

try:
    data = json.loads(path.read_text())
except Exception:
    raise SystemExit(0)

pid = data.get("pid", "")
if pid:
    print(pid)
PY2
    )"

    if [[ -z "$pid" ]]; then
        rm -f "$coords_path"
        return 0
    fi

    if ! pid_is_running "$pid"; then
        rm -f "$coords_path"
        return 0
    fi

    kill_pid_then_group "${pid}" 2
    rm -f "$coords_path"
}

cleanup_elastos_runtime_home() {
    local home_dir="$1"
    local data_dir="${2:-${home_dir}/xdg-data/elastos}"

    stop_runtime_from_coords "${data_dir}/home-runtime-coords.json"
    stop_runtime_from_coords "${data_dir}/runtime-coords.json"
}
