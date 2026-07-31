#!/usr/bin/env python3

import json
import mimetypes
import os
import pathlib
import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import unquote, urlparse

ROOT = pathlib.Path(sys.argv[1]).resolve()
STATE_DIR = pathlib.Path(sys.argv[2]).resolve()
BROWSER_ROOT = ROOT / "capsules" / "gba-emulator" / "browser"
FIXTURE_ROOT = pathlib.Path(__file__).parent
ROM = ROOT / "capsules" / "gba-ucity" / "ucity.gba"
SAVE = STATE_DIR / "game.sav"
SAVE_STATE = STATE_DIR / "game.ss1"
RESULT = STATE_DIR / "result.json"
STATE = {
    "put_count": 0,
    "get_after_put": 0,
    "save_bytes": 0,
    "state_put_count": 0,
    "state_get_after_put": 0,
    "state_bytes": 0,
    "api_origins": [],
    "topology": {},
    "trusted_input": {},
}
LOCK = threading.Lock()

mimetypes.add_type("application/wasm", ".wasm")
mimetypes.add_type("text/javascript", ".js")

HARNESS = r"""<!doctype html>
<html lang="en">
<head><meta charset="utf-8"><title>Opaque uCity acceptance</title></head>
<body>
  <iframe id="negative-control" title="same-origin negative control"
    sandbox="allow-scripts allow-same-origin" src="/negative-control.html"></iframe>
  <iframe id="gba-frame" title="uCity via GBA actor"
    sandbox="allow-scripts allow-forms allow-pointer-lock allow-modals"
    allow="autoplay"
    src="/apps/gba-emulator/?capsule=gba-ucity#home_token=opaque-frame-browser-proof"></iframe>
  <script>
    const frame = document.querySelector("#gba-frame");
    const control = document.querySelector("#negative-control");
    let controlReady = false;
    window.addEventListener("message", async (event) => {
      if (event.source === control.contentWindow && event.data?.type === "negative-control-ready") {
        controlReady = true;
        return;
      }
      if (event.source !== frame.contentWindow || event.data?.type !== "gba-proof-ready") return;
      const deadline = Date.now() + 2000;
      while (!controlReady && Date.now() < deadline) {
        await new Promise((resolve) => setTimeout(resolve, 10));
      }
      const route = new URL(frame.src);
      const topology = {
        message_origin: event.origin,
        allows_same_origin: frame.sandbox.contains("allow-same-origin"),
        allows_popup_escape: frame.sandbox.contains("allow-popups-to-escape-sandbox"),
        credentialless: frame.hasAttribute("credentialless"),
        parent_can_read_frame: frame.contentDocument !== null,
        negative_control_readable: controlReady && control.contentDocument !== null,
        selected_resource: route.searchParams.get("capsule"),
        executable_actor: route.pathname.split("/").filter(Boolean)[1] || "",
      };
      await fetch("/proof/topology", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(topology),
      });
      frame.focus();
      document.body.dataset.gbaReady = "true";
    });
  </script>
</body>
</html>
"""


class Handler(BaseHTTPRequestHandler):
    def end_headers(self):
        # A sandboxed same-URL frame has an opaque request origin. These are
        # the product headers needed by its module, WASM, and Runtime fetches.
        self.send_header("Access-Control-Allow-Origin", "null")
        self.send_header(
            "Access-Control-Allow-Headers", "content-type, x-elastos-home-token"
        )
        self.send_header("Access-Control-Allow-Methods", "GET, PUT, POST, OPTIONS")
        self.send_header("Cross-Origin-Resource-Policy", "cross-origin")
        self.send_header("Cache-Control", "no-store")
        super().end_headers()

    def send_bytes(self, status, body, content_type):
        self.send_response(status)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        if body:
            self.wfile.write(body)

    def send_json(self, status, value):
        self.send_bytes(status, json.dumps(value).encode(), "application/json")

    def record_api_origin(self, path):
        if not path.startswith("/api/"):
            return
        origin = self.headers.get("Origin", "<missing>")
        with LOCK:
            if origin not in STATE["api_origins"]:
                STATE["api_origins"].append(origin)

    def do_OPTIONS(self):
        path = unquote(urlparse(self.path).path)
        self.record_api_origin(path)
        self.send_bytes(204, b"", "text/plain")

    def do_GET(self):
        path = unquote(urlparse(self.path).path)
        self.record_api_origin(path)
        if path == "/":
            self.send_bytes(200, HARNESS.encode(), "text/html; charset=utf-8")
            return
        if path == "/negative-control.html":
            self.send_bytes(
                200,
                b'<script>parent.postMessage({type:"negative-control-ready"}, "*")</script>',
                "text/html; charset=utf-8",
            )
            return
        if path in ("/apps/gba-emulator", "/apps/gba-emulator/"):
            source = (BROWSER_ROOT / "index.html").read_text()
            source = source.replace(
                "</head>", '<script src="/product-probe.js"></script></head>'
            )
            source = source.replace(
                "</body>", '<script type="module" src="/proof.js"></script></body>'
            )
            self.send_bytes(200, source.encode(), "text/html; charset=utf-8")
            return
        if path == "/proof.js":
            self.send_bytes(200, (FIXTURE_ROOT / "proof.js").read_bytes(), "text/javascript")
            return
        if path == "/product-probe.js":
            self.send_bytes(
                200, (FIXTURE_ROOT / "product-probe.js").read_bytes(), "text/javascript"
            )
            return
        if path == "/api/viewers/gba-emulator/content/gba-ucity":
            self.send_bytes(200, ROM.read_bytes(), "application/octet-stream")
            return
        if path == "/api/viewers/gba-emulator/library":
            self.send_json(200, {"items": [{"capsule": "gba-ucity", "title": "uCity"}]})
            return
        if path.startswith("/api/viewers/gba-emulator/storage/gba-ucity/save/"):
            if not SAVE.exists():
                self.send_json(404, {"error": "save not found"})
                return
            with LOCK:
                if STATE["put_count"]:
                    STATE["get_after_put"] += 1
            self.send_bytes(200, SAVE.read_bytes(), "application/octet-stream")
            return
        if path.startswith("/api/viewers/gba-emulator/storage/gba-ucity/state/"):
            if not SAVE_STATE.exists():
                self.send_json(404, {"error": "state not found"})
                return
            with LOCK:
                if STATE["state_put_count"]:
                    STATE["state_get_after_put"] += 1
            self.send_bytes(200, SAVE_STATE.read_bytes(), "application/octet-stream")
            return
        if path == "/proof/save-status":
            with LOCK:
                value = json.loads(json.dumps(STATE))
            self.send_json(200, value)
            return
        if path == "/proof/trusted-input-status":
            with LOCK:
                value = dict(STATE["trusted_input"])
            self.send_json(200, value)
            return
        prefix = "/apps/gba-emulator/"
        relative = path[len(prefix) :] if path.startswith(prefix) else path.lstrip("/")
        candidate = (BROWSER_ROOT / relative).resolve()
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
        self.record_api_origin(path)
        is_save = path.startswith("/api/viewers/gba-emulator/storage/gba-ucity/save/")
        is_state = path.startswith("/api/viewers/gba-emulator/storage/gba-ucity/state/")
        if not is_save and not is_state:
            self.send_json(404, {"error": "not found"})
            return
        length = int(self.headers.get("Content-Length", "0"))
        body = self.rfile.read(length)
        target = SAVE if is_save else SAVE_STATE
        target.write_bytes(body)
        with LOCK:
            if is_save:
                STATE["put_count"] += 1
                STATE["save_bytes"] = len(body)
            else:
                STATE["state_put_count"] += 1
                STATE["state_bytes"] = len(body)
        self.send_json(200, {"status": "ok"})

    def do_POST(self):
        path = unquote(urlparse(self.path).path)
        self.record_api_origin(path)
        length = int(self.headers.get("Content-Length", "0"))
        payload = json.loads(self.rfile.read(length) or b"{}")
        if path == "/proof/topology":
            with LOCK:
                STATE["topology"] = payload
            self.send_json(200, {"status": "ok"})
            return
        if path == "/proof/trusted-input":
            with LOCK:
                STATE["trusted_input"] = payload
            self.send_json(200, {"status": "ok"})
            return
        if path != "/proof":
            self.send_json(404, {"error": "not found"})
            return
        with LOCK:
            payload["topology"] = {
                **STATE["topology"],
                "api_origins": list(STATE["api_origins"]),
            }
            payload["trusted_input"] = dict(STATE["trusted_input"])
        RESULT.write_text(json.dumps(payload, indent=2))
        self.send_json(200, {"status": "ok"})

    def log_message(self, message, *args):
        sys.stderr.write("[gba-proof-server] " + (message % args) + "\n")
        sys.stderr.flush()


STATE_DIR.mkdir(parents=True, exist_ok=True)
server = ThreadingHTTPServer(
    ("127.0.0.1", int(os.environ.get("ELASTOS_GBA_SERVER_PORT", "0"))), Handler
)
(STATE_DIR / "server-port").write_text(str(server.server_address[1]))
server.serve_forever()
