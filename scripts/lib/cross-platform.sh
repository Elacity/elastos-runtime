#!/usr/bin/env bash
# Phase 5 Day 1 — cross-platform shell helpers used by the Mac
# smoke port.
#
# Contract:
#   - bash 3.2 clean (macOS default). No `mapfile`, no
#     `readarray`, no `${var,,}` lowercase substitution.
#   - No `/proc/<pid>` checks (Linux-only) — every PID-liveness
#     check uses `kill -0 PID` (POSIX, works on both Linux and
#     BSD/macOS).
#   - No GNU-only `pgrep` flags. `pgrep -f` is POSIX-portable
#     and used identically on both Linux and macOS.
#
# Sourced by:
#   - scripts/local-carrier-setup-smoke.sh (Phase 5 Day 1)
#   - scripts/home-frontdoor-smoke.sh       (Phase 5 Day 2 —
#     planned)
#   - scripts/chat-wasm-native-interop-smoke.sh (Phase 5 Day 3
#     — planned)

# Returns 0 if the named process is alive, non-zero otherwise.
# Cross-platform replacement for `[[ -d "/proc/$pid" ]]` checks.
# Empty PID is treated as "not running" (consistent with the
# behaviour callers want when they read a missing pid from a
# coords file).
pid_is_running() {
    local pid="${1:-}"
    [[ -z "$pid" ]] && return 1
    kill -0 "$pid" 2>/dev/null
}

# Read newline-separated PIDs from stdin into a bash array
# named by $1. Bash-3.2-clean replacement for
# `mapfile -t arr < <(...)`.
#
# Usage:
#   declare -a my_pids
#   read_pids_into_array my_pids < <(pgrep -f "$root" || true)
read_pids_into_array() {
    local array_name="$1"
    local line
    eval "${array_name}=()"
    while IFS= read -r line; do
        [[ -z "$line" ]] && continue
        eval "${array_name}+=(\"\$line\")"
    done
}

# Detect whether Vz is plausibly usable on this host. **Mac
# only.** Linux callers can treat the return as "no" and route
# through the existing crosvm path.
#
# Returns 0 (yes) if BOTH:
#   - The host OS is Darwin.
#   - sw_vers reports a major version >= 12 (Apple's Vz
#     Linux-guest support landed in macOS 12 / Monterey).
#
# Does NOT call into `Virtualization.framework` itself —
# Phase 1's `is_supported()` does the real check via objc2
# bindings. This helper is just the cheap pre-flight smokes
# can run BEFORE building the Rust binary.
vz_host_is_capable() {
    [[ "$(uname -s)" == "Darwin" ]] || return 1
    command -v sw_vers >/dev/null 2>&1 || return 1
    local product_version
    product_version="$(sw_vers -productVersion 2>/dev/null || echo "")"
    [[ -n "$product_version" ]] || return 1
    local major
    major="${product_version%%.*}"
    [[ -n "$major" ]] || return 1
    # Reject non-numeric majors defensively (bash 3.2 has no
    # `[[ $major =~ ^[0-9]+$ ]]` portability concern but the
    # check is cheap and the failure mode would be silent
    # otherwise).
    case "$major" in
        ''|*[!0-9]*) return 1 ;;
    esac
    [[ "$major" -ge 12 ]]
}

# Locate an installed Vz-launchable capsule on the host.
# **Phase 5 Day 1.** Returns 0 and prints the capsule name on
# stdout if a `<data_dir>/capsules/<name>/rootfs.ext4` plus a
# parseable `capsule.json` is found. Returns non-zero
# otherwise — callers visibly-skip in that case.
#
# Mirrors the discovery logic in
# `elastos-server/tests/vz_supervisor_smoke.rs`'s
# `discover_smoke_capsule` (Phase 4 Day 4). Kept intentionally
# simple — the Rust test is the source of truth for the full
# manifest filter; this helper only proves "something is
# launchable" for the shell smoke.
vz_discover_launchable_capsule() {
    local data_dir="${1:-${ELASTOS_DATA_DIR:-${HOME}/.local/share/elastos}}"
    [[ -d "${data_dir}/capsules" ]] || return 1
    local capsule_dir name
    for capsule_dir in "${data_dir}"/capsules/*/; do
        [[ -d "$capsule_dir" ]] || continue
        [[ -f "${capsule_dir}/rootfs.ext4" ]] || continue
        [[ -f "${capsule_dir}/capsule.json" ]] || continue
        name="$(basename "${capsule_dir}")"
        printf '%s\n' "$name"
        return 0
    done
    return 1
}
