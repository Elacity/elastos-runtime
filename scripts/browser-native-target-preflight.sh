#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

usage() {
  cat <<'USAGE'
Usage:
  scripts/browser-native-target-preflight.sh \
    --out-dir /path/to/elastos/data/config \
    --browser-program /absolute/path/to/chromium-or-cef \
    [--native-audio] [--native-video] [--require-native-media] \
    [--artifact-out /path/to/native-preflight.json]

This target-host preflight fails closed. It generates the native Browser config
bundle, validates it against the actual provider capsules, and requires the
host-gated supervisor/proxy namespace smoke to pass without a skip.

Native audio/video are not assumed. Use --native-audio and --native-video only
after the selected native adapter has a real host audio/compositor path. Use
--require-native-media when the preflight is meant to prove product media
readiness instead of only network isolation.
When --artifact-out is provided, the final native preflight JSON receipt is
written to that path so manual UX evidence can hash the accepted proof.
USAGE
}

out_dir=""
browser_program=""
native_audio="0"
native_video="0"
require_native_media="0"
artifact_out=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --out-dir)
      shift
      out_dir="${1:-}"
      ;;
    --browser-program)
      shift
      browser_program="${1:-}"
      ;;
    --native-audio)
      native_audio="1"
      ;;
    --native-video)
      native_video="1"
      ;;
    --require-native-media)
      require_native_media="1"
      ;;
    --artifact-out)
      shift
      artifact_out="${1:-}"
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

if [[ -z "$out_dir" || -z "$browser_program" ]]; then
  usage >&2
  exit 1
fi

if [[ "$require_native_media" == "1" && ( "$native_audio" != "1" || "$native_video" != "1" ) ]]; then
  echo "--require-native-media requires both --native-audio and --native-video" >&2
  exit 1
fi

if [[ "$browser_program" != /* ]]; then
  echo "--browser-program must be absolute" >&2
  exit 1
fi

if [[ ! -x "$browser_program" ]]; then
  echo "--browser-program is not executable: $browser_program" >&2
  exit 1
fi
if [[ -n "$artifact_out" ]]; then
  artifact_parent="$(dirname "$artifact_out")"
  if [[ ! -d "$artifact_parent" ]]; then
    echo "--artifact-out parent directory does not exist: $artifact_parent" >&2
    exit 1
  fi
fi

host_capability_args=(--browser-program "$browser_program" --require-network-isolation)
if [[ "$require_native_media" == "1" ]]; then
  host_capability_args+=(--require-native-media)
fi
host_capability_report="$(mktemp /tmp/elastos-browser-native-host-capability-XXXXXX.json)"
if ! node "$repo_root/scripts/browser-native-host-capability.mjs" "${host_capability_args[@]}" >"$host_capability_report"; then
  echo "native host capability probe failed; target host is not ready for native Browser preflight" >&2
  cat "$host_capability_report" >&2 || true
  rm -f "$host_capability_report"
  exit 1
fi
rm -f "$host_capability_report"

browser_identity="$("$browser_program" --version 2>&1 | head -n 1 || true)"
browser_name="$(basename "$browser_program")"
if ! printf '%s %s\n' "$browser_name" "$browser_identity" \
  | grep -Eiq '(chromium|chrome|google-chrome|cefclient|cefsimple|cef|brave|msedge)'; then
  echo "--browser-program does not look like a Chromium/CEF-compatible browser: $browser_program" >&2
  if [[ -n "$browser_identity" ]]; then
    echo "reported identity: $browser_identity" >&2
  fi
  exit 1
fi

cd "$repo_root"

cargo build --quiet --manifest-path elastos/tools/browser-engine-supervisor/Cargo.toml
cargo build --quiet --manifest-path elastos/tools/browser-native-proxy-engine/Cargo.toml

supervisor_bin="$repo_root/elastos/tools/browser-engine-supervisor/target/debug/browser-engine-supervisor"
proxy_engine_bin="$repo_root/elastos/tools/browser-native-proxy-engine/target/debug/browser-native-proxy-engine"

operator_config_args=(
  --out-dir "$out_dir"
  --browser-program "$browser_program"
  --supervisor-bin "$supervisor_bin"
  --proxy-engine-bin "$proxy_engine_bin"
)
if [[ "$native_audio" == "1" ]]; then
  operator_config_args+=(--native-audio)
fi
if [[ "$native_video" == "1" ]]; then
  operator_config_args+=(--native-video)
fi

node scripts/browser-native-operator-config.mjs \
  "${operator_config_args[@]}"

if [[ "$require_native_media" == "1" ]]; then
  node -e '
    const fs = require("node:fs");
    const config = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
    const supervisor = JSON.parse(config.adapters[0].supervisor.env.ELASTOS_BROWSER_ENGINE_SUPERVISOR_CONFIG);
    if (supervisor.display_capabilities?.audio !== true || supervisor.display_capabilities?.video !== true) {
      console.error("native media readiness requires display_capabilities audio=true and video=true");
      process.exit(1);
    }
  ' "$out_dir/browser-engine-adapter.json"
fi

node -e 'const fs=require("fs"); const config=JSON.parse(fs.readFileSync(process.argv[1], "utf8")); console.log(JSON.stringify({op:"init", config})); console.log(JSON.stringify({op:"shutdown"}));' \
  "$out_dir/exit-provider.json" \
  | cargo run --quiet --manifest-path capsules/exit-provider/Cargo.toml >/dev/null

node -e 'const fs=require("fs"); const config=JSON.parse(fs.readFileSync(process.argv[1], "utf8")); console.log(JSON.stringify({op:"init", config})); console.log(JSON.stringify({op:"status"})); console.log(JSON.stringify({op:"shutdown"}));' \
  "$out_dir/browser-engine-adapter.json" \
  | cargo run --quiet --manifest-path capsules/browser-engine-adapter/Cargo.toml >/dev/null

smoke_output="$(scripts/browser-native-supervisor-proxy-smoke.sh)"
if [[ "$smoke_output" == *'"skipped":true'* ]]; then
  echo "native supervisor/proxy proof skipped; target host is not proven" >&2
  echo "$smoke_output" >&2
  exit 1
fi

read -r native_audio_proven native_video_proven < <(
  SMOKE_OUTPUT="$smoke_output" node -e '
    const raw = process.env.SMOKE_OUTPUT || "";
    const lines = raw.trim().split(/\n/).filter(Boolean);
    let proof = null;
    for (let index = lines.length - 1; index >= 0; index -= 1) {
      try {
        const parsed = JSON.parse(lines[index]);
        if (
          parsed &&
          Object.prototype.hasOwnProperty.call(parsed, "native_audio_proven") &&
          Object.prototype.hasOwnProperty.call(parsed, "native_video_proven")
        ) {
          proof = parsed;
          break;
        }
      } catch {
        // Ignore non-JSON build output and keep looking for the proof line.
      }
    }
    if (!proof) {
      console.error("native supervisor/proxy smoke did not emit native media proof");
      process.exit(1);
    }
    console.log(`${proof.native_audio_proven === true} ${proof.native_video_proven === true}`);
  '
)

if [[ "$require_native_media" == "1" && ( "$native_audio_proven" != "true" || "$native_video_proven" != "true" ) ]]; then
  echo "native media readiness requires the target proof to report native_audio_proven=true and native_video_proven=true" >&2
  echo "$smoke_output" >&2
  exit 1
fi

echo "$smoke_output"
preflight_json="$(printf '{"schema":"elastos.browser.native-target-preflight/v1","ok":true,"out_dir":"%s","browser_program":"%s","network_mode":"runtime_net_only","direct_network":false,"native_audio_declared":%s,"native_video_declared":%s,"native_audio_proven":%s,"native_video_proven":%s,"native_media_required":%s}' \
  "$out_dir" \
  "$browser_program" \
  "$([[ "$native_audio" == "1" ]] && echo true || echo false)" \
  "$([[ "$native_video" == "1" ]] && echo true || echo false)" \
  "$native_audio_proven" \
  "$native_video_proven" \
  "$([[ "$require_native_media" == "1" ]] && echo true || echo false)")"
if [[ -n "$artifact_out" ]]; then
  printf '%s\n' "$preflight_json" >"$artifact_out"
fi
printf '%s\n' "$preflight_json"
