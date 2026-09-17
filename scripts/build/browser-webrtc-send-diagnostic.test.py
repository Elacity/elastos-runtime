#!/usr/bin/env python3
"""Source checks for the webrtcbin sink diagnostic. Real Gst belongs to the builder."""

import hashlib
import json
from pathlib import Path
import runpy
import unittest
from types import SimpleNamespace


HELPER = runpy.run_path(str(Path(__file__).with_name("browser-webrtc-send-diagnostic.py")))
DUMPED = Path("/Users/anders/Code/elastos-runtime/.git/browser-analysis/u9-r8-gstwebrtc-app-76967c5b.py")


class Pad:
    def __init__(self, name):
        self.name = name
        self.callbacks = {}

    def get_name(self):
        return self.name

    def add_probe(self, mask, callback, name):
        self.callbacks[1] = (mask, callback, name)
        return 1

    def remove_probe(self, probe):
        del self.callbacks[probe]


GST = SimpleNamespace(
    PadProbeType=SimpleNamespace(BUFFER=1, BUFFER_LIST=2),
    PadProbeReturn=SimpleNamespace(OK=0),
    IteratorResult=SimpleNamespace(OK=1, DONE=2),
)


class PadIterator:
    def __init__(self, pads):
        self.pads = list(pads)

    def next(self):
        if not self.pads:
            return GST.IteratorResult.DONE, None
        return GST.IteratorResult.OK, self.pads.pop(0)


class Webrtc:
    def __init__(self, pads):
        self._pads = pads

    def iterate_sink_pads(self):
        return PadIterator(self._pads)


class Buffer:
    def __init__(self, size):
        self._size = size

    def get_size(self):
        return self._size


class TestPatch(unittest.TestCase):
    def test_dumped_image_source_matches_pin(self):
        text = DUMPED.read_text()
        self.assertEqual(hashlib.sha256(text.encode()).hexdigest(), HELPER["BASE_SHA256"])

    def test_patch_is_idempotent_on_dumped_source(self):
        text = DUMPED.read_text()
        once = HELPER["patch_source"](text)
        twice = HELPER["patch_source"](once)
        self.assertEqual(once, twice)
        self.assertIn("webrtc_send_diagnostic", once)
        self.assertIn("ELASTOS_BROWSER_WEBRTC_SEND_DIAGNOSTIC", once)
        compile(once, "gstwebrtc_app.py", "exec")

    def test_observer_counts_sink_bytes(self):
        pad = Pad("sink_1")
        webrtc = Webrtc([pad])
        log = SimpleNamespace(rows=[], info=lambda tag, payload: log.rows.append((tag, json.loads(payload))))
        ns = {}
        exec("import time, json\n" + HELPER["OBSERVER"], {
            "time": __import__("time"),
            "json": __import__("json"),
        }, ns)
        diagnostic = ns["_ElastosWebrtcSendDiagnostic"](webrtc, GST, log)
        self.assertEqual(list(diagnostic.pads), ["sink_1"])
        info = SimpleNamespace(type=GST.PadProbeType.BUFFER, get_buffer=lambda: Buffer(120))
        self.assertEqual(diagnostic.observe(pad, info, "sink_1"), GST.PadProbeReturn.OK)
        diagnostic.report("sample")
        self.assertTrue(log.rows[-1][0].startswith("webrtc_send_diagnostic"))
        self.assertEqual(log.rows[-1][1]["pads"]["sink_1"]["bytes"], 120)
        self.assertEqual(log.rows[-1][1]["pads"]["sink_1"]["packets"], 1)
        diagnostic.stop()
        self.assertEqual(pad.callbacks, {})


if __name__ == "__main__":
    unittest.main()
