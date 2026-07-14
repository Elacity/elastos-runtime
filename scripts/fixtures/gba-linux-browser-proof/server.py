#!/usr/bin/env python3

import json
import mimetypes
import pathlib
import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import unquote, urlparse

ROOT = pathlib.Path(sys.argv[1]).resolve()
STATE_DIR = pathlib.Path(sys.argv[2]).resolve()
BROWSER_ROOT = ROOT / "capsules" / "gba-emulator" / "browser"
ROM = ROOT / "capsules" / "gba-ucity" / "ucity.gba"
SAVE = STATE_DIR / "game.sav"
RESULT = STATE_DIR / "result.json"
STATE = {"put_count": 0, "get_after_put": 0, "save_bytes": 0}
LOCK = threading.Lock()

mimetypes.add_type("application/wasm", ".wasm")
mimetypes.add_type("text/javascript", ".js")


class Handler(BaseHTTPRequestHandler):
    def end_headers(self):
        self.send_header("Cross-Origin-Opener-Policy", "same-origin")
        self.send_header("Cross-Origin-Embedder-Policy", "require-corp")
        self.send_header("Cross-Origin-Resource-Policy", "same-origin")
        self.send_header("Cache-Control", "no-store")
        super().end_headers()

    def send_bytes(self, status, body, content_type):
        self.send_response(status)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def send_json(self, status, value):
        self.send_bytes(status, json.dumps(value).encode(), "application/json")

    def do_GET(self):
        path = unquote(urlparse(self.path).path)
        if path in ("/", "/index.html"):
            source = (BROWSER_ROOT / "index.html").read_text()
            source = source.replace(
                "</body>",
                '<script type="module" src="/proof.js"></script></body>',
            )
            self.send_bytes(200, source.encode(), "text/html; charset=utf-8")
            return
        if path == "/proof.js":
            self.send_bytes(
                200,
                (pathlib.Path(__file__).parent / "proof.js").read_bytes(),
                "text/javascript",
            )
            return
        if path == "/api/viewers/gba-emulator/content/gba-ucity":
            self.send_bytes(200, ROM.read_bytes(), "application/octet-stream")
            return
        if path == "/api/viewers/gba-emulator/library":
            self.send_json(200, {"items": [{"capsule": "gba-ucity", "title": "uCity"}]})
            return
        if path.startswith("/api/viewers/gba-emulator/storage/gba-emulator/save/"):
            if not SAVE.exists():
                self.send_json(404, {"error": "save not found"})
                return
            with LOCK:
                if STATE["put_count"]:
                    STATE["get_after_put"] += 1
            self.send_bytes(200, SAVE.read_bytes(), "application/octet-stream")
            return
        if path == "/proof/save-status":
            with LOCK:
                self.send_json(200, dict(STATE))
            return
        candidate = (BROWSER_ROOT / path.lstrip("/")).resolve()
        if BROWSER_ROOT not in candidate.parents or not candidate.is_file():
            self.send_json(404, {"error": "not found"})
            return
        self.send_bytes(
            200,
            candidate.read_bytes(),
            mimetypes.guess_type(candidate.name)[0] or "application/octet-stream",
        )

    def do_PUT(self):
        path = unquote(urlparse(self.path).path)
        if not path.startswith("/api/viewers/gba-emulator/storage/gba-emulator/save/"):
            self.send_json(404, {"error": "not found"})
            return
        length = int(self.headers.get("Content-Length", "0"))
        body = self.rfile.read(length)
        SAVE.write_bytes(body)
        with LOCK:
            STATE["put_count"] += 1
            STATE["save_bytes"] = len(body)
        self.send_json(200, {"status": "ok"})

    def do_POST(self):
        if urlparse(self.path).path != "/proof":
            self.send_json(404, {"error": "not found"})
            return
        length = int(self.headers.get("Content-Length", "0"))
        payload = json.loads(self.rfile.read(length))
        RESULT.write_text(json.dumps(payload, indent=2))
        self.send_json(200, {"status": "ok"})

    def log_message(self, message, *args):
        sys.stderr.write("[gba-proof-server] " + (message % args) + "\n")
        sys.stderr.flush()


STATE_DIR.mkdir(parents=True, exist_ok=True)
ThreadingHTTPServer(("127.0.0.1", 8765), Handler).serve_forever()
