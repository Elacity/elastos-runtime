#!/usr/bin/env bash

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

    if [[ ! -d "/proc/${pid}" ]]; then
        rm -f "$coords_path"
        return 0
    fi

    kill "${pid}" 2>/dev/null || true
    for _ in $(seq 1 20); do
        [[ ! -d "/proc/${pid}" ]] && break
        sleep 0.1
    done
    if [[ -d "/proc/${pid}" ]]; then
        kill -KILL "${pid}" 2>/dev/null || true
    fi
    rm -f "$coords_path"
}

cleanup_elastos_runtime_home() {
    local home_dir="$1"
    local data_dir="${2:-${home_dir}/xdg-data/elastos}"

    stop_runtime_from_coords "${data_dir}/pc2-runtime-coords.json"
    stop_runtime_from_coords "${data_dir}/runtime-coords.json"
}
