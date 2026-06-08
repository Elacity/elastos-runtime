#!/usr/bin/env bash
#
# ddrm-drift-check.sh — guard the dDRM provider chain against contract drift.
#
# The dDRM providers (drm/rights/key/decrypt) consume shared types from
# `elastos-common::protected_content`. Anders is actively redoing 0.4.0 commits,
# so those types can move under us. This script fails LOUDLY (non-zero) if any
# schema constant or struct/field the chain depends on is missing from the
# CURRENTLY CHECKED-OUT `protected_content.rs` — so a rebase onto a new 0.4.0 is a
# button-press verification, not an archaeology dig.
#
# It also prints the `encrypt-provider` self-contained types as the explicit
# "reconcile me to elastos-common once 0.4.0 stabilises" list (see
# docs/convergence/DDRM_ENCRYPT_INVARIANT.md).
#
# Usage:
#   scripts/ddrm-drift-check.sh
# Exit code: 0 = contract intact, 1 = drift detected.

set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PC="$ROOT/elastos/crates/elastos-common/src/protected_content.rs"

red()   { printf '\033[31m%s\033[0m\n' "$1"; }
green() { printf '\033[32m%s\033[0m\n' "$1"; }
bold()  { printf '\033[1m%s\033[0m\n' "$1"; }

if [[ ! -f "$PC" ]]; then
  red "FAIL: cannot find $PC (is elastos-common present on this base?)"
  exit 1
fi

# Symbols the dDRM chain depends on. If any disappears or is renamed, a provider
# will fail to compile — catch it here with a clear message instead.
REQUIRED_CONSTS=(
  "SEALED_OBJECT_SCHEMA"
  "RIGHTS_POLICY_SCHEMA"
  "KEY_RELEASE_REQUEST_SCHEMA"
  "DECRYPT_SESSION_REQUEST_SCHEMA"
  "DECRYPT_SESSION_SCHEMA"
  "RELEASE_RECEIPT_SCHEMA"
  "RIGHTS_DECISION_RECEIPT_SCHEMA"
  "PROTECTED_CONTENT_ACTIONS"
  "PROTECTED_CONTENT_OUTPUTS"
  # Default algorithm sets consumed by drm/key (the PQ-hybrid negotiation surface;
  # the crown-jewel mandate lives here — a rename must fail loud, not silently).
  "DEFAULT_PROTECTED_CONTENT_CIPHER"
  "DEFAULT_PROTECTED_CONTENT_KEMS"
  "DEFAULT_PROTECTED_CONTENT_SHARE_SCHEME"
  "DEFAULT_PROTECTED_CONTENT_SIGNATURES"
)

REQUIRED_STRUCTS=(
  "SealedObjectV1"
  "KeyEnvelopeV1"
  "KeyEnvelopeAlgorithmsV1"
  "RightsPolicyV1"
  "ViewerRequirementV1"
  "KeyReleaseRequestV1"
  "DecryptSessionRequestV1"
  "DecryptSessionV1"
  "ReleaseReceiptV1"
  "RightsDecisionReceiptV1"
)

# Free functions the chain calls into. A rename here breaks drm + key compilation
# but leaves every struct/const intact, so it must be pinned explicitly.
REQUIRED_FNS=(
  "validate_protected_content_key_envelope_algorithms"
)

# Field-level invariants: the chain-binding fields whose loss would silently break
# authorization composition (rights -> key -> decrypt) or PQ-algorithm negotiation.
# Format: "Struct:field".
REQUIRED_FIELDS=(
  "KeyReleaseRequestV1:rights_receipt"
  "DecryptSessionRequestV1:release_receipt"
  "ReleaseReceiptV1:session_id"
  "ReleaseReceiptV1:action"
  "RightsDecisionReceiptV1:allowed"
  "KeyEnvelopeV1:wrapped_cek"
  # PQ negotiation surface validated by validate_protected_content_key_envelope_algorithms.
  "KeyEnvelopeAlgorithmsV1:cipher"
  "KeyEnvelopeAlgorithmsV1:kem"
  "KeyEnvelopeAlgorithmsV1:signature"
  "KeyEnvelopeAlgorithmsV1:share_scheme"
)

fail=0

bold "dDRM contract drift check"
echo "base: $(git -C "$ROOT" rev-parse --short HEAD 2>/dev/null || echo '?')  file: protected_content.rs"
echo

bold "schema constants:"
for c in "${REQUIRED_CONSTS[@]}"; do
  if grep -qE "pub const ${c}\b" "$PC"; then
    green "  ok   $c"
  else
    red   "  MISS $c"
    fail=1
  fi
done

echo
bold "structs:"
for s in "${REQUIRED_STRUCTS[@]}"; do
  if grep -qE "pub struct ${s}\b" "$PC"; then
    green "  ok   $s"
  else
    red   "  MISS $s"
    fail=1
  fi
done

echo
bold "free functions:"
for fn in "${REQUIRED_FNS[@]}"; do
  if grep -qE "pub fn ${fn}\b" "$PC"; then
    green "  ok   $fn"
  else
    red   "  MISS $fn"
    fail=1
  fi
done

echo
bold "chain-binding fields:"
for f in "${REQUIRED_FIELDS[@]}"; do
  struct="${f%%:*}"
  field="${f##*:}"
  # Extract the struct body (from `pub struct X {` to the next closing brace at
  # column 0) and check the field is declared in it.
  if awk "/pub struct ${struct}[[:space:]]*\\{/{f=1} f{print} f&&/^}/{exit}" "$PC" \
       | grep -qE "(^|[[:space:]])pub ${field}:"; then
    green "  ok   ${struct}.${field}"
  else
    red   "  MISS ${struct}.${field}"
    fail=1
  fi
done

echo
bold "encrypt-provider — reconcile-to-elastos-common list (informational):"
echo "  encrypt-provider is intentionally self-contained while 0.4.0 churns."
echo "  When 0.4.0 stabilises, replace its local types with shared ones:"
echo "    local SealRequest          -> add an EncryptSealRequestV1 to protected_content (or reuse)"
echo "    local sealed output (test) -> elastos_common::protected_content::SealedObjectV1"
echo "  Tracking: docs/convergence/DDRM_ENCRYPT_INVARIANT.md"

echo
if [[ "$fail" -eq 0 ]]; then
  green "PASS: dDRM contract surface intact on this base."
  exit 0
else
  red "FAIL: dDRM contract drifted — a provider will not compile. Reconcile before rebase/PR."
  exit 1
fi
