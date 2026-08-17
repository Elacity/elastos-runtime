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
<head><meta charset="utf-8"><title>Home browser-context proof</title></head>
<body>
  <iframe id="home-gui" data-target="home-gui" title="opaque Home GUI"
    sandbox="allow-scripts allow-forms allow-pointer-lock allow-modals"></iframe>
  <script type="module">
    import {
      HOME_BROWSER_CONTEXT_STORAGE_KEY,
      isHomeBrowserContextId,
      loadOrCreateHomeBrowserContextId,
    } from "/capsules/home/browser/home-browser-context.js?v=home-20260807a";

    const phaseKey = "elastos.home.browser-context-proof-phase";
    const priorObservationKey = "elastos.home.browser-context-proof-prior";
    const phase = Number(window.localStorage.getItem(phaseKey) || "1");
    const hostContext = loadOrCreateHomeBrowserContextId(
      window.localStorage,
      window.crypto,
    );
    const frame = document.querySelector("#home-gui");
    const expectedToken = `fixture-token-${phase}`;
    const observations = {
      phase,
      host_context: hostContext,
      host_context_valid: isHomeBrowserContextId(hostContext),
      host_storage_context:
        window.localStorage.getItem(HOME_BROWSER_CONTEXT_STORAGE_KEY),
      rejected_ready_count: 0,
      accepted_ready_count: 0,
    };

    frame.src = `/child.html?phase=${phase}&home_origin=${encodeURIComponent(window.location.origin)}#home_token=${expectedToken}`;

    window.addEventListener("message", async (event) => {
      const data = event.data && typeof event.data === "object" ? event.data : null;
      if (!data || event.source !== frame.contentWindow || event.origin !== "null") {
        return;
      }
      if (data.type === "home:shell-ready") {
        const accepted =
          frame.dataset.target === "home-gui" &&
          data.homeToken === expectedToken;
        if (!accepted) {
          observations.rejected_ready_count += 1;
          return;
        }
        observations.accepted_ready_count += 1;
        frame.contentWindow.postMessage({
          type: "home:shell-context",
          browserContextId: hostContext,
        }, "*");
        return;
      }
      if (data.type !== "proof:child-bound") {
        return;
      }
      observations.child = data;
      if (phase === 1) {
        window.localStorage.setItem(
          priorObservationKey,
          JSON.stringify(observations),
        );
        window.localStorage.setItem(phaseKey, "2");
        window.location.reload();
        return;
      }
      const first = JSON.parse(
        window.localStorage.getItem(priorObservationKey) || "{}",
      );
      const result = {
        ok: true,
        platform: navigator.platform,
        first,
        second: observations,
      };
      result.same_top_level_profile_context =
        first.host_context === observations.host_context;
      result.new_opaque_child =
        Boolean(first.child?.instance_id) &&
        first.child.instance_id !== observations.child.instance_id;
      const response = await fetch("/proof", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(result),
      });
      if (!response.ok) {
        throw new Error(`proof receipt failed: ${response.status}`);
      }
      document.body.dataset.proofComplete = "true";
    });
  </script>
</body>
</html>
"""

CHILD = r"""<!doctype html>
<html lang="en">
<head><meta charset="utf-8"><title>Opaque Home GUI child</title></head>
<body>
  <script type="module">
    const route = new URL(window.location.href);
    const homeOrigin = route.searchParams.get("home_origin") || "";
    const homeToken = new URLSearchParams(route.hash.replace(/^#/, "")).get("home_token") || "";
    let localStorageUnavailable = false;
    try {
      window.localStorage.getItem("elastos.home.browser-context-id");
    } catch (_error) {
      localStorageUnavailable = true;
    }

    const {
      acceptHomeBrowserContextId,
      shellState,
    } = await import("/capsules/home-gui/browser/shell-core.js?v=home-20260807a");
    const initialContext = shellState.browserContextId;
    const instanceId = window.crypto.randomUUID();
    let contextMessagesBeforeAcceptedReady = 0;
    let acceptedReadySent = false;

    window.addEventListener("message", (event) => {
      if (event.source !== window.parent || event.origin !== homeOrigin) {
        return;
      }
      const message = event.data && typeof event.data === "object" ? event.data : null;
      if (message?.type !== "home:shell-context") {
        return;
      }
      if (!acceptedReadySent) {
        contextMessagesBeforeAcceptedReady += 1;
      }
      const accepted = acceptHomeBrowserContextId(message.browserContextId);
      window.parent.postMessage({
        type: "proof:child-bound",
        homeToken,
        phase: Number(route.searchParams.get("phase") || "0"),
        instance_id: instanceId,
        local_storage_unavailable: localStorageUnavailable,
        initial_context: initialContext,
        accepted,
        accepted_context: shellState.browserContextId,
        context_messages_before_accepted_ready: contextMessagesBeforeAcceptedReady,
      }, homeOrigin);
    });

    window.parent.postMessage({
      type: "home:shell-ready",
      homeToken: `${homeToken}-wrong`,
    }, homeOrigin);
    window.setTimeout(() => {
      acceptedReadySent = true;
      window.parent.postMessage({
        type: "home:shell-ready",
        homeToken,
      }, homeOrigin);
    }, 20);
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
        relative = path.lstrip("/")
        candidate = (ROOT / relative).resolve()
        if (
            ROOT not in candidate.parents
            or not candidate.is_file()
            or not relative.startswith(("capsules/home/browser/", "capsules/home-gui/browser/"))
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
        sys.stderr.write("[home-context-proof-server] " + (message % args) + "\n")
        sys.stderr.flush()


STATE_DIR.mkdir(parents=True, exist_ok=True)
server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
(STATE_DIR / "server-port").write_text(str(server.server_port))
server.serve_forever()
