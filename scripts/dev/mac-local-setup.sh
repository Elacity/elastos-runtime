#!/usr/bin/env bash
# scripts/dev/mac-local-setup.sh
#
# Phase 9 Day 1+2 — Mac source-checkout bootstrap for `elastos home`.
#
# On a fresh Mac there is no canonical install at
# `~/Library/Application Support/elastos/`, so `elastos setup` fails
# with "No trusted source configured" — that flow expects to download
# first-party artifacts over Carrier from a publisher already known to
# the host. The Linux developer escape hatch (`scripts/home-demo-local.sh`)
# uses `getent`, GNU `sha256sum`/`stat -c`, and `scripts/install.sh`'s
# stamped-installer path, none of which work on Mac.
#
# This script is the Mac equivalent. It builds and stages every
# first-party Home component the runtime can look up on disk:
#
#   - Seven host providers (Rust binaries → `<data_dir>/bin/<name>`):
#       shell, localhost-provider, did-provider,
#       webspace-provider, site-provider,
#       ipfs-provider, tunnel-provider
#
#   - Two WASM capsules (wasm32-wasip1 → `<data_dir>/capsules/<name>/`):
#       home, system
#
#   - Three data capsules (HTML only → `<data_dir>/capsules/<name>/`):
#       documents, library, inbox
#
# The manifest at `<data_dir>/components.json` is rewritten with the
# live `sha256:<hex>` + `size` for every staged provider so
# `verify_installed_component_binary` accepts them. Capsule entries
# keep their empty CIDs (matching the empty CIDs in our manifest, so
# the capsule install-state check is satisfied without writing
# `.elastos-cid` files).
#
# Third-party binaries (`kubo`, `cloudflared`) are not built here.
# If they're on PATH (`brew install kubo cloudflared`) the runtime
# auto-discovers them via `find_installed_provider_binary`. The
# script prints the install hint when either is missing.
#
# Usage:
#   scripts/dev/mac-local-setup.sh
#
# This script is idempotent (re-runs stage identical bytes when
# sources are unchanged) and Mac-only.
#
# Anchors:
#   - docs/vz-backend/PHASE_9_DAY_1_NOTES.md (Day-1 baseline: 3/8 ready)
#   - docs/vz-backend/PHASE_9_DAY_2_NOTES.md (this script's full surface)
#   - elastos-server::binaries::find_installed_provider_binary
#   - elastos-server::setup::verify_installed_component_binary
#   - elastos-server::setup::component_install_state

set -euo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "error: mac-local-setup.sh is Mac-only (got $(uname -s))." >&2
  echo "       Linux source checkouts should use scripts/home-demo-local.sh." >&2
  exit 2
fi

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
DATA_DIR="${ELASTOS_DATA_DIR:-$HOME/Library/Application Support/elastos}"
PLATFORM="darwin-arm64"

echo "[mac-local-setup] repo:      $REPO_ROOT"
echo "[mac-local-setup] data-dir:  $DATA_DIR"
echo "[mac-local-setup] platform:  $PLATFORM"
echo

mkdir -p "$DATA_DIR/bin" "$DATA_DIR/capsules"

# Hold staged provider metadata in a single TSV stream so the inline
# python stamper has one source of truth. Format per line:
#   name<TAB>sha256<TAB>size
PROVIDER_STAMPS_FILE="$(mktemp -t mac-local-setup-stamps.XXXXXX)"
trap 'rm -f "$PROVIDER_STAMPS_FILE"' EXIT

# ── Helpers ──────────────────────────────────────────────────────────

# build_and_stage_provider <name> <manifest_path> <target_dir>
#
# Builds the named Rust crate in release mode, stages the resulting
# binary under <data_dir>/bin/<name>, prints checksum + size, and
# appends a TSV stamp row that the manifest writer consumes later.
build_and_stage_provider() {
  local name="$1"
  local manifest="$REPO_ROOT/$2"
  local target_dir="$REPO_ROOT/$3"

  echo "[mac-local-setup] building provider: $name"
  (
    cd "$REPO_ROOT"
    cargo build --release --manifest-path "$manifest" -p "$name" 2>&1 \
      | sed 's/^/  /'
  )

  local src="$target_dir/$name"
  if [[ ! -x "$src" ]]; then
    echo "error: built binary missing: $src" >&2
    exit 1
  fi

  local dest="$DATA_DIR/bin/$name"
  install -m 0755 "$src" "$dest"

  local sha size
  sha="$(shasum -a 256 "$dest" | awk '{print $1}')"
  size="$(stat -f '%z' "$dest")"
  printf '%s\t%s\t%s\n' "$name" "$sha" "$size" >> "$PROVIDER_STAMPS_FILE"

  echo "  staged $dest"
  echo "    sha256: $sha"
  echo "    size:   $size"
}

# build_and_stage_wasm_capsule <name> <manifest_path> <target_dir>
#
# Builds the named crate to wasm32-wasip1 release and stages
# <name>.wasm + capsule.json at <data_dir>/capsules/<name>/.
build_and_stage_wasm_capsule() {
  local name="$1"
  local manifest="$REPO_ROOT/$2"
  local target_dir="$REPO_ROOT/$3"

  echo "[mac-local-setup] building wasm capsule: $name"
  (
    cd "$REPO_ROOT"
    cargo build --release --target wasm32-wasip1 \
      --manifest-path "$manifest" -p "$name" 2>&1 \
      | sed 's/^/  /'
  )

  local src_wasm="$target_dir/${name}.wasm"
  if [[ ! -f "$src_wasm" ]]; then
    echo "error: built wasm missing: $src_wasm" >&2
    exit 1
  fi

  local src_manifest="$REPO_ROOT/capsules/$name/capsule.json"
  if [[ ! -f "$src_manifest" ]]; then
    echo "error: capsule.json missing: $src_manifest" >&2
    exit 1
  fi

  local dest_dir="$DATA_DIR/capsules/$name"
  mkdir -p "$dest_dir"
  install -m 0644 "$src_wasm" "$dest_dir/${name}.wasm"
  install -m 0644 "$src_manifest" "$dest_dir/capsule.json"

  echo "  staged $dest_dir/{${name}.wasm, capsule.json}"
}

# stage_data_capsule <name>
#
# Copies the HTML-only data capsule's manifest + entrypoint asset(s)
# from capsules/<name>/ into <data_dir>/capsules/<name>/. Skips any
# build artefacts (target/, *.lock).
stage_data_capsule() {
  local name="$1"
  local src_dir="$REPO_ROOT/capsules/$name"
  local dest_dir="$DATA_DIR/capsules/$name"

  if [[ ! -f "$src_dir/capsule.json" ]]; then
    echo "error: data capsule missing capsule.json: $src_dir" >&2
    exit 1
  fi

  echo "[mac-local-setup] staging data capsule: $name"
  mkdir -p "$dest_dir"

  # `rsync` is portable on macOS and skips build/cache directories
  # without needing GNU `cp -ru`. The exclusions match what the runtime
  # ignores when discovering capsule directories.
  rsync -a --delete \
    --exclude 'target/' \
    --exclude '*.lock' \
    --exclude '.elastos-cid' \
    --exclude '.elastos-artifact-sha256' \
    --exclude 'browser/' \
    "$src_dir/" "$dest_dir/"

  echo "  staged $dest_dir/"
}

# ── 1. Native providers ──────────────────────────────────────────────

build_and_stage_provider shell \
  "elastos/Cargo.toml" \
  "elastos/target/release"

build_and_stage_provider localhost-provider \
  "elastos/Cargo.toml" \
  "elastos/target/release"

build_and_stage_provider did-provider \
  "capsules/did-provider/Cargo.toml" \
  "capsules/did-provider/target/release"

build_and_stage_provider webspace-provider \
  "capsules/webspace-provider/Cargo.toml" \
  "capsules/webspace-provider/target/release"

build_and_stage_provider site-provider \
  "capsules/site-provider/Cargo.toml" \
  "capsules/site-provider/target/release"

build_and_stage_provider ipfs-provider \
  "capsules/ipfs-provider/Cargo.toml" \
  "capsules/ipfs-provider/target/release"

build_and_stage_provider tunnel-provider \
  "capsules/tunnel-provider/Cargo.toml" \
  "capsules/tunnel-provider/target/release"

echo

# ── 2. WASM capsules ─────────────────────────────────────────────────

build_and_stage_wasm_capsule home \
  "capsules/home/Cargo.toml" \
  "capsules/home/target/wasm32-wasip1/release"

build_and_stage_wasm_capsule system \
  "capsules/system/Cargo.toml" \
  "capsules/system/target/wasm32-wasip1/release"

echo

# ── 3. Data capsules ─────────────────────────────────────────────────

stage_data_capsule documents
stage_data_capsule library
stage_data_capsule inbox

echo

# ── 4. Stamp the manifest ────────────────────────────────────────────

python3 - "$REPO_ROOT/components.json" "$DATA_DIR/components.json" "$PLATFORM" \
    "$PROVIDER_STAMPS_FILE" <<'PY'
import json
import sys

src_path, dst_path, platform, stamps_path = sys.argv[1:5]

with open(stamps_path, "r", encoding="utf-8") as f:
    stamps = []
    for line in f:
        line = line.strip()
        if not line:
            continue
        name, sha, size = line.split("\t")
        stamps.append((name, sha, int(size)))

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

# ── 5. Third-party dependency hints ──────────────────────────────────

THIRD_PARTY_HINTS=()
for dep in kubo cloudflared; do
  if ! command -v "$dep" >/dev/null 2>&1; then
    THIRD_PARTY_HINTS+=("$dep")
  fi
done

if (( ${#THIRD_PARTY_HINTS[@]} > 0 )); then
  echo "[mac-local-setup] third-party dependencies not on PATH:"
  for dep in "${THIRD_PARTY_HINTS[@]}"; do
    echo "  - $dep   (install with: brew install $dep)"
  done
  echo "  Content Exchange and/or Public Edge services will remain"
  echo "  in 'missing prerequisites' state until they are installed."
  echo
fi

# ── 6. Auto re-sign after a cargo rebuild ────────────────────────────
#
# `cargo build -p elastos-server` invalidates the codesign signature
# (the linker rewrites the binary), so any rebuild silently strips the
# four entitlements the dev-sign plist bakes in. Without them:
#   - `com.apple.security.virtualization` missing → Vz refuses to boot.
#   - `com.apple.security.cs.allow-jit` missing  → macOS SIGKILLs
#     wasmtime the first time it `mprotect(PROT_EXEC)`s a JIT page
#     (no stderr, exit 137).
# Both failures look like silent misbehaviour to a fresh operator.
#
# Detect the missing entitlement and invoke the existing dev-sign
# script. Idempotent: the check is a substring search on the
# codesign XML output, so a correctly-signed binary is a no-op.

DEBUG_ELASTOS="$REPO_ROOT/elastos/target/debug/elastos"
SIGN_SCRIPT="$REPO_ROOT/scripts/dev/sign-elastos-vz/sign.sh"

if [[ -x "$DEBUG_ELASTOS" ]]; then
  if ! codesign -d --entitlements - --xml "$DEBUG_ELASTOS" 2>&1 \
        | grep -q "com.apple.security.virtualization"; then
    echo "[mac-local-setup] debug binary missing Vz/JIT entitlements — re-signing"
    "$SIGN_SCRIPT" "$DEBUG_ELASTOS" 2>&1 | sed 's/^/  /'
  fi
fi

# ── 7. Self-verify ───────────────────────────────────────────────────

if [[ -x "$DEBUG_ELASTOS" ]]; then
  echo "[mac-local-setup] verifying via: elastos home --status --json"
  "$DEBUG_ELASTOS" home --status --json \
    | python3 -c '
import json, sys
snap = json.load(sys.stdin)
services = snap.get("system_services", [])
ready_count = sum(1 for s in services if s.get("ready"))
total = len(services)
print(f"  services ready: {ready_count} / {total}")
for s in services:
    state = "ok " if s.get("ready") else "no "
    name = s.get("name")
    backing = s.get("backing")
    print(f"    [{state}] {name}  ({backing})")

# Day-2 floor: at least 5 services ready (3 host providers + WebSpaces + Site Edge).
if ready_count < 5:
    print("  FAILED: fewer than 5 services ready.")
    sys.exit(1)
'
else
  echo "[mac-local-setup] note: $DEBUG_ELASTOS not built — skipping live check."
  echo "[mac-local-setup]   build it with: cargo build --manifest-path \"$REPO_ROOT/elastos/Cargo.toml\" -p elastos-server"
fi

echo
echo "[mac-local-setup] OK"
echo "  Try: $DEBUG_ELASTOS home"
