#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

python3 - "$repo_root" <<'PY'
import gzip
import hashlib
import json
import os
import pathlib
import shutil
import signal
import socket
import stat
import subprocess
import sys
import tempfile
import time


ROOT = pathlib.Path(sys.argv[1])
RESTART = ROOT / "scripts/mac-source-home-restart.sh"
INSTALL_SCHEMA = "elastos.source-home.installation-receipt/v1"
RESTART_SCHEMA = "elastos.mac-source-home-restart/v1"
PID_SCHEMA = "elastos.mac-source-home-gateway-pid/v1"
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


def write(path, value, mode=0o600):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(value)
    path.chmod(mode)


def owner_dir(path):
    path.mkdir(parents=True, exist_ok=True)
    path.chmod(0o700)


def free_port():
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def wait_listener(port, expected=True):
    for _ in range(100):
        with socket.socket() as sock:
            sock.settimeout(0.05)
            listening = sock.connect_ex(("127.0.0.1", port)) == 0
        if listening == expected:
            return
        time.sleep(0.05)
    raise AssertionError(f"listener state did not become {expected} on fixture port")


def process_start_identity(pid):
    result = subprocess.run(
        ["ps", "-ww", "-p", str(pid), "-o", "uid=", "-o", "lstart=", "-o", "command="],
        check=True,
        stdout=subprocess.PIPE,
        text=True,
    )
    parts = result.stdout.strip().split(None, 6)
    if len(parts) != 7:
        raise AssertionError("fixture process identity is unavailable")
    return hashlib.sha256(" ".join(parts[1:6]).encode("ascii")).hexdigest()


def process_alive(pid):
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    state = subprocess.run(
        ["ps", "-p", str(pid), "-o", "stat="],
        check=False,
        stdout=subprocess.PIPE,
        text=True,
    ).stdout.strip()
    return bool(state) and not state.startswith("Z")


def stop_pid(pid):
    if not pid or not process_alive(pid):
        return
    os.kill(pid, signal.SIGTERM)
    for _ in range(60):
        if not process_alive(pid):
            return
        time.sleep(0.05)
    if process_alive(pid):
        os.kill(pid, signal.SIGKILL)
    for _ in range(40):
        if not process_alive(pid):
            return
        time.sleep(0.05)


def static_contract():
    source = RESTART.read_text(encoding="utf-8")
    required = (
        'gateway_bin="${data_dir}/bin/elastos"',
        'installation_receipt="${data_dir}/receipts/source-home-installation.json"',
        "validate_installation_receipt",
        "process_identity",
        "write_owned_pid_file",
        "stop_verified_gateway",
        "check_no_existing_rollback",
        "principal_root_rollback",
        "installation_receipt_sha256",
        "installed_runtime_sha256",
        "verify_browser_helper_freshness",
        "home_cli_renderer_source_sha256",
    )
    missing = [value for value in required if value not in source]
    if missing:
        raise AssertionError(f"restart ownership contract is incomplete: {missing}")
    forbidden = (
        "pkill",
        "killall",
        "pgrep",
        "--gateway-bin",
        "--pid-file",
        "target/release/elastos",
        "cargo_built_binary_path",
        "for pid in $(lsof",
    )
    present = [value for value in forbidden if value in source]
    if present:
        raise AssertionError(f"restart retained broad or fallback ownership: {present}")
    if source.count('gateway_bin="${data_dir}/bin/elastos"') != 1:
        raise AssertionError("restart must select one stable gateway identity")
    if source.count("start_gateway_process()") != 1:
        raise AssertionError("restart must have one detached launcher")


class Fixture:
    def __init__(self, root, name):
        self.root = root / name
        self.home = self.root / "test home"
        self.data = self.home / "Library/Application Support/elastos"
        self.runtime = self.data / "bin/elastos"
        self.components = self.data / "components.json"
        self.capsule_receipt = self.data / "receipts/source-home-capsules.json"
        self.install_receipt = self.data / "receipts/source-home-installation.json"
        self.restart_receipt = self.data / "receipts/mac-source-home-restart.json"
        self.pid_file = self.data / "run/gateway.pid"
        self.port = free_port()
        self.addr = f"127.0.0.1:{self.port}"
        self.processes = []
        self._create()

    def _create(self):
        for path in (
            self.home,
            self.data,
            self.data / "bin",
            self.data / "receipts",
            self.data / "scripts",
            self.data / "browser-vm",
            self.data / "capsules/home/browser",
            self.data / "capsules/home-cli/browser",
        ):
            owner_dir(path)
        gateway = r'''#!/usr/bin/env python3
import http.server
import json
import os
import pathlib
import signal
import sys


def value_after(flag):
    return sys.argv[sys.argv.index(flag) + 1]


if len(sys.argv) >= 2 and sys.argv[1] == "principal-root-upgrade":
    backup = pathlib.Path(value_after("--backup-dir"))
    backup.mkdir(mode=0o700, parents=True, exist_ok=False)
    proof = backup / "rollback.json"
    proof.write_text('{"schema":"fixture.rollback/v1"}\n', encoding="utf-8")
    proof.chmod(0o600)
    unsafe_rollback = os.environ.get("ELASTOS_SMOKE_UNSAFE_ROLLBACK")
    if unsafe_rollback == "fifo":
        os.mkfifo(backup / "unsafe-entry", mode=0o600)
    elif unsafe_rollback == "hardlink":
        os.link(proof, backup / "unsafe-hardlink")
    print(json.dumps({"schema": "elastos.principal-root-upgrade/v1", "ok": True}))
    raise SystemExit(0)

if len(sys.argv) < 2 or sys.argv[1] != "gateway":
    raise SystemExit(2)
if os.environ.get("ELASTOS_SMOKE_FAIL_START") == "1":
    raise SystemExit(73)
host, port_text = value_after("--addr").rsplit(":", 1)
data = pathlib.Path(os.environ["HOME"]) / "Library/Application Support/elastos"
payload = (data / "capsules/home/browser/index.html").read_bytes()
if os.environ.get("ELASTOS_SMOKE_BAD_HOME") == "1":
    payload = b"bad-home\n"


class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path != "/apps/home/":
            self.send_error(404)
            return
        self.send_response(200)
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def log_message(self, _format, *_args):
        return


server = http.server.ThreadingHTTPServer((host, int(port_text)), Handler)
signal.signal(signal.SIGTERM, lambda _signum, _frame: (_ for _ in ()).throw(SystemExit(0)))
try:
    server.serve_forever()
finally:
    server.server_close()
'''
        write(self.runtime, gateway.encode(), 0o700)
        self.components.write_text('{"profile":"source-home"}\n', encoding="utf-8")
        self.capsule_receipt.write_text(
            '{"schema":"elastos.source-home.managed-capsules/v1","capsules":[]}\n',
            encoding="utf-8",
        )
        helper = ROOT / "scripts/browser-selkies-control-service.mjs"
        shutil.copyfile(helper, self.data / "scripts/browser-selkies-control-service.mjs")
        (self.data / "browser-vm/rootfs.ext4").write_bytes(b"fixture-rootfs\n")
        self._write_initrd(helper)
        shutil.copyfile(
            ROOT / "capsules/home/browser/index.html",
            self.data / "capsules/home/browser/index.html",
        )
        shutil.copytree(
            ROOT / "capsules/home-cli/browser",
            self.data / "capsules/home-cli/browser",
            dirs_exist_ok=True,
        )
        write(
            self.root / "debugfs-fixture",
            b'#!/usr/bin/env bash\nset -euo pipefail\ncat "$ELASTOS_SMOKE_BROWSER_HELPER"\n',
            0o700,
        )
        self.write_install_receipt()

    def _write_initrd(self, helper):
        tree = self.root / "initrd-tree"
        owner_dir(tree)
        owner_dir(tree / "bin")
        shutil.copyfile(helper, tree / "bin/browser-selkies-control-service.mjs")
        archive = subprocess.run(
            ["cpio", "-o", "-H", "newc"],
            cwd=tree,
            input=b".\n./bin\n./bin/browser-selkies-control-service.mjs\n",
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            check=True,
        ).stdout
        (self.data / "bin/initrd").write_bytes(gzip.compress(archive))
        shutil.rmtree(tree)

    def write_install_receipt(self, commit=SOURCE_COMMIT, clean=True):
        runtime_hash = "sha256:" + sha256(self.runtime)
        receipt = {
            "schema": INSTALL_SCHEMA,
            "source": {"commit": commit, "tree": SOURCE_TREE, "clean": clean},
            "runtime": {
                "built_sha256": runtime_hash,
                "installed_sha256": runtime_hash,
                "parity": True,
            },
            "components_sha256": "sha256:" + sha256(self.components),
            "source_home_capsule_metadata_receipt_sha256": "sha256:" + sha256(self.capsule_receipt),
            "platform": "darwin-arm64",
            "installation_time_utc": "2026-08-28T12:00:00Z",
        }
        self.install_receipt.write_text(json.dumps(receipt, indent=2) + "\n", encoding="utf-8")
        self.install_receipt.chmod(0o600)

    def replace_runtime(self):
        previous = self.runtime.read_bytes()
        replacement = self.runtime.with_name(f".{self.runtime.name}.replacement")
        write(replacement, previous + b"\n# fixture atomic replacement\n", 0o700)
        os.replace(replacement, self.runtime)
        self.write_install_receipt()

    def environment(self, **values):
        env = os.environ.copy()
        env["ELASTOS_DEBUGFS_BIN"] = str(self.root / "debugfs-fixture")
        env["ELASTOS_SMOKE_BROWSER_HELPER"] = str(
            ROOT / "scripts/browser-selkies-control-service.mjs"
        )
        env.update({key: str(value) for key, value in values.items()})
        return env

    def command(self, *extra):
        return [
            str(RESTART),
            "--test-home",
            str(self.home),
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
            env={**os.environ, "HOME": str(self.home)},
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        self.processes.append(process.pid)
        tracked_pids.add(process.pid)
        wait_listener(self.port)
        return process

    def write_pid(self, pid, start_identity=None, runtime_hash=None, mode=0o600):
        owner_dir(self.pid_file.parent)
        value = {
            "schema": PID_SCHEMA,
            "pid": pid,
            "start_identity": start_identity or process_start_identity(pid),
            "runtime_sha256": runtime_hash or sha256(self.runtime),
            "addr": self.addr,
        }
        self.pid_file.write_text(json.dumps(value, separators=(",", ":")) + "\n")
        self.pid_file.chmod(mode)

    def cleanup(self):
        if self.pid_file.is_file() and not self.pid_file.is_symlink():
            try:
                pid = json.loads(self.pid_file.read_text()).get("pid")
                if isinstance(pid, int):
                    self.processes.append(pid)
                    tracked_pids.add(pid)
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


def assert_rejected(fixture, needle, *, env=None):
    result = fixture.run(env=env, ok=False)
    if needle not in result.stderr:
        raise AssertionError(f"missing rejection {needle!r}: {result.stderr!r}")
    private_values = (str(fixture.home), str(fixture.data), str(fixture.runtime))
    if any(value in result.stderr for value in private_values):
        raise AssertionError("restart failure leaked a private fixture path")
    return result


def run_smoke(temp_root):
    static_contract()
    invalid_addr = subprocess.run(
        [str(RESTART), "--dry-run", "--addr", "not-an-address"],
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        check=False,
    )
    if invalid_addr.returncode == 0:
        raise AssertionError("restart accepted an invalid address")
    main = Fixture(temp_root, "main")
    before = snapshot(main.root)
    dry = main.run("--dry-run")
    after = snapshot(main.root)
    if before != after:
        raise AssertionError("dry run changed the fixture")
    plan = json.loads(dry.stdout)
    if plan.get("schema") != RESTART_SCHEMA or plan.get("ok") is not True or plan.get("dry_run") is not True:
        raise AssertionError("dry-run receipt status mismatch")
    if plan.get("gateway_bin") != str(main.runtime):
        raise AssertionError("dry run did not select the stable installed Runtime")
    if plan.get("installed_runtime_sha256") != sha256(main.runtime):
        raise AssertionError("dry run did not bind the installed Runtime hash")
    if any("rollback" in key for key in plan):
        raise AssertionError("dry run claimed a retained or planned rollback")
    if main.restart_receipt.exists() or (main.data / "run").exists() or (main.home / "logs").exists():
        raise AssertionError("dry run created restart state")

    source_dirt = ROOT / f".mac-source-home-restart-smoke-dirt-{os.getpid()}"
    try:
        source_dirt.write_text("untracked source dirt\n", encoding="utf-8")
        assert_rejected(main, "source_dirty")
    finally:
        source_dirt.unlink(missing_ok=True)

    tracked_source = ROOT / "PRINCIPLES.md"
    tracked_bytes = tracked_source.read_bytes()
    tracked_mode = stat.S_IMODE(tracked_source.stat().st_mode)
    try:
        tracked_source.write_bytes(tracked_bytes + b"\n")
        assert_rejected(main, "source_dirty")
    finally:
        tracked_source.write_bytes(tracked_bytes)
        tracked_source.chmod(tracked_mode)

    dead_stale = Fixture(temp_root, "dead-stale-pid")
    dead_stale.write_pid(
        99999999,
        start_identity="1" * 64,
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

    old = main.start_gateway()
    original_components = main.components.read_bytes()
    main.write_pid(old.pid)
    main.components.write_bytes(b"changed\n")
    assert_rejected(main, "components_hash")
    if not process_alive(old.pid):
        raise AssertionError("hash mismatch stopped the old gateway")
    main.components.write_bytes(original_components)

    original_receipt = main.install_receipt.read_bytes()
    main.write_install_receipt(commit="0" * 40)
    assert_rejected(main, "source_identity")
    if not process_alive(old.pid):
        raise AssertionError("stale source receipt stopped the old gateway")
    main.install_receipt.write_bytes(original_receipt)
    main.install_receipt.chmod(0o600)

    main.pid_file.write_text("not-json\n", encoding="utf-8")
    main.pid_file.chmod(0o600)
    assert_rejected(main, "unsafe or malformed")
    if not process_alive(old.pid):
        raise AssertionError("malformed PID file stopped the old gateway")

    main.pid_file.unlink()
    unsafe_target = main.root / "unsafe-pid-target"
    write(unsafe_target, b"preserve\n")
    main.pid_file.symlink_to(unsafe_target)
    assert_rejected(main, "unsafe or malformed")
    if unsafe_target.read_bytes() != b"preserve\n" or not process_alive(old.pid):
        raise AssertionError("unsafe PID rejection changed unrelated state")

    main.pid_file.unlink()
    main.write_pid(old.pid, start_identity="0" * 64)
    assert_rejected(main, "identity changed")
    if not process_alive(old.pid):
        raise AssertionError("PID identity rejection stopped the old gateway")

    unrelated = Fixture(temp_root, "unrelated")
    unrelated_process = subprocess.Popen(
        [sys.executable, "-m", "http.server", str(unrelated.port), "--bind", "127.0.0.1"],
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    unrelated.processes.append(unrelated_process.pid)
    tracked_pids.add(unrelated_process.pid)
    wait_listener(unrelated.port)
    assert_rejected(unrelated, "unrelated listener")
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
    stdout_receipt = json.loads(success.stdout)
    if receipt != stdout_receipt or receipt.get("ok") is not True or receipt.get("dry_run") is not False:
        raise AssertionError("successful restart receipt mismatch")
    new_pid = receipt.get("gateway_pid")
    if not isinstance(new_pid, int) or new_pid == old.pid or not process_alive(new_pid):
        raise AssertionError("matching gateway was not replaced by one exact child")
    main.processes.append(new_pid)
    tracked_pids.add(new_pid)
    pid_value = json.loads(main.pid_file.read_text(encoding="utf-8"))
    if pid_value != {
        "schema": PID_SCHEMA,
        "pid": new_pid,
        "start_identity": process_start_identity(new_pid),
        "runtime_sha256": sha256(main.runtime),
        "addr": main.addr,
    }:
        raise AssertionError("published PID identity is not exact")
    if stat.S_IMODE(main.pid_file.stat().st_mode) != 0o600 or main.pid_file.stat().st_nlink != 1:
        raise AssertionError("published PID file is not owner-only with one link")
    wait_listener(main.port)
    if receipt.get("gateway_bin") != str(main.runtime):
        raise AssertionError("success receipt gateway path mismatch")
    if receipt.get("gateway_bin_sha256") != sha256(main.runtime):
        raise AssertionError("success receipt Runtime hash mismatch")
    if receipt.get("installed_runtime_sha256") != sha256(main.runtime):
        raise AssertionError("success receipt installed Runtime hash mismatch")
    if receipt.get("installation_receipt_sha256") != sha256(main.install_receipt):
        raise AssertionError("success receipt installation receipt hash mismatch")
    if not (
        receipt.get("served_index_sha256")
        == receipt.get("installed_index_sha256")
        == receipt.get("source_index_sha256")
    ):
        raise AssertionError("successful Home hash parity mismatch")
    if not (
        receipt.get("browser_helper_source_sha256")
        == receipt.get("browser_helper_installed_sha256")
        == receipt.get("browser_helper_initrd_sha256")
        == receipt.get("browser_helper_rootfs_sha256")
    ):
        raise AssertionError("successful Browser helper parity mismatch")
    if receipt.get("home_cli_renderer_source_sha256") != receipt.get("home_cli_renderer_installed_sha256"):
        raise AssertionError("successful Home CLI renderer parity mismatch")
    rollback = receipt.get("principal_root_rollback")
    if not isinstance(rollback, dict) or not rollback.get("relative_identity", "").startswith("backups/principal-root-upgrade-"):
        raise AssertionError("successful receipt lacks the relative rollback identity")
    if rollback.get("size_bytes", 0) <= 0 or rollback.get("reason") != "principal_root_upgrade":
        raise AssertionError("successful receipt rollback facts mismatch")
    rollbacks = list((main.data / "backups").glob("principal-root-upgrade-*"))
    if len(rollbacks) != 1 or stat.S_IMODE(rollbacks[0].stat().st_mode) != 0o700:
        raise AssertionError("restart did not retain exactly one owner-only rollback")
    if (
        stat.S_IMODE(main.restart_receipt.stat().st_mode) != 0o600
        or main.restart_receipt.stat().st_nlink != 1
        or main.restart_receipt.stat().st_size > 32 * 1024
    ):
        raise AssertionError("restart receipt is not owner-only")
    for log in (main.home / "logs").iterdir():
        if log.is_file() and stat.S_IMODE(log.stat().st_mode) != 0o600:
            raise AssertionError("restart log is not owner-only")

    second = main.run(ok=False)
    if "rollback reconciliation" not in second.stderr or not process_alive(new_pid):
        raise AssertionError("second rollback was not blocked before gateway stop")
    if len(list((main.data / "backups").glob("principal-root-upgrade-*"))) != 1:
        raise AssertionError("blocked restart created a second rollback")

    failed = Fixture(temp_root, "failed")
    failed_old = failed.start_gateway()
    failed.write_pid(failed_old.pid)
    failed_env = failed.environment(ELASTOS_SMOKE_BAD_HOME="1")
    failed_result = failed.run(env=failed_env, ok=False)
    failed_old.wait(timeout=5)
    failed_receipt = json.loads(failed.restart_receipt.read_text(encoding="utf-8"))
    if failed_receipt.get("ok") is not False or failed_receipt.get("dry_run") is not False:
        raise AssertionError("failed startup did not write ok=false receipt")
    failed_new_pid = failed_receipt.get("gateway_pid")
    if not isinstance(failed_new_pid, int) or process_alive(failed_new_pid):
        raise AssertionError("failed startup left the new exact gateway running")
    if failed.pid_file.exists() or failed.pid_file.is_symlink():
        raise AssertionError("failed startup retained its PID file")
    if len(list((failed.data / "backups").glob("principal-root-upgrade-*"))) != 1:
        raise AssertionError("failed startup did not preserve one rollback")
    if "Home readiness parity failed" not in failed_result.stderr:
        raise AssertionError("failed startup did not reach the readiness boundary")

    unsafe_fifo = Fixture(temp_root, "unsafe-rollback-fifo")
    fifo_result = unsafe_fifo.run(
        env=unsafe_fifo.environment(ELASTOS_SMOKE_UNSAFE_ROLLBACK="fifo"),
        ok=False,
    )
    if "principal-root rollback is unsafe" not in fifo_result.stderr:
        raise AssertionError("restart accepted a FIFO in the retained rollback")
    fifo_entries = list((unsafe_fifo.data / "backups").glob("principal-root-upgrade-*/unsafe-entry"))
    if len(fifo_entries) != 1 or not stat.S_ISFIFO(fifo_entries[0].lstat().st_mode):
        raise AssertionError("FIFO rollback rejection did not preserve exact evidence")

    unsafe_hardlink = Fixture(temp_root, "unsafe-rollback-hardlink")
    hardlink_result = unsafe_hardlink.run(
        env=unsafe_hardlink.environment(ELASTOS_SMOKE_UNSAFE_ROLLBACK="hardlink"),
        ok=False,
    )
    if "principal-root rollback is unsafe" not in hardlink_result.stderr:
        raise AssertionError("restart accepted a hard-linked rollback file")
    hardlink_entries = list(
        (unsafe_hardlink.data / "backups").glob("principal-root-upgrade-*/rollback.json")
    )
    if len(hardlink_entries) != 1 or hardlink_entries[0].stat().st_nlink != 2:
        raise AssertionError("hard-link rollback rejection did not preserve exact evidence")

    disposable_root = pathlib.Path(tempfile.mkdtemp(prefix="elastos-restart-disposable.", dir="/tmp"))
    try:
        disposable = Fixture(disposable_root, "fixture")
        result = disposable.run("--dry-run", ok=False)
        if "disposable_location" not in result.stderr:
            raise AssertionError("restart accepted a disposable installed Runtime")
    finally:
        shutil.rmtree(disposable_root)

    main.cleanup()
    dead_stale.cleanup()
    unrelated.cleanup()
    failed.cleanup()
    unsafe_fifo.cleanup()
    unsafe_hardlink.cleanup()


def main():
    if sys.platform != "darwin":
        raise SystemExit("mac-source-home-restart smoke requires macOS")
    base = pathlib.Path(tempfile.mkdtemp(prefix="elastos-mac-restart-smoke.")).resolve()
    base.chmod(0o700)
    try:
        run_smoke(base)
    finally:
        for pid in list(tracked_pids):
            stop_pid(pid)
        shutil.rmtree(base)
    if tracked_pids:
        raise AssertionError("restart smoke left tracked fixture processes")
    print(
        json.dumps(
            {
                "schema": "elastos.mac-source-home-restart-smoke/v1",
                "ok": True,
                "stable_receipt": True,
                "exact_process_owner": True,
                "single_rollback": True,
                "failed_restart_cleanup": True,
                "browser_helper_freshness_gate_present": True,
                "invalid_addr_rejected": True,
                "fixture_residue": False,
            },
            separators=(",", ":"),
        )
    )


if __name__ == "__main__":
    main()
PY
