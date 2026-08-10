#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PUBLISHER_GATEWAY="${ELASTOS_PUBLISHER_GATEWAY:-https://elastos.elacitylabs.com}"
MAINTAINER_DID="${ELASTOS_MAINTAINER_DID:-did:key:z6MkrFPDgDi98Ek6AFHM3VT9bVJytnDf5mfHAV6gyrD5frYj}"
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

usage() {
    cat <<'EOF'
Usage:
  bash scripts/home-demo-local.sh
  bash scripts/home-demo-local.sh --prepare-only
  bash scripts/home-demo-local.sh --skip-build
  bash scripts/home-demo-local.sh --home /tmp/elastos-demo-fixed

What it does:
  1. Builds the repo-local elastos binary and Home CLI component (unless --skip-build)
  2. Installs into a clean temp home using the canonical maintainer DID + gateway
  3. Generates a local override manifest for the current demo profile
  4. Reuses host-installed `crosvm` / `vmlinux` when available
  5. Runs `setup --profile demo`
  6. Stages a tiny MyWebSite demo page
  7. Launches repo-local `elastos` into Home (unless --prepare-only)

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

echo "[home-demo-local] root:      $ROOT"
echo "[home-demo-local] home:      $DEMO_HOME"
echo "[home-demo-local] xdg-data:  $XDG_DATA_HOME"
echo "[home-demo-local] host-home: $HOST_HOME"

if [[ -z "$HOST_SOURCE_CONNECT_TICKET" || -z "$HOST_PUBLISHER_NODE_ID" ]]; then
    echo "[home-demo-local] load canonical trusted-source bootstrap"
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
    echo "[home-demo-local] missing live trusted-source Carrier bootstrap in host sources." >&2
    echo "[home-demo-local] refresh the canonical install first, then rerun this proof." >&2
    exit 1
fi

if [[ "$SKIP_BUILD" -eq 0 ]]; then
    echo "[home-demo-local] build elastos binary"
    host_cargo build --manifest-path "$ROOT/elastos/Cargo.toml" -p elastos-server

    echo "[home-demo-local] build Home CLI renderer"
    host_cargo build --manifest-path "$ROOT/capsules/home-cli/Cargo.toml" --release --bin home-cli
fi

echo "[home-demo-local] install clean temp-home runtime"
HOME="$DEMO_HOME" \
XDG_DATA_HOME="$XDG_DATA_HOME" \
ELASTOS_DATA_DIR="$ELASTOS_DATA_DIR" \
ELASTOS_PUBLISHER_GATEWAY="$PUBLISHER_GATEWAY" \
ELASTOS_MAINTAINER_DID="$MAINTAINER_DID" \
ELASTOS_SOURCE_CONNECT_TICKET="$HOST_SOURCE_CONNECT_TICKET" \
ELASTOS_PUBLISHER_NODE_ID="$HOST_PUBLISHER_NODE_ID" \
ELASTOS_IPNS_NAME="$HOST_IPNS_NAME" \
bash "$ROOT/scripts/install.sh"

echo "[home-demo-local] stage host KVM assets when available"
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

echo "[home-demo-local] write override components manifest"
LOCAL_MANIFEST="$DEMO_HOME/components.local.json"
export HOST_CROSVM_PATH HOST_CROSVM_SHA HOST_CROSVM_SIZE
export HOST_VMLINUX_PATH HOST_VMLINUX_SHA HOST_VMLINUX_SIZE
python3 - "$ROOT/components.json" "$XDG_DATA_HOME/elastos/components.json" "$LOCAL_MANIFEST" \
    "$SETUP_PLATFORM" <<'PY'
import copy
import json
import os
import sys

source_path, installed_path, output_path, setup_platform = sys.argv[1:5]

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

echo "[home-demo-local] setup demo profile"
HOME="$DEMO_HOME" \
XDG_DATA_HOME="$XDG_DATA_HOME" \
ELASTOS_DATA_DIR="$ELASTOS_DATA_DIR" \
"$ROOT/elastos/target/debug/elastos" setup --profile demo

mkdir -p "$SITE_SRC"
cat > "$SITE_SRC/index.html" <<'HTML'
<!doctype html>
<html>
  <head><meta charset="utf-8"><title>Home Demo</title></head>
  <body><h1>Home Demo Site</h1></body>
</html>
HTML

echo "[home-demo-local] stage demo site"
HOME="$DEMO_HOME" \
XDG_DATA_HOME="$XDG_DATA_HOME" \
ELASTOS_DATA_DIR="$ELASTOS_DATA_DIR" \
"$ROOT/elastos/target/debug/elastos" site stage "$SITE_SRC"

echo
echo "[home-demo-local] ready"
echo "  home: $DEMO_HOME"
echo "  installed manifest: $XDG_DATA_HOME/elastos/components.json"
echo "  rerun elastos manually with:"
echo "    HOME=\"$DEMO_HOME\" XDG_DATA_HOME=\"$XDG_DATA_HOME\" \"$ROOT/elastos/target/debug/elastos\""

if [[ "$LAUNCH" -eq 1 ]]; then
    echo
    echo "[home-demo-local] launch Home"
    HOME="$DEMO_HOME" \
    XDG_DATA_HOME="$XDG_DATA_HOME" \
    ELASTOS_DATA_DIR="$ELASTOS_DATA_DIR" \
    "$ROOT/elastos/target/debug/elastos"
fi
