#!/usr/bin/env bash
set -euo pipefail

usage() {
    cat <<'EOF'
Usage: ./scripts/check-versioning.sh <version>

Checks whether <version> matches the ElastOS runtime release policy.

Accepted:
  X.Y.Z
  X.Y.Z-alpha.N
  X.Y.Z-beta.N
  X.Y.Z-rc.N

Temporarily accepted for compatibility:
  X.Y.Z-alphaN
  X.Y.Z-betaN
  X.Y.Z-rcN

Optional SemVer build metadata is also allowed:
  X.Y.Z-rc.N+build.1
EOF
}

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
    usage
    exit 0
fi

VERSION="${1:-}"
if [[ -z "$VERSION" ]]; then
    echo "Error: version is required" >&2
    usage >&2
    exit 1
fi

core='(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)'
meta='(\+[0-9A-Za-z.-]+)?'
preferred="^${core}(-(alpha|beta|rc)\.(0|[1-9][0-9]*))?${meta}$"
legacy="^${core}(-(alpha|beta|rc)(0|[1-9][0-9]*))${meta}$"

if [[ "$VERSION" =~ $preferred ]]; then
    exit 0
fi

if [[ "$VERSION" =~ $legacy ]]; then
    echo "Warning: legacy prerelease form accepted for compatibility: $VERSION" >&2
    echo "Preferred form for new releases is X.Y.Z-rc.N / -beta.N / -alpha.N" >&2
    exit 0
fi

echo "Error: invalid release version: $VERSION" >&2
echo "Expected X.Y.Z or X.Y.Z-{alpha|beta|rc}.N" >&2
echo "See docs/VERSIONING.md for the runtime release policy." >&2
exit 1
