#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TEST_HOME="$(mktemp -d /tmp/elastos-interop-XXXXXX)"
PUBLISHER_GATEWAY="${ELASTOS_PUBLISHER_GATEWAY:-https://elastos.elacitylabs.com}"

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

echo "[interop] install published runtime"
HOME="${TEST_HOME}" \
XDG_DATA_HOME="${TEST_HOME}/xdg-data" \
ELASTOS_PUBLISHER_GATEWAY="${PUBLISHER_GATEWAY}" \
bash -lc 'mkdir -p "$HOME" "$XDG_DATA_HOME" && curl -fsSL "${ELASTOS_PUBLISHER_GATEWAY%/}/install.sh" | bash' \
    >/tmp/elastos-chat-wasm-interop-install.log

INSTALLED_BIN="${TEST_HOME}/.local/bin/elastos"
[[ -x "${INSTALLED_BIN}" ]] || {
    echo "[interop] installed binary missing: ${INSTALLED_BIN}" >&2
    exit 1
}
ELASTOS_BIN="${ELASTOS_BIN_OVERRIDE:-${INSTALLED_BIN}}"
[[ -x "${ELASTOS_BIN}" ]] || {
    echo "[interop] override binary missing: ${ELASTOS_BIN}" >&2
    exit 1
}
COMPONENTS_MANIFEST="${TEST_HOME}/xdg-data/elastos/components.json"
[[ -f "${COMPONENTS_MANIFEST}" ]] || {
    echo "[interop] installed components manifest missing: ${COMPONENTS_MANIFEST}" >&2
    exit 1
}

echo "[interop] setup required chat + chat-wasm components"
HOME="${TEST_HOME}" \
XDG_DATA_HOME="${TEST_HOME}/xdg-data" \
"${ELASTOS_BIN}" setup \
    --with shell \
    --with localhost-provider \
    --with did-provider \
    --with chat-wasm >/tmp/elastos-chat-wasm-interop-setup.log

if [[ -n "${ELASTOS_BIN_OVERRIDE:-}" ]]; then
    echo "[interop] overlay local source chat-wasm artifact"
    cargo build \
        --manifest-path "${ROOT}/capsules/chat/Cargo.toml" \
        --bin chat-stdio \
        --target wasm32-wasip1 \
        --no-default-features \
        --release >/tmp/elastos-chat-wasm-interop-build.log
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
    # Kill any managed runtime we started
    local coords="${TEST_HOME}/xdg-data/elastos/runtime-coords.json"
    if [[ -f "$coords" ]]; then
        local pid
        pid=$(python3 -c "import json; print(json.load(open('$coords')).get('pid',''))" 2>/dev/null || true)
        [[ -n "$pid" ]] && kill "$pid" 2>/dev/null || true
    fi
    rm -rf "${TEST_HOME}"
}
trap cleanup EXIT

# Run the interop proof via Python for pty control
env ELASTOS_BIN="${ELASTOS_BIN}" "${SHARED_ENV[@]}" python3 - <<'PY'
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
