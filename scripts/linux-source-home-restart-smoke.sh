#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

python3 - "$repo_root" <<'PY'
import hashlib
import json
import os
import pathlib
import platform
import shutil
import signal
import socket
import stat
import subprocess
import sys
import tempfile
import time


ROOT = pathlib.Path(sys.argv[1])
RESTART = ROOT / "scripts/linux-source-home-restart.sh"
INSTALL_SCHEMA = "elastos.source-home.installation-receipt/v1"
RESTART_SCHEMA = "elastos.linux-source-home-restart/v1"
PID_SCHEMA = "elastos.linux-source-home-gateway-pid/v1"
tracked_pids = set()


def sha256(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def git_value(revision):
    return subprocess.run(
        ["git", "-C", str(ROOT), "rev-parse", revision],
        check=True,
        stdout=subprocess.PIPE,
        text=True,
    ).stdout.strip()


SOURCE_COMMIT = git_value("HEAD")
SOURCE_TREE = git_value("HEAD^{tree}")


def expected_platform():
    machine = platform.machine().lower()
    if machine in {"x86_64", "amd64"}:
        return "linux-amd64"
    if machine in {"aarch64", "arm64"}:
        return "linux-arm64"
    raise AssertionError(f"unsupported fixture architecture: {machine}")


def owner_dir(path):
    path.mkdir(parents=True, exist_ok=True)
    path.chmod(0o700)


def write(path, value, mode=0o600):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(value)
    path.chmod(mode)


def free_port():
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def process_alive(pid):
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    if platform.system() == "Linux":
        try:
            value = (pathlib.Path("/proc") / str(pid) / "stat").read_text()
            close = value.rfind(")")
            return close >= 0 and value[close + 2 :].split()[0] != "Z"
        except (OSError, IndexError):
            return False
    return True


def stop_pid(pid):
    if not pid or not process_alive(pid):
        return
    os.kill(pid, signal.SIGTERM)
    for _ in range(100):
        if not process_alive(pid):
            return
        time.sleep(0.02)
    if process_alive(pid):
        os.kill(pid, signal.SIGKILL)
    for _ in range(100):
        if not process_alive(pid):
            return
        time.sleep(0.02)


def wait_listener(port, expected=True):
    for _ in range(100):
        with socket.socket() as sock:
            sock.settimeout(0.05)
            listening = sock.connect_ex(("127.0.0.1", port)) == 0
        if listening == expected:
            return
        time.sleep(0.05)
    raise AssertionError(f"listener state did not become {expected}")


def process_identity(pid, gateway, addr, runtime_hash):
    proc = pathlib.Path("/proc") / str(pid)
    value = (proc / "stat").read_text(encoding="ascii")
    close = value.rfind(")")
    fields = value[close + 2 :].split()
    start_time = fields[19]
    uid_line = next(
        line for line in (proc / "status").read_text(encoding="ascii").splitlines()
        if line.startswith("Uid:")
    )
    uid = int(uid_line.split()[1])
    executable_link = os.readlink(proc / "exe")
    cmdline = (proc / "cmdline").read_bytes()
    expected = os.fsencode(str(gateway)) + b"\0gateway\0--addr\0" + os.fsencode(addr) + b"\0"
    stable = str(gateway.resolve())
    if (
        uid != os.geteuid()
        or executable_link not in {stable, f"{stable} (deleted)"}
        or cmdline != expected
    ):
        raise AssertionError("fixture gateway process identity mismatch")
    with open(proc / "exe", "rb") as executable:
        executable_hash = hashlib.sha256(executable.read()).hexdigest()
    if executable_hash != runtime_hash:
        raise AssertionError("fixture gateway executable hash mismatch")
    material = (
        str(uid).encode("ascii")
        + b"\0"
        + os.fsencode(stable)
        + b"\0"
        + start_time.encode("ascii")
        + b"\0"
        + runtime_hash.encode("ascii")
        + b"\0"
        + cmdline
    )
    return hashlib.sha256(material).hexdigest()


def static_contract():
    source = RESTART.read_text(encoding="utf-8")
    required = (
        'gateway = data_dir / "bin" / "elastos"',
        "elastos.source-home.installation-receipt/v1",
        "elastos.linux-source-home-gateway-pid/v1",
        "GIT_OPTIONAL_LOCKS",
        'pathlib.Path("/proc")',
        'cmdline != expected',
        'deleted_executable = f"{stable_executable} (deleted)"',
        'descriptor = os.open(proc / "exe", os.O_RDONLY)',
        "pidfd_send_signal",
        "listener_pids",
        "rollback_size",
        "principal_root_rollback",
        "installation_receipt_sha256",
        'proof[f"{name}_served_index_sha256"]',
    )
    missing = [value for value in required if value not in source]
    if missing:
        raise AssertionError(f"Linux restart ownership contract is incomplete: {missing}")
    forbidden = (
        "pkill",
        "killall",
        "kill_port_listeners",
        "stop_pid_file_process",
        "--gateway-bin",
        "--pid-file",
        "target/release/elastos",
        "lsof",
        "fuser",
    )
    present = [value for value in forbidden if value in source]
    if present:
        raise AssertionError(f"Linux restart retained broad or caller-selected ownership: {present}")
    if source.count('gateway = data_dir / "bin" / "elastos"') != 1:
        raise AssertionError("Linux restart must select one stable Runtime")
    if source.count("def start_gateway(") != 1:
        raise AssertionError("Linux restart must have one detached launcher")
    if source.count("RESTART_SCHEMA =") != 1 or source.count("PID_SCHEMA =") != 1:
        raise AssertionError("Linux restart must have one receipt and PID schema")


class Fixture:
    def __init__(self, root, name):
        self.root = root / name
        self.home = self.root / "source home"
        self.xdg = self.root / "xdg data"
        self.data = self.xdg / "elastos"
        self.runtime = self.data / "bin/elastos"
        self.components = self.data / "components.json"
        self.capsule_receipt = self.data / "receipts/source-home-capsules.json"
        self.install_receipt = self.data / "receipts/source-home-installation.json"
        self.restart_receipt = self.data / "receipts/linux-source-home-restart.json"
        self.pid_file = self.data / "run/gateway.pid"
        self.port = free_port()
        self.addr = f"127.0.0.1:{self.port}"
        self.processes = []
        self._create()

    def _create(self):
        for path in (
            self.root,
            self.home,
            self.xdg,
            self.data,
            self.data / "bin",
            self.data / "receipts",
            self.data / "capsules/home/browser",
            self.data / "capsules/services/browser",
        ):
            owner_dir(path)
        shutil.copyfile(pathlib.Path(sys.executable).resolve(), self.runtime)
        self.runtime.chmod(0o700)
        write(self.components, b'{"profile":"source-home"}\n')
        write(
            self.capsule_receipt,
            b'{"schema":"elastos.source-home.managed-capsules/v1","capsules":[]}\n',
        )
        for name in ("home", "services"):
            shutil.copyfile(
                ROOT / f"capsules/{name}/browser/index.html",
                self.data / f"capsules/{name}/browser/index.html",
            )
            (self.data / f"capsules/{name}/browser/index.html").chmod(0o600)
        write(self.data / "gateway", self._gateway_source())
        write(self.data / "principal-root-upgrade", self._upgrade_source())
        self.write_install_receipt()

    def _gateway_source(self):
        return b'''import http.server
import os
import pathlib
import signal
import sys

if os.environ.get("ELASTOS_SMOKE_FAIL_START") == "1":
    raise SystemExit(73)
addr = sys.argv[sys.argv.index("--addr") + 1]
host, port_text = addr.rsplit(":", 1)
data = pathlib.Path(os.environ["XDG_DATA_HOME"]) / "elastos"
home = (data / "capsules/home/browser/index.html").read_bytes()
services = (data / "capsules/services/browser/index.html").read_bytes()
if os.environ.get("ELASTOS_SMOKE_BAD_HOME") == "1":
    home = b"wrong Home\n"

class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        values = {
            "/apps/home/": home,
            "/apps/services/": services,
        }
        body = values.get(self.path)
        if body is None:
            self.send_error(404)
            return
        self.send_response(200)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, _format, *_args):
        return

server = http.server.ThreadingHTTPServer((host, int(port_text)), Handler)
signal.signal(signal.SIGTERM, lambda _signal, _frame: (_ for _ in ()).throw(SystemExit(0)))
try:
    server.serve_forever()
finally:
    server.server_close()
'''

    def _upgrade_source(self):
        return b'''import json
import os
import pathlib
import sys

if os.environ.get("ELASTOS_SMOKE_FAIL_UPGRADE") == "1":
    raise SystemExit(41)
backup = pathlib.Path(sys.argv[sys.argv.index("--backup-dir") + 1])
backup.mkdir(mode=0o700, parents=True, exist_ok=False)
proof = backup / "rollback.json"
proof.write_text('{"schema":"fixture.rollback/v1"}\\n', encoding="utf-8")
proof.chmod(0o600)
unsafe = os.environ.get("ELASTOS_SMOKE_UNSAFE_ROLLBACK")
if unsafe == "fifo":
    os.mkfifo(backup / "unsafe-entry", mode=0o600)
elif unsafe == "hardlink":
    os.link(proof, backup / "unsafe-hardlink")
print(json.dumps({"schema":"elastos.principal-root-upgrade/v1","ok":True}))
'''

    def write_install_receipt(self, *, commit=SOURCE_COMMIT, tree=SOURCE_TREE, clean=True):
        runtime_hash = "sha256:" + sha256(self.runtime)
        value = {
            "schema": INSTALL_SCHEMA,
            "source": {"commit": commit, "tree": tree, "clean": clean},
            "runtime": {
                "built_sha256": runtime_hash,
                "installed_sha256": runtime_hash,
                "parity": True,
            },
            "components_sha256": "sha256:" + sha256(self.components),
            "source_home_capsule_metadata_receipt_sha256": "sha256:" + sha256(self.capsule_receipt),
            "platform": expected_platform(),
            "installation_time_utc": "2026-08-28T12:00:00Z",
        }
        write(self.install_receipt, (json.dumps(value, indent=2) + "\n").encode())

    def replace_runtime(self):
        previous = self.runtime.read_bytes()
        replacement = self.runtime.with_name(f".{self.runtime.name}.replacement")
        write(replacement, previous + b"\n# fixture atomic replacement\n", 0o700)
        os.replace(replacement, self.runtime)
        self.write_install_receipt()

    def environment(self, **values):
        environment = os.environ.copy()
        environment.update({name: str(value) for name, value in values.items()})
        return environment

    def command(self, *extra):
        return [
            str(RESTART),
            "--home",
            str(self.home),
            "--xdg-data-home",
            str(self.xdg),
            "--addr",
            self.addr,
            "--wait-seconds",
            "3",
            *extra,
        ]

    def run(self, *extra, env=None, ok=True):
        result = subprocess.run(
            self.command(*extra),
            env=self.environment() if env is None else env,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            check=False,
            timeout=20,
        )
        if ok and result.returncode != 0:
            raise AssertionError(
                f"restart failed: status={result.returncode} stdout={result.stdout!r} stderr={result.stderr!r}"
            )
        if not ok and result.returncode == 0:
            raise AssertionError("restart accepted a rejected fixture")
        return result

    def start_gateway(self):
        process = subprocess.Popen(
            [str(self.runtime), "gateway", "--addr", self.addr],
            cwd=self.data,
            env={**os.environ, "HOME": str(self.home), "XDG_DATA_HOME": str(self.xdg)},
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        self.processes.append(process.pid)
        tracked_pids.add(process.pid)
        wait_listener(self.port)
        return process

    def write_pid(self, pid, *, identity=None, runtime_hash=None, mode=0o600):
        owner_dir(self.pid_file.parent)
        recorded_runtime_hash = runtime_hash or sha256(self.runtime)
        value = {
            "schema": PID_SCHEMA,
            "pid": pid,
            "start_identity": identity
            or process_identity(pid, self.runtime, self.addr, recorded_runtime_hash),
            "runtime_sha256": recorded_runtime_hash,
            "addr": self.addr,
        }
        write(self.pid_file, (json.dumps(value, separators=(",", ":")) + "\n").encode(), mode)

    def cleanup(self):
        if self.pid_file.is_file() and not self.pid_file.is_symlink():
            try:
                value = json.loads(self.pid_file.read_text(encoding="utf-8"))
                if isinstance(value.get("pid"), int):
                    self.processes.append(value["pid"])
                    tracked_pids.add(value["pid"])
            except (OSError, json.JSONDecodeError):
                pass
        for pid in set(self.processes):
            stop_pid(pid)
            tracked_pids.discard(pid)


def snapshot(root):
    result = {}
    for path in sorted(root.rglob("*")):
        name = path.relative_to(root).as_posix()
        if path.is_symlink():
            result[name] = ("link", os.readlink(path))
        elif path.is_file():
            result[name] = ("file", sha256(path), stat.S_IMODE(path.stat().st_mode))
        elif path.is_dir():
            result[name] = ("dir", stat.S_IMODE(path.stat().st_mode))
    return result


def assert_rejected(fixture, needle, *, env=None, extra=()):
    result = fixture.run(*extra, env=env, ok=False)
    if needle not in result.stderr:
        raise AssertionError(f"missing rejection {needle!r}: {result.stderr!r}")
    for private in (str(fixture.home), str(fixture.data), str(fixture.runtime)):
        if private in result.stderr:
            raise AssertionError("restart error leaked a private fixture path")
    return result


def dry_run_proof(base):
    fixture = Fixture(base, "dry-run")
    before = snapshot(fixture.root)
    result = fixture.run("--dry-run")
    after = snapshot(fixture.root)
    if before != after:
        raise AssertionError("dry run changed the stable fixture")
    value = json.loads(result.stdout)
    if value.get("schema") != RESTART_SCHEMA or value.get("ok") is not True or value.get("dry_run") is not True:
        raise AssertionError("dry-run receipt status mismatch")
    if value.get("gateway_bin") != str(fixture.runtime.resolve()):
        raise AssertionError("dry run did not select the stable Runtime")
    if value.get("installed_runtime_sha256") != sha256(fixture.runtime):
        raise AssertionError("dry run did not bind the installed Runtime hash")
    if any("rollback" in name for name in value):
        raise AssertionError("dry run claimed an actual or planned rollback")
    if fixture.restart_receipt.exists() or (fixture.data / "run").exists() or (fixture.data / "logs").exists():
        raise AssertionError("dry run created restart state")

    original_components = fixture.components.read_bytes()
    fixture.components.write_bytes(b"changed\n")
    assert_rejected(fixture, "installation_receipt_components_hash", extra=("--dry-run",))
    fixture.components.write_bytes(original_components)
    fixture.components.chmod(0o600)

    original_receipt = fixture.install_receipt.read_bytes()
    fixture.write_install_receipt(commit="0" * 40)
    assert_rejected(fixture, "installation_receipt_source_identity", extra=("--dry-run",))
    fixture.install_receipt.write_bytes(original_receipt)
    fixture.install_receipt.chmod(0o600)

    source_dirt = ROOT / f".linux-source-home-restart-smoke-dirt-{os.getpid()}"
    try:
        source_dirt.write_text("untracked source dirt\n", encoding="utf-8")
        assert_rejected(fixture, "source_dirty", extra=("--dry-run",))
    finally:
        source_dirt.unlink(missing_ok=True)

    tracked_source = ROOT / "PRINCIPLES.md"
    tracked_bytes = tracked_source.read_bytes()
    tracked_mode = stat.S_IMODE(tracked_source.stat().st_mode)
    try:
        tracked_source.write_bytes(tracked_bytes + b"\n")
        assert_rejected(fixture, "source_dirty", extra=("--dry-run",))
    finally:
        tracked_source.write_bytes(tracked_bytes)
        tracked_source.chmod(tracked_mode)
    return fixture


def linux_active_proof(base):
    dead_stale = Fixture(base, "dead-stale-pid")
    dead_stale.write_pid(
        99999999,
        identity="1" * 64,
        runtime_hash="2" * 64,
    )
    dead_success = dead_stale.run()
    dead_receipt = json.loads(dead_success.stdout)
    dead_pid = dead_receipt.get("gateway_pid")
    if not isinstance(dead_pid, int) or not process_alive(dead_pid):
        raise AssertionError("dead prior PID record did not admit one new gateway")
    dead_stale.processes.append(dead_pid)
    tracked_pids.add(dead_pid)
    dead_pid_value = json.loads(dead_stale.pid_file.read_text(encoding="utf-8"))
    if dead_pid_value.get("runtime_sha256") != sha256(dead_stale.runtime):
        raise AssertionError("dead prior PID replacement did not publish the new Runtime hash")

    main = Fixture(base, "active-main")
    old = main.start_gateway()
    main.write_pid(old.pid)

    source_dirt = ROOT / f".linux-source-home-restart-active-dirt-{os.getpid()}"
    try:
        source_dirt.write_text("untracked source dirt\n", encoding="utf-8")
        assert_rejected(main, "source_dirty")
        if not process_alive(old.pid):
            raise AssertionError("source dirt stopped the exact old gateway")
    finally:
        source_dirt.unlink(missing_ok=True)

    original_components = main.components.read_bytes()
    main.components.write_bytes(b"changed\n")
    assert_rejected(main, "installation_receipt_components_hash")
    if not process_alive(old.pid):
        raise AssertionError("artifact mismatch stopped the exact old gateway")
    main.components.write_bytes(original_components)
    main.components.chmod(0o600)

    main.pid_file.write_text("not-json\n", encoding="utf-8")
    main.pid_file.chmod(0o600)
    assert_rejected(main, "pid_file_malformed")
    if not process_alive(old.pid):
        raise AssertionError("malformed PID file stopped the old gateway")

    main.write_pid(old.pid, identity="0" * 64)
    assert_rejected(main, "process_identity_changed")
    if not process_alive(old.pid):
        raise AssertionError("identity mismatch stopped the old gateway")

    main.pid_file.unlink()
    unsafe_target = main.root / "unsafe-pid-target"
    write(unsafe_target, b"preserve\n")
    main.pid_file.symlink_to(unsafe_target)
    assert_rejected(main, "pid_file_unsafe")
    if unsafe_target.read_bytes() != b"preserve\n" or not process_alive(old.pid):
        raise AssertionError("unsafe PID rejection changed unrelated state")
    main.pid_file.unlink()

    unrelated = Fixture(base, "unrelated")
    unrelated_process = subprocess.Popen(
        [sys.executable, "-m", "http.server", str(unrelated.port), "--bind", "127.0.0.1"],
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    unrelated.processes.append(unrelated_process.pid)
    tracked_pids.add(unrelated_process.pid)
    wait_listener(unrelated.port)
    assert_rejected(unrelated, "unrelated_listener")
    if not process_alive(unrelated_process.pid):
        raise AssertionError("restart disturbed an unrelated listener")

    previous_runtime_hash = sha256(main.runtime)
    main.write_pid(old.pid, runtime_hash=previous_runtime_hash)
    main.replace_runtime()
    if sha256(main.runtime) == previous_runtime_hash:
        raise AssertionError("fixture Runtime replacement did not change its hash")
    success = main.run()
    old.wait(timeout=5)
    receipt = json.loads(main.restart_receipt.read_text(encoding="utf-8"))
    if receipt != json.loads(success.stdout) or receipt.get("ok") is not True:
        raise AssertionError("successful restart receipt mismatch")
    new_pid = receipt.get("gateway_pid")
    if not isinstance(new_pid, int) or new_pid == old.pid or not process_alive(new_pid):
        raise AssertionError("matching gateway was not replaced")
    main.processes.append(new_pid)
    tracked_pids.add(new_pid)
    pid_value = json.loads(main.pid_file.read_text(encoding="utf-8"))
    if pid_value != {
        "schema": PID_SCHEMA,
        "pid": new_pid,
        "start_identity": process_identity(
            new_pid, main.runtime, main.addr, sha256(main.runtime)
        ),
        "runtime_sha256": sha256(main.runtime),
        "addr": main.addr,
    }:
        raise AssertionError("published PID identity mismatch")
    if stat.S_IMODE(main.pid_file.stat().st_mode) != 0o600 or main.pid_file.stat().st_nlink != 1:
        raise AssertionError("published PID file is unsafe")
    for name in ("home", "services"):
        if not (
            receipt.get(f"{name}_served_index_sha256")
            == receipt.get(f"{name}_installed_index_sha256")
            == receipt.get(f"{name}_source_index_sha256")
        ):
            raise AssertionError(f"{name} parity mismatch")
    rollback = receipt.get("principal_root_rollback")
    if not isinstance(rollback, dict) or rollback.get("size_bytes", 0) <= 0:
        raise AssertionError("successful restart lacks rollback evidence")
    if stat.S_IMODE(main.restart_receipt.stat().st_mode) != 0o600 or main.restart_receipt.stat().st_nlink != 1:
        raise AssertionError("restart receipt is unsafe")

    second = main.run(ok=False)
    if "rollback_reconciliation_required" not in second.stderr or not process_alive(new_pid):
        raise AssertionError("second rollback was not blocked before stop")

    failed = Fixture(base, "failed-readiness")
    failed_old = failed.start_gateway()
    failed.write_pid(failed_old.pid)
    failed_result = failed.run(
        env=failed.environment(ELASTOS_SMOKE_BAD_HOME="1"),
        ok=False,
    )
    failed_old.wait(timeout=5)
    failed_receipt = json.loads(failed.restart_receipt.read_text(encoding="utf-8"))
    failed_pid = failed_receipt.get("gateway_pid")
    if failed_receipt.get("ok") is not False or not isinstance(failed_pid, int) or process_alive(failed_pid):
        raise AssertionError("failed readiness did not settle the exact new gateway")
    if failed.pid_file.exists() or failed.pid_file.is_symlink():
        raise AssertionError("failed readiness retained its PID file")
    if "home_readiness_parity_failed" not in failed_result.stderr:
        raise AssertionError("failed readiness did not reach Home parity")

    unsafe_fifo = Fixture(base, "unsafe-rollback-fifo")
    fifo = unsafe_fifo.run(
        env=unsafe_fifo.environment(ELASTOS_SMOKE_UNSAFE_ROLLBACK="fifo"),
        ok=False,
    )
    if "rollback_unsafe" not in fifo.stderr:
        raise AssertionError("restart accepted a FIFO rollback entry")
    fifo_entries = list((unsafe_fifo.data / "backups").glob("principal-root-upgrade-*/unsafe-entry"))
    if len(fifo_entries) != 1 or not stat.S_ISFIFO(fifo_entries[0].lstat().st_mode):
        raise AssertionError("FIFO rollback evidence was not retained")

    unsafe_hardlink = Fixture(base, "unsafe-rollback-hardlink")
    hardlink = unsafe_hardlink.run(
        env=unsafe_hardlink.environment(ELASTOS_SMOKE_UNSAFE_ROLLBACK="hardlink"),
        ok=False,
    )
    if "rollback_unsafe" not in hardlink.stderr:
        raise AssertionError("restart accepted a hard-linked rollback file")

    for fixture in (dead_stale, main, unrelated, failed, unsafe_fifo, unsafe_hardlink):
        fixture.cleanup()


def main():
    static_contract()
    home = pathlib.Path.home().resolve()
    base = pathlib.Path(tempfile.mkdtemp(prefix=".elastos-linux-restart-smoke.", dir=home))
    base.chmod(0o700)
    active = platform.system() == "Linux"
    try:
        dry_fixture = dry_run_proof(base)
        dry_fixture.cleanup()
        if active:
            linux_active_proof(base)
    finally:
        for pid in list(tracked_pids):
            stop_pid(pid)
            tracked_pids.discard(pid)
        shutil.rmtree(base)
    if tracked_pids or base.exists():
        raise AssertionError("Linux restart smoke left fixture residue")
    print(
        json.dumps(
            {
                "schema": "elastos.linux-source-home-restart-smoke/v1",
                "ok": True,
                "stable_installation_receipt": True,
                "dry_run_no_effect": True,
                "source_dirt_rejected": True,
                "exact_process_owner": True if active else "skipped_non_linux",
                "rollback_and_failure_cleanup": True if active else "skipped_non_linux",
                "home_services_parity": True if active else "skipped_non_linux",
                "fixture_residue": False,
            },
            separators=(",", ":"),
        )
    )


if __name__ == "__main__":
    main()
PY
