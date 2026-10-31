#!/usr/bin/env python3
"""Prewarm from a second Runtime of the same Home keeps the live control service."""
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import tempfile
import time
import unittest

ROOT = Path(__file__).resolve().parents[1]


def started(pid):
    """Process start time; with the pid it identifies one process."""
    result = subprocess.run(["ps", "-o", "lstart=", "-p", str(pid)], capture_output=True, text=True)
    return result.stdout.strip() or None


def wait_gone(pid, start, seconds):
    deadline = time.monotonic() + seconds
    while started(pid) == start and time.monotonic() < deadline:
        time.sleep(0.05)
    return started(pid) != start


class BrowserVmPrewarmOwner(unittest.TestCase):
    def setUp(self):
        self.directory = Path(tempfile.mkdtemp(prefix="browser-prewarm-", dir="/tmp"))
        self.writers = []
        self.services = {}  # pid -> start time of each control service this test started
        node = shutil.which("node")
        self.service = self.directory / "service"
        self.service.write_text(f'#!/bin/sh\nexec "{node}" "{ROOT}/scripts/browser-vm-control-service.mjs"\n')
        self.service.chmod(0o700)
        self.launcher = self.directory / "launcher"
        self.launcher.write_text("#!/bin/sh\nexit 1\n")  # prewarm starts no guest VM
        self.launcher.chmod(0o700)
        self.node = node

    def tearDown(self):
        # Closing every owner pipe stops the services; force only a process
        # that is still the exact one this test started.
        for writer in self.writers:
            self.close_owner(writer)
        for pid, start in self.services.items():
            if not wait_gone(pid, start, 5) and started(pid) == start:
                os.kill(pid, signal.SIGKILL)
        shutil.rmtree(self.directory, ignore_errors=True)

    def close_owner(self, writer):
        if writer in self.writers:
            self.writers.remove(writer)
            os.close(writer)

    def prewarm(self, **extra):
        # Each Runtime (gateway and its local runtime) owns its own lifetime pipe.
        reader, writer = os.pipe()
        self.writers.append(writer)
        stat = os.fstat(reader)
        environment = {
            "PATH": os.environ["PATH"],
            "ELASTOS_UPDATE_PARENT_PIPE": f"{reader}:{stat.st_dev}:{stat.st_ino}",
            "ELASTOS_BROWSER_VM_PREWARM_CONTROL_SERVICE": "1",
            "ELASTOS_BROWSER_VM_CONTROL_SERVICE": str(self.service),
            "ELASTOS_BROWSER_VM_CONTROL_LAUNCHER": str(self.launcher),
            "ELASTOS_BROWSER_VM_CONTROL_SOCKET": str(self.directory / "control.sock"),
            "ELASTOS_BROWSER_VM_DATA_DIR": str(self.directory),
            "ELASTOS_BROWSER_VM_ROOT": str(self.directory / "vms"),
            "ELASTOS_BROWSER_VM_PLATFORM": "darwin-arm64",
            **extra,
        }
        try:
            result = subprocess.run([self.node, str(ROOT / "scripts/browser-vm-engine-supervisor.mjs")],
                                    env=environment, pass_fds=(reader,), capture_output=True, text=True, timeout=20)
        finally:
            os.close(reader)
        self.assertEqual(result.returncode, 0, result.stderr)
        pid = json.loads(result.stdout)["control_status"]["pid"]
        self.services.setdefault(pid, started(pid))
        return pid, writer

    def status(self):
        result = subprocess.run(["curl", "-sS", "--max-time", "3", "--unix-socket",
                                 str(self.directory / "control.sock"), "http://localhost/status"],
                                capture_output=True, text=True, timeout=5, check=True)
        return json.loads(result.stdout)

    def test_second_runtime_prewarm_keeps_the_live_service(self):
        first, _ = self.prewarm()
        before = self.status()
        self.assertRegex(before.get("config_fingerprint_without_owner") or "", "^[0-9a-f]{64}$")
        second, _ = self.prewarm()
        self.assertEqual(second, first, "an owner-only difference must not restart the service")
        self.assertEqual(self.status()["config_fingerprint"], before["config_fingerprint"])

    def test_changed_config_still_replaces_the_idle_service(self):
        first, _ = self.prewarm()
        replaced, _ = self.prewarm(ELASTOS_BROWSER_VM_IDLE_KEEPALIVE_MS="1234")
        self.assertNotEqual(replaced, first, "a changed config must restart the service")
        self.assertEqual(self.status()["idle_vm_keepalive_ms"], 1234)

    def test_second_owner_closing_keeps_the_first_owners_service(self):
        first, _ = self.prewarm()
        second, second_owner = self.prewarm()
        self.assertEqual(second, first)
        self.close_owner(second_owner)
        time.sleep(1.0)
        self.assertEqual(started(first), self.services[first], "the kept service must outlive the other Runtime")
        self.assertEqual(self.status()["pid"], first)

    def test_real_owner_closing_stops_the_service(self):
        first, owner = self.prewarm()
        self.prewarm()  # the second Runtime keeps it; ownership stays with the first
        self.close_owner(owner)
        self.assertTrue(wait_gone(first, self.services[first], 5), "the service must exit with its owner")
        self.assertFalse((self.directory / "control.sock").exists(), "owned socket remains")


if __name__ == "__main__":
    unittest.main()
