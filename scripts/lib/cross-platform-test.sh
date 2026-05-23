#!/usr/bin/env bash
# Phase 5 Day 1 — unit tests for `scripts/lib/cross-platform.sh`.
#
# Bash-only, no external test framework. Run directly:
#
#   bash scripts/lib/cross-platform-test.sh
#
# Exits 0 on success, non-zero on first failure with a clear
# diagnostic. Designed to run on bash 3.2 (macOS default) and
# bash 4+ (Linux).
set -eu

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

# shellcheck source=cross-platform.sh
. "${REPO_ROOT}/scripts/lib/cross-platform.sh"

PASS=0
FAIL=0
TEST_NAME=""

ok() {
    PASS=$((PASS + 1))
    printf '  OK   %s\n' "$1"
}

fail() {
    FAIL=$((FAIL + 1))
    printf '  FAIL %s: %s\n' "$TEST_NAME" "$1" >&2
}

assert_eq() {
    local actual="$1"
    local expected="$2"
    local label="$3"
    if [[ "$actual" == "$expected" ]]; then
        ok "$label (got '$actual')"
    else
        fail "$label: expected '$expected', got '$actual'"
    fi
}

assert_true() {
    local label="$1"
    shift
    if "$@"; then
        ok "$label"
    else
        fail "$label: command failed"
    fi
}

assert_false() {
    local label="$1"
    shift
    if ! "$@"; then
        ok "$label"
    else
        fail "$label: command unexpectedly succeeded"
    fi
}

# ─── pid_is_running ─────────────────────────────────────────

TEST_NAME="pid_is_running"
echo "${TEST_NAME}:"

assert_true "live PID (self) reported running" pid_is_running "$$"
assert_false "dead PID 99999 reported not running" pid_is_running 99999
assert_false "empty PID reported not running" pid_is_running ""

# ─── read_pids_into_array ───────────────────────────────────

TEST_NAME="read_pids_into_array"
echo "${TEST_NAME}:"

declare -a parsed
read_pids_into_array parsed <<'EOF'
100
200
300
EOF

assert_eq "${#parsed[@]}" "3" "three lines yield three entries"
assert_eq "${parsed[0]}" "100" "first entry preserved"
assert_eq "${parsed[1]}" "200" "second entry preserved"
assert_eq "${parsed[2]}" "300" "third entry preserved"

# Empty input must yield a zero-length array, NOT trip set -u.
declare -a empty_arr
read_pids_into_array empty_arr </dev/null
assert_eq "${#empty_arr[@]}" "0" "empty input yields empty array"

# Mixed empty lines must be skipped (defensive against
# `pgrep ... || true` returning blank lines).
declare -a mixed_arr
read_pids_into_array mixed_arr <<'EOF'
42

73

EOF
assert_eq "${#mixed_arr[@]}" "2" "blank lines filtered"
assert_eq "${mixed_arr[0]}" "42" "first non-blank preserved"
assert_eq "${mixed_arr[1]}" "73" "second non-blank preserved"

# ─── vz_host_is_capable ─────────────────────────────────────

TEST_NAME="vz_host_is_capable"
echo "${TEST_NAME}:"

# On the Linux runner: not capable. On macOS 12+: capable.
# Either outcome is valid; we just assert the function returns
# a 0-or-1 status without crashing under `set -eu`.
if [[ "$(uname -s)" == "Darwin" ]]; then
    if vz_host_is_capable; then
        ok "Darwin host: vz_host_is_capable returned true (sw_vers reports >= 12)"
    else
        ok "Darwin host: vz_host_is_capable returned false (pre-macOS-12 or sw_vers unavailable)"
    fi
else
    assert_false "non-Darwin host reported not capable" vz_host_is_capable
fi

# ─── vz_discover_launchable_capsule ─────────────────────────

TEST_NAME="vz_discover_launchable_capsule"
echo "${TEST_NAME}:"

# Synthesise a fake data dir with a parseable capsule manifest
# + rootfs marker file. The helper returns the name on stdout
# and exits 0 if a launchable capsule is found.
SCRATCH="$(mktemp -d "${TMPDIR:-/tmp}/elastos-cp-test-XXXXXX")"
trap 'rm -rf "${SCRATCH}"' EXIT

mkdir -p "${SCRATCH}/capsules/synthetic-fixture-vm"
cat >"${SCRATCH}/capsules/synthetic-fixture-vm/capsule.json" <<'EOF'
{ "name": "synthetic-fixture-vm", "version": "0.0.0", "capsule_type": "microvm" }
EOF
# An empty file is sufficient — the helper only checks for
# existence, not contents.
: > "${SCRATCH}/capsules/synthetic-fixture-vm/rootfs.ext4"

found_name="$(vz_discover_launchable_capsule "${SCRATCH}")"
assert_eq "${found_name}" "synthetic-fixture-vm" "discovers seeded fixture capsule"

# Empty data dir: no capsule → non-zero exit, no stdout.
EMPTY_SCRATCH="$(mktemp -d "${TMPDIR:-/tmp}/elastos-cp-test-empty-XXXXXX")"
if found_empty="$(vz_discover_launchable_capsule "${EMPTY_SCRATCH}" 2>/dev/null)"; then
    fail "empty data dir should not return a capsule name (got '${found_empty}')"
else
    ok "empty data dir reports no launchable capsule"
fi
rm -rf "${EMPTY_SCRATCH}"

# Capsule without rootfs.ext4 must NOT be picked up.
mkdir -p "${SCRATCH}/capsules/wasm-only"
cat >"${SCRATCH}/capsules/wasm-only/capsule.json" <<'EOF'
{ "name": "wasm-only", "capsule_type": "wasm" }
EOF
# Remove the synthetic-fixture-vm rootfs so the only remaining
# capsule is the rootfs-less one.
rm -f "${SCRATCH}/capsules/synthetic-fixture-vm/rootfs.ext4"
if vz_discover_launchable_capsule "${SCRATCH}" >/dev/null 2>&1; then
    fail "wasm-only capsule (no rootfs.ext4) must not be discoverable"
else
    ok "wasm-only capsule correctly skipped"
fi

# ─── kill_pid_then_group ────────────────────────────────────

TEST_NAME="kill_pid_then_group"
echo "${TEST_NAME}:"

# Empty / missing PIDs must be no-op returns (callers reading
# from coords files often pass through missing values; tripping
# `set -u` here would mask real bugs further up the stack).
assert_true "empty PID is no-op return 0" kill_pid_then_group ""

# Non-numeric PIDs are rejected silently.
assert_true "non-numeric PID is no-op return 0" kill_pid_then_group "not-a-pid"

# Already-dead PIDs no-op (the live-pid kill path is exercised
# below in a real spawn + kill test).
assert_true "dead PID is no-op return 0" kill_pid_then_group 99999

# Live PID gets terminated within the grace window. Use a
# 30 s sleep with a 1 s grace; we expect SIGTERM to land first
# and the process to be gone well before the grace expires.
# Suppress bash's "Terminated: 15" job-control noise on
# macOS — it's expected and not a failure.
sleep 30 &
sleep_pid=$!
disown "$sleep_pid" 2>/dev/null || true
sleep 0.2
assert_true "spawned sleep child is alive" pid_is_running "$sleep_pid"
kill_pid_then_group "$sleep_pid" 1
assert_false "kill_pid_then_group terminates live PID within grace" \
    pid_is_running "$sleep_pid"

# ─── free_port_via_python3 ──────────────────────────────────

TEST_NAME="free_port_via_python3"
echo "${TEST_NAME}:"

port="$(free_port_via_python3)"
if [[ -n "$port" ]] && [[ "$port" =~ ^[0-9]+$ ]] \
        && [[ "$port" -ge 1024 ]] && [[ "$port" -le 65535 ]]; then
    ok "returns port in 1024–65535 (got $port)"
else
    fail "expected an ephemeral port, got '$port'"
fi

# ─── cross_platform_assert_native_binary_release_metadata ───

TEST_NAME="cross_platform_assert_native_binary_release_metadata"
echo "${TEST_NAME}:"

# Fixture: components.json under the scratch dir. The helper
# reads via $MANIFEST_PATH so we don't pollute the real
# repo file.
ASSERT_SCRATCH="$(mktemp -d "${TMPDIR:-/tmp}/elastos-cp-assert-XXXXXX")"
# Track for cleanup. Original SCRATCH dir was already trapped
# via `trap 'rm -rf "${SCRATCH}"' EXIT`; extend it.
trap 'rm -rf "${SCRATCH}" "${ASSERT_SCRATCH}"' EXIT

# Synthesise a manifest with no darwin-arm64 entry.
cat >"${ASSERT_SCRATCH}/no-darwin.json" <<'EOF'
{
    "external": {
        "shell": {
            "platforms": {
                "linux-amd64": { "release_path": "shell/linux-amd64/shell" }
            }
        }
    }
}
EOF
if [[ "$(uname -s)" == "Darwin" ]]; then
    # On Mac the helper checks for darwin-arm64; the fixture
    # lacks it → expect failure.
    if cross_platform_assert_native_binary_release_metadata \
            "${ASSERT_SCRATCH}/no-darwin.json" shell 2>/dev/null; then
        fail "no-darwin manifest must fail on Darwin"
    else
        ok "no-darwin manifest correctly fails on Darwin"
    fi
else
    # On Linux the helper checks for linux-amd64 / linux-arm64;
    # the fixture HAS linux-amd64 so the test reverses meaning.
    # Skip the meaningful assertion on non-Darwin hosts and
    # log it as ok for Linux byte-identical behaviour.
    if cross_platform_assert_native_binary_release_metadata \
            "${ASSERT_SCRATCH}/no-darwin.json" shell 2>/dev/null; then
        ok "Linux host: manifest with linux-amd64 entry passes (byte-identical Day-1 behaviour)"
    else
        fail "Linux host: linux-amd64 entry must pass"
    fi
fi

# Synthesise a manifest with explicit darwin-arm64 entry.
cat >"${ASSERT_SCRATCH}/with-darwin.json" <<'EOF'
{
    "external": {
        "shell": {
            "platforms": {
                "darwin-arm64": { "release_path": "shell/darwin-arm64/shell" },
                "linux-amd64":  { "release_path": "shell/linux-amd64/shell" }
            }
        }
    }
}
EOF
if cross_platform_assert_native_binary_release_metadata \
        "${ASSERT_SCRATCH}/with-darwin.json" shell; then
    ok "manifest with darwin-arm64 entry passes"
else
    fail "manifest with darwin-arm64 entry must pass"
fi

# Synthesise a manifest with wildcard `"*"` entry (e.g. WASM
# capsules). Helper must accept it regardless of host OS.
cat >"${ASSERT_SCRATCH}/wildcard.json" <<'EOF'
{
    "external": {
        "home-cli": {
            "platforms": {
                "*": { "release_path": "home-cli/home-cli.tar.gz" }
            }
        }
    }
}
EOF
if cross_platform_assert_native_binary_release_metadata \
        "${ASSERT_SCRATCH}/wildcard.json" home-cli; then
    ok "manifest with wildcard '*' entry passes on all hosts"
else
    fail "manifest with wildcard '*' entry must pass"
fi

# Missing manifest path → helper returns 1, no crash.
if cross_platform_assert_native_binary_release_metadata \
        "${ASSERT_SCRATCH}/does-not-exist.json" shell 2>/dev/null; then
    fail "missing manifest must fail"
else
    ok "missing manifest correctly fails"
fi

# No names supplied → helper returns 1 (defensive).
if cross_platform_assert_native_binary_release_metadata \
        "${ASSERT_SCRATCH}/with-darwin.json" 2>/dev/null; then
    fail "no names supplied must fail"
else
    ok "no names supplied correctly fails"
fi

# ─── summary ────────────────────────────────────────────────

echo
echo "cross-platform.sh: ${PASS} passed, ${FAIL} failed"
if [[ "${FAIL}" -gt 0 ]]; then
    exit 1
fi
exit 0
