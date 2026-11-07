#!/usr/bin/env bash
# Read-only check that a verification runtime is the one this run started.
set -euo pipefail

ENV_FILE="${1:-}"
if [[ -z "$ENV_FILE" || ! -f "$ENV_FILE" ]]; then
  echo "usage: doctor.sh /path/to/run.env" >&2
  exit 2
fi
# shellcheck disable=SC1090
source "$ENV_FILE"

if [[ ! -d "/proc/$PID" ]]; then
  echo "doctor: pid $PID is not running" >&2
  exit 1
fi
cmdline="$(tr '\0' ' ' <"/proc/$PID/cmdline")"
case "$cmdline" in
  "$BIN serve --addr 127.0.0.1:$PORT"*) ;;
  *)
    echo "doctor: pid $PID is not this run's runtime: $cmdline" >&2
    exit 1
    ;;
esac

python3 - "$PID" "$PORT" <<'PY'
import os, pathlib, sys
pid, port = sys.argv[1], int(sys.argv[2])
inodes = set()
for fd in pathlib.Path(f"/proc/{pid}/fd").iterdir():
    try:
        target = os.readlink(fd)
    except OSError:
        continue
    if target.startswith("socket:[") and target.endswith("]"):
        inodes.add(target[len("socket:[") : -1])
port_hex = f"{port:04X}"
owned = False
for name in ("/proc/net/tcp", "/proc/net/tcp6"):
    path = pathlib.Path(name)
    if not path.exists():
        continue
    for line in path.read_text().splitlines()[1:]:
        fields = line.split()
        local, inode = fields[1], fields[9]
        if local.rsplit(":", 1)[-1].upper() == port_hex and inode in inodes:
            owned = True
if not owned:
    sys.stderr.write(f"doctor: port {port} is not owned by pid {pid}\n")
    sys.exit(1)
PY

health="$(curl -fsS -m 5 "http://127.0.0.1:$PORT/api/health")"
printf '%s\n' "$health" | grep -q '"status":"ok"'
version="$(printf '%s\n' "$health" | python3 -c 'import json,sys; print(json.load(sys.stdin)["version"])')"
built="$("$BIN" --version)"
case "$built" in
  *" $version"|*"$version") ;;
  *)
    echo "doctor: health version $version does not match $built" >&2
    exit 1
    ;;
esac

echo "doctor ok pid=$PID port=$PORT version=$version data=$DATA"
