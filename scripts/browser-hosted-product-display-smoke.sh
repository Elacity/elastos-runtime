#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

usage() {
  cat <<'USAGE'
Usage:
  scripts/browser-hosted-product-display-smoke.sh \
    --adapter-config /path/to/browser-engine-adapter.json

This smoke is the hosted Browser product-display gate. It fails unless the
Browser Engine Adapter can launch a webrtc_remote_display session that reports:
  display_backend != cdp_screencast_i420
  backend_class = product_compositor
  audio = true
  video = true
  direct_network = false

The current Playwright/CDP proof is expected to fail this gate.
USAGE
}

adapter_config=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --adapter-config)
      shift
      adapter_config="${1:-}"
      ;;
    --help|-h)
      usage
      exit 0
      ;;
    *)
      echo "unknown option: $1" >&2
      usage >&2
      exit 1
      ;;
  esac
  shift || true
done

if [[ -z "$adapter_config" ]]; then
  usage >&2
  exit 1
fi

if [[ ! -f "$adapter_config" ]]; then
  echo "--adapter-config does not exist: $adapter_config" >&2
  exit 1
fi

cd "$repo_root"

cargo build --quiet --manifest-path capsules/browser-engine-adapter/Cargo.toml
adapter_bin="${CARGO_TARGET_DIR:-capsules/browser-engine-adapter/target}/debug/browser-engine-adapter"

request_json="$(CONFIG_PATH="$adapter_config" node - <<'NODE'
const fs = require("node:fs");
const config = JSON.parse(fs.readFileSync(process.env.CONFIG_PATH, "utf8"));
const adapter = config.adapters?.[0];
if (adapter?.kind === "selkies_gstreamer" || adapter?.display_modes?.includes("webrtc_remote_display")) {
  const controlSocket = adapter?.supervisor?.control_socket_path;
  if (typeof controlSocket !== "string" || !controlSocket.startsWith("/")) {
    throw new Error("hosted product display config must include an absolute supervisor.control_socket_path");
  }
}
const streamSession = {
  schema: "elastos.exit.stream-session/v1",
  stream_id: "stream:hosted-product-display-smoke",
  target: "tls://example.com:443",
  byte_transport: "adapter_ipc",
  adapter_ipc: {
    schema: "elastos.adapter-ipc/v1",
    kind: "unix_socket",
    path: "/tmp/elastos-browser-product-display-smoke-adapter.sock",
    stream_id: "stream:hosted-product-display-smoke",
    runtime_stream_path: "/tmp/elastos-browser-product-display-smoke-runtime.sock",
  },
};
console.log(JSON.stringify({ op: "init", config }));
console.log(JSON.stringify({
  op: "launch",
  url: "https://example.com/",
  stream_session: streamSession,
  principal_id: "person:local:hosted-product-display-smoke",
  reason: "verify hosted product display session",
  display_mode: "webrtc_remote_display",
  viewport: { width: 1280, height: 720 },
}));
console.log(JSON.stringify({ op: "shutdown" }));
NODE
)"

output="$(
  printf '%s\n' "$request_json" \
    | "$adapter_bin"
)"

result="$(
  OUTPUT="$output" node - <<'NODE'
const lines = process.env.OUTPUT
  .split(/\r?\n/)
  .map((line) => line.trim())
  .filter(Boolean)
  .map((line) => JSON.parse(line));

function fail(message, payload) {
  if (payload !== undefined) {
    console.error(JSON.stringify(payload, null, 2));
  }
  throw new Error(message);
}

const launch = lines.find((line) => line.status === "ok" && line.data?.schema === "elastos.browser.engine.page/v1")
  || lines.find((line) => line.status === "error");

if (!launch) {
  fail("browser-engine-adapter did not return a launch result", lines);
}
if (launch.status !== "ok") {
  fail(`hosted product display unavailable: ${launch.code || "error"} ${launch.message || ""}`, launch);
}

const session = launch.data.display_session || {};
if (session.schema !== "elastos.browser.display-session/v1") {
  fail("display session schema is not elastos.browser.display-session/v1", session);
}
if (session.mode !== "webrtc_remote_display") {
  fail(`display session mode is not webrtc_remote_display: ${session.mode || "none"}`, session);
}
if (session.backend_class !== "product_compositor") {
  fail(`display session is not product_compositor: ${session.backend_class || "none"}`, session);
}
if (session.display_backend === "cdp_screencast_i420") {
  fail("CDP screencast proof surface cannot satisfy hosted product display", session);
}
if (session.audio !== true) {
  fail("hosted product display must advertise audio=true", session);
}
if (session.video !== true) {
  fail("hosted product display must advertise video=true", session);
}
if (session.display_backend === "selkies_gstreamer_webrtc") {
  if (session.offerer !== "engine") {
    fail("Selkies/GStreamer hosted display must use engine-offer WebRTC negotiation", session);
  }
  const initialOffer = session.initial_offer || {};
  if (
    initialOffer.schema !== "elastos.browser.webrtc-offer/v1" ||
    initialOffer.type !== "offer" ||
    typeof initialOffer.sdp !== "string" ||
    !initialOffer.sdp.includes("m=video") ||
    !initialOffer.sdp.includes("m=audio")
  ) {
    fail("Selkies/GStreamer hosted display must include an initial audio/video WebRTC offer", session);
  }
}
if (session.direct_network !== false || launch.data.direct_network !== false) {
  fail("hosted product display reported direct network authority", { launch: launch.data, session });
}
if (session.network_mode !== "runtime_net_only" || launch.data.network_mode !== "runtime_net_only") {
  fail("hosted product display must remain runtime_net_only", { launch: launch.data, session });
}

console.log(JSON.stringify({
  ok: true,
  page_id: launch.data.page_id,
  display_backend: session.display_backend || null,
  backend_class: session.backend_class,
  audio: session.audio,
  video: session.video,
  direct_network: false,
}));
NODE
)"

page_id="$(RESULT="$result" node -e 'const result=JSON.parse(process.env.RESULT); process.stdout.write(result.page_id || "");')"
if [[ -n "$page_id" ]]; then
  close_json="$(CONFIG_PATH="$adapter_config" PAGE_ID="$page_id" node - <<'NODE'
const fs = require("node:fs");
const config = JSON.parse(fs.readFileSync(process.env.CONFIG_PATH, "utf8"));
console.log(JSON.stringify({ op: "init", config }));
console.log(JSON.stringify({ op: "close_page", page_id: process.env.PAGE_ID }));
console.log(JSON.stringify({ op: "shutdown" }));
NODE
)"
  printf '%s\n' "$close_json" \
    | "$adapter_bin" >/dev/null || true
fi

printf '%s\n' "$result"
