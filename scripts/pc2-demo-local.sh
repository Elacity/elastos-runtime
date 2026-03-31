#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PUBLISHER_GATEWAY="${ELASTOS_PUBLISHER_GATEWAY:-https://elastos.elacitylabs.com}"
MAINTAINER_DID="${ELASTOS_MAINTAINER_DID:-did:key:z6Mkf2nCJ1pcN4JioAxHEiyDsPC298QFtn2Dgg9tjt2ezHeK}"
LAUNCH=1
SKIP_BUILD=0
DEMO_HOME="${ELASTOS_DEMO_HOME:-}"
HOST_HOME="${ELASTOS_HOST_HOME:-$(getent passwd "$(id -un)" | cut -d: -f6)}"
LOCAL_MANIFEST=""
HOST_SOURCE_CONNECT_TICKET="${ELASTOS_SOURCE_CONNECT_TICKET:-}"
HOST_PUBLISHER_NODE_ID="${ELASTOS_PUBLISHER_NODE_ID:-}"
HOST_IPNS_NAME="${ELASTOS_IPNS_NAME:-}"

host_cargo() {
    HOME="$HOST_HOME" cargo "$@"
}

host_shell() {
    HOME="$HOST_HOME" "$@"
}

usage() {
    cat <<'EOF'
Usage:
  bash scripts/pc2-demo-local.sh
  bash scripts/pc2-demo-local.sh --prepare-only
  bash scripts/pc2-demo-local.sh --skip-build
  bash scripts/pc2-demo-local.sh --home /tmp/elastos-demo-fixed

What it does:
  1. Builds the repo-local elastos binary and pc2.wasm (unless --skip-build)
  2. Builds a local IRC microVM chat bundle and stages it into the temp home
  3. Installs into a clean temp home using the canonical maintainer DID + gateway
  4. Generates a local override manifest so `setup --profile demo` and
     `setup --profile irc` can use the current source demo profile plus the
     locally staged IRC bundle
  5. Reuses host-installed `crosvm` / `vmlinux` when available
  6. Runs `setup --profile demo` and `setup --profile irc`
  7. Stages a tiny MyWebSite demo page
  8. Launches repo-local `elastos` into PC2 (unless --prepare-only)

When it finishes preparing, it prints the demo HOME path so you can reuse it.
EOF
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --prepare-only)
            LAUNCH=0
            shift
            ;;
        --skip-build)
            SKIP_BUILD=1
            shift
            ;;
        --home)
            [[ -n "${2:-}" ]] || { echo "Usage: --home /path" >&2; exit 1; }
            DEMO_HOME="$2"
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
    DEMO_HOME="$(mktemp -d /tmp/elastos-demo-XXXXXX)"
else
    mkdir -p "$DEMO_HOME"
fi

XDG_DATA_HOME="${DEMO_HOME}/xdg-data"
ELASTOS_DATA_DIR="${XDG_DATA_HOME}/elastos"
SITE_SRC="${DEMO_HOME}/demo-site"

case "$(uname -m)" in
    x86_64)
        CAPSULE_PLATFORM="x86_64-linux"
        SETUP_PLATFORM="linux-amd64"
        ;;
    aarch64|arm64)
        CAPSULE_PLATFORM="aarch64-linux"
        SETUP_PLATFORM="linux-arm64"
        ;;
    *)
        echo "Unsupported architecture: $(uname -m)" >&2
        exit 1
        ;;
esac

echo "[pc2-demo-local] root:      $ROOT"
echo "[pc2-demo-local] home:      $DEMO_HOME"
echo "[pc2-demo-local] xdg-data:  $XDG_DATA_HOME"
echo "[pc2-demo-local] host-home: $HOST_HOME"

if [[ -z "$HOST_SOURCE_CONNECT_TICKET" || -z "$HOST_PUBLISHER_NODE_ID" ]]; then
    echo "[pc2-demo-local] load canonical trusted-source bootstrap"
    mapfile -t _source_fields < <(
        python3 - "$HOST_HOME/.local/share/elastos/sources.json" <<'PY'
import json
import sys

path = sys.argv[1]
with open(path, "r", encoding="utf-8") as f:
    data = json.load(f)

default_name = data.get("default_source", "default")
source = None
for entry in data.get("sources", []):
    if entry.get("name") == default_name:
        source = entry
        break

if source is None:
    raise SystemExit("default trusted source not found")

print(source.get("connect_ticket", ""))
print(source.get("publisher_node_id", ""))
print(source.get("ipns_name", ""))
PY
    )
    HOST_SOURCE_CONNECT_TICKET="${HOST_SOURCE_CONNECT_TICKET:-${_source_fields[0]:-}}"
    HOST_PUBLISHER_NODE_ID="${HOST_PUBLISHER_NODE_ID:-${_source_fields[1]:-}}"
    HOST_IPNS_NAME="${HOST_IPNS_NAME:-${_source_fields[2]:-}}"
fi

if [[ -z "$HOST_SOURCE_CONNECT_TICKET" || -z "$HOST_PUBLISHER_NODE_ID" ]]; then
    echo "[pc2-demo-local] missing live trusted-source Carrier bootstrap in host sources." >&2
    echo "[pc2-demo-local] refresh the canonical install first, then rerun this proof." >&2
    exit 1
fi

if [[ "$SKIP_BUILD" -eq 0 ]]; then
    echo "[pc2-demo-local] build elastos binary"
    host_cargo build --manifest-path "$ROOT/elastos/Cargo.toml" -p elastos-server

    echo "[pc2-demo-local] build pc2 wasm"
    host_cargo build --manifest-path "$ROOT/capsules/pc2/Cargo.toml" --target wasm32-wasip1 --release
    cp "$ROOT/capsules/pc2/target/wasm32-wasip1/release/pc2.wasm" "$ROOT/capsules/pc2/pc2.wasm"
fi

if [[ "$SKIP_BUILD" -eq 0 || ! -f "$ROOT/artifacts/chat.capsule.tar.gz" ]]; then
    echo "[pc2-demo-local] build IRC microVM bundle"
    host_shell bash "$ROOT/scripts/build/build-rootfs.sh" chat --output "$ROOT/artifacts"
fi

echo "[pc2-demo-local] install clean temp-home runtime"
HOME="$DEMO_HOME" \
XDG_DATA_HOME="$XDG_DATA_HOME" \
ELASTOS_DATA_DIR="$ELASTOS_DATA_DIR" \
ELASTOS_PUBLISHER_GATEWAY="$PUBLISHER_GATEWAY" \
ELASTOS_MAINTAINER_DID="$MAINTAINER_DID" \
ELASTOS_SOURCE_CONNECT_TICKET="$HOST_SOURCE_CONNECT_TICKET" \
ELASTOS_PUBLISHER_NODE_ID="$HOST_PUBLISHER_NODE_ID" \
ELASTOS_IPNS_NAME="$HOST_IPNS_NAME" \
bash "$ROOT/scripts/install.sh"

echo "[pc2-demo-local] stage local IRC bundle"
CHAT_ARTIFACT_RAW="$ROOT/artifacts/chat.capsule.tar.gz"
CHAT_ARTIFACT_STAGE="$DEMO_HOME/.chat-artifact-stage"
CHAT_ARTIFACT="$DEMO_HOME/chat-${SETUP_PLATFORM}.tar.gz"
CHAT_RELEASE_PATH="chat-local-${SETUP_PLATFORM}.tar.gz"
HOST_PUBLISHER_ARTIFACTS="$HOST_HOME/.local/share/elastos/ElastOS/SystemServices/Publisher/artifacts"
rm -rf "$CHAT_ARTIFACT_STAGE"
mkdir -p "$CHAT_ARTIFACT_STAGE/chat"
tar -xzf "$CHAT_ARTIFACT_RAW" -C "$CHAT_ARTIFACT_STAGE/chat"
tar -czf "$CHAT_ARTIFACT" -C "$CHAT_ARTIFACT_STAGE" chat
mkdir -p "$HOST_PUBLISHER_ARTIFACTS"
cp "$CHAT_ARTIFACT" "$HOST_PUBLISHER_ARTIFACTS/$CHAT_RELEASE_PATH"
CHAT_SHA="$(sha256sum "$CHAT_ARTIFACT" | awk '{print $1}')"
CHAT_SIZE="$(stat -c '%s' "$CHAT_ARTIFACT")"
CHAT_CID="local-chat-${CHAT_SHA:0:16}"
CHAT_DIR="$XDG_DATA_HOME/elastos/capsules/chat"
mkdir -p "$XDG_DATA_HOME/elastos/capsules"
rm -rf "$CHAT_DIR"
mkdir -p "$CHAT_DIR"
tar -xzf "$CHAT_ARTIFACT" -C "$CHAT_DIR"
printf '%s\n' "$CHAT_CID" > "$CHAT_DIR/.elastos-cid"
printf '%s\n' "$CHAT_SHA" > "$CHAT_DIR/.elastos-artifact-sha256"

echo "[pc2-demo-local] stage host KVM assets when available"
mkdir -p "$XDG_DATA_HOME/elastos/bin"
HOST_CROSVM_PATH=""
HOST_CROSVM_SHA=""
HOST_CROSVM_SIZE=""
HOST_VMLINUX_PATH=""
HOST_VMLINUX_SHA=""
HOST_VMLINUX_SIZE=""
if [[ -f "$HOST_HOME/.local/share/elastos/bin/crosvm" ]]; then
    cp "$HOST_HOME/.local/share/elastos/bin/crosvm" "$XDG_DATA_HOME/elastos/bin/crosvm"
    HOST_CROSVM_PATH="$XDG_DATA_HOME/elastos/bin/crosvm"
    HOST_CROSVM_SHA="$(sha256sum "$HOST_CROSVM_PATH" | awk '{print $1}')"
    HOST_CROSVM_SIZE="$(stat -c '%s' "$HOST_CROSVM_PATH")"
fi
if [[ -f "$HOST_HOME/.local/share/elastos/bin/vmlinux" ]]; then
    cp "$HOST_HOME/.local/share/elastos/bin/vmlinux" "$XDG_DATA_HOME/elastos/bin/vmlinux"
    HOST_VMLINUX_PATH="$XDG_DATA_HOME/elastos/bin/vmlinux"
    HOST_VMLINUX_SHA="$(sha256sum "$HOST_VMLINUX_PATH" | awk '{print $1}')"
    HOST_VMLINUX_SIZE="$(stat -c '%s' "$HOST_VMLINUX_PATH")"
fi

echo "[pc2-demo-local] write override components manifest"
LOCAL_MANIFEST="$DEMO_HOME/components.local.json"
export HOST_CROSVM_PATH HOST_CROSVM_SHA HOST_CROSVM_SIZE
export HOST_VMLINUX_PATH HOST_VMLINUX_SHA HOST_VMLINUX_SIZE
python3 - "$ROOT/components.json" "$XDG_DATA_HOME/elastos/components.json" "$LOCAL_MANIFEST" \
    "$SETUP_PLATFORM" "$CAPSULE_PLATFORM" "$CHAT_CID" "$CHAT_SHA" "$CHAT_SIZE" <<'PY'
import copy
import json
import os
import sys

source_path, installed_path, output_path, setup_platform, capsule_platform, chat_cid, chat_sha, chat_size = sys.argv[1:9]
chat_size = int(chat_size)

with open(source_path, "r", encoding="utf-8") as f:
    data = json.load(f)

installed = {}
if os.path.exists(installed_path):
    with open(installed_path, "r", encoding="utf-8") as f:
        installed = json.load(f)

for name, source_component in data.get("external", {}).items():
    installed_component = (installed.get("external") or {}).get(name)
    if not installed_component:
        continue
    source_platforms = source_component.setdefault("platforms", {})
    for plat, plat_info in (installed_component.get("platforms") or {}).items():
        merged = copy.deepcopy(source_platforms.get(plat, {}))
        merged.update(plat_info)
        source_platforms[plat] = merged

data.setdefault("capsules", {})["chat"] = {
    "cid": chat_cid,
    "sha256": chat_sha,
    "size": chat_size,
    "platforms": [capsule_platform],
}

chat_component = data.setdefault("external", {}).setdefault("chat", {})
chat_component.setdefault("version", "0.1.0")
chat_component.setdefault("install_path", "capsules/chat")
chat_component.setdefault(
    "description",
    "Packaged IRC-style microVM chat bundle for the full-screen Carrier chat path",
)
chat_platforms = chat_component.setdefault("platforms", {})
chat_platforms[setup_platform] = {
    "cid": chat_cid,
    "checksum": f"sha256:{chat_sha}",
    "size": chat_size,
    "release_path": f"chat-local-{setup_platform}.tar.gz",
    "extract_path": "chat",
    "install_path": "capsules/chat",
}

for name, install_path_env, sha_env, size_env in (
    ("crosvm", "HOST_CROSVM_PATH", "HOST_CROSVM_SHA", "HOST_CROSVM_SIZE"),
    ("vmlinux", "HOST_VMLINUX_PATH", "HOST_VMLINUX_SHA", "HOST_VMLINUX_SIZE"),
):
    install_path = os.environ.get(install_path_env, "").strip()
    if not install_path:
        continue
    sha_value = os.environ.get(sha_env, "").strip()
    size_value = os.environ.get(size_env, "").strip()
    rel_install = os.path.relpath(install_path, os.path.dirname(installed_path))
    component = data.setdefault("external", {}).setdefault(name, {})
    platforms = component.setdefault("platforms", {})
    info = copy.deepcopy(platforms.get(setup_platform, {}))
    info["install_path"] = rel_install
    if sha_value:
        info["checksum"] = f"sha256:{sha_value}"
    if size_value:
        info["size"] = int(size_value)
    platforms[setup_platform] = info

with open(output_path, "w", encoding="utf-8") as f:
    json.dump(data, f, indent=2)
    f.write("\n")
PY
cp "$LOCAL_MANIFEST" "$XDG_DATA_HOME/elastos/components.json"

echo "[pc2-demo-local] setup demo profile"
HOME="$DEMO_HOME" \
XDG_DATA_HOME="$XDG_DATA_HOME" \
ELASTOS_DATA_DIR="$ELASTOS_DATA_DIR" \
"$ROOT/elastos/target/debug/elastos" setup --profile demo

echo "[pc2-demo-local] setup irc profile"
HOME="$DEMO_HOME" \
XDG_DATA_HOME="$XDG_DATA_HOME" \
ELASTOS_DATA_DIR="$ELASTOS_DATA_DIR" \
"$ROOT/elastos/target/debug/elastos" setup --profile irc

mkdir -p "$SITE_SRC"
cat > "$SITE_SRC/index.html" <<'HTML'
<!doctype html>
<html>
  <head><meta charset="utf-8"><title>PC2 Demo</title></head>
  <body><h1>PC2 Demo Site</h1></body>
</html>
HTML

echo "[pc2-demo-local] stage demo site"
HOME="$DEMO_HOME" \
XDG_DATA_HOME="$XDG_DATA_HOME" \
ELASTOS_DATA_DIR="$ELASTOS_DATA_DIR" \
"$ROOT/elastos/target/debug/elastos" site stage "$SITE_SRC"

echo
echo "[pc2-demo-local] ready"
echo "  home: $DEMO_HOME"
echo "  installed manifest: $XDG_DATA_HOME/elastos/components.json"
echo "  rerun elastos manually with:"
echo "    HOME=\"$DEMO_HOME\" XDG_DATA_HOME=\"$XDG_DATA_HOME\" \"$ROOT/elastos/target/debug/elastos\""

if [[ "$LAUNCH" -eq 1 ]]; then
    echo
    echo "[pc2-demo-local] launch PC2"
    HOME="$DEMO_HOME" \
    XDG_DATA_HOME="$XDG_DATA_HOME" \
    ELASTOS_DATA_DIR="$ELASTOS_DATA_DIR" \
    "$ROOT/elastos/target/debug/elastos"
fi
