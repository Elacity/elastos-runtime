#!/usr/bin/env bash
#
# ddrm-ladder-check.sh — assert the dDRM test ladder + wasm builds are intact.
#
# The contract/conformance gates (ddrm-drift-check, pc2-conformance) prove the
# *interfaces* hold. This gate proves the *implementation* ladder is all green
# with the EXPECTED test counts — so a silently-dropped, auto-skipped, or
# feature-gated-out test fails the gate instead of passing unnoticed.
#
# Two checks:
#   1. test ladder   — each provider suite / decrypt-provider feature rung runs
#                      and its `N passed` is asserted to equal the expected count
#                      (and `0 failed`).
#   2. wasm builds    — the providers build to wasm32-wasip1 (the capability
#                      substrate), incl. the decrypt-provider PQ/rail features.
#                      Skips clean if the wasm target is not installed.
#
# Exit code: 0 = ladder intact, non-zero = a count drifted or a build failed.
#
# NOTE: this is the heavier gate (the `harden` rung runs an ML-DSA-65 corruption
# sweep). Run it before a rebase/PR; the cheap contract gate runs first.
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CAPSULES="$ROOT/capsules"

bold() { printf '\033[1m%s\033[0m\n' "$1"; }
red()  { printf '\033[31m%s\033[0m\n' "$1"; }
green(){ printf '\033[32m%s\033[0m\n' "$1"; }

rc=0

# --- 1. test ladder (label | dir | features | expected_passed) --------------
#
# Counts are pinned; update them in lockstep with the suites (and with
# DDRM_STATUS.md / HANDOVER.md) when tests are intentionally added/removed.
LADDER=(
  "encrypt-provider (default)|encrypt-provider||20"
  "encrypt-provider escrow|encrypt-provider|escrow|25"
  "drm-provider (default)|drm-provider||15"
  "publish-provider (default)|publish-provider||16"
  "content-market (default)|content-market||29"
  "rights-provider (default)|rights-provider||9"
  "rights-provider chain-rights|rights-provider|chain-rights|18"
  "ddrm-envelope (lib)|ddrm-envelope||16"
  "ddrm-plan-runner (lib)|ddrm-plan-runner||45"
  "key-provider (default)|key-provider||18"
  "key-provider key-authority-ref|key-provider|key-authority-ref|38"
  "decrypt-provider (default)|decrypt-provider||25"
  "decrypt-provider rail-prep|decrypt-provider|rail-prep|27"
  "decrypt-provider pq-envelope|decrypt-provider|pq-envelope|29"
  "decrypt-provider pq-rail-prep|decrypt-provider|pq-rail-prep|31"
  "decrypt-provider vectors|decrypt-provider|vectors|42"
  "decrypt-provider rail-shim|decrypt-provider|rail-shim|45"
  "decrypt-provider pq-mldsa|decrypt-provider|pq-mldsa|34"
  "decrypt-provider envelope-conformance|decrypt-provider|envelope-conformance|35"
  "decrypt-provider pq-mldsa-hybrid|decrypt-provider|pq-mldsa-hybrid|37"
  "decrypt-provider rail-shim-mldsa|decrypt-provider|rail-shim-mldsa|54"
  "decrypt-provider harden|decrypt-provider|harden|65"
  "decrypt-provider rail-live|decrypt-provider|rail-live|57"
  "decrypt-provider rail-bind|decrypt-provider|rail-bind|60"
  "decrypt-provider rail-mint|decrypt-provider|rail-mint|62"
  "decrypt-provider rail-audit|decrypt-provider|rail-audit|62"
  "decrypt-provider rail-material|decrypt-provider|rail-material|65"
)

bold "== dDRM ladder (1/2): test suites + asserted counts =="
for row in "${LADDER[@]}"; do
  IFS='|' read -r label dir features expected <<< "$row"
  if [ -n "$features" ]; then
    out="$(cd "$CAPSULES/$dir" && cargo test --features "$features" 2>/dev/null)"
  else
    out="$(cd "$CAPSULES/$dir" && cargo test 2>/dev/null)"
  fi
  # Sum across all "test result:" lines (unit + any doc/integration sections).
  passed="$(printf '%s\n' "$out" | sed -n 's/^test result: ok\. \([0-9]*\) passed.*/\1/p' | awk '{s+=$1} END {print s+0}')"
  failed="$(printf '%s\n' "$out" | sed -n 's/.*ok\. [0-9]* passed; \([0-9]*\) failed.*/\1/p' | awk '{s+=$1} END {print s+0}')"
  ok_lines="$(printf '%s\n' "$out" | grep -c 'test result: ok')"
  if [ "$ok_lines" -eq 0 ] || [ "$failed" -ne 0 ] || [ "$passed" -ne "$expected" ]; then
    red "  FAIL  $label — expected $expected passed / 0 failed, got $passed passed / $failed failed"
    rc=1
  else
    green "  ok    $label — $passed passed"
  fi
done

# --- 1b. cross-invariant seam: the encrypt->decrypt round-trips MUST be exercised -
#
# The round-trip goldens (produced by encrypt-provider's real in-boundary engine,
# replayed by decrypt-provider) are the artifacts that pin invariant #1 <-> #2 over
# real playback shapes: single-sample, multi-sample, and subsample (clear leader).
# Run them BY NAME (filter `round_trip_golden` -> exactly these 3) so a rename /
# cfg-drift / encrypt-side break that silently drops one fails the gate instead of
# just shifting the count.
SEAM_EXPECTED=3
echo
bold "== dDRM ladder (1b/2): encrypt<->decrypt seam exercised =="
seam_out="$(cd "$CAPSULES/decrypt-provider" && cargo test --features vectors round_trip_golden 2>/dev/null)"
seam_passed="$(printf '%s\n' "$seam_out" | sed -n 's/^test result: ok\. \([0-9]*\) passed.*/\1/p' | awk '{s+=$1} END {print s+0}')"
seam_failed="$(printf '%s\n' "$seam_out" | sed -n 's/.*ok\. [0-9]* passed; \([0-9]*\) failed.*/\1/p' | awk '{s+=$1} END {print s+0}')"
if [ "$seam_passed" -eq "$SEAM_EXPECTED" ] && [ "$seam_failed" -eq 0 ]; then
  green "  ok    *_round_trip_golden — $seam_passed passed (single + multisample + subsample seams live)"
else
  red "  FAIL  *_round_trip_golden — expected $SEAM_EXPECTED passed / 0 failed, got $seam_passed/$seam_failed (a seam dropped?)"
  rc=1
fi

# --- 1c. chain-provider mint calldata assembly (filtered, deterministic) -----------
#
# chain-provider's full suite carries one env-dependent loopback-supervisor test, so it
# is NOT laddered as a whole. The content-mint ABI encoder (Day 62) IS pinned here by
# running ONLY the `mint`-named tests, which decode the produced calldata back against
# the Solidity ABI spec (PC2 mint(string,uint16,bytes,bytes) fidelity).
MINT_EXPECTED=10
echo
bold "== dDRM ladder (1c/2): chain-provider mint calldata assembly =="
mint_out="$(cd "$CAPSULES/chain-provider" && cargo test mint 2>/dev/null)"
mint_passed="$(printf '%s\n' "$mint_out" | sed -n 's/^test result: ok\. \([0-9]*\) passed.*/\1/p' | awk '{s+=$1} END {print s+0}')"
mint_failed="$(printf '%s\n' "$mint_out" | sed -n 's/.*ok\. [0-9]* passed; \([0-9]*\) failed.*/\1/p' | awk '{s+=$1} END {print s+0}')"
if [ "$mint_passed" -eq "$MINT_EXPECTED" ] && [ "$mint_failed" -eq 0 ]; then
  green "  ok    mint* — $mint_passed passed (mint calldata decodes to spec)"
else
  red "  FAIL  mint* — expected $MINT_EXPECTED passed / 0 failed, got $mint_passed/$mint_failed"
  rc=1
fi

# --- 2. wasm32-wasip1 builds (dir | features) -------------------------------
echo
bold "== dDRM ladder (2/2): wasm32-wasip1 builds =="
if rustup target list --installed 2>/dev/null | grep -q '^wasm32-wasip1$'; then
  WASM=(
    "encrypt-provider|"
    "encrypt-provider|escrow"
    "drm-provider|"
    "publish-provider|"
    "content-market|"
    "rights-provider|"
    "rights-provider|chain-rights"
    "ddrm-envelope|"
    "key-provider|"
    "key-provider|key-authority-ref"
    "decrypt-provider|"
      "decrypt-provider|pq-mldsa"
      "decrypt-provider|pq-mldsa-hybrid"
      "decrypt-provider|rail-shim-mldsa"
      "decrypt-provider|rail-live"
      "decrypt-provider|rail-bind"
      "decrypt-provider|rail-mint"
      "decrypt-provider|rail-audit"
      "decrypt-provider|rail-material"
  )
  for row in "${WASM[@]}"; do
    IFS='|' read -r dir features <<< "$row"
    if [ -n "$features" ]; then
      label="$dir [$features]"
      build_ok=$( (cd "$CAPSULES/$dir" && cargo build --quiet --target wasm32-wasip1 --features "$features" 2>/dev/null) && echo y || echo n )
    else
      label="$dir"
      build_ok=$( (cd "$CAPSULES/$dir" && cargo build --quiet --target wasm32-wasip1 2>/dev/null) && echo y || echo n )
    fi
    if [ "$build_ok" = y ]; then
      green "  ok    $label"
    else
      red "  FAIL  $label — wasm32-wasip1 build failed"
      rc=1
    fi
  done
else
  echo "  (wasm32-wasip1 target not installed — skipping wasm builds clean)"
fi

echo
if [ "$rc" -eq 0 ]; then
  green "dDRM ladder: INTACT (counts match, wasm builds clean)."
else
  red "dDRM ladder: DRIFTED — a count changed or a build failed."
fi
exit "$rc"
