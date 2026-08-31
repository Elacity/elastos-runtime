#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

exec python3 - "$repo_root" "$@" <<'PY'
import argparse
import datetime
import hashlib
import http.client
import json
import os
import pathlib
import platform
import re
import secrets
import select
import signal
import stat
import subprocess
import sys
import time
import urllib.parse


INSTALL_SCHEMA = "elastos.source-home.installation-receipt/v1"
RESTART_SCHEMA = "elastos.linux-source-home-restart/v1"
PID_SCHEMA = "elastos.linux-source-home-gateway-pid/v1"
MAX_INSTALL_RECEIPT = 16 * 1024
MAX_RESTART_RECEIPT = 32 * 1024
MAX_PID_FILE = 4 * 1024
MAX_RUNTIME = 2 * 1024 * 1024 * 1024
MAX_COMPONENTS = 8 * 1024 * 1024
MAX_CAPSULE_RECEIPT = 8 * 1024 * 1024
MAX_PAGE = 8 * 1024 * 1024
MAX_ROLLBACK = 1024 * 1024 * 1024 * 1024
HASH_RE = re.compile(r"[0-9a-f]{64}")
SOURCE_ID_RE = re.compile(r"[0-9a-f]{40,64}")
UTC_RE = re.compile(r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z")


class RestartError(Exception):
    pass


def reject(code):
    raise RestartError(code)


def utc_now():
    return datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def expected_platform():
    machine = platform.machine().lower()
    if machine in {"x86_64", "amd64"}:
        return "linux-amd64"
    if machine in {"aarch64", "arm64"}:
        return "linux-arm64"
    reject("unsupported_linux_architecture")


def sha256_bytes(value):
    return hashlib.sha256(value).hexdigest()


def read_regular(path, limit, *, owner_only=False, executable=False):
    try:
        metadata = path.lstat()
    except OSError:
        reject("required_artifact_unavailable")
    if (
        not stat.S_ISREG(metadata.st_mode)
        or stat.S_ISLNK(metadata.st_mode)
        or metadata.st_uid not in {0, os.geteuid()}
        or metadata.st_mode & 0o022
        or metadata.st_nlink != 1
        or metadata.st_size <= 0
        or metadata.st_size > limit
        or (owner_only and metadata.st_mode & 0o077)
        or (executable and not metadata.st_mode & stat.S_IXUSR)
    ):
        reject("unsafe_artifact")
    flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0)
    try:
        descriptor = os.open(path, flags)
        opened = os.fstat(descriptor)
        if (opened.st_dev, opened.st_ino) != (metadata.st_dev, metadata.st_ino):
            reject("artifact_changed")
        chunks = []
        total = 0
        while True:
            chunk = os.read(descriptor, min(1024 * 1024, limit + 1 - total))
            if not chunk:
                break
            chunks.append(chunk)
            total += len(chunk)
            if total > limit:
                reject("artifact_unbounded")
        closed = os.fstat(descriptor)
        if (
            (closed.st_dev, closed.st_ino, closed.st_size)
            != (metadata.st_dev, metadata.st_ino, metadata.st_size)
        ):
            reject("artifact_changed")
    except OSError:
        reject("artifact_read_failed")
    finally:
        try:
            os.close(descriptor)
        except (NameError, OSError):
            pass
    return b"".join(chunks)


def sha256_file(path, limit, **options):
    return sha256_bytes(read_regular(path, limit, **options))


def ensure_owner_directory(path, *, create=False):
    if create:
        try:
            path.mkdir(mode=0o700, parents=True, exist_ok=True)
        except OSError:
            reject("owner_directory_unavailable")
    try:
        metadata = path.lstat()
    except OSError:
        reject("owner_directory_unavailable")
    if (
        not stat.S_ISDIR(metadata.st_mode)
        or stat.S_ISLNK(metadata.st_mode)
        or metadata.st_uid != os.geteuid()
        or metadata.st_mode & 0o077
    ):
        reject("unsafe_owner_directory")


def path_is_within(path, parent):
    try:
        path.relative_to(parent)
        return True
    except ValueError:
        return False


def validate_stable_install(data_dir, gateway, source_root):
    try:
        canonical_data = data_dir.resolve(strict=True)
        canonical_gateway = gateway.resolve(strict=True)
        canonical_source = source_root.resolve(strict=True)
    except OSError:
        reject("stable_install_unavailable")
    if data_dir.absolute() != canonical_data or gateway.absolute() != canonical_gateway:
        reject("unstable_runtime_path")
    disposable = tuple(pathlib.Path(value) for value in ("/tmp", "/private/tmp", "/var/tmp"))
    if any(path_is_within(canonical_data, root) for root in disposable):
        reject("disposable_install_location")
    if any(part.lower() == "target" for part in canonical_data.parts):
        reject("disposable_install_location")
    if path_is_within(canonical_data, canonical_source):
        reject("source_checkout_install_location")
    if canonical_gateway != canonical_data / "bin" / "elastos":
        reject("unstable_runtime_path")
    ensure_owner_directory(canonical_data)
    ensure_owner_directory(canonical_data / "bin")
    ensure_owner_directory(canonical_data / "receipts")
    sha256_file(canonical_gateway, MAX_RUNTIME, executable=True)
    return canonical_data, canonical_gateway, canonical_source


def clean_git_environment():
    environment = os.environ.copy()
    environment["GIT_OPTIONAL_LOCKS"] = "0"
    for name in (
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_COMMON_DIR",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    ):
        environment.pop(name, None)
    return environment


def git_source_identity(source_root):
    environment = clean_git_environment()
    values = []
    try:
        for revision in ("HEAD", "HEAD^{tree}"):
            result = subprocess.run(
                ["git", "-C", str(source_root), "rev-parse", "--verify", revision],
                env=environment,
                stdin=subprocess.DEVNULL,
                stdout=subprocess.PIPE,
                stderr=subprocess.DEVNULL,
                timeout=10,
                check=False,
                text=True,
            )
            value = result.stdout.strip().lower()
            if result.returncode != 0 or not SOURCE_ID_RE.fullmatch(value):
                reject("source_git")
            values.append(value)
        status = subprocess.run(
            [
                "git",
                "-C",
                str(source_root),
                "status",
                "--porcelain=v1",
                "--untracked-files=all",
            ],
            env=environment,
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
    return values


def validate_installation_receipt(receipt_path, data_dir, gateway, source_root):
    receipt_bytes = read_regular(
        receipt_path, MAX_INSTALL_RECEIPT, owner_only=True
    )
    try:
        receipt = json.loads(receipt_bytes.decode("utf-8"))
    except (UnicodeError, json.JSONDecodeError):
        reject("installation_receipt_malformed")
    if set(receipt) != {
        "schema",
        "source",
        "runtime",
        "components_sha256",
        "source_home_capsule_metadata_receipt_sha256",
        "platform",
        "installation_time_utc",
    } or receipt.get("schema") != INSTALL_SCHEMA:
        reject("installation_receipt_schema")
    if receipt.get("platform") != expected_platform() or not UTC_RE.fullmatch(
        str(receipt.get("installation_time_utc", ""))
    ):
        reject("installation_receipt_platform")
    source = receipt.get("source")
    runtime = receipt.get("runtime")
    if not isinstance(source, dict) or set(source) != {"commit", "tree", "clean"}:
        reject("installation_receipt_source")
    if source.get("clean") is not True:
        reject("installation_receipt_source_clean")
    if not isinstance(runtime, dict) or set(runtime) != {
        "built_sha256",
        "installed_sha256",
        "parity",
    }:
        reject("installation_receipt_runtime")
    if (
        runtime.get("parity") is not True
        or runtime.get("built_sha256") != runtime.get("installed_sha256")
    ):
        reject("installation_receipt_runtime_parity")
    source_commit, source_tree = git_source_identity(source_root)
    if source.get("commit") != source_commit or source.get("tree") != source_tree:
        reject("installation_receipt_source_identity")
    runtime_hash = sha256_file(gateway, MAX_RUNTIME, executable=True)
    components_hash = sha256_file(data_dir / "components.json", MAX_COMPONENTS)
    capsules_hash = sha256_file(
        data_dir / "receipts" / "source-home-capsules.json", MAX_CAPSULE_RECEIPT
    )
    if runtime.get("installed_sha256") != f"sha256:{runtime_hash}":
        reject("installation_receipt_runtime_hash")
    if receipt.get("components_sha256") != f"sha256:{components_hash}":
        reject("installation_receipt_components_hash")
    if receipt.get("source_home_capsule_metadata_receipt_sha256") != f"sha256:{capsules_hash}":
        reject("installation_receipt_capsule_hash")
    return {
        "installation_receipt_sha256": sha256_bytes(receipt_bytes),
        "installed_runtime_sha256": runtime_hash,
        "installed_components_sha256": components_hash,
        "source_home_capsule_metadata_receipt_sha256": capsules_hash,
    }


def validate_asset_parity(source_root, data_dir):
    result = {}
    for name in ("home", "services"):
        source = source_root / "capsules" / name / "browser" / "index.html"
        installed = data_dir / "capsules" / name / "browser" / "index.html"
        source_hash = sha256_file(source, MAX_PAGE)
        installed_hash = sha256_file(installed, MAX_PAGE)
        if source_hash != installed_hash:
            reject(f"{name}_installed_source_mismatch")
        result[f"{name}_source_index_sha256"] = source_hash
        result[f"{name}_installed_index_sha256"] = installed_hash
    return result


def process_state_and_start(pid):
    try:
        value = (pathlib.Path("/proc") / str(pid) / "stat").read_text(encoding="ascii")
    except OSError:
        return None
    close = value.rfind(")")
    if close < 0:
        return None
    fields = value[close + 2 :].split()
    if len(fields) < 20 or fields[0] == "Z" or not fields[19].isdigit():
        return None
    return fields[0], fields[19]


def process_identity(pid, gateway, addr, runtime_hash):
    proc = pathlib.Path("/proc") / str(pid)
    if not isinstance(runtime_hash, str) or not HASH_RE.fullmatch(runtime_hash):
        return None
    state_start = process_state_and_start(pid)
    if state_start is None:
        return None
    _, start_time = state_start
    try:
        status_lines = (proc / "status").read_text(encoding="ascii").splitlines()
        uid_line = next(line for line in status_lines if line.startswith("Uid:"))
        uid_values = uid_line.split()[1:]
        if len(uid_values) < 2 or int(uid_values[0]) != os.geteuid() or int(uid_values[1]) != os.geteuid():
            return None
        executable_link = os.readlink(proc / "exe")
        cmdline = (proc / "cmdline").read_bytes()
    except (OSError, StopIteration, ValueError):
        return None
    stable_executable = str(gateway)
    deleted_executable = f"{stable_executable} (deleted)"
    if executable_link not in {stable_executable, deleted_executable}:
        return None
    descriptor = None
    try:
        descriptor = os.open(proc / "exe", os.O_RDONLY)
        metadata = os.fstat(descriptor)
        if (
            not stat.S_ISREG(metadata.st_mode)
            or metadata.st_uid not in {0, os.geteuid()}
            or metadata.st_mode & 0o022
            or not metadata.st_mode & stat.S_IXUSR
            or metadata.st_size <= 0
            or metadata.st_size > MAX_RUNTIME
            or (executable_link == stable_executable and metadata.st_nlink != 1)
            or (executable_link == deleted_executable and metadata.st_nlink != 0)
        ):
            return None
        digest = hashlib.sha256()
        total = 0
        while True:
            chunk = os.read(descriptor, min(1024 * 1024, MAX_RUNTIME + 1 - total))
            if not chunk:
                break
            total += len(chunk)
            if total > MAX_RUNTIME:
                return None
            digest.update(chunk)
        if digest.hexdigest() != runtime_hash:
            return None
    except OSError:
        return None
    finally:
        if descriptor is not None:
            os.close(descriptor)
    expected = os.fsencode(str(gateway)) + b"\0gateway\0--addr\0" + os.fsencode(addr) + b"\0"
    if cmdline != expected:
        return None
    material = (
        str(os.geteuid()).encode("ascii")
        + b"\0"
        + os.fsencode(stable_executable)
        + b"\0"
        + start_time.encode("ascii")
        + b"\0"
        + runtime_hash.encode("ascii")
        + b"\0"
        + cmdline
    )
    return hashlib.sha256(material).hexdigest()


def process_running(pid):
    return process_state_and_start(pid) is not None


def pid_payload(pid, identity, runtime_hash, addr):
    return {
        "schema": PID_SCHEMA,
        "pid": pid,
        "start_identity": identity,
        "runtime_sha256": runtime_hash,
        "addr": addr,
    }


def validate_pid_destination(path):
    ensure_owner_directory(path.parent, create=True)
    try:
        metadata = path.lstat()
    except FileNotFoundError:
        return
    except OSError:
        reject("pid_file_unavailable")
    if (
        not stat.S_ISREG(metadata.st_mode)
        or stat.S_ISLNK(metadata.st_mode)
        or metadata.st_uid != os.geteuid()
        or metadata.st_mode & 0o077
        or metadata.st_nlink != 1
        or metadata.st_size <= 0
        or metadata.st_size > MAX_PID_FILE
    ):
        reject("pid_file_unsafe")


def read_pid_file(path, addr):
    try:
        path.lstat()
    except FileNotFoundError:
        return None
    payload = read_regular(path, MAX_PID_FILE, owner_only=True)
    try:
        value = json.loads(payload.decode("utf-8"))
    except (UnicodeError, json.JSONDecodeError):
        reject("pid_file_malformed")
    if set(value) != {"schema", "pid", "start_identity", "runtime_sha256", "addr"}:
        reject("pid_file_malformed")
    if (
        value.get("schema") != PID_SCHEMA
        or not isinstance(value.get("pid"), int)
        or value["pid"] <= 1
        or not isinstance(value.get("start_identity"), str)
        or not HASH_RE.fullmatch(value["start_identity"])
        or not isinstance(value.get("runtime_sha256"), str)
        or not HASH_RE.fullmatch(value["runtime_sha256"])
        or value.get("addr") != addr
    ):
        reject("pid_file_malformed")
    value["_bytes"] = payload
    return value


def atomic_write(path, payload, limit, *, replace=True):
    if len(payload) <= 0 or len(payload) > limit:
        reject("bounded_write_failed")
    ensure_owner_directory(path.parent)
    try:
        existing = path.lstat()
    except FileNotFoundError:
        existing = None
    except OSError:
        reject("destination_unavailable")
    if existing is not None and (
        not replace
        or not stat.S_ISREG(existing.st_mode)
        or stat.S_ISLNK(existing.st_mode)
        or existing.st_uid != os.geteuid()
        or existing.st_mode & 0o077
        or existing.st_nlink != 1
    ):
        reject("destination_unsafe")
    temporary = path.with_name(f".{path.name}.{os.getpid()}.{secrets.token_hex(8)}.tmp")
    descriptor = None
    try:
        descriptor = os.open(
            temporary,
            os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_NOFOLLOW", 0),
            0o600,
        )
        view = memoryview(payload)
        while view:
            written = os.write(descriptor, view)
            if written <= 0:
                reject("bounded_write_failed")
            view = view[written:]
        os.fsync(descriptor)
        os.close(descriptor)
        descriptor = None
        os.replace(temporary, path)
        parent_fd = os.open(path.parent, os.O_RDONLY | getattr(os, "O_DIRECTORY", 0))
        try:
            os.fsync(parent_fd)
        finally:
            os.close(parent_fd)
    except OSError:
        reject("bounded_write_failed")
    finally:
        if descriptor is not None:
            os.close(descriptor)
        try:
            temporary.unlink()
        except FileNotFoundError:
            pass


def pid_file_bytes(value):
    public = {
        name: value[name]
        for name in ("schema", "pid", "start_identity", "runtime_sha256", "addr")
    }
    return (json.dumps(public, sort_keys=True, separators=(",", ":")) + "\n").encode()


def write_pid_file(path, payload):
    atomic_write(path, payload, MAX_PID_FILE)


def remove_matching_pid_file(path, expected):
    try:
        current = read_regular(path, MAX_PID_FILE, owner_only=True)
    except RestartError:
        reject("pid_file_changed")
    if current != expected["_bytes"]:
        reject("pid_file_changed")
    try:
        path.unlink()
        parent_fd = os.open(path.parent, os.O_RDONLY | getattr(os, "O_DIRECTORY", 0))
        try:
            os.fsync(parent_fd)
        finally:
            os.close(parent_fd)
    except OSError:
        reject("pid_file_changed")


def listener_inodes(port):
    inodes = set()
    for table in (pathlib.Path("/proc/net/tcp"), pathlib.Path("/proc/net/tcp6")):
        try:
            lines = table.read_text(encoding="ascii").splitlines()[1:]
        except OSError:
            reject("listener_inventory_unavailable")
        for line in lines:
            fields = line.split()
            if len(fields) < 10 or fields[3] != "0A":
                continue
            try:
                local_port = int(fields[1].rsplit(":", 1)[1], 16)
            except (IndexError, ValueError):
                reject("listener_inventory_malformed")
            if local_port == port:
                inodes.add(fields[9])
    return inodes


def listener_pids(port):
    inodes = listener_inodes(port)
    if not inodes:
        return set()
    owners = set()
    found = set()
    for entry in pathlib.Path("/proc").iterdir():
        if not entry.name.isdigit():
            continue
        try:
            status = (entry / "status").read_text(encoding="ascii")
            uid_line = next(line for line in status.splitlines() if line.startswith("Uid:"))
            if int(uid_line.split()[1]) != os.geteuid():
                continue
            descriptors = list((entry / "fd").iterdir())
        except (OSError, StopIteration, ValueError):
            continue
        for descriptor in descriptors:
            try:
                target = os.readlink(descriptor)
            except OSError:
                continue
            match = re.fullmatch(r"socket:\[(\d+)\]", target)
            if match and match.group(1) in inodes:
                owners.add(int(entry.name))
                found.add(match.group(1))
    if found != inodes:
        reject("listener_owner_unavailable")
    return owners


def stop_exact_process(pid, identity, gateway, addr, runtime_hash):
    if process_identity(pid, gateway, addr, runtime_hash) != identity:
        reject("process_identity_changed")
    try:
        pidfd = os.pidfd_open(pid, 0)
    except (AttributeError, OSError):
        reject("exact_process_control_unavailable")
    try:
        if process_identity(pid, gateway, addr, runtime_hash) != identity:
            reject("process_identity_changed")
        signal.pidfd_send_signal(pidfd, signal.SIGTERM)
        poller = select.poll()
        poller.register(pidfd, select.POLLIN)
        if poller.poll(5000):
            return
        if process_identity(pid, gateway, addr, runtime_hash) != identity:
            reject("process_identity_changed")
        signal.pidfd_send_signal(pidfd, signal.SIGKILL)
        if not poller.poll(5000):
            reject("process_shutdown_timeout")
    except (AttributeError, OSError):
        reject("exact_process_control_failed")
    finally:
        os.close(pidfd)


def check_no_existing_rollback(backup_root):
    try:
        metadata = backup_root.lstat()
    except FileNotFoundError:
        return
    except OSError:
        reject("rollback_inventory_unavailable")
    if (
        not stat.S_ISDIR(metadata.st_mode)
        or stat.S_ISLNK(metadata.st_mode)
        or metadata.st_uid != os.geteuid()
        or metadata.st_mode & 0o077
    ):
        reject("rollback_inventory_unsafe")
    try:
        entries = list(backup_root.iterdir())
    except OSError:
        reject("rollback_inventory_unavailable")
    for entry in entries:
        if not entry.name.startswith("principal-root-upgrade-"):
            continue
        try:
            value = entry.lstat()
        except OSError:
            reject("rollback_reconciliation_required")
        if not stat.S_ISDIR(value.st_mode) or stat.S_ISLNK(value.st_mode):
            reject("rollback_reconciliation_required")
        reject("rollback_reconciliation_required")


def rollback_size(root):
    total = 0
    pending = [root]
    while pending:
        directory = pending.pop()
        try:
            metadata = directory.lstat()
        except OSError:
            reject("rollback_unsafe")
        if (
            not stat.S_ISDIR(metadata.st_mode)
            or stat.S_ISLNK(metadata.st_mode)
            or metadata.st_uid != os.geteuid()
            or metadata.st_mode & 0o077
        ):
            reject("rollback_unsafe")
        try:
            entries = list(os.scandir(directory))
        except OSError:
            reject("rollback_unsafe")
        for entry in entries:
            try:
                value = entry.stat(follow_symlinks=False)
            except OSError:
                reject("rollback_unsafe")
            if value.st_uid != os.geteuid() or value.st_mode & 0o077:
                reject("rollback_unsafe")
            path = pathlib.Path(entry.path)
            if stat.S_ISDIR(value.st_mode) and not entry.is_symlink():
                pending.append(path)
            elif stat.S_ISREG(value.st_mode) and not entry.is_symlink() and value.st_nlink == 1:
                total += value.st_size
                if total > MAX_ROLLBACK:
                    reject("rollback_unbounded")
            else:
                reject("rollback_unsafe")
    return total


def rollback_receipt(root, size):
    return {
        "relative_identity": f"backups/{root.name}",
        "size_bytes": size,
        "reason": "principal_root_upgrade",
        "cleanup_condition": "remove_after_verified_restart_and_explicit_operator_reconciliation",
    }


def validate_receipt_destination(path):
    ensure_owner_directory(path.parent, create=True)
    try:
        metadata = path.lstat()
    except FileNotFoundError:
        return
    except OSError:
        reject("restart_receipt_unavailable")
    if (
        not stat.S_ISREG(metadata.st_mode)
        or stat.S_ISLNK(metadata.st_mode)
        or metadata.st_uid != os.geteuid()
        or metadata.st_mode & 0o077
        or metadata.st_nlink != 1
        or metadata.st_size > MAX_RESTART_RECEIPT
    ):
        reject("restart_receipt_unsafe")


def invalidate_receipt(path):
    try:
        path.unlink()
    except FileNotFoundError:
        return
    except OSError:
        reject("restart_receipt_invalidation_failed")
    parent_fd = os.open(path.parent, os.O_RDONLY | getattr(os, "O_DIRECTORY", 0))
    try:
        os.fsync(parent_fd)
    finally:
        os.close(parent_fd)


def create_log(path):
    ensure_owner_directory(path.parent, create=True)
    try:
        return os.open(
            path,
            os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_NOFOLLOW", 0),
            0o600,
        )
    except OSError:
        reject("log_creation_failed")


def settle_spawned_child(pid, pidfd):
    poller = select.poll()
    poller.register(pidfd, select.POLLIN)
    try:
        signal.pidfd_send_signal(pidfd, signal.SIGKILL)
    except (AttributeError, OSError):
        if not poller.poll(0):
            reject("exact_process_control_failed")
    if not poller.poll(5000):
        reject("process_shutdown_timeout")
    try:
        os.waitpid(pid, 0)
    except ChildProcessError:
        pass


def start_gateway(gateway, addr, runtime_hash, data_dir, home, xdg_data_home, log_path):
    log_fd = create_log(log_path)
    try:
        ready_read, ready_write = os.pipe()
    except OSError:
        os.close(log_fd)
        reject("gateway_start_failed")
    try:
        pid = os.fork()
    except OSError:
        os.close(log_fd)
        os.close(ready_read)
        os.close(ready_write)
        reject("gateway_start_failed")
    if pid == 0:
        try:
            os.close(ready_write)
            if os.read(ready_read, 1) != b"1":
                os._exit(126)
            os.close(ready_read)
            os.setsid()
            os.chdir(data_dir)
            devnull = os.open(os.devnull, os.O_RDONLY)
            os.dup2(devnull, 0)
            os.dup2(log_fd, 1)
            os.dup2(log_fd, 2)
            os.close(devnull)
            os.close(log_fd)
            environment = os.environ.copy()
            environment["HOME"] = str(home)
            environment["XDG_DATA_HOME"] = str(xdg_data_home)
            os.execve(
                gateway,
                [str(gateway), "gateway", "--addr", addr],
                environment,
            )
        except BaseException:
            os._exit(127)
    os.close(log_fd)
    os.close(ready_read)
    try:
        pidfd = os.pidfd_open(pid, 0)
    except (AttributeError, OSError):
        os.close(ready_write)
        try:
            os.waitpid(pid, 0)
        except ChildProcessError:
            pass
        reject("exact_process_control_unavailable")
    try:
        try:
            os.write(ready_write, b"1")
        except OSError:
            settle_spawned_child(pid, pidfd)
            reject("gateway_start_failed")
        finally:
            os.close(ready_write)
        deadline = time.monotonic() + 3
        while time.monotonic() < deadline:
            identity = process_identity(pid, gateway, addr, runtime_hash)
            if identity is not None:
                return pid, identity
            if not process_running(pid):
                break
            time.sleep(0.02)
        settle_spawned_child(pid, pidfd)
        reject("gateway_start_identity_failed")
    finally:
        os.close(pidfd)


def run_upgrade(gateway, data_dir, home, xdg_data_home, rollback, log_path):
    log_fd = create_log(log_path)
    environment = os.environ.copy()
    environment["HOME"] = str(home)
    environment["XDG_DATA_HOME"] = str(xdg_data_home)
    try:
        result = subprocess.run(
            [
                str(gateway),
                "principal-root-upgrade",
                "--data-dir",
                str(data_dir),
                "--backup-dir",
                str(rollback),
            ],
            cwd=data_dir,
            env=environment,
            stdin=subprocess.DEVNULL,
            stdout=log_fd,
            stderr=subprocess.STDOUT,
            timeout=300,
            check=False,
        )
    except (OSError, subprocess.SubprocessError):
        reject("principal_root_upgrade_failed")
    finally:
        os.close(log_fd)
    if result.returncode != 0:
        reject("principal_root_upgrade_failed")


def fetch_page(url):
    current = url
    for redirect_count in range(2):
        parsed = urllib.parse.urlsplit(current)
        if parsed.scheme != "http" or not parsed.hostname or parsed.port is None:
            reject("http_probe_url_invalid")
        connection = http.client.HTTPConnection(parsed.hostname, parsed.port, timeout=2)
        try:
            connection.request("GET", parsed.path or "/")
            response = connection.getresponse()
            if response.status in {301, 302, 303, 307, 308}:
                location = response.getheader("Location")
                response.read(MAX_PAGE + 1)
                if redirect_count == 1 or not location:
                    reject("http_redirect_rejected")
                target = urllib.parse.urljoin(current, location)
                target_parts = urllib.parse.urlsplit(target)
                if (
                    target_parts.scheme != "http"
                    or target_parts.hostname != parsed.hostname
                    or target_parts.port != parsed.port
                ):
                    reject("http_redirect_rejected")
                current = target
                continue
            body = response.read(MAX_PAGE + 1)
            if len(body) > MAX_PAGE:
                reject("http_response_unbounded")
            return response.status, body
        except (OSError, http.client.HTTPException):
            return 0, b""
        finally:
            connection.close()
    reject("http_redirect_rejected")


def wait_for_ready(pid, identity, gateway, addr, runtime_hash, urls, wait_seconds):
    deadline = time.monotonic() + wait_seconds
    while time.monotonic() < deadline:
        if process_identity(pid, gateway, addr, runtime_hash) != identity:
            reject("gateway_exited_before_readiness")
        results = [fetch_page(url) for url in urls]
        if all(status == 200 for status, _ in results):
            return results
        time.sleep(0.1)
    reject("gateway_readiness_timeout")


def receipt_payload(context, *, ok, dry_run, error=None, process=None, proof=None, rollback=None):
    value = {
        "schema": RESTART_SCHEMA,
        "ok": ok,
        "dry_run": dry_run,
        "generated_at": utc_now(),
        "repo": str(context["source_root"]),
        "home": str(context["home"]),
        "xdg_data_home": str(context["xdg_data_home"]),
        "data_dir": str(context["data_dir"]),
        "addr": context["addr"],
        "home_url": context["home_url"],
        "services_url": context["services_url"],
        "gateway_bin": str(context["gateway"]),
        "gateway_bin_sha256": context["installed_runtime_sha256"],
        "installation_receipt_sha256": context["installation_receipt_sha256"],
        "installed_runtime_sha256": context["installed_runtime_sha256"],
        "installed_components_sha256": context["installed_components_sha256"],
        "source_home_capsule_metadata_receipt_sha256": context[
            "source_home_capsule_metadata_receipt_sha256"
        ],
        "gateway_log": str(context["gateway_log"]),
        "pid_file": str(context["pid_file"]),
        **context["asset_hashes"],
    }
    if error:
        value["error"] = error
    if process:
        value["gateway_pid"] = process["pid"]
    if proof:
        value.update(proof)
    if rollback:
        value["principal_root_rollback"] = rollback
    payload = (json.dumps(value, sort_keys=True, indent=2) + "\n").encode()
    if len(payload) > MAX_RESTART_RECEIPT:
        reject("restart_receipt_unbounded")
    return value, payload


def parse_arguments(arguments):
    parser = argparse.ArgumentParser(
        description="Restart the stable Linux source-home Runtime with exact installed proof."
    )
    parser.add_argument("--home", default=os.environ.get("LINUX_SOURCE_HOME", os.environ.get("HOME", "")))
    parser.add_argument("--xdg-data-home", default=os.environ.get("XDG_DATA_HOME", ""))
    parser.add_argument("--addr", default=os.environ.get("LINUX_GATEWAY_ADDR", "localhost:8090"))
    parser.add_argument("--wait-seconds", type=int, default=40)
    parser.add_argument("--dry-run", action="store_true")
    values = parser.parse_args(arguments)
    if not values.home:
        reject("home_required")
    if not re.fullmatch(r"[^\s:]+:[0-9]+", values.addr):
        reject("invalid_address")
    host, port_text = values.addr.rsplit(":", 1)
    port = int(port_text)
    if port < 1 or port > 65535:
        reject("invalid_address")
    if values.wait_seconds < 1 or values.wait_seconds > 300:
        reject("invalid_wait_seconds")
    home = pathlib.Path(values.home).expanduser()
    xdg = pathlib.Path(values.xdg_data_home).expanduser() if values.xdg_data_home else home / ".local/share"
    return values, home, xdg, host, port


def build_context(source_root, values, home, xdg, host, port):
    data_dir = xdg / "elastos"
    gateway = data_dir / "bin" / "elastos"
    data_dir, gateway, source_root = validate_stable_install(data_dir, gateway, source_root)
    installation = validate_installation_receipt(
        data_dir / "receipts" / "source-home-installation.json",
        data_dir,
        gateway,
        source_root,
    )
    asset_hashes = validate_asset_parity(source_root, data_dir)
    probe_host = "localhost" if host in {"0.0.0.0", "*"} else host
    stamp = datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    return {
        "source_root": source_root,
        "home": home,
        "xdg_data_home": xdg,
        "data_dir": data_dir,
        "gateway": gateway,
        "addr": values.addr,
        "port": port,
        "home_url": f"http://{probe_host}:{port}/apps/home/",
        "services_url": f"http://{probe_host}:{port}/apps/services/",
        "gateway_log": data_dir / "logs" / f"gateway-{stamp}-{os.getpid()}.log",
        "upgrade_log": data_dir / "logs" / f"principal-root-upgrade-{stamp}-{os.getpid()}.json",
        "pid_file": data_dir / "run" / "gateway.pid",
        "restart_receipt": data_dir / "receipts" / "linux-source-home-restart.json",
        "backup_root": data_dir / "backups",
        "rollback": data_dir / "backups" / f"principal-root-upgrade-{stamp}-{os.getpid()}",
        "asset_hashes": asset_hashes,
        **installation,
    }


def active_restart(context, wait_seconds):
    if platform.system() != "Linux":
        reject("linux_active_restart_required")
    pid_file = context["pid_file"]
    receipt_path = context["restart_receipt"]
    gateway = context["gateway"]
    runtime_hash = context["installed_runtime_sha256"]
    addr = context["addr"]
    ensure_owner_directory(context["data_dir"] / "logs", create=True)
    ensure_owner_directory(context["data_dir"] / "run", create=True)
    ensure_owner_directory(context["backup_root"], create=True)
    validate_pid_destination(pid_file)
    validate_receipt_destination(receipt_path)
    check_no_existing_rollback(context["backup_root"])
    old = read_pid_file(pid_file, addr)
    if old is not None:
        if process_running(old["pid"]):
            identity = process_identity(
                old["pid"], gateway, addr, old["runtime_sha256"]
            )
            if identity != old["start_identity"]:
                reject("process_identity_changed")
        else:
            remove_matching_pid_file(pid_file, old)
            old = None
    listeners = listener_pids(context["port"])
    if len(listeners) > 1:
        reject("multiple_listeners")
    if listeners:
        listener = next(iter(listeners))
        if old is None or listener != old["pid"]:
            reject("unrelated_listener")
        if process_identity(
            listener, gateway, addr, old["runtime_sha256"]
        ) != old["start_identity"]:
            reject("process_identity_changed")
    elif old is not None:
        reject("pid_listener_mismatch")

    invalidate_receipt(receipt_path)
    effects_started = True
    new = None
    rollback = None
    try:
        if old is not None:
            stop_exact_process(
                old["pid"],
                old["start_identity"],
                gateway,
                addr,
                old["runtime_sha256"],
            )
            remove_matching_pid_file(pid_file, old)
        if listener_pids(context["port"]):
            reject("listener_did_not_settle")
        run_upgrade(
            gateway,
            context["data_dir"],
            context["home"],
            context["xdg_data_home"],
            context["rollback"],
            context["upgrade_log"],
        )
        rollback = rollback_receipt(context["rollback"], rollback_size(context["rollback"]))
        pid, identity = start_gateway(
            gateway,
            addr,
            runtime_hash,
            context["data_dir"],
            context["home"],
            context["xdg_data_home"],
            context["gateway_log"],
        )
        new = pid_payload(pid, identity, runtime_hash, addr)
        new["_bytes"] = pid_file_bytes(new)
        write_pid_file(pid_file, new["_bytes"])
        if read_regular(pid_file, MAX_PID_FILE, owner_only=True) != new["_bytes"]:
            reject("pid_file_changed")
        if process_identity(pid, gateway, addr, runtime_hash) != identity:
            reject("process_identity_changed")
        results = wait_for_ready(
            pid,
            identity,
            gateway,
            addr,
            runtime_hash,
            [context["home_url"], context["services_url"]],
            wait_seconds,
        )
        listeners = listener_pids(context["port"])
        if listeners != {pid}:
            reject("exclusive_listener_failed")
        proof = {}
        for name, (status, body) in zip(("home", "services"), results):
            served_hash = sha256_bytes(body)
            if (
                status != 200
                or served_hash != context["asset_hashes"][f"{name}_source_index_sha256"]
                or served_hash != context["asset_hashes"][f"{name}_installed_index_sha256"]
            ):
                reject(f"{name}_readiness_parity_failed")
            proof[f"{name}_http_code"] = status
            proof[f"{name}_served_index_sha256"] = served_hash
        value, payload = receipt_payload(
            context,
            ok=True,
            dry_run=False,
            process=new,
            proof=proof,
            rollback=rollback,
        )
        atomic_write(receipt_path, payload, MAX_RESTART_RECEIPT)
        return value
    except RestartError as error:
        if new is not None:
            identity = process_identity(new["pid"], gateway, addr, runtime_hash)
            if identity == new["start_identity"]:
                try:
                    stop_exact_process(
                        new["pid"],
                        new["start_identity"],
                        gateway,
                        addr,
                        runtime_hash,
                    )
                    try:
                        os.waitpid(new["pid"], 0)
                    except ChildProcessError:
                        pass
                except RestartError:
                    pass
            try:
                remove_matching_pid_file(pid_file, new)
            except RestartError:
                pass
        if rollback is None and context["rollback"].exists() and not context["rollback"].is_symlink():
            try:
                rollback = rollback_receipt(context["rollback"], rollback_size(context["rollback"]))
            except RestartError:
                rollback = None
        if effects_started:
            try:
                _, payload = receipt_payload(
                    context,
                    ok=False,
                    dry_run=False,
                    error=str(error),
                    process=new,
                    rollback=rollback,
                )
                atomic_write(receipt_path, payload, MAX_RESTART_RECEIPT)
            except RestartError:
                pass
        raise


def main():
    source_root = pathlib.Path(sys.argv[1])
    values, home, xdg, host, port = parse_arguments(sys.argv[2:])
    context = build_context(source_root, values, home, xdg, host, port)
    if values.dry_run:
        value, _ = receipt_payload(context, ok=True, dry_run=True)
    else:
        value = active_restart(context, values.wait_seconds)
    json.dump(value, sys.stdout, sort_keys=True, indent=2)
    sys.stdout.write("\n")


try:
    main()
except RestartError as error:
    print(f"linux-source-home-restart: {error}", file=sys.stderr)
    raise SystemExit(1)
PY
