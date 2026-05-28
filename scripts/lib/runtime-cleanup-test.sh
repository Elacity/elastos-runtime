#!/usr/bin/env bash
# Phase 5 Day 2 — unit tests for the Mac-fixed runtime
# cleanup helper. Replaces the Linux-only `/proc/<pid>`
# liveness checks with the cross-platform `pid_is_running`
# helper from Day 1.
#
# Run directly:
#
#   bash scripts/lib/runtime-cleanup-test.sh
#
# Exits 0 on success, non-zero on first failure.
set -eu

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
# shellcheck source=runtime-cleanup.sh
. "${REPO_ROOT}/scripts/lib/runtime-cleanup.sh"

PASS=0
FAIL=0

ok() {
    PASS=$((PASS + 1))
    printf '  OK   %s\n' "$1"
}

fail() {
    FAIL=$((FAIL + 1))
    printf '  FAIL %s\n' "$1" >&2
}

SCRATCH="$(mktemp -d "${TMPDIR:-/tmp}/elastos-runtime-cleanup-test-XXXXXX")"
trap 'rm -rf "${SCRATCH}"' EXIT

# ─── stop_runtime_from_coords ───────────────────────────────

echo "stop_runtime_from_coords:"

# Case 1 — missing coords file is a clean no-op return 0.
stop_runtime_from_coords "${SCRATCH}/no-such-file.json" \
    && ok "missing coords file is no-op" \
    || fail "missing coords file must return 0"

# Case 2 — live PID gets killed and the coords file is removed.
# Spawn a `sleep 60`, write its PID to a coords file, invoke
# the cleanup, assert: (a) coords file removed; (b) child dead
# within the 2 s grace.
sleep 60 &
sleep_pid=$!
disown "$sleep_pid" 2>/dev/null || true
sleep 0.2
coords="${SCRATCH}/coords.json"
printf '{"pid": "%s"}\n' "$sleep_pid" > "$coords"

stop_runtime_from_coords "$coords"

if [[ ! -f "$coords" ]]; then
    ok "coords file removed after cleanup"
else
    fail "coords file should have been removed: ${coords}"
fi

# Give the kill a moment to propagate even though
# kill_pid_then_group polls internally.
sleep 0.3
if ! kill -0 "$sleep_pid" 2>/dev/null; then
    ok "live PID killed cleanly within grace window"
else
    fail "PID ${sleep_pid} still alive after cleanup"
    kill -9 "$sleep_pid" 2>/dev/null || true
fi

# Case 3 — dead PID in coords file: file is still removed, no
# kill issued.
echo '{"pid": "99999"}' > "$coords"
stop_runtime_from_coords "$coords"
if [[ ! -f "$coords" ]]; then
    ok "dead-PID coords file removed without kill"
else
    fail "dead-PID coords file should have been removed: ${coords}"
fi

# Case 4 — empty PID in coords file: file is removed, no kill
# issued, no `set -u` trip.
echo '{"pid": ""}' > "$coords"
stop_runtime_from_coords "$coords"
if [[ ! -f "$coords" ]]; then
    ok "empty-PID coords file removed without trip"
else
    fail "empty-PID coords file should have been removed: ${coords}"
fi

# ─── summary ────────────────────────────────────────────────

echo
echo "runtime-cleanup.sh: ${PASS} passed, ${FAIL} failed"
if [[ "${FAIL}" -gt 0 ]]; then
    exit 1
fi
exit 0
