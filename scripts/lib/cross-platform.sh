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

# Terminate a process tree cleanly. **Phase 5 Day 2.**
#
# POSIX-clean replacement for the `kill -- "-${pid}" || kill
# "${pid}"` pattern that several smokes use today. The
# semantics differ between GNU and BSD `kill` for the negative
# argument (Linux accepts both `kill -- -N` and `kill -N`; BSD
# accepts `kill -- -N` and returns non-zero if no children
# exist), so the pattern needs a single audited helper.
#
# Sequence:
#   1. If $pid is empty / non-numeric, no-op return 0
#      (callers reading from coords files often pass through
#      missing values; tripping `set -u` here would mask
#      real bugs further up the stack).
#   2. Send SIGTERM to the process group, then to the bare PID.
#      Either is fine; both are tried so a daemonised child
#      that escaped its group still receives a kill.
#   3. Poll `pid_is_running` every 100 ms for $grace_secs
#      seconds (default 2 s).
#   4. If still alive after the grace window, escalate to
#      SIGKILL on both the group and the bare PID.
#   5. Return 0 unconditionally — kill failures (already-dead
#      processes, etc.) are not the caller's problem.
#
# Usage:
#   kill_pid_then_group 12345        # 2 s grace
#   kill_pid_then_group 12345 5      # 5 s grace
kill_pid_then_group() {
    local pid="${1:-}"
    local grace_secs="${2:-2}"
    [[ -z "$pid" ]] && return 0
    # Reject non-numeric PIDs — they're nonsense and could
    # confuse `kill` into interpreting them as signal names.
    case "$pid" in
        ''|*[!0-9]*) return 0 ;;
    esac
    if ! pid_is_running "$pid"; then
        return 0
    fi
    kill -- "-${pid}" 2>/dev/null || true
    kill "${pid}" 2>/dev/null || true
    local i
    # ${grace_secs} * 10 iterations at 100 ms each. Capped at
    # 100 (10 s) to bound the worst case; callers wanting
    # longer can run the helper twice.
    local max_iters="$((grace_secs * 10))"
    [[ "$max_iters" -gt 100 ]] && max_iters=100
    for ((i = 0; i < max_iters; i++)); do
        if ! pid_is_running "$pid"; then
            return 0
        fi
        sleep 0.1
    done
    kill -KILL -- "-${pid}" 2>/dev/null || true
    kill -KILL "${pid}" 2>/dev/null || true
    return 0
}

# Bind a fresh ephemeral TCP port via python3 and print it to
# stdout. **Phase 5 Day 2.** Hoisted from the inline copies in
# `local-carrier-setup-smoke.sh` (L38-45) and
# `home-frontdoor-smoke.sh`'s `free_port()` (L44-53) so future
# smokes can DRY.
#
# Returns the port number on stdout. Exits non-zero if python3
# fails (callers should treat that as a hard error — the smoke
# can't run without a port). The port is RELEASED before
# return; callers must bind it promptly or live with the
# inherent TOCTOU race (same posture as the existing inline
# implementations).
free_port_via_python3() {
    python3 - <<'PY'
import socket
import sys

with socket.socket() as s:
    s.bind(("127.0.0.1", 0))
    port = s.getsockname()[1]

if port < 1024 or port > 65535:
    sys.stderr.write(f"free_port_via_python3: bogus port {port}\n")
    sys.exit(1)
print(port)
PY
}

# Mac pre-flight: assert that `components.json` carries
# `darwin-arm64` (or wildcard `*`) release metadata for every
# named native binary. **Phase 5 Day 2.**
#
# Lifts the Day-1 inline Python block out of
# `local-carrier-setup-smoke.sh` and makes it shareable across
# Days 1 / 2 / 3 / Phase-6 install smokes. The check is
# read-only — no mutation of components.json. On any missing
# entry, prints the actionable Phase-6 message on stderr and
# returns 1; on success, returns 0 silently.
#
# Linux callers: the check is unconditional (the helper
# doesn't gate on OS). Linux's components.json already carries
# `linux-amd64` / `linux-arm64` entries for the same binaries
# so callers running on Linux always pass through this helper
# successfully — that's the byte-identical guarantee.
#
# Usage:
#   cross_platform_assert_native_binary_release_metadata \
#       "${REPO_ROOT}/components.json" \
#       shell localhost-provider did-provider webspace-provider
cross_platform_assert_native_binary_release_metadata() {
    local manifest_path="$1"
    shift
    if [[ -z "$manifest_path" || ! -f "$manifest_path" ]]; then
        echo "[cross-platform] components.json not found at: ${manifest_path}" >&2
        return 1
    fi
    if [[ $# -eq 0 ]]; then
        echo "[cross-platform] no native-binary names supplied to assert" >&2
        return 1
    fi
    # The check key. On Darwin we require darwin-arm64 today
    # (Phase 6 deliverable per PLAN.md L321). On any other OS
    # the platform is host-shaped (e.g. linux-amd64); callers
    # passing this helper on Linux already have those entries.
    local platform_key
    case "$(uname -s)" in
        Darwin) platform_key="darwin-arm64" ;;
        Linux)
            case "$(uname -m)" in
                x86_64)        platform_key="linux-amd64" ;;
                aarch64|arm64) platform_key="linux-arm64" ;;
                *)             platform_key="linux-unknown" ;;
            esac
            ;;
        *) platform_key="unknown" ;;
    esac
    local names_csv
    names_csv="$(printf '%s,' "$@")"
    names_csv="${names_csv%,}"
    MANIFEST_PATH="$manifest_path" \
    PLATFORM_KEY="$platform_key" \
    NAMES_CSV="$names_csv" \
    python3 - <<'PY' || return 1
import json
import os
import sys

manifest_path = os.environ["MANIFEST_PATH"]
platform_key = os.environ["PLATFORM_KEY"]
names = [n for n in os.environ["NAMES_CSV"].split(",") if n]

try:
    manifest = json.loads(open(manifest_path).read())
except Exception as e:
    sys.stderr.write(
        f"[cross-platform] components.json unparseable: {manifest_path}: {e}\n"
    )
    sys.exit(1)

missing = []
for name in names:
    plats = (
        manifest.get("external", {}).get(name, {}).get("platforms")
    ) or {}
    if platform_key not in plats and "*" not in plats:
        missing.append(name)

if missing:
    sys.stderr.write(
        f"[cross-platform] components.json missing {platform_key} entries for: "
        f"{', '.join(missing)}\n"
    )
    sys.exit(1)
sys.exit(0)
PY
}

# Probe a URL via `curl --head` and emit a structured skip on
# non-2xx / timeout. **Phase 5 Day 3.** Hoist of the
# publisher-gateway dependency check the Day-3 chat-wasm
# interop smoke needs before paying for an `install.sh` curl
# pipe to bash.
#
# Returns 0 (reachable) if `curl -fsS --head --max-time 5
# "$url"` succeeds. Returns 1 (unreachable) otherwise, after
# printing the operator-facing skip message AND the
# Day-3-standardised escape-hatch reminder (the
# `ELASTOS_CHAT_INTEROP_OFFLINE=1` flag).
#
# This helper does NOT exit — the caller decides whether
# unreachability is a hard fail or a soft skip. Smokes that
# can fall back to a local build call `_OFFLINE=1` and exit 0;
# CI gates that REQUIRE the gateway call this and exit 1.
#
# Usage:
#   if ! cross_platform_curl_or_skip "${PUBLISHER_GATEWAY%/}/install.sh" "[interop]"; then
#       if [[ "${ELASTOS_CHAT_INTEROP_OFFLINE:-0}" == "1" ]]; then
#           echo "[interop] offline mode: skipping curl install"
#       else
#           exit 0
#       fi
#   fi
cross_platform_curl_or_skip() {
    local url="$1"
    local prefix="${2:-[cross-platform]}"
    if [[ -z "$url" ]]; then
        echo "${prefix} cross_platform_curl_or_skip: empty URL supplied" >&2
        return 1
    fi
    if ! command -v curl >/dev/null 2>&1; then
        echo "${prefix} cross_platform_curl_or_skip: curl not installed" >&2
        return 1
    fi
    # `--head` for a cheap HEAD; `-fsS` for failure-on-non-2xx,
    # silent body, but surfaced errors; `--max-time 5` bounds
    # the probe so a hung gateway doesn't pin the smoke for
    # minutes.
    if curl -fsS --head --max-time 5 "$url" >/dev/null 2>&1; then
        return 0
    fi
    cat >&2 <<MSG
${prefix} publisher gateway probe failed: ${url}

  Cause:      curl --head returned non-2xx or timed out within 5 s.
  Effect:     this smoke's published-install assertion cannot complete.
  Mitigation: set ELASTOS_CHAT_INTEROP_OFFLINE=1 to skip the gateway
              install step and fall back to the local-build path
              (requires ELASTOS_BIN_OVERRIDE to point at a built
              \`elastos\` binary).
MSG
    return 1
}

# Grep one or more log files for `VzError::Display` tokens.
# **Phase 5 Day 3.** The Day-3 alerting hook that turns the
# Phase-4-Day-7 `Display` contract into an active smoke-level
# tripwire.
#
# `VzError::Display` (anchored in `elastos-vz/src/error.rs`'s
# `vz_error_display_includes_kind_label_for_log_grep` test)
# guarantees every rendered `VzError` starts with one of the
# nine stable `kind_label:` tokens:
#   - vz_internal:
#   - vz_invalid_configuration:
#   - vz_invalid_state:
#   - vz_invalid_state_transition:
#   - vz_network_error:
#   - vz_operation_cancelled:
#   - vz_not_supported:
#   - vz_timed_out:
#   - vz_unknown:
# Operators alerting on Vz failures grep for those tokens. The
# Day-3 smoke runs this helper at its tail across every log it
# collected; if any token is found, the smoke fails LOUDLY
# with the matched lines and a runbook pointer.
#
# Returns 0 if NO tokens found (the happy path); returns 1 if
# any token found, having printed the matched lines + a
# runbook pointer to stderr.
#
# Usage:
#   if ! cross_platform_alert_on_vz_error_in_logs \
#           "${install_log}" "${setup_log}" "${session_log}"; then
#       exit 1
#   fi
cross_platform_alert_on_vz_error_in_logs() {
    if [[ $# -eq 0 ]]; then
        echo "[cross-platform] cross_platform_alert_on_vz_error_in_logs: no log paths supplied" >&2
        return 1
    fi
    # Build the existing-files list. Missing files are
    # silently ignored — log files are best-effort outputs
    # and the smoke may legitimately have skipped a phase.
    local files=()
    local f
    for f in "$@"; do
        [[ -n "$f" && -f "$f" ]] && files+=("$f")
    done
    if [[ ${#files[@]} -eq 0 ]]; then
        # No files to inspect: treat as no-alerts. The smoke's
        # primary assertions already covered the happy path.
        return 0
    fi
    # Token regex — the nine stable kind_label prefixes from
    # `VzError::Display`. Use grep -E (POSIX ERE) so the
    # pattern is portable across GNU grep (Linux) and BSD grep
    # (macOS). `-l` would list filenames only; we want the
    # actual lines so operators can see the kind_label +
    # description in one go.
    local pattern='vz_(internal|invalid_configuration|invalid_state|invalid_state_transition|network_error|operation_cancelled|not_supported|timed_out|unknown):'
    local matches
    matches="$(grep -E -H -n "$pattern" "${files[@]}" 2>/dev/null || true)"
    if [[ -z "$matches" ]]; then
        return 0
    fi
    cat >&2 <<MSG
[cross-platform] ALERT: VzError kind_label tokens found in collected logs.

  Phase:     Phase 5 Day 3 alerting hook on the Phase 4 Day 7
             VzError::Display contract.
  Tokens:    vz_internal: / vz_invalid_configuration: / vz_invalid_state: /
             vz_invalid_state_transition: / vz_network_error: /
             vz_operation_cancelled: / vz_not_supported: / vz_timed_out: /
             vz_unknown:

  Runbook:   See docs/vz-backend/PHASE_4_DAY_7_NOTES.md and
             docs/vz-backend/PHASE_4_DAY_8_NOTES.md for the typed-error
             surface; the kind_label is the stable telemetry label.

  Matched lines (file:line:content):
MSG
    printf '%s\n' "$matches" >&2
    return 1
}

# Print the actionable Phase-6 operator message and exit 0.
# **Phase 5 Day 2.** Hoisted from Day 1's inline `cat >&2`
# block so Day-2's home-frontdoor smoke + future smokes share
# the same operator-facing wording. The message names the
# phase, the file, and the escape hatches.
cross_platform_print_phase6_skip_message() {
    cat >&2 <<'MSG'
[cross-platform] Mac pre-flight: components.json has no darwin-arm64 release metadata for one or more native binaries.

  Phase:      Phase 6 deliverable (see docs/vz-backend/PLAN.md L321).
  Status:     Pre-Work removed the dishonest darwin entries; Phase 6
              restores truthful ones once Mac substrate + signing land.

  This smoke validates the Carrier-backed install pipeline for the
  native-binary path, which cannot complete on Mac until the metadata
  is restored. The Phase-5 Day-1/2 deliverables (script ports, bash 3.2
  portability, helper library, Vz substrate probe) are unaffected and
  have already been validated by reaching this point.

  To skip this guard and exercise the WASM/data half regardless,
  rerun with: ELASTOS_VZ_SMOKE_FORCE_PROCEED=1 ...

  To dry-run only (CI fast lane), set: ELASTOS_VZ_SMOKE_DRY_RUN=1 ...
MSG
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
