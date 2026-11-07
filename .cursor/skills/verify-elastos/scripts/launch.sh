#!/usr/bin/env bash
# Start one isolated source-built ElastOS operator runtime for verification.
set -euo pipefail
umask 077

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/../../../.." && pwd)"
RUN_ID="${1:-$(date -u +%Y%m%dT%H%M%SZ)-$$}"
STATE_ROOT="${VERIFY_ELASTOS_STATE_ROOT:-$HOME/.local/share/elastos-verify-runs}"
STATE="$STATE_ROOT/$RUN_ID"
EVIDENCE="$ROOT/artifacts/verify-elastos/$RUN_ID"
BIN="$ROOT/elastos/target/release/elastos"
PROV="$ROOT/elastos/target/release/localhost-provider"

if [[ ! -x "$BIN" || ! -f "$PROV" ]]; then
  echo "missing release binary. From the repo root run: just build" >&2
  exit 1
fi

mkdir -p "$STATE" "$EVIDENCE"
XDG_DATA_HOME="$STATE/xdg"
DATA="$XDG_DATA_HOME/elastos"
mkdir -p "$DATA/bin" "$STATE/work"
install -m 0755 "$PROV" "$DATA/bin/localhost-provider"
chmod 0700 "$STATE" "$XDG_DATA_HOME" "$DATA" "$DATA/bin" "$STATE/work"

python3 - "$DATA" <<'PY'
import hashlib, json, os, pathlib, sys
data = pathlib.Path(sys.argv[1])
binary = data / "bin" / "localhost-provider"
digest = hashlib.sha256(binary.read_bytes()).hexdigest()
manifest = {
    "external": {
        "localhost-provider": {
            "install_path": "bin/localhost-provider",
            "description": "Host localhost provider binary required for runtime storage access",
            "provider_runtime": {
                "role": "provider",
                "substrate": "native",
                "runtime_abi": "elastos.provider-stdio/v1",
                "execution": "native-provider",
                "provides": "localhost://*",
            },
            "platforms": {
                "linux-amd64": {
                    "checksum": f"sha256:{digest}",
                    "install_path": "bin/localhost-provider",
                }
            },
        }
    },
    "capsules": {},
    "profiles": {},
}
path = data / "components.json"
path.write_text(json.dumps(manifest, indent=2) + "\n")
os.chmod(path, 0o600)
PY

PORT="$(python3 - <<'PY'
import socket
s = socket.socket()
s.bind(("127.0.0.1", 0))
print(s.getsockname()[1])
s.close()
PY
)"

export XDG_DATA_HOME
export ELASTOS_COLLABORATION_STARTUP_MODE=isolated
nohup "$BIN" serve --addr "127.0.0.1:$PORT" >"$EVIDENCE/serve.log" 2>&1 &
PID=$!

python3 - "$PORT" "$PID" <<'PY'
import os, sys, time, urllib.request
port, pid = sys.argv[1], int(sys.argv[2])
url = f"http://127.0.0.1:{port}/api/health"
deadline = time.time() + 30
while time.time() < deadline:
    try:
        with urllib.request.urlopen(url, timeout=1) as resp:
            if resp.status == 200 and b'"status":"ok"' in resp.read():
                sys.exit(0)
    except Exception:
        pass
    try:
        os.kill(pid, 0)
    except OSError:
        sys.stderr.write("runtime exited before health\n")
        sys.exit(1)
    time.sleep(0.2)
sys.stderr.write("runtime health timed out\n")
sys.exit(1)
PY

cat >"$STATE/run.env" <<EOF
RUN_ID=$RUN_ID
ROOT=$ROOT
BIN=$BIN
PID=$PID
PORT=$PORT
STATE=$STATE
XDG_DATA_HOME=$XDG_DATA_HOME
DATA=$DATA
EVIDENCE=$EVIDENCE
EOF
cp "$STATE/run.env" "$EVIDENCE/run.env"
echo "$STATE/run.env"
