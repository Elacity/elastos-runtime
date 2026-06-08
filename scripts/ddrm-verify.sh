#!/usr/bin/env bash
#
# ddrm-verify.sh — the standing dDRM pre-rebase / pre-PR gate.
#
# Aggregates the convergence guards into one button-press so a rebase onto a new
# 0.4.0 (or any refactor) is verified, not assumed:
#
#   1. contract drift   — scripts/ddrm-drift-check.sh (every schema/struct/field
#                         the chain depends on still exists on the current base).
#   2. cross-impl parity — scripts/pc2-conformance.sh (our committed golden vectors
#                         decrypt byte-for-byte under PC2 ddrm-decrypt's real code;
#                         skips clean when the PC2 repo is absent).
#
# Exit code: 0 = all gates pass (or skip clean), non-zero = a gate failed.
#
# The host test suites are run separately (`cargo test` per provider); this gate
# is the cheap, network-free contract + conformance check meant to run first.
set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

bold() { printf '\033[1m%s\033[0m\n' "$1"; }
red()  { printf '\033[31m%s\033[0m\n' "$1"; }
green(){ printf '\033[32m%s\033[0m\n' "$1"; }

rc=0

bold "== dDRM verify (1/2): contract drift =="
if ! "$HERE/ddrm-drift-check.sh"; then
  rc=1
fi

echo
bold "== dDRM verify (2/2): PC2 cross-impl conformance =="
if ! "$HERE/pc2-conformance.sh"; then
  rc=1
fi

echo
if [ "$rc" -eq 0 ]; then
  green "dDRM verify: ALL GATES PASS (or skipped clean)."
else
  red "dDRM verify: a gate FAILED — reconcile before rebase/PR."
fi
exit "$rc"
