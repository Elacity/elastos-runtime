#!/usr/bin/env python3
"""Source and lifecycle checks. Real Gst/PyGI validation belongs to the builder."""

import hashlib
import json
import os
from pathlib import Path
import runpy
import struct
from types import SimpleNamespace
import unittest
from unittest.mock import patch


HELPER = runpy.run_path(str(Path(__file__).with_name("browser-audio-queue-diagnostic.py")))
FIXTURE = '''import os, json, time, asyncio
class GSTWebRTCApp:
    def build_audio_pipeline(self):
        rtpopuspay_queue = self.queue
        rtpopuspay_queue.set_property("leaky", "downstream")
        rtpopuspay_queue.set_property("max-size-time", 16000000)
    # [END build_audio_pipeline]
    async def handle_bus_calls(self):
        running = True
        while running:
            await asyncio.sleep(0.1)
    def stop_pipeline(self):
        logger.info("stopping pipeline")
'''
FIXTURE_SHA = hashlib.sha256(FIXTURE.encode()).hexdigest()
GST = SimpleNamespace(
    PadProbeType=SimpleNamespace(BUFFER=1, BUFFER_LIST=2, EVENT_DOWNSTREAM=4, EVENT_FLUSH=8),
    PadProbeReturn=SimpleNamespace(OK=0),
    EventType=SimpleNamespace(EOS=1, FLUSH_START=2, FLUSH_STOP=3))


class Pad:
    def __init__(self):
        self.callbacks = {}

    def add_probe(self, mask, callback, side):
        self.callbacks[1] = (mask, callback, side)
        return 1

    def remove_probe(self, probe):
        del self.callbacks[probe]


class Queue:
    def __init__(self):
        self.pads = {name: Pad() for name in ("sink", "src")}
        self.handlers = {}
        self.properties = {}

    def get_static_pad(self, side):
        return self.pads[side]

    def connect(self, name, callback):
        self.handlers[1] = callback
        return 1

    def disconnect(self, handler):
        del self.handlers[handler]

    def get_property(self, name):
        return 0

    def set_property(self, name, value):
        self.properties[name] = value

    def get_name(self):
        return "queue-test"


class Buffer:
    def __init__(self, seq, ssrc=42):
        self.data = struct.pack("!BBHII", 128, 111, seq, seq * 480, ssrc)

    def get_size(self):
        return len(self.data)

    def extract_dup(self, offset, length):
        return self.data[offset:offset + length]


class DiagnosticTests(unittest.TestCase):
    def setUp(self):
        self.now = 100.0
        self.rows = []
        self.clock = SimpleNamespace(monotonic=lambda: self.now, time=lambda: self.now + 1000)
        self.log = SimpleNamespace(info=lambda fmt, *args:
            self.rows.append(json.loads(args[0])) if args else None)
        self.namespace = dict(time=self.clock, json=json)
        exec(HELPER["OBSERVER"], self.namespace)
        self.queue = Queue()
        self.diag = self.namespace["_ElastosAudioQueueDiagnostic"](self.queue, GST, self.log)

    def packet(self, side, seq, ssrc=42):
        info = SimpleNamespace(type=1, get_buffer=lambda: Buffer(seq, ssrc))
        self.assertEqual(self.diag.observe(None, info, side), GST.PadProbeReturn.OK)

    def event(self, side, kind):
        info = SimpleNamespace(type=4, get_event=lambda: SimpleNamespace(type=kind))
        self.diag.observe(None, info, side)

    def assert_detached(self):
        self.assertFalse(self.queue.handlers)
        self.assertTrue(all(not pad.callbacks for pad in self.queue.pads.values()))

    def test_unblocked_and_downstream_holes_are_distinguished(self):
        for seq in range(100):
            self.packet("sink", seq)
            if seq % 5 == 0 or seq == 99:
                self.packet("src", seq)
            else:
                self.diag.overrun()
        self.diag.stop()
        row = self.rows[-1]
        self.assertEqual((row["sink"]["packets"], row["sink"]["forward_gaps"]), (100, 0))
        self.assertEqual((row["src"]["packets"], row["src"]["forward_gaps"]), (21, 79))
        self.assertEqual(row["overrun"], 79)
        self.assert_detached()

    def test_unblocked_100_packets_preserve_both_sequences(self):
        for seq in range(100):
            self.packet("sink", seq)
            self.packet("src", seq)
        self.diag.stop()
        for side in ("sink", "src"):
            self.assertEqual(self.rows[-1][side]["packets"], 100)
            self.assertEqual(self.rows[-1][side]["forward_gaps"], 0)
        self.assertEqual(self.rows[-1]["overrun"], 0)

    def test_wrap_duplicates_reorder_and_ssrc_change_do_not_become_huge_loss(self):
        for seq in (65534, 65535, 0, 0, 65535, 1):
            self.packet("sink", seq)
            self.packet("src", seq)
        self.packet("src", 120, ssrc=43)
        state = self.diag.sides["src"]
        self.assertEqual(state["forward_gaps"], 0)
        self.assertEqual((state["duplicates"], state["reordered"], state["ssrc_changes"]), (1, 1, 1))

    def test_eos_and_flush_are_separate_from_active_loss(self):
        self.packet("src", 0)
        self.event("sink", GST.EventType.EOS)
        self.diag.overrun()
        self.packet("src", 5)
        self.event("sink", GST.EventType.FLUSH_START)
        self.event("src", GST.EventType.FLUSH_STOP)
        self.packet("src", 1000)
        state = self.diag.sides["src"]
        self.assertEqual(state["forward_gaps"], 0)
        self.assertEqual((state["boundary_gaps"], state["boundary_packets"]), (4, 2))
        self.assertEqual(self.diag.sides["sink"]["eos"], 1)
        self.assertEqual(self.diag.boundary_overruns, 1)

    def test_buffer_list_and_malformed_header(self):
        values = [Buffer(0), Buffer(1)]
        buffers = SimpleNamespace(length=lambda: 2, get=lambda index: values[index])
        self.diag.observe(None, SimpleNamespace(type=2, get_buffer_list=lambda: buffers), "sink")
        bad = Buffer(2)
        bad.data = b"short"
        self.diag.observe(None, SimpleNamespace(type=1, get_buffer=lambda: bad), "sink")
        self.assertEqual(self.diag.sides["sink"]["packets"], 2)
        self.assertEqual(self.diag.sides["sink"]["malformed"], 1)

    def test_one_hz_timeout_bounds_and_idempotent_stop(self):
        for tenth in range(1, 601):
            self.now = 100.0 + tenth / 10
            self.diag.tick()
        self.assertLessEqual(len(self.rows), 60)
        self.assertEqual(self.rows[-1]["reason"], "timeout")
        self.assert_detached()
        before = len(self.rows)
        self.diag.stop()
        self.diag.tick()
        self.assertEqual(len(self.rows), before)
        self.assertLess(sum(len(json.dumps(row)) for row in self.rows), 128 * 1024)

    def test_delayed_bus_does_not_extend_measurement_or_catch_up_logs(self):
        self.packet("sink", 0)
        self.now += 61
        self.packet("sink", 1)
        self.diag.overrun()
        self.assertEqual(self.diag.sides["sink"]["packets"], 1)
        self.diag.tick()
        self.assertEqual(len(self.rows), 1)
        self.assert_detached()

    def test_probe_error_passes_media_and_detaches_at_next_tick(self):
        def broken():
            raise RuntimeError("test detail must not enter logs")
        self.assertEqual(self.diag.observe(None, SimpleNamespace(type=1, get_buffer=broken), "sink"), 0)
        self.diag.tick()
        self.assertEqual(self.rows[-1]["error"], "probe_failed")
        self.assert_detached()

    def test_patcher_is_exact_idempotent_and_rejects_tampering(self):
        apply = HELPER["patch_source"]
        result = apply(FIXTURE, FIXTURE_SHA)
        self.assertEqual(apply(result, FIXTURE_SHA), result)
        for source in (FIXTURE + "\n", result.replace("self.deadline = self.started + 60.0", "self.deadline = self.started + 600.0"),
                       result.replace(HELPER["STOP_HOOK"], "")):
            with self.subTest(source=source[:30]), self.assertRaises(ValueError):
                apply(source, FIXTURE_SHA)
        self.assertEqual([line for line in FIXTURE.splitlines() if ".set_property(" in line],
                         [line for line in result.splitlines() if ".set_property(" in line])

    def test_opt_in_is_exact_and_stop_hook_detaches(self):
        source = HELPER["patch_source"](FIXTURE, FIXTURE_SHA)
        for enabled in (None, "0", "true", "1"):
            namespace = dict(Gst=GST, logger=self.log)
            exec(source, namespace)
            queue = Queue()
            app = namespace["GSTWebRTCApp"]()
            app.queue = queue
            with patch.dict(os.environ, {}, clear=True):
                if enabled is not None:
                    os.environ["ELASTOS_BROWSER_AUDIO_QUEUE_DIAGNOSTIC"] = enabled
                app.build_audio_pipeline()
            self.assertEqual(bool(queue.handlers), enabled == "1")
            self.assertEqual(queue.properties, {"leaky": "downstream", "max-size-time": 16000000})
            app.stop_pipeline()
            self.assertFalse(queue.handlers)
            self.assertTrue(all(not pad.callbacks for pad in queue.pads.values()))


if __name__ == "__main__":
    unittest.main()
