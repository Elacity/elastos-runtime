#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DEMO_HOME="${ELASTOS_DEMO_HOME:-/tmp/elastos-irc-demo}"
SKIP_BUILD=0
NICK="${ELASTOS_IRC_NICK:-demo}"

usage() {
    cat <<'EOF'
Usage:
  bash scripts/irc-demo-local.sh
  bash scripts/irc-demo-local.sh --skip-build
  bash scripts/irc-demo-local.sh --home /tmp/elastos-irc-demo
  bash scripts/irc-demo-local.sh --nick demo

What it does:
  1. Prepares a clean local temp-home via pc2-demo-local.sh
  2. Requires a KVM-capable host
  3. Launches repo-local `elastos capsule chat --lifecycle interactive --interactive`
EOF
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --skip-build)
            SKIP_BUILD=1
            shift
            ;;
        --home)
            [[ -n "${2:-}" ]] || { echo "Usage: --home /path" >&2; exit 1; }
            DEMO_HOME="$2"
            shift 2
            ;;
        --nick)
            [[ -n "${2:-}" ]] || { echo "Usage: --nick demo" >&2; exit 1; }
            NICK="$2"
            shift 2
            ;;
        --help|-h)
            usage
            exit 0
            ;;
        *)
            echo "Unknown argument: $1" >&2
            usage >&2
            exit 1
            ;;
    esac
done

PREP_ARGS=(--prepare-only --home "$DEMO_HOME")
if [[ "$SKIP_BUILD" -eq 1 ]]; then
    PREP_ARGS+=(--skip-build)
fi

echo "[irc-demo-local] prepare local demo home"
bash "$ROOT/scripts/pc2-demo-local.sh" "${PREP_ARGS[@]}"

if [[ ! -e /dev/kvm ]]; then
    echo "[irc-demo-local] /dev/kvm is not available on this host." >&2
    echo "[irc-demo-local] IRC microVM proof requires a KVM-capable Linux host." >&2
    exit 2
fi

echo
echo "[irc-demo-local] launch IRC microVM chat"
HOME="$DEMO_HOME" \
XDG_DATA_HOME="$DEMO_HOME/xdg-data" \
ELASTOS_DATA_DIR="$DEMO_HOME/xdg-data/elastos" \
"$ROOT/elastos/target/debug/elastos" capsule chat --lifecycle interactive --interactive --config "{\"nick\":\"$NICK\"}"
