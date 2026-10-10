#!/usr/bin/env python3
"""Hold one Browser ownership writer for a complete operator smoke command."""
import os
import signal
import subprocess
import sys

reader, writer = os.pipe()
stat = os.fstat(reader)
env = dict(os.environ, ELASTOS_UPDATE_PARENT_PIPE=f"{reader}:{stat.st_dev}:{stat.st_ino}")
try:
    child = subprocess.Popen(sys.argv[1:], env=env, pass_fds=(reader,))
    os.close(reader)
    def stop(signum, _frame):
        child.send_signal(signum)
    signal.signal(signal.SIGTERM, stop)
    signal.signal(signal.SIGINT, stop)
    result = child.wait()
finally:
    os.close(writer)
raise SystemExit(result if result >= 0 else 128 - result)
