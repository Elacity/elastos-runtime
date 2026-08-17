#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DATA_DIR="${ELASTOS_DATA_DIR:-${XDG_DATA_HOME:-$HOME/.local/share}/elastos}"
COMPONENTS_JSON="${ELASTOS_COMPONENTS_JSON:-$DATA_DIR/components.json}"

usage() {
    cat <<EOF
Usage: $(basename "$0") [component ...]

Verifies installed external component binaries against the installed
components.json manifest. With no component arguments, verifies every installed
external component that has an install_path and binary present.

Environment:
  ELASTOS_DATA_DIR         Installed runtime data dir. Default: \$XDG_DATA_HOME/elastos
  ELASTOS_COMPONENTS_JSON  Manifest path. Default: \$ELASTOS_DATA_DIR/components.json
  ELASTOS_SETUP_PLATFORM   Platform key override, e.g. linux-amd64
EOF
}

need_cmd() {
    command -v "$1" >/dev/null 2>&1 || {
        echo "[installed-provider-verify] missing required command: $1" >&2
        exit 2
    }
}

detect_platform() {
    local os arch
    case "$(uname -s)" in
        Linux) os="linux" ;;
        Darwin) os="macos" ;;
        *) os="$(uname -s | tr '[:upper:]' '[:lower:]')" ;;
    esac
    case "$(uname -m)" in
        x86_64 | amd64) arch="amd64" ;;
        aarch64 | arm64) arch="arm64" ;;
        *) arch="$(uname -m)" ;;
    esac
    printf '%s-%s\n' "$os" "$arch"
}

platform_alias() {
    case "$1" in
        linux-amd64) echo "x86_64-linux" ;;
        linux-arm64) echo "aarch64-linux" ;;
        macos-arm64) echo "darwin-arm64" ;;
        darwin-arm64) echo "macos-arm64" ;;
        x86_64-linux) echo "linux-amd64" ;;
        aarch64-linux) echo "linux-arm64" ;;
        *) echo "" ;;
    esac
}

checksum_for_file() {
    local path="$1"
    local expected="$2"

    case "$expected" in
        sha256:*) printf 'sha256:%s\n' "$(sha256sum "$path" | awk '{print $1}')" ;;
        sha512:*) printf 'sha512:%s\n' "$(sha512sum "$path" | awk '{print $1}')" ;;
        *)
            echo "[installed-provider-verify] unsupported checksum format for $path: $expected" >&2
            return 2
            ;;
    esac
}

verify_component() {
    local name="$1"
    local explicit="$2"
    local info install_path checksum extract_path strategy source binary actual
    VERIFY_COMPONENT_VERDICT="skipped"

    info="$(
        jq -cer \
            --arg name "$name" \
            --arg platform "$PLATFORM" \
            --arg alias "$PLATFORM_ALIAS" \
            '
              .external[$name] as $component
              | if $component == null then empty else
                  ($component.platforms[$platform] // $component.platforms[$alias] // $component.platforms["*"] // {}) as $platform_info
                  | {
                      install_path: ($platform_info.install_path // $component.install_path // ""),
                      checksum: ($platform_info.checksum // ""),
                      extract_path: ($platform_info.extract_path // ""),
                      strategy: ($platform_info.strategy // ""),
                      source: ($platform_info.source // "")
                  }
                end
            ' "$COMPONENTS_JSON" 2>/dev/null || true
    )"

    if [[ -z "$info" ]]; then
        echo "[installed-provider-verify] missing external component: $name" >&2
        return 1
    fi

    install_path="$(jq -r '.install_path' <<<"$info")"
    checksum="$(jq -r '.checksum' <<<"$info")"
    extract_path="$(jq -r '.extract_path' <<<"$info")"
    strategy="$(jq -r '.strategy' <<<"$info")"
    source="$(jq -r '.source' <<<"$info")"

    if [[ -z "$install_path" ]]; then
        if [[ "$explicit" == "1" ]]; then
            echo "[installed-provider-verify] $name has no install_path" >&2
            return 1
        fi
        return 0
    fi

    binary="$DATA_DIR/$install_path"
    if [[ ! -f "$binary" ]]; then
        if [[ "$explicit" == "1" ]]; then
            echo "[installed-provider-verify] missing installed binary for $name: $binary" >&2
            return 1
        fi
        return 0
    fi

    if [[ -z "$checksum" ]]; then
        if [[ "$strategy" == "local-copy" ]]; then
            echo "[installed-provider-verify] skip: $name uses local-copy without manifest checksum (${source:-unknown source})"
            return 0
        fi
        echo "[installed-provider-verify] missing checksum for installed $name in $COMPONENTS_JSON" >&2
        return 1
    fi

    if [[ -n "$extract_path" ]]; then
        echo "[installed-provider-verify] skip: $name uses archive checksum before extraction ($checksum)"
        return 0
    fi

    actual="$(checksum_for_file "$binary" "$checksum")"
    local actual_lower checksum_lower
    actual_lower="$(printf '%s' "$actual" | tr '[:upper:]' '[:lower:]')"
    checksum_lower="$(printf '%s' "$checksum" | tr '[:upper:]' '[:lower:]')"
    if [[ "$actual_lower" != "$checksum_lower" ]]; then
        echo "[installed-provider-verify] checksum mismatch for $name" >&2
        echo "  binary:   $binary" >&2
        echo "  expected: $checksum" >&2
        echo "  actual:   $actual" >&2
        return 1
    fi

    echo "[installed-provider-verify] ok: $name ($checksum)"
    VERIFY_COMPONENT_VERDICT="verified"
}

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
    usage
    exit 0
fi

need_cmd jq
need_cmd awk
if ! command -v sha256sum >/dev/null 2>&1; then
    sha256sum() { shasum -a 256 "$@"; }
fi
if ! command -v sha512sum >/dev/null 2>&1; then
    sha512sum() { shasum -a 512 "$@"; }
fi
need_cmd sha256sum
need_cmd sha512sum

if [[ ! -f "$COMPONENTS_JSON" ]]; then
    echo "[installed-provider-verify] components.json not found: $COMPONENTS_JSON" >&2
    exit 1
fi

PLATFORM="${ELASTOS_SETUP_PLATFORM:-$(detect_platform)}"
PLATFORM_ALIAS="$(platform_alias "$PLATFORM")"

cd "$ROOT"

if [[ "$#" -gt 0 ]]; then
    for component in "$@"; do
        verify_component "$component" "1"
    done
else
    verified=0
    while IFS= read -r component; do
        if verify_component "$component" "0"; then
            binary_path="$(
                jq -r \
                    --arg name "$component" \
                    --arg platform "$PLATFORM" \
                    --arg alias "$PLATFORM_ALIAS" \
                    '.external[$name] as $component
                     | ($component.platforms[$platform] // $component.platforms[$alias] // $component.platforms["*"] // {}) as $platform_info
                     | ($platform_info.install_path // $component.install_path // "")' \
                    "$COMPONENTS_JSON"
            )"
            if [[ "$VERIFY_COMPONENT_VERDICT" == "verified" && -n "$binary_path" && -f "$DATA_DIR/$binary_path" ]]; then
                verified=$((verified + 1))
            fi
        else
            exit 1
        fi
    done < <(jq -r '.external | keys[]' "$COMPONENTS_JSON")
    echo "[installed-provider-verify] verified $verified installed component(s)"
fi
