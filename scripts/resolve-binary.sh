#!/bin/bash
# Resolve elastos binary locations, respecting custom cargo target-dir.
# Source this from other scripts:
#   source "$(dirname "${BASH_SOURCE[0]}")/resolve-binary.sh"
#
# Exposes:
#   REPO_ELASTOS_BIN        canonical repo binary path
#   INSTALLED_ELASTOS_BIN   canonical installed binary path
#   ELASTOS_BIN             legacy "repo-or-installed" compatibility value
#   ELASTOS_BIN_SOURCE      source label for ELASTOS_BIN
#
# Root repo launchers should choose explicitly:
#   - repo binary by default
#   - installed binary only behind an explicit flag
# Do not rely on ELASTOS_BIN for canonical launcher behavior.

PROJECT_ROOT="${PROJECT_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
REPO_ELASTOS_BIN="${PROJECT_ROOT}/elastos/target/release/elastos"
INSTALLED_ELASTOS_BIN="${HOME}/.local/bin/elastos"

if [[ -f "${PROJECT_ROOT}/elastos/.cargo/config.toml" ]]; then
    cargo_target_dir=$(
        {
            grep -E '^\s*target-dir\s*=' "${PROJECT_ROOT}/elastos/.cargo/config.toml" || true
        } | head -1 | sed 's/.*=\s*"\(.*\)"/\1/' | sed "s|.*=\s*'\(.*\)'|\1|" | tr -d ' '
    )
    if [[ -n "${cargo_target_dir:-}" ]]; then
        REPO_ELASTOS_BIN="${cargo_target_dir}/release/elastos"
    fi
fi

_resolve_elastos_binary() {
    for candidate in \
        "${REPO_ELASTOS_BIN}" \
        "${INSTALLED_ELASTOS_BIN}"; do
        if [[ -n "$candidate" && -x "$candidate" ]]; then
            echo "$candidate"
            return 0
        fi
    done
    return 1
}

_elastos_bin_version() {
    local bin="$1"
    "$bin" --version 2>/dev/null | head -1
}

resolve_elastos_binary() {
    local mode="${1:-either}"
    case "$mode" in
        repo)
            [[ -x "$REPO_ELASTOS_BIN" ]] && echo "$REPO_ELASTOS_BIN"
            ;;
        installed)
            [[ -x "$INSTALLED_ELASTOS_BIN" ]] && echo "$INSTALLED_ELASTOS_BIN"
            ;;
        either)
            _resolve_elastos_binary
            ;;
        *)
            echo "unknown resolve_elastos_binary mode: $mode" >&2
            return 2
            ;;
    esac
}

elastos_bin_source() {
    local bin="${1:-}"
    case "$bin" in
        "$REPO_ELASTOS_BIN") echo "repo" ;;
        "$INSTALLED_ELASTOS_BIN") echo "installed" ;;
        "") echo "missing" ;;
        *) echo "custom" ;;
    esac
}

describe_elastos_binary() {
    local bin="${1:-}"
    local source
    source="$(elastos_bin_source "$bin")"
    if [[ -x "$bin" ]]; then
        printf '%s -> %s (%s)\n' "$source" "$bin" "$(_elastos_bin_version "$bin")"
    else
        printf '%s -> %s\n' "$source" "${bin:-missing}"
    fi
}

ELASTOS_BIN="$(resolve_elastos_binary either || true)"
ELASTOS_BIN_SOURCE="$(elastos_bin_source "$ELASTOS_BIN")"
