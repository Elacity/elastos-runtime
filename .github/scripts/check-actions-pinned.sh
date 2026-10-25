#!/usr/bin/env bash
# Fails when a workflow or composite action uses a remote action or reusable workflow by a
# mutable tag or branch instead of a full 40-character commit SHA (supply-chain gate).
#   allowed: ./local/action   owner/repo[/path]@<40-hex> # vX.Y.Z   docker://image@sha256:<64-hex>
# Usage: check-actions-pinned.sh [repo-root]   (exit 1 and one ::error line per unpinned reference)
set -euo pipefail
cd "${1:-.}"
fail=0
count=0
while IFS= read -r -d '' f; do
  n=0
  while IFS= read -r line || [ -n "$line" ]; do
    n=$((n + 1))
    code="${line%%#*}"
    if [[ "$code" =~ ^[[:space:]]*(-[[:space:]]+)?uses:[[:space:]]*[\"\']?([^\"\'[:space:]]+) ]]; then
      ref="${BASH_REMATCH[2]}"
      count=$((count + 1))
      case "$ref" in
        ./*) continue ;;
        docker://*) [[ "$ref" =~ @sha256:[0-9a-f]{64}$ ]] && continue ;;
        *@*) [[ "${ref##*@}" =~ ^[0-9a-f]{40}$ ]] && continue ;;
      esac
      echo "::error file=${f},line=${n}::unpinned action reference '${ref}': pin it to a full commit SHA with the tag in a trailing comment"
      fail=$((fail + 1))
    fi
  done <"$f"
done < <(find .github/workflows .github/actions -type f \( -name '*.yml' -o -name '*.yaml' \) -print0 2>/dev/null)
echo "checked ${count} uses: references, ${fail} unpinned"
[ "$fail" -eq 0 ]
