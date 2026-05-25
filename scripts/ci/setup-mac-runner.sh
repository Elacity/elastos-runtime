#!/usr/bin/env bash
# scripts/ci/setup-mac-runner.sh — Phase 6 Day 5 (5a, agent-shipped).
#
# One-shot bootstrap recipe for a self-hosted **macOS Apple-Silicon**
# GitHub Actions runner targeting the `mac-vz-full-boot` lane in
# `.github/workflows/mac-vz.yml`. Sibling to Day-4a's
# `scripts/build-vmlinux-arm64.sh` and `scripts/release-mac.sh` — same
# split: this script is the **reproducible recipe**; the operator
# (Day 5b) runs it on the physical machine to actually provision the
# runner.
#
# What this script does (in order, idempotent where possible):
#   1. Preflight HW/OS — confirm arm64 + macOS 13+ + RAM/disk floor.
#   2. Toolchain install — Xcode CLT + rustup stable.
#   3. Vz framework presence check.
#   4. components.json verifier (delegates to
#      scripts/lib/components-json-verify.sh).
#   5. Day-4b artifact probe — sniff for an operator-built vmlinux
#      Image, verify its sha256 against components.json when populated;
#      log a clear "pending Day-4b" message when not.
#   6. Operator handoff — print the exact `gh` + `svc.sh` commands the
#      operator runs next to finish runner activation (Day 5b).
#
# What this script does NOT do (deliberately deferred to Day 5b):
#   - Register the GitHub Actions runner agent (`./config.sh`,
#     `./svc.sh`). The agent is downloaded per-repo from the GitHub UI
#     under `Settings → Actions → Runners → New self-hosted runner`,
#     and the registration token is short-lived (single-use, ~1h
#     validity), so it can't be baked into a script.
#   - Set the `MAC_VZ_FULL_BOOT_ENABLED` repository variable. That's a
#     `gh variable set` invocation requiring repo-admin credentials in
#     the shell; we print the exact command instead.
#   - Fetch the operator-built artefacts. Those come from the release
#     pipeline (post-Day-4b) and are NOT a setup-script concern.
#
# Exit codes (typed; the operator can wire `&&` chains on these):
#   0    All checks green; provisioning ready for Day-5b handoff.
#   1    HW/OS prerequisite failed (Intel Mac, macOS < 13, disk/RAM
#        floor not met). Diagnostic on stderr.
#   2    Toolchain install failed (xcode-select / rustup error).
#        Stderr captures the underlying failure.
#   3    Virtualization.framework absent (should be impossible on a
#        supported macOS — fail hard so operator notices).
#   4    components.json verifier failed (drift in the manifest;
#        re-run Phase 6 Day 1–4 verifier).
#
# Re-runnable: the script avoids destructive operations. Re-running it
# on a partially-provisioned machine completes the gaps and reports a
# clean ledger of "what was already done" vs "what was added".
#
# Anchors:
#   - docs/vz-backend/SELF_HOSTED_RUNNER_SPEC.md (contract this fulfils)
#   - docs/vz-backend/PHASE_6_PLAN.md § Day 5
#   - docs/vz-backend/PHASE_6_DAY_4_NOTES.md § 4 (Day-4b operator queue)
#   - scripts/lib/components-json-verify.sh (delegated verifier)
#   - scripts/build-vmlinux-arm64.sh (Day-4b artefact producer)

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"

# Hardware/OS floors are taken verbatim from SELF_HOSTED_RUNNER_SPEC.md
# § 2 so a future spec update only has to edit one place.
readonly MIN_MACOS_MAJOR=13
readonly MIN_RAM_GB=16
readonly MIN_FREE_DISK_GB=100
readonly REQUIRED_LABELS="self-hosted,macOS,ARM64,vz-capable"
readonly REPO_VAR_NAME="MAC_VZ_FULL_BOOT_ENABLED"

# Operator-visible data dir; mirrors `cross_platform_data_dir` in
# scripts/lib/cross-platform.sh. Kept literal here because we need it
# before sourcing helpers (and to keep the recipe single-file readable).
DATA_DIR="${XDG_DATA_HOME:-$HOME/.local/share}/elastos"

log()  { printf '[setup-mac-runner] %s\n' "$*"; }
warn() { printf '[setup-mac-runner] WARN: %s\n' "$*" >&2; }
die()  { printf '[setup-mac-runner] ERROR: %s\n' "$*" >&2; exit "${2:-1}"; }
hr()   { printf '%s\n' "── $* ────────────────────────────────────────"; }

# ── 1. HW / OS preflight ──────────────────────────────────────────────────

hr "1. HW/OS preflight"

if [[ "$(uname -s)" != "Darwin" ]]; then
    die "this script only runs on macOS (uname=$(uname -s))" 1
fi

ARCH="$(uname -m)"
if [[ "${ARCH}" != "arm64" ]]; then
    die "Apple Silicon (arm64) required; got '${ARCH}'. Intel Macs are out of scope for Phase 6 (see PLAN.md L337)." 1
fi
log "architecture: arm64 (Apple Silicon) ✓"

# sw_vers prints e.g. `14.4.1`. Take the major component.
MACOS_VER="$(sw_vers -productVersion 2>/dev/null || echo 0)"
MACOS_MAJOR="${MACOS_VER%%.*}"
if [[ -z "${MACOS_MAJOR}" || "${MACOS_MAJOR}" -lt "${MIN_MACOS_MAJOR}" ]]; then
    die "macOS ${MIN_MACOS_MAJOR}+ required for the Vz APIs the runtime uses; got '${MACOS_VER}'." 1
fi
log "macOS: ${MACOS_VER} (>= ${MIN_MACOS_MAJOR}) ✓"

# `sysctl hw.memsize` returns total bytes.
RAM_BYTES="$(sysctl -n hw.memsize 2>/dev/null || echo 0)"
RAM_GB=$(( RAM_BYTES / 1024 / 1024 / 1024 ))
if [[ "${RAM_GB}" -lt "${MIN_RAM_GB}" ]]; then
    die "RAM ${MIN_RAM_GB}+ GB required; got ${RAM_GB} GB. Vz microVMs cap at 4 GiB each; spec § 2 requires ${MIN_RAM_GB} GB for host+runner+capsule headroom." 1
fi
log "RAM: ${RAM_GB} GB (>= ${MIN_RAM_GB}) ✓"

# `df -g` reports gigabytes on macOS BSD coreutils; column 4 is `Avail`.
FREE_DISK_GB="$(df -g "${HOME}" 2>/dev/null | awk 'NR==2 {print $4}' || echo 0)"
if [[ -z "${FREE_DISK_GB}" || "${FREE_DISK_GB}" -lt "${MIN_FREE_DISK_GB}" ]]; then
    die "free disk ${MIN_FREE_DISK_GB}+ GB required on \$HOME volume; got ${FREE_DISK_GB} GB. Rootfs caches + overlays can grow during long runs." 1
fi
log "free disk on \$HOME: ${FREE_DISK_GB} GB (>= ${MIN_FREE_DISK_GB}) ✓"

# ── 2. Toolchain ──────────────────────────────────────────────────────────

hr "2. Toolchain"

# 2a. Xcode Command-Line Tools — required for codesign, ld, etc.
if xcode-select -p >/dev/null 2>&1; then
    log "Xcode CLT: $(xcode-select -p) ✓"
else
    log "Xcode CLT absent; triggering 'xcode-select --install'…"
    log "  (this opens a GUI prompt; the operator must complete the install before re-running this script)"
    xcode-select --install 2>&1 || die "xcode-select --install failed" 2
    die "Xcode CLT install is async (operator must accept GUI prompt). Re-run this script after the install completes." 2
fi

# 2b. Rust stable — workflow caches via Swatinem/rust-cache with
# prefix-key=mac-vz-self-hosted; the toolchain itself has to exist on
# the runner because Actions runs scoped to the runner's PATH.
if command -v rustc >/dev/null 2>&1; then
    RUSTC_VER="$(rustc --version 2>/dev/null || echo unknown)"
    log "rustc: ${RUSTC_VER} ✓"
else
    log "rustc absent; installing via rustup stable…"
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
        | sh -s -- -y --default-toolchain stable --no-modify-path \
        || die "rustup install failed" 2
    # shellcheck disable=SC1090
    source "$HOME/.cargo/env"
    log "rustc installed: $(rustc --version)"
fi

# 2c. clippy + rustfmt — workflow runs both. Idempotent component-add.
rustup component add clippy rustfmt >/dev/null 2>&1 || warn "rustup component add (clippy/rustfmt) returned non-zero; may be harmless if already installed"

# ── 3. Vz framework presence ──────────────────────────────────────────────

hr "3. Virtualization.framework"

if [[ -d /System/Library/Frameworks/Virtualization.framework ]]; then
    log "Virtualization.framework PRESENT ✓"
else
    die "Virtualization.framework not found at /System/Library/Frameworks/Virtualization.framework. This should not happen on a supported macOS — file a bug." 3
fi

# ── 4. components.json verifier ───────────────────────────────────────────

hr "4. components.json invariants"

VERIFIER="${REPO_ROOT}/scripts/lib/components-json-verify.sh"
if [[ ! -x "${VERIFIER}" ]]; then
    die "verifier script absent: ${VERIFIER}. Phase 6 Day 2+ must be on the checked-out branch." 4
fi

# The verifier emits its own pass/fail line; we just propagate.
if "${VERIFIER}"; then
    log "components.json verifier: green ✓"
else
    die "components.json verifier failed; see diagnostics above. Phase 6 Day 1–4 invariants are broken." 4
fi

# ── 5. Day-4b artefact probe ──────────────────────────────────────────────

hr "5. Day-4b artefact probe (informational)"

VMLINUX_INSTALL_PATH="${DATA_DIR}/bin/vmlinux"
VMLINUX_EXPECTED_CHECKSUM="$(jq -r '.external.vmlinux.platforms["darwin-arm64"].checksum // ""' "${REPO_ROOT}/components.json" 2>/dev/null || echo "")"

if [[ ! -f "${VMLINUX_INSTALL_PATH}" ]]; then
    warn "vmlinux not yet present at ${VMLINUX_INSTALL_PATH}"
    warn "  → Day-4b operator handoff pending. Run:"
    warn "      bash ${REPO_ROOT}/scripts/build-vmlinux-arm64.sh"
    warn "    then copy the produced Image to ${VMLINUX_INSTALL_PATH}."
    warn "    (The mac-vz-full-boot smokes will skip until this is done.)"
else
    log "vmlinux PRESENT at ${VMLINUX_INSTALL_PATH}"
    if [[ -n "${VMLINUX_EXPECTED_CHECKSUM}" ]]; then
        ACTUAL_CHECKSUM="sha256:$(shasum -a 256 "${VMLINUX_INSTALL_PATH}" | awk '{print $1}')"
        if [[ "${ACTUAL_CHECKSUM}" == "${VMLINUX_EXPECTED_CHECKSUM}" ]]; then
            log "vmlinux sha256 matches components.json: ${ACTUAL_CHECKSUM} ✓"
        else
            die "vmlinux sha256 MISMATCH! Expected ${VMLINUX_EXPECTED_CHECKSUM}, got ${ACTUAL_CHECKSUM}. Rebuild via scripts/build-vmlinux-arm64.sh." 4
        fi
    else
        warn "components.json darwin-arm64 checksum is empty (Day-4b not committed yet)."
        warn "  → cannot verify the local vmlinux Image; trusting it for now."
    fi
fi

# Class-E helpers (kubo / cloudflared / llama-server) — the runtime's
# install.sh fetches these from upstream at first boot; we don't
# pre-warm them here because the smokes' install lane is itself part of
# what the runner validates. Just report whether they're already
# cached so the operator knows whether the first run will pay the
# network cost.
log "Class-E helpers cache state:"
for helper in kubo cloudflared llama-server; do
    if [[ -f "${DATA_DIR}/bin/${helper}" ]]; then
        printf '  %-14s  cached (%s)\n' "${helper}" "${DATA_DIR}/bin/${helper}"
    else
        printf '  %-14s  not cached (will be fetched on first smoke run)\n' "${helper}"
    fi
done

# ── 6. Operator handoff ───────────────────────────────────────────────────

hr "6. Operator handoff (Day 5b — runner registration)"

cat <<EOF

╔═══════════════════════════════════════════════════════════════════════════
║ Provisioning preflight GREEN. Continue with Day-5b operator handoff.
╠═══════════════════════════════════════════════════════════════════════════
║
║ STEP A — Download + register the GitHub Actions runner agent
║   1. From the GitHub UI: Settings → Actions → Runners →
║      "New self-hosted runner" → macOS / ARM64.
║   2. Copy the registration token from the page.
║   3. Run the displayed commands; when prompted for labels, type
║      EXACTLY this set (no extras, no missing):
║
║        ${REQUIRED_LABELS}
║
║      Spec § 3 mandates the four-label set; any subset prevents
║      mac-vz-full-boot and _self-hosted-probe.yml from scheduling.
║
║   4. Install the runner as a launch-agent so it survives reboots:
║
║        ./svc.sh install
║        ./svc.sh start
║
║      (run from the actions-runner directory the GitHub installer
║      created in step 3.)
║
║ STEP B — Enable the lane via the repository variable
║
║   Method 1: gh CLI (preferred, scriptable):
║     gh variable set ${REPO_VAR_NAME} \\
║         --repo <owner>/<repo> \\
║         --body true
║
║   Method 2: GitHub UI:
║     Settings → Secrets and variables → Actions → Variables →
║     New repository variable. Name: ${REPO_VAR_NAME}. Value: true.
║
║   Effect is immediate; the next mac-vz.yml workflow run picks it up.
║
║ STEP C — Verify
║
║   1. From the Actions UI, run _self-hosted-probe.yml manually
║      (workflow_dispatch). It must complete < 1 min with
║      "Virtualization.framework PRESENT" printed.
║   2. Push a commit (or workflow_dispatch mac-vz.yml). The
║      mac-vz-full-boot job should schedule on this runner and
║      run the 3 Phase-5 smokes with ELASTOS_VZ_SMOKE_FORCE_FULL=1.
║
║ KILL SWITCHES (in case of trouble; either is immediate):
║
║   - gh variable delete ${REPO_VAR_NAME} --repo <owner>/<repo>
║   - Remove the 'vz-capable' label from the runner in the GitHub UI
║     (or take the runner offline).
║
║ See docs/vz-backend/SELF_HOSTED_RUNNER_SPEC.md § 5 for the full
║ security posture + on-going operational guidance.
╚═══════════════════════════════════════════════════════════════════════════
EOF
