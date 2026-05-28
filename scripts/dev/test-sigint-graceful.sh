#!/usr/bin/env bash
# scripts/dev/test-sigint-graceful.sh
#
# Phase 10 Day 9 — regression test for graceful shutdown of
# `elastos run <microvm>` on macOS.
#
# Before this fix, sending SIGINT (Ctrl-C) or SIGTERM to a
# foreground `elastos run ubuntu-base` did not reliably stop the
# underlying `com.apple.Virtualization.VirtualMachine` XPC
# process. Operators had to follow up with `pkill -KILL` to
# clean up — observed during the Phase 9 Day-6 live walkthrough.
#
# This script boots a real microVM, sends SIGINT, waits a
# bounded period, and asserts that the Vz XPC process spawned
# by that run has terminated. Then it repeats with SIGTERM.
#
# Why a shell script rather than `cargo test`:
#   Spawning Apple's Vz framework requires the full Mac
#   bootstrap (signed binary + kernel + rootfs + entitlements).
#   A Rust integration test would have to invoke this same
#   binary as a child process anyway, plus replicate all of
#   `mac-local-setup.sh`'s preflight. The shell harness is the
#   honest seam.
#
# Usage:
#   ./scripts/dev/test-sigint-graceful.sh
#
# Prerequisites (run once):
#   ./scripts/dev/mac-local-setup.sh
#   elastos setup --profile minimal   # provides vmlinux + ubuntu-base rootfs
#
# Exit codes:
#   0  — both SIGINT and SIGTERM produced clean shutdown
#   1  — one or both signals failed to terminate the Vz process
#   2  — environment not ready (binary missing, kernel missing, wrong OS)

set -euo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "error: test-sigint-graceful.sh is macOS-only (got $(uname -s))." >&2
  exit 2
fi

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
ELASTOS_BINARY="$REPO_ROOT/elastos/target/debug/elastos"
DATA_DIR="${ELASTOS_DATA_DIR:-$HOME/Library/Application Support/elastos}"
KERNEL_PATH="$DATA_DIR/bin/vmlinux"
CAPSULE_DIR="$DATA_DIR/capsules/ubuntu-base"

# Bounded wait windows. Boot to Vz visibility takes ~2-3s on M1;
# a clean shutdown after SIGINT should complete inside the 10s
# `VZ_STOP_TIMEOUT` set in run_cmd.rs plus a small drop margin.
readonly BOOT_WAIT_SECS=15
readonly SHUTDOWN_WAIT_SECS=15

if [[ ! -x "$ELASTOS_BINARY" ]]; then
  echo "error: $ELASTOS_BINARY missing or non-executable." >&2
  echo "       run \`./scripts/dev/mac-local-setup.sh\` first." >&2
  exit 2
fi

if [[ ! -f "$KERNEL_PATH" ]]; then
  echo "error: guest kernel missing at $KERNEL_PATH." >&2
  echo "       run \`elastos setup --profile minimal\` first." >&2
  exit 2
fi

if [[ ! -d "$CAPSULE_DIR" ]]; then
  echo "error: ubuntu-base capsule not installed at $CAPSULE_DIR." >&2
  echo "       run \`elastos setup --profile minimal\` first." >&2
  exit 2
fi

# vz_pid_set
#
# Emits one PID per line for every Apple Vz XPC process
# (`com.apple.Virtualization.VirtualMachine`) currently
# visible to the user. Apple launches these via XPC, so the
# spawned process is re-parented to launchd (PPID=1) rather
# than to `elastos run` — a PPID filter therefore misses them.
# We instead diff this set before and after launch to identify
# the VM(s) belonging to this test.
vz_pid_set() {
  /bin/ps -A -o pid,comm \
    | /usr/bin/awk '$2 ~ /Virtualization\.VirtualMachine/ { print $1 }'
}

# vz_diff_count <before_file>
#
# Compares the current Vz PID set against the saved baseline
# and prints the number of newly-appeared PIDs. Used both to
# detect when the test VM has booted and to confirm it is gone
# after shutdown.
vz_diff_count() {
  local before_file="$1"
  local after_pids
  after_pids="$(vz_pid_set)"
  /usr/bin/comm -23 \
    <(echo "$after_pids" | /usr/bin/sort -u) \
    <(/usr/bin/sort -u "$before_file") \
    | /usr/bin/grep -c '^[0-9]' || true
}

# run_signal_test <signal_name>
#
# Boots `elastos run ubuntu-base` in the background, waits for
# Vz to appear, delivers the named signal, then waits for the
# Vz XPC child to terminate. Returns 0 on clean shutdown, 1
# otherwise.
run_signal_test() {
  local signal_name="$1"
  local elastos_log baseline_pids
  elastos_log="$(mktemp -t elastos-sigtest-log.XXXXXX)"
  baseline_pids="$(mktemp -t elastos-sigtest-pids.XXXXXX)"

  echo "--- testing $signal_name shutdown ---"
  echo "  log: $elastos_log"

  vz_pid_set >"$baseline_pids"
  local baseline_count
  baseline_count="$(/usr/bin/wc -l <"$baseline_pids" | /usr/bin/tr -d ' ')"
  echo "  Vz baseline before launch: $baseline_count process(es)"

  "$ELASTOS_BINARY" run ubuntu-base </dev/null >"$elastos_log" 2>&1 &
  local elastos_pid=$!
  echo "  elastos run PID: $elastos_pid"

  local waited=0
  local vz_new=0
  while (( waited < BOOT_WAIT_SECS )); do
    vz_new="$(vz_diff_count "$baseline_pids")"
    if (( vz_new > 0 )); then
      break
    fi
    /bin/sleep 1
    waited=$((waited + 1))
  done

  if (( vz_new == 0 )); then
    echo "  FAIL: no new Vz XPC process appeared in ${BOOT_WAIT_SECS}s"
    echo "  --- elastos log (tail) ---"
    /usr/bin/tail -30 "$elastos_log" | /usr/bin/sed 's/^/    /'
    /bin/kill -KILL "$elastos_pid" 2>/dev/null || true
    /bin/rm -f "$elastos_log" "$baseline_pids"
    return 1
  fi
  echo "  new Vz XPC visible after ${waited}s (delta=+${vz_new}). Sending SIG${signal_name}..."

  /bin/kill -"$signal_name" "$elastos_pid"

  local shutdown_waited=0
  while (( shutdown_waited < SHUTDOWN_WAIT_SECS )); do
    if ! /bin/kill -0 "$elastos_pid" 2>/dev/null; then
      break
    fi
    /bin/sleep 1
    shutdown_waited=$((shutdown_waited + 1))
  done

  if /bin/kill -0 "$elastos_pid" 2>/dev/null; then
    echo "  FAIL: elastos run PID $elastos_pid still alive after ${SHUTDOWN_WAIT_SECS}s"
    /bin/kill -KILL "$elastos_pid" 2>/dev/null || true
    /bin/rm -f "$elastos_log" "$baseline_pids"
    return 1
  fi

  # Give Apple's XPC teardown a brief grace window (the elastos
  # process is gone but the Vz XPC tail can flush for ~1s).
  /bin/sleep 2

  local remaining_new
  remaining_new="$(vz_diff_count "$baseline_pids")"
  if (( remaining_new > 0 )); then
    echo "  FAIL: $remaining_new Vz XPC process(es) survived SIG$signal_name shutdown"
    /bin/ps -A -o pid,comm | /usr/bin/grep 'Virtualization\.VirtualMachine' | /usr/bin/grep -v grep | /usr/bin/sed 's/^/    /' || true
    /bin/rm -f "$elastos_log" "$baseline_pids"
    return 1
  fi

  echo "  PASS: elastos run + Vz XPC both gone after SIG$signal_name (run=${shutdown_waited}s)"
  /bin/rm -f "$elastos_log" "$baseline_pids"
  return 0
}

failures=0

if ! run_signal_test INT; then
  failures=$((failures + 1))
fi

if ! run_signal_test TERM; then
  failures=$((failures + 1))
fi

echo
if (( failures == 0 )); then
  echo "OK: SIGINT and SIGTERM both produced clean shutdown."
  exit 0
fi

echo "FAIL: $failures signal(s) did not produce clean shutdown."
exit 1
