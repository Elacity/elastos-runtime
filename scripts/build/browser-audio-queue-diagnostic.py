#!/usr/bin/env python3
"""Patch a reviewed Selkies module for an opt-in, bounded RTP queue experiment.

Apply only to a task rootfs clone. The injected observer is self-contained;
ELASTOS_BROWSER_AUDIO_QUEUE_DIAGNOSTIC=1 enables it in the Selkies process.
It changes no media properties and collects no PCM, payload, or credentials.
"""

import argparse
import hashlib
import json
import os
from pathlib import Path
import tempfile


BASE_SHA256 = "3f17ebaa8f01215788247696f25f8348cc846088f44b5f0d6e063bde3aeafc94"
MARKER = "# ElastOS bounded audio queue diagnostic v1"
OBSERVER = r'''
# ElastOS bounded audio queue diagnostic v1
class _ElastosAudioQueueDiagnostic:
    def __init__(self, queue, gst, log):
        import threading
        self.queue, self.gst, self.log = queue, gst, log
        self.lock = threading.Lock()
        self.started = time.monotonic()
        self.deadline = self.started + 60.0
        self.next_report = self.started + 1.0
        self.closed = False
        self.rows = 0
        self.phase = "active"
        self.overruns = 0
        self.boundary_overruns = 0
        self.error = None
        self.last = {}
        self.sides = {side: dict(packets=0, bytes=0, forward_gaps=0,
            duplicates=0, reordered=0, ssrc_changes=0, malformed=0,
            boundary_packets=0, boundary_gaps=0, eos=0, flush_start=0,
            flush_stop=0, first_seq=None, last_seq=None, ssrc=None,
            rtp_timestamp=None) for side in ("sink", "src")}
        self.probes = []
        self.handler = None
        try:
            mask = (gst.PadProbeType.BUFFER | gst.PadProbeType.BUFFER_LIST |
                    gst.PadProbeType.EVENT_DOWNSTREAM | gst.PadProbeType.EVENT_FLUSH)
            for side in ("sink", "src"):
                pad = queue.get_static_pad(side)
                self.probes.append((pad, pad.add_probe(mask, self.observe, side)))
            self.handler = queue.connect("overrun", self.overrun)
        except Exception:
            self.error = "attach_failed"
            self.stop("error")

    def overrun(self, *_args):
        with self.lock:
            if not self.closed and time.monotonic() < self.deadline:
                self.overruns += 1
                if self.phase != "active":
                    self.boundary_overruns += 1

    def packet(self, buffer, side):
        # Read only the fixed RTP header, as in the matching-GStreamer repro.
        state = self.sides[side]
        size = buffer.get_size()
        if size < 12:
            state["malformed"] += 1
            return
        header = buffer.extract_dup(0, 12)
        if header[0] >> 6 != 2:
            state["malformed"] += 1
            return
        seq = int.from_bytes(header[2:4], "big")
        stamp = int.from_bytes(header[4:8], "big")
        ssrc = int.from_bytes(header[8:12], "big")
        state["packets"] += 1
        state["bytes"] += size
        if self.phase != "active":
            state["boundary_packets"] += 1
        previous = self.last.get(side)
        if previous is None or previous[0] != ssrc:
            if previous is not None:
                state["ssrc_changes"] += 1
            self.last[side] = (ssrc, seq)
        else:
            step = (seq - previous[1]) & 65535
            if step == 0:
                state["duplicates"] += 1
            elif step < 32768:
                key = "forward_gaps" if self.phase == "active" else "boundary_gaps"
                state[key] += step - 1
                self.last[side] = (ssrc, seq)
            else:
                state["reordered"] += 1
        if state["first_seq"] is None:
            state["first_seq"] = seq
        state.update(last_seq=seq, ssrc=ssrc, rtp_timestamp=stamp)

    def observe(self, _pad, info, side):
        try:
            with self.lock:
                # A delayed bus loop cannot extend the measurement window.
                if self.closed or time.monotonic() >= self.deadline:
                    return self.gst.PadProbeReturn.OK
                if info.type & (self.gst.PadProbeType.EVENT_DOWNSTREAM |
                                self.gst.PadProbeType.EVENT_FLUSH):
                    event = info.get_event().type
                    names = {self.gst.EventType.EOS: "eos",
                             self.gst.EventType.FLUSH_START: "flush_start",
                             self.gst.EventType.FLUSH_STOP: "flush_stop"}
                    if event in names:
                        name = names[event]
                        self.sides[side][name] += 1
                        self.phase = name
                        # Flush starts a new sequence interval, not packet loss.
                        if name.startswith("flush"):
                            self.last.clear()
                elif info.type & self.gst.PadProbeType.BUFFER_LIST:
                    buffers = info.get_buffer_list()
                    for index in range(buffers.length()):
                        self.packet(buffers.get(index), side)
                elif info.type & self.gst.PadProbeType.BUFFER:
                    self.packet(info.get_buffer(), side)
        except Exception:
            # The observer cannot block or reject media on a diagnostic error.
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
            self.next_report = now + 1.0
            self.report("sample")

    def report(self, reason):
        with self.lock:
            if self.rows >= 61:
                return
            self.rows += 1
            row = dict(schema="elastos.browser.audio-queue-diagnostic/v1",
                reason=reason, row=self.rows, wall_time_ms=time.time() * 1000,
                monotonic_ms=time.monotonic() * 1000,
                elapsed_ms=(time.monotonic() - self.started) * 1000,
                measurement_limit_ms=60000, phase=self.phase,
                overrun=self.overruns, boundary_overrun=self.boundary_overruns,
                error=self.error,
                sink=dict(self.sides["sink"]), src=dict(self.sides["src"]))
        # Queue getters and logging run outside the streaming callback lock.
        try:
            row["levels"] = {name: self.queue.get_property(name) for name in
                ("current-level-buffers", "current-level-bytes", "current-level-time")}
        except Exception:
            row["levels"] = None
        row["queue"] = self.queue.get_name()
        self.log.info("audio_queue_diagnostic %s", json.dumps(row, separators=(",", ":")))

    def stop(self, reason="stop"):
        with self.lock:
            if self.closed:
                return
            self.closed = True
        # Disconnect outside our lock: callbacks can be on streaming threads.
        for pad, probe in self.probes:
            pad.remove_probe(probe)
        self.probes.clear()
        if self.handler is not None:
            self.queue.disconnect(self.handler)
            self.handler = None
        self.report(reason)
'''

AUDIO_HOOK = '''        if os.environ.get("ELASTOS_BROWSER_AUDIO_QUEUE_DIAGNOSTIC") == "1":
            self._audio_queue_diagnostic = _ElastosAudioQueueDiagnostic(rtpopuspay_queue, Gst, logger)
'''
BUS_HOOK = '''            diagnostic = getattr(self, "_audio_queue_diagnostic", None)
            if diagnostic is not None:
                diagnostic.tick() if running else diagnostic.stop("bus_stop")
'''
STOP_HOOK = '''        diagnostic = getattr(self, "_audio_queue_diagnostic", None)
        if diagnostic is not None:
            diagnostic.stop()
'''
HOOKS = (
    ("    # [END build_audio_pipeline]\n", AUDIO_HOOK),
    ("            await asyncio.sleep(0.1)\n", BUS_HOOK),
    ('        logger.info("stopping pipeline")\n', STOP_HOOK),
)


def patch_source(source, expected_sha256=BASE_SHA256):
    """Reject unknown or partial inputs, including modified previous patches."""
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
        raise ValueError("Selkies input SHA-256 differs from reviewed source")
    for anchor, hook in HOOKS:
        if source.count(anchor) != 1:
            raise ValueError("Selkies diagnostic insertion point changed")
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
    print(json.dumps(dict(schema="elastos.browser.audio-queue-patch/v1",
        input_sha256=hashlib.sha256(before).hexdigest(),
        output_sha256=hashlib.sha256(after).hexdigest(), changed=after != before,
        enabled_by_default=False, max_seconds=60)))


if __name__ == "__main__":
    main()
