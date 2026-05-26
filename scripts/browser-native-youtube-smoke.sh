#!/usr/bin/env bash
set -euo pipefail

if [[ -z "${DISPLAY:-}" && -z "${ELASTOS_BROWSER_NATIVE_YOUTUBE_XVFB:-}" ]]; then
  exec env ELASTOS_BROWSER_NATIVE_YOUTUBE_XVFB=1 xvfb-run -a "$0" "$@"
fi

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
tmp_dir="$(mktemp -d)"
local_exit_pid=""
engine_pid=""

cleanup() {
  if [[ -n "$engine_pid" ]]; then
    kill "$engine_pid" >/dev/null 2>&1 || true
  fi
  if [[ -n "$local_exit_pid" ]]; then
    kill "$local_exit_pid" >/dev/null 2>&1 || true
  fi
  rm -rf "$tmp_dir"
}
trap cleanup EXIT

cd "$repo_root"

cargo build --quiet --manifest-path elastos/tools/browser-local-exit/Cargo.toml
cargo build --quiet --manifest-path elastos/tools/browser-native-proxy-engine/Cargo.toml

browser_program="${BROWSER_NATIVE_BROWSER_PROGRAM:-}"
if [[ -z "$browser_program" ]]; then
  browser_program="$(cd elastos/tools/browser-playwright-engine && node - <<'NODE'
import { chromium } from 'playwright';
console.log(chromium.executablePath());
NODE
)"
fi

relay_socket="$tmp_dir/local-exit.sock"
debug_port="$((43000 + RANDOM % 10000))"
youtube_url="${BROWSER_YOUTUBE_URL:-https://www.youtube.com/embed/dQw4w9WgXcQ?autoplay=1&mute=0&controls=1&origin=https%3A%2F%2Felastos.elacitylabs.com}"
referer="${BROWSER_YOUTUBE_REFERER:-https://elastos.elacitylabs.com/apps/home/}"

ELASTOS_BROWSER_LOCAL_EXIT_CONFIG="$(node - <<NODE
const config = {
  schema: "elastos.browser.local-exit.config/v1",
  relay_ipc_path: "$relay_socket",
  allowed_hosts: ["*"],
  allowed_schemes: ["tcp", "tls"],
  allowed_ports: [80, 443],
  address_family: process.env.BROWSER_SMOKE_ADDRESS_FAMILY || "prefer_ipv4",
  allow_private_targets: false,
  replace_existing_socket: true
};
if (process.env.BROWSER_SMOKE_UPSTREAM_HTTP_PROXY) {
  config.upstream_http_proxy = {
    url: process.env.BROWSER_SMOKE_UPSTREAM_HTTP_PROXY,
  };
  if (process.env.BROWSER_SMOKE_UPSTREAM_PROXY_AUTHORIZATION) {
    config.upstream_http_proxy.authorization_header = process.env.BROWSER_SMOKE_UPSTREAM_PROXY_AUTHORIZATION;
  }
}
console.log(JSON.stringify(config));
NODE
)" \
  elastos/tools/browser-local-exit/target/debug/browser-local-exit \
  >"$tmp_dir/local-exit.out" 2>"$tmp_dir/local-exit.err" &
local_exit_pid="$!"

for _ in {1..100}; do
  [[ -S "$relay_socket" ]] && break
  sleep 0.05
done
if [[ ! -S "$relay_socket" ]]; then
  cat "$tmp_dir/local-exit.err" >&2 || true
  exit 1
fi

engine_config="$(node - <<NODE
console.log(JSON.stringify({
  schema: "elastos.browser.native-proxy-engine.config/v1",
  browser_program: "$browser_program",
  browser_args: [
    "--proxy-server={proxy_url}",
    "--proxy-bypass-list=<-loopback>",
    "--host-resolver-rules=MAP * ~NOTFOUND, EXCLUDE 127.0.0.1",
    "--disable-background-networking",
    "--disable-component-update",
    "--disable-default-apps",
    "--disable-sync",
    "--disable-quic",
    "--autoplay-policy=no-user-gesture-required",
    "--no-first-run",
    "--no-sandbox",
    "--user-data-dir=$tmp_dir/profile",
    "--remote-debugging-port=$debug_port",
    "about:blank"
  ],
  relay_ipc_path: "$relay_socket",
  startup_grace_ms: 1000
}));
NODE
)"

ELASTOS_BROWSER_NATIVE_PROXY_ENGINE_CONFIG="$engine_config" \
ELASTOS_BROWSER_ENGINE_URL="about:blank" \
ELASTOS_BROWSER_ENGINE_STREAM_ID="stream:native-youtube-smoke" \
  elastos/tools/browser-native-proxy-engine/target/debug/browser-native-proxy-engine \
  >"$tmp_dir/native-engine.out" 2>"$tmp_dir/native-engine.err" &
engine_pid="$!"

for _ in {1..100}; do
  if curl -fsS "http://127.0.0.1:$debug_port/json/version" >/dev/null 2>&1; then
    break
  fi
  if ! kill -0 "$engine_pid" >/dev/null 2>&1; then
    cat "$tmp_dir/native-engine.err" >&2 || true
    exit 1
  fi
  sleep 0.1
done

YOUTUBE_URL="$youtube_url" \
YOUTUBE_REFERER="$referer" \
DEBUG_PORT="$debug_port" \
  node --input-type=module - <<'NODE'
import playwright from './elastos/tools/browser-playwright-engine/node_modules/playwright/index.js';
const { chromium } = playwright;

const endpoint = `http://127.0.0.1:${process.env.DEBUG_PORT}`;
const browser = await chromium.connectOverCDP(endpoint);
const context = browser.contexts()[0] || await browser.newContext();
let page = context.pages()[0] || await context.newPage();
await page.goto(process.env.YOUTUBE_URL, {
  waitUntil: 'domcontentloaded',
  timeout: 45000,
  referer: process.env.YOUTUBE_REFERER,
});

async function mediaSnapshot() {
  const elements = [];
  const frame_summaries = [];
  for (const [frameIndex, frame] of page.frames().entries()) {
    const summary = await frame.evaluate((frameIndexInPage) => ({
      frame_index: frameIndexInPage,
      frame_url: String(window.location.href || ''),
      title: String(document.title || ''),
      text_sample: String(document.body?.innerText || '').replace(/\s+/g, ' ').trim().slice(0, 240),
    }), frameIndex).catch(() => null);
    if (summary) {
      frame_summaries.push(summary);
    }
    const items = await frame.evaluate((frameIndexInPage) => {
      return Array.from(document.querySelectorAll('video,audio')).map((element, index) => ({
        frame_index: frameIndexInPage,
        frame_url: String(window.location.href || ''),
        title: String(document.title || ''),
        text_sample: String(document.body?.innerText || '').replace(/\s+/g, ' ').trim().slice(0, 240),
        index,
        tag: element.tagName.toLowerCase(),
        current_time: Number(element.currentTime || 0),
        duration: Number.isFinite(Number(element.duration)) ? Number(element.duration) : null,
        paused: Boolean(element.paused),
        muted: Boolean(element.muted),
        volume: Number(element.volume),
        ready_state: Number(element.readyState),
        current_src: String(element.currentSrc || element.src || ''),
        video_width: Number(element.videoWidth || 0),
        video_height: Number(element.videoHeight || 0),
        audio_decoded_bytes: Number(element.webkitAudioDecodedByteCount || 0),
        video_decoded_bytes: Number(element.webkitVideoDecodedByteCount || 0),
      }));
    }, frameIndex).catch(() => []);
    elements.push(...items);
  }
  return { elements, frame_summaries };
}

await page.mouse.click(512, 360).catch(() => {});
let first = null;
let last = null;
let lastMedia = null;
for (let attempt = 0; attempt < 24; attempt += 1) {
  await page.waitForTimeout(1000);
  const media = await mediaSnapshot();
  lastMedia = media;
  const text = media.frame_summaries.map((item) => `${item.title} ${item.text_sample}`).join(' ');
  if (/not a bot|kein bot|confirm you.?re not a bot|sign in to confirm/i.test(text)) {
    throw new Error(`YouTube upstream bot challenge on selected Browser Exit: ${text.slice(0, 320)}`);
  }
  const video = media.elements.find((item) => item.tag === 'video' && item.ready_state >= 2);
  if (!video) {
    continue;
  }
  if (!first) {
    first = video;
    if (video.paused) {
      await page.keyboard.press('Space').catch(() => {});
      await page.mouse.click(512, 360).catch(() => {});
    }
  }
  last = video;
  const timeDelta = Number(last.current_time || 0) - Number(first.current_time || 0);
  const videoDelta = Number(last.video_decoded_bytes || 0) - Number(first.video_decoded_bytes || 0);
  const audioDelta = Number(last.audio_decoded_bytes || 0) - Number(first.audio_decoded_bytes || 0);
  if (timeDelta >= 2 && videoDelta > 0 && audioDelta > 0 && !last.paused && !last.muted) {
    console.log(JSON.stringify({
      ok: true,
      actual_url: page.url(),
      current_time_delta: timeDelta,
      video_decoded_delta: videoDelta,
      audio_decoded_delta: audioDelta,
      direct_network: false,
      browser: 'native-proxy-engine',
    }));
    await browser.close().catch(() => {});
    process.exit(0);
  }
}
throw new Error(`native YouTube playback did not reach stable video+audio decode: ${JSON.stringify({ first, last, lastMedia })}`);
NODE
