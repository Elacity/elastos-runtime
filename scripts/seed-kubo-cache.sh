#!/usr/bin/env bash
# Seed an installed Kubo into a Runtime data dir from a local tarball cache so
# `elastos setup` treats the component as already installed and never reaches
# for dist.ipfs.tech (a single 600s download attempt with no retry, which
# intermittently times out on GitHub macOS runners — ELACITY-2308).
#
# Usage: seed-kubo-cache.sh <cache-dir> <data-dir> <platform>
#   cache-dir  Directory holding (or receiving) the verified release tarball.
#   data-dir   Runtime data dir to seed (bin/kubo is written under it).
#   platform   components.json platform key, e.g. darwin-arm64, linux-arm64.
set -euo pipefail
# Runtime validators require owner-only data-root directories.
umask 077

if [[ $# -ne 3 ]]; then
    echo "usage: $0 <cache-dir> <data-dir> <platform>" >&2
    exit 2
fi

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CACHE_DIR="$1"
DATA_DIR="$2"
PLATFORM="$3"

read -r KUBO_URL KUBO_CHECKSUM KUBO_EXTRACT_PATH < <(python3 - "$REPO_ROOT/components.json" "$PLATFORM" <<'PY'
import json
import sys

manifest = json.load(open(sys.argv[1]))
info = manifest["external"]["kubo"]["platforms"][sys.argv[2]]
for field in ("url", "checksum", "extract_path"):
    if not info.get(field):
        raise SystemExit(f"kubo {sys.argv[2]} manifest entry missing {field}")
print(info["url"], info["checksum"], info["extract_path"])
PY
)

mkdir -p "$CACHE_DIR"
TARBALL="${CACHE_DIR}/kubo-${PLATFORM}.tar.gz"

if [[ ! -f "$TARBALL" ]]; then
    echo "[seed-kubo] downloading ${KUBO_URL}"
    curl -fsSL --retry 5 --retry-delay 5 --retry-all-errors \
        --connect-timeout 30 --max-time 300 \
        -o "${TARBALL}.partial" "$KUBO_URL"
    mv "${TARBALL}.partial" "$TARBALL"
else
    echo "[seed-kubo] using cached ${TARBALL}"
fi

python3 - "$TARBALL" "$KUBO_CHECKSUM" <<'PY'
import hashlib
import sys

path, expected = sys.argv[1], sys.argv[2]
algo, _, digest = expected.partition(":")
if algo not in ("sha256", "sha512") or not digest:
    raise SystemExit(f"unsupported checksum format: {expected}")
h = hashlib.new(algo)
with open(path, "rb") as handle:
    for chunk in iter(lambda: handle.read(1024 * 1024), b""):
        h.update(chunk)
if h.hexdigest() != digest:
    raise SystemExit(f"kubo tarball checksum mismatch: {h.hexdigest()} != {digest}")
print("[seed-kubo] checksum verified", algo)
PY

mkdir -p "${DATA_DIR}/bin"
tar -xOzf "$TARBALL" "$KUBO_EXTRACT_PATH" > "${DATA_DIR}/bin/kubo.partial"
chmod 700 "${DATA_DIR}/bin/kubo.partial"
mv "${DATA_DIR}/bin/kubo.partial" "${DATA_DIR}/bin/kubo"
echo "[seed-kubo] seeded ${DATA_DIR}/bin/kubo"
