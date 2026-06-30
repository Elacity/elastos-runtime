#!/usr/bin/env bash
set -euo pipefail

SERVER_URL="${ELASTOS_CHAT_ROOM_SERVER_URL:-http://127.0.0.1:8090}"
MAC_URL="${ELASTOS_CHAT_ROOM_MAC_URL:-http://localhost:61180}"
MAC_SSH="${ELASTOS_CHAT_ROOM_MAC_SSH:-elastos-mac-staging}"
EXPECTED_DIDS="${ELASTOS_CHAT_ROOM_EXPECTED_DIDS:-}"

usage() {
    cat <<'EOF'
Usage:
  scripts/chat-room-live-roster-smoke.sh

Environment:
  ELASTOS_CHAT_ROOM_SERVER_URL       Linux/public gateway URL. Default: http://127.0.0.1:8090
  ELASTOS_CHAT_ROOM_MAC_URL          Mac staging gateway URL as seen from the Mac. Default: http://localhost:61180
  ELASTOS_CHAT_ROOM_MAC_SSH          SSH host used to fetch Mac summary. Default: elastos-mac-staging
                                     Set empty to skip Mac.
  ELASTOS_CHAT_ROOM_EXPECTED_DIDS    Optional comma-separated member DIDs expected in room_control on every checked runtime.

This smoke is intentionally non-mutating. It fetches existing Chat Room summaries
and verifies the user-visible roster invariants: room membership is synced,
active roster rows are not duplicated by DID, and every active member row belongs
to the room.
EOF
}

if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
    usage
    exit 0
fi

tmpdir="$(mktemp -d "${TMPDIR:-/tmp}/elastos-chat-roster.XXXXXX")"
cleanup() {
    rm -rf "${tmpdir}"
}
trap cleanup EXIT

summary_transport_ready() {
    local path="$1"
    python3 - "${path}" <<'PY'
import json
import sys

with open(sys.argv[1], "r", encoding="utf-8") as handle:
    summary = json.load(handle)
transport = summary.get("transport") or {}
sys.exit(0 if transport.get("available") is not False else 1)
PY
}

fetch_local_summary() {
    local label="$1"
    local url="$2"
    local out="$3"
    local fetched=0
    for _ in $(seq 1 12); do
        if curl -fsS "${url%/}/api/apps/chat-room/summary" >"${out}"; then
            fetched=1
            summary_transport_ready "${out}" && return 0
        fi
        sleep 1
    done
    if [[ "${fetched}" == "1" ]]; then
        return 0
    fi
    echo "[chat-room-roster] ${label}: failed to fetch ${url%/}/api/apps/chat-room/summary" >&2
    return 1
}

fetch_ssh_summary() {
    local label="$1"
    local ssh_host="$2"
    local url="$3"
    local out="$4"
    local fetched=0
    for _ in $(seq 1 12); do
        if ssh -o BatchMode=yes -o ConnectTimeout=8 "${ssh_host}" \
            "curl -fsS '${url%/}/api/apps/chat-room/summary'" >"${out}"; then
            fetched=1
            summary_transport_ready "${out}" && return 0
        fi
        sleep 1
    done
    if [[ "${fetched}" == "1" ]]; then
        return 0
    fi
    echo "[chat-room-roster] ${label}: failed to fetch ${url%/}/api/apps/chat-room/summary via ${ssh_host}" >&2
    return 1
}

declare -a labels files

server_json="${tmpdir}/server.json"
fetch_local_summary "server" "${SERVER_URL}" "${server_json}"
labels+=("server")
files+=("${server_json}")

if [[ -n "${MAC_SSH}" && -n "${MAC_URL}" ]]; then
    mac_json="${tmpdir}/mac.json"
    fetch_ssh_summary "mac" "${MAC_SSH}" "${MAC_URL}" "${mac_json}"
    labels+=("mac")
    files+=("${mac_json}")
fi

python3 - "${EXPECTED_DIDS}" "${labels[*]}" "${files[@]}" <<'PY'
import json
import sys
from collections import Counter

expected_raw = sys.argv[1].strip()
labels = sys.argv[2].split()
files = sys.argv[3:]
expected = [item.strip() for item in expected_raw.split(",") if item.strip()]
summaries = []

for label, path in zip(labels, files):
    with open(path, "r", encoding="utf-8") as handle:
        summaries.append((label, json.load(handle)))

if not expected:
    member_dids = []
    for _, summary in summaries:
        room_control = summary.get("room_control") or {}
        for member in room_control.get("members") or []:
            did = (member.get("member_did") or "").strip()
            if did and did not in member_dids:
                member_dids.append(did)
    expected = member_dids

failures = []

for label, summary in summaries:
    room_control = summary.get("room_control") or {}
    room_members = room_control.get("members") or []
    member_dids = [
        (member.get("member_did") or "").strip()
        for member in room_members
        if (member.get("member_did") or "").strip()
    ]
    participants = summary.get("active_participants") or []
    participant_dids = [
        (participant.get("member_did") or "").strip()
        for participant in participants
        if (participant.get("member_did") or "").strip()
    ]
    duplicate_members = sorted(did for did, count in Counter(member_dids).items() if count > 1)
    duplicates = sorted(did for did, count in Counter(participant_dids).items() if count > 1)
    missing_members = [did for did in expected if did not in member_dids]
    unknown_active = sorted(did for did in set(participant_dids) if did not in set(member_dids))
    transport = summary.get("transport") or {}
    transport_available = transport.get("available")

    print(f"[chat-room-roster] {label}:")
    print("  members:")
    for member in room_members:
        did = member.get("member_did") or "<missing>"
        role = member.get("role") or "member"
        profile = member.get("profile_card") or {}
        display = profile.get("display_name") or did.rsplit(":", 1)[-1][-8:]
        print(f"  - {display} [{role}] did={did}")
    print("  active:")
    for participant in participants:
        did = participant.get("member_did") or "<guest>"
        role = participant.get("role") or "guest"
        display = participant.get("display_name") or "<unnamed>"
        local_count = participant.get("local_session_count")
        print(f"  - {display} [{role}] did={did} local_sessions={local_count}")

    if duplicate_members:
        failures.append(f"{label}: duplicate room_control member DID rows: {', '.join(duplicate_members)}")
    if duplicates:
        failures.append(f"{label}: duplicate active participant DID rows: {', '.join(duplicates)}")
    if missing_members:
        failures.append(f"{label}: missing expected room member DIDs: {', '.join(missing_members)}")
    if unknown_active:
        failures.append(f"{label}: active participant DIDs are not room members: {', '.join(unknown_active)}")
    if transport_available is False:
        failures.append(f"{label}: Chat transport reports unavailable")

if len(summaries) > 1:
    first_members = {
        (member.get("member_did") or "").strip()
        for member in (summaries[0][1].get("room_control") or {}).get("members") or []
        if (member.get("member_did") or "").strip()
    }
    for label, summary in summaries[1:]:
        members = {
            (member.get("member_did") or "").strip()
            for member in (summary.get("room_control") or {}).get("members") or []
            if (member.get("member_did") or "").strip()
        }
        if members != first_members:
            failures.append(f"{label}: room_control member set differs from {summaries[0][0]}")

if failures:
    for failure in failures:
        print(f"[chat-room-roster] FAIL: {failure}", file=sys.stderr)
    sys.exit(1)

print("[chat-room-roster] pass")
PY
