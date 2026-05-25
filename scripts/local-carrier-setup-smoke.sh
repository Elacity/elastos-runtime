#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ELASTOS_ROOT="${REPO_ROOT}/elastos"
ELASTOS_BIN="${ELASTOS_ROOT}/target/debug/elastos"

# Phase 5 Day 1 — cross-platform helpers (bash 3.2 clean,
# Linux + macOS BSD-util compatible). See
# `scripts/lib/cross-platform.sh` for the full rationale.
# shellcheck source=lib/cross-platform.sh
. "${REPO_ROOT}/scripts/lib/cross-platform.sh"

case "$(uname -s)" in
    Linux)  OS_TOKEN="linux"  ;;
    Darwin) OS_TOKEN="darwin" ;;
    *)
        echo "Unsupported OS for local-carrier-setup-smoke: $(uname -s)" >&2
        exit 1
        ;;
esac

case "$(uname -m)" in
    x86_64)         ARCH_TOKEN="amd64" ;;
    aarch64|arm64)  ARCH_TOKEN="arm64" ;;
    *)
        echo "Unsupported machine architecture: $(uname -m)" >&2
        exit 1
        ;;
esac

SETUP_PLATFORM="${OS_TOKEN}-${ARCH_TOKEN}"

TEST_ROOT="${ELASTOS_LOCAL_TEST_ROOT:-$(mktemp -d "${TMPDIR:-/tmp}/elastos-local-carrier-setup.XXXXXX")}"
XDG_DATA_HOME="${TEST_ROOT}/xdg-data"
DATA_DIR="${XDG_DATA_HOME}/elastos"
# `dirs::data_dir()` is platform-specific (Library/Application Support on
# macOS, $XDG_DATA_HOME on Linux). Use ELASTOS_DATA_DIR for cross-platform
# isolation so the smoke test never leaks into the user's real runtime.
export ELASTOS_DATA_DIR="${DATA_DIR}"
PUBLISHER_ROOT="${DATA_DIR}/ElastOS/SystemServices/Publisher"
ARTIFACTS_DIR="${PUBLISHER_ROOT}/artifacts"
LOG_PATH="${TEST_ROOT}/serve.log"
API_PORT="$(python3 - <<'PY'
import socket
s = socket.socket()
s.bind(("127.0.0.1", 0))
print(s.getsockname()[1])
s.close()
PY
)"

stop_source_runtime() {
    if [[ -z "${SERVE_PID:-}" ]]; then
        rm -f "${RUNTIME_COORDS:-}"
        return 0
    fi

    if ! kill -0 "${SERVE_PID}" 2>/dev/null; then
        wait "${SERVE_PID}" 2>/dev/null || true
        rm -f "${RUNTIME_COORDS:-}"
        unset SERVE_PID
        return 0
    fi

    kill -- "-${SERVE_PID}" 2>/dev/null || kill "${SERVE_PID}" 2>/dev/null || true
    for _ in $(seq 1 20); do
        if ! kill -0 "${SERVE_PID}" 2>/dev/null; then
            break
        fi
        sleep 0.1
    done
    if kill -0 "${SERVE_PID}" 2>/dev/null; then
        kill -KILL -- "-${SERVE_PID}" 2>/dev/null || kill -KILL "${SERVE_PID}" 2>/dev/null || true
    fi
    rm -f "${RUNTIME_COORDS:-}"
    unset SERVE_PID
}

kill_temp_processes() {
    local root="$1"
    local skip_pid="${2:-}"
    # bash 3.2 (macOS default) has no mapfile/readarray. Read line-by-line and
    # guard against empty arrays under `set -u` with the `${arr[@]+...}` form.
    local pids=()
    while IFS= read -r _pid; do
        pids+=("$_pid")
    done < <(pgrep -f "$root" || true)
    if [[ ${#pids[@]} -eq 0 ]]; then
        return 0
    fi
    for pid in "${pids[@]}"; do
        [[ "$pid" == "$$" ]] && continue
        [[ -n "${skip_pid}" && "$pid" == "${skip_pid}" ]] && continue
        kill "$pid" 2>/dev/null || true
    done
    sleep 0.2
    for pid in "${pids[@]}"; do
        [[ "$pid" == "$$" ]] && continue
        [[ -n "${skip_pid}" && "$pid" == "${skip_pid}" ]] && continue
        if kill -0 "$pid" 2>/dev/null; then
            kill -9 "$pid" 2>/dev/null || true
        fi
    done
    return 0
}

cleanup() {
    stop_source_runtime
    kill_temp_processes "${TEST_ROOT}"
    return 0
}
trap cleanup EXIT

echo "[local-carrier-setup] test root: ${TEST_ROOT}"

# Phase 5 Day 6 — FORCE_FULL override (highest precedence).
# Used by the self-hosted Mac runner spec (see
# docs/vz-backend/SELF_HOSTED_RUNNER_SPEC.md) to opt back into
# the full smoke lane after Day-5's CI auto-detect would have
# downgraded the run to dry-run. Layered precedence (top wins):
#   1. ELASTOS_VZ_SMOKE_FORCE_FULL=1   → full run, even in CI.
#   2. ELASTOS_VZ_SMOKE_DRY_RUN=0/1   → explicit operator setting.
#   3. CI auto-detect + DRY_RUN unset → DRY_RUN=1 (Day 5).
#   4. Default                        → full run.
if [[ "${ELASTOS_VZ_SMOKE_FORCE_FULL:-0}" == "1" ]]; then
    echo "[local-carrier-setup] FORCE_FULL=1 — forcing full smoke run (overrides CI auto-detect)"
    export ELASTOS_VZ_SMOKE_DRY_RUN=0
fi

# Phase 5 Day 5 — auto-dry-run in CI. GitHub Actions runners
# don't have a provisioned `~/.local/share/elastos`, so the
# full smoke would visible-skip on every PR. Explicit operator
# override via `ELASTOS_VZ_SMOKE_DRY_RUN=0` keeps the full
# semantics available for self-hosted CI runners that DO have a
# data dir provisioned (Phase 5 Day 6+ deliverable). The
# explicit `=1` setting always wins below; this branch only
# fires when the env is otherwise silent.
if [[ -z "${ELASTOS_VZ_SMOKE_DRY_RUN:-}" ]] && cross_platform_in_ci; then
    echo "[local-carrier-setup] CI detected (GITHUB_ACTIONS or CI env set); auto-enabling ELASTOS_VZ_SMOKE_DRY_RUN=1"
    export ELASTOS_VZ_SMOKE_DRY_RUN=1
fi

# Phase 5 Day 1 — dry-run mode for CI. Exits successfully after
# the bash-portability checks + helper sourcing, BEFORE paying
# the cargo-build cost. Lets a Mac CI runner prove the script
# at least parses + sources its helpers without committing to a
# full ~10-minute end-to-end run on every push.
if [[ "${ELASTOS_VZ_SMOKE_DRY_RUN:-0}" == "1" ]]; then
    echo "[local-carrier-setup] dry-run mode: parse OK, helper sourced OK; exiting before cargo build"
    if [[ "${OS_TOKEN}" == "darwin" ]]; then
        if vz_host_is_capable; then
            echo "[local-carrier-setup] dry-run: Vz host capability check passed (macOS 12+)"
        else
            echo "[local-carrier-setup] dry-run: Vz host capability check skipped (not macOS 12+ or sw_vers unavailable)"
        fi
    fi
    exit 0
fi

# Phase 5 Day 1 — Mac pre-flight: detect whether components.json
# has the darwin-arm64 release metadata required for the
# Carrier install half of this smoke.
#
# Phase 5 Day 2 — the inline Python check was hoisted into
# `cross_platform_assert_native_binary_release_metadata`
# (`scripts/lib/cross-platform.sh`) so Day 1 / Day 2 / Day 3
# / future Phase-6 install smokes share one source of truth
# for the platform-key + names check.
#
# Pre-Work removed the dishonest darwin entries; Phase 6
# restores truthful ones (per `docs/vz-backend/PLAN.md` L321).
# Between those, on Mac, this smoke cannot exercise the
# native-binary install path end-to-end. We exit 0 with a
# clear operator-facing message + skip telemetry. The
# substrate probe at the tail is unaffected — it visibly-skips
# for the same reason.
#
# We run this BEFORE the "building current binary…" echo so the
# operator-facing skip output isn't preceded by a misleading
# "we're building things" header.
if [[ "${OS_TOKEN}" == "darwin" ]] \
        && [[ "${ELASTOS_VZ_SMOKE_FORCE_PROCEED:-0}" != "1" ]]; then
    if ! cross_platform_assert_native_binary_release_metadata \
            "${REPO_ROOT}/components.json" \
            shell localhost-provider did-provider webspace-provider 2>/dev/null
    then
        cross_platform_print_phase6_skip_message
        echo "[local-carrier-setup] Mac pre-flight: SKIP (Phase 6 prerequisite not met)"
        # Exit 0 — this is a clean skip, not a smoke failure. CI dashboards
        # alert on the skip telemetry separately.
        exit 0
    fi
fi

echo "[local-carrier-setup] building current binary and first-party Home core assets"

(cd "${ELASTOS_ROOT}" && cargo build -p elastos-server)
(cd "${REPO_ROOT}/elastos/capsules/shell" && cargo build --release)
(cd "${REPO_ROOT}/elastos/capsules/localhost-provider" && cargo build --release)
(cd "${REPO_ROOT}/capsules/did-provider" && cargo build --release)
(cd "${REPO_ROOT}/capsules/webspace-provider" && cargo build --release)
(cd "${REPO_ROOT}/capsules/home-cli" && cargo build --target wasm32-wasip1 --release)
(cd "${REPO_ROOT}/capsules/home" && cargo build --target wasm32-wasip1 --release)
(cd "${REPO_ROOT}/capsules/system" && cargo build --target wasm32-wasip1 --release)
(cd "${REPO_ROOT}/capsules/chat-room" && cargo build --target wasm32-wasip1 --release)

mkdir -p "${ARTIFACTS_DIR}"
mkdir -p "${DATA_DIR}/bin"

# `elastos serve` now fails closed unless localhost-provider is already
# installed. Seed the one required host provider before starting the local
# source runtime; the rest of the setup still proves Carrier-backed install.
install -m 755 \
    "${REPO_ROOT}/elastos/target/release/localhost-provider" \
    "${DATA_DIR}/bin/localhost-provider"

COMPONENTS_SRC="${REPO_ROOT}/components.json" \
COMPONENTS_DEST="${DATA_DIR}/components.json" \
DATA_DIR="${DATA_DIR}" \
PUBLISHER_ROOT="${PUBLISHER_ROOT}" \
SETUP_PLATFORM="${SETUP_PLATFORM}" \
SHELL_BIN="${REPO_ROOT}/elastos/target/release/shell" \
LOCALHOST_PROVIDER_BIN="${REPO_ROOT}/elastos/target/release/localhost-provider" \
DID_PROVIDER_BIN="${REPO_ROOT}/capsules/did-provider/target/release/did-provider" \
WEBSPACE_PROVIDER_BIN="${REPO_ROOT}/capsules/webspace-provider/target/release/webspace-provider" \
HOME_CLI_DIR="${REPO_ROOT}/capsules/home-cli" \
HOME_CAPSULE_DIR="${REPO_ROOT}/capsules/home" \
SYSTEM_CAPSULE_DIR="${REPO_ROOT}/capsules/system" \
CHAT_ROOM_CAPSULE_DIR="${REPO_ROOT}/capsules/chat-room" \
DOCUMENTS_CAPSULE_DIR="${REPO_ROOT}/capsules/documents" \
LIBRARY_CAPSULE_DIR="${REPO_ROOT}/capsules/library" \
INBOX_CAPSULE_DIR="${REPO_ROOT}/capsules/inbox" \
GBA_EMULATOR_CAPSULE_DIR="${REPO_ROOT}/capsules/gba-emulator" \
GBA_UCITY_CAPSULE_DIR="${REPO_ROOT}/capsules/gba-ucity" \
python3 - <<'PY'
import hashlib
import json
import os
import pathlib
import shutil
import tarfile

components_src = pathlib.Path(os.environ["COMPONENTS_SRC"])
components_dest = pathlib.Path(os.environ["COMPONENTS_DEST"])
data_dir = pathlib.Path(os.environ["DATA_DIR"])
publisher_root = pathlib.Path(os.environ["PUBLISHER_ROOT"])
artifacts_dir = publisher_root / "artifacts"
artifacts_dir.mkdir(parents=True, exist_ok=True)
platform = os.environ["SETUP_PLATFORM"]

manifest = json.loads(components_src.read_text())

def platform_info(name):
    platforms = manifest["external"][name].get("platforms") or {}
    info = platforms.get(platform) or platforms.get("*")
    if not info:
        raise SystemExit(f"{name} missing release metadata for {platform}")
    return info

mapping = {
    "shell": pathlib.Path(os.environ["SHELL_BIN"]),
    "localhost-provider": pathlib.Path(os.environ["LOCALHOST_PROVIDER_BIN"]),
    "did-provider": pathlib.Path(os.environ["DID_PROVIDER_BIN"]),
    "webspace-provider": pathlib.Path(os.environ["WEBSPACE_PROVIDER_BIN"]),
}

for name, src in mapping.items():
    if not src.is_file():
        raise SystemExit(f"missing built artifact for {name}: {src}")
    info = platform_info(name)
    release_path = info.get("release_path")
    if not release_path:
        raise SystemExit(f"{name} missing release_path for {platform}")
    dest = artifacts_dir / release_path
    shutil.copy2(src, dest)
    data = dest.read_bytes()
    info["checksum"] = "sha256:" + hashlib.sha256(data).hexdigest()
    info["size"] = len(data)

home_cli_dir = pathlib.Path(os.environ["HOME_CLI_DIR"])
home_cli_manifest = platform_info("home-cli")
home_cli_release_path = home_cli_manifest.get("release_path")
if not home_cli_release_path:
    raise SystemExit(f"home-cli missing release_path for {platform}")
home_cli_archive = artifacts_dir / home_cli_release_path
with tarfile.open(home_cli_archive, "w:gz") as tar:
    tar.add(home_cli_dir / "capsule.json", arcname="home-cli/capsule.json")
    tar.add(
        home_cli_dir / "target/wasm32-wasip1/release/home-cli.wasm",
        arcname="home-cli/home-cli.wasm",
    )
home_cli_data = home_cli_archive.read_bytes()
home_cli_manifest["checksum"] = "sha256:" + hashlib.sha256(home_cli_data).hexdigest()
home_cli_manifest["size"] = len(home_cli_data)

# Browser WASM capsules: capsule.json + {name}.wasm + browser/ assets.
# chat-room follows the same shape (Cargo crate that builds chat-room.wasm
# under target/wasm32-wasip1/release and a sibling browser/ dir).
browser_capsules = {
    "home": pathlib.Path(os.environ["HOME_CAPSULE_DIR"]),
    "system": pathlib.Path(os.environ["SYSTEM_CAPSULE_DIR"]),
    "chat-room": pathlib.Path(os.environ["CHAT_ROOM_CAPSULE_DIR"]),
}
for name, capsule_dir in browser_capsules.items():
    info = platform_info(name)
    release_path = info.get("release_path")
    if not release_path:
        raise SystemExit(f"{name} missing release_path for {platform}")
    archive = artifacts_dir / release_path
    with tarfile.open(archive, "w:gz") as tar:
        tar.add(capsule_dir / "capsule.json", arcname=f"{name}/capsule.json")
        tar.add(
            capsule_dir / "target/wasm32-wasip1/release" / f"{name}.wasm",
            arcname=f"{name}/{name}.wasm",
        )
        tar.add(capsule_dir / "browser", arcname=f"{name}/browser")
    data = archive.read_bytes()
    info["checksum"] = "sha256:" + hashlib.sha256(data).hexdigest()
    info["size"] = len(data)

# Data capsules: tar every source-side file so multi-file viewers (e.g.
# gba-emulator with its mgba.wasm + emulator.js + style.css) and content
# capsules with non-`index.html` entrypoints (e.g. gba-ucity → ucity.gba)
# both install correctly. Build artifacts and VCS metadata are excluded.
_DATA_EXCLUDE_NAMES = {"target", "Cargo.lock", ".git", "node_modules", ".DS_Store"}
data_capsules = {
    "documents": pathlib.Path(os.environ["DOCUMENTS_CAPSULE_DIR"]),
    "library": pathlib.Path(os.environ["LIBRARY_CAPSULE_DIR"]),
    "inbox": pathlib.Path(os.environ["INBOX_CAPSULE_DIR"]),
    "gba-emulator": pathlib.Path(os.environ["GBA_EMULATOR_CAPSULE_DIR"]),
    "gba-ucity": pathlib.Path(os.environ["GBA_UCITY_CAPSULE_DIR"]),
}
for name, capsule_dir in data_capsules.items():
    info = platform_info(name)
    release_path = info.get("release_path")
    if not release_path:
        raise SystemExit(f"{name} missing release_path for {platform}")
    archive = artifacts_dir / release_path
    with tarfile.open(archive, "w:gz") as tar:
        for child in sorted(capsule_dir.iterdir()):
            if child.name in _DATA_EXCLUDE_NAMES:
                continue
            tar.add(child, arcname=f"{name}/{child.name}")
    data = archive.read_bytes()
    info["checksum"] = "sha256:" + hashlib.sha256(data).hexdigest()
    info["size"] = len(data)

components_dest.parent.mkdir(parents=True, exist_ok=True)
components_dest.write_text(json.dumps(manifest, indent=2) + "\n")
PY

echo "[local-carrier-setup] staged local artifacts into ${ARTIFACTS_DIR}"

mkdir -p "${DATA_DIR}"
SERVE_PID="$(
    ELASTOS_ROOT="${ELASTOS_ROOT}" \
    ELASTOS_BIN="${ELASTOS_BIN}" \
    XDG_DATA_HOME="${XDG_DATA_HOME}" \
    API_PORT="${API_PORT}" \
    LOG_PATH="${LOG_PATH}" \
    python3 - <<'PY'
import os
import subprocess

env = os.environ.copy()
with open(os.environ["LOG_PATH"], "ab") as log:
    proc = subprocess.Popen(
        [
            os.environ["ELASTOS_BIN"],
            "serve",
            "--addr",
            f'127.0.0.1:{os.environ["API_PORT"]}',
        ],
        cwd=os.environ["ELASTOS_ROOT"],
        env=env,
        stdin=subprocess.DEVNULL,
        stdout=log,
        stderr=log,
        start_new_session=True,
        close_fds=True,
    )
print(proc.pid)
PY
)"

RUNTIME_COORDS="${DATA_DIR}/runtime-coords.json"
for _ in $(seq 1 60); do
    if [[ -f "${RUNTIME_COORDS}" ]]; then
        break
    fi
    sleep 0.5
done

if [[ ! -f "${RUNTIME_COORDS}" ]]; then
    echo "runtime-coords.json was not created. See ${LOG_PATH}" >&2
    exit 1
fi

SOURCE_BOOTSTRAP_FILE="${TEST_ROOT}/source-bootstrap.txt"
for _ in $(seq 1 60); do
    if RUNTIME_COORDS="${RUNTIME_COORDS}" SOURCE_BOOTSTRAP_FILE="${SOURCE_BOOTSTRAP_FILE}" python3 - <<'PY'
import json
import os
import urllib.request
import urllib.error

coords = json.loads(open(os.environ["RUNTIME_COORDS"]).read())
api_url = coords["api_url"]
secret = coords["attach_secret"]

try:
    with urllib.request.urlopen(api_url + "/api/health", timeout=2) as resp:
        if resp.status != 200:
            raise RuntimeError("runtime not healthy yet")

    attach_req = urllib.request.Request(
        api_url + "/api/auth/attach",
        data=json.dumps({"secret": secret, "scope": "shell"}).encode("utf-8"),
        headers={"Content-Type": "application/json"},
    )
    with urllib.request.urlopen(attach_req, timeout=5) as resp:
        token = json.loads(resp.read().decode("utf-8"))["token"]

    ticket_req = urllib.request.Request(
        api_url + "/api/provider/peer/get_ticket",
        data=b"{}",
        headers={
            "Content-Type": "application/json",
            "Authorization": f"Bearer {token}",
        },
    )
    with urllib.request.urlopen(ticket_req, timeout=5) as resp:
        body = json.loads(resp.read().decode("utf-8"))

    with open(os.environ["SOURCE_BOOTSTRAP_FILE"], "w", encoding="utf-8") as f:
        f.write(body["data"]["ticket"] + "\n")
        f.write(body["data"]["node_id"] + "\n")
except Exception:
    raise SystemExit(1)
PY
    then
        break
    fi
    sleep 0.5
done

if [[ ! -f "${SOURCE_BOOTSTRAP_FILE}" ]]; then
    echo "failed to obtain local Carrier bootstrap details. See ${LOG_PATH}" >&2
    exit 1
fi

# bash 3.2 (macOS default) has no readarray; read line-by-line.
SOURCE_BOOTSTRAP=()
while IFS= read -r _line; do
    SOURCE_BOOTSTRAP+=("$_line")
done < "${SOURCE_BOOTSTRAP_FILE}"
CONNECT_TICKET="${SOURCE_BOOTSTRAP[0]:-}"
NODE_ID="${SOURCE_BOOTSTRAP[1]:-}"

if [[ -z "${CONNECT_TICKET}" || -z "${NODE_ID}" ]]; then
    echo "failed to obtain local Carrier bootstrap details. See ${LOG_PATH}" >&2
    exit 1
fi

SOURCES_PATH="${DATA_DIR}/sources.json"
CONNECT_TICKET="${CONNECT_TICKET}" \
NODE_ID="${NODE_ID}" \
ELASTOS_BIN_PATH="${ELASTOS_BIN}" \
SOURCES_PATH="${SOURCES_PATH}" \
python3 - <<'PY'
import json
import os

sources = {
    "schema": "elastos.trusted-sources/v1",
    "default_source": "default",
    "sources": [
        {
            "name": "default",
            "publisher_dids": ["did:key:local-carrier-test"],
            "channel": "stable",
            "discovery_uri": "elastos://source/stable/local-carrier-test",
            "connect_ticket": os.environ["CONNECT_TICKET"],
            "publisher_node_id": os.environ["NODE_ID"],
            "ipns_name": "",
            "gateways": [],
            "install_path": os.environ["ELASTOS_BIN_PATH"],
            "installed_version": "",
            "head_cid": "",
        }
    ],
}

with open(os.environ["SOURCES_PATH"], "w", encoding="utf-8") as f:
    json.dump(sources, f, indent=2)
    f.write("\n")
PY

echo "[local-carrier-setup] running Carrier-only setup smoke"
(
    cd "${ELASTOS_ROOT}"
    XDG_DATA_HOME="${XDG_DATA_HOME}" \
    "${ELASTOS_BIN}" setup
)

# Layer the universal-platform browser/data capsules on top of the home profile
# install. These are platform-agnostic (`platforms: ["*"]`) and run anywhere
# the gateway runs, so they're the first natural extension beyond the home
# profile on macOS.
echo "[local-carrier-setup] installing universal demo capsules (chat-room, gba-emulator, gba-ucity)"
(
    cd "${ELASTOS_ROOT}"
    XDG_DATA_HOME="${XDG_DATA_HOME}" \
    "${ELASTOS_BIN}" setup \
        --with chat-room \
        --with gba-emulator \
        --with gba-ucity
)

stop_source_runtime

for installed in \
    "${DATA_DIR}/bin/shell" \
    "${DATA_DIR}/bin/localhost-provider" \
    "${DATA_DIR}/bin/did-provider" \
    "${DATA_DIR}/bin/webspace-provider" \
    "${DATA_DIR}/capsules/home-cli/home-cli.wasm" \
    "${DATA_DIR}/capsules/home-cli/capsule.json" \
    "${DATA_DIR}/capsules/home/home.wasm" \
    "${DATA_DIR}/capsules/home/browser/index.html" \
    "${DATA_DIR}/capsules/system/system.wasm" \
    "${DATA_DIR}/capsules/system/browser/index.html" \
    "${DATA_DIR}/capsules/documents/index.html" \
    "${DATA_DIR}/capsules/library/index.html" \
    "${DATA_DIR}/capsules/inbox/index.html" \
    "${DATA_DIR}/capsules/chat-room/chat-room.wasm" \
    "${DATA_DIR}/capsules/chat-room/capsule.json" \
    "${DATA_DIR}/capsules/gba-emulator/index.html" \
    "${DATA_DIR}/capsules/gba-emulator/mgba.wasm" \
    "${DATA_DIR}/capsules/gba-ucity/ucity.gba" \
    "${DATA_DIR}/capsules/gba-ucity/capsule.json"
do
    if [[ ! -f "${installed}" ]]; then
        echo "expected installed file missing: ${installed}" >&2
        exit 1
    fi
done

STATUS_OUT="${TEST_ROOT}/home-status.txt"
(
    cd "${ELASTOS_ROOT}"
    XDG_DATA_HOME="${XDG_DATA_HOME}" \
    "${ELASTOS_BIN}" home --status >"${STATUS_OUT}"
)
grep -q "ElastOS Home" "${STATUS_OUT}" || {
    echo "expected home status output missing from ${STATUS_OUT}" >&2
    exit 1
}

HOME_OUT="${TEST_ROOT}/home.txt"
(
    cd "${ELASTOS_ROOT}"
    printf 'q\n' | XDG_DATA_HOME="${XDG_DATA_HOME}" \
    "${ELASTOS_BIN}" >"${HOME_OUT}"
)
grep -q "ElastOS Home" "${HOME_OUT}" || {
    echo "expected home output missing from ${HOME_OUT}" >&2
    exit 1
}

echo "[local-carrier-setup] OK"

# Phase 5 Day 1 — Vz-substrate readiness probe. The smoke
# itself validates the Carrier install pipeline; this tail step
# additionally documents whether the host can launch a real
# microVM via Vz (Mac) or crosvm (Linux) end-to-end.
#
# The probe deliberately does NOT fail the smoke. The smoke's
# contract is "install pipeline works"; substrate readiness is
# diagnostic. Operators / CI dashboards alert on the "skip"
# signal separately.
#
# The probe visibly-skips if no installed MicroVM-typed
# capsule with a rootfs.ext4 is present. The Day-1 smoke does
# NOT install a rootfs (none of the published microVM-typed
# capsules ship one in the trusted-source pipeline yet — that
# lands as part of Phase 6's `components.json` restoration),
# so the skip is the expected outcome on a fresh host. The
# probe still validates the helper logic.
if [[ "${ELASTOS_VZ_SMOKE_SKIP_PROBE:-0}" != "1" ]]; then
    if [[ "${OS_TOKEN}" == "darwin" ]]; then
        if vz_host_is_capable; then
            echo "[local-carrier-setup] Vz substrate probe: host capable (macOS 12+, sw_vers reachable)"
        else
            echo "[local-carrier-setup] Vz substrate probe: host NOT capable (sw_vers unavailable or pre-macOS-12)" >&2
        fi
    fi
    if probe_capsule="$(vz_discover_launchable_capsule "${DATA_DIR}" 2>/dev/null)"; then
        echo "[local-carrier-setup] Vz substrate probe: discovered launchable capsule '${probe_capsule}' under ${DATA_DIR}/capsules/"
    else
        echo "[local-carrier-setup] Vz substrate probe: no launchable capsule found under ${DATA_DIR}/capsules/ (no rootfs.ext4 installed — expected on a fresh host pre-Phase-6)"
    fi
fi

echo "[local-carrier-setup] temp data dir: ${DATA_DIR}"
echo "[local-carrier-setup] runtime log:   ${LOG_PATH}"
echo "[local-carrier-setup] inspect with:"
echo "  XDG_DATA_HOME=\"${XDG_DATA_HOME}\" \"${ELASTOS_BIN}\""
