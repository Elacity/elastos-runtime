#!/usr/bin/env bash
# Save a Public file through a shell attach session and read it back.
set -euo pipefail
umask 077

ENV_FILE="${1:-}"
if [[ -z "$ENV_FILE" || ! -f "$ENV_FILE" ]]; then
  echo "usage: drive-localhost-file.sh /path/to/run.env" >&2
  exit 2
fi
# shellcheck disable=SC1090
source "$ENV_FILE"

PAYLOAD='Release checklist'
OUT="$EVIDENCE/localhost-file.txt"

python3 - "$PORT" "$DATA" "$OUT" "$PAYLOAD" <<'PY'
import json, pathlib, sys, urllib.error, urllib.request

port, data, out_path, payload = sys.argv[1:]
base = f"http://127.0.0.1:{port}"
coords = json.loads((pathlib.Path(data) / "runtime-coords.json").read_text())
secret = coords["attach_secret"]

def request(method, path, body=None, headers=None):
    req = urllib.request.Request(base + path, data=body, headers=headers or {}, method=method)
    try:
        with urllib.request.urlopen(req, timeout=5) as resp:
            return resp.status, resp.read()
    except urllib.error.HTTPError as err:
        return err.code, err.read()

def attach(scope):
    status, raw = request(
        "POST",
        "/api/auth/attach",
        json.dumps({"secret": secret, "scope": scope}).encode(),
        {"Content-Type": "application/json"},
    )
    if status != 200:
        raise SystemExit(f"attach {scope} status {status}")
    body = json.loads(raw)
    return body["token"], body["session_type"]

shell_token, shell_type = attach("shell")
client_token, client_type = attach("client")
if shell_type != "shell" or client_type != "capsule":
    raise SystemExit(f"unexpected session types shell={shell_type} client={client_type}")

path = "/api/localhost/Public/verify-note.txt"
shell_headers = {"Authorization": f"Bearer {shell_token}"}
client_headers = {"Authorization": f"Bearer {client_token}"}

denied, _ = request("PUT", path, payload.encode(), client_headers)
unauth, _ = request("PUT", path, payload.encode(), {})
written, write_raw = request("PUT", path, payload.encode(), shell_headers)
read_status, read_raw = request("GET", path, None, shell_headers)

disk = pathlib.Path(data) / "Public" / "verify-note.txt"
disk_bytes = disk.read_bytes() if disk.is_file() else b""

lines = [
    f"client_put={denied}",
    f"unauth_put={unauth}",
    f"shell_put={written}",
    f"shell_get={read_status}",
    f"write_body={write_raw.decode(errors='replace').strip()}",
    f"read_body={read_raw.decode(errors='replace')}",
    f"disk_path={disk}",
    f"disk_matches={disk_bytes.decode(errors='replace') == payload}",
]
pathlib.Path(out_path).write_text("\n".join(lines) + "\n")
print("\n".join(lines))

if denied != 403 or unauth != 401 or written != 200 or read_status != 200:
    raise SystemExit("localhost file statuses diverged")
if read_raw.decode() != payload or disk_bytes.decode() != payload:
    raise SystemExit("localhost file bytes diverged")
PY
