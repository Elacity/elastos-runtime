#!/usr/bin/env python3
"""Real VM-control autostart path: adapter loss stops its isolated helper group."""
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import unittest

ROOT = Path(__file__).resolve().parents[1]


def alive(pid):
    try:
        os.kill(pid, 0)
        return True
    except ProcessLookupError:
        return False


def worker(directory, spawn_only=False, provider=False):
    directory = Path(directory)
    if provider:
        reader = int(os.environ["ELASTOS_UPDATE_PARENT_PIPE"].split(":")[0])
    else:
        reader, writer = os.pipe()  # writer remains with this Runtime stand-in
    stat = os.fstat(reader)
    node = shutil.which("node")
    service = directory / "service"
    if spawn_only:
        helper = directory / "helper.mjs"
        helper.write_text(f"""import fs from 'node:fs';
import {{watchParentPipe}} from '{ROOT}/scripts/browser-vm-control-service.mjs';
watchParentPipe(() => {{ fs.writeFileSync('{directory}/clean-stop', 'clean'); process.exit(0); }}, 1000);
fs.writeFileSync('{directory}/helper.pid', String(process.pid));
setInterval(() => {{}}, 1000);
""")
        service.write_text(f'#!/bin/sh\nexec "{node}" "{helper}"\n')
    else:
        service.write_text(f'#!/bin/sh\nexec "{node}" "{ROOT}/scripts/browser-vm-control-service.mjs"\n')
    service.chmod(0o700)
    launcher = directory / "launcher"
    launcher.write_text("#!/bin/sh\nexit 1\n")  # prewarm starts no guest VM
    launcher.chmod(0o700)
    environment = {
        **os.environ,
        "ELASTOS_UPDATE_PARENT_PIPE": f"{reader}:{stat.st_dev}:{stat.st_ino}",
        "ELASTOS_BROWSER_VM_PREWARM_CONTROL_SERVICE": "1",
        "ELASTOS_BROWSER_VM_CONTROL_SERVICE": str(service),
        "ELASTOS_BROWSER_VM_CONTROL_LAUNCHER": str(launcher),
        "ELASTOS_BROWSER_VM_CONTROL_SOCKET": str(directory / "control.sock"),
        "ELASTOS_BROWSER_VM_DATA_DIR": str(directory),
        "ELASTOS_BROWSER_VM_ROOT": str(directory / "vms"),
    }
    command = [node, str(ROOT / "scripts/browser-vm-engine-supervisor.mjs")]
    if spawn_only:
        # Exercise the production spawn and EOF watcher without opening a socket.
        command = [node, "--input-type=module", "-e", f"""
import {{startLocalVmControlService}} from '{ROOT}/scripts/browser-vm-engine-supervisor.mjs';
startLocalVmControlService({{controlSocket:'{directory}/control.sock', dataDir:'{directory}',
  platform:process.platform+'-'+process.arch, root:'{directory}/vms'}});
"""]
    result = subprocess.run(
        command,
        env=environment, pass_fds=(reader,), capture_output=True, text=True, timeout=15,
    )
    if result.returncode:
        log = directory / "logs/browser-vm-control-service.log"
        raise RuntimeError(result.stderr + (log.read_text() if log.exists() else ""))
    if spawn_only:
        deadline = time.monotonic() + 3
        while not (directory / "helper.pid").exists():
            if time.monotonic() >= deadline:
                raise RuntimeError("helper did not start")
            time.sleep(0.05)
        pid = int((directory / "helper.pid").read_text())
    else:
        ready = json.loads(result.stdout)
        pid = ready["control_status"]["pid"]
    # The transient engine launcher has exited; the adapter still owns the helper.
    assert alive(pid), "control service stopped before its owner"
    (directory / "ready.json").write_text(json.dumps({"helper": pid}))
    if provider:
        for line in sys.stdin:
            request = json.loads(line)
            print('{"status":"ok"}', flush=True)
            if request["op"] == "shutdown":
                break
    else:
        while True:
            time.sleep(1)


class BrowserHelperLifetime(unittest.TestCase):
    def test_real_spawn_stops_after_term_int_and_crash(self):
        self.check_parent_loss(spawn_only=True)

    def test_real_autostart_stops_after_term_int_and_crash(self):
        self.check_parent_loss(spawn_only=False)

    def check_parent_loss(self, spawn_only):
        for stop_signal in (signal.SIGTERM, signal.SIGINT, signal.SIGKILL):
            with self.subTest(signal=stop_signal), tempfile.TemporaryDirectory(prefix="browser-owner-", dir="/tmp") as directory:
                parent = subprocess.Popen([sys.executable, __file__, "--spawn-worker" if spawn_only else "--worker", directory],
                                          stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True)
                helper = None
                try:
                    ready = Path(directory) / "ready.json"
                    deadline = time.monotonic() + 20
                    while not ready.exists():
                        if parent.poll() is not None:
                            self.fail(f"autostart failed: {parent.stderr.read()}")
                        self.assertLess(time.monotonic(), deadline, "autostart timed out")
                        time.sleep(0.05)
                    helper = json.loads(ready.read_text())["helper"]
                    parent.send_signal(stop_signal)
                    parent.wait(timeout=3)
                    deadline = time.monotonic() + 5
                    while alive(helper) and time.monotonic() < deadline:
                        time.sleep(0.05)
                    self.assertFalse(alive(helper), f"helper {helper} survived adapter {stop_signal}")
                    self.assertFalse((Path(directory) / "control.sock").exists(), "owned socket remains")
                    if spawn_only:
                        self.assertTrue((Path(directory) / "clean-stop").exists(), "normal stop callback was skipped")
                finally:
                    if parent.poll() is None:
                        parent.kill()
                    parent.wait(timeout=3)
                    parent.stderr.close()
                    if helper and alive(helper):
                        os.killpg(helper, signal.SIGKILL)


if __name__ == "__main__":
    if len(sys.argv) == 3 and sys.argv[1] in ("--worker", "--spawn-worker", "--provider"):
        worker(sys.argv[2], spawn_only=sys.argv[1] != "--worker", provider=sys.argv[1] == "--provider")
    else:
        unittest.main()
