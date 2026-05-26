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
# Day-5 — equivalent stream for capsule entries written into the
# `capsules:` map of components.json. Mirrors the canonical
# chat-staging pattern from `scripts/home-demo-local.sh` lines 167-188.
# Format per line:
#   name<TAB>cid<TAB>sha256<TAB>size<TAB>platform
CAPSULE_STAMPS_FILE="$(mktemp -t mac-local-setup-capsule-stamps.XXXXXX)"
trap 'rm -f "$PROVIDER_STAMPS_FILE" "$CAPSULE_STAMPS_FILE"' EXIT

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

  stamp_local_capsule_cid "$name" "any"

  echo "  staged $dest_dir/{${name}.wasm, capsule.json}"
}

# stamp_local_capsule_cid <name> <platform>
#
# Day-5 — mirrors the canonical `local-<name>-<sha:0:16>` CID pattern
# from `scripts/home-demo-local.sh` lines 179-188. After the capsule
# directory is staged, this writes:
#
#   <data_dir>/capsules/<name>/.elastos-cid              ← local CID marker
#   <data_dir>/capsules/<name>/.elastos-artifact-sha256  ← artifact sha256
#
# and appends a TSV row to $CAPSULE_STAMPS_FILE that the python
# manifest writer consumes, ensuring the `capsules:<name>` entry in
# `<data_dir>/components.json` carries the same cid/sha256/size. With
# both stores matching, `Supervisor::ensure_capsule` short-circuits on
# the cached-CID match (supervisor.rs:1530) and returns the existing
# directory without ever attempting an IPFS fetch.
#
# Hash scheme: sha256 over a deterministic stream of (relative file
# path) + (file bytes) for every regular file in the capsule directory,
# excluding the two stamp files themselves. This produces a stable
# sha across re-runs as long as the staged content is unchanged, and
# tracks content drift without requiring a tarball intermediate.
#
# Platform: passed through to the manifest's `platforms` list. WASM
# uses `"any"`; data capsules use `"any"` too (HTML is portable).
stamp_local_capsule_cid() {
  local name="$1"
  local platform="$2"
  local dest_dir="$DATA_DIR/capsules/$name"

  if [[ ! -d "$dest_dir" ]]; then
    echo "error: cannot stamp CID; capsule dir missing: $dest_dir" >&2
    exit 1
  fi

  local sha
  sha="$(
    cd "$dest_dir" \
      && find . -type f \
           -not -name '.elastos-cid' \
           -not -name '.elastos-artifact-sha256' \
           -print0 \
      | LC_ALL=C sort -z \
      | xargs -0 cat \
      | shasum -a 256 \
      | awk '{print $1}'
  )"

  # Sum all artifact bytes for the `size` field. `find` runs from the
  # capsule dir so paths are relative — pipe them straight back through
  # `stat -f '%z'` from the same cwd to avoid bouncing between shells.
  local size
  size="$(
    cd "$dest_dir" \
      && find . -type f \
           -not -name '.elastos-cid' \
           -not -name '.elastos-artifact-sha256' \
           -exec stat -f '%z' {} + \
      | awk '{ total += $1 } END { print total + 0 }'
  )"

  local cid="local-${name}-${sha:0:16}"

  printf '%s\n' "$cid" > "$dest_dir/.elastos-cid"
  printf '%s\n' "$sha" > "$dest_dir/.elastos-artifact-sha256"

  printf '%s\t%s\t%s\t%s\t%s\n' \
    "$name" "$cid" "$sha" "$size" "$platform" \
    >> "$CAPSULE_STAMPS_FILE"
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
  # ignores when discovering capsule directories. The two `.elastos-*`
  # files are written by stamp_local_capsule_cid below; rsync excludes
  # them so the stamps survive across re-runs.
  rsync -a --delete \
    --exclude 'target/' \
    --exclude '*.lock' \
    --exclude '.elastos-cid' \
    --exclude '.elastos-artifact-sha256' \
    --exclude 'browser/' \
    "$src_dir/" "$dest_dir/"

  stamp_local_capsule_cid "$name" "any"

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
    "$PROVIDER_STAMPS_FILE" "$CAPSULE_STAMPS_FILE" <<'PY'
import json
import sys

src_path, dst_path, platform, provider_stamps_path, capsule_stamps_path = sys.argv[1:6]

# Provider stamps: name<TAB>sha<TAB>size  → stamped onto external[<name>].platforms[<platform>].
with open(provider_stamps_path, "r", encoding="utf-8") as f:
    provider_stamps = []
    for line in f:
        line = line.strip()
        if not line:
            continue
        name, sha, size = line.split("\t")
        provider_stamps.append((name, sha, int(size)))

# Capsule stamps: name<TAB>cid<TAB>sha<TAB>size<TAB>platform
# Mirrors the canonical `capsules.<name> = {cid, sha256, size, platforms}` shape
# that `home-demo-local.sh` stamps for the locally-built chat bundle (lines 243-248).
with open(capsule_stamps_path, "r", encoding="utf-8") as f:
    capsule_stamps = []
    for line in f:
        line = line.strip()
        if not line:
            continue
        name, cid, sha, size, plat = line.split("\t")
        capsule_stamps.append((name, cid, sha, int(size), plat))

with open(src_path, "r", encoding="utf-8") as f:
    data = json.load(f)

external = data.setdefault("external", {})
for name, sha, size in provider_stamps:
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

# Day-5 — register each staged capsule in the `capsules:` map so the supervisor's
# `resolve-plan` + `ensure-capsule` see them as locally-cached artifacts whose
# CIDs match the on-disk .elastos-cid stamps (supervisor.rs:1530 short-circuits
# on cached-CID match → no IPFS fetch attempted).
capsules = data.setdefault("capsules", {})
for name, cid, sha, size, plat in capsule_stamps:
    capsules[name] = {
        "cid": cid,
        "sha256": sha,
        "size": size,
        "platforms": [plat],
    }

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

# resign_binary_if_missing_entitlement <binary_path>
#
# Idempotent codesign helper: re-runs the dev-sign script only
# when the Vz entitlement is missing. Same XML-substring check
# as the main-binary loop above, factored out so the
# test-binary loop below can use it without duplicating the
# `codesign | grep` recipe.
resign_binary_if_missing_entitlement() {
  local binary_path="$1"
  local label="$2"
  if ! codesign -d --entitlements - --xml "$binary_path" 2>&1 \
        | grep -q "com.apple.security.virtualization"; then
    echo "[mac-local-setup] $label missing Vz/JIT entitlements — re-signing"
    "$SIGN_SCRIPT" "$binary_path" 2>&1 | sed 's/^/  /'
  fi
}

if [[ -x "$DEBUG_ELASTOS" ]]; then
  resign_binary_if_missing_entitlement "$DEBUG_ELASTOS" "debug binary"
fi

# ── 6b. Auto re-sign elastos-vz integration test binaries ────────────
#
# `cargo test -p elastos-vz --test <name>` produces a per-test
# binary at `elastos/target/{debug,release}/deps/<name>-<hash>`,
# and like the main binary, every rebuild strips the codesign
# signature. Without the Vz entitlement the tests `panic!` on
# Apple's `VZVirtualMachineConfiguration.validateWithError`,
# which contributors then mistake for genuine test failures.
#
# Day-4 of Phase 9 added auto-resign for `target/debug/elastos`
# (above); this section extends the same idempotent recipe to
# every integration-test binary that exercises Vz. The plist is
# the same — Vz + JIT entitlements — because tests load both
# Apple's framework and `wasmtime` indirectly through the
# crate's build matrix.
#
# Why a fixed list of test names rather than `find … deps/*`:
# `deps/` also holds compiled dependency rlibs and helper
# binaries we must not sign. An allow-list mirrors the
# `tests/*.rs` source layout and is easy to audit when a new
# test is added.

ELASTOS_VZ_TEST_BINARIES=(concurrent_launch smoke)

resign_vz_test_binaries_for_profile() {
  local profile="$1"
  local deps_dir="$REPO_ROOT/elastos/target/$profile/deps"

  if [[ ! -d "$deps_dir" ]]; then
    return 0
  fi

  shopt -s nullglob
  local test_name
  for test_name in "${ELASTOS_VZ_TEST_BINARIES[@]}"; do
    local candidate
    for candidate in "$deps_dir/${test_name}-"*; do
      # Skip cargo's dep-info sidecar files — only the matching
      # executable carries the codesign signature.
      [[ "$candidate" == *.d ]] && continue
      # Skip directories (cargo occasionally creates per-test
      # incremental dirs alongside the binaries).
      [[ -d "$candidate" ]] && continue
      # An executable Mach-O is the only thing worth signing.
      [[ -x "$candidate" && -f "$candidate" ]] || continue

      resign_binary_if_missing_entitlement \
        "$candidate" \
        "$profile/${candidate##*/}"
    done
  done
  shopt -u nullglob
}

for profile in debug release; do
  resign_vz_test_binaries_for_profile "$profile"
done

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

# Day-5 — verify the canonical capsule-registry chain: every home-surface
# capsule we stage must appear in components.json `capsules:` with a CID
# that exactly matches the `.elastos-cid` file on disk. This is the
# precondition `Supervisor::ensure_capsule` checks (supervisor.rs:1530)
# before short-circuiting the IPFS fetch — without it, every launch of
# a non-WASM capsule fails with "capsule '<name>' not in registry" or
# "capsule cache CID metadata missing or stale".
echo "[mac-local-setup] verifying capsule registry consistency"
python3 - "$DATA_DIR" <<'PY' || exit 1
import json
import sys
from pathlib import Path

data_dir = Path(sys.argv[1])
manifest_path = data_dir / "components.json"
with open(manifest_path, "r", encoding="utf-8") as f:
    manifest = json.load(f)

capsules = manifest.get("capsules", {})
failed = 0
for name in ("home", "system", "documents", "library", "inbox"):
    entry = capsules.get(name)
    if entry is None:
        print(f"    [no ] {name}: missing components.json capsules entry")
        failed += 1
        continue
    cid_file = data_dir / "capsules" / name / ".elastos-cid"
    if not cid_file.is_file():
        print(f"    [no ] {name}: missing .elastos-cid stamp on disk")
        failed += 1
        continue
    on_disk_cid = cid_file.read_text().strip()
    entry_cid = entry.get("cid", "")
    if on_disk_cid != entry_cid:
        print(
            f"    [no ] {name}: on-disk CID {on_disk_cid!r} != manifest CID {entry_cid!r}"
        )
        failed += 1
        continue
    print(f"    [ok ] {name}: cid={entry_cid}")

if failed:
    print(f"  FAILED: {failed} capsule(s) have inconsistent registry / stamp state.")
    sys.exit(1)
PY
else
  echo "[mac-local-setup] note: $DEBUG_ELASTOS not built — skipping live check."
  echo "[mac-local-setup]   build it with: cargo build --manifest-path \"$REPO_ROOT/elastos/Cargo.toml\" -p elastos-server"
fi

echo
echo "[mac-local-setup] OK"
echo "  Try: $DEBUG_ELASTOS home"
