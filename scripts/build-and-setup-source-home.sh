#!/usr/bin/env bash
# Stage trusted local media executables, then use the canonical installer.
set -euo pipefail
umask 077

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
media_source="${SETUP_SOURCE_HOME_MEDIA_TOOLS_DIR:-}"
check_tools=0

usage() {
    cat <<'EOF'
Usage: scripts/build-and-setup-source-home.sh [options]

  --media-tools-dir PATH   Trusted installed ffmpeg/ffprobe directory.
                          Default: SETUP_SOURCE_HOME_MEDIA_TOOLS_DIR, then PATH.
  --check-tools            Check private copies, then exit before build/setup.
  --help                  Show this help.

Copies the tools (including Homebrew symlink targets) into a temporary private
directory, checks that they run, and invokes setup-source-home.sh. Copies are
removed on exit; setup imports its own persistent copies. Homebrew libraries
remain dependencies of Homebrew executables.

Defaults to isolated collaboration. Set ELASTOS_COLLABORATION_STARTUP_MODE and
the canonical setup variables to select another configuration. HOME selects
the macOS installation home; Linux also supports XDG_DATA_HOME.

Builds and installs only. Browser TURN startup is disabled for this operation.
Use the platform source-home restart script separately after setup succeeds.
EOF
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --media-tools-dir)
            [[ -n "${2:-}" ]] || { echo '--media-tools-dir requires a path' >&2; exit 2; }
            media_source="$2"
            shift 2
            ;;
        --check-tools) check_tools=1; shift ;;
        --help|-h) usage; exit 0 ;;
        *) echo "Unknown argument: $1" >&2; usage >&2; exit 2 ;;
    esac
done

# Runtime checks every ancestor of imported tools. Shared /tmp is writable by
# other users, so stage inside the selected home, including for Linux setup.
mkdir -p "$HOME"
stage="$(mktemp -d "${HOME}/.elastos-media-tools.XXXXXX")"
trap 'rm -rf "$stage"' EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
chmod 700 "$stage"

for tool in ffmpeg ffprobe; do
    if [[ -n "$media_source" ]]; then
        source_path="${media_source}/${tool}"
    else
        source_path="$(command -v "$tool" || true)"
    fi
    if [[ ! -f "$source_path" || ! -x "$source_path" ]]; then
        echo "Missing executable ${tool}: install FFmpeg or use --media-tools-dir PATH." >&2
        exit 1
    fi
    echo "[build-and-setup] copy ${tool} from ${source_path}"
    # install follows the source symlink and creates a regular private file.
    install -m 700 "$source_path" "${stage}/${tool}"
    "${stage}/${tool}" -version >/dev/null
done

if [[ "$check_tools" == 1 ]]; then
    echo '[build-and-setup] media tool checks passed; temporary copies will be removed'
    exit 0
fi

export SETUP_SOURCE_HOME_MEDIA_TOOLS_DIR="$stage"
export ELASTOS_COLLABORATION_STARTUP_MODE="${ELASTOS_COLLABORATION_STARTUP_MODE:-isolated}"
export SETUP_SOURCE_HOME_RUNTIME_TURN=0
/bin/bash "${ROOT}/scripts/setup-source-home.sh"

echo '[build-and-setup] setup complete; start Home separately:'
case "$(uname -s)" in
    Darwin) printf '  %q --test-home %q\n' "${ROOT}/scripts/mac-source-home-restart.sh" "$HOME" ;;
    Linux) printf '  %q --help\n' "${ROOT}/scripts/linux-source-home-restart.sh" ;;
esac
