#!/usr/bin/env bash
set -euo pipefail
umask 077

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

usage() {
  cat >&2 <<'USAGE'
Usage:
  scripts/mac-source-home-restart.sh [options]

Options:
  --test-home <path>      Source-home root. Default: $MAC_TEST_HOME or ~/elastos-mac-test-home.
  --init                  First-provision admission: require exact clean-source
                          receipt parity (the strict validation path). Without
                          it, the helper starts an existing installation and
                          skips source-parity checks.
  --addr <host:port>      Gateway bind address. Default: $MAC_GATEWAY_ADDR or localhost:61180.
  --log-dir <path>        Restart log directory. Default: <test-home>/logs.
  --dry-run               Validate and print the restart plan without file or process effects.
  --down                  Safe shutdown only: stop the owned gateway for this
                          installation, then exit. No upgrade, validation, or
                          start is performed.
  --json-out <path>       Active receipt. Default: <data-dir>/receipts/mac-source-home-restart.json.
  --wait-seconds <n>      Seconds to wait for Home after start. Default: 40.

Restarts the exact installed source-home Runtime after installation receipt,
process ownership, migration rollback, and product parity checks pass.
USAGE
}

redacted_error() {
  printf 'mac-source-home-restart: %s\n' "$1" >&2
}

sha256_file() {
  python3 - "$1" <<'PY'
import hashlib
import os
import stat
import sys

try:
    descriptor = os.open(sys.argv[1], os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0))
    metadata = os.fstat(descriptor)
    if not stat.S_ISREG(metadata.st_mode) or metadata.st_size <= 0 or metadata.st_size > 2 * 1024 * 1024 * 1024:
        raise OSError
    digest = hashlib.sha256()
    for chunk in iter(lambda: os.read(descriptor, 1024 * 1024), b""):
        digest.update(chunk)
    print(digest.hexdigest())
except OSError:
    raise SystemExit("artifact hash failed")
finally:
    try:
        os.close(descriptor)
    except (NameError, OSError):
        pass
PY
}

tree_sha256() {
  python3 - "$1" <<'PY'
import hashlib
import pathlib
import stat
import sys

root = pathlib.Path(sys.argv[1])
digest = hashlib.sha256()
if not root.is_dir() or root.is_symlink():
    raise SystemExit("renderer tree is unavailable")
for path in sorted(root.rglob("*")):
    metadata = path.lstat()
    if stat.S_ISLNK(metadata.st_mode):
        raise SystemExit("renderer tree is unsafe")
    if not stat.S_ISREG(metadata.st_mode):
        continue
    relative = path.relative_to(root).as_posix().encode()
    digest.update(len(relative).to_bytes(4, "big"))
    digest.update(relative)
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
print(digest.hexdigest())
PY
}

validate_stable_gateway() {
  python3 - "$gateway_bin" "$data_dir" "$repo_root" <<'PY'
import os
import pathlib
import stat
import sys


def reject(code):
    raise SystemExit(f"stable Runtime admission failed: {code}")


try:
    path = pathlib.Path(sys.argv[1])
    data = pathlib.Path(sys.argv[2])
    source = pathlib.Path(sys.argv[3]).resolve(strict=True)
    expected = data / "bin" / "elastos"
    canonical = path.resolve(strict=True)
    canonical_data = data.resolve(strict=True)
except (OSError, RuntimeError):
    reject("unavailable")
text = canonical.as_posix()
if (
    text == "/tmp"
    or text.startswith("/tmp/")
    or text == "/private/tmp"
    or text.startswith("/private/tmp/")
    or text == "/var/tmp"
    or text.startswith("/var/tmp/")
    or "target" in canonical.parts
    or canonical == source
    or source in canonical.parents
):
    reject("disposable_location")
if path != expected or canonical != path or canonical_data != data:
    reject("identity")
try:
    metadata = path.lstat()
except OSError:
    reject("metadata")
if (
    not stat.S_ISREG(metadata.st_mode)
    or stat.S_ISLNK(metadata.st_mode)
    or metadata.st_uid not in (0, os.geteuid())
    or metadata.st_nlink != 1
    or metadata.st_mode & 0o022
    or metadata.st_mode & 0o111 == 0
    or not os.access(path, os.X_OK)
):
    reject("executable")
for parent in (path.parent, data):
    try:
        metadata = parent.lstat()
    except OSError:
        reject("parent")
    if (
        not stat.S_ISDIR(metadata.st_mode)
        or stat.S_ISLNK(metadata.st_mode)
        or metadata.st_uid not in (0, os.geteuid())
        or metadata.st_mode & 0o022
    ):
        reject("parent")
PY
}

validate_installation_receipt() {
  python3 - "$installation_receipt" "$gateway_bin" "$data_dir" "$repo_root" "$init_mode" <<'PY'
import hashlib
import json
import os
import pathlib
import re
import stat
import subprocess
import sys

SCHEMA = "elastos.source-home.installation-receipt/v1"
MAX_RECEIPT = 16 * 1024
MAX_COMPONENTS = 4 * 1024 * 1024
MAX_CAPSULE_RECEIPT = 4 * 1024 * 1024
MAX_RUNTIME = 2 * 1024 * 1024 * 1024


def reject(code):
    raise SystemExit(f"installation receipt rejected: {code}")


def read_hash(path, limit, owner_only=False, executable=False, keep_payload=False):
    flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0)
    try:
        descriptor = os.open(path, flags)
        metadata = os.fstat(descriptor)
        if (
            not stat.S_ISREG(metadata.st_mode)
            or metadata.st_uid not in (0, os.geteuid())
            or metadata.st_nlink != 1
            or metadata.st_size <= 0
            or metadata.st_size > limit
            or (owner_only and metadata.st_mode & 0o077)
            or (executable and metadata.st_mode & 0o111 == 0)
        ):
            reject("unsafe_artifact")
        digest = hashlib.sha256()
        chunks = []
        while True:
            chunk = os.read(descriptor, 1024 * 1024)
            if not chunk:
                break
            digest.update(chunk)
            if keep_payload:
                chunks.append(chunk)
        final = os.fstat(descriptor)
        if final.st_size != metadata.st_size or final.st_mtime_ns != metadata.st_mtime_ns:
            reject("changed")
        return "sha256:" + digest.hexdigest(), b"".join(chunks) if keep_payload else None
    except OSError:
        reject("unavailable")
    finally:
        try:
            os.close(descriptor)
        except (NameError, OSError):
            pass


receipt_path, runtime_path, data_path, source_path = map(pathlib.Path, sys.argv[1:5])
init_mode = sys.argv[5] == "1"
receipt_hash, payload = read_hash(
    receipt_path, MAX_RECEIPT, owner_only=True, keep_payload=True
)
try:
    receipt = json.loads(payload.decode("utf-8"))
except (AttributeError, UnicodeError, json.JSONDecodeError):
    reject("malformed")
if set(receipt) != {
    "schema",
    "source",
    "runtime",
    "components_sha256",
    "source_home_capsule_metadata_receipt_sha256",
    "platform",
    "installation_time_utc",
} or receipt.get("schema") != SCHEMA:
    reject("schema")
if receipt.get("platform") != "darwin-arm64" or not re.fullmatch(
    r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z",
    str(receipt.get("installation_time_utc", "")),
):
    reject("platform_or_time")
source = receipt.get("source")
runtime = receipt.get("runtime")
if not isinstance(source, dict) or set(source) != {"commit", "tree", "clean"}:
    reject("source")
if init_mode and source.get("clean") is not True:
    reject("source_clean")
if not isinstance(runtime, dict) or set(runtime) != {
    "built_sha256",
    "installed_sha256",
    "parity",
}:
    reject("runtime")
if runtime.get("parity") is not True or runtime.get("built_sha256") != runtime.get("installed_sha256"):
    reject("runtime_parity")
env = os.environ.copy()
env["GIT_OPTIONAL_LOCKS"] = "0"
for name in (
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_COMMON_DIR",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
):
    env.pop(name, None)
if init_mode:
    git_values = []
    try:
        for revision in ("HEAD", "HEAD^{tree}"):
            result = subprocess.run(
                ["git", "-C", str(source_path), "rev-parse", "--verify", revision],
                env=env,
                stdin=subprocess.DEVNULL,
                stdout=subprocess.PIPE,
                stderr=subprocess.DEVNULL,
                timeout=10,
                check=False,
                text=True,
            )
            value = result.stdout.strip().lower()
            if result.returncode != 0 or not re.fullmatch(r"[0-9a-f]{40,64}", value):
                reject("source_git")
            git_values.append(value)
    except (OSError, subprocess.SubprocessError):
        reject("source_git")
    if source.get("commit") != git_values[0] or source.get("tree") != git_values[1]:
        reject("source_identity")
    try:
        status = subprocess.run(
            [
                "git",
                "-C",
                str(source_path),
                "status",
                "--porcelain=v1",
                "--untracked-files=all",
            ],
            env=env,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            timeout=10,
            check=False,
        )
    except (OSError, subprocess.SubprocessError):
        reject("source_git")
    if status.returncode != 0:
        reject("source_git")
    if status.stdout:
        reject("source_dirty")
runtime_hash, _ = read_hash(runtime_path, MAX_RUNTIME, executable=True)
components_hash, _ = read_hash(data_path / "components.json", MAX_COMPONENTS)
capsules_hash, _ = read_hash(
    data_path / "receipts" / "source-home-capsules.json", MAX_CAPSULE_RECEIPT
)
if runtime.get("installed_sha256") != runtime_hash:
    reject("runtime_hash")
if receipt.get("components_sha256") != components_hash:
    reject("components_hash")
if receipt.get("source_home_capsule_metadata_receipt_sha256") != capsules_hash:
    reject("capsule_metadata_hash")
for value in (runtime_hash, components_hash, capsules_hash, receipt_hash):
    if not re.fullmatch(r"sha256:[0-9a-f]{64}", value):
        reject("hash")
print(
    receipt_hash.removeprefix("sha256:"),
    runtime_hash.removeprefix("sha256:"),
    components_hash.removeprefix("sha256:"),
    capsules_hash.removeprefix("sha256:"),
)
PY
}

find_debugfs() {
  if [[ -n "${ELASTOS_DEBUGFS_BIN:-}" && -x "${ELASTOS_DEBUGFS_BIN}" ]]; then
    printf '%s\n' "${ELASTOS_DEBUGFS_BIN}"
    return 0
  fi
  if command -v debugfs >/dev/null 2>&1; then
    command -v debugfs
    return 0
  fi
  for candidate in \
    /opt/homebrew/opt/e2fsprogs/sbin/debugfs \
    /usr/local/opt/e2fsprogs/sbin/debugfs \
    /usr/sbin/debugfs \
    /sbin/debugfs
  do
    [[ -x "$candidate" ]] || continue
    printf '%s\n' "$candidate"
    return 0
  done
  return 1
}

initrd_browser_helper_sha256() {
  local initrd="$1"
  local tmp_dir
  tmp_dir="$(mktemp -d)"
  gzip -dc "$initrd" | (cd "$tmp_dir" && cpio -id --quiet bin/browser-selkies-control-service.mjs)
  sha256_file "$tmp_dir/bin/browser-selkies-control-service.mjs"
  rm -rf "$tmp_dir"
}

rootfs_browser_helper_sha256() {
  local rootfs="$1"
  local debugfs="$2"
  local tmp_file
  tmp_file="$(mktemp)"
  "$debugfs" -R "cat /opt/elastos/bin/browser-selkies-control-service.mjs" "$rootfs" >"$tmp_file" 2>/dev/null
  sha256_file "$tmp_file"
  rm -f "$tmp_file"
}

verify_browser_helper_freshness() {
  local source="${repo_root}/scripts/browser-selkies-control-service.mjs"
  local installed="${data_dir}/scripts/browser-selkies-control-service.mjs"
  local initrd="${data_dir}/bin/initrd"
  local rootfs="${data_dir}/browser-vm/rootfs.ext4"
  local debugfs
  if [[ ! -f "$initrd" && ! -f "$rootfs" ]]; then
    # No Browser VM artifacts installed: the documented no-Browser Mac path.
    # Browser VM launch stays unavailable; everything else proceeds.
    printf 'mac-source-home-restart: Browser VM artifacts absent; skipping Browser helper verification
' >&2
    return 0
  fi
  if [[ ! -f "$source" || ! -f "$installed" || ! -f "$initrd" || ! -f "$rootfs" ]]; then
    redacted_error "Mac source-home Browser helper verification failed: artifacts unavailable"
    return 1
  fi
  if ! command -v gzip >/dev/null 2>&1 || ! command -v cpio >/dev/null 2>&1; then
    redacted_error "Browser helper inspection tools are unavailable"
    return 1
  fi
  debugfs="$(find_debugfs || true)"
  if [[ -z "$debugfs" ]]; then
    redacted_error "Browser rootfs inspection is unavailable"
    return 1
  fi
  browser_helper_source_sha="$(sha256_file "$source")"
  browser_helper_installed_sha="$(sha256_file "$installed")"
  browser_helper_initrd_sha="$(initrd_browser_helper_sha256 "$initrd")"
  browser_helper_rootfs_sha="$(rootfs_browser_helper_sha256 "$rootfs" "$debugfs")"
  if [[ "$browser_helper_source_sha" != "$browser_helper_installed_sha" ||
        "$browser_helper_source_sha" != "$browser_helper_initrd_sha" ||
        "$browser_helper_source_sha" != "$browser_helper_rootfs_sha" ]]; then
    redacted_error "Mac source-home Browser helper verification failed: artifact parity"
    return 1
  fi
}

prepare_owner_directory() {
  python3 - "$1" <<'PY'
import os
import pathlib
import stat
import sys

path = pathlib.Path(sys.argv[1])
try:
    path.mkdir(mode=0o700, parents=False, exist_ok=True)
    metadata = path.lstat()
except OSError:
    raise SystemExit("owner directory preparation failed")
if (
    not stat.S_ISDIR(metadata.st_mode)
    or stat.S_ISLNK(metadata.st_mode)
    or metadata.st_uid != os.geteuid()
    or metadata.st_mode & 0o077
):
    raise SystemExit("owner directory admission failed")
PY
}

process_identity() {
  local pid="$1"
  python3 - "$pid" "$gateway_bin" "$addr" <<'PY'
import hashlib
import os
import subprocess
import sys

try:
    pid = int(sys.argv[1])
except ValueError:
    raise SystemExit(1)
binary, addr = sys.argv[2:]
if pid <= 1:
    raise SystemExit(1)
result = subprocess.run(
    ["ps", "-ww", "-p", str(pid), "-o", "uid=", "-o", "lstart=", "-o", "command="],
    stdin=subprocess.DEVNULL,
    stdout=subprocess.PIPE,
    stderr=subprocess.DEVNULL,
    check=False,
    text=True,
)
parts = result.stdout.strip().split(None, 6)
if result.returncode != 0 or len(parts) != 7 or not parts[0].isdigit():
    raise SystemExit(1)
if int(parts[0]) != os.geteuid():
    raise SystemExit(1)
expected = f"{binary} gateway --addr {addr}"
command = parts[6]
try:
    with open(binary, "rb") as handle:
        interpreted = handle.read(2) == b"#!"
except OSError:
    raise SystemExit(1)
if command != expected and not (interpreted and command.endswith(" " + expected)):
    raise SystemExit(1)
start = " ".join(parts[1:6]).encode("ascii", errors="strict")
print(hashlib.sha256(start).hexdigest())
PY
}

process_matches_gateway() {
  local pid="$1"
  local expected_start="${2:-}"
  local actual_start
  if ! actual_start="$(process_identity "$pid")"; then
    return 1
  fi
  [[ -z "$expected_start" || "$actual_start" == "$expected_start" ]]
}

process_is_running() {
  local pid="$1"
  local state
  if ! kill -0 "$pid" 2>/dev/null; then
    return 1
  fi
  state="$(ps -p "$pid" -o stat= 2>/dev/null | awk '{print $1}')"
  [[ -n "$state" && "$state" != Z* ]]
}

listener_pids_for_port() {
  lsof -nP -tiTCP:"$port" -sTCP:LISTEN 2>/dev/null | sort -nu || true
}

read_owned_pid_file() {
  python3 - "$pid_file" "$addr" <<'PY'
import hashlib
import json
import os
import re
import stat
import sys

path, addr = sys.argv[1:]
try:
    descriptor = os.open(path, os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0))
except FileNotFoundError:
    raise SystemExit(3)
except OSError:
    raise SystemExit(2)
try:
    metadata = os.fstat(descriptor)
    if (
        not stat.S_ISREG(metadata.st_mode)
        or metadata.st_uid != os.geteuid()
        or metadata.st_mode & 0o077
        or metadata.st_nlink != 1
        or metadata.st_size <= 0
        or metadata.st_size > 512
    ):
        raise SystemExit(2)
    payload = os.read(descriptor, 513)
finally:
    os.close(descriptor)
try:
    value = json.loads(payload.decode("utf-8"))
except (UnicodeError, json.JSONDecodeError):
    raise SystemExit(2)
if set(value) != {"schema", "pid", "start_identity", "runtime_sha256", "addr"}:
    raise SystemExit(2)
if (
    value.get("schema") != "elastos.mac-source-home-gateway-pid/v1"
    or not isinstance(value.get("pid"), int)
    or value["pid"] <= 1
    or not re.fullmatch(r"[0-9a-f]{64}", str(value.get("start_identity", "")))
    or not re.fullmatch(r"[0-9a-f]{64}", str(value.get("runtime_sha256", "")))
    or value.get("addr") != addr
):
    raise SystemExit(2)
print(
    value["pid"],
    value["start_identity"],
    value["runtime_sha256"],
    hashlib.sha256(payload).hexdigest(),
)
PY
}

write_owned_pid_file() {
  local pid="$1"
  local start_identity="$2"
  python3 - "$pid_file" "$pid" "$start_identity" "$installed_runtime_sha" "$addr" <<'PY'
import hashlib
import json
import os
import pathlib
import secrets
import stat
import sys

path = pathlib.Path(sys.argv[1])
value = {
    "schema": "elastos.mac-source-home-gateway-pid/v1",
    "pid": int(sys.argv[2]),
    "start_identity": sys.argv[3],
    "runtime_sha256": sys.argv[4],
    "addr": sys.argv[5],
}
payload = (json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n").encode()
temporary = path.with_name(f".{path.name}.{os.getpid()}.{secrets.token_hex(8)}.tmp")
descriptor = os.open(
    temporary,
    os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_NOFOLLOW", 0),
    0o600,
)
try:
    view = memoryview(payload)
    while view:
        written = os.write(descriptor, view)
        if written <= 0:
            raise OSError("PID write did not advance")
        view = view[written:]
    os.fsync(descriptor)
finally:
    os.close(descriptor)
try:
    os.replace(temporary, path)
    parent = os.open(path.parent, os.O_RDONLY)
    try:
        os.fsync(parent)
    finally:
        os.close(parent)
finally:
    temporary.unlink(missing_ok=True)
metadata = path.lstat()
if (
    not stat.S_ISREG(metadata.st_mode)
    or metadata.st_uid != os.geteuid()
    or metadata.st_mode & 0o077
    or metadata.st_nlink != 1
):
    raise SystemExit("gateway PID publication was unsafe")
print(hashlib.sha256(payload).hexdigest())
PY
}

remove_owned_pid_file() {
  local expected_pid="$1"
  local expected_start="$2"
  local expected_runtime_sha="$3"
  local expected_payload_sha="$4"
  python3 - "$pid_file" "$expected_pid" "$expected_start" "$expected_runtime_sha" "$addr" "$expected_payload_sha" <<'PY'
import hashlib
import json
import os
import pathlib
import re
import stat
import sys

path = pathlib.Path(sys.argv[1])
expected_pid = int(sys.argv[2])
expected_start, runtime_sha, addr, payload_sha = sys.argv[3:]
if not re.fullmatch(r"[0-9a-f]{64}", payload_sha):
    raise SystemExit(1)
descriptor = os.open(path, os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0))
try:
    metadata = os.fstat(descriptor)
    if (
        not stat.S_ISREG(metadata.st_mode)
        or metadata.st_uid != os.geteuid()
        or metadata.st_mode & 0o077
        or metadata.st_nlink != 1
        or metadata.st_size <= 0
        or metadata.st_size > 512
    ):
        raise SystemExit(1)
    payload = os.read(descriptor, 513)
    value = json.loads(payload.decode("utf-8"))
finally:
    os.close(descriptor)
if hashlib.sha256(payload).hexdigest() != payload_sha:
    raise SystemExit(1)
if value != {
    "schema": "elastos.mac-source-home-gateway-pid/v1",
    "pid": expected_pid,
    "start_identity": expected_start,
    "runtime_sha256": runtime_sha,
    "addr": addr,
}:
    raise SystemExit(1)
current = path.lstat()
if current.st_ino != metadata.st_ino or current.st_dev != metadata.st_dev:
    raise SystemExit(1)
os.unlink(path)
parent = os.open(path.parent, os.O_RDONLY)
try:
    os.fsync(parent)
finally:
    os.close(parent)
PY
}

stop_verified_gateway() {
  local pid="$1"
  local start_identity="$2"
  local attempt
  if ! process_matches_gateway "$pid" "$start_identity"; then
    redacted_error "gateway process identity changed"
    return 1
  fi
  kill -TERM "$pid"
  for attempt in $(seq 1 50); do
    if ! process_is_running "$pid"; then
      return 0
    fi
    sleep 0.1
  done
  if ! process_matches_gateway "$pid" "$start_identity"; then
    redacted_error "gateway process identity changed before forced stop"
    return 1
  fi
  kill -KILL "$pid"
  for attempt in $(seq 1 20); do
    if ! process_is_running "$pid"; then
      return 0
    fi
    sleep 0.1
  done
  redacted_error "gateway process did not stop"
  return 1
}

start_gateway_process() {
  GATEWAY_BIN="$gateway_bin" python3 - "$data_dir" "$test_home" "$addr" "$gateway_log" <<'PY'
import os
import subprocess
import sys

data_dir, home, addr, log_path = sys.argv[1:]
environment = os.environ.copy()
environment["HOME"] = home
log_descriptor = os.open(
    log_path,
    os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_NOFOLLOW", 0),
    0o600,
)
try:
    process = subprocess.Popen(
        [environment["GATEWAY_BIN"], "gateway", "--addr", addr],
        cwd=data_dir,
        env=environment,
        stdin=subprocess.DEVNULL,
        stdout=log_descriptor,
        stderr=subprocess.STDOUT,
        start_new_session=True,
        close_fds=True,
    )
finally:
    os.close(log_descriptor)
print(process.pid)
PY
}

check_no_existing_rollback() {
  python3 - "$backup_root" <<'PY'
import pathlib
import stat
import sys

root = pathlib.Path(sys.argv[1])
if not root.exists():
    raise SystemExit(0)
try:
    root_metadata = root.lstat()
    if not stat.S_ISDIR(root_metadata.st_mode) or stat.S_ISLNK(root_metadata.st_mode):
        raise OSError
    entries = list(root.iterdir())
except OSError:
    raise SystemExit("principal-root rollback inventory is unavailable")
matches = []
for entry in entries:
    if not entry.name.startswith("principal-root-upgrade-"):
        continue
    metadata = entry.lstat()
    if not stat.S_ISDIR(metadata.st_mode) or stat.S_ISLNK(metadata.st_mode):
        raise SystemExit("principal-root rollback reconciliation is required")
    matches.append(entry)
if matches:
    raise SystemExit("principal-root rollback reconciliation is required before restart")
PY
}

rollback_size() {
  python3 - "$principal_root_backup_dir" "$principal_root_upgrade_log" <<'PY'
import json
import os
import pathlib
import stat
import sys

root = pathlib.Path(sys.argv[1])
if not os.path.lexists(root):
    # Only the Runtime's exact empty-install receipt permits no rollback.
    def unique_object(pairs):
        value = dict(pairs)
        if len(value) != len(pairs):
            raise ValueError
        return value

    try:
        descriptor = os.open(sys.argv[2], os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
        with os.fdopen(descriptor, "rb") as receipt_file:
            metadata = os.fstat(receipt_file.fileno())
            if (
                not stat.S_ISREG(metadata.st_mode)
                or metadata.st_uid != os.geteuid()
                or metadata.st_mode & 0o077
                or metadata.st_nlink != 1
                or not 0 < metadata.st_size <= 4096
            ):
                raise ValueError
            receipt = json.loads(receipt_file.read(4097), object_pairs_hook=unique_object)
        expected = {
            "schema": "elastos.principal-root.upgrade-receipt/v1",
            "status": "already_ready",
            "root_count": 0,
            "object_count": 0,
            "roots": [],
        }
        if (
            not isinstance(receipt, dict)
            or receipt != expected
            or type(receipt["root_count"]) is not int
            or type(receipt["object_count"]) is not int
        ):
            raise ValueError
    except (OSError, ValueError, TypeError, KeyError):
        raise SystemExit("principal-root rollback is missing without a verified empty upgrade")
    print(0)
    raise SystemExit(0)
total = 0
for base, directories, files in os.walk(root, followlinks=False):
    base_path = pathlib.Path(base)
    metadata = base_path.lstat()
    if (
        not stat.S_ISDIR(metadata.st_mode)
        or stat.S_ISLNK(metadata.st_mode)
        or metadata.st_uid != os.geteuid()
        or metadata.st_mode & 0o077
    ):
        raise SystemExit("principal-root rollback is unsafe")
    for name in directories + files:
        path = base_path / name
        metadata = path.lstat()
        is_directory = stat.S_ISDIR(metadata.st_mode)
        is_regular = stat.S_ISREG(metadata.st_mode)
        if (
            stat.S_ISLNK(metadata.st_mode)
            or not (is_directory or is_regular)
            or metadata.st_uid != os.geteuid()
            or metadata.st_mode & 0o077
            or (is_regular and metadata.st_nlink != 1)
        ):
            raise SystemExit("principal-root rollback is unsafe")
        if is_regular:
            total += metadata.st_size
            if total > 1024 * 1024 * 1024 * 1024:
                raise SystemExit("principal-root rollback is unbounded")
print(total)
PY
}

emit_json() {
  local out="$1"
  python3 - "$out" <<'PY'
import json
import os
import pathlib
import secrets
import stat
import sys

keys = [
    "schema", "ok", "dry_run", "admission_mode", "generated_at", "repo", "test_home",
    "data_dir", "addr", "home_url", "gateway_bin", "gateway_bin_sha256",
    "installation_receipt_sha256", "installed_runtime_sha256",
    "installed_components_sha256", "source_home_capsule_metadata_receipt_sha256",
    "pid_file", "gateway_pid", "gateway_log", "http_code",
    "served_index_sha256", "installed_index_sha256", "source_index_sha256",
    "browser_helper_source_sha256", "browser_helper_installed_sha256",
    "browser_helper_initrd_sha256", "browser_helper_rootfs_sha256",
    "home_cli_renderer_source_sha256", "home_cli_renderer_installed_sha256",
]
data = {}
for key in keys:
    value = os.environ.get(key.upper())
    if value is None or value == "" or (key in {"gateway_pid", "http_code"} and not value):
        continue
    if key in {"ok", "dry_run"}:
        data[key] = value == "1"
    elif key in {"http_code", "gateway_pid"}:
        data[key] = int(value)
    else:
        data[key] = value
rollback_identity = os.environ.get("ROLLBACK_RELATIVE_IDENTITY", "")
if rollback_identity:
    data["principal_root_rollback"] = {
        "relative_identity": rollback_identity,
        "size_bytes": int(os.environ.get("ROLLBACK_SIZE_BYTES", "0")),
        "reason": "principal_root_upgrade",
        "cleanup_condition": "remove_after_verified_restart_and_explicit_operator_reconciliation",
    }
payload = (json.dumps(data, sort_keys=True, indent=2) + "\n").encode()
if len(payload) > 32 * 1024:
    raise SystemExit("restart receipt is unbounded")
if not sys.argv[1]:
    sys.stdout.buffer.write(payload)
    raise SystemExit(0)
path = pathlib.Path(sys.argv[1])
parent = path.parent
metadata = parent.lstat()
if (
    not stat.S_ISDIR(metadata.st_mode)
    or stat.S_ISLNK(metadata.st_mode)
    or metadata.st_uid != os.geteuid()
    or metadata.st_mode & 0o077
):
    raise SystemExit("restart receipt parent is unsafe")
try:
    existing = path.lstat()
except FileNotFoundError:
    existing = None
if existing is not None and (
    not stat.S_ISREG(existing.st_mode)
    or stat.S_ISLNK(existing.st_mode)
    or existing.st_uid != os.geteuid()
    or existing.st_mode & 0o077
    or existing.st_nlink != 1
):
    raise SystemExit("restart receipt destination is unsafe")
temporary = path.with_name(f".{path.name}.{os.getpid()}.{secrets.token_hex(8)}.tmp")
descriptor = os.open(
    temporary,
    os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_NOFOLLOW", 0),
    0o600,
)
try:
    view = memoryview(payload)
    while view:
        written = os.write(descriptor, view)
        if written <= 0:
            raise OSError("receipt write did not advance")
        view = view[written:]
    os.fsync(descriptor)
finally:
    os.close(descriptor)
try:
    os.replace(temporary, path)
    parent_descriptor = os.open(parent, os.O_RDONLY)
    try:
        os.fsync(parent_descriptor)
    finally:
        os.close(parent_descriptor)
finally:
    temporary.unlink(missing_ok=True)
sys.stdout.buffer.write(payload)
PY
}

validate_receipt_destination() {
  python3 - "$json_out" <<'PY'
import os
import pathlib
import stat
import sys

path = pathlib.Path(sys.argv[1])
try:
    parent = path.parent.lstat()
except OSError:
    raise SystemExit("restart receipt parent is unavailable")
if (
    not stat.S_ISDIR(parent.st_mode)
    or stat.S_ISLNK(parent.st_mode)
    or parent.st_uid != os.geteuid()
    or parent.st_mode & 0o077
):
    raise SystemExit("restart receipt parent is unsafe")
try:
    metadata = path.lstat()
except FileNotFoundError:
    raise SystemExit(0)
if (
    not stat.S_ISREG(metadata.st_mode)
    or stat.S_ISLNK(metadata.st_mode)
    or metadata.st_uid != os.geteuid()
    or metadata.st_mode & 0o077
    or metadata.st_nlink != 1
):
    raise SystemExit("restart receipt destination is unsafe")
PY
}

invalidate_restart_receipt() {
  python3 - "$json_out" <<'PY'
import os
import pathlib
import sys

path = pathlib.Path(sys.argv[1])
try:
    os.unlink(path)
except FileNotFoundError:
    raise SystemExit(0)
parent = os.open(path.parent, os.O_RDONLY)
try:
    os.fsync(parent)
finally:
    os.close(parent)
PY
}

receipt_environment() {
  SCHEMA="elastos.mac-source-home-restart/v1" \
  OK="$receipt_ok" DRY_RUN="$dry_run" \
  ADMISSION_MODE="$([[ "$init_mode" -eq 1 ]] && printf init || printf existing)" \
  GENERATED_AT="$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
  REPO="$repo_root" TEST_HOME="$test_home" DATA_DIR="$data_dir" ADDR="$addr" \
  HOME_URL="$home_url" GATEWAY_BIN="$gateway_bin" \
  GATEWAY_BIN_SHA256="$installed_runtime_sha" \
  INSTALLATION_RECEIPT_SHA256="$installation_receipt_sha" \
  INSTALLED_RUNTIME_SHA256="$installed_runtime_sha" \
  INSTALLED_COMPONENTS_SHA256="$installed_components_sha" \
  SOURCE_HOME_CAPSULE_METADATA_RECEIPT_SHA256="$capsule_metadata_receipt_sha" \
  PID_FILE="$pid_file" GATEWAY_PID="$gateway_pid" GATEWAY_LOG="$gateway_log" \
  HTTP_CODE="$http_code" SERVED_INDEX_SHA256="$served_hash" \
  INSTALLED_INDEX_SHA256="$installed_hash" SOURCE_INDEX_SHA256="$source_hash" \
  BROWSER_HELPER_SOURCE_SHA256="$browser_helper_source_sha" \
  BROWSER_HELPER_INSTALLED_SHA256="$browser_helper_installed_sha" \
  BROWSER_HELPER_INITRD_SHA256="$browser_helper_initrd_sha" \
  BROWSER_HELPER_ROOTFS_SHA256="$browser_helper_rootfs_sha" \
  HOME_CLI_RENDERER_SOURCE_SHA256="$home_cli_renderer_source_sha" \
  HOME_CLI_RENDERER_INSTALLED_SHA256="$home_cli_renderer_installed_sha" \
  ROLLBACK_RELATIVE_IDENTITY="$rollback_relative_identity" \
  ROLLBACK_SIZE_BYTES="$rollback_size_bytes" \
  emit_json "$1"
}

test_home="${MAC_TEST_HOME:-${HOME}/elastos-mac-test-home}"
addr="${MAC_GATEWAY_ADDR:-localhost:61180}"
log_dir=""
wait_seconds=40
dry_run=0
init_mode=0
down_mode=0
json_out=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --test-home)
      test_home="${2:-}"
      [[ -n "$test_home" ]] || { redacted_error "--test-home requires a path"; exit 2; }
      shift 2
      ;;
    --addr)
      addr="${2:-}"
      [[ -n "$addr" ]] || { redacted_error "--addr requires host:port"; exit 2; }
      shift 2
      ;;
    --log-dir)
      log_dir="${2:-}"
      [[ -n "$log_dir" ]] || { redacted_error "--log-dir requires a path"; exit 2; }
      shift 2
      ;;
    --wait-seconds)
      wait_seconds="${2:-}"
      [[ -n "$wait_seconds" ]] || { redacted_error "--wait-seconds requires a value"; exit 2; }
      shift 2
      ;;
    --dry-run)
      dry_run=1
      shift
      ;;
    --init)
      init_mode=1
      shift
      ;;
    --down)
      down_mode=1
      shift
      ;;
    --json-out)
      json_out="${2:-}"
      [[ -n "$json_out" ]] || { redacted_error "--json-out requires a path"; exit 2; }
      shift 2
      ;;
    --help|-h)
      usage
      exit 0
      ;;
    *)
      redacted_error "unknown argument"
      usage
      exit 2
      ;;
  esac
done

if [[ "$test_home" != /* || ( -n "$log_dir" && "$log_dir" != /* ) || ( -n "$json_out" && "$json_out" != /* ) ]]; then
  redacted_error "paths must be absolute"
  exit 2
fi
if [[ ! "$addr" =~ ^[^[:space:]:]+:[0-9]+$ ]]; then
  redacted_error "--addr must be host:port"
  exit 2
fi
port="${addr##*:}"
if [[ "$port" -lt 1 || "$port" -gt 65535 ]]; then
  redacted_error "--addr port is out of range"
  exit 2
fi
if [[ ! "$wait_seconds" =~ ^[0-9]+$ || "$wait_seconds" -lt 1 || "$wait_seconds" -gt 300 ]]; then
  redacted_error "--wait-seconds is out of range"
  exit 2
fi
if [[ "$dry_run" -eq 1 && -n "$json_out" ]]; then
  redacted_error "--json-out is unavailable in dry-run mode"
  exit 2
fi

data_dir="${test_home}/Library/Application Support/elastos"
gateway_bin="${data_dir}/bin/elastos"
installation_receipt="${data_dir}/receipts/source-home-installation.json"
pid_file="${data_dir}/run/gateway.pid"
log_dir="${log_dir:-${test_home}/logs}"
json_out="${json_out:-${data_dir}/receipts/mac-source-home-restart.json}"
gateway_log="${log_dir}/gateway-$(date -u +%Y%m%dT%H%M%SZ)-$$.log"
home_url="http://${addr}/apps/home/"
backup_root="${data_dir}/backups"
principal_root_backup_dir="${backup_root}/principal-root-upgrade-$(date -u +%Y%m%dT%H%M%SZ)-$$"
planned_rollback_relative_identity="backups/${principal_root_backup_dir##*/}"
rollback_relative_identity=""
principal_root_upgrade_log="${log_dir}/principal-root-upgrade-$(date -u +%Y%m%dT%H%M%SZ)-$$.json"

if [[ "$dry_run" -eq 1 ]]; then
  validate_stable_gateway
  if ! installation_values="$(validate_installation_receipt)"; then
    exit 1
  fi
  read -r installation_receipt_sha installed_runtime_sha installed_components_sha \
    capsule_metadata_receipt_sha <<<"$installation_values"
  gateway_pid=""
  gateway_start_identity=""
  gateway_pid_runtime_sha="$installed_runtime_sha"
  gateway_pid_payload_sha=""
  http_code=""
  served_hash=""
  installed_hash=""
  source_hash=""
  browser_helper_source_sha=""
  browser_helper_installed_sha=""
  browser_helper_initrd_sha=""
  browser_helper_rootfs_sha=""
  home_cli_renderer_source_sha=""
  home_cli_renderer_installed_sha=""
  rollback_size_bytes="0"
  receipt_ok=1
  receipt_environment ""
  exit 0
fi

if [[ "$down_mode" -eq 1 ]]; then
  if [[ "$(uname -s)" != "Darwin" ]]; then
    redacted_error "this restart owner requires macOS"
    exit 2
  fi
  down_status=0
  # Shutdown is install-scoped, not addr-scoped: adopt the addr the owned
  # PID file records so --down works without remembering the bind address.
  if [[ -e "$pid_file" && ! -L "$pid_file" ]]; then
    pid_file_addr="$(python3 - "$pid_file" <<'PY' 2>/dev/null
import json, sys
value = json.load(open(sys.argv[1]))
addr = value.get("addr", "")
if isinstance(addr, str) and addr:
    print(addr)
PY
)" || pid_file_addr=""
    [[ -n "$pid_file_addr" ]] && addr="$pid_file_addr"
  fi
  if pid_values="$(read_owned_pid_file)"; then
    read -r down_pid down_start down_runtime_sha down_payload_sha <<<"$pid_values"
    if process_is_running "$down_pid"; then
      if stop_verified_gateway "$down_pid" "$down_start"; then
        printf 'mac-source-home-restart: gateway stopped
' >&2
      else
        redacted_error "gateway stop failed; PID file retained"
        down_status=1
      fi
    fi
    if [[ "$down_status" -eq 0 ]]; then
      remove_owned_pid_file "$down_pid" "$down_start" "$down_runtime_sha" "$down_payload_sha" 2>/dev/null || {
        redacted_error "gateway PID file changed during shutdown"
        down_status=1
      }
    fi
  elif [[ $? -ne 3 ]]; then
    redacted_error "gateway PID file is unsafe or malformed"
    down_status=1
  fi
  if command -v lsof >/dev/null 2>&1 && [[ -n "$(listener_pids_for_port)" ]]; then
    redacted_error "an unrelated listener still owns the selected address; not touched"
    down_status=1
  fi
  exit "$down_status"
fi

validate_stable_gateway
if ! installation_values="$(validate_installation_receipt)"; then
  exit 1
fi
read -r installation_receipt_sha installed_runtime_sha installed_components_sha \
  capsule_metadata_receipt_sha <<<"$installation_values"

gateway_pid=""
gateway_start_identity=""
gateway_pid_runtime_sha="$installed_runtime_sha"
gateway_pid_payload_sha=""
http_code=""
served_hash=""
installed_hash=""
source_hash=""
browser_helper_source_sha=""
browser_helper_installed_sha=""
browser_helper_initrd_sha=""
browser_helper_rootfs_sha=""
home_cli_renderer_source_sha=""
home_cli_renderer_installed_sha=""
rollback_size_bytes="0"
receipt_ok=1

rollback_relative_identity="$planned_rollback_relative_identity"
if [[ "$(uname -s)" != "Darwin" ]]; then
  redacted_error "this restart owner requires macOS"
  exit 2
fi
if ! command -v lsof >/dev/null 2>&1; then
  redacted_error "exact listener inspection is unavailable"
  exit 2
fi

if [[ "$init_mode" -eq 1 ]]; then
  verify_browser_helper_freshness
  home_cli_renderer_source_sha="$(tree_sha256 "${repo_root}/capsules/home-cli/browser")"
  home_cli_renderer_installed_sha="$(tree_sha256 "${data_dir}/capsules/home-cli/browser")"
  if [[ "$home_cli_renderer_source_sha" != "$home_cli_renderer_installed_sha" ]]; then
    redacted_error "Home CLI renderer parity failed"
    exit 1
  fi
  source_hash="$(sha256_file "${repo_root}/capsules/home/browser/index.html")"
  if [[ "$source_hash" != "$(sha256_file "${data_dir}/capsules/home/browser/index.html")" ]]; then
    redacted_error "Home source and installed parity failed"
    exit 1
  fi
fi
installed_hash="$(sha256_file "${data_dir}/capsules/home/browser/index.html")"

check_no_existing_rollback
prepare_owner_directory "$log_dir"
prepare_owner_directory "${data_dir}/run"
validate_receipt_destination

restart_succeeded=0
cleanup_failed_restart() {
  local status=$?
  trap - EXIT
  if [[ "$restart_succeeded" -ne 1 && -n "$gateway_pid" ]]; then
    if [[ -z "$gateway_start_identity" ]]; then
      gateway_start_identity="$(process_identity "$gateway_pid" 2>/dev/null || true)"
    fi
    if [[ -n "$gateway_start_identity" ]] && process_matches_gateway "$gateway_pid" "$gateway_start_identity"; then
      stop_verified_gateway "$gateway_pid" "$gateway_start_identity" >/dev/null 2>&1 || true
    fi
    if [[ -n "$gateway_start_identity" && ( -e "$pid_file" || -L "$pid_file" ) ]]; then
      if [[ -z "$gateway_pid_payload_sha" ]]; then
        cleanup_pid_values="$(read_owned_pid_file 2>/dev/null || true)"
        read -r cleanup_pid cleanup_start cleanup_runtime cleanup_payload <<<"$cleanup_pid_values"
        if [[ "$cleanup_pid" == "$gateway_pid" &&
              "$cleanup_start" == "$gateway_start_identity" &&
              "$cleanup_runtime" == "$gateway_pid_runtime_sha" ]]; then
          gateway_pid_payload_sha="$cleanup_payload"
        fi
      fi
      if [[ -n "$gateway_pid_payload_sha" ]]; then
        remove_owned_pid_file "$gateway_pid" "$gateway_start_identity" \
          "$gateway_pid_runtime_sha" "$gateway_pid_payload_sha" >/dev/null 2>&1 || true
      fi
    fi
  fi
  if [[ -d "$principal_root_backup_dir" && ! -L "$principal_root_backup_dir" ]]; then
    rollback_size_bytes="$(rollback_size 2>/dev/null || printf '0')"
  else
    rollback_relative_identity=""
    rollback_size_bytes="0"
  fi
  receipt_ok=0
  receipt_environment "$json_out" >/dev/null 2>&1 || true
  exit "$status"
}
trap cleanup_failed_restart EXIT

old_gateway_pid=""
old_gateway_start=""
old_gateway_runtime_sha=""
old_gateway_pid_payload_sha=""
pid_status=0
if pid_values="$(read_owned_pid_file)"; then
  read -r old_gateway_pid old_gateway_start old_gateway_runtime_sha \
    old_gateway_pid_payload_sha <<<"$pid_values"
  if process_is_running "$old_gateway_pid"; then
    if ! process_matches_gateway "$old_gateway_pid" "$old_gateway_start"; then
      redacted_error "PID file process identity changed"
      exit 1
    fi
  else
    remove_owned_pid_file "$old_gateway_pid" "$old_gateway_start" \
      "$old_gateway_runtime_sha" "$old_gateway_pid_payload_sha" 2>/dev/null || {
      redacted_error "stale PID file changed"
      exit 1
    }
    old_gateway_pid=""
    old_gateway_start=""
  fi
else
  pid_status=$?
  if [[ "$pid_status" -ne 3 ]]; then
    redacted_error "gateway PID file is unsafe or malformed"
    exit 1
  fi
fi

listener_count=0
listener_pid=""
while IFS= read -r candidate; do
  [[ -n "$candidate" ]] || continue
  listener_count=$((listener_count + 1))
  listener_pid="$candidate"
done < <(listener_pids_for_port)
if [[ "$listener_count" -gt 1 ]]; then
  redacted_error "multiple listeners own the selected address"
  exit 1
fi
if [[ "$listener_count" -eq 1 ]]; then
  if [[ -z "$old_gateway_pid" || "$listener_pid" != "$old_gateway_pid" ]]; then
    redacted_error "an unrelated listener owns the selected address"
    exit 1
  fi
  if ! process_matches_gateway "$listener_pid" "$old_gateway_start"; then
    redacted_error "listener process identity changed"
    exit 1
  fi
elif [[ -n "$old_gateway_pid" ]]; then
  redacted_error "the PID file process does not own the selected listener"
  exit 1
fi

if ! invalidate_restart_receipt 2>/dev/null; then
  redacted_error "restart receipt invalidation failed"
  exit 1
fi
if [[ -n "$old_gateway_pid" ]]; then
  stop_verified_gateway "$old_gateway_pid" "$old_gateway_start"
  remove_owned_pid_file "$old_gateway_pid" "$old_gateway_start" \
    "$old_gateway_runtime_sha" "$old_gateway_pid_payload_sha" 2>/dev/null || {
    redacted_error "gateway PID file changed during stop"
    exit 1
  }
fi
if [[ -n "$(listener_pids_for_port)" ]]; then
  redacted_error "the selected listener did not settle"
  exit 1
fi

prepare_owner_directory "$backup_root"

if ! python3 - "$principal_root_upgrade_log" 2>/dev/null <<'PY'
import os
import sys

descriptor = os.open(
    sys.argv[1],
    os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_NOFOLLOW", 0),
    0o600,
)
os.close(descriptor)
PY
then
  redacted_error "principal-root upgrade log creation failed"
  exit 1
fi
if ! "$gateway_bin" principal-root-upgrade \
  --data-dir "$data_dir" \
  --backup-dir "$principal_root_backup_dir" \
  >"$principal_root_upgrade_log" 2>&1; then
  redacted_error "principal-root upgrade failed; rollback retained for reconciliation"
  exit 1
fi
rollback_size_bytes="$(rollback_size)"
if [[ ! -e "$principal_root_backup_dir" && ! -L "$principal_root_backup_dir" ]]; then
  rollback_relative_identity=""
fi

if ! gateway_pid="$(start_gateway_process 2>/dev/null)"; then
  redacted_error "gateway process start failed"
  exit 1
fi
if [[ ! "$gateway_pid" =~ ^[0-9]+$ || "$gateway_pid" -le 1 ]]; then
  redacted_error "gateway start did not return a valid process"
  exit 1
fi
if ! gateway_start_identity="$(process_identity "$gateway_pid")"; then
  redacted_error "started gateway identity is unavailable"
  exit 1
fi
if ! gateway_pid_payload_sha="$(write_owned_pid_file "$gateway_pid" "$gateway_start_identity" 2>/dev/null)"; then
  redacted_error "gateway PID publication failed"
  exit 1
fi
if ! process_matches_gateway "$gateway_pid" "$gateway_start_identity"; then
  redacted_error "started gateway identity changed"
  exit 1
fi

for _ in $(seq 1 "$wait_seconds"); do
  if ! process_matches_gateway "$gateway_pid" "$gateway_start_identity"; then
    redacted_error "gateway exited before readiness"
    exit 1
  fi
  if curl -fsS -o /dev/null "$home_url" 2>/dev/null; then
    break
  fi
  sleep 1
done
if ! process_matches_gateway "$gateway_pid" "$gateway_start_identity"; then
  redacted_error "gateway identity changed before readiness"
  exit 1
fi
live_listener_count=0
live_listener_pid=""
while IFS= read -r candidate; do
  [[ -n "$candidate" ]] || continue
  live_listener_count=$((live_listener_count + 1))
  live_listener_pid="$candidate"
done < <(listener_pids_for_port)
if [[ "$live_listener_count" -ne 1 || "$live_listener_pid" != "$gateway_pid" ]]; then
  redacted_error "gateway does not exclusively own the selected listener"
  exit 1
fi

http_code="$(curl -fsS -o /dev/null -w '%{http_code}' "$home_url" 2>/dev/null || true)"
served_hash="$(curl -fsS "$home_url" 2>/dev/null | shasum -a 256 | awk '{print $1}')"
if [[ "$http_code" != "200" || "$served_hash" != "$installed_hash" ]]; then
  redacted_error "Home readiness parity failed"
  exit 1
fi
if [[ "$init_mode" -eq 1 && "$served_hash" != "$source_hash" ]]; then
  redacted_error "Home readiness source parity failed"
  exit 1
fi

receipt_ok=1
if ! receipt_output="$(receipt_environment "$json_out" 2>/dev/null)"; then
  redacted_error "restart receipt publication failed"
  exit 1
fi
printf '%s\n' "$receipt_output"
restart_succeeded=1
trap - EXIT
