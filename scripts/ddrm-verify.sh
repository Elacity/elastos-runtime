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
#   3. ladder intact     — scripts/ddrm-ladder-check.sh (every provider suite +
#                         decrypt-provider feature rung runs at its EXPECTED count,
#                         and the wasm32-wasip1 builds are clean). Catches a
#                         silently-dropped/feature-gated-out test. Heaviest gate.
#   4. WASI smoke        — scripts/ddrm-chain-smoke.sh (the 4 chain providers run
#                         under wasmtime, fail-closed end-to-end). Skips clean when
#                         wasmtime is absent (so CI without the CLI is never broken).
#
# Exit code: 0 = all gates pass (or skip clean), non-zero = a gate failed.
#
# Gates 1+2 are the cheap, network-free contract + conformance checks (run first);
# gates 3+4 compile + run the suites / wasm, so they are slower (the `harden` rung
# runs an ML-DSA-65 corruption sweep; gate 4 executes under wasmtime). Set
# DDRM_VERIFY_FAST=1 to skip the heavy gates (3+4).
set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

bold() { printf '\033[1m%s\033[0m\n' "$1"; }
red()  { printf '\033[31m%s\033[0m\n' "$1"; }
green(){ printf '\033[32m%s\033[0m\n' "$1"; }

rc=0

bold "== dDRM verify (1/4): contract drift =="
if ! "$HERE/ddrm-drift-check.sh"; then
  rc=1
fi

echo
bold "== dDRM verify (2/4): PC2 cross-impl conformance =="
if ! "$HERE/pc2-conformance.sh"; then
  rc=1
fi

echo
bold "== dDRM verify (3/4): test ladder + wasm builds =="
if [ "${DDRM_VERIFY_FAST:-0}" = "1" ]; then
  echo "  (DDRM_VERIFY_FAST=1 — skipping the heavy ladder gate)"
else
  if ! "$HERE/ddrm-ladder-check.sh"; then
    rc=1
  fi
fi

echo
bold "== dDRM verify (4/4): WASI chain smoke =="
if [ "${DDRM_VERIFY_FAST:-0}" = "1" ]; then
  echo "  (DDRM_VERIFY_FAST=1 — skipping the heavy WASI smoke gate)"
elif ! command -v wasmtime >/dev/null 2>&1; then
  echo "  SKIP: wasmtime not found (install: brew install wasmtime) — gate skipped clean."
else
  if ! "$HERE/ddrm-chain-smoke.sh"; then
    rc=1
  fi
fi

echo
if [ "$rc" -eq 0 ]; then
  green "dDRM verify: ALL GATES PASS (or skipped clean)."
else
  red "dDRM verify: a gate FAILED — reconcile before rebase/PR."
fi
exit "$rc"
