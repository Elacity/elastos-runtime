#!/usr/bin/env bash
# scripts/ci/local-day6-smoke.sh — Phase 6 Day 6a (agent-shipped).
#
# One-command local lane for the three Phase-5 smokes under
# `ELASTOS_VZ_SMOKE_FORCE_FULL=1`. Designed for the operator who
# wants to validate the Phase-6 substrate on a dev Mac **without**
# provisioning a separate self-hosted GitHub Actions runner.
#
# Lane comparison:
#   - GitHub-hosted (`macos-latest`) → no Vz → dry-run only.
#   - Self-hosted GitHub runner       → real Vz → needs a dedicated Mac.
#   - Local-dev lane (this script)    → real Vz → uses this dev Mac.
#
# The local lane is the most pragmatic substrate-validation path on
# Phase-6 Day-6: it produces real `mac-vz-full-boot`-equivalent runs
# without the operator overhead of registering a self-hosted runner.
#
# What this script does:
#   1. Preflight — verify the dev Mac is provision-ready (delegate to
#      setup-mac-runner.sh's first 4 stages, skipping the operator-
#      handoff block).
#   2. Build — `cargo build` the debug `elastos` binary (the smokes
#      hardcode `target/debug/elastos`; they don't honor BIN_OVERRIDE
#      for the supervisor-launch path).
#   3. Vmlinux — confirm `$DATA_DIR/bin/vmlinux` exists; if not, name
#      the recipe and exit with a typed code so the operator knows
#      exactly what to run next.
#   4. Smokes — run all three with `ELASTOS_VZ_SMOKE_FORCE_FULL=1`
#      (and the chat-interop offline + bin-override pair for the WASM
#      smoke). Capture per-smoke logs + exit codes.
#   5. Triage — emit a structured summary showing pass/fail per smoke
#      + log paths + headline failures + suggested next steps.
#
# Inputs (env vars, all defaulted):
#   - ELASTOS_LOCAL_DAY6_OUT          directory for per-run artefacts.
#                                     Default: ${REPO_ROOT}/elastos/target/local-day6.
#   - ELASTOS_LOCAL_DAY6_SKIP_BUILD   if "1", skip the cargo build step
#                                     (e.g. operator already built it).
#                                     Default: "0".
#   - ELASTOS_LOCAL_DAY6_SKIP_SETUP   if "1", skip the setup-mac-runner.sh
#                                     preflight delegate (e.g. re-running
#                                     after a fix without re-verifying
#                                     HW/OS floors).
#                                     Default: "0".
#
# Exit codes:
#   0  All three smokes green.
#   1  Preflight failed (delegate exit). Diagnostic on stderr.
#   2  Cargo build failed. Stderr captures the cargo output.
#   3  vmlinux Image absent at the expected install path. Recipe path
#      printed in the diagnostic.
#   4  ≥1 smoke failed. Per-smoke detail in the triage block.
#
# Anchors:
#   - docs/vz-backend/PHASE_6_DAY_6_NOTES.md (the day this implements)
#   - docs/vz-backend/PHASE_6_PLAN.md § Day 6
#   - scripts/ci/setup-mac-runner.sh (delegated preflight, Day 5a)
#   - scripts/build-vmlinux-arm64.sh (Day-4b-recipe-but-runs-locally)

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
ELASTOS_ROOT="${REPO_ROOT}/elastos"

LOCAL_DAY6_OUT="${ELASTOS_LOCAL_DAY6_OUT:-${ELASTOS_ROOT}/target/local-day6}"
SKIP_BUILD="${ELASTOS_LOCAL_DAY6_SKIP_BUILD:-0}"
SKIP_SETUP="${ELASTOS_LOCAL_DAY6_SKIP_SETUP:-0}"

DATA_DIR="${XDG_DATA_HOME:-$HOME/.local/share}/elastos"
VMLINUX_INSTALL_PATH="${DATA_DIR}/bin/vmlinux"

log()  { printf '[local-day6] %s\n' "$*"; }
warn() { printf '[local-day6] WARN: %s\n' "$*" >&2; }
die()  { printf '[local-day6] ERROR: %s\n' "$*" >&2; exit "${2:-1}"; }
hr()   { printf '\n── %s ────────────────────────────────────────\n' "$*"; }

mkdir -p "${LOCAL_DAY6_OUT}"

# ── 1. Preflight (delegate) ───────────────────────────────────────────────

hr "1. Preflight (delegate to setup-mac-runner.sh)"
if [[ "${SKIP_SETUP}" == "1" ]]; then
    log "SKIP (ELASTOS_LOCAL_DAY6_SKIP_SETUP=1)"
else
    bash "${REPO_ROOT}/scripts/ci/setup-mac-runner.sh" > "${LOCAL_DAY6_OUT}/preflight.log" 2>&1 || {
        cat "${LOCAL_DAY6_OUT}/preflight.log" >&2
        die "preflight failed; see ${LOCAL_DAY6_OUT}/preflight.log" 1
    }
    log "preflight green (full log: ${LOCAL_DAY6_OUT}/preflight.log)"
fi

# ── 2. Cargo build ────────────────────────────────────────────────────────

hr "2. Cargo build (debug elastos binary)"
ELASTOS_BIN="${ELASTOS_ROOT}/target/debug/elastos"
if [[ "${SKIP_BUILD}" == "1" ]]; then
    [[ -x "${ELASTOS_BIN}" ]] \
        || die "ELASTOS_LOCAL_DAY6_SKIP_BUILD=1 but ${ELASTOS_BIN} is missing or non-executable" 2
    log "SKIP (ELASTOS_LOCAL_DAY6_SKIP_BUILD=1; using existing ${ELASTOS_BIN})"
else
    log "cargo build -p elastos-server (debug)…"
    (
        cd "${ELASTOS_ROOT}"
        cargo build -p elastos-server 2>&1
    ) > "${LOCAL_DAY6_OUT}/cargo-build.log" 2>&1 || {
        tail -40 "${LOCAL_DAY6_OUT}/cargo-build.log" >&2
        die "cargo build failed; see ${LOCAL_DAY6_OUT}/cargo-build.log" 2
    }
    [[ -x "${ELASTOS_BIN}" ]] \
        || die "cargo build claimed success but ${ELASTOS_BIN} is not executable" 2
    log "binary ready: ${ELASTOS_BIN}"
fi

# ── 3. Vmlinux probe ──────────────────────────────────────────────────────

hr "3. Vmlinux Image probe"
if [[ ! -f "${VMLINUX_INSTALL_PATH}" ]]; then
    cat >&2 <<MSG

[local-day6] vmlinux NOT FOUND at ${VMLINUX_INSTALL_PATH}.

The smokes will fail at the LaunchMicroVm step without a kernel
Image. Run the build recipe once (~30–40 min wall-clock on M1/M2):

    brew install aarch64-elf-gcc make elfutils openssl@3 bc jq
    bash ${REPO_ROOT}/scripts/build-vmlinux-arm64.sh
    mkdir -p $(dirname "${VMLINUX_INSTALL_PATH}")
    cp ${ELASTOS_ROOT}/target/vmlinux-darwin-arm64/Image "${VMLINUX_INSTALL_PATH}"

Then re-run this script.
MSG
    exit 3
fi

VMLINUX_SHA="sha256:$(shasum -a 256 "${VMLINUX_INSTALL_PATH}" | awk '{print $1}')"
VMLINUX_SIZE="$(stat -f%z "${VMLINUX_INSTALL_PATH}" 2>/dev/null || stat -c%s "${VMLINUX_INSTALL_PATH}")"
log "vmlinux at ${VMLINUX_INSTALL_PATH}"
log "  sha256: ${VMLINUX_SHA}"
log "  size:   ${VMLINUX_SIZE} bytes"

# ── 4. Smokes ─────────────────────────────────────────────────────────────

hr "4. Running 3 FORCE_FULL smokes"

# Per-smoke command + override-set table (smoke-name → env-var-prefix).
# Hard-coded so the runner contract is auditable from a single place.
SMOKE_NAMES=(local-carrier-setup home-frontdoor chat-wasm-native-interop)
SMOKE_SCRIPTS=(
    "${REPO_ROOT}/scripts/local-carrier-setup-smoke.sh"
    "${REPO_ROOT}/scripts/home-frontdoor-smoke.sh"
    "${REPO_ROOT}/scripts/chat-wasm-native-interop-smoke.sh"
)

# Shared base env every smoke gets. The smokes themselves source
# scripts/lib/cross-platform.sh which uses these env vars to flip
# the FORCE_FULL precedence (see CI_RUNBOOK.md § 3a.1).
COMMON_ENV=(
    "ELASTOS_VZ_SMOKE_FORCE_FULL=1"
)

# chat-wasm needs the offline + override pair because the gateway path
# requires upstream binaries we don't have darwin-arm64 CIDs for yet.
CHAT_INTEROP_ENV=(
    "ELASTOS_CHAT_INTEROP_OFFLINE=1"
    "ELASTOS_BIN_OVERRIDE=${ELASTOS_BIN}"
)

# Build a per-smoke result table.
declare -a SMOKE_RESULTS
declare -a SMOKE_LOGS
declare -a SMOKE_HEADLINES

idx=0
for smoke in "${SMOKE_NAMES[@]}"; do
    script="${SMOKE_SCRIPTS[$idx]}"
    log_path="${LOCAL_DAY6_OUT}/${smoke}.log"
    log "running: ${smoke}"
    log "  script: ${script}"
    log "  log:    ${log_path}"

    # Build the env-var prefix as a printable string for the log,
    # plus the array we'll splice into env(1).
    env_pairs=("${COMMON_ENV[@]}")
    if [[ "${smoke}" == "chat-wasm-native-interop" ]]; then
        env_pairs+=("${CHAT_INTEROP_ENV[@]}")
    fi
    log "  env:    ${env_pairs[*]}"

    smoke_start="$(date +%s)"
    set +e
    env "${env_pairs[@]}" bash "${script}" > "${log_path}" 2>&1
    smoke_rc=$?
    set -e
    smoke_elapsed=$(( $(date +%s) - smoke_start ))

    SMOKE_RESULTS+=("${smoke_rc}")
    SMOKE_LOGS+=("${log_path}")

    if [[ "${smoke_rc}" == "0" ]]; then
        SMOKE_HEADLINES+=("PASS  (${smoke_elapsed}s)")
        log "  result: PASS (${smoke_elapsed}s)"
    else
        # Capture the last non-trace line that looks like an error.
        # Falls back to the last line of the log if nothing matches.
        headline="$(grep -iE '(error|fail|panic|denied|missing|not found|refused)' "${log_path}" | tail -1 || true)"
        if [[ -z "${headline}" ]]; then
            headline="$(tail -1 "${log_path}")"
        fi
        # Truncate to 120 chars so the summary table stays readable.
        headline_short="${headline:0:120}"
        SMOKE_HEADLINES+=("FAIL exit=${smoke_rc} (${smoke_elapsed}s) — ${headline_short}")
        warn "  result: FAIL exit=${smoke_rc} (${smoke_elapsed}s)"
        warn "  headline: ${headline_short}"
    fi
    idx=$(( idx + 1 ))
done

# ── 5. Triage summary ─────────────────────────────────────────────────────

hr "5. Triage summary"

OVERALL_RC=0
for rc in "${SMOKE_RESULTS[@]}"; do
    if [[ "${rc}" != "0" ]]; then
        OVERALL_RC=4
        break
    fi
done

printf '\n'
printf '%-30s  %s\n' "smoke" "result"
printf '%-30s  %s\n' "──────────────────────────────" "──────"
idx=0
for smoke in "${SMOKE_NAMES[@]}"; do
    printf '%-30s  %s\n' "${smoke}" "${SMOKE_HEADLINES[$idx]}"
    idx=$(( idx + 1 ))
done
printf '\nlogs in: %s/\n\n' "${LOCAL_DAY6_OUT}"

if [[ "${OVERALL_RC}" == "0" ]]; then
    cat <<EOF
╔═══════════════════════════════════════════════════════════════════════════
║ Phase 6 Day 6a local-lane: 3/3 GREEN
╠═══════════════════════════════════════════════════════════════════════════
║ Substrate validated end-to-end on this Mac. Next steps:
║
║   1. Capture wall-clock measurements per smoke from the logs.
║   2. Update PHASE_6_DAY_6_NOTES.md § Day-6b with the green-state
║      baseline + the per-smoke timings.
║   3. (Optional) repeat on a registered self-hosted GitHub Actions
║      runner per docs/vz-backend/SELF_HOSTED_RUNNER_SPEC.md to gate
║      the GitHub-side mac-vz-full-boot lane too.
╚═══════════════════════════════════════════════════════════════════════════
EOF
else
    cat <<EOF
╔═══════════════════════════════════════════════════════════════════════════
║ Phase 6 Day 6a local-lane: ≥1 smoke FAILED (overall exit=${OVERALL_RC})
╠═══════════════════════════════════════════════════════════════════════════
║ Triage steps:
║
║   1. Read the per-smoke log (paths above). The headline above is the
║      last regex-matched error/fail/panic line; the full context is
║      typically in the surrounding ~50 lines.
║
║   2. Classify each failure:
║      a) Real Vz substrate bug → file a fix commit, retry.
║      b) Environment issue (sandbox / FD limit / sysctl) → see
║         docs/vz-backend/CI_RUNBOOK.md § "Self-hosted lane" for
║         remediation matrix.
║      c) Stale state from a prior run → 'rm -rf ${DATA_DIR}/.../runtime-coords'
║         and retry. (The smokes use mktemp dirs but the data-dir
║         shape can persist across panics.)
║
║   3. After fixes, re-run with ELASTOS_LOCAL_DAY6_SKIP_SETUP=1 to
║      skip the (unchanged) preflight delegate and save ~5 s.
║
║   4. When all 3 are green, update PHASE_6_DAY_6_NOTES.md per the
║      pass-path block above.
╚═══════════════════════════════════════════════════════════════════════════
EOF
fi

exit ${OVERALL_RC}
