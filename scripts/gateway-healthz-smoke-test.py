#!/usr/bin/env python3
"""The gateway smoke script accepts a loopback probe and rejects the other cases."""

import os
import socket
import stat
import subprocess
import tempfile
import textwrap
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SMOKE = ROOT / "scripts" / "gateway-healthz-smoke.sh"

STUB = textwrap.dedent(
    """\
    #!/usr/bin/env python3
    import os, sys
    from http.server import BaseHTTPRequestHandler, HTTPServer
    mode = os.environ.get("SMOKE_STUB", "ok")
    if mode == "exit":
        sys.exit(1)
    if mode == "sleep":
        import time
        time.sleep(30)
        sys.exit(0)
    args = sys.argv[1:]
    if args[:1] != ["gateway"] or "--addr" not in args:
        sys.exit(2)
    host, port = args[args.index("--addr") + 1].rsplit(":", 1)
    class Handler(BaseHTTPRequestHandler):
        def do_GET(self):
            body = b"OK" if mode == "ok" else b"NO"
            self.send_response(200)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
        def log_message(self, *args):
            return
    HTTPServer((host, int(port)), Handler).serve_forever()
    """
)


class GatewayHealthzSmokeTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.binary = Path(self.tmp.name) / "elastos"
        self.binary.write_text(STUB)
        self.binary.chmod(self.binary.stat().st_mode | stat.S_IEXEC)
        self.port = self.free_port()

    @staticmethod
    def free_port():
        with socket.socket() as sock:
            sock.bind(("127.0.0.1", 0))
            return sock.getsockname()[1]

    def run_smoke(self, stub, addr=None, deadline="3"):
        env = os.environ.copy()
        env["SMOKE_STUB"] = stub
        env["ELASTOS_GATEWAY_HEALTH_DEADLINE"] = deadline
        if addr is not None:
            env["ELASTOS_GATEWAY_ADDR"] = addr
        else:
            env["ELASTOS_GATEWAY_ADDR"] = f"127.0.0.1:{self.port}"
        return subprocess.run(
            ["bash", str(SMOKE), str(self.binary)],
            env=env,
            capture_output=True,
            text=True,
            timeout=15,
        )

    def test_ok_body_exits_zero(self):
        result = self.run_smoke("ok")
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_other_body_exits_one(self):
        result = self.run_smoke("other")
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertIn("body is not OK", result.stderr)

    def test_early_exit_exits_one(self):
        result = self.run_smoke("exit", deadline="5")
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertIn("exited before", result.stderr)

    def test_timeout_exits_one(self):
        result = self.run_smoke("sleep", deadline="1")
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertIn("did not return OK", result.stderr)

    def test_non_loopback_addr_exits_two(self):
        result = self.run_smoke("ok", addr="0.0.0.0:9")
        self.assertEqual(result.returncode, 2, result.stderr)
        self.assertIn("loopback", result.stderr)

    def test_missing_binary_exits_one(self):
        missing = Path(self.tmp.name) / "missing"
        result = subprocess.run(
            ["bash", str(SMOKE), str(missing)],
            capture_output=True,
            text=True,
            timeout=10,
        )
        self.assertEqual(result.returncode, 1, result.stderr)


if __name__ == "__main__":
    unittest.main()
