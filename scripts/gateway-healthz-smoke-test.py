#!/usr/bin/env python3
"""The gateway smoke accepts contract readiness and refuses every other case.

Readiness is defined in docs/LAUNCHER_CONTRACT.md "Readiness and Home URL".
A stub stands in for the elastos binary so each refusal path runs on Linux.
"""

import os
import signal
import socket
import stat
import subprocess
import tempfile
import textwrap
import threading
import time
import unittest
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SMOKE = ROOT / "scripts" / "gateway-healthz-smoke.sh"

STUB = textwrap.dedent(
    """\
    #!/usr/bin/env python3
    import json, os, signal, socket, sys, time
    from http.server import BaseHTTPRequestHandler, HTTPServer
    mode = os.environ.get("SMOKE_STUB", "ok")
    lock_mode = os.environ.get("SMOKE_LOCK", "good")
    args = sys.argv[1:]
    if "SMOKE_ARGS" in os.environ:
        with open(os.environ["SMOKE_ARGS"], "w", encoding="utf-8") as handle:
            json.dump(args, handle)
    def fork_sleeper(ignore_term=False):
        child = os.fork()
        if child == 0:
            if ignore_term:
                signal.signal(signal.SIGTERM, signal.SIG_IGN)
            with open(os.environ["SMOKE_CHILD_PID"], "w", encoding="utf-8") as handle:
                handle.write(str(os.getpid()))
            time.sleep(60)
            os._exit(0)
    if mode == "exit":
        sys.exit(1)
    if mode == "secret":
        print("attach_secret=planted-secret", flush=True)
        sys.exit(1)
    if mode == "sleep":
        time.sleep(30)
        sys.exit(0)
    if mode in ("fork", "ignore-term"):
        fork_sleeper(ignore_term=(mode == "ignore-term"))
        time.sleep(60)
        sys.exit(0)
    if args[:2] != ["gateway", "--addr"] or len(args) != 3:
        sys.exit(2)
    addr = args[2]
    host, port = addr.rsplit(":", 1)
    host = host.strip("[]")
    data = os.path.join(os.environ["XDG_DATA_HOME"], "elastos")
    os.makedirs(data, exist_ok=True)
    record = {"pid": os.getpid(), "role": "gateway", "addr": addr}
    if lock_mode == "other-pid":
        record["pid"] = os.getpid() + 100000
    elif lock_mode == "bad-role":
        record["role"] = "serve"
    elif lock_mode == "bad-addr":
        record["addr"] = "127.0.0.1:1"
    if lock_mode == "garbage":
        with open(os.path.join(data, "host-process.lock"), "w") as handle:
            handle.write("not json")
    elif lock_mode != "none":
        with open(os.path.join(data, "host-process.lock"), "w") as handle:
            json.dump(record, handle)
    if mode == "ok-fork":
        fork_sleeper()
    not_ready = int(os.environ.get("SMOKE_503_COUNT", "0"))
    class Handler(BaseHTTPRequestHandler):
        def do_GET(self):
            global not_ready
            status, body = 200, b"OK"
            if not_ready > 0:
                not_ready -= 1
                status, body = 503, b""
            elif mode == "newline":
                body = b"OK\\n"
            elif mode == "redirect":
                status = 302
            elif mode == "no-content":
                status = 204
            elif mode == "server-error":
                status = 500
            elif mode == "other":
                body = b"NO"
            elif mode == "always-503":
                status, body = 503, b""
            self.send_response(status)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
        def log_message(self, *args):
            return
    if host == "localhost":
        host = os.environ.get("SMOKE_LOCALHOST_BIND", "127.0.0.1")
    class Server(HTTPServer):
        address_family = socket.AF_INET6 if ":" in host else socket.AF_INET
    Server((host, int(port)), Handler).serve_forever()
    """
)


def ipv6_loopback():
    try:
        with socket.socket(socket.AF_INET6) as sock:
            sock.bind(("::1", 0))
        return True
    except OSError:
        return False


def free_port(family=socket.AF_INET, host="127.0.0.1"):
    with socket.socket(family) as sock:
        sock.bind((host, 0))
        return sock.getsockname()[1]


def process_gone(pid):
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return True
    return False


class OkHandler(BaseHTTPRequestHandler):
    def do_GET(self):
        body = b"OK"
        self.send_response(200)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *args):
        return


class GatewayHealthzSmokeTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.binary = Path(self.tmp.name) / "elastos"
        self.binary.write_text(STUB)
        self.binary.chmod(self.binary.stat().st_mode | stat.S_IEXEC)
        self.port = free_port()
        self.child_pid = Path(self.tmp.name) / "child.pid"

    def env(self, stub, addr=None, deadline="3", **extra):
        env = os.environ.copy()
        env["SMOKE_STUB"] = stub
        env["SMOKE_CHILD_PID"] = str(self.child_pid)
        env["ELASTOS_GATEWAY_HEALTH_DEADLINE"] = deadline
        env["ELASTOS_GATEWAY_ADDR"] = addr if addr is not None else f"127.0.0.1:{self.port}"
        env.update(extra)
        return env

    def run_smoke(self, stub, addr=None, deadline="3", **extra):
        return subprocess.run(
            ["bash", str(SMOKE), str(self.binary)],
            env=self.env(stub, addr, deadline, **extra),
            capture_output=True,
            text=True,
            timeout=30,
        )

    def serve_ok(self, family, host, port):
        class Server(HTTPServer):
            address_family = family

        server = Server((host, port), OkHandler)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        self.addCleanup(server.server_close)
        self.addCleanup(server.shutdown)

    # Ready.

    def test_ok_with_owned_lock_is_ready(self):
        result = self.run_smoke("ok")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn(f"returned HTTP 200 and OK from gateway pid", result.stdout)
        self.assertIn("host-process.lock names pid", result.stdout)
        self.assertIn(f"listens on: 127.0.0.1:{self.port}", result.stdout)
        self.assertRegex(result.stdout, rf"fact: GET http://127\.0\.0\.1:{self.port}/healthz: "
                         r"HTTP 200, body OK, from gateway pid \d+ on 127\.0\.0\.1:")
        self.assertIn(f"fact: GET http://[::1]:{self.port}/healthz: no answer", result.stdout)

    def test_503_then_200_is_ready(self):
        result = self.run_smoke("ok", deadline="10", SMOKE_503_COUNT="2")
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_localhost_name_is_probed_as_given(self):
        result = self.run_smoke("ok", addr=f"localhost:{self.port}")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn(f"GET http://localhost:{self.port}/healthz", result.stdout)
        self.assertIn("resolves localhost to", result.stdout)

    def test_default_is_the_contract_command(self):
        args = Path(self.tmp.name) / "args.json"
        env = self.env("exit", SMOKE_ARGS=str(args))
        env.pop("ELASTOS_GATEWAY_ADDR")
        result = subprocess.run(["bash", str(SMOKE), str(self.binary)], env=env,
                                capture_output=True, text=True, timeout=30)
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertEqual(args.read_text(), '["gateway", "--addr", "localhost:8090"]')

    @unittest.skipUnless(ipv6_loopback(), "this host has no IPv6 loopback")
    def test_ipv6_loopback_ok(self):
        port = free_port(socket.AF_INET6, "::1")
        result = self.run_smoke("ok", addr=f"[::1]:{port}")
        self.assertEqual(result.returncode, 0, result.stderr + result.stdout)
        self.assertIn(f"on [::1]:{port}", result.stdout)

    @unittest.skipUnless(ipv6_loopback(), "this host has no IPv6 loopback")
    def test_facts_name_another_process_on_the_other_family(self):
        # The macOS case: localhost gives ::1 first, so the gateway holds only
        # [::1]. Another process on 127.0.0.1 then answers the IPv4 URL.
        port = free_port()
        self.serve_ok(socket.AF_INET, "127.0.0.1", port)
        result = self.run_smoke("ok", addr=f"localhost:{port}", SMOKE_LOCALHOST_BIND="::1")
        if f"answered on 127.0.0.1:{port}" in result.stderr:
            self.skipTest("curl on this host chose 127.0.0.1 for localhost")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn(f"listens on: [::1]:{port}", result.stdout)
        self.assertIn(f"fact: GET http://127.0.0.1:{port}/healthz: HTTP 200, body OK, "
                      f"from another process on 127.0.0.1:{port}", result.stdout)

    # Not ready.

    def test_503_until_deadline_is_not_ready(self):
        result = self.run_smoke("always-503", deadline="2")
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertIn("last: HTTP 503", result.stderr)

    def test_other_status_codes_are_refused(self):
        for stub in ("redirect", "no-content", "server-error"):
            with self.subTest(stub=stub):
                result = self.run_smoke(stub)
                self.assertEqual(result.returncode, 1, result.stderr)
                self.assertIn("Only 200 (ready) or 503 (not ready)", result.stderr)

    def test_other_body_is_refused(self):
        for stub in ("other", "newline"):
            with self.subTest(stub=stub):
                result = self.run_smoke(stub)
                self.assertEqual(result.returncode, 1, result.stderr)
                self.assertIn("not the two bytes OK", result.stderr)

    def test_lock_that_is_not_this_gateway_is_refused(self):
        cases = {
            "other-pid": "not gateway pid",
            "bad-role": "not gateway",
            "bad-addr": "addr is '127.0.0.1:1'",
            "none": "missing or is not JSON",
            "garbage": "missing or is not JSON",
        }
        for lock_mode, message in cases.items():
            with self.subTest(lock=lock_mode):
                result = self.run_smoke("ok", SMOKE_LOCK=lock_mode)
                self.assertEqual(result.returncode, 1, result.stderr)
                self.assertIn("host-process.lock", result.stderr)
                self.assertIn(message, result.stderr)
                self.assertNotIn("ready:", result.stdout)

    def test_ok_from_another_process_on_the_port_is_refused(self):
        self.serve_ok(socket.AF_INET, "127.0.0.1", self.port)
        result = self.run_smoke("sleep")
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertIn("not a listen socket of gateway pid", result.stderr)

    @unittest.skipUnless(ipv6_loopback(), "this host has no IPv6 loopback")
    def test_localhost_answer_on_another_family_is_refused(self):
        # The gateway holds 127.0.0.1 and another process holds [::1] on the
        # same port. curl tries ::1 first for localhost and gets that OK.
        port = free_port()
        try:
            self.serve_ok(socket.AF_INET6, "::1", port)
        except OSError:
            self.skipTest("[::1] port is not free")
        result = self.run_smoke("ok", addr=f"localhost:{port}",
                                SMOKE_LOCALHOST_BIND="127.0.0.1")
        if "on 127.0.0.1:" in result.stdout:
            self.skipTest("curl on this host chose 127.0.0.1 for localhost")
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn(f"answered on [::1]:{port}", result.stderr)

    def test_early_exit_is_not_ready(self):
        result = self.run_smoke("exit", deadline="5")
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertIn("exited before", result.stderr)

    def test_no_answer_until_deadline_is_not_ready(self):
        result = self.run_smoke("sleep", deadline="1")
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertIn("last: no answer", result.stderr)

    def test_proxy_cannot_supply_the_ok_body(self):
        proxy = socket.socket()
        proxy.bind(("127.0.0.1", 0))
        proxy.listen(1)
        self.addCleanup(proxy.close)

        def serve_proxy():
            try:
                connection, _ = proxy.accept()
            except OSError:
                return
            connection.sendall(b"HTTP/1.0 200 OK\r\nContent-Length: 2\r\n\r\nOK")
            connection.close()

        threading.Thread(target=serve_proxy, daemon=True).start()
        proxy_url = f"http://127.0.0.1:{proxy.getsockname()[1]}"
        result = self.run_smoke("other", http_proxy=proxy_url, HTTP_PROXY=proxy_url,
                                all_proxy=proxy_url, ALL_PROXY=proxy_url)
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("not the two bytes OK", result.stderr)

    # Input checks fail closed before the gateway starts.

    def test_deadline_must_be_a_whole_number_of_seconds(self):
        marker = Path(self.tmp.name) / "pwned"
        for value in ("abc", "-5", "", "0", "1.5", " 5", "10000", "07",
                      "x[$(touch %s)]" % marker):
            with self.subTest(value=value):
                result = self.run_smoke("ok", deadline=value)
                self.assertEqual(result.returncode, 2, result.stderr)
                self.assertIn("ELASTOS_GATEWAY_HEALTH_DEADLINE", result.stderr)
                self.assertFalse(marker.exists())

    def test_address_must_be_loopback_host_and_port(self):
        for addr in ("0.0.0.0:9", "example.com:80", "127.0.0.1:1@example.com:80",
                     "localhost", "localhost:0", "localhost:65536", "localhost:08090",
                     "[::]:8090", "127.0.0.2:8090", ""):
            with self.subTest(addr=addr):
                result = self.run_smoke("ok", addr=addr)
                self.assertEqual(result.returncode, 2, result.stderr)
                self.assertIn("loopback host:port", result.stderr)

    def test_missing_binary_exits_one(self):
        missing = Path(self.tmp.name) / "missing"
        result = subprocess.run(["bash", str(SMOKE), str(missing)],
                                capture_output=True, text=True, timeout=10)
        self.assertEqual(result.returncode, 1, result.stderr)

    # Cleanup stops the whole process group on every exit path.

    def assert_child_stopped(self, result):
        self.assertTrue(self.child_pid.exists(), result.stderr)
        self.assertTrue(process_gone(int(self.child_pid.read_text())),
                        "a gateway group member survived the smoke")

    def test_group_is_stopped_after_success(self):
        result = self.run_smoke("ok-fork")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assert_child_stopped(result)

    def test_group_is_stopped_after_timeout(self):
        result = self.run_smoke("fork", deadline="1")
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assert_child_stopped(result)

    def test_child_ignoring_term_is_killed(self):
        result = self.run_smoke("ignore-term", deadline="1")
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assert_child_stopped(result)

    def test_group_is_stopped_when_the_smoke_is_interrupted(self):
        proc = subprocess.Popen(["bash", str(SMOKE), str(self.binary)],
                                env=self.env("fork", deadline="60"),
                                stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        try:
            for _ in range(100):
                if self.child_pid.exists() and self.child_pid.read_text():
                    break
                time.sleep(0.1)
            proc.send_signal(signal.SIGTERM)
            _, stderr = proc.communicate(timeout=20)
        finally:
            if proc.poll() is None:
                proc.kill()
        self.assertEqual(proc.returncode, 143, stderr)
        self.assertTrue(process_gone(int(self.child_pid.read_text())))

    def test_log_is_kept_and_not_echoed(self):
        log = Path(self.tmp.name) / "gateway.log"
        result = self.run_smoke("secret", ELASTOS_GATEWAY_LOG=str(log))
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertNotIn("planted-secret", result.stdout)
        self.assertNotIn("planted-secret", result.stderr)
        self.assertIn("planted-secret", log.read_text())

    # The smoke follows the contract text, which is the one source of truth.

    def test_contract_names_the_command_and_url_this_smoke_uses(self):
        contract = (ROOT / "docs/LAUNCHER_CONTRACT.md").read_text()
        smoke = SMOKE.read_text()
        self.assertIn("elastos gateway --addr localhost:8090", contract)
        self.assertIn("GET http://localhost:8090/healthz", contract)
        self.assertIn("host-process.lock", contract)
        self.assertIn("ELASTOS_GATEWAY_ADDR-localhost:8090", smoke)
        mac = (ROOT / "docs/MAC.md").read_text()
        install = (ROOT / "scripts/install.sh").read_text()
        # This smoke is no Intel download. install.sh stops an Intel Mac in
        # detect_platform before release lookup, and MAC.md quotes that stop.
        message = "Intel Mac has no signed release yet. See docs/MAC.md for the source-home path."
        self.assertIn(f'[[ "$ARCH" == aarch64 ]] || die "{message}"', install)
        main = install.split("# ── Main", 1)[1]
        self.assertLess(main.index("detect_platform"), main.index("release-head.json"))
        self.assertIn(f"`{message}`", mac)
        self.assertNotIn("macOS release lookup requires Apple silicon", install + mac)


if __name__ == "__main__":
    unittest.main()
