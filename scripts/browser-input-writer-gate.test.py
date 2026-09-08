import ast
import importlib.util
import json
from pathlib import Path
import socket
import tempfile
import threading
import unittest
from types import SimpleNamespace

spec = importlib.util.spec_from_file_location("writer", Path(__file__).with_name("browser-input-writer-gate.py"))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)

class WriterTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.events = []
        self.now = 1.0
        self.native = SimpleNamespace(on_message=self.events.append,
            send_x11_keypress=lambda key, down:self.events.append(f"ku,{key}"),
            send_x11_mouse=lambda x,y,mask,magnitude,relative:self.events.append(("pointer",x,y,mask,magnitude,relative)),
            xdisplay=SimpleNamespace(sync=lambda:self.events.append("synced")))
        self.gate = module.InputWriterGate(self.native, self.directory.name + "/gate.sock", lambda: self.now)
        self.lease = "a" * 32

    def tearDown(self):
        self.gate.close()
        self.assertFalse(self.gate.thread.is_alive())
        self.directory.cleanup()

    def command(self, command, lease=None, **values):
        request = {"command":command, "admission_id":lease or self.lease, **values}
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as client:
            client.settimeout(2)
            client.connect(self.gate.path)
            client.sendall(json.dumps(request).encode() + b"\n")
            return json.loads(client.recv(2048))

    def test_real_socket_handoff_settles_keys_and_pointer_then_preserves_pong(self):
        self.gate.on_message("kd,97")
        self.gate.on_message("m,10,20,1,0")
        self.assertTrue(self.command("acquire",duration_ms=30000)["active"])
        self.assertEqual(self.events[-3:], ["ku,97", ("pointer",0,0,0,0,True), "synced"])
        before = len(self.events)
        for message in ["kd,98", "m,0,0,1,0", "cw,secret", "kr"]:
            self.gate.on_message(message)
        self.assertEqual(len(self.events), before)
        self.gate.on_message("pong")
        self.assertEqual(self.events[-1], "pong")
        self.assertFalse(self.command("release")["active"])
        self.gate.on_message("kd,99")
        self.assertEqual(self.events[-1], "kd,99")

    def test_ack_waits_for_prior_native_input_call(self):
        entered, release, acquired = threading.Event(), threading.Event(), threading.Event()
        def handler(message):
            if message == "kd,97":
                entered.set()
                self.assertTrue(release.wait(1))
            self.events.append(message)
        self.gate.handler = handler
        native = threading.Thread(target=lambda:self.gate.on_message("kd,97"))
        native.start(); self.assertTrue(entered.wait(1))
        result = []
        def acquire():
            result.append(self.command("acquire", duration_ms=30000)); acquired.set()
        waiter = threading.Thread(target=acquire); waiter.start()
        self.assertFalse(acquired.wait(.03))
        release.set(); native.join(1); waiter.join(1)
        self.assertTrue(result[0]["active"])
        self.assertEqual(self.events[-3:], ["ku,97", ("pointer",0,0,0,0,True), "synced"])

    def test_expiry_is_monotonic_and_same_id_cannot_extend_or_reacquire(self):
        self.command("acquire",duration_ms=2000)
        self.now += 1
        self.command("acquire",duration_ms=30000)
        self.now += 1
        self.assertFalse(self.command("check")["active"])
        self.assertIn("error",self.command("acquire",duration_ms=30000))
        self.gate.on_message("kd,97")
        self.assertEqual(self.events[-1], "kd,97")

    def test_foreign_release_does_not_release_owner_and_future_acquire_is_denied(self):
        self.command("acquire",duration_ms=30000)
        self.command("release",lease="b"*32)
        self.assertTrue(self.command("check")["active"])
        self.assertIn("error",self.command("acquire",lease="b"*32,duration_ms=30000))

    def test_failed_native_settlement_does_not_acknowledge_writer(self):
        self.native.send_x11_mouse = lambda *args, **kwargs: (_ for _ in ()).throw(RuntimeError("native failure"))
        self.assertIn("error",self.command("acquire",duration_ms=30000))
        self.assertIsNone(self.gate.lease)

    def test_late_native_settlement_cannot_start_a_fresh_lease(self):
        self.native.xdisplay.sync = lambda:setattr(self,"now",self.now + 31)
        self.assertIn("error",self.command("acquire",duration_ms=30000))
        self.assertIsNone(self.gate.lease)
        self.assertIn(self.lease,self.gate.retired)

    def test_pending_native_effect_survives_expiry_and_release_until_exact_ack(self):
        effect = "c" * 32
        self.command("acquire", duration_ms=2000)
        self.assertEqual(self.command("begin", effect_id=effect)["pending_effect"], effect)
        before = len(self.events)
        self.now += 3
        status = self.command("check")
        self.assertFalse(status["active"])
        self.assertTrue(status["held"])
        self.gate.on_message("kd,98")
        self.assertEqual(len(self.events), before)
        released = self.command("release")
        self.assertTrue(released["held"], "release cannot certify unresolved native input")
        self.assertEqual(released["pending_effect"], effect)
        self.gate.on_message("m,0,0,1,0")
        self.assertEqual(len(self.events), before)
        self.assertIn("error", self.command("settle", effect_id="d" * 32))
        self.assertIn("error", self.command("settle", lease="b" * 32, effect_id=effect))
        self.assertTrue(self.command("check")["held"])
        settled = self.command("settle", effect_id=effect)
        self.assertFalse(settled["held"])
        self.gate.on_message("kd,99")
        self.assertEqual(self.events[-1], "kd,99")
        self.assertIn("error", self.command("acquire", duration_ms=30000))

    def test_pending_effect_blocks_new_lease_even_when_original_authority_expired(self):
        self.command("acquire", duration_ms=2000)
        self.command("begin", effect_id="c" * 32)
        self.now += 31
        self.assertIn("error", self.command("acquire", lease="b" * 32, duration_ms=30000))
        self.assertTrue(self.command("release")["held"])
        self.gate.on_message("pong")
        self.assertEqual(self.events[-1], "pong")

    def test_producer_restart_preserves_pending_effect_hold_and_exact_late_settlement(self):
        effect = "c" * 32
        self.command("acquire", duration_ms=2000)
        self.command("begin", effect_id=effect)
        pending_path = Path(self.gate.pending_path)
        self.assertEqual(pending_path.stat().st_mode & 0o777, 0o600)
        self.gate.close()
        self.assertTrue(pending_path.exists())
        self.gate = module.InputWriterGate(self.native, self.directory.name + "/gate.sock", lambda:self.now)
        before = len(self.events)
        self.gate.on_message("kd,98")
        self.assertEqual(len(self.events), before)
        self.assertFalse(self.command("check")["active"])
        self.assertTrue(self.command("release")["held"])
        self.assertFalse(self.command("settle", effect_id=effect)["held"])
        self.assertFalse(pending_path.exists())
        self.gate.on_message("kd,99")
        self.assertEqual(self.events[-1], "kd,99")

    def test_uinput_backend_cannot_claim_x_server_handoff(self):
        self.native.uinput_mouse_socket_path = "/unavailable-native-input"
        self.assertIn("error",self.command("acquire",duration_ms=30000))
        self.assertIsNone(self.gate.lease)
        self.assertEqual(self.events, [])

    def test_bounds_and_socket_permissions(self):
        self.assertEqual(Path(self.gate.path).stat().st_mode & 0o777,0o600)
        for duration in [True,0,1999,30001]:
            self.assertIn("error",self.command("acquire",duration_ms=duration))
        for number in range(256):
            self.command("release",lease=f"{number:032x}")
        self.assertLessEqual(len(self.gate.retired),128)
        self.assertIn("error",self.command("acquire",duration_ms=30000))

class StagePatchTests(unittest.TestCase):
    def test_stage_wires_real_native_input_and_cleanup_once_or_fails_closed(self):
        source = (Path(__file__).parent / "build/stage-browser-vm-target.sh").read_text()
        start = source.index("def patch_selkies_input_writer(source):")
        stop = source.index("\nmain_text = patch_selkies_input_writer", start)
        namespace = {}
        exec(compile(ast.parse(source[start:stop]), "stage-input-writer-patch", "exec"), namespace)
        patch = namespace["patch_selkies_input_writer"]
        upstream = "import sys\n    app.on_data_message = webrtc_input.on_message\n        webrtc_input.disconnect()\n"
        candidate = patch(upstream)
        self.assertEqual(candidate, patch(candidate))
        self.assertIn('InputWriterGate(webrtc_input, "/run/elastos/browser-input-writer.sock")', candidate)
        self.assertLess(candidate.index("input_writer.close()"), candidate.index("webrtc_input.disconnect()"))
        for line in upstream.splitlines(keepends=True):
            with self.assertRaises(SystemExit):
                patch(upstream.replace(line, ""))
            with self.assertRaises(SystemExit):
                patch(upstream + line)
        with self.assertRaises(SystemExit):
            patch(candidate.replace("        input_writer.close()\n", ""))

if __name__ == "__main__":
    unittest.main()
