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
    import os, socket, sys
    from http.server import BaseHTTPRequestHandler, HTTPServer
    mode = os.environ.get("SMOKE_STUB", "ok")
    if mode == "exit":
        sys.exit(1)
    if mode == "secret":
        print("attach_secret=planted-secret", flush=True)
        sys.exit(1)
    if mode == "sleep":
        import time
        time.sleep(30)
        sys.exit(0)
    if mode == "fork":
        import time
        child = os.fork()
        if child == 0:
            with open(os.environ["SMOKE_CHILD_PID"], "w", encoding="utf-8") as handle:
                handle.write(str(os.getpid()))
            time.sleep(30)
            sys.exit(0)
        time.sleep(30)
        sys.exit(0)
    args = sys.argv[1:]
    if args[:1] != ["gateway"] or "--addr" not in args:
        sys.exit(2)
    host, port = args[args.index("--addr") + 1].rsplit(":", 1)
    if host.startswith("[") and host.endswith("]"):
        host = host[1:-1]
    class Handler(BaseHTTPRequestHandler):
        def do_GET(self):
            if mode == "newline":
                body = b"OK\\n"
            elif mode == "ok":
                body = b"OK"
            else:
                body = b"NO"
            self.send_response(200)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
        def log_message(self, *args):
            return
    class Server(HTTPServer):
        address_family = socket.AF_INET6 if host == "::1" else socket.AF_INET
    Server((host, int(port)), Handler).serve_forever()
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
        self.assertIn("two bytes OK", result.stderr)

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

    def test_default_addr_uses_a_free_port(self):
        env = os.environ.copy()
        env["SMOKE_STUB"] = "ok"
        env.pop("ELASTOS_GATEWAY_ADDR", None)
        env["ELASTOS_GATEWAY_HEALTH_DEADLINE"] = "5"
        result = subprocess.run(
            ["bash", str(SMOKE), str(self.binary)],
            env=env,
            capture_output=True,
            text=True,
            timeout=15,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("two bytes OK from pid", result.stdout)
        self.assertNotIn(":8090", result.stdout)

    def test_address_injection_exits_two(self):
        result = self.run_smoke("ok", addr="127.0.0.1:1@example.com:80")
        self.assertEqual(result.returncode, 2, result.stderr)
        self.assertNotIn("example.com", result.stdout)

    def test_localhost_name_exits_two(self):
        result = self.run_smoke("ok", addr="localhost:8090")
        self.assertEqual(result.returncode, 2, result.stderr)

    def test_newline_body_exits_one(self):
        result = self.run_smoke("newline")
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertIn("two bytes OK", result.stderr)

    def test_prebound_ok_is_not_this_process(self):
        from http.server import BaseHTTPRequestHandler, HTTPServer
        import threading

        class Handler(BaseHTTPRequestHandler):
            def do_GET(self):
                body = b"OK"
                self.send_response(200)
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)

            def log_message(self, *args):
                return

        server = HTTPServer(("127.0.0.1", self.port), Handler)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        self.addCleanup(server.server_close)
        self.addCleanup(server.shutdown)
        result = self.run_smoke("sleep", deadline="3")
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertIn("not gateway pid", result.stderr)

    def test_process_group_is_stopped(self):
        child_pid = Path(self.tmp.name) / "child.pid"
        env = os.environ.copy()
        env["SMOKE_STUB"] = "fork"
        env["SMOKE_CHILD_PID"] = str(child_pid)
        env["ELASTOS_GATEWAY_ADDR"] = f"127.0.0.1:{self.port}"
        env["ELASTOS_GATEWAY_HEALTH_DEADLINE"] = "1"
        result = subprocess.run(
            ["bash", str(SMOKE), str(self.binary)],
            env=env,
            capture_output=True,
            text=True,
            timeout=15,
        )
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertTrue(child_pid.exists(), result.stderr)
        grandchild = int(child_pid.read_text())
        with self.assertRaises(ProcessLookupError):
            os.kill(grandchild, 0)

    def test_log_is_kept_and_not_echoed(self):
        log = Path(self.tmp.name) / "gateway.log"
        env = os.environ.copy()
        env["SMOKE_STUB"] = "secret"
        env["ELASTOS_GATEWAY_ADDR"] = f"127.0.0.1:{self.port}"
        env["ELASTOS_GATEWAY_LOG"] = str(log)
        env["ELASTOS_GATEWAY_HEALTH_DEADLINE"] = "3"
        result = subprocess.run(
            ["bash", str(SMOKE), str(self.binary)],
            env=env,
            capture_output=True,
            text=True,
            timeout=15,
        )
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertNotIn("planted-secret", result.stdout)
        self.assertNotIn("planted-secret", result.stderr)
        self.assertIn("planted-secret", log.read_text())

    def test_ipv6_loopback_ok(self):
        with socket.socket(socket.AF_INET6) as sock:
            sock.bind(("::1", 0))
            port = sock.getsockname()[1]
        result = self.run_smoke("ok", addr=f"[::1]:{port}")
        self.assertEqual(result.returncode, 0, result.stderr + result.stdout)


if __name__ == "__main__":
    unittest.main()
