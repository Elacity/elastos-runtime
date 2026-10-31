"""Injected backends only: no CDLL, X11 connection, child process or network."""
import ctypes as C
import copy
import contextlib
import io
import json
from pathlib import Path
import unittest

# Execute the actual bundled helper with injected X11 backends only.
import subprocess
import types
source = subprocess.check_output(["node", "--input-type=module", "-e",
    "import {browserX11GeometrySource} from './scripts/browser-selkies-control-service.mjs'; process.stdout.write(browserX11GeometrySource)"],
    cwd=Path(__file__).resolve().parent.parent, text=True)
module = types.ModuleType("browser_x11_geometry")
exec(compile(source, "browser-x11-geometry", "exec"), module.__dict__)
TITLE = "A page whose title may change"
def forbidden_cdll(*_args, **_kwargs):
    raise AssertionError("Regression tests must never load a native library")
module.C.CDLL = forbidden_cdll


class FakeBackend:
    def __init__(self):
        self.tree = {1: [2], 2: []}
        self.classes = {2: ["fixture", "Chromium"]}
        self.titles = {2: TITLE}
        self.pids = {2: 123}
        self.hidden = set()

    def root(self): return 1
    def attributes(self, window):
        return dict(x=5, y=6, width=390, height=1200, border_width=0,
                    map_state=0 if window in self.hidden else 2, override_redirect=0)
    def children(self, window, limit):
        result = self.tree[window]
        if len(result) > limit: raise module.GeometryError("X11 traversal node limit exceeded")
        return result
    def wm_class(self, window): return self.classes.get(window)
    def title(self, window): return self.titles.get(window)
    def pid(self, window): return self.pids.get(window)
    def position(self, window, root): return 11, 12
    def parent(self, window): return 1


class Function:
    def __init__(self, callback): self.callback = callback
    def __call__(self, *args): return self.callback(*args)


class FakeLibrary:
    def __init__(self):
        self.freed, self.handlers, self.closed, self.buffers = [], [], [], []
        for name in ("XOpenDisplay", "XCloseDisplay", "XDefaultRootWindow", "XGetWindowAttributes",
                     "XQueryTree", "XTranslateCoordinates", "XInternAtom", "XGetWindowProperty",
                     "XFree", "XSync", "XSetErrorHandler", "XGrabServer", "XUngrabServer", "XMoveResizeWindow"):
            setattr(self, name, Function(getattr(self, name)))

    def XOpenDisplay(self, _): return 9
    def XCloseDisplay(self, display): self.closed.append(display); return 0
    def XDefaultRootWindow(self, _): return 1
    def XGetWindowAttributes(self, *_): return 0
    def XQueryTree(self, _display, _window, _root, _parent, children, count):
        value = (module.Window * 1)(2); self.buffers.append(value)
        C.cast(children, C.POINTER(C.POINTER(module.Window)))[0] = C.cast(value, C.POINTER(module.Window))
        C.cast(count, C.POINTER(C.c_uint))[0] = 1
        return 0  # An error may still return allocated output.
    def XTranslateCoordinates(self, *_): return 0
    def XInternAtom(self, _display, name, _only_existing): return 6 if name == b"CARDINAL" else 7
    def XGetWindowProperty(self, _display, _window, _atom, _offset, _length, _delete, _type,
                           actual, size, count, remaining, data):
        value = (C.c_ulong * 1)(123); self.buffers.append(value)
        C.cast(actual, C.POINTER(C.c_ulong))[0] = 6
        C.cast(size, C.POINTER(C.c_int))[0] = 32
        C.cast(count, C.POINTER(C.c_ulong))[0] = 1
        C.cast(remaining, C.POINTER(C.c_ulong))[0] = 0
        C.cast(data, C.POINTER(C.c_void_p))[0] = C.cast(value, C.c_void_p)
        return 0
    def XFree(self, pointer): self.freed.append(C.cast(pointer, C.c_void_p).value); return 0
    def XSync(self, *_): return 0
    def XSetErrorHandler(self, handler): self.handlers.append(handler); return 1234
    def XGrabServer(self, *_): return 0
    def XUngrabServer(self, *_): return 0
    def XMoveResizeWindow(self, *_): return 1


class ConfigureBackend(FakeBackend):
    def __init__(self):
        super().__init__()
        self.geometry = dict(x=0, y=0, width=1919, height=1079)
        self.root_size = dict(width=1920, height=1080)
        self.parent_id, self.border, self.grabbed = 1, 0, False
        self.operations, self.moves = [], []
        self.no_effect = self.partial = self.reverse = self.grab_error = self.ungrab_error = False

    def attributes(self, window):
        return dict(x=0, y=0, **(self.root_size if window == 1 else
                    {key: self.geometry[key] for key in ("width", "height")}),
                    border_width=self.border if window != 1 else 0, map_state=2, override_redirect=0)
    def parent(self, window): return self.parent_id
    def position(self, window, root): return self.geometry["x"], self.geometry["y"]
    def grab(self):
        self.operations.append("grab"); self.grabbed = True
        if self.grab_error: raise module.GeometryError("grab sync failed")
    def ungrab(self):
        self.operations.append("ungrab"); self.grabbed = False
        if self.reverse: self.geometry = dict(x=0, y=0, width=1919, height=1079)
        if self.ungrab_error: raise module.GeometryError("ungrab sync failed")
    def move_resize(self, window, geometry):
        if not self.grabbed: raise AssertionError("Configure requires the server grab")
        self.operations.append("configure"); self.moves.append((window, dict(geometry)))
        if not self.no_effect: self.geometry = dict(geometry)
        if self.partial: raise module.GeometryError("configure protocol error after partial effect")


def apply_request(backend):
    identity = module.observe(backend)
    backend.root_size = dict(width=390, height=1200)
    return {"identity": identity, "root": dict(backend.root_size),
            "window": dict(x=0, y=0, **backend.root_size), "restore": False}


class Regression(unittest.TestCase):
    def test_unique_and_translated_geometry(self):
        value = module.observe(FakeBackend())
        self.assertEqual(value["schema"], module.SCHEMA)
        self.assertEqual(value["window"]["xid"], "0x2")
        self.assertEqual([value["window"][k] for k in ("x", "y", "width", "height")], [11, 12, 390, 1200])

    def test_missing_and_ambiguous_refuse(self):
        backend = FakeBackend(); backend.hidden.add(2)
        with self.assertRaisesRegex(module.GeometryError, "found 0"): module.observe(backend)
        backend = FakeBackend(); backend.tree[1].append(3); backend.tree[3] = []
        backend.classes[3] = backend.classes[2]; backend.titles[3] = TITLE + " - Chromium"; backend.pids[3] = 456
        with self.assertRaisesRegex(module.GeometryError, "found 2"): module.observe(backend)

    def test_page_titles_are_not_window_identity(self):
        backend = FakeBackend()
        backend.title = lambda *_: (_ for _ in ()).throw(AssertionError("Title must not be read"))
        self.assertEqual(module.observe(backend)["window"]["pid"], 123)

    def test_hidden_or_wrong_class_never_matches(self):
        for change in (lambda b: b.hidden.add(2), lambda b: b.classes.update({2: ["app", "Another"]})):
            backend = FakeBackend(); change(backend)
            with self.assertRaisesRegex(module.GeometryError, "found 0"): module.observe(backend)

    def test_pid_must_be_valid(self):
        for pid in (None, 0, -1, True, 2**31):
            backend = FakeBackend(); backend.pids[2] = pid
            with self.assertRaisesRegex(module.GeometryError, "valid _NET_WM_PID"): module.observe(backend)

    def test_node_depth_and_repeat_bounds(self):
        backend = FakeBackend(); backend.tree[1] = list(range(2, 258))
        with self.assertRaisesRegex(module.GeometryError, "node limit"): module.observe(backend)
        backend = FakeBackend(); backend.tree = {i: [i+1] for i in range(1, 10)}; backend.tree[10] = []
        with self.assertRaisesRegex(module.GeometryError, "depth limit"): module.observe(backend)
        backend = FakeBackend(); backend.tree[2] = [1]
        with self.assertRaisesRegex(module.GeometryError, "repeated"): module.observe(backend)

    def test_library_custody_and_long_property_abi(self):
        library = FakeLibrary()
        with module.X11(":40", library) as backend:
            self.assertEqual(backend.pid(2), 123)
            self.assertEqual(library.XGetWindowProperty.restype, C.c_int)
            self.assertEqual(library.XDefaultRootWindow.restype, C.c_ulong)
        self.assertEqual(len(library.freed), 1)
        self.assertEqual(library.closed, [9]); self.assertEqual(library.handlers[-1], 1234)

    def test_allocated_children_freed_on_status_failure(self):
        library = FakeLibrary()
        with module.X11(":40", library) as backend:
            with self.assertRaisesRegex(module.GeometryError, "tree"): backend.children(1, 256)
        self.assertEqual(len(library.freed), 1)

    def test_property_freed_on_protocol_error(self):
        library = FakeLibrary()
        with module.X11(":40", library) as backend:
            backend.atom("_NET_WM_PID"); backend.atom("CARDINAL")
            backend.errors.append((3, 20))
            with self.assertRaisesRegex(module.GeometryError, "protocol error"): backend.pid(2)
        self.assertEqual(len(library.freed), 1)

    def test_property_freed_on_size_or_type_failure(self):
        for kind in ("remaining", "type", "status"):
            library = FakeLibrary(); original = library.XGetWindowProperty.callback
            def changed(*args):
                result = original(*args)
                if kind == "remaining": C.cast(args[-2], C.POINTER(C.c_ulong))[0] = 4
                if kind == "type": C.cast(args[-5], C.POINTER(C.c_ulong))[0] = 99
                return 1 if kind == "status" else result
            library.XGetWindowProperty.callback = changed
            with module.X11(":40", library) as backend:
                with self.assertRaises(module.GeometryError): backend.pid(2)
            self.assertEqual(len(library.freed), 1)

    def test_protocol_handler_and_64_bit_layout(self):
        self.assertEqual(C.sizeof(C.c_void_p), 8, "Fixture requires the admitted 64-bit guest ABI")
        self.assertEqual(C.sizeof(module.WindowAttributes), 136)
        self.assertEqual(module.WindowAttributes.screen.offset, 128)
        self.assertEqual(C.sizeof(module.ErrorEvent), 40)
        library = FakeLibrary()
        with module.X11(":40", library) as backend:
            event = module.ErrorEvent(error_code=3, request_code=20)
            self.assertEqual(backend.handler(None, C.pointer(event)), 0)
            with self.assertRaisesRegex(module.GeometryError, "protocol error 3"): backend.root()

    def test_total_bound_and_terminator_structure_remain_strict(self):
        library = FakeLibrary()
        with module.X11(":40", library) as reader:
            maximum = b"x" * 246 + b"\0Chromium\0"
            self.assertEqual(len(maximum), 256)
            reader.property = lambda *_: maximum
            self.assertEqual(reader.wm_class(2)[1], "Chromium")
            for bad in (b"x" + maximum, maximum[:-1], b"instance\0Chromium\0\0"):
                reader.property = lambda *_: bad
                with self.assertRaisesRegex(module.GeometryError, "Malformed Chromium"):
                    reader.wm_class(2)
            evidence = reader.class_diagnostics[-1]
            self.assertEqual(evidence["parts"], 4)
            self.assertEqual(evidence["part_lengths"], [8, 8, 0, 0])
            self.assertTrue(evidence["terminal_nul"])

    def test_long_instance_label_stays_inside_total_bound(self):
        value = b"chromium (" + b"x" * 119 + b")\0Chromium\0"
        self.assertEqual(len(value), 140)
        library = FakeLibrary()
        with module.X11(":40", library) as reader:
            reader.property = lambda *_: value
            self.assertEqual(reader.wm_class(2), [value.split(b"\0")[0].decode(), "Chromium"])

    def test_unrelated_malformed_class_does_not_block_unique_match(self):
        library = FakeLibrary()
        with module.X11(":40", library) as reader:
            reader.property = lambda window, *_: b"fixture\0Chromium\0" if window == 2 else b"unrelated"
            backend = FakeBackend(); backend.tree[1].append(3); backend.tree[3] = []
            backend.wm_class = reader.wm_class; backend.class_diagnostics = reader.class_diagnostics
            result = module.observe(backend)
            self.assertEqual(result["window"]["xid"], "0x2")
            self.assertEqual(result["class_diagnostics"][0]["xid"], "0x3")
            backend.hidden.add(2)
            with self.assertRaisesRegex(module.GeometryError, "WM_CLASS@0x3 len=9 .*hex="): module.observe(backend)

    def test_chromium_looking_malformed_class_refuses(self):
        library = FakeLibrary()
        with module.X11(":40", library) as reader:
            reader.property = lambda window, *_: b"fixture\0Chromium\0" if window == 2 else b"app\0Chromium"
            backend = FakeBackend(); backend.tree[1].append(3); backend.tree[3] = []
            backend.wm_class = reader.wm_class; backend.class_diagnostics = reader.class_diagnostics
            with self.assertRaisesRegex(module.GeometryError, "Malformed Chromium WM_CLASS; WM_CLASS@0x3 len=12 .*hex="):
                module.observe(backend)

    def test_class_diagnostics_are_bounded(self):
        library = FakeLibrary()
        with module.X11(":40", library) as reader:
            reader.property = lambda *_: b"x" * 256
            for window in range(20): self.assertIsNone(reader.wm_class(window))
            self.assertEqual(len(reader.class_diagnostics), 8)
            self.assertTrue(all(len(value["raw_hex"]) == 95 and value["raw_hex"].count(":") == 31 and value["length"] == 256
                                and value["truncated"] for value in reader.class_diagnostics))

    def test_root_is_never_a_client_candidate(self):
        backend = FakeBackend(); original = backend.wm_class
        def classes(window):
            self.assertNotEqual(window, 1, "The root has no client-window identity")
            return original(window)
        backend.wm_class = classes
        self.assertEqual(module.observe(backend)["window"]["xid"], "0x2")

    def test_missing_attributes_or_translation_refuse(self):
        library = FakeLibrary()
        with module.X11(":40", library) as backend:
            with self.assertRaisesRegex(module.GeometryError, "attributes"): backend.attributes(2)
            with self.assertRaisesRegex(module.GeometryError, "translate"): backend.position(2, 1)

    def test_open_failure_restores_handler_and_remote_display_refuses(self):
        library = FakeLibrary(); library.XOpenDisplay.callback = lambda _: None
        with self.assertRaisesRegex(module.GeometryError, "Cannot open"): module.X11(":40", library)
        self.assertEqual(library.handlers[-1], 1234); self.assertEqual(library.closed, [])
        with self.assertRaisesRegex(module.GeometryError, "local "): module.X11("host:40", library)

    def test_configure_apply_requeries_after_ungrab(self):
        backend = ConfigureBackend(); payload = apply_request(backend)
        result = module.configure(backend, payload)
        self.assertEqual(backend.operations, ["grab", "configure", "ungrab"])
        self.assertEqual(backend.moves, [(2, payload["window"])])
        self.assertTrue(result["ok"]); self.assertTrue(result["configure"]["server_verified"])
        self.assertEqual(module.window_geometry(result["window"]), payload["window"])
        self.assertEqual(result["window"]["parent_xid"], "0x1")

    def test_invalid_request_refuses_before_grab(self):
        for mutate in (
            lambda p: p.update(restore=1), lambda p: p["root"].update(width=391),
            lambda p: p["root"].update(width=True), lambda p: p["window"].update(x=1),
            lambda p: p["window"].update(height=0), lambda p: p["window"].update(width=3841),
            lambda p: p["identity"]["window"].update(parent_xid="0x9"),
            lambda p: p["identity"]["window"].update(border_width=1),
            lambda p: p["identity"]["window"].update(pid=True),
            lambda p: p["identity"].update(ok=False), lambda p: p.update(extra=True)):
            backend = ConfigureBackend(); payload = apply_request(backend); mutate(payload)
            with self.assertRaises(module.GeometryError): module.configure(backend, payload)
            self.assertEqual(backend.operations, []); self.assertEqual(backend.moves, [])

    def test_changed_identity_refuses_without_configure(self):
        for mutate in (
            lambda b, p: b.pids.update({2: 124}),
            lambda b, p: b.classes.update({2: ["different-instance", "Chromium"]}),
            lambda b, p: setattr(b, "parent_id", 9), lambda b, p: setattr(b, "border", 1),
            lambda b, p: b.root_size.update(height=1202),
            lambda b, p: p["identity"]["window"].update(xid="0x3"),
            lambda b, p: (p["identity"]["root"].update(xid="0x9"), p["identity"]["window"].update(parent_xid="0x9"))):
            backend = ConfigureBackend(); payload = apply_request(backend); mutate(backend, payload)
            with self.assertRaises(module.GeometryError): module.configure(backend, payload)
            self.assertEqual(backend.operations, ["grab", "ungrab"]); self.assertEqual(backend.moves, [])

    def test_missing_and_ambiguous_during_grab_refuse(self):
        for ambiguous in (False, True):
            backend = ConfigureBackend(); payload = apply_request(backend)
            if ambiguous:
                backend.tree[1].append(3); backend.tree[3] = []
                backend.classes[3] = backend.classes[2]; backend.titles[3] = TITLE; backend.pids[3] = 456
            else: backend.classes[2] = ["app", "Different"]
            with self.assertRaises(module.GeometryError): module.configure(backend, payload)
            self.assertEqual(backend.moves, []); self.assertFalse(backend.grabbed)

    def test_no_effect_or_later_reversal_is_failure(self):
        for option in ("no_effect", "reverse"):
            backend = ConfigureBackend(); payload = apply_request(backend); setattr(backend, option, True)
            with self.assertRaises(module.GeometryError) as raised: module.configure(backend, payload)
            self.assertFalse(backend.grabbed)
            self.assertEqual(raised.exception.evidence["after"]["window"]["width"], 1919)

    def test_partial_and_ungrab_errors_preserve_observation(self):
        backend = ConfigureBackend(); payload = apply_request(backend); backend.partial = backend.ungrab_error = True
        with self.assertRaises(module.GeometryError) as raised: module.configure(backend, payload)
        self.assertIn("partial effect", str(raised.exception)); self.assertIn("ungrab", str(raised.exception))
        self.assertEqual(raised.exception.evidence["after"]["window"]["width"], 390)
        self.assertEqual([value["stage"] for value in raised.exception.evidence["errors"]], ["configure", "ungrab"])
        self.assertFalse(backend.grabbed)

    def test_grab_error_still_ungrabs(self):
        backend = ConfigureBackend(); payload = apply_request(backend); backend.grab_error = True
        with self.assertRaisesRegex(module.GeometryError, "grab sync failed"): module.configure(backend, payload)
        self.assertEqual(backend.operations, ["grab", "ungrab"]); self.assertFalse(backend.grabbed)

    def test_exact_odd_baseline_restoration(self):
        backend = ConfigureBackend(); backend.geometry.update(x=-7, y=9)
        identity = module.observe(backend)
        backend.geometry = dict(x=0, y=0, width=390, height=1200)
        payload = {"identity": identity, "root": module.root_geometry(identity["root"]),
                   "window": module.window_geometry(identity["window"]), "restore": True}
        result = module.configure(backend, payload)
        self.assertEqual(module.window_geometry(result["window"]), dict(x=-7, y=9, width=1919, height=1079))
        self.assertTrue(result["configure"]["restore"])
        for field in ("window", "root"):
            changed = copy.deepcopy(payload); changed[field]["width"] += 2
            backend.operations.clear(); backend.moves.clear()
            with self.assertRaisesRegex(module.GeometryError, "saved baseline"): module.configure(backend, changed)
            self.assertEqual(backend.operations, [])

    def test_configure_native_function_types_and_sync_order(self):
        library = FakeLibrary(); calls = []
        library.XGrabServer.callback = lambda *_: calls.append("grab") or 0
        library.XUngrabServer.callback = lambda *_: calls.append("ungrab") or 0
        library.XMoveResizeWindow.callback = lambda *args: calls.append(("move", args[1:])) or 1
        library.XSync.callback = lambda *_: calls.append("sync") or 0
        with module.X11(":40", library) as backend:
            backend.grab(); backend.move_resize(2, dict(x=-7, y=9, width=1919, height=1079)); backend.ungrab()
            self.assertEqual(library.XMoveResizeWindow.argtypes[-4:], [C.c_int, C.c_int, C.c_uint, C.c_uint])
        self.assertEqual(calls, ["grab", "sync", ("move", (2, -7, 9, 1919, 1079)), "sync", "ungrab", "sync"])

    def test_default_cli_observes_without_configure(self):
        backend = ConfigureBackend()
        class Context:
            def __enter__(self): return backend
            def __exit__(self, *_): return None
        original, args = module.X11, module.sys.argv
        try:
            module.X11 = lambda _: Context(); module.sys.argv = ["-c"]
            with contextlib.redirect_stdout(io.StringIO()) as output: self.assertEqual(module.main(), 0)
            self.assertTrue(json.loads(output.getvalue())["ok"]); self.assertEqual(backend.operations, [])
        finally: module.X11, module.sys.argv = original, args

    def test_invalid_cli_never_loads_native_library(self):
        original = module.sys.argv
        try:
            for args in (["--configure"], ["--unknown", "{}"], ["--configure", "{}"]):
                module.sys.argv = ["-c", *args]
                with contextlib.redirect_stderr(io.StringIO()) as output: self.assertEqual(module.main(), 1)
                self.assertFalse(json.loads(output.getvalue())["ok"])
        finally: module.sys.argv = original

    def test_configure_cli_returns_observation_schema(self):
        backend = ConfigureBackend(); payload = apply_request(backend)
        class Context:
            def __enter__(self): return backend
            def __exit__(self, *_): return None
        original, args = module.X11, module.sys.argv
        try:
            module.X11 = lambda _: Context(); module.sys.argv = ["-c", "--configure", json.dumps(payload)]
            with contextlib.redirect_stdout(io.StringIO()) as output: self.assertEqual(module.main(), 0)
            result = json.loads(output.getvalue())
            self.assertEqual(result["schema"], module.SCHEMA); self.assertEqual(result["window"]["width"], 390)
            self.assertEqual(backend.operations, ["grab", "configure", "ungrab"])
        finally: module.X11, module.sys.argv = original, args


if __name__ == "__main__":
    unittest.main(verbosity=2)
