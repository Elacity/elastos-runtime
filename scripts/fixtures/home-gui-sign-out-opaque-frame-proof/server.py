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
RESULT = STATE_DIR / "result.json"
LOCK = threading.Lock()

mimetypes.add_type("text/javascript", ".js")

HARNESS = r"""<!doctype html>
<html lang="en">
<head><meta charset="utf-8"><title>Home GUI sign-out proof</title></head>
<body>
  <iframe id="valid-gui" title="valid opaque Home GUI"
    sandbox="allow-scripts allow-forms allow-pointer-lock allow-modals"></iframe>
  <iframe id="wrong-origin-gui" title="wrong-origin opaque Home GUI"
    sandbox="allow-scripts allow-forms allow-pointer-lock allow-modals"></iframe>
  <iframe id="attacker" title="same-origin attacker" src="/attacker.html"></iframe>
  <script type="module">
    const validFrame = document.querySelector("#valid-gui");
    const wrongOriginFrame = document.querySelector("#wrong-origin-gui");
    const attackerFrame = document.querySelector("#attacker");
    const origin = window.location.origin;
    const wrongOrigin = `http://localhost:${window.location.port}`;
    const signedSummary = {
      authority: {
        signed_in: true,
        principal_id: "principal:home-gui-sign-out-proof",
      },
      active_shell: {
        active: "home-gui",
        candidates: [{
          name: "home-gui",
          title: "Home GUI",
          role: "shell",
          launchable: true,
          route: "/apps/home-gui/",
        }],
      },
      app: { id: "home", route: "/apps/home/" },
      appearance: {},
      browser_state: {
        principal_id: "principal:home-gui-sign-out-proof",
        layout: {
          desktop: {},
          taskbar: [],
          desktopHidden: [],
          desktopIconsVisible: true,
        },
        recent_targets: [],
        session: { windows: [] },
      },
      desktop_objects: { objects: [] },
      identity: {},
      notifications: {},
      people: {},
      runtime: { running: true },
      services: {},
      site: {},
      targets: [],
    };
    const signedOutSummary = {
      ...signedSummary,
      authority: { signed_in: false },
    };
    const observations = {
      sign_out_messages: 0,
      valid_shell_ready: false,
      wrong_origin_helper_ready: false,
    };
    const pendingInspections = new Map();
    let inspectionSerial = 0;

    validFrame.src =
      `/child.html?proof_origin=${encodeURIComponent(origin)}` +
      `&home_origin=${encodeURIComponent(origin)}#home_token=gui-token`;
    wrongOriginFrame.src =
      `/child.html?proof_origin=${encodeURIComponent(origin)}` +
      `&home_origin=${encodeURIComponent(wrongOrigin)}#home_token=wrong-origin-token`;

    function waitFor(predicate, label) {
      return new Promise((resolve, reject) => {
        let attempts = 0;
        const poll = async () => {
          if (await predicate()) {
            resolve();
            return;
          }
          attempts += 1;
          if (attempts >= 200) {
            reject(new Error(`timed out waiting for ${label}`));
            return;
          }
          window.setTimeout(poll, 10);
        };
        poll();
      });
    }

    function inspect(frame) {
      const requestId = `inspect-${++inspectionSerial}`;
      return new Promise((resolve, reject) => {
        const timeout = window.setTimeout(() => {
          pendingInspections.delete(requestId);
          reject(new Error(`timed out waiting for ${requestId}`));
        }, 2_000);
        pendingInspections.set(requestId, { resolve, timeout });
        frame.contentWindow.postMessage({ type: "proof:inspect", requestId }, "*");
      });
    }

    window.addEventListener("message", (event) => {
      const data = event.data && typeof event.data === "object" ? event.data : null;
      if (!data) {
        return;
      }
      if (
        event.source === validFrame.contentWindow &&
        event.origin === "null" &&
        data.type === "home:shell-ready" &&
        data.homeToken === "gui-token"
      ) {
        observations.valid_shell_ready = true;
        return;
      }
      if (
        event.source === validFrame.contentWindow &&
        event.origin === "null" &&
        data.type === "home:sign-out" &&
        data.homeToken === "gui-token"
      ) {
        observations.sign_out_messages += 1;
        validFrame.contentWindow.postMessage({
          type: "home:shell-response",
          requestId: data.requestId,
          result: true,
        }, "*");
        return;
      }
      if (
        data.type === "proof:ready" &&
        event.source === wrongOriginFrame.contentWindow &&
        event.origin === "null"
      ) {
        observations.wrong_origin_helper_ready = true;
        return;
      }
      if (data.type !== "proof:state" || event.origin !== "null") {
        return;
      }
      const pending = pendingInspections.get(data.requestId);
      if (!pending) {
        return;
      }
      pendingInspections.delete(data.requestId);
      window.clearTimeout(pending.timeout);
      pending.resolve(data);
    });

    async function run() {
      await waitFor(
        () => observations.valid_shell_ready && observations.wrong_origin_helper_ready,
        "both Home GUI proof frames",
      );

      observations.initial = await inspect(validFrame);

      attackerFrame.contentWindow.postMessage({
        type: "proof:forge-summary",
        summary: signedSummary,
      }, origin);
      await new Promise((resolve) => window.setTimeout(resolve, 30));
      observations.forged_source = await inspect(validFrame);

      validFrame.contentWindow.postMessage({
        type: "home:shell-summary",
        summary: signedSummary,
      }, "*");
      await waitFor(async () => {
        const state = await inspect(validFrame);
        observations.trusted_signed = state;
        return state.authority === "signed" && state.display === "flex";
      }, "trusted signed summary projection");

      validFrame.contentWindow.postMessage({
        type: "home:shell-summary",
        summary: signedOutSummary,
      }, "*");
      await waitFor(async () => {
        const state = await inspect(validFrame);
        observations.trusted_signed_out = state;
        return state.authority === "unsigned" && state.display === "none";
      }, "trusted signed-out summary projection");

      wrongOriginFrame.contentWindow.postMessage({
        type: "home:shell-summary",
        summary: signedSummary,
      }, "*");
      await new Promise((resolve) => window.setTimeout(resolve, 30));
      observations.forged_origin = await inspect(wrongOriginFrame);

      validFrame.contentWindow.postMessage({
        type: "home:shell-summary",
        summary: signedSummary,
      }, "*");
      await waitFor(async () => {
        const state = await inspect(validFrame);
        observations.before_click = state;
        return state.authority === "signed" && state.display === "flex";
      }, "signed projection before click");
      validFrame.contentWindow.postMessage({ type: "proof:click-sign-out" }, "*");
      await waitFor(
        () => observations.sign_out_messages === 1,
        "one Home GUI sign-out request",
      );
      await new Promise((resolve) => window.setTimeout(resolve, 50));

      observations.ok = true;
      const response = await fetch("/proof", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(observations),
      });
      if (!response.ok) {
        throw new Error(`proof receipt failed: ${response.status}`);
      }
      document.body.dataset.proofComplete = "true";
    }

    run().catch(async (error) => {
      observations.ok = false;
      observations.error = error instanceof Error ? error.message : String(error);
      await fetch("/proof", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(observations),
      });
    });
  </script>
</body>
</html>
"""

CHILD = r"""<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <title>Opaque Home GUI child</title>
  <link rel="stylesheet" href="/capsules/home-gui/browser/style.css?v=home-20260725a">
</head>
<body data-home-status="booting" data-home-shell="desktop" data-home-gui="mounted">
  <div class="home-gui-shell"></div>
  <script type="module" src="/capsules/home-gui/browser/home-gui-shell.js?v=home-20260726a"></script>
  <script type="module">
    const route = new URL(window.location.href);
    const proofOrigin = route.searchParams.get("proof_origin") || "";

    function sendState(requestId) {
      const button = document.querySelector("#toolbar-sign-out");
      window.parent.postMessage({
        type: "proof:state",
        requestId,
        authority: document.body.dataset.homeAuthority || "",
        display: button ? window.getComputedStyle(button).display : "missing",
      }, proofOrigin);
    }

    function announceReady() {
      if (!document.querySelector("#toolbar-sign-out")) {
        window.setTimeout(announceReady, 10);
        return;
      }
      window.parent.postMessage({ type: "proof:ready" }, proofOrigin);
    }

    window.addEventListener("message", (event) => {
      if (event.source !== window.parent || event.origin !== proofOrigin) {
        return;
      }
      const data = event.data && typeof event.data === "object" ? event.data : null;
      if (data?.type === "proof:inspect") {
        sendState(data.requestId);
      } else if (data?.type === "proof:click-sign-out") {
        document.querySelector("#toolbar-sign-out")?.click();
      }
    });

    announceReady();
  </script>
</body>
</html>
"""

ATTACKER = r"""<!doctype html>
<html lang="en">
<head><meta charset="utf-8"><title>Home GUI attacker</title></head>
<body>
  <script>
    window.addEventListener("message", (event) => {
      const data = event.data && typeof event.data === "object" ? event.data : null;
      if (
        event.source !== window.parent ||
        event.origin !== window.location.origin ||
        data?.type !== "proof:forge-summary"
      ) {
        return;
      }
      window.parent.document.querySelector("#valid-gui").contentWindow.postMessage({
        type: "home:shell-summary",
        summary: data.summary,
      }, "*");
    });
  </script>
</body>
</html>
"""


class Handler(BaseHTTPRequestHandler):
    def end_headers(self):
        self.send_header("Access-Control-Allow-Origin", "null")
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

    def do_GET(self):
        path = unquote(urlparse(self.path).path)
        if path == "/":
            self.send_bytes(200, HARNESS.encode(), "text/html; charset=utf-8")
            return
        if path == "/child.html":
            self.send_bytes(200, CHILD.encode(), "text/html; charset=utf-8")
            return
        if path == "/attacker.html":
            self.send_bytes(200, ATTACKER.encode(), "text/html; charset=utf-8")
            return
        relative = path.lstrip("/")
        candidate = (ROOT / relative).resolve()
        if (
            ROOT not in candidate.parents
            or not candidate.is_file()
            or not relative.startswith("capsules/home-gui/browser/")
        ):
            self.send_json(404, {"error": "not found"})
            return
        self.send_bytes(
            200,
            candidate.read_bytes(),
            mimetypes.guess_type(candidate.name)[0] or "application/octet-stream",
        )

    def do_POST(self):
        path = unquote(urlparse(self.path).path)
        if path != "/proof":
            self.send_json(404, {"error": "not found"})
            return
        length = int(self.headers.get("Content-Length", "0"))
        payload = json.loads(self.rfile.read(length) or b"{}")
        with LOCK:
            RESULT.write_text(json.dumps(payload, indent=2))
        self.send_json(200, {"status": "ok"})

    def log_message(self, message, *args):
        sys.stderr.write("[home-gui-sign-out-proof-server] " + (message % args) + "\n")
        sys.stderr.flush()


STATE_DIR.mkdir(parents=True, exist_ok=True)
server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
(STATE_DIR / "server-port").write_text(str(server.server_port))
server.serve_forever()
