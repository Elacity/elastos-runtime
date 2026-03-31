#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DEMO_HOME="${ELASTOS_CHAT_WASM_HOME:-}"
SKIP_BUILD=0
NICK="${ELASTOS_CHAT_WASM_NICK:-demo}"
ADDR="${ELASTOS_CHAT_WASM_ADDR:-}"
CONNECT="${ELASTOS_CHAT_WASM_CONNECT:-}"

usage() {
    cat <<'EOF'
Usage:
  bash scripts/chat-wasm-local.sh
  bash scripts/chat-wasm-local.sh --skip-build
  bash scripts/chat-wasm-local.sh --home /tmp/elastos-chat-wasm
  bash scripts/chat-wasm-local.sh --nick demo
  bash scripts/chat-wasm-local.sh --connect <ticket>
  bash scripts/chat-wasm-local.sh --addr 127.0.0.1:3100

What it does:
  1. Builds the repo-local operator runtime and chat-wasm artifact unless --skip-build
  2. Starts a repo-local operator runtime
  3. Launches the explicit WASM chat target from capsules/chat-wasm

This is the explicit local/dev path for the WASM IRC/chat target.
It is not the shipping IRC microVM path.
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
        --connect)
            [[ -n "${2:-}" ]] || { echo "Usage: --connect <ticket>" >&2; exit 1; }
            CONNECT="$2"
            shift 2
            ;;
        --addr)
            [[ -n "${2:-}" ]] || { echo "Usage: --addr 127.0.0.1:3100" >&2; exit 1; }
            ADDR="$2"
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

if [[ -z "$DEMO_HOME" ]]; then
    DEMO_HOME="$(mktemp -d /tmp/elastos-chat-wasm-XXXXXX)"
else
    mkdir -p "$DEMO_HOME"
fi

if [[ -z "$ADDR" ]]; then
    ADDR="127.0.0.1:$(python3 - <<'PY'
import socket
s = socket.socket()
s.bind(("127.0.0.1", 0))
print(s.getsockname()[1])
s.close()
PY
)"
fi

XDG_DATA_HOME="${DEMO_HOME}/xdg-data"
ELASTOS_DATA_DIR="${XDG_DATA_HOME}/elastos"
RUNTIME_COORDS="${ELASTOS_DATA_DIR}/runtime-coords.json"
SERVE_LOG="${DEMO_HOME}/serve.log"
LOCAL_COMPONENTS_MANIFEST="${ELASTOS_DATA_DIR}/components.json"

case "$(uname -m)" in
    x86_64)
        SETUP_PLATFORM="linux-amd64"
        ;;
    aarch64|arm64)
        SETUP_PLATFORM="linux-arm64"
        ;;
    *)
        echo "Unsupported architecture: $(uname -m)" >&2
        exit 1
        ;;
esac

cleanup() {
    if [[ -n "${SERVE_PID:-}" ]] && kill -0 "${SERVE_PID}" 2>/dev/null; then
        kill "${SERVE_PID}" 2>/dev/null || true
        wait "${SERVE_PID}" 2>/dev/null || true
    fi
}
trap cleanup EXIT

echo "[chat-wasm-local] root:      $ROOT"
echo "[chat-wasm-local] home:      $DEMO_HOME"
echo "[chat-wasm-local] xdg-data:  $XDG_DATA_HOME"
echo "[chat-wasm-local] runtime:   $ADDR"

if [[ "$SKIP_BUILD" -eq 0 ]]; then
    echo "[chat-wasm-local] build operator runtime and first-party dependencies"
    cargo build --manifest-path "$ROOT/elastos/Cargo.toml" -p elastos-server
    cargo build --manifest-path "$ROOT/elastos/capsules/shell/Cargo.toml" --release
    cargo build --manifest-path "$ROOT/elastos/capsules/localhost-provider/Cargo.toml" --release
    cargo build --manifest-path "$ROOT/capsules/did-provider/Cargo.toml" --release

    echo "[chat-wasm-local] build chat-wasm artifact"
    cargo build \
        --manifest-path "$ROOT/capsules/chat/Cargo.toml" \
        --bin chat-stdio \
        --target wasm32-wasip1 \
        --no-default-features \
        --release
    cp "$ROOT/capsules/chat/target/wasm32-wasip1/release/chat-stdio.wasm" \
        "$ROOT/capsules/chat-wasm/chat-stdio.wasm"
fi

echo "[chat-wasm-local] stage required host binaries into temp home"
mkdir -p "$ELASTOS_DATA_DIR/bin"
cp "$ROOT/elastos/target/release/shell" \
    "$ELASTOS_DATA_DIR/bin/shell"
cp "$ROOT/elastos/target/release/localhost-provider" \
    "$ELASTOS_DATA_DIR/bin/localhost-provider"
cp "$ROOT/capsules/did-provider/target/release/did-provider" \
    "$ELASTOS_DATA_DIR/bin/did-provider"

echo "[chat-wasm-local] write local components manifest"
ROOT_COMPONENTS_JSON="$ROOT/components.json" \
LOCAL_COMPONENTS_MANIFEST="$LOCAL_COMPONENTS_MANIFEST" \
SETUP_PLATFORM="$SETUP_PLATFORM" \
python3 - <<'PY'
import hashlib
import json
import os
import pathlib

source = pathlib.Path(os.environ["ROOT_COMPONENTS_JSON"])
dest = pathlib.Path(os.environ["LOCAL_COMPONENTS_MANIFEST"])
platform = os.environ["SETUP_PLATFORM"]
data_dir = dest.parent

manifest = json.loads(source.read_text())
for name in ("shell", "localhost-provider", "did-provider"):
    path = data_dir / "bin" / name
    if not path.is_file():
        raise SystemExit(f"missing staged binary for {name}: {path}")
    blob = path.read_bytes()
    info = manifest["external"][name]["platforms"][platform]
    info["install_path"] = f"bin/{name}"
    info["checksum"] = "sha256:" + hashlib.sha256(blob).hexdigest()
    info["size"] = len(blob)

dest.parent.mkdir(parents=True, exist_ok=True)
dest.write_text(json.dumps(manifest, indent=2) + "\n")
PY

echo "[chat-wasm-local] start local operator runtime"
HOME="$DEMO_HOME" \
XDG_DATA_HOME="$XDG_DATA_HOME" \
ELASTOS_DATA_DIR="$ELASTOS_DATA_DIR" \
"$ROOT/elastos/target/debug/elastos" serve --addr "$ADDR" >"$SERVE_LOG" 2>&1 &
SERVE_PID=$!

for _ in $(seq 1 60); do
    if [[ -f "$RUNTIME_COORDS" ]]; then
        break
    fi
    sleep 0.5
done

if [[ ! -f "$RUNTIME_COORDS" ]]; then
    echo "[chat-wasm-local] runtime-coords.json missing. See $SERVE_LOG" >&2
    exit 1
fi

echo "[chat-wasm-local] launch explicit WASM chat target"
RUN_CMD=(
    "$ROOT/elastos/target/debug/elastos"
    run
    "$ROOT/capsules/chat-wasm"
    --
    --nick
    "$NICK"
)
if [[ -n "$CONNECT" ]]; then
    RUN_CMD+=(--connect "$CONNECT")
fi

HOME="$DEMO_HOME" \
XDG_DATA_HOME="$XDG_DATA_HOME" \
ELASTOS_DATA_DIR="$ELASTOS_DATA_DIR" \
"${RUN_CMD[@]}"
