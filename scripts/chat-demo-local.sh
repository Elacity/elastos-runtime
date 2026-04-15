#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DEMO_HOME="${ELASTOS_DEMO_HOME:-/tmp/elastos-chat-demo}"
SKIP_BUILD=0
NICK="${ELASTOS_CHAT_NICK:-demo}"

usage() {
    cat <<'EOF'
Usage:
  bash scripts/chat-demo-local.sh
  bash scripts/chat-demo-local.sh --skip-build
  bash scripts/chat-demo-local.sh --home /tmp/elastos-chat-demo
  bash scripts/chat-demo-local.sh --nick demo

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

echo "[chat-demo-local] prepare local demo home"
bash "$ROOT/scripts/pc2-demo-local.sh" "${PREP_ARGS[@]}"

if [[ ! -e /dev/kvm ]]; then
    echo "[chat-demo-local] /dev/kvm is not available on this host." >&2
    echo "[chat-demo-local] Full-screen chat microVM proof requires a KVM-capable Linux host." >&2
    exit 2
fi

echo
echo "[chat-demo-local] launch full-screen chat microVM"
HOME="$DEMO_HOME" \
XDG_DATA_HOME="$DEMO_HOME/xdg-data" \
ELASTOS_DATA_DIR="$DEMO_HOME/xdg-data/elastos" \
"$ROOT/elastos/target/debug/elastos" capsule chat --lifecycle interactive --interactive --config "{\"nick\":\"$NICK\"}"
