#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

usage() {
  cat <<'USAGE'
Usage:
  scripts/browser-hosted-provider-bakeoff.sh \
    --candidate browserbox|kasm-workspaces|selkies|<id> \
    --adapter-config /path/to/browser-engine-adapter.json \
    --cdp-endpoint http://127.0.0.1:PORT \
    [--media-url https://www.w3schools.com/html/mov_bbb.mp4] \
    [--youtube-url https://www.youtube.com/watch?v=dQw4w9WgXcQ] \
    [--hold-ms 10000] \
    [--resize-width 1000 --resize-height 700] \
    [--timeout-ms 120000] \
    [--artifact-out /path/to/hosted-bakeoff.json] \
    [--skip-youtube]

Runs the machine portion of the hosted Browser provider bake-off.

The candidate must already be exposed behind browser-engine-adapter using the
hosted_remote_browser or selkies_gstreamer product-compositor contract. This
script does not launch BrowserBox, Kasm, or Selkies directly; it verifies the
Runtime-facing contract only.

Acceptance:
  1. Hosted provider candidate gate passes:
     product display, audio/video, datachannel input, navigation, controlled
     media quality, Runtime wallet bridge, Glide wallet flow, direct_network=false.
  2. Product-compositor YouTube stress passes. --skip-youtube is only for a
     partial diagnostic run and never produces an accepted bake-off artifact.
  3. Manual UX review is still required after machine gates:
     typing, scrolling, page stability, perceived latency, YouTube playback.

When --artifact-out is provided, the exact JSON emitted on stdout is also
written to that path so manual UX evidence can hash the accepted machine proof.
USAGE
}

candidate=""
adapter_config=""
cdp_endpoint=""
media_url="https://www.w3schools.com/html/mov_bbb.mp4"
youtube_url="https://www.youtube.com/watch?v=dQw4w9WgXcQ"
hold_ms="10000"
resize_width="1000"
resize_height="700"
timeout_ms="120000"
artifact_out=""
skip_youtube="0"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --candidate)
      candidate="${2:-}"
      shift 2
      ;;
    --adapter-config)
      adapter_config="${2:-}"
      shift 2
      ;;
    --cdp-endpoint)
      cdp_endpoint="${2:-}"
      shift 2
      ;;
    --media-url)
      media_url="${2:-}"
      shift 2
      ;;
    --youtube-url)
      youtube_url="${2:-}"
      shift 2
      ;;
    --hold-ms)
      hold_ms="${2:-}"
      shift 2
      ;;
    --resize-width)
      resize_width="${2:-}"
      shift 2
      ;;
    --resize-height)
      resize_height="${2:-}"
      shift 2
      ;;
    --timeout-ms)
      timeout_ms="${2:-}"
      shift 2
      ;;
    --artifact-out)
      artifact_out="${2:-}"
      shift 2
      ;;
    --skip-youtube)
      skip_youtube="1"
      shift
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
done

if [[ -z "$candidate" || -z "$adapter_config" || -z "$cdp_endpoint" ]]; then
  usage >&2
  exit 1
fi
if [[ ! "$candidate" =~ ^[A-Za-z0-9_.:-]+$ ]]; then
  echo "--candidate must be a safe identifier" >&2
  exit 1
fi
if [[ ! -f "$adapter_config" ]]; then
  echo "--adapter-config does not exist: $adapter_config" >&2
  exit 1
fi
if [[ ! "$cdp_endpoint" =~ ^https?://(127\.0\.0\.1|localhost):[0-9]+/?$ ]]; then
  echo "--cdp-endpoint must be an operator-private loopback HTTP endpoint" >&2
  exit 1
fi
if [[ ! "$media_url" =~ ^https?:// || ! "$youtube_url" =~ ^https?:// ]]; then
  echo "--media-url and --youtube-url must use http or https" >&2
  exit 1
fi
if [[ ! "$hold_ms" =~ ^[0-9]+$ || "$hold_ms" -lt 0 || "$hold_ms" -gt 300000 ]]; then
  echo "--hold-ms must be an integer from 0 to 300000" >&2
  exit 1
fi
if [[ ! "$resize_width" =~ ^[0-9]+$ || ! "$resize_height" =~ ^[0-9]+$ || "$resize_width" -lt 320 || "$resize_height" -lt 240 ]]; then
  echo "--resize-width/--resize-height must be integers at least 320x240" >&2
  exit 1
fi
if [[ ! "$timeout_ms" =~ ^[0-9]+$ || "$timeout_ms" -lt 5000 || "$timeout_ms" -gt 120000 ]]; then
  echo "--timeout-ms must be an integer from 5000 to 120000" >&2
  exit 1
fi
if [[ -n "$artifact_out" ]]; then
  artifact_parent="$(dirname "$artifact_out")"
  if [[ ! -d "$artifact_parent" ]]; then
    echo "--artifact-out parent directory does not exist: $artifact_parent" >&2
    exit 1
  fi
fi

cd "$repo_root"

tmp_dir="$(mktemp -d)"
cleanup() {
  rm -rf "$tmp_dir"
}
trap cleanup EXIT

candidate_out="$tmp_dir/candidate.out"
youtube_out="$tmp_dir/youtube.out"

set +e
scripts/browser-hosted-provider-candidate-smoke.sh \
  --adapter-config "$adapter_config" \
  --cdp-endpoint "$cdp_endpoint" \
  --media-url "$media_url" \
  --hold-ms "$hold_ms" \
  --resize-width "$resize_width" \
  --resize-height "$resize_height" \
  --timeout-ms "$timeout_ms" \
  >"$candidate_out" 2>&1
candidate_status="$?"
set -e

youtube_status="0"
if [[ "$skip_youtube" == "0" && "$candidate_status" == "0" ]]; then
  set +e
  scripts/browser-hosted-product-webrtc-smoke.sh \
    --adapter-config "$adapter_config" \
    --cdp-endpoint "$cdp_endpoint" \
    --require-media \
    --url "$youtube_url" \
    --hold-ms "$hold_ms" \
    --resize-width "$resize_width" \
    --resize-height "$resize_height" \
    --timeout-ms "$timeout_ms" \
    >"$youtube_out" 2>&1
  youtube_status="$?"
  set -e
else
  : >"$youtube_out"
fi

CANDIDATE="$candidate" \
SKIP_YOUTUBE="$skip_youtube" \
CANDIDATE_STATUS="$candidate_status" \
YOUTUBE_STATUS="$youtube_status" \
CANDIDATE_OUT="$candidate_out" \
YOUTUBE_OUT="$youtube_out" \
ARTIFACT_OUT="$artifact_out" \
node --input-type=module - <<'NODE'
import fs from "node:fs";

import { requiredManualChecksForSchema } from "./scripts/browser-manual-ux-checks.mjs";

function read(file) {
  return fs.readFileSync(file, "utf8");
}

function lastJson(raw) {
  const lines = raw.trim().split(/\r?\n/).filter(Boolean);
  for (let index = lines.length - 1; index >= 0; index -= 1) {
    const line = lines[index].trim();
    if (!line.startsWith("{")) continue;
    try {
      return JSON.parse(line);
    } catch {
      continue;
    }
  }
  return null;
}

const candidateRaw = read(process.env.CANDIDATE_OUT);
const youtubeRaw = read(process.env.YOUTUBE_OUT);
const candidateStatus = Number(process.env.CANDIDATE_STATUS || "1");
const youtubeStatus = Number(process.env.YOUTUBE_STATUS || "1");
const skipYoutube = process.env.SKIP_YOUTUBE === "1";
const candidateGate = lastJson(candidateRaw);
const youtubeStress = skipYoutube ? null : lastJson(youtubeRaw);
const candidateOk = candidateStatus === 0 && candidateGate?.ok === true;
const youtubeOk = !skipYoutube && youtubeStatus === 0 && youtubeStress?.ok === true;
const machineAccepted = candidateOk && youtubeOk;

const artifact = {
  ok: machineAccepted,
  schema: "elastos.browser.hosted-provider-bakeoff/v1",
  candidate: process.env.CANDIDATE,
  candidate_gate: {
    ok: candidateOk,
    status: candidateStatus,
    result: candidateGate,
    error_tail: candidateOk ? null : candidateRaw.trim().split(/\r?\n/).slice(-20),
  },
  youtube_stress: skipYoutube ? {
    skipped: true,
    reason: "operator skipped product-compositor YouTube stress gate",
  } : {
    ok: youtubeOk,
    status: youtubeStatus,
    result: youtubeStress,
    error_tail: youtubeOk ? null : youtubeRaw.trim().split(/\r?\n/).slice(-20),
  },
  partial_candidate_ok: candidateOk,
  manual_ux_required: true,
  manual_ux_schema: "elastos.browser.manual-ux/v1",
  manual_ux_checks: requiredManualChecksForSchema("elastos.browser.hosted-provider-bakeoff/v1"),
  product_acceptance: machineAccepted
    ? "machine gates passed; manual UX review still required"
    : skipYoutube
      ? "rejected because product-compositor YouTube stress was skipped"
      : "rejected by machine gate",
};

const serialized = JSON.stringify(artifact, null, 2);
if (process.env.ARTIFACT_OUT) {
  fs.writeFileSync(process.env.ARTIFACT_OUT, `${serialized}\n`);
}
console.log(serialized);

if (!machineAccepted) {
  process.exit(1);
}
NODE
