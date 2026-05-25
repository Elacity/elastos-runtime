#!/usr/bin/env bash
# scripts/build-vmlinux-arm64.sh — Phase 6 Day 4 (Sub-1).
#
# Deterministic build recipe for the `vmlinux-darwin-arm64` artifact
# (raw ARM64 Image kernel) the runtime fetches via
# `external.vmlinux.platforms.darwin-arm64` in components.json.
#
# This recipe is part of the **operator handoff** for Phase 6 Day 4b:
# the script is shipped here for reproducibility; the operator runs it
# on a dev Mac with the cross-compile toolchain installed, then
# populates `vmlinux.darwin-arm64.{checksum,size}` in components.json
# from the build output (the file paths this script prints).
#
# Anchor: docs/vz-backend/PHASE_6_COMPONENTS_AUDIT.md § 4.3 + § 5
# Decision A (build same 6.1.59 source for arm64).
#
# Why this artifact exists:
#   - macOS hosts have no `/boot/Image` (the linux-arm64 strategy
#     `local-copy` from `/boot/Image` doesn't apply on darwin — Mac
#     runs XNU, not Linux).
#   - Apple's Vz `VZLinuxBootLoader` consumes the same raw ARM64
#     `Image` format that crosvm aarch64 uses.
#   - The runtime's content-addressed identity model (PHASE_0_SCOPE.md
#     § C) prefers a single guest-kernel build across hosts. Hence
#     "same 6.1.59 source, recompiled for arm64".
#
# Prerequisites (operator-side, dev Mac):
#   - Cross-compile toolchain (one of):
#       a) `brew install aarch64-elf-gcc` (Homebrew formula provides
#          the ARM64 cross-compiler suitable for Linux kernel builds).
#       b) `brew install llvm` then build with `CC=clang` and
#          `--target=aarch64-linux-gnu` flags (slower path; less
#          mainstream-tested).
#   - GNU make (`brew install make` — macOS ships an old BSD make
#     which the Linux kernel Makefile does not accept).
#   - libelf / openssl / bc (Linux kernel build deps):
#       `brew install elfutils openssl@3 bc`
#   - ~10 GB free disk; ~30 min wall-clock on M1/M2.
#
# Inputs:
#   - ELASTOS_VMLINUX_SRC          path to a pristine Linux 6.1.59 tree
#                                  (default: ${PWD}/build/linux-6.1.59).
#                                  If absent, downloads via
#                                  ELASTOS_VMLINUX_SRC_URL.
#   - ELASTOS_VMLINUX_SRC_URL      tarball URL (default: kernel.org).
#   - ELASTOS_VMLINUX_CONFIG       path to a Kconfig template
#                                  (default: scripts/release/vmlinux-arm64.config).
#   - ELASTOS_VMLINUX_OUT          output directory
#                                  (default: ${PWD}/elastos/target/vmlinux-darwin-arm64).
#   - CROSS_COMPILE                toolchain prefix (auto-detected; e.g.
#                                  aarch64-elf- or aarch64-linux-gnu-).
#
# Outputs:
#   - ${OUT}/Image                 the raw ARM64 kernel image.
#   - ${OUT}/Image.sha256          its sha256 (for components.json).
#   - ${OUT}/Image.size            byte size (for components.json).
#   - ${OUT}/build.log             full kernel build transcript.
#
# Smoke-tests performed at the end:
#   1. `file Image` → must report "Linux kernel ARM64 boot executable Image"
#      (or equivalent; the verifier below uses the byte-magic check
#      from elastos/crates/elastos-crosvm/src/config.rs::looks_like_arm64_image).
#   2. Byte-magic check (offsets 0x38..0x3c == "ARMd", 0x40..0x44 == "PE\0\0").
#
# Exit codes:
#   0   build + verification green; print path to Image, sha256, size.
#   1   prerequisite missing (toolchain, make, etc.). Diagnostic on stderr.
#   2   build failed (kernel make returned non-zero). Tail of build.log
#       on stderr.
#   3   produced Image fails the byte-magic check (compromised build —
#       e.g. wrong ARCH). Diagnostic on stderr.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"

VMLINUX_SRC="${ELASTOS_VMLINUX_SRC:-${REPO_ROOT}/build/linux-6.1.59}"
VMLINUX_SRC_URL="${ELASTOS_VMLINUX_SRC_URL:-https://cdn.kernel.org/pub/linux/kernel/v6.x/linux-6.1.59.tar.xz}"
VMLINUX_CONFIG="${ELASTOS_VMLINUX_CONFIG:-${REPO_ROOT}/scripts/release/vmlinux-arm64.config}"
VMLINUX_OUT="${ELASTOS_VMLINUX_OUT:-${REPO_ROOT}/elastos/target/vmlinux-darwin-arm64}"

log() { printf '[vmlinux-arm64] %s\n' "$*" >&2; }
die() { log "ERROR: $*"; exit "${2:-1}"; }

# ── Prerequisites ──────────────────────────────────────────────────────────
log "verifying toolchain prerequisites"

if [[ -z "${CROSS_COMPILE:-}" ]]; then
    for candidate in aarch64-elf- aarch64-linux-gnu- aarch64-none-linux-gnu-; do
        if command -v "${candidate}gcc" >/dev/null 2>&1; then
            CROSS_COMPILE="${candidate}"
            log "auto-detected CROSS_COMPILE=${CROSS_COMPILE}"
            break
        fi
    done
fi
[[ -n "${CROSS_COMPILE:-}" ]] || die "no aarch64 cross-compiler found. Install via 'brew install aarch64-elf-gcc' or set CROSS_COMPILE explicitly."

GMAKE="$(command -v gmake || command -v make)"
[[ -n "${GMAKE}" ]] || die "neither gmake nor make found"
if "${GMAKE}" --version 2>/dev/null | grep -q "GNU Make"; then
    log "using ${GMAKE} (GNU Make confirmed)"
else
    die "macOS BSD make is not compatible with Linux kernel build. Install GNU make: 'brew install make' and re-run with GMAKE=/opt/homebrew/bin/gmake."
fi

for tool in tar xz curl shasum file; do
    command -v "${tool}" >/dev/null 2>&1 || die "missing prerequisite: ${tool}"
done

# ── Fetch source if needed ─────────────────────────────────────────────────
if [[ ! -d "${VMLINUX_SRC}" ]]; then
    log "source tree absent at ${VMLINUX_SRC} — downloading from ${VMLINUX_SRC_URL}"
    mkdir -p "$(dirname "${VMLINUX_SRC}")"
    tarball="$(dirname "${VMLINUX_SRC}")/linux-6.1.59.tar.xz"
    curl -fsSL -o "${tarball}" "${VMLINUX_SRC_URL}"
    (cd "$(dirname "${VMLINUX_SRC}")" && tar -xJf "${tarball}")
    rm -f "${tarball}"
    [[ -d "${VMLINUX_SRC}" ]] || die "extraction did not produce ${VMLINUX_SRC}"
fi
log "source tree: ${VMLINUX_SRC}"

# ── Apply config ───────────────────────────────────────────────────────────
mkdir -p "${VMLINUX_OUT}"

if [[ ! -f "${VMLINUX_CONFIG}" ]]; then
    die "config template absent at ${VMLINUX_CONFIG}. Run scripts/release/seed-vmlinux-config.sh first, or supply ELASTOS_VMLINUX_CONFIG=<path>."
fi

cp "${VMLINUX_CONFIG}" "${VMLINUX_SRC}/.config"
log "applied config template from ${VMLINUX_CONFIG}"
log "running olddefconfig to materialise the full .config from the template seeds"
(
    cd "${VMLINUX_SRC}"
    "${GMAKE}" -j1 ARCH=arm64 CROSS_COMPILE="${CROSS_COMPILE}" olddefconfig
) >"${VMLINUX_OUT}/build.log" 2>&1 || {
    tail -40 "${VMLINUX_OUT}/build.log" >&2
    die "olddefconfig failed; see ${VMLINUX_OUT}/build.log" 2
}

# ── Cross-compile ──────────────────────────────────────────────────────────
ncpu="$(sysctl -n hw.ncpu 2>/dev/null || nproc 2>/dev/null || echo 4)"
log "building ARCH=arm64 with -j${ncpu} … (this takes ~20–40 min on Apple Silicon)"
log "build log → ${VMLINUX_OUT}/build.log"

start_epoch="$(date +%s)"
(
    cd "${VMLINUX_SRC}"
    "${GMAKE}" -j"${ncpu}" ARCH=arm64 CROSS_COMPILE="${CROSS_COMPILE}" Image
) >>"${VMLINUX_OUT}/build.log" 2>&1 || {
    tail -80 "${VMLINUX_OUT}/build.log" >&2
    die "kernel build failed; see ${VMLINUX_OUT}/build.log for the full transcript" 2
}
elapsed=$(( $(date +%s) - start_epoch ))
log "build completed in ${elapsed}s"

# ── Stage output ───────────────────────────────────────────────────────────
IMAGE_SRC="${VMLINUX_SRC}/arch/arm64/boot/Image"
IMAGE_OUT="${VMLINUX_OUT}/Image"
[[ -f "${IMAGE_SRC}" ]] || die "expected Image at ${IMAGE_SRC} but build did not produce it"
cp "${IMAGE_SRC}" "${IMAGE_OUT}"

# ── Verify (byte-magic check, identical to runtime's looks_like_arm64_image) ─
python3 - <<PY || die "byte-magic check failed — produced Image is not a valid ARM64 kernel" 3
import sys
data = open("${IMAGE_OUT}", "rb").read()
if len(data) <= 0x44:
    sys.exit(1)
if data[0x38:0x3c] != b"ARMd":
    sys.stderr.write(f"offset 0x38..0x3c = {data[0x38:0x3c]!r}, expected b'ARMd'\n")
    sys.exit(1)
if data[0x40:0x44] != b"PE\\x00\\x00":
    sys.stderr.write(f"offset 0x40..0x44 = {data[0x40:0x44]!r}, expected b'PE\\\\x00\\\\x00'\n")
    sys.exit(1)
sys.exit(0)
PY
log "byte-magic check passed: ${IMAGE_OUT} is a valid ARM64 Linux Image"

shasum -a 256 "${IMAGE_OUT}" | awk '{print "sha256:" $1}' > "${IMAGE_OUT}.sha256"
SIZE_BYTES="$(stat -f%z "${IMAGE_OUT}" 2>/dev/null || stat -c%s "${IMAGE_OUT}")"
echo "${SIZE_BYTES}" > "${IMAGE_OUT}.size"

# ── Print operator handoff summary ─────────────────────────────────────────
cat <<EOF

╔═══════════════════════════════════════════════════════════════════════════
║ vmlinux-darwin-arm64 build complete
╠═══════════════════════════════════════════════════════════════════════════
║ Image:    ${IMAGE_OUT}
║ Checksum: $(cat "${IMAGE_OUT}.sha256")
║ Size:     ${SIZE_BYTES} bytes
║ Build:    ${elapsed}s  (log: ${VMLINUX_OUT}/build.log)
╠═══════════════════════════════════════════════════════════════════════════
║ Next step (operator handoff to commit the artifact):
║
║   1. Update components.json with the real values:
║
║      jq --arg cs "$(cat "${IMAGE_OUT}.sha256")" \\
║         --argjson sz "${SIZE_BYTES}" \\
║         '.external.vmlinux.platforms["darwin-arm64"].checksum = \$cs |
║          .external.vmlinux.platforms["darwin-arm64"].size = \$sz' \\
║         components.json > components.json.tmp \\
║      && mv components.json.tmp components.json
║
║   2. Re-run the verifier to confirm green:
║
║      bash scripts/lib/components-json-verify.sh
║
║   3. Commit components.json with message:
║      "Phase 6 Day 4b — populate vmlinux-darwin-arm64 checksum + size"
╚═══════════════════════════════════════════════════════════════════════════
EOF
