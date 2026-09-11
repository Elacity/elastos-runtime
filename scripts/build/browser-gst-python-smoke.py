#!/usr/bin/env python3
"""Check the guest's Python bindings and real Selkies module before image creation.

Uses installed GI/GStreamer/Selkies only. No display, audio server, pipeline,
network connection, or Runtime is started. Debian supplies the required Gst
overrides and native value marshalling in python3-gst-1.0.
"""

import importlib
import json


def verify():
    import gi
    for namespace in ("Gst", "GstWebRTC", "GstSdp", "GstRtp"):
        gi.require_version(namespace, "1.0")
    from gi.repository import Gst, GstWebRTC, GstSdp, GstRtp

    # Raw GI ignores Structure constructor arguments and can corrupt native
    # boxed memory. Reject absent overrides BEFORE calling either constructor.
    if (Gst.Structure.__module__ != "gi.overrides.Gst"
            or Gst.Fraction.__module__ != "gi.overrides.Gst"):
        raise RuntimeError("Browser guest requires python3-gst-1.0 and its Gst overrides")
    Gst.init(None)
    options = Gst.Structure("application/data-channel")
    values = {"ordered": True, "priority": "high", "max-retransmits": 0}
    for key, value in values.items():
        options.set_value(key, value)
    if options.get_name() != "application/data-channel" or any(
            options.get_value(key) != value for key, value in values.items()):
        raise RuntimeError("Gst.Structure data-channel options did not round-trip")

    caps = Gst.Caps.from_string("video/x-raw")
    caps.set_value("framerate", Gst.Fraction(60, 1))
    fraction = caps.get_structure(0).get_value("framerate")
    if fraction.num != 60 or fraction.denom != 1:
        raise RuntimeError("Gst.Fraction caps value did not round-trip")

    # Import the actual module, including its Fraction constructor and RTP
    # extension class; the package namespace alone does not exercise these.
    module = importlib.import_module("selkies_gstreamer.gstwebrtc_app")
    module.GSTWebRTCApp(encoder="openh264enc", stun_servers=[], turn_servers=[])
    return {"schema": "elastos.browser.gst-python-smoke/v1", "ok": True,
            "gstreamer": Gst.version_string(), "pygobject": gi.__version__,
            "gst_overrides": True, "structure_round_trip": True,
            "fraction_round_trip": True, "selkies_module": True}


if __name__ == "__main__":
    print(json.dumps(verify(), sort_keys=True))
