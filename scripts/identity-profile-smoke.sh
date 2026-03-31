#!/usr/bin/env bash
set -euo pipefail

ELASTOS_BIN="${ELASTOS_BIN:-$(command -v elastos || true)}"
HOME_DIR="${ELASTOS_IDENTITY_HOME:-${HOME:-}}"
XDG_DIR="${ELASTOS_IDENTITY_XDG_DATA_HOME:-${XDG_DATA_HOME:-${HOME_DIR}/xdg-data}}"
DATA_DIR="${ELASTOS_DATA_DIR:-${XDG_DIR}/elastos}"
PROFILE_NICK="${ELASTOS_IDENTITY_NICK:-anders-smoke}"
PERSONA_NAME="${ELASTOS_AGENT_PERSONA_NAME:-codex}"

usage() {
    cat <<'EOF'
Usage:
  ELASTOS_BIN=/path/to/elastos \
  ELASTOS_IDENTITY_HOME=/tmp/elastos-proof \
  bash scripts/identity-profile-smoke.sh

What it proves:
  1. `elastos identity nickname set` persists the DID-backed nickname
  2. `identity show` and `identity nickname get` return the same profile data
  3. `pc2 --status --json` exposes the same nickname and People action
  4. `chat` starts with the DID nickname by default when `--nick` is omitted
  5. the `codex` agent persona resolves to a DID distinct from the root profile DID
EOF
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --help|-h)
            usage
            exit 0
            ;;
        *)
            echo "Unknown argument: $1" >&2
            usage >&2
            exit 1
            ;;
    esac
done

if [[ -z "${ELASTOS_BIN}" || ! -x "${ELASTOS_BIN}" ]]; then
    echo "[identity-profile-smoke] missing elastos binary: ${ELASTOS_BIN:-<empty>}" >&2
    exit 1
fi

if [[ -z "${HOME_DIR}" ]]; then
    echo "[identity-profile-smoke] missing home directory" >&2
    exit 1
fi

mkdir -p "${HOME_DIR}" "${XDG_DIR}"

run_elastos() {
    local -a env_args=(
        "HOME=${HOME_DIR}"
        "XDG_DATA_HOME=${XDG_DIR}"
        "ELASTOS_DATA_DIR=${DATA_DIR}"
    )
    env "${env_args[@]}" "${ELASTOS_BIN}" "$@"
}

echo "[identity-profile-smoke] set DID nickname"
SET_OUT="$(run_elastos identity nickname set "${PROFILE_NICK}")"
echo "${SET_OUT}"
if ! grep -q "Nickname set to '${PROFILE_NICK}'." <<<"${SET_OUT}"; then
    echo "[identity-profile-smoke] nickname set output was unexpected" >&2
    exit 1
fi

echo "[identity-profile-smoke] read DID nickname"
GET_OUT="$(run_elastos identity nickname get | tr -d '\r')"
echo "${GET_OUT}"
if [[ "${GET_OUT}" != "${PROFILE_NICK}" ]]; then
    echo "[identity-profile-smoke] nickname get returned '${GET_OUT}', expected '${PROFILE_NICK}'" >&2
    exit 1
fi

echo "[identity-profile-smoke] show DID profile"
SHOW_OUT="$(run_elastos identity show)"
echo "${SHOW_OUT}"
if ! grep -q "^Profile:[[:space:]]\+initialized" <<<"${SHOW_OUT}"; then
    echo "[identity-profile-smoke] identity show did not report an initialized profile" >&2
    exit 1
fi
ROOT_DID="$(awk '/^DID:/{print $2}' <<<"${SHOW_OUT}")"
if [[ -z "${ROOT_DID}" || "${ROOT_DID}" == "(not" ]]; then
    echo "[identity-profile-smoke] identity show did not return a real DID" >&2
    exit 1
fi
if ! grep -q "^Nickname:[[:space:]]\+${PROFILE_NICK}$" <<<"${SHOW_OUT}"; then
    echo "[identity-profile-smoke] identity show did not reflect nickname '${PROFILE_NICK}'" >&2
    exit 1
fi

echo "[identity-profile-smoke] prove PC2 snapshot sees the same nickname"
PC2_JSON="$(run_elastos pc2 --status --json)"
PROFILE_NICK="${PROFILE_NICK}" jq -e '
    .nickname == env.PROFILE_NICK
    and (.actions | any(.id == "identity-nickname-set"))
' >/dev/null <<<"${PC2_JSON}" || {
    echo "[identity-profile-smoke] pc2 status json did not reflect the DID nickname/action" >&2
    echo "${PC2_JSON}" >&2
    exit 1
}

echo "[identity-profile-smoke] prove chat defaults to the DID nickname"
ELASTOS_BIN="${ELASTOS_BIN}" \
HOME_DIR="${HOME_DIR}" \
XDG_DIR="${XDG_DIR}" \
DATA_DIR="${DATA_DIR}" \
PROFILE_NICK="${PROFILE_NICK}" \
python3 - <<'PY'
import os
import pty
import select
import signal
import subprocess
import time

elastos_bin = os.environ["ELASTOS_BIN"]
home = os.environ["HOME_DIR"]
xdg = os.environ["XDG_DIR"]
data_dir = os.environ["DATA_DIR"]
profile_nick = os.environ["PROFILE_NICK"]

env = os.environ.copy()
env["HOME"] = home
env["XDG_DATA_HOME"] = xdg
env["ELASTOS_DATA_DIR"] = data_dir
env["ELASTOS_CHAT_FORCE_STDIN"] = "1"

master, slave = pty.openpty()
proc = subprocess.Popen(
    [elastos_bin, "chat"],
    stdin=slave,
    stdout=slave,
    stderr=slave,
    env=env,
    close_fds=True,
    start_new_session=True,
)
os.close(slave)

def read_for(seconds: float) -> str:
    end = time.time() + seconds
    chunks: list[bytes] = []
    while time.time() < end:
        ready, _, _ = select.select([master], [], [], 0.1)
        if not ready:
            continue
        try:
            data = os.read(master, 65536)
        except OSError:
            break
        if not data:
            break
        chunks.append(data)
    return b"".join(chunks).decode("utf-8", "replace")

def send(data: bytes, pause: float = 0.2) -> None:
    os.write(master, data)
    time.sleep(pause)

try:
    initial = read_for(8.0)
    if f"Chat as '{profile_nick}' on #general." not in initial:
        raise SystemExit(
            "[identity-profile-smoke] chat did not default to DID nickname:\n" + initial
        )
    send(b"/quit\r", 0.5)
    tail = read_for(2.0)
    proc.wait(timeout=5)
    if proc.returncode != 0:
        raise SystemExit(
            f"[identity-profile-smoke] chat exited with {proc.returncode}:\n{initial}{tail}"
        )
finally:
    if proc.poll() is None:
        os.killpg(proc.pid, signal.SIGTERM)
        try:
            proc.wait(timeout=2)
        except Exception:
            os.killpg(proc.pid, signal.SIGKILL)
            proc.wait(timeout=2)
    os.close(master)
PY

echo "[identity-profile-smoke] prove agent persona DID stays distinct from root DID"
COORDS_FILE="${DATA_DIR}/runtime-coords.json"
if [[ ! -f "${COORDS_FILE}" ]]; then
    echo "[identity-profile-smoke] runtime coords missing: ${COORDS_FILE}" >&2
    exit 1
fi
ROOT_DID="${ROOT_DID}" PERSONA_NAME="${PERSONA_NAME}" COORDS_FILE="${COORDS_FILE}" python3 - <<'PY'
import json
import os
import sys
import time
import urllib.error
import urllib.request

coords = json.loads(open(os.environ["COORDS_FILE"], "r", encoding="utf-8").read())
root_did = os.environ["ROOT_DID"]
persona_name = os.environ["PERSONA_NAME"]
api_url = coords["api_url"]
secret = coords["attach_secret"]

def post(url: str, body: dict, headers: dict[str, str]) -> dict:
    req = urllib.request.Request(
        url,
        data=json.dumps(body).encode("utf-8"),
        headers={"Content-Type": "application/json", **headers},
    )
    with urllib.request.urlopen(req, timeout=5) as resp:
        return json.loads(resp.read().decode("utf-8"))

attach = post(
    api_url + "/api/auth/attach",
    {"secret": secret, "scope": "shell"},
    {},
)
token = attach["token"]

capability = post(
    api_url + "/api/capability/request",
    {"resource": "elastos://did/*", "action": "execute"},
    {"Authorization": f"Bearer {token}"},
)
did_cap = capability.get("token")
request_id = capability.get("request_id")
if not did_cap and request_id:
    for _ in range(60):
        req = urllib.request.Request(
            api_url + f"/api/capability/request/{request_id}",
            headers={"Authorization": f"Bearer {token}"},
        )
        with urllib.request.urlopen(req, timeout=5) as resp:
            status = json.loads(resp.read().decode("utf-8"))
        did_cap = status.get("token")
        if did_cap:
            break
        if status.get("status") in {"denied", "expired"}:
            raise SystemExit(
                "[identity-profile-smoke] did capability request failed: "
                + json.dumps(status)
            )
        time.sleep(0.1)

if not did_cap:
    raise SystemExit("[identity-profile-smoke] did capability request timed out")

headers = {
    "Authorization": f"Bearer {token}",
    "X-Capability-Token": did_cap,
}
persona = post(
    api_url + "/api/provider/did/get_persona_did",
    {"name": persona_name},
    headers,
)
data = persona.get("data", {})
persona_did = data.get("did", "")
owner_did = data.get("owner_did", "")

if not persona_did or not owner_did:
    raise SystemExit(
        "[identity-profile-smoke] get_persona_did returned incomplete data: "
        + json.dumps(persona)
    )
if owner_did != root_did:
    raise SystemExit(
        "[identity-profile-smoke] persona owner DID mismatch: "
        + json.dumps({"root_did": root_did, "owner_did": owner_did, "persona_did": persona_did})
    )
if persona_did == root_did:
    raise SystemExit(
        "[identity-profile-smoke] persona DID should differ from root DID: "
        + json.dumps({"root_did": root_did, "persona_did": persona_did})
    )

print(
    json.dumps(
        {"root_did": root_did, "persona_name": persona_name, "persona_did": persona_did},
        indent=2,
    )
)
PY

echo "[identity-profile-smoke] OK"
echo "  home:         ${HOME_DIR}"
echo "  nickname:     ${PROFILE_NICK}"
echo "  root did:     ${ROOT_DID}"
echo "  persona name: ${PERSONA_NAME}"
