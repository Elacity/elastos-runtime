#!/usr/bin/env bash
# scripts/release/release-mac.sh
#
# Phase 10 Day 14 — operator-runnable release script for the
# `aarch64-apple-darwin` (Apple Silicon) target. Produces a
# signed, smoke-checked tarball that a Mac user can download,
# extract, optionally notarise, and run.
#
# This script does NOT notarise. Notarisation needs the
# operator's Apple Developer credentials, which must never live
# in CI secrets at this stage. The script ends by printing the
# exact `xcrun notarytool submit` + `xcrun stapler staple`
# commands the operator runs on their own machine before
# attaching the stapled tarball to the GitHub release.
#
# Usage:
#   scripts/release/release-mac.sh <TAG>
#   scripts/release/release-mac.sh <TAG> --dry-run
#
# Examples:
#   # Real release (operator on a clean checkout at the tag):
#   scripts/release/release-mac.sh v0.2.0
#
#   # Local dry-run (any branch state; skips tag verification):
#   scripts/release/release-mac.sh v0.0.0-test --dry-run
#
# Exit codes:
#   0 — staged tarball + sha256 produced; smoke checks passed
#   1 — pre-flight check failed (wrong OS, dirty tree, etc.)
#   2 — build failure
#   3 — sign / entitlement-verify failure
#   4 — smoke-check failure
#   5 — tarball / checksum failure
#
# Anchors:
#   - docs/vz-backend/PHASE_10_PLAN.md § Day 14
#   - scripts/dev/sign-elastos-vz/sign.sh (re-used unchanged)
#   - scripts/dev/sign-elastos-vz/vz.entitlements.plist
set -euo pipefail

# ────────────────────────────────────────────────────────────
# Constants — single source of truth so the workflow YAML and
# the operator both see identical names and paths.
# ────────────────────────────────────────────────────────────

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
readonly SCRIPT_DIR
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
readonly REPO_ROOT
readonly CARGO_WORKSPACE="$REPO_ROOT/elastos"
readonly BUILT_BINARY_REL="target/release/elastos"
readonly BUILT_BINARY="$CARGO_WORKSPACE/$BUILT_BINARY_REL"
readonly SIGN_SCRIPT="$REPO_ROOT/scripts/dev/sign-elastos-vz/sign.sh"
readonly ENTITLEMENTS_PLIST="$REPO_ROOT/scripts/dev/sign-elastos-vz/vz.entitlements.plist"
readonly TARGET_TRIPLE="aarch64-apple-darwin"

# Required entitlements the post-sign verifier asserts. Pinned
# to the four keys in vz.entitlements.plist so a future plist
# drift triggers a release failure rather than a silently
# under-entitled binary.
readonly REQUIRED_ENTITLEMENTS=(
  "com.apple.security.virtualization"
  "com.apple.security.cs.allow-jit"
  "com.apple.security.cs.allow-unsigned-executable-memory"
  "com.apple.security.cs.disable-executable-page-protection"
)

# ────────────────────────────────────────────────────────────
# Output helpers — minimal, no colour codes (CI logs render
# raw text best).
# ────────────────────────────────────────────────────────────

step() { echo; echo "── $* ──"; }
info() { echo "  $*"; }
err()  { echo "ERROR: $*" >&2; }

# ────────────────────────────────────────────────────────────
# Pre-flight checks
# ────────────────────────────────────────────────────────────

require_macos_arm64() {
  if [[ "$(uname -s)" != "Darwin" ]]; then
    err "release-mac.sh only runs on macOS (got $(uname -s))."
    exit 1
  fi
  local arch
  arch="$(uname -m)"
  if [[ "$arch" != "arm64" ]]; then
    err "release-mac.sh requires an Apple Silicon host (got $arch)."
    err "Cross-compiling to aarch64-apple-darwin from x86_64-apple-darwin"
    err "would skip the codesign-on-build-host invariant and is not"
    err "supported by this script. Build on arm64 hardware."
    exit 1
  fi
}

require_clean_tree_on_tag() {
  local tag="$1"
  # `git diff-index --quiet HEAD` exits non-zero if there are
  # unstaged or staged modifications relative to HEAD. Untracked
  # files are tolerated — they don't get into the tarball.
  if ! git -C "$REPO_ROOT" diff-index --quiet HEAD --; then
    err "working tree has uncommitted changes."
    err "release builds must come from a clean tree on the named tag."
    err "stash or commit your changes, then re-run with the tag."
    exit 1
  fi
  # Confirm HEAD == the named tag. We resolve the tag via
  # `git rev-parse refs/tags/<tag>` so annotated and lightweight
  # tags both work; the `^{commit}` peels annotated tags down
  # to their commit object for the comparison.
  local head_sha tag_sha
  head_sha="$(git -C "$REPO_ROOT" rev-parse HEAD)"
  if ! tag_sha="$(git -C "$REPO_ROOT" rev-parse "refs/tags/${tag}^{commit}" 2>/dev/null)"; then
    err "tag '$tag' not found in this repository."
    err "create it first: git tag -a $tag -m 'release $tag' && git push origin $tag"
    exit 1
  fi
  if [[ "$head_sha" != "$tag_sha" ]]; then
    err "HEAD is not at tag '$tag'."
    err "  HEAD: $head_sha"
    err "  tag:  $tag_sha"
    err "check out the tag before building: git checkout $tag"
    exit 1
  fi
}

# ────────────────────────────────────────────────────────────
# Build
# ────────────────────────────────────────────────────────────

cargo_release_build() {
  step "Build (cargo --release)"
  info "workspace: $CARGO_WORKSPACE"
  info "target:    $TARGET_TRIPLE"
  # Build the elastos binary from elastos-server and the vz
  # library it depends on. We don't pass --target because the
  # script enforces arm64 host above; default native target
  # already matches aarch64-apple-darwin and skipping --target
  # avoids the extra `target/aarch64-apple-darwin/...` path
  # layer that would invalidate the cache between this build
  # and the dev `cargo build --release`.
  (
    cd "$CARGO_WORKSPACE"
    cargo build --release --bin elastos -p elastos-server
    cargo build --release -p elastos-vz
  ) || { err "cargo build failed"; exit 2; }

  if [[ ! -f "$BUILT_BINARY" ]]; then
    err "cargo build reported success but $BUILT_BINARY is missing."
    exit 2
  fi
  info "binary: $BUILT_BINARY ($(stat -f '%z' "$BUILT_BINARY") bytes)"
}

# ────────────────────────────────────────────────────────────
# Stage + sign + verify entitlements
# ────────────────────────────────────────────────────────────

stage_binary() {
  local tag="$1"
  local stage_dir="$2"
  step "Stage signed binary"
  mkdir -p "$stage_dir"
  # `cp -p` preserves mtime + mode; rm + cp is idempotent on
  # re-runs so the staged file always reflects this build's
  # output (not a stale copy from a previous run).
  rm -f "$stage_dir/elastos"
  cp -p "$BUILT_BINARY" "$stage_dir/elastos"
  info "staged at $stage_dir/elastos"
}

sign_staged_binary() {
  local staged_binary="$1"
  step "Sign with Vz/JIT entitlements"
  info "sign script:  $SIGN_SCRIPT"
  info "entitlements: $ENTITLEMENTS_PLIST"
  # Re-use the existing dev signer unchanged. It takes a
  # positional binary path; ad-hoc signs (`--sign -`) with the
  # four Vz/JIT entitlements baked in. Re-signing in place is
  # idempotent — codesign --force overwrites.
  bash "$SIGN_SCRIPT" "$staged_binary" || {
    err "signing failed"
    exit 3
  }
}

verify_entitlements() {
  local staged_binary="$1"
  step "Verify entitlements"
  # Apple's `codesign -d --entitlements -` writes the
  # entitlements plist (binary or XML, depending on version) to
  # stdout. We don't parse it strictly; we grep for each
  # required key. If any are missing the binary is rejected
  # because Vz / wasmtime JIT would fail at runtime in ways
  # that show up only after the user tries to boot a microVM
  # — too late to be useful.
  local extracted
  if ! extracted="$(codesign -d --entitlements - "$staged_binary" 2>&1)"; then
    err "codesign verify failed for $staged_binary"
    exit 3
  fi
  local missing=()
  for key in "${REQUIRED_ENTITLEMENTS[@]}"; do
    if ! grep -q "$key" <<<"$extracted"; then
      missing+=("$key")
    fi
  done
  if [[ ${#missing[@]} -gt 0 ]]; then
    err "binary is missing required entitlements:"
    for k in "${missing[@]}"; do
      err "  - $k"
    done
    err "inspect with: codesign -d --entitlements - $staged_binary"
    exit 3
  fi
  info "all ${#REQUIRED_ENTITLEMENTS[@]} required entitlements present:"
  for k in "${REQUIRED_ENTITLEMENTS[@]}"; do
    info "  ✓ $k"
  done
}

# ────────────────────────────────────────────────────────────
# Smoke check — runs the staged binary in two no-side-effect
# modes. Better to publish nothing than a broken binary.
# ────────────────────────────────────────────────────────────

smoke_check_staged_binary() {
  local staged_binary="$1"
  step "Smoke check"

  # 1) --version must print *something* with "elastos" in it.
  #    clap's auto-version reads CARGO_PKG_VERSION; the
  #    workspace pins this to 0.2.0 at the time of writing, so
  #    asserting the substring keeps the test stable as the
  #    version bumps without coupling to a specific number.
  local version_out
  if ! version_out="$("$staged_binary" --version 2>&1)"; then
    err "smoke: \`elastos --version\` exited non-zero"
    err "output: $version_out"
    exit 4
  fi
  if ! grep -qi "elastos" <<<"$version_out"; then
    err "smoke: \`elastos --version\` output does not contain 'elastos':"
    err "       $version_out"
    exit 4
  fi
  info "✓ --version: $version_out"

  # 2) `vm-debug --help` must exit 0 and mention the boot
  #    subcommand. This proves the macOS-only Vz code path
  #    linked correctly — on a Linux build the vm-debug
  #    subcommand exists but the help text and runtime
  #    behaviour are degraded. On a Mac release tarball the
  #    boot subcommand must be present.
  local vmdebug_out
  if ! vmdebug_out="$("$staged_binary" vm-debug --help 2>&1)"; then
    err "smoke: \`elastos vm-debug --help\` exited non-zero"
    err "output: $vmdebug_out"
    exit 4
  fi
  if ! grep -qi "boot" <<<"$vmdebug_out"; then
    err "smoke: \`elastos vm-debug --help\` output does not mention 'boot'"
    err "       (expected the macOS-only Vz boot subcommand to be present)"
    err "output: $vmdebug_out"
    exit 4
  fi
  info "✓ vm-debug --help mentions the boot subcommand"
}

# ────────────────────────────────────────────────────────────
# Tarball + checksum — deterministic top-level dir so the
# downloader gets `elastos-<tag>/elastos` after extraction,
# not a flat `./elastos` that collides with their working dir.
# ────────────────────────────────────────────────────────────

create_tarball_and_checksum() {
  local tag="$1"
  local stage_dir="$2"
  local release_dir="$3"

  step "Tarball + SHA256"
  local tarball_basename="elastos-${tag}-${TARGET_TRIPLE}.tar.gz"
  local toplevel="elastos-${tag}-${TARGET_TRIPLE}"
  local tarball_path="$release_dir/$tarball_basename"
  local sha256_path="$tarball_path.sha256"

  # Move the staged binary under the deterministic top-level
  # dir so the tarball preserves that layout. We tar from
  # $stage_dir's parent and reference the renamed dir.
  local toplevel_dir="$stage_dir/$toplevel"
  rm -rf "$toplevel_dir"
  mkdir -p "$toplevel_dir"
  cp -p "$stage_dir/elastos" "$toplevel_dir/elastos"
  # Include a small VERSION + README inside the tarball so the
  # downloader sees the provenance without having to run the
  # binary first.
  printf '%s\n' "$tag" > "$toplevel_dir/VERSION"
  cat > "$toplevel_dir/README.txt" <<EOF
elastos $tag — Apple Silicon (aarch64-apple-darwin)

This tarball contains an ad-hoc-signed elastos binary with the
four entitlements required to drive Apple's Virtualization.framework
and wasmtime JIT.

To install:
  tar xzf $tarball_basename
  ./$toplevel/elastos --version

Required macOS: 13.0 (Ventura) or later.
Required CPU:   Apple Silicon (M1 / M2 / M3 / M4).

If you downloaded this from a GitHub release the artifact has been
notarised by Apple; macOS Gatekeeper will accept it without
prompts. If you built it yourself with this script, the ad-hoc
signature will trigger a Gatekeeper warning on first run — right-click
the binary in Finder and choose Open to bypass.

See https://github.com/Elacity/elastos-runtime for documentation.
EOF

  # tar with the deterministic top-level dir as the only entry.
  # gzip's -n strips the embedded mtime so the tarball is
  # bit-reproducible across builds of the same commit.
  (cd "$stage_dir" && tar --format=ustar -cf - "$toplevel") | gzip -n > "$tarball_path"

  # Strip the dir we just packaged so re-running this step
  # doesn't accumulate stale copies.
  rm -rf "$toplevel_dir"

  # shasum -a 256 is BSD-default; matches `sha256sum -c` shape
  # if someone wants to verify on Linux.
  local sha
  sha="$(shasum -a 256 "$tarball_path" | awk '{print $1}')"
  printf '%s  %s\n' "$sha" "$tarball_basename" > "$sha256_path"

  info "tarball: $tarball_path ($(stat -f '%z' "$tarball_path") bytes)"
  info "sha256:  $sha"
  info "          → $sha256_path"

  # Export so the caller can print the notarise commands with
  # the right paths.
  RELEASE_TARBALL_PATH="$tarball_path"
  RELEASE_SHA256_PATH="$sha256_path"
}

print_notarize_followup() {
  local tag="$1"
  step "Next steps (manual, operator-only)"
  cat <<EOF
The signed tarball is ready, but Gatekeeper on user machines will
warn until the binary is notarised. Notarisation requires your
Apple Developer credentials, which must NOT live in CI secrets
at this stage of the project (Phase 10).

On a machine that has \`xcrun notarytool\` configured with your
credentials (\`xcrun notarytool store-credentials\` run once),
submit and staple:

  # 1) Submit the tarball; --wait blocks until Apple's notary
  #    service returns Accepted / Rejected (typically 1-5 min).
  xcrun notarytool submit "$RELEASE_TARBALL_PATH" \\
    --keychain-profile elastos-notary --wait

  # 2) If Accepted, staple the notarisation ticket to the
  #    binary inside the tarball so offline machines can verify.
  #    notarytool doesn't auto-staple; you must repackage the
  #    stapled output before uploading to the GitHub release.

  TMPDIR=\$(mktemp -d)
  tar -xzf "$RELEASE_TARBALL_PATH" -C "\$TMPDIR"
  xcrun stapler staple "\$TMPDIR"/elastos-${tag}-${TARGET_TRIPLE}/elastos
  ( cd "\$TMPDIR" && tar --format=ustar -cf - "elastos-${tag}-${TARGET_TRIPLE}" | gzip -n > "$RELEASE_TARBALL_PATH" )
  shasum -a 256 "$RELEASE_TARBALL_PATH" | awk '{print \$1 "  elastos-${tag}-${TARGET_TRIPLE}.tar.gz"}' > "$RELEASE_SHA256_PATH"

  # 3) Upload the (now-stapled) tarball + .sha256 to the
  #    GitHub release for tag $tag, replacing any CI-uploaded
  #    pre-notarisation artifact.

EOF
}

# ────────────────────────────────────────────────────────────
# Main
# ────────────────────────────────────────────────────────────

usage() {
  cat <<USAGE
Usage: $0 <TAG> [--dry-run]

Builds, signs, smoke-checks, and packages an Apple Silicon
release tarball for the named tag.

Arguments:
  TAG          Release tag (e.g. v0.2.0). Must exist as a git
               tag in this repo unless --dry-run is set.

Flags:
  --dry-run    Skip the clean-tree + tag-existence checks.
               Useful for local validation; the produced
               tarball is functionally complete but should
               NOT be published.

Output:
  target/release-mac/<TAG>/elastos-<TAG>-${TARGET_TRIPLE}.tar.gz
  target/release-mac/<TAG>/elastos-<TAG>-${TARGET_TRIPLE}.tar.gz.sha256
USAGE
}

main() {
  if [[ $# -lt 1 ]]; then
    usage >&2
    exit 1
  fi

  local tag="" dry_run=0
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --dry-run) dry_run=1; shift ;;
      -h|--help) usage; exit 0 ;;
      --*)       err "unknown flag '$1'"; usage >&2; exit 1 ;;
      *)         tag="$1"; shift ;;
    esac
  done

  if [[ -z "$tag" ]]; then
    err "TAG is required."
    usage >&2
    exit 1
  fi

  step "release-mac.sh $tag$([[ $dry_run -eq 1 ]] && echo ' (DRY RUN)' || true)"
  info "repo:     $REPO_ROOT"
  info "branch:   $(git -C "$REPO_ROOT" rev-parse --abbrev-ref HEAD)"
  info "head:     $(git -C "$REPO_ROOT" rev-parse --short HEAD)"

  require_macos_arm64
  if [[ $dry_run -eq 0 ]]; then
    require_clean_tree_on_tag "$tag"
  else
    info "skipping clean-tree + tag-existence checks (--dry-run)"
  fi

  cargo_release_build

  local release_dir="$CARGO_WORKSPACE/target/release-mac/$tag"
  local stage_dir="$release_dir/stage"
  rm -rf "$stage_dir"
  stage_binary "$tag" "$stage_dir"
  sign_staged_binary "$stage_dir/elastos"
  verify_entitlements "$stage_dir/elastos"
  smoke_check_staged_binary "$stage_dir/elastos"
  create_tarball_and_checksum "$tag" "$stage_dir" "$release_dir"

  # Strip the staging copy now the tarball is built — keeps the
  # output dir to just the two artifacts the workflow uploads.
  rm -f "$stage_dir/elastos"
  rmdir "$stage_dir" 2>/dev/null || true

  print_notarize_followup "$tag"

  step "Done"
  info "tarball:  $RELEASE_TARBALL_PATH"
  info "sha256:   $RELEASE_SHA256_PATH"
  if [[ $dry_run -eq 1 ]]; then
    info "(this was a DRY RUN; do not publish this artifact)"
  fi
}

main "$@"
