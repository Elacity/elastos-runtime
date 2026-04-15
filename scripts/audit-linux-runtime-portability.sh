#!/usr/bin/env bash
set -euo pipefail

PLATFORM=""
BINARY=""
LABEL=""

usage() {
    cat <<EOF
Usage: ./scripts/audit-linux-runtime-portability.sh --platform x86_64-linux --binary /path/to/elastos [--label "host runtime"]

Fail closed unless the Linux runtime binary is portable:
  - no ELF NEEDED shared-library entries
  - no GLIBC versioned symbol requirements
  - ldd must report a non-dynamic/static binary when available
EOF
}

die() {
    echo "[runtime-portability] $*" >&2
    exit 1
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --platform)
            PLATFORM="${2:-}"
            shift 2
            ;;
        --binary)
            BINARY="${2:-}"
            shift 2
            ;;
        --label)
            LABEL="${2:-}"
            shift 2
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            usage >&2
            die "unknown option: $1"
            ;;
    esac
done

[[ -n "$PLATFORM" ]] || die "--platform is required"
[[ -n "$BINARY" ]] || die "--binary is required"
[[ -f "$BINARY" ]] || die "binary not found: $BINARY"

LABEL="${LABEL:-$PLATFORM runtime binary}"

case "$PLATFORM" in
    *-linux) ;;
    *)
        echo "[runtime-portability] skipping non-Linux platform: $PLATFORM"
        exit 0
        ;;
esac

command -v readelf >/dev/null 2>&1 || die "readelf is required"

dynamic_section="$(readelf -d "$BINARY" 2>/dev/null || true)"
if grep -q '(NEEDED)' <<<"$dynamic_section"; then
    die "$LABEL is dynamically linked; shared-library dependencies are not allowed for public Linux releases"
fi

version_section="$(readelf -W -V "$BINARY" 2>/dev/null || true)"
if grep -q 'GLIBC_' <<<"$version_section"; then
    die "$LABEL references GLIBC versioned symbols; public Linux releases must be musl/static"
fi

if command -v ldd >/dev/null 2>&1; then
    ldd_output="$(ldd "$BINARY" 2>&1 || true)"
    if [[ "$ldd_output" != *"statically linked"* ]] && [[ "$ldd_output" != *"not a dynamic executable"* ]]; then
        if grep -Eq '=>|ld-linux|libc\.so' <<<"$ldd_output"; then
            die "$LABEL is not static according to ldd:\n$ldd_output"
        fi
    fi
fi

echo "[runtime-portability] OK: $LABEL"
