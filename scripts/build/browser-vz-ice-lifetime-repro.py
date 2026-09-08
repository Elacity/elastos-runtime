#!/usr/bin/env python3
"""Compare retained and Python-owned VZ ICE references on an isolated Linux guest.

Uses installed GI/GStreamer packages, two webrtcbin instances, and loopback only.
Does not start Selkies, Chromium, TURN, a display, or a Runtime. Each case runs in
a bounded child with core dumps disabled. No packages or installed files change.

Ownership sources (the reference restored by _ref is released by webrtcbin;
the separate Python wrapper reference is released when its attribute is cleared):
https://github.com/GStreamer/gstreamer/blob/1.22.0/subprojects/gst-plugins-bad/ext/webrtc/gstwebrtcbin.c#L7710-L7758
https://github.com/GStreamer/gstreamer/blob/1.22.0/subprojects/gst-plugins-bad/gst-libs/gst/webrtc/nice/nice.c#L1567-L1571
https://github.com/GNOME/pygobject/blob/3.42.2/gi/pygi-value.c#L736-L737
https://github.com/GNOME/pygobject/blob/3.42.2/gi/pygobject-object.c#L973-L978
https://github.com/GNOME/pygobject/blob/3.42.2/gi/pygobject-object.c#L1146-L1157
https://github.com/GNOME/pygobject/blob/3.42.2/gi/overrides/GObject.py#L481-L489
"""

import argparse
import ast
import gc
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import time
import weakref


# The current hook and stop order from the staged Selkies 1.6.1 source. The
# repaired case applies the ACTUAL stage patch to this small factory lifecycle.
RETAINED_SOURCE = '''class Probe:
    def start(self):
        self.pipeline = Gst.Pipeline.new()
        self.webrtcbin = Gst.ElementFactory.make("webrtcbin", "app")
        if self.webrtcbin is None:
            raise RuntimeError("webrtcbin unavailable")
        self.webrtcbin.set_property("ice-transport-policy", "relay")
        self.pipeline.add(self.webrtcbin)
        if os.environ.get("ELASTOS_BROWSER_VM_VZ_TRANSPORT", "").strip() == "vsock_v1":
            self._elastos_vz_ice_agent = self.webrtcbin.get_property("ice-agent")
            if self._elastos_vz_ice_agent is None:
                raise GSTWebRTCAppError("VZ ICE agent is unavailable")
            if not self._elastos_vz_ice_agent.emit("add-local-ip-address", "127.0.0.1"):
                raise GSTWebRTCAppError("VZ ICE loopback address was rejected")
            logger.info("using explicit VZ ICE local address: 127.0.0.1")

    def stop(self):
        if self.pipeline:
            self.pipeline.set_state(Gst.State.NULL)
            self.pipeline = None
        if self.webrtcbin:
            self.webrtcbin.set_state(Gst.State.NULL)
            self.webrtcbin = None
            logger.info("webrtcbin set to state NULL")
        logger.info("pipeline stopped")
'''


def load_patch(stage_source):
    source = Path(stage_source).read_text()
    match = re.search(r"^def patch_selkies_vz_ice_ownership\(source\):\n.*?"
                      r"(?=^text = patch_selkies_vz_ice_ownership\(text\))", source, re.M | re.S)
    if not match:
        raise ValueError("stage ICE ownership patch unavailable")
    namespace = {}
    exec(compile(ast.parse(match[0]), str(stage_source), "exec"), namespace)
    return namespace["patch_selkies_vz_ice_ownership"]


def emit(kind, **fields):
    print(json.dumps({"kind": kind, **fields}, sort_keys=True), flush=True)


def worker(args):
    import resource
    resource.setrlimit(resource.RLIMIT_CORE, (0, 0))
    resource.setrlimit(resource.RLIMIT_AS, (1024 ** 3, 1024 ** 3))
    resource.setrlimit(resource.RLIMIT_FSIZE, (1024 ** 2, 1024 ** 2))
    import gi
    gi.require_version("Gst", "1.0")
    gi.require_version("GstWebRTC", "1.0")
    from gi.repository import GLib, Gst, GstWebRTC  # Register the same GI types as Selkies.
    Gst.init(None)
    packages = subprocess.run([
        "dpkg-query", "-W", "-f=${binary:Package}=${Version}\n",
        "python3-gi", "libgstreamer1.0-0", "libgstreamer-plugins-bad1.0-0",
        "gstreamer1.0-plugins-bad", "gstreamer1.0-nice", "libnice10",
    ], capture_output=True, text=True, timeout=3, check=True).stdout.splitlines()
    factory = Gst.ElementFactory.find("webrtcbin")
    if factory is None:
        raise RuntimeError("webrtcbin plugin unavailable")
    plugin = Path(factory.get_plugin().get_filename())
    emit("libraries", gstreamer=Gst.version_string(), pygobject=gi.__version__,
         glib=".".join(map(str, (GLib.MAJOR_VERSION, GLib.MINOR_VERSION, GLib.MICRO_VERSION))),
         packages=packages, webrtc_plugin_sha256=hashlib.sha256(plugin.read_bytes()).hexdigest())
    if Gst.version()[:2] != (1, 22) or tuple(gi.version_info[:2]) != (3, 42):
        raise RuntimeError("this ownership experiment targets Debian GStreamer 1.22 / PyGObject 3.42")

    source = RETAINED_SOURCE
    if args.worker != "retained":
        source = load_patch(args.stage_source)(source)
    if args.worker == "already-owned":
        # Leave the bin with a nonfloating reference before the actual hook
        # retrieves it. This models an upstream fixed constructor using the
        # same real GObject, with no Python wrapper retained from the setup.
        marker = '        self.pipeline.add(self.webrtcbin)\n'
        source = source.replace(marker, marker + '''        primed = self.webrtcbin.get_property("ice-agent")
        primed._ref()
        primed = None
''', 1)
    import logging
    namespace = {"os": os, "Gst": Gst, "GSTWebRTCAppError": RuntimeError,
                 "logger": logging.getLogger("ice-lifetime-repro")}
    exec(compile(source, "<staged-vz-ice-lifecycle>", "exec"), namespace)
    peers = [namespace["Probe"](), namespace["Probe"]()]
    finalized_count = 0
    for iteration in range(args.iterations):
        records = []
        for peer in peers:
            peer.start()
            agent = peer._elastos_vz_ice_agent
            record = {"finalized": False}
            def finalized(record=record):
                record["finalized"] = True
                if record["wrapper_weak"]() is not None:
                    emit("native_finalized_with_live_wrapper", iteration=iteration,
                         refs_after_get=record["refs_after_get"])
                    # Stop before GC or a later assignment touches the dangling
                    # wrapper; the parent reaps this disposable child.
                    os._exit(20)
            record["native_weak"] = agent.weak_ref(finalized)
            record["wrapper_weak"] = weakref.ref(agent)
            record["refs_after_get"] = agent.__grefcount__
            records.append(record)
            agent = None
        for peer in peers:
            peer.stop()
        gc.collect()
        for record in records:
            wrapper_alive = record["wrapper_weak"]() is not None
            # Detect the dangling wrapper before another assignment can damage
            # allocator state. _exit avoids calling its invalid destructor.
            if record["finalized"] and wrapper_alive:
                emit("native_finalized_with_live_wrapper", iteration=iteration,
                     refs_after_get=record["refs_after_get"])
                os._exit(20)
            if args.worker != "retained" and (not record["finalized"] or wrapper_alive):
                emit("release_unproven", iteration=iteration,
                     native_finalized=record["finalized"], wrapper_alive=wrapper_alive,
                     refs_after_get=record["refs_after_get"])
                os._exit(21)
            finalized_count += int(record["finalized"])
        if iteration < 2:
            emit("cycle", iteration=iteration, refs_after_get=[r["refs_after_get"] for r in records],
                 finalized=[r["finalized"] for r in records])
    emit("completed", iterations=args.iterations, finalized_agents=finalized_count)


def run_case(args, mode):
    environment = {**os.environ, "G_DEBUG": "fatal-warnings", "GST_DEBUG": "0",
                   "ELASTOS_BROWSER_VM_VZ_TRANSPORT": "vsock_v1", "PYTHONDONTWRITEBYTECODE": "1"}
    command = [sys.executable, "-u", str(Path(__file__).resolve()), "--worker", mode,
               "--iterations", str(args.iterations), "--stage-source", str(args.stage_source)]
    started = time.monotonic()
    # Child diagnostics have a 1 MiB file cap; only fixed warning classes leave
    # this private temporary file. It is removed on every parent exit path.
    with tempfile.TemporaryFile() as errors:
        try:
            result = subprocess.run(command, env=environment, stdout=subprocess.PIPE,
                                    stderr=errors, timeout=args.timeout, text=True)
            code, stdout, timed_out = result.returncode, result.stdout, False
        except subprocess.TimeoutExpired as error:
            code, stdout, timed_out = None, error.stdout or b"", True
            if isinstance(stdout, bytes):
                stdout = stdout.decode("utf8", "replace")
        errors.seek(0)
        stderr = errors.read(1024 ** 2).decode("utf8", "replace")
    events = [json.loads(line) for line in stdout.splitlines() if line.startswith('{"')]
    warning_classes = [name for name in ["g_object_get_qdata", "g_object_set_qdata_full",
                       "g_object_unref", "gst_object_unref", "IS_MUTABLE", "failed to allocate"]
                       if name in stderr]
    return {"mode": mode, "returncode": code, "timed_out": timed_out,
            "elapsed_ms": round((time.monotonic() - started) * 1000), "events": events,
            "warning_classes": warning_classes, "stderr_present": bool(stderr)}


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--stage-source", type=Path, default=Path(__file__).with_name("stage-browser-vm-target.sh"))
    parser.add_argument("--iterations", type=int, default=100)
    parser.add_argument("--timeout", type=int, default=20, help="seconds per isolated child, maximum 30")
    parser.add_argument("--worker", choices=["retained", "python-owned", "already-owned"], help=argparse.SUPPRESS)
    args = parser.parse_args()
    if not 1 <= args.iterations <= 1000 or not 1 <= args.timeout <= 30:
        parser.error("iterations must be 1..1000 and timeout 1..30")
    if sys.platform != "linux":
        parser.error("execute only on the isolated Linux guest-library builder")
    if args.worker:
        worker(args)
        return 0
    load_patch(args.stage_source)  # Validate source before starting either case.
    cases = [run_case(args, mode) for mode in ("retained", "python-owned", "already-owned")]
    old, new, owned = cases
    reproduced = old["returncode"] == 20 and any(
        e["kind"] == "native_finalized_with_live_wrapper" for e in old["events"])
    def released(case):
        return case["returncode"] == 0 and any(
            e["kind"] == "completed" and e["finalized_agents"] == args.iterations * 2 for e in case["events"])
    fixed, already_owned = released(new), released(owned)
    print(json.dumps({"schema": "elastos.browser.vz-ice-lifetime-repro/v1",
                      "ok": reproduced and fixed and already_owned, "baseline_reproduced": reproduced,
                      "python_owned_passed": fixed, "already_owned_passed": already_owned, "cases": cases,
                      "stage_sha256": hashlib.sha256(args.stage_source.read_bytes()).hexdigest(),
                      "repro_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest()}, sort_keys=True))
    return 0 if reproduced and fixed and already_owned else 1


if __name__ == "__main__":
    sys.exit(main())
