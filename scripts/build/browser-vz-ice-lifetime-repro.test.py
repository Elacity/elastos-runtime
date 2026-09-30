#!/usr/bin/env python3
"""Source patch guards; native ownership proof is the separate Linux repro."""
import ast
import os
from pathlib import Path
import runpy
from types import SimpleNamespace
import unittest
from unittest.mock import patch

HERE = Path(__file__).parent
REPRO = runpy.run_path(str(HERE / "browser-vz-ice-lifetime-repro.py"))
PATCH = REPRO["load_patch"](HERE / "stage-browser-vm-target.sh")
BEFORE = REPRO["RETAINED_SOURCE"]


class StageIceLifetimeTests(unittest.TestCase):
    def test_patch_is_idempotent_and_preserves_surrounding_source(self):
        after = PATCH(BEFORE)
        ast.parse(after)
        self.assertEqual(PATCH(after), after)
        self.assertEqual(after.count('._ref()'), 1)
        self.assertEqual(after.count('self._elastos_vz_ice_agent = None'), 1)
        self.assertLess(after.index('self.webrtcbin = None'),
                        after.index('self._elastos_vz_ice_agent = None'))
        self.assertEqual(after.replace('        self._elastos_vz_ice_agent = None\n', '')
                         .split('    def stop(self):')[1], BEFORE.split('    def stop(self):')[1])

    def test_unexpected_or_duplicate_targets_fail(self):
        for source in [BEFORE.replace('self.pipeline.add(self.webrtcbin)', 'self.pipeline.add(other)'),
                       BEFORE.replace('logger.info("pipeline stopped")', 'logger.info("changed")'),
                       BEFORE + BEFORE,
                       BEFORE.replace('"ice-agent"', '"different-agent"')]:
            with self.subTest(source=source[:20]), self.assertRaises(SystemExit):
                PATCH(source)

    def exercise(self, refs, *, transport="vsock_v1", accepted=True, missing=False):
        events = []
        class Agent:
            __grefcount__ = refs
            def _ref(self):
                events.append("ref")
            def emit(self, signal, address):
                events.append((signal, address))
                return accepted
        agent = None if missing else Agent()
        class Bin:
            def set_property(self, key, value):
                events.append((key, value))
            def get_property(self, key):
                events.append(("get", key))
                return agent
            def set_state(self, state):
                events.append("bin-null")
        class Pipeline:
            def add(self, value):
                events.append("parented")
            def set_state(self, state):
                events.append("pipeline-null")
        gst = SimpleNamespace(Pipeline=SimpleNamespace(new=Pipeline),
                              ElementFactory=SimpleNamespace(make=lambda *_: Bin()),
                              State=SimpleNamespace(NULL=0))
        namespace = {"Gst": gst, "os": os, "GSTWebRTCAppError": RuntimeError,
                     "logger": SimpleNamespace(info=lambda *_: None)}
        exec(PATCH(BEFORE), namespace)
        peer = namespace["Probe"]()
        with patch.dict(os.environ, {"ELASTOS_BROWSER_VM_VZ_TRANSPORT": transport}):
            try:
                if missing or not accepted:
                    with self.assertRaises(RuntimeError):
                        peer.start()
                else:
                    peer.start()
            finally:
                peer.stop()
        self.assertIsNone(peer._elastos_vz_ice_agent)
        return events

    def test_missing_reference_is_added_once_after_parenting(self):
        events = self.exercise(1)
        self.assertEqual(events.count("ref"), 1)
        self.assertLess(events.index("parented"), events.index("ref"))
        self.assertLess(events.index("ref"), events.index(("add-local-ip-address", "127.0.0.1")))

    def test_already_owned_reference_is_preserved(self):
        for refs in (2, 3):
            with self.subTest(refs=refs):
                self.assertNotIn("ref", self.exercise(refs))

    def test_non_vz_path_never_reads_or_references_agent(self):
        events = self.exercise(1, transport="")
        self.assertNotIn(("get", "ice-agent"), events)
        self.assertNotIn("ref", events)

    def test_missing_agent_and_rejected_address_keep_teardown(self):
        for missing in (False, True):
            with self.subTest(missing=missing):
                events = self.exercise(1, accepted=False, missing=missing)
                self.assertIn("pipeline-null", events)
                self.assertIn("bin-null", events)
                self.assertEqual(events.count("ref"), 0 if missing else 1)


if __name__ == "__main__":
    unittest.main()
