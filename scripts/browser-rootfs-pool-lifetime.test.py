#!/usr/bin/env python3
"""Copy cancellation reaps cp before removing its incomplete pool image."""
import os
from pathlib import Path
import shutil
import signal
import subprocess
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


class RootfsPoolLifetime(unittest.TestCase):
    def test_interrupted_copy_is_reaped_and_partial_removed(self):
        for stop in (signal.SIGTERM, signal.SIGINT, "eof", "timeout"):
            with self.subTest(stop=stop), tempfile.TemporaryDirectory(prefix="browser-copy-") as directory:
                directory = Path(directory)
                rootfs = directory / "rootfs.ext4"
                rootfs.write_bytes(b"fixture")
                cp = directory / "cp"
                cp.write_text(f"""#!{shutil.which('python3')}
import os, pathlib, signal, sys, time
signal.signal(signal.SIGTERM, signal.SIG_IGN)
pathlib.Path(sys.argv[-1]).write_bytes(b'partial')
pathlib.Path('{directory}/cp.pid').write_text(str(os.getpid()))
while True: time.sleep(.1)
""")
                cp.chmod(0o700)
                env = dict(os.environ, PATH=f"{directory}:{os.environ['PATH']}",
                           ELASTOS_BROWSER_LOCAL_EXIT_PARENT_EOF="1")
                if stop == "timeout":
                    env["ELASTOS_BROWSER_VM_ROOTFS_COPY_TIMEOUT_MS"] = "1000"
                process = subprocess.Popen([shutil.which("node"), str(ROOT / "scripts/browser-vm-prepare-rootfs-pool.mjs"),
                                            "--data-dir", str(directory), "--rootfs", str(rootfs),
                                            "--pool-dir", str(directory / "pool")],
                                           stdin=subprocess.PIPE, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, env=env)
                copy_pid = None
                try:
                    deadline = time.monotonic() + 5
                    while not (directory / "cp.pid").exists():
                        self.assertIsNone(process.poll(), "copy did not start")
                        self.assertLess(time.monotonic(), deadline)
                        time.sleep(.02)
                    copy_pid = int((directory / "cp.pid").read_text())
                    if stop == "eof":
                        process.stdin.close()
                    elif stop != "timeout":
                        process.send_signal(stop)
                    process.wait(timeout=5)
                    self.assertFalse(alive(copy_pid), "cp survived its owner")
                    self.assertEqual(list((directory / "pool").glob("*.partial")), [], "partial image leaked")
                finally:
                    if process.poll() is None:
                        process.kill()
                    process.wait(timeout=3)
                    if copy_pid and alive(copy_pid):
                        os.kill(copy_pid, signal.SIGKILL)
                    if not process.stdin.closed:
                        process.stdin.close()
                    process.stderr.close()


if __name__ == "__main__":
    unittest.main()
