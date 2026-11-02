#!/usr/bin/env python3
"""Exercise the wrapper without building or touching an installed Home."""
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


SCRIPT = Path(__file__).with_name("build-and-setup-source-home.sh")


class BuildAndSetupTest(unittest.TestCase):
    def test_private_copies_cleanup_and_setup_failure(self):
        with tempfile.TemporaryDirectory(prefix="source-home-wrapper-") as temporary:
            root = Path(temporary)
            scripts = root / "scripts"
            scripts.mkdir()
            shutil.copyfile(SCRIPT, scripts / SCRIPT.name)
            source = root / "shared tools"
            source.mkdir(mode=0o755)
            executable = root / "tool"
            executable.write_text('#!/bin/sh\ntest "$1" = -version\n')
            executable.chmod(0o755)
            for name in ("ffmpeg", "ffprobe"):
                (source / name).symlink_to(executable)
            record = root / "stage-path"
            (scripts / "setup-source-home.sh").write_text('''#!/bin/bash
set -eu
python3 - <<'PY'
import os, pathlib, stat
p = pathlib.Path(os.environ["SETUP_SOURCE_HOME_MEDIA_TOOLS_DIR"])
for item in (p, p / "ffmpeg", p / "ffprobe"):
    s = item.lstat()
    assert not stat.S_ISLNK(s.st_mode)
    assert stat.S_IMODE(s.st_mode) == 0o700
    assert s.st_uid == os.geteuid()
assert os.environ["ELASTOS_COLLABORATION_STARTUP_MODE"] == "isolated"
assert os.environ["SETUP_SOURCE_HOME_RUNTIME_TURN"] == "0"
pathlib.Path(os.environ["TEST_RECORD"]).write_text(str(p))
PY
exit "${TEST_EXIT:-0}"
''')
            env = {key: value for key, value in os.environ.items()
                   if not key.startswith(("SETUP_SOURCE_HOME_", "ELASTOS_COLLABORATION_"))}
            env.update(TEST_RECORD=str(record), TMPDIR=temporary, HOME=temporary)
            command = ["/bin/bash", str(scripts / SCRIPT.name), "--media-tools-dir", str(source)]
            for exit_code in (0, 7):
                env["TEST_EXIT"] = str(exit_code)
                result = subprocess.run(command, env=env, capture_output=True, text=True)
                self.assertEqual(result.returncode, exit_code, result.stderr)
                self.assertFalse(Path(record.read_text()).exists())
            self.assertEqual(source.stat().st_mode & 0o777, 0o755)
            self.assertEqual(executable.stat().st_mode & 0o777, 0o755)
            self.assertTrue((source / "ffmpeg").is_symlink())
            record.unlink()
            result = subprocess.run(command + ["--check-tools"], env=env, capture_output=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertFalse(record.exists())
            (source / "ffprobe").unlink()
            result = subprocess.run(command, env=env, capture_output=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertFalse(record.exists())


if __name__ == "__main__":
    unittest.main()
