#!/usr/bin/env bash
# scripts/check-linux-untouched.sh
#
# Linux-untouched gate for the Vz backend work.
#
# Per `docs/vz-backend/PLAN.md`, the Mac/Vz backend lives in
# `elastos/crates/elastos-vz/` and additive `cfg(target_os = "macos")`
# sites in `elastos-server`. The four crates listed in PROTECTED_PATHS
# below MUST NOT be modified by any Phase 1–5 Vz commit. This script
# fails CI (and pre-push hooks) if anything in those crates has been
# touched relative to the merge-base with the base branch.
#
# Usage:
#   scripts/check-linux-untouched.sh                    # vs origin/main
#   scripts/check-linux-untouched.sh main               # vs local main
#   scripts/check-linux-untouched.sh origin/anders      # vs upstream
#
# Exit codes:
#   0  — clean: no protected paths touched
#   1  — protected paths touched (lists the files)
#   2  — git plumbing failure (no merge-base, bad ref, etc.)
#
# Anchors:
#   - docs/vz-backend/PLAN.md → "Linux-untouched: explicit guarantees"
#   - PRINCIPLES.md #10 "One Canonical Path"
set -euo pipefail

BASE_REF="${1:-origin/main}"

PROTECTED_PATHS=(
  "elastos/crates/elastos-crosvm/"
  "elastos/crates/elastos-runtime/"
  "elastos/crates/elastos-common/"
  "elastos/crates/elastos-compute/"
)

# Resolve to a merge-base so the diff is "changes on this branch
# only", not "everything between two refs". This makes the gate
# meaningful for both feature branches and PRs.
if ! merge_base=$(git merge-base HEAD "$BASE_REF" 2>/dev/null); then
  echo "error: cannot compute merge-base with '$BASE_REF'" >&2
  echo "       run: git fetch origin && retry"          >&2
  exit 2
fi

touched=$(git diff --name-only "$merge_base" -- "${PROTECTED_PATHS[@]}" || true)

if [[ -n "$touched" ]]; then
  echo "Linux-untouched gate FAILED."
  echo
  echo "The following files in protected crates were modified relative to"
  echo "$BASE_REF (merge-base $merge_base):"
  echo
  while IFS= read -r line; do
    echo "  $line"
  done <<< "$touched"
  echo
  echo "Per docs/vz-backend/PLAN.md the Vz backend must NOT modify these"
  echo "crates. If a change there is genuinely required, surface the"
  echo "question in the PR description and update PLAN.md before merging."
  exit 1
fi

echo "Linux-untouched gate OK (no changes in protected crates vs $BASE_REF)."
exit 0
