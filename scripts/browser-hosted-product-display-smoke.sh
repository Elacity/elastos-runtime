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
Set ELASTOS_TEST_BROWSER_ENGINE_ADAPTER_BIN and its matching
ELASTOS_TEST_BROWSER_ENGINE_ADAPTER_SHA256 to reuse a verified fixture binary.
Otherwise this script builds the adapter. Fake signaling proves only the fixture.
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

adapter_bin="${ELASTOS_TEST_BROWSER_ENGINE_ADAPTER_BIN:-}"
if [[ -z "$adapter_bin" ]]; then
  cargo build --quiet --manifest-path capsules/browser-engine-adapter/Cargo.toml
  adapter_bin="${CARGO_TARGET_DIR:-capsules/browser-engine-adapter/target}/debug/browser-engine-adapter"
elif [[ "$adapter_bin" != /* || ! -x "$adapter_bin" || -z "${ELASTOS_TEST_BROWSER_ENGINE_ADAPTER_SHA256:-}" ]]; then
  echo "fixture binary override requires an absolute executable and its SHA-256" >&2
  exit 1
fi

CONFIG_PATH="$adapter_config" ADAPTER_BIN="$adapter_bin" node - <<'NODE'
const fs = require("node:fs");
const { createHash, randomUUID } = require("node:crypto");
const { spawn } = require("node:child_process");
const { createInterface } = require("node:readline");
const { once } = require("node:events");
const { isDeepStrictEqual } = require("node:util");
if (process.env.ELASTOS_TEST_BROWSER_ENGINE_ADAPTER_BIN) {
  const hash = createHash("sha256").update(fs.readFileSync(process.env.ADAPTER_BIN)).digest("hex");
  if (hash !== process.env.ELASTOS_TEST_BROWSER_ENGINE_ADAPTER_SHA256) {
    throw new Error("fixture adapter binary checksum mismatch");
  }
}
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
const browserProfile = {
  schema: "elastos.browser.profile/v1",
  scope: "active_principal",
  storage: "principal_owned_profile_disk",
  storage_posture: "principal_owned_reset_scoped_unprotected",
  protected_storage: false,
  encrypted: false,
  recoverable: false,
  recovery: "not_recovery_kit_packaged",
  uri: "localhost://Users/0123456789ab/BrowserProfiles/default/profile.ext4",
  public_uri: "localhost://Users/self/BrowserProfiles/default/profile.ext4",
  profile_key: "profile-0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
  disk_path: "/tmp/elastos-browser-hosted-product-display/BrowserProfiles/default/profile.ext4",
  reset: "whole_profile",
};
const generation = `hosted-product-display-smoke-${randomUUID()}`;
const principal = "person:local:hosted-product-display-smoke";
const launchRequest = {
  op: "launch",
  url: "https://example.com/",
  stream_session: streamSession,
  lifecycle_generation: generation,
  profile: browserProfile,
  principal_id: principal,
  reason: "verify hosted product display session",
  display_mode: "webrtc_remote_display",
  guarantee_level: "operator_rbi",
  viewport: { width: 1280, height: 720 },
};

function fail(message, payload) {
  if (payload !== undefined) console.error(JSON.stringify(payload, null, 2));
  throw new Error(message);
}

function validateDisplay(launch) {
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
if (session.audio !== true && session.audio !== false) {
  fail("hosted product display must report audio availability", session);
}
if (session.video !== true) {
  fail("hosted product display must advertise video=true", session);
}
if (session.display_backend === "selkies_gstreamer_webrtc") {
  if (session.offerer !== "engine") {
    fail("Selkies/GStreamer hosted display must use engine-offer WebRTC negotiation", session);
  }
  const initialOffer = session.initial_offer || {};
  const audioOffer = session.audio_offer || {};
  if (
    initialOffer.schema !== "elastos.browser.webrtc-offer/v1" ||
    initialOffer.type !== "offer" ||
    typeof initialOffer.sdp !== "string" ||
    !initialOffer.sdp.includes("m=video")
  ) {
    fail("Selkies/GStreamer hosted display must include an initial video WebRTC offer", session);
  }
  const audioSdp = typeof audioOffer.sdp === "string" ? audioOffer.sdp : initialOffer.sdp;
  if (session.audio === true && !audioSdp.includes("m=audio")) {
    fail("Selkies/GStreamer hosted display advertised audio without an audio WebRTC offer", session);
  }
}
if (session.direct_network !== false || launch.data.direct_network !== false) {
  fail("hosted product display reported direct network authority", { launch: launch.data, session });
}
if (session.network_mode !== "runtime_net_only" || launch.data.network_mode !== "runtime_net_only") {
  fail("hosted product display must remain runtime_net_only", { launch: launch.data, session });
}

return {
  ok: true,
  page_id: launch.data.page_id,
  display_backend: session.display_backend || null,
  backend_class: session.backend_class,
  audio: session.audio,
  video: session.video,
  direct_network: false,
};
}

async function run() {
  const child = spawn(process.env.ADAPTER_BIN, [], { stdio: ["pipe", "pipe", "inherit"] });
  const exited = once(child, "close");
  const lines = createInterface({ input: child.stdout })[Symbol.asyncIterator]();
  const deadline = setTimeout(() => child.kill("SIGKILL"), 120000);
  let cleanup;
  let result;
  let failure;
  function recordFailure(error) {
    if (failure) console.error(error);
    else failure = error;
  }
  async function request(payload) {
    child.stdin.write(JSON.stringify(payload) + "\n");
    const line = await lines.next();
    if (line.done) fail(`adapter exited before ${payload.op} response`);
    const response = JSON.parse(line.value);
    if (response.status !== "ok") fail(`adapter ${payload.op} failed`, response);
    return response.data;
  }
  try {
    await request({ op: "init", config });
    try {
      const data = await request(launchRequest);
      cleanup = data?.runtime_cleanup;
      if (data?.schema !== "elastos.browser.engine.page/v1" ||
          cleanup?.page_id !== data.page_id || cleanup?.generation !== generation ||
          cleanup?.stream_id !== streamSession.stream_id || cleanup?.principal_id !== principal) {
        fail("launch did not return the exact cleanup binding", data);
      }
      result = validateDisplay({ data });
    } catch (error) {
      recordFailure(error);
    } finally {
      try {
        if (cleanup) {
          const closed = await request({ op: "close_page", page_id: cleanup.page_id,
            principal_id: principal, runtime_cleanup: cleanup });
          if (closed?.schema !== "elastos.browser.engine-cleanup-result/v2" ||
              !isDeepStrictEqual(closed.binding, cleanup) || closed.generation !== generation ||
              closed.page_id !== cleanup.page_id || closed.terminal !== true ||
              !["page_absent", "child_absent", "vm_absent", "route_absent", "socket_absent"]
                .every((effect) => closed.effects?.[effect] === true)) {
            fail("adapter close did not prove exact terminal cleanup", closed);
          }
        }
      } catch (error) {
        recordFailure(error);
      } finally {
        try {
          await request({ op: "shutdown" });
        } catch (error) {
          recordFailure(error);
        } finally {
          child.stdin.end();
          const [code, signal] = await exited;
          if (code !== 0 || signal) recordFailure(new Error(`adapter shutdown failed: ${code}/${signal}`));
        }
      }
    }
    if (failure) throw failure;
    console.log(JSON.stringify({ ...result, cleanup_terminal: true, shutdown: true }));
  } finally {
    clearTimeout(deadline);
    if (child.exitCode === null && child.signalCode === null) child.kill("SIGKILL");
    await exited;
  }
}
run().catch((error) => { console.error(error); process.exitCode = 1; });
NODE
