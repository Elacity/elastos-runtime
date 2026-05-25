#!/usr/bin/env bash
# scripts/dev/mac-local-setup.sh
#
# Phase 9 Day 1 — Mac source-checkout bootstrap for `elastos home`.
#
# On a fresh Mac there is no canonical install at
# `~/Library/Application Support/elastos/`, so `elastos setup` fails
# with "No trusted source configured" — that flow expects to download
# first-party artifacts over Carrier from a publisher already known to
# the host. The Linux developer escape hatch (`scripts/home-demo-local.sh`)
# uses `getent`, GNU `sha256sum`/`stat -c`, and `scripts/install.sh`'s
# stamped-installer path, none of which work on Mac.
#
# This script is the Mac equivalent: build the three host providers
# (`shell`, `localhost-provider`, `did-provider`) from the workspace,
# stage them under `<data_dir>/bin/`, and write a `components.json`
# whose `darwin-arm64` entries carry the actual `sha256:` checksums
# of the staged binaries — which is what
# `verify_installed_component_binary` checks before letting the
# runtime spawn them.
#
# Usage:
#   scripts/dev/mac-local-setup.sh
#
# After it finishes:
#   elastos/target/debug/elastos home --status     # prereqs ready
#
# This script is idempotent (re-stamps with current binaries each run)
# and Mac-only — it bails out on any other OS.
#
# Anchors:
#   - docs/vz-backend/PHASE_9_DAY_1_NOTES.md
#   - elastos-server::binaries::find_installed_provider_binary
#   - elastos-server::setup::verify_installed_component_binary

set -euo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "error: mac-local-setup.sh is Mac-only (got $(uname -s))." >&2
  echo "       Linux source checkouts should use scripts/home-demo-local.sh." >&2
  exit 2
fi

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
DATA_DIR="${ELASTOS_DATA_DIR:-$HOME/Library/Application Support/elastos}"

# Platform key as detect_platform() emits on darwin-arm64.
PLATFORM="darwin-arm64"

# Map each provider to the workspace it belongs to. `shell` and
# `localhost-provider` are members of the elastos workspace; the
# `did-provider` crate has its own Cargo.toml (capsules/did-provider).
COMPONENT_NAMES=(shell localhost-provider did-provider)
COMPONENT_MANIFESTS=(
  "elastos/Cargo.toml"
  "elastos/Cargo.toml"
  "capsules/did-provider/Cargo.toml"
)
COMPONENT_TARGET_DIRS=(
  "elastos/target/release"
  "elastos/target/release"
  "capsules/did-provider/target/release"
)

echo "[mac-local-setup] repo:      $REPO_ROOT"
echo "[mac-local-setup] data-dir:  $DATA_DIR"
echo "[mac-local-setup] platform:  $PLATFORM"
echo

# 1. Build the three providers in release mode.
for idx in "${!COMPONENT_NAMES[@]}"; do
  name="${COMPONENT_NAMES[$idx]}"
  manifest="$REPO_ROOT/${COMPONENT_MANIFESTS[$idx]}"
  echo "[mac-local-setup] building $name (manifest=$manifest)"
  (
    cd "$REPO_ROOT"
    cargo build --release --manifest-path "$manifest" -p "$name"
  )
done
echo

# 2. Stage the binaries into <data_dir>/bin and compute sha256 + size
#    in a Mac-portable way (BSD `shasum -a 256`, BSD `stat -f`).
mkdir -p "$DATA_DIR/bin"
declare -a STAGED_SHAS=()
declare -a STAGED_SIZES=()
for idx in "${!COMPONENT_NAMES[@]}"; do
  name="${COMPONENT_NAMES[$idx]}"
  target_dir="$REPO_ROOT/${COMPONENT_TARGET_DIRS[$idx]}"
  src="$target_dir/$name"
  dest="$DATA_DIR/bin/$name"

  if [[ ! -x "$src" ]]; then
    echo "error: built binary missing: $src" >&2
    exit 1
  fi

  # `install -m` always re-copies, but the bytes only change when the
  # build changed — which is what makes the operation effectively
  # idempotent for the manifest writer below.
  install -m 0755 "$src" "$dest"

  STAGED_SHAS[$idx]="$(shasum -a 256 "$dest" | awk '{print $1}')"
  STAGED_SIZES[$idx]="$(stat -f '%z' "$dest")"
  echo "[mac-local-setup] staged $dest"
  echo "  sha256: ${STAGED_SHAS[$idx]}"
  echo "  size:   ${STAGED_SIZES[$idx]}"
done
echo

# 3. Read the source-checkout components.json, stamp the darwin-arm64
#    entries for the three providers with the staged checksum/size, and
#    write it to <data_dir>/components.json. `load_manifest` resolves
#    this path first, so the runtime sees our local manifest.
python3 - \
    "$REPO_ROOT/components.json" \
    "$DATA_DIR/components.json" \
    "$PLATFORM" \
    "${COMPONENT_NAMES[0]}" "${STAGED_SHAS[0]}" "${STAGED_SIZES[0]}" \
    "${COMPONENT_NAMES[1]}" "${STAGED_SHAS[1]}" "${STAGED_SIZES[1]}" \
    "${COMPONENT_NAMES[2]}" "${STAGED_SHAS[2]}" "${STAGED_SIZES[2]}" <<'PY'
import json
import sys

src_path, dst_path, platform = sys.argv[1:4]
stamps = []
i = 4
while i < len(sys.argv):
    stamps.append((sys.argv[i], sys.argv[i + 1], int(sys.argv[i + 2])))
    i += 3

with open(src_path, "r", encoding="utf-8") as f:
    data = json.load(f)

external = data.setdefault("external", {})
for name, sha, size in stamps:
    component = external.get(name)
    if component is None:
        raise SystemExit(f"components.json missing external entry for {name!r}")
    platforms = component.setdefault("platforms", {})
    entry = platforms.get(platform)
    if entry is None:
        raise SystemExit(
            f"components.json {name!r} has no {platform} platform entry"
        )
    entry["checksum"] = f"sha256:{sha}"
    entry["size"] = size

with open(dst_path, "w", encoding="utf-8") as f:
    json.dump(data, f, indent=2)
    f.write("\n")
PY
echo "[mac-local-setup] wrote $DATA_DIR/components.json"
echo

# 4. Quick sanity check using the local debug binary if present, so the
#    operator immediately sees whether the prereq check is satisfied.
DEBUG_ELASTOS="$REPO_ROOT/elastos/target/debug/elastos"
if [[ -x "$DEBUG_ELASTOS" ]]; then
  echo "[mac-local-setup] verifying via: elastos home --status --json"
  "$DEBUG_ELASTOS" home --status --json \
    | python3 -c '
import json, sys
snap = json.load(sys.stdin)
services = snap.get("system_services", [])
host_backings = {"shell", "localhost-provider", "did-provider"}
problems = [s for s in services if not s.get("ready") and s.get("backing") in host_backings]
if problems:
    for s in problems:
        name = s.get("name")
        backing = s.get("backing")
        state = s.get("state")
        print(f"  NOT READY: {name} (backing={backing}, state={state})")
    sys.exit(1)
print("[mac-local-setup] all three host providers report ready.")
'
else
  echo "[mac-local-setup] note: $DEBUG_ELASTOS not built — skipping live check."
  echo "[mac-local-setup]   build it with: cargo build --manifest-path \"$REPO_ROOT/elastos/Cargo.toml\" -p elastos-server"
fi

echo
echo "[mac-local-setup] OK"
echo "  Try: $DEBUG_ELASTOS home --status"
