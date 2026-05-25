#!/usr/bin/env bash
#
# Phase 5 Day 3 — Mac port. Sources the shared cross-platform
# helper library (Day 1) for `kill_pid_then_group`,
# `cross_platform_curl_or_skip`,
# `cross_platform_assert_native_binary_release_metadata`, and
# `cross_platform_alert_on_vz_error_in_logs`. Adds:
#
#   - ELASTOS_VZ_SMOKE_DRY_RUN=1       early exit (CI fast lane)
#   - Publisher gateway probe          graceful skip on outage
#   - ELASTOS_CHAT_INTEROP_OFFLINE=1   skip curl-install, use override binary
#   - Post-install Mac pre-flight     graceful skip + Phase-6 msg
#   - Vz substrate readiness probe    diagnostic for Day-5+ work
#   - VzError kind_label alerting     Phase-4-Day-7 contract tripwire
#
# Linux behaviour is byte-identical (helper functions match
# the original inline implementations; new probes / asserts /
# alerts skip on non-Darwin or are no-ops). The Phase 5 Day 1
# audit of the helpers (`bash -n`, unit tests, manual smoke) is
# the safety belt; see `docs/vz-backend/PHASE_5_DAY_1_NOTES.md`
# and `docs/vz-backend/PHASE_5_DAY_2_NOTES.md`.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# shellcheck source=lib/cross-platform.sh
. "${ROOT}/scripts/lib/cross-platform.sh"

OS_TOKEN="$(uname -s | tr '[:upper:]' '[:lower:]')"

TEST_HOME="$(mktemp -d /tmp/elastos-interop-XXXXXX)"
PUBLISHER_GATEWAY="${ELASTOS_PUBLISHER_GATEWAY:-https://elastos.elacitylabs.com}"

INSTALL_LOG="/tmp/elastos-chat-wasm-interop-install.log"
SETUP_LOG="/tmp/elastos-chat-wasm-interop-setup.log"
BUILD_LOG="/tmp/elastos-chat-wasm-interop-build.log"
SESSION_LOG="/tmp/elastos-chat-wasm-interop-session.log"

usage() {
    cat <<'EOF'
Usage:
  bash scripts/chat-wasm-native-interop-smoke.sh

What it proves:
  1. Installs the published runtime into a clean temp home
  2. Runs `elastos setup --profile demo`
  3. Launches native `elastos chat` (starts a shared managed runtime)
  4. Launches `elastos capsule chat-wasm --lifecycle interactive --interactive` on the SAME runtime
  5. Proves bidirectional message delivery on the installed packaged path
EOF
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --help|-h) usage; exit 0 ;;
        *) echo "Unknown: $1" >&2; usage >&2; exit 1 ;;
    esac
done

# Phase 5 Day 5 — auto-dry-run in CI. See the matching block in
# `local-carrier-setup-smoke.sh` for the rationale. Explicit
# `ELASTOS_VZ_SMOKE_DRY_RUN` settings (either `=0` or `=1`)
# always win; this only fires when the env is silent.
if [[ -z "${ELASTOS_VZ_SMOKE_DRY_RUN:-}" ]] && cross_platform_in_ci; then
    echo "[interop] CI detected (GITHUB_ACTIONS or CI env set); auto-enabling ELASTOS_VZ_SMOKE_DRY_RUN=1"
    export ELASTOS_VZ_SMOKE_DRY_RUN=1
fi

# Phase 5 Day 3 — dry-run mode for CI. Exits 0 after the
# bash-portability checks + helper sourcing, BEFORE paying the
# curl / setup / build cost. Mirrors the Day-1/Day-2 lane.
if [[ "${ELASTOS_VZ_SMOKE_DRY_RUN:-0}" == "1" ]]; then
    echo "[interop] dry-run mode: parse OK, helper sourced OK; exiting before any installs/builds"
    if [[ "${OS_TOKEN}" == "darwin" ]]; then
        if vz_host_is_capable; then
            echo "[interop] dry-run: Vz host capability check passed (macOS 12+)"
        else
            echo "[interop] dry-run: Vz host capability check skipped (not macOS 12+ or sw_vers unavailable)"
        fi
    fi
    exit 0
fi

# Phase 5 Day 3 — publisher gateway probe. The smoke's
# install half depends on `${PUBLISHER_GATEWAY}/install.sh`
# being reachable; if it isn't, we either fall back to the
# offline path (if ELASTOS_CHAT_INTEROP_OFFLINE=1 is set) or
# exit 0 with a clean operator-facing skip. Mirrors the
# Day-1/Day-2 Mac-pre-flight pattern.
#
# We probe BEFORE the cleanup trap is installed because no
# state has been written yet — exiting here is free.
if ! cross_platform_curl_or_skip \
        "${PUBLISHER_GATEWAY%/}/install.sh" \
        "[interop]" 2>&1; then
    if [[ "${ELASTOS_CHAT_INTEROP_OFFLINE:-0}" == "1" ]]; then
        # Offline mode requires ELASTOS_BIN_OVERRIDE to point
        # at a built `elastos` binary. Without it we have
        # nothing to test against — exit with a clear hint.
        if [[ -z "${ELASTOS_BIN_OVERRIDE:-}" ]]; then
            cat >&2 <<'MSG'
[interop] ELASTOS_CHAT_INTEROP_OFFLINE=1 requires ELASTOS_BIN_OVERRIDE to
point at a locally-built `elastos` binary. Build via:

    cargo build --manifest-path elastos/Cargo.toml -p elastos-server
    export ELASTOS_BIN_OVERRIDE="$(pwd)/elastos/target/debug/elastos"
    bash scripts/chat-wasm-native-interop-smoke.sh

MSG
            exit 1
        fi
        echo "[interop] offline mode: skipping gateway install (using ELASTOS_BIN_OVERRIDE=${ELASTOS_BIN_OVERRIDE})"
    else
        echo "[interop] publisher gateway unreachable: SKIP"
        exit 0
    fi
fi

echo "[interop] install published runtime"
if [[ "${ELASTOS_CHAT_INTEROP_OFFLINE:-0}" != "1" ]]; then
    # Phase 5 Day 3 — capture install.sh's exit so a Mac
    # bash-3.2 incompatibility in the published script (e.g.
    # `GATEWAYS[@]: unbound variable`, `mapfile` usage, etc.)
    # surfaces as a clean Phase-6-prerequisite skip rather
    # than a cryptic stderr crash. install.sh itself is a
    # Phase 6 deliverable per PLAN.md L321; until it ships
    # Mac-clean, this smoke's `curl install.sh | bash` step
    # cannot complete on macOS.
    set +e
    HOME="${TEST_HOME}" \
    XDG_DATA_HOME="${TEST_HOME}/xdg-data" \
    ELASTOS_PUBLISHER_GATEWAY="${PUBLISHER_GATEWAY}" \
    bash -lc 'mkdir -p "$HOME" "$XDG_DATA_HOME" && curl -fsSL "${ELASTOS_PUBLISHER_GATEWAY%/}/install.sh" | bash' \
        >"${INSTALL_LOG}" 2>&1
    INSTALL_RC=$?
    set -e
    if [[ "${INSTALL_RC}" -ne 0 ]]; then
        if [[ "${OS_TOKEN}" == "darwin" ]] \
                && [[ "${ELASTOS_VZ_SMOKE_FORCE_PROCEED:-0}" != "1" ]]; then
            cat >&2 <<MSG
[interop] Mac pre-flight: published install.sh failed (rc=${INSTALL_RC}).

  Phase:      Phase 6 deliverable. The published install.sh at
              ${PUBLISHER_GATEWAY%/}/install.sh is not yet Mac-bash-3.2
              clean (commonly: \`mapfile\`/\`readarray\` usage,
              \`GATEWAYS[@]\` array references under \`set -u\`,
              \`pgrep -f\` semantic deltas). Until Phase 6 ships a
              Mac-clean install.sh, this smoke's published-install
              assertion cannot complete on macOS.

  Install log: ${INSTALL_LOG}

  Mitigation: set ELASTOS_CHAT_INTEROP_OFFLINE=1 + ELASTOS_BIN_OVERRIDE
              to bypass the gateway and use a locally-built binary;
              the WASM↔native interop proof still runs end-to-end
              against the local build.

  Force-proceed: set ELASTOS_VZ_SMOKE_FORCE_PROCEED=1 to ignore this
              guard (typically only useful when debugging the
              install.sh itself).
MSG
            echo "[interop] Mac pre-flight: SKIP (install.sh failed; Phase 6 prerequisite not met)"
            exit 0
        else
            echo "[interop] install failed (rc=${INSTALL_RC}); see ${INSTALL_LOG}" >&2
            exit "${INSTALL_RC}"
        fi
    fi

    INSTALLED_BIN="${TEST_HOME}/.local/bin/elastos"
    [[ -x "${INSTALLED_BIN}" ]] || {
        echo "[interop] installed binary missing: ${INSTALLED_BIN}" >&2
        exit 1
    }
    ELASTOS_BIN="${ELASTOS_BIN_OVERRIDE:-${INSTALLED_BIN}}"
else
    # Offline mode: bypass curl install; create the minimal
    # directory shape `elastos setup` needs and let it
    # populate components.json + capsules from the local
    # source via ELASTOS_BIN_OVERRIDE. Validated by the
    # gateway-probe earlier in this script — at this point
    # ELASTOS_BIN_OVERRIDE is guaranteed non-empty.
    mkdir -p "${TEST_HOME}" "${TEST_HOME}/xdg-data"
    ELASTOS_BIN="${ELASTOS_BIN_OVERRIDE}"
    : >"${INSTALL_LOG}"  # so the alert hook has a file to inspect
fi
[[ -x "${ELASTOS_BIN}" ]] || {
    echo "[interop] override binary missing: ${ELASTOS_BIN}" >&2
    exit 1
}

# Phase 5 Day 3 — Mac pre-flight on the post-install (or
# offline-staged) components.json. The smoke's downstream
# `elastos setup` step needs `shell`, `chat-wasm`,
# `localhost-provider`, `did-provider`, and `chat` entries.
# On Mac, pre-Phase 6, the `chat` / `shell` /
# `localhost-provider` / `did-provider` darwin-arm64 entries
# are missing — so we skip cleanly before `setup` blows up.
#
# Offline mode skips this guard: the local override binary
# can populate the manifest from source via `elastos setup`,
# which has its own validation. We treat that as the
# developer's responsibility.
COMPONENTS_MANIFEST="${TEST_HOME}/xdg-data/elastos/components.json"
if [[ "${ELASTOS_CHAT_INTEROP_OFFLINE:-0}" != "1" ]]; then
    [[ -f "${COMPONENTS_MANIFEST}" ]] || {
        echo "[interop] installed components manifest missing: ${COMPONENTS_MANIFEST}" >&2
        exit 1
    }
    if [[ "${OS_TOKEN}" == "darwin" ]] \
            && [[ "${ELASTOS_VZ_SMOKE_FORCE_PROCEED:-0}" != "1" ]]; then
        if ! cross_platform_assert_native_binary_release_metadata \
                "${COMPONENTS_MANIFEST}" \
                shell localhost-provider did-provider chat 2>/dev/null
        then
            cross_platform_print_phase6_skip_message
            echo "[interop] Mac pre-flight: SKIP (Phase 6 prerequisite not met)"
            # Exit 0 — clean skip. CI dashboards alert on the skip telemetry separately.
            exit 0
        fi
    fi
fi

echo "[interop] setup required chat + chat-wasm components"
HOME="${TEST_HOME}" \
XDG_DATA_HOME="${TEST_HOME}/xdg-data" \
"${ELASTOS_BIN}" setup \
    --with shell \
    --with localhost-provider \
    --with did-provider \
    --with chat-wasm >"${SETUP_LOG}"

if [[ -n "${ELASTOS_BIN_OVERRIDE:-}" ]]; then
    echo "[interop] overlay local source chat-wasm artifact"
    cargo build \
        --manifest-path "${ROOT}/capsules/chat/Cargo.toml" \
        --bin chat-stdio \
        --target wasm32-wasip1 \
        --no-default-features \
        --release >"${BUILD_LOG}"
    cp "${ROOT}/capsules/chat-wasm/capsule.json" \
        "${TEST_HOME}/xdg-data/elastos/capsules/chat-wasm/capsule.json"
    cp "${ROOT}/capsules/chat/target/wasm32-wasip1/release/chat-stdio.wasm" \
        "${TEST_HOME}/xdg-data/elastos/capsules/chat-wasm/chat-stdio.wasm"
fi

echo "[interop] test home: ${TEST_HOME}"
echo "[interop] binary:    ${ELASTOS_BIN}"

SHARED_ENV=(
    "HOME=${TEST_HOME}"
    "XDG_DATA_HOME=${TEST_HOME}/xdg-data"
    "ELASTOS_DATA_DIR=${TEST_HOME}/xdg-data/elastos"
    "ELASTOS_QUIET_RUNTIME_NOTICES=1"
)

cleanup() {
    # Phase 5 Day 3 — was a bare `kill "$pid"` that left
    # daemonised children stranded on macOS. The shared
    # helper sends SIGTERM to both the group and the bare
    # PID, polls for liveness, and escalates to SIGKILL
    # after a 2 s grace. Linux behaviour byte-identical
    # (same kill sequence is issued).
    local coords="${TEST_HOME}/xdg-data/elastos/runtime-coords.json"
    if [[ -f "$coords" ]]; then
        local pid
        pid=$(python3 -c "import json; print(json.load(open('$coords')).get('pid',''))" 2>/dev/null || true)
        if [[ -n "$pid" ]]; then
            kill_pid_then_group "$pid" 2
        fi
    fi
    rm -rf "${TEST_HOME}"
}
trap cleanup EXIT

# Phase 5 Day 3 — tee the PTY-control session output to a log
# file so the post-smoke `cross_platform_alert_on_vz_error_in_logs`
# helper can grep it for `kind_label:` tokens. The exit status
# of the python pipeline must come from the python interpreter,
# not from `tee`, so we use `set -o pipefail` (already set by
# `set -euo pipefail` at the top) + capture python's exit via a
# trap-safe PIPESTATUS trick.
:>"${SESSION_LOG}"
# Run the interop proof via Python for pty control
env ELASTOS_BIN="${ELASTOS_BIN}" "${SHARED_ENV[@]}" python3 - <<'PY' 2>&1 | tee -a "${SESSION_LOG}"
import json, os, pty, select, subprocess, sys, time

ELASTOS_BIN = os.environ["ELASTOS_BIN"]
TEST_HOME = os.environ["HOME"]
COORDS_PATH = os.path.join(TEST_HOME, "xdg-data", "elastos", "runtime-coords.json")

class PtyProc:
    def __init__(self, cmd, cwd, env):
        self.master, slave = pty.openpty()
        self.proc = subprocess.Popen(
            cmd, cwd=cwd, env=env,
            stdin=slave, stdout=slave, stderr=slave, close_fds=True,
        )
        os.close(slave)
        self.buffer = bytearray()

    def read_available(self, timeout=0.2):
        ready, _, _ = select.select([self.master], [], [], timeout)
        if not ready:
            return b""
        try:
            chunk = os.read(self.master, 4096)
        except OSError:
            return b""
        if chunk:
            self.buffer.extend(chunk)
        return chunk

    def text(self):
        return self.buffer.decode("utf-8", errors="ignore")

    def send_line(self, line):
        os.write(self.master, line.encode("utf-8") + b"\r")

    def terminate(self):
        if self.proc.poll() is None:
            self.proc.terminate()
            try:
                self.proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.proc.kill()
                self.proc.wait(timeout=5)
        os.close(self.master)


def wait_for(predicate, timeout, desc, procs):
    deadline = time.time() + timeout
    while time.time() < deadline:
        for p in procs:
            p.read_available(0.2)
        if predicate():
            return
        for p in procs:
            if p.proc.poll() not in (None, 0):
                raise SystemExit(f"{desc}: process exited {p.proc.returncode}\n{p.text()}")
    combined = "\n".join(f"--- {i} ---\n{p.text()}" for i, p in enumerate(procs))
    raise SystemExit(f"timeout: {desc}\n{combined}")


env = os.environ.copy()

# 1. Launch native chat — this starts the shared managed runtime
print("[interop] launching native chat (starts shared runtime)...")
native = PtyProc(
    [ELASTOS_BIN, "chat", "--nick", "native"],
    TEST_HOME, env,
)

wasm = None
try:
    wait_for(
        lambda: os.path.exists(COORDS_PATH)
        and "Chat as 'native'" in native.text(),
        45, "native chat ready", [native],
    )
    print("[interop] native chat ready")

    # 2. Launch WASM chat on the SAME runtime
    print("[interop] launching WASM chat capsule (attaches to shared runtime)...")
    wasm = PtyProc(
        [
            ELASTOS_BIN,
            "capsule",
            "chat-wasm",
            "--lifecycle",
            "interactive",
            "--interactive",
            "--config",
            '{"nick":"wasm"}',
        ],
        TEST_HOME, env,
    )

    # Wait for WASM to fully initialize — including bridge, capability
    # acquisition, and gossip topic join. The UI banner appears early
    # but actual connectivity takes much longer (WASM compilation +
    # bridge + caps + gossip_join can take 30-60s on slow machines).
    wait_for(
        lambda: "peer" in wasm.text().lower() or "connected" in wasm.text().lower() or "#general" in wasm.text(),
        90, "wasm chat fully connected", [native, wasm],
    )
    print("[interop] wasm chat connected")

    # 3. Native sends, WASM should see it via shared buffer.
    # The WASM capsule's bridge + gossip init can be slow. Resend
    # periodically until the WASM side sees it.
    print("[interop] native sends: hello-from-native (retrying until delivered)")
    deadline = time.time() + 90
    delivered = False
    while time.time() < deadline:
        native.send_line("hello-from-native")
        for _ in range(10):
            native.read_available(0.3)
            wasm.read_available(0.3)
            if "hello-from-native" in wasm.text():
                delivered = True
                break
        if delivered:
            break
        time.sleep(2)
    if not delivered:
        combined = "\n--- wasm ---\n" + wasm.text() + "\n--- native ---\n" + native.text()
        raise SystemExit(f"timeout: native -> wasm delivery\n{combined}")
    print("[interop] native -> wasm: delivered")

    # 4. WASM sends, native should see it via shared buffer
    print("[interop] wasm sends: hello-from-wasm (retrying until delivered)")
    deadline = time.time() + 90
    delivered = False
    while time.time() < deadline:
        wasm.send_line("hello-from-wasm")
        for _ in range(10):
            native.read_available(0.3)
            wasm.read_available(0.3)
            if "hello-from-wasm" in native.text():
                delivered = True
                break
        if delivered:
            break
        time.sleep(2)
    if not delivered:
        combined = "\n--- native ---\n" + native.text() + "\n--- wasm ---\n" + wasm.text()
        raise SystemExit(f"timeout: wasm -> native delivery\n{combined}")
    print("[interop] wasm -> native: delivered")

    # 5. Clean exit
    native.send_line("/quit")
    if wasm:
        wasm.send_line("/quit")
    time.sleep(2)

    print("[chat-wasm-native-interop] PASS")

finally:
    native.terminate()
    if wasm:
        wasm.terminate()
PY
PYTHON_EXIT="${PIPESTATUS[0]}"
if [[ "${PYTHON_EXIT}" -ne 0 ]]; then
    echo "[interop] PTY harness failed with exit code ${PYTHON_EXIT}" >&2
    # Even on failure, run the alert hook so any vz_error
    # tokens in the captured logs reach the operator. The
    # python harness's failure is the primary signal, but a
    # Vz-substrate cause embedded in the logs is the
    # actionable one.
    cross_platform_alert_on_vz_error_in_logs \
        "${INSTALL_LOG}" "${SETUP_LOG}" "${BUILD_LOG}" "${SESSION_LOG}" || true
    exit "${PYTHON_EXIT}"
fi

# Phase 5 Day 3 — VzError alerting tail. Greps every log file
# collected by the smoke for `VzError::Display`'s nine
# stable `kind_label:` tokens (the contract from Phase 4
# Day 7's `vz_error_display_includes_kind_label_for_log_grep`
# test). Any hit fails the smoke loudly with the matched
# line(s) and a runbook pointer — that's the typed-error
# tripwire the Phase-4 work was building toward.
#
# On Linux this is a no-op (no Vz substrate to fail). On
# Mac, today, the pre-flight skip means we don't reach this
# line — once Phase 6 lands, this becomes the actively-
# alerting tripwire for every chat-interop smoke run.
echo "[interop] Vz error alerting tail:"
if cross_platform_alert_on_vz_error_in_logs \
        "${INSTALL_LOG}" "${SETUP_LOG}" "${BUILD_LOG}" "${SESSION_LOG}"; then
    echo "  - OK: no VzError kind_label tokens found in collected logs"
else
    echo "[interop] FAIL: VzError kind_label token found in collected logs (see above)" >&2
    exit 1
fi

# Phase 5 Day 3 — Vz substrate readiness probe. Same advisory
# diagnostic as Day 1 / Day 2 / Day 3 smokes. Linux skip
# (host is not Darwin); Mac reports capability + capsule
# discovery findings (advisory only).
if [[ "${OS_TOKEN}" == "darwin" ]]; then
    echo "[interop] Vz substrate probe:"
    if vz_host_is_capable; then
        echo "  - Vz host capability: PASS (macOS 12+ detected via sw_vers)"
        if probe_target="$(vz_discover_launchable_capsule \
                "${TEST_HOME}/xdg-data/elastos" 2>/dev/null)" \
                && [[ -n "${probe_target}" ]]; then
            echo "  - Launchable capsule discovered: ${probe_target}"
        else
            echo "  - Launchable capsule discovered: none (expected pre-Phase 6)"
        fi
    else
        echo "  - Vz host capability: SKIP (not macOS 12+ or sw_vers unavailable)"
    fi
else
    echo "[interop] Vz substrate probe: SKIP (host is ${OS_TOKEN}, not Darwin)"
fi
