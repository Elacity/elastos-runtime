#!/usr/bin/env python3
"""Patch the in-image Selkies module for an opt-in webrtcbin sink experiment.

Apply only to a task rootfs clone. The observer counts RTP bytes that enter
webrtcbin after the audio-queue src. It changes no media properties and
collects no PCM, payload, ICE credentials, or addresses.
"""

import argparse
import hashlib
import json
import os
from pathlib import Path
import tempfile


# gstwebrtc_app.py from installed image 76967c5b, which already has the
# audio-queue diagnostic.
BASE_SHA256 = "d1813fc03015077d0c96e697123c53115220d870d44616431594bbc77fda9168"
MARKER = "# ElastOS bounded webrtc send diagnostic v1"
OBSERVER = r'''
# ElastOS bounded webrtc send diagnostic v1
class _ElastosWebrtcSendDiagnostic:
    def __init__(self, webrtcbin, gst, log):
        import threading
        self.webrtcbin, self.gst, self.log = webrtcbin, gst, log
        self.lock = threading.Lock()
        self.started = time.monotonic()
        self.deadline = self.started + 60.0
        self.next_report = self.started + 0.2
        self.closed = False
        self.rows = 0
        self.error = None
        self.pads = {}
        self.probes = []
        try:
            mask = (gst.PadProbeType.BUFFER | gst.PadProbeType.BUFFER_LIST)
            iterator = webrtcbin.iterate_sink_pads()
            while True:
                result, pad = iterator.next()
                if result != gst.IteratorResult.OK:
                    break
                name = pad.get_name()
                self.pads[name] = dict(packets=0, bytes=0)
                self.probes.append((pad, pad.add_probe(mask, self.observe, name)))
            if not self.probes:
                self.error = "no_sink_pads"
        except Exception:
            self.error = "attach_failed"

    def observe(self, _pad, info, name):
        try:
            with self.lock:
                if self.closed or time.monotonic() >= self.deadline:
                    return self.gst.PadProbeReturn.OK
                state = self.pads.setdefault(name, dict(packets=0, bytes=0))
                if info.type & self.gst.PadProbeType.BUFFER_LIST:
                    buffers = info.get_buffer_list()
                    for index in range(buffers.length()):
                        size = buffers.get(index).get_size()
                        state["packets"] += 1
                        state["bytes"] += size
                elif info.type & self.gst.PadProbeType.BUFFER:
                    size = info.get_buffer().get_size()
                    state["packets"] += 1
                    state["bytes"] += size
        except Exception:
            with self.lock:
                self.error = "probe_failed"
        return self.gst.PadProbeReturn.OK

    def tick(self):
        now = time.monotonic()
        if self.closed:
            return
        if self.error or now >= self.deadline:
            self.stop("error" if self.error else "timeout")
        elif now >= self.next_report:
            self.next_report = now + 0.2
            self.report("sample")

    def report(self, reason):
        with self.lock:
            if self.rows >= 301:
                return
            self.rows += 1
            row = dict(schema="elastos.browser.webrtc-send-diagnostic/v1",
                reason=reason, row=self.rows, wall_time_ms=time.time() * 1000,
                elapsed_ms=(time.monotonic() - self.started) * 1000,
                measurement_limit_ms=60000, error=self.error,
                pads={name: dict(state) for name, state in self.pads.items()})
        self.log.info("webrtc_send_diagnostic %s", json.dumps(row, separators=(",", ":")))

    def stop(self, reason="stop"):
        with self.lock:
            if self.closed:
                return
            self.closed = True
        for pad, probe in self.probes:
            pad.remove_probe(probe)
        self.probes.clear()
        self.report(reason)
'''

AUDIO_HOOK = '''        if os.environ.get("ELASTOS_BROWSER_WEBRTC_SEND_DIAGNOSTIC") == "1":
            self._webrtc_send_diagnostic = _ElastosWebrtcSendDiagnostic(self.webrtcbin, Gst, logger)
'''
BUS_HOOK = '''            webrtc = getattr(self, "_webrtc_send_diagnostic", None)
            if webrtc is not None:
                webrtc.tick() if running else webrtc.stop("bus_stop")
'''
STOP_HOOK = '''        webrtc = getattr(self, "_webrtc_send_diagnostic", None)
        if webrtc is not None:
            webrtc.stop()
'''
HOOKS = (
    ("    # [END build_audio_pipeline]\n", AUDIO_HOOK),
    ("            await asyncio.sleep(0.1)\n", BUS_HOOK),
    ('        logger.info("stopping pipeline")\n', STOP_HOOK),
)


def patch_source(source, expected_sha256=BASE_SHA256):
    original = source
    if MARKER in source:
        if not source.endswith(OBSERVER):
            raise ValueError("diagnostic payload differs from this patcher")
        source = source[:-len(OBSERVER)]
        for anchor, hook in HOOKS:
            if source.count(hook + anchor) != 1:
                raise ValueError("incomplete diagnostic hook")
            source = source.replace(hook + anchor, anchor, 1)
    if hashlib.sha256(source.encode()).hexdigest() != expected_sha256:
        raise ValueError("Selkies input SHA-256 differs from the in-image source")
    for anchor, hook in HOOKS:
        if source.count(anchor) != 1:
            raise ValueError("Selkies webrtc diagnostic insertion point changed")
        source = source.replace(anchor, hook + anchor, 1)
    result = source + OBSERVER
    compile(result, "gstwebrtc_app.py", "exec")
    if MARKER in original and original != result:
        raise ValueError("diagnostic patch is inconsistent")
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("path", type=Path, help="gstwebrtc_app.py inside the task rootfs clone")
    args = parser.parse_args()
    before = args.path.read_bytes()
    after = patch_source(before.decode()).encode()
    if after != before:
        temporary = None
        try:
            with tempfile.NamedTemporaryFile(dir=args.path.parent, delete=False) as handle:
                temporary = Path(handle.name)
                handle.write(after)
                handle.flush()
                os.fsync(handle.fileno())
            os.chmod(temporary, args.path.stat().st_mode & 0o777)
            os.replace(temporary, args.path)
        finally:
            if temporary is not None and temporary.exists():
                temporary.unlink()
    print(json.dumps(dict(schema="elastos.browser.webrtc-send-patch/v1",
        input_sha256=hashlib.sha256(before).hexdigest(),
        output_sha256=hashlib.sha256(after).hexdigest(), changed=after != before,
        enabled_by_default=False, max_seconds=60, interval_ms=200)))


if __name__ == "__main__":
    main()
