#!/usr/bin/env bash
# Install a source Home built for iteration rather than for running.
#
#   ./scripts/setup-dev-home.sh --mode isolated
#
# This is `setup-source-home.sh` with one thing changed: every crate is built
# with the dev profile instead of release. It is deliberately a wrapper rather
# than a second installer -- what "install a Home" means lives in one script,
# and a copy of nineteen hundred lines would drift from it within a week.
#
# Two consequences worth knowing before using it:
#
#   - The Home is SLOWER where it hurts. Custody and the crypto paths are the
#     longest work this Home does; unoptimised they are painful rather than
#     merely slower. Use release for anything about minting, opening or chain
#     behaviour, and this for UI work.
#
#   - The free-space gate is lowered. The full gate reserves sixteen GiB for a
#     cold release build of everything; this build reuses what the editor
#     already compiled and needs a fraction of that, and a gate that stops an
#     iteration loop over headroom it never uses gets bypassed rather than
#     heeded. It is lowered rather than removed: running out of disk mid-install
#     leaves a half-written Home, which is worse than being told to free space.
#
#   - The gateway artifact is shared with the VS Code launch. That task builds
#     with `profile.dev.package.*.debug=2`, and those overrides change the
#     fingerprint -- so the same overrides are passed here. Without them cargo
#     would keep a third copy rather than reuse the launch's.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
mode=""
passthrough=()

while [[ $# -gt 0 ]]; do
    case "$1" in
        --mode)
            [[ $# -ge 2 ]] || { echo "--mode needs a value (configured|isolated)" >&2; exit 2; }
            mode="$2"
            shift 2
            ;;
        --mode=*)
            mode="${1#--mode=}"
            shift
            ;;
        -h|--help)
            sed -n '2,24p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
            exit 0
            ;;
        *)
            passthrough+=("$1")
            shift
            ;;
    esac
done

# The installer validates this too; refusing here means the answer arrives
# before a build rather than after one.
mode="${mode:-${ELASTOS_COLLABORATION_STARTUP_MODE:-}}"
case "${mode}" in
    configured|isolated) ;;
    "")
        echo "usage: $(basename "${BASH_SOURCE[0]}") --mode (configured|isolated)" >&2
        exit 2
        ;;
    *)
        echo "--mode must be configured or isolated, got: ${mode}" >&2
        exit 2
        ;;
esac

# The VS Code launch's own overrides, so both builds land on one artifact.
# Kept in step with `.vscode/launch.json`'s build task.
debug_overrides=(
    --config profile.dev.package.elastos-server.debug=2
    --config profile.dev.package.elastos-runtime.debug=2
    --config profile.dev.package.elastos-protected-content-runtime.debug=2
    --config profile.dev.package.elastos-protected-content-custody.debug=2
    --config profile.dev.package.elastos-protected-content-contracts.debug=2
    --config profile.dev.package.elastos-protected-content-rights.debug=2
    --config profile.dev.package.elastos-protected-content-provider-contracts.debug=2
    --config profile.dev.package.custody-provider.debug=2
)

echo "[setup-dev-home] dev profile, mode=${mode}"
echo "[setup-dev-home] a dev Home is slow at custody and crypto; use setup-source-home.sh for chain work"

exec env \
    ELASTOS_COLLABORATION_STARTUP_MODE="${mode}" \
    SOURCE_HOME_MIN_FREE_GIB="${SOURCE_HOME_MIN_FREE_GIB:-4}" \
    SOURCE_HOME_CARGO_PROFILE=dev \
    SOURCE_HOME_CARGO_EXTRA_ARGS="${debug_overrides[*]}" \
    "${here}/setup-source-home.sh" ${passthrough+"${passthrough[@]}"}
