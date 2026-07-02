#!/usr/bin/env bash
set -euo pipefail

find_node() {
  if [[ -n "${ELASTOS_NODE_BIN:-}" && -x "${ELASTOS_NODE_BIN}" ]]; then
    printf '%s\n' "${ELASTOS_NODE_BIN}"
    return 0
  fi
  if command -v node >/dev/null 2>&1; then
    command -v node
    return 0
  fi
  local bundled="${HOME}/.elastos/node/node-v22.13.1-darwin-arm64/bin/node"
  if [[ -x "$bundled" ]]; then
    printf '%s\n' "$bundled"
    return 0
  fi
  return 1
}

usage() {
  cat >&2 <<'USAGE'
Usage: scripts/browser-mac-vm-proof.sh [--artifact-out proof.json]

Collects a Mac Browser VM machine proof from the local source-home gateway.
This does not replace hash-bound manual UX/product-audio acceptance.

Environment:
  ELASTOS_BASE_URL                         default: http://localhost:61180
  ELASTOS_BROWSER_MAC_VM_PROOF_URL         default: https://ela.city/channels
  ELASTOS_BROWSER_MAC_VM_PROOF_HOLD_MS     default: 12000
  ELASTOS_BROWSER_MAC_VM_MAX_VIDEO_READY_MS default: 12000
  ELASTOS_BROWSER_MAC_VM_MAX_NAV_MS         default: 15000
  ELASTOS_BROWSER_MAC_VM_MAX_CONTROL_UPTIME_MS default: 300000
  ELASTOS_BROWSER_MAC_VM_MIN_DECODED_DELTA  default: 5
  ELASTOS_BROWSER_MAC_VM_MAX_DROPPED_FRAMES default: 1
  ELASTOS_BROWSER_MAC_VM_MIN_IMAGES          default: 1
  ELASTOS_BROWSER_MAC_VM_MIN_CLICKABLES      default: 1
  ELASTOS_BROWSER_MAC_VM_EXPECT_DPR         default: 1
  ELASTOS_BROWSER_MAC_VM_EXPECT_VIEWPORT_WIDTH  default: 1280
  ELASTOS_BROWSER_MAC_VM_EXPECT_VIEWPORT_HEIGHT default: 720
  HOME_VIRTUAL_AUTH_BROWSER_OPEN_VIEWPORT_WIDTH  defaults to ELASTOS_BROWSER_MAC_VM_EXPECT_VIEWPORT_WIDTH
  HOME_VIRTUAL_AUTH_BROWSER_OPEN_VIEWPORT_HEIGHT defaults to ELASTOS_BROWSER_MAC_VM_EXPECT_VIEWPORT_HEIGHT
  ELASTOS_BROWSER_MAC_VM_CLICK_HREF_RE      optional href regex for in-page click proof
  ELASTOS_BROWSER_MAC_VM_CLICK_EXPECT_URL_RE optional address/status URL regex after that click
  ELASTOS_BROWSER_MAC_VM_CLICK_NAV_TIMEOUT_MS default: 30000
  HOME_VIRTUAL_AUTH_BROWSER_DIAGNOSTIC_CLICK_TEXT_RE optional text regex to click in page diagnostics
  HOME_VIRTUAL_AUTH_BROWSER_DIAGNOSTIC_CLICK_EXPECT_TEXT_RE optional text regex expected after diagnostic click
  ELASTOS_BROWSER_MAC_VM_PROFILE_RESET_PROOF default: 0; set 1 to reset the virtual test principal profile after proof pages close
  ELASTOS_BROWSER_MAC_VM_PROOF_AUTH_PROFILE optional persistent virtual-auth profile dir for authenticated ela.city proof
  HOME_VIRTUAL_AUTH_BROWSER_DIAGNOSTIC_CLICK_OPTIONAL default: 1 for this proof
  ELASTOS_BROWSER_VM_CONTROL_SOCKET        default: /tmp/elastos-browser-vm-control-darwin-arm64.sock
  ELASTOS_MAC_BROWSER_DATA_DIR             default: ~/elastos-mac-test-home/Library/Application Support/elastos
  ELASTOS_NODE_BIN                         optional node binary
USAGE
}

artifact_out=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --artifact-out)
      artifact_out="${2:-}"
      if [[ -z "$artifact_out" ]]; then
        echo "--artifact-out requires a path" >&2
        exit 2
      fi
      shift 2
      ;;
    --help|-h)
      usage
      exit 0
      ;;
    *)
      echo "unknown argument: $1" >&2
      usage
      exit 2
      ;;
  esac
done

node_bin="$(find_node || true)"
if [[ -z "$node_bin" ]]; then
  echo "node not found. Install Node or set ELASTOS_NODE_BIN to an executable node binary." >&2
  exit 2
fi

base_url="${ELASTOS_BASE_URL:-http://localhost:61180}"
base_url="${base_url%/}"
home_url="${base_url}/apps/home/"
proof_url="${ELASTOS_BROWSER_MAC_VM_PROOF_URL:-https://ela.city/channels}"
hold_ms="${ELASTOS_BROWSER_MAC_VM_PROOF_HOLD_MS:-12000}"
expected_viewport_width="${ELASTOS_BROWSER_MAC_VM_EXPECT_VIEWPORT_WIDTH:-1280}"
expected_viewport_height="${ELASTOS_BROWSER_MAC_VM_EXPECT_VIEWPORT_HEIGHT:-720}"
export HOME_VIRTUAL_AUTH_BROWSER_OPEN_VIEWPORT_WIDTH="${HOME_VIRTUAL_AUTH_BROWSER_OPEN_VIEWPORT_WIDTH:-$expected_viewport_width}"
export HOME_VIRTUAL_AUTH_BROWSER_OPEN_VIEWPORT_HEIGHT="${HOME_VIRTUAL_AUTH_BROWSER_OPEN_VIEWPORT_HEIGHT:-$expected_viewport_height}"
click_href_re="${ELASTOS_BROWSER_MAC_VM_CLICK_HREF_RE:-${HOME_VIRTUAL_AUTH_BROWSER_UI_CLICK_HREF_RE:-}}"
click_expect_url_re="${ELASTOS_BROWSER_MAC_VM_CLICK_EXPECT_URL_RE:-${HOME_VIRTUAL_AUTH_BROWSER_UI_CLICK_EXPECT_URL_RE:-}}"
click_nav_timeout_ms="${ELASTOS_BROWSER_MAC_VM_CLICK_NAV_TIMEOUT_MS:-${HOME_VIRTUAL_AUTH_BROWSER_UI_CLICK_NAV_TIMEOUT_MS:-30000}}"
embedded_open_urls="${HOME_VIRTUAL_AUTH_BROWSER_OPEN_URLS:-}"
if [[ -n "$click_href_re" ]]; then
  embedded_open_urls="$proof_url"
fi
control_socket="${ELASTOS_BROWSER_VM_CONTROL_SOCKET:-/tmp/elastos-browser-vm-control-darwin-arm64.sock}"
data_dir="${ELASTOS_MAC_BROWSER_DATA_DIR:-${HOME}/elastos-mac-test-home/Library/Application Support/elastos}"
installed_home="${data_dir}/capsules/home/browser/index.html"
source_home="capsules/home/browser/index.html"
tmp_dir="$(mktemp -d /tmp/elastos-browser-mac-vm-proof-XXXXXX)"

cleanup() {
  rm -rf "$tmp_dir"
}
trap cleanup EXIT

read_control_status() {
  local phase="$1"
  local out="$2"
  local status
  if curl -fsS -m 2 --unix-socket "$control_socket" http://browser-engine/status >"$out" 2>/dev/null; then
    return 0
  fi
  status="$?"
  printf '{"ok":false,"available":false,"phase":"%s","error":"control_status_unavailable","curl_status":%s}\n' "$phase" "$status" >"$out"
}

home_http_code="$(curl -fsS -o /dev/null -w '%{http_code}' "$home_url")"

installed_home_sha=""
source_home_sha=""
if [[ -f "$installed_home" ]]; then
  installed_home_sha="$(shasum -a 256 "$installed_home" | awk '{print $1}')"
fi
if [[ -f "$source_home" ]]; then
  source_home_sha="$(shasum -a 256 "$source_home" | awk '{print $1}')"
fi

control_before="$tmp_dir/control-before.json"
control_after="$tmp_dir/control-after.json"
embedded_json="$tmp_dir/embedded.json"
open_json="$tmp_dir/open.json"
proof_json="$tmp_dir/proof.json"
auth_profile="${ELASTOS_BROWSER_MAC_VM_PROOF_AUTH_PROFILE:-}"
virtual_auth_env=()
virtual_auth_persistent=0
virtual_auth_cleanup="${HOME_VIRTUAL_AUTH_CLEANUP:-1}"

if [[ -n "$auth_profile" ]]; then
  mkdir -p "$auth_profile"
  virtual_auth_persistent=1
  virtual_auth_cleanup="${HOME_VIRTUAL_AUTH_CLEANUP:-0}"
  virtual_auth_env+=(
    "HOME_VIRTUAL_AUTH_PROFILE=$auth_profile"
    "HOME_VIRTUAL_AUTH_PRESERVE_PROFILE=1"
    "HOME_VIRTUAL_AUTH_CLEANUP=$virtual_auth_cleanup"
    "HOME_VIRTUAL_AUTH_NAME=${HOME_VIRTUAL_AUTH_NAME:-Mac Browser VM Proof}"
  )
fi

run_virtual_auth_smoke() {
  if [[ "${#virtual_auth_env[@]}" -gt 0 ]]; then
    env "${virtual_auth_env[@]}" "$@"
  else
    env "$@"
  fi
}

read_control_status before "$control_before"

run_virtual_auth_smoke \
ELASTOS_BASE_URL="$base_url" \
HOME_VIRTUAL_AUTH_BROWSER=1 \
HOME_VIRTUAL_AUTH_BROWSER_EMBEDDED_UI_INPUT=1 \
HOME_VIRTUAL_AUTH_BROWSER_OPEN_URLS="$embedded_open_urls" \
HOME_VIRTUAL_AUTH_BROWSER_UI_NAV_URL="$proof_url" \
HOME_VIRTUAL_AUTH_BROWSER_UI_CLICK_HREF_RE="$click_href_re" \
HOME_VIRTUAL_AUTH_BROWSER_UI_CLICK_EXPECT_URL_RE="$click_expect_url_re" \
HOME_VIRTUAL_AUTH_BROWSER_UI_CLICK_NAV_TIMEOUT_MS="$click_nav_timeout_ms" \
HOME_VIRTUAL_AUTH_BROWSER_REMOTE_VIDEO_TIMEOUT_MS=180000 \
HOME_VIRTUAL_AUTH_BROWSER_UI_PAGE_ID_TIMEOUT_MS=180000 \
HOME_VIRTUAL_AUTH_BROWSER_INPUT_CLICK_X=640 \
HOME_VIRTUAL_AUTH_BROWSER_INPUT_CLICK_Y=350 \
"$node_bin" scripts/home-passkey-virtual-auth-smoke.mjs >"$embedded_json"

run_virtual_auth_smoke \
ELASTOS_BASE_URL="$base_url" \
HOME_VIRTUAL_AUTH_BROWSER=1 \
HOME_VIRTUAL_AUTH_BROWSER_OPEN=1 \
HOME_VIRTUAL_AUTH_BROWSER_SUMMARY=1 \
HOME_VIRTUAL_AUTH_BROWSER_DIAGNOSTICS=1 \
HOME_VIRTUAL_AUTH_BROWSER_PROFILE_RESET="${ELASTOS_BROWSER_MAC_VM_PROFILE_RESET_PROOF:-0}" \
HOME_VIRTUAL_AUTH_BROWSER_DIAGNOSTIC_CLICK_OPTIONAL="${HOME_VIRTUAL_AUTH_BROWSER_DIAGNOSTIC_CLICK_OPTIONAL:-1}" \
HOME_VIRTUAL_AUTH_BROWSER_OPEN_URLS="$proof_url" \
HOME_VIRTUAL_AUTH_BROWSER_OPEN_HOLD_MS="$hold_ms" \
HOME_VIRTUAL_AUTH_BROWSER_REMOTE_VIDEO_TIMEOUT_MS=180000 \
"$node_bin" scripts/home-passkey-virtual-auth-smoke.mjs >"$open_json"

read_control_status after "$control_after"

HOME_HTTP_CODE="$home_http_code" \
INSTALLED_HOME_SHA="$installed_home_sha" \
SOURCE_HOME_SHA="$source_home_sha" \
HOME_URL="$home_url" \
PROOF_URL="$proof_url" \
CONTROL_SOCKET="$control_socket" \
CONTROL_BEFORE="$control_before" \
CONTROL_AFTER="$control_after" \
EMBEDDED_JSON="$embedded_json" \
OPEN_JSON="$open_json" \
VIRTUAL_AUTH_PERSISTENT="$virtual_auth_persistent" \
VIRTUAL_AUTH_CLEANUP="$virtual_auth_cleanup" \
"$node_bin" - "$proof_json" <<'NODE'
const fs = require("node:fs");

const [
  outPath,
] = process.argv.slice(2);

function readJson(path) {
  return JSON.parse(fs.readFileSync(path, "utf8"));
}

const embedded = readJson(process.env.EMBEDDED_JSON);
const opened = readJson(process.env.OPEN_JSON);
const controlBefore = readJson(process.env.CONTROL_BEFORE);
const controlAfter = readJson(process.env.CONTROL_AFTER);
const embeddedUi = embedded.browser_embedded_ui_input || {};
const ready = embeddedUi.video?.ready || {};
const afterClick = embeddedUi.video?.after_click || {};
const navigation = embeddedUi.navigation || {};
const clickNavigation = embeddedUi.click_navigation || null;
const openPage = opened.browser_open?.pages?.[0] || {};
const openStreamSession = openPage.stream_session || null;
const diagnostics = openPage.diagnostics || {};
const closeResults = opened.browser_open?.close_results || [];
const profileReset = opened.browser_profile_reset || null;

function number(value, fallback = 0) {
  const parsed = Number(value);
  return Number.isFinite(parsed) ? parsed : fallback;
}

function boundedEnv(name, fallback) {
  const value = number(process.env[name], fallback);
  return value > 0 ? value : fallback;
}

function closeEnough(left, right, tolerance = 2) {
  return Number.isFinite(left)
    && Number.isFinite(right)
    && Math.abs(left - right) <= tolerance;
}

function aspectCloseEnough(width, height, expected, tolerance = 0.02) {
  return Number.isFinite(width)
    && Number.isFinite(height)
    && height > 0
    && Number.isFinite(expected)
    && Math.abs((width / height) - expected) <= tolerance;
}

function publicPageDiagnostics(entry) {
  const images = Array.isArray(entry.images) ? entry.images : [];
  const visiblePendingImageSamples = images
    .filter((image) => image && image.visible === true && image.complete === false)
    .slice(0, 12);
  const visibleBrokenImageSamples = images
    .filter((image) => image && image.visible === true && image.complete === true && (number(image.natural_width) <= 0 || number(image.natural_height) <= 0))
    .slice(0, 12);
  return {
    url: entry.url || null,
    title: entry.title || null,
    ready_state: entry.ready_state || null,
    viewport_width: entry.viewport_width || null,
    viewport_height: entry.viewport_height || null,
    device_pixel_ratio: entry.device_pixel_ratio || null,
    body_text: entry.body_text || null,
    body_child_count: entry.body_child_count ?? null,
    root_child_count: entry.root_child_count ?? null,
    image_count: entry.image_count || 0,
    visible_image_count: entry.visible_image_count || 0,
    broken_image_count: entry.broken_image_count || 0,
    pending_image_count: entry.pending_image_count || 0,
    pending_ipfs_image_count: entry.pending_ipfs_image_count || 0,
    visible_broken_image_count: number(entry.visible_broken_image_count, visibleBrokenImageSamples.length),
    visible_pending_image_count: number(entry.visible_pending_image_count, visiblePendingImageSamples.length),
    visible_pending_ipfs_image_count: entry.visible_pending_ipfs_image_count || 0,
    visible_pending_image_samples: visiblePendingImageSamples,
    visible_broken_image_samples: visibleBrokenImageSamples,
    resource_count: entry.resource_count || 0,
    resource_samples: Array.isArray(entry.resources) ? entry.resources.slice(0, 80) : [],
    ipfs_resource_samples: Array.isArray(entry.ipfs_resources) ? entry.ipfs_resources.slice(0, 80) : [],
    clickable_count: entry.clickable_count || 0,
    clickable_elements: Array.isArray(entry.clickable_elements) ? entry.clickable_elements.slice(0, 80) : [],
    visible_text_sample_count: entry.visible_text_sample_count || 0,
    visible_text_samples: Array.isArray(entry.visible_text_samples) ? entry.visible_text_samples.slice(0, 80) : [],
    dialog_count: entry.dialog_count || 0,
    dialog_elements: Array.isArray(entry.dialog_elements) ? entry.dialog_elements.slice(0, 20) : [],
    navigation: entry.navigation || null,
  };
}

function turnIceServerCount(displaySession, requireCredential = false) {
  const iceServers = Array.isArray(displaySession?.ice_servers)
    ? displaySession.ice_servers
    : [];
  return iceServers.filter((server) => {
    const urls = Array.isArray(server?.urls) ? server.urls : [server?.urls].filter(Boolean);
    const hasTurn = urls.some((url) => /^turns?:/i.test(String(url || "").trim()));
    if (!hasTurn) {
      return false;
    }
    if (!requireCredential) {
      return true;
    }
    if (server?.credential_present === true) {
      return true;
    }
    return Number(server?.credential_length || 0) > 0;
  }).length;
}

function publicBrowserRoute(session) {
  if (!session || typeof session !== "object") {
    return null;
  }
  return {
    schema: session.schema || null,
    backend: session.backend || null,
    byte_transport: session.byte_transport || null,
    grant_id: session.grant_id || null,
    stream_id: session.stream_id || null,
    target: session.target || null,
    carrier_service: session.carrier_service || null,
    carrier_schema: session.carrier_schema || null,
    carrier_peer_did: session.carrier_peer_did || null,
    carrier_connect_ticket_exposed: session.carrier_connect_ticket_exposed === true,
    adapter_ipc_exposed: session.adapter_ipc_exposed === true,
    relay_ipc_exposed: session.relay_ipc_exposed === true,
    accounting: session.accounting || null,
  };
}

const thresholds = {
  max_remote_video_ready_ms: boundedEnv("ELASTOS_BROWSER_MAC_VM_MAX_VIDEO_READY_MS", 12000),
  max_navigation_ms: boundedEnv("ELASTOS_BROWSER_MAC_VM_MAX_NAV_MS", 15000),
  max_control_uptime_ms: boundedEnv("ELASTOS_BROWSER_MAC_VM_MAX_CONTROL_UPTIME_MS", 300000),
  min_decoded_frame_delta: boundedEnv("ELASTOS_BROWSER_MAC_VM_MIN_DECODED_DELTA", 5),
  max_dropped_frames: number(process.env.ELASTOS_BROWSER_MAC_VM_MAX_DROPPED_FRAMES, 1),
  min_images: number(process.env.ELASTOS_BROWSER_MAC_VM_MIN_IMAGES, 1),
  min_clickables: number(process.env.ELASTOS_BROWSER_MAC_VM_MIN_CLICKABLES, 1),
  expected_device_pixel_ratio: number(process.env.ELASTOS_BROWSER_MAC_VM_EXPECT_DPR, 1),
  expected_viewport_width: boundedEnv("ELASTOS_BROWSER_MAC_VM_EXPECT_VIEWPORT_WIDTH", 1280),
  expected_viewport_height: boundedEnv("ELASTOS_BROWSER_MAC_VM_EXPECT_VIEWPORT_HEIGHT", 720),
};
const beforeClickFrames = number((embeddedUi.video?.before_click || {}).decoded_frames, 0);
const afterClickFrames = number(afterClick.decoded_frames, 0);
const decodedDelta = afterClickFrames - beforeClickFrames;
const beforeClickDropped = number((embeddedUi.video?.before_click || {}).dropped_frames, 0);
const droppedAfterClick = number(afterClick.dropped_frames, 0);
const droppedDelta = Math.max(0, droppedAfterClick - beforeClickDropped);
const remoteVideoReadyMs = number(ready.ready_duration_ms, Infinity);
const embeddedNavigationMs = number(navigation.duration_ms, Infinity);
const diagnosticsNavigationMs = number(diagnostics.navigation?.duration_ms, Infinity);
const initialPanel = embeddedUi.geometry?.initial?.panel || {};
const initialVideo = embeddedUi.geometry?.initial?.video || {};
const afterPanel = embeddedUi.geometry?.after_navigation?.panel || {};
const afterVideo = embeddedUi.geometry?.after_navigation?.video || {};
const sourceVideoWidth = number(ready.video_width, 0);
const sourceVideoHeight = number(ready.video_height, 0);
const displaySession = embeddedUi.display_session || null;
const displaySessionProof = displaySession ? {
  schema: displaySession.schema || null,
  mode: displaySession.mode || null,
  media_transport: displaySession.media_transport || null,
  display_backend: displaySession.display_backend || null,
  backend_class: displaySession.backend_class || null,
  offerer: displaySession.offerer || null,
  turn_ice_server_count: turnIceServerCount(displaySession),
  credentialed_turn_ice_server_count: turnIceServerCount(displaySession, true),
} : null;
const runtimeMediaRelayOk =
  embeddedUi.display_mode === "webrtc_remote_display"
  && displaySessionProof
  && displaySessionProof.mode === "webrtc_remote_display"
  && displaySessionProof.media_transport === "runtime_relay"
  && displaySessionProof.turn_ice_server_count > 0
  && displaySessionProof.credentialed_turn_ice_server_count > 0;
const diagnosticImages = Array.isArray(diagnostics.images) ? diagnostics.images : [];
const pendingImageSamples = diagnosticImages
  .filter((image) => image && image.complete === false)
  .slice(0, 12);
const visiblePendingImageSamples = diagnosticImages
  .filter((image) => image && image.visible === true && image.complete === false)
  .slice(0, 12);
const brokenImageSamples = diagnosticImages
  .filter((image) => image && image.complete === true && (number(image.natural_width) <= 0 || number(image.natural_height) <= 0))
  .slice(0, 12);
const visibleBrokenImageSamples = diagnosticImages
  .filter((image) => image && image.visible === true && image.complete === true && (number(image.natural_width) <= 0 || number(image.natural_height) <= 0))
  .slice(0, 12);
const visiblePendingImageCount = number(diagnostics.visible_pending_image_count, visiblePendingImageSamples.length);
const visibleBrokenImageCount = number(diagnostics.visible_broken_image_count, visibleBrokenImageSamples.length);
const clickExpectPattern = process.env.ELASTOS_BROWSER_MAC_VM_CLICK_EXPECT_URL_RE || process.env.HOME_VIRTUAL_AUTH_BROWSER_UI_CLICK_EXPECT_URL_RE || "";
const clickExpected = Boolean(clickExpectPattern);
const clickAddressValue = clickNavigation?.address_value || "";
const clickActualUrl = clickNavigation?.status?.actual_url || "";
const clickRegex = clickExpected ? new RegExp(clickExpectPattern) : null;
const performanceChecks = {
  remote_video_ready_ms_within_threshold: remoteVideoReadyMs <= thresholds.max_remote_video_ready_ms,
  embedded_navigation_ms_within_threshold: embeddedNavigationMs <= thresholds.max_navigation_ms,
  diagnostics_navigation_ms_within_threshold: diagnosticsNavigationMs <= thresholds.max_navigation_ms,
  decoded_frame_delta_ok: decodedDelta >= thresholds.min_decoded_frame_delta,
  dropped_frame_delta_ok: droppedDelta <= thresholds.max_dropped_frames,
};
const zoomChecks = {
  device_pixel_ratio_ok: number(diagnostics.device_pixel_ratio, 0) === thresholds.expected_device_pixel_ratio,
  viewport_width_ok: number(diagnostics.viewport_width, 0) === thresholds.expected_viewport_width,
  viewport_height_ok: number(diagnostics.viewport_height, 0) === thresholds.expected_viewport_height,
  panel_aspect_matches_viewport: aspectCloseEnough(
    number(initialPanel.width),
    number(initialPanel.height),
    thresholds.expected_viewport_width / thresholds.expected_viewport_height,
  ) && aspectCloseEnough(
    number(afterPanel.width),
    number(afterPanel.height),
    thresholds.expected_viewport_width / thresholds.expected_viewport_height,
  ),
  initial_video_matches_panel: closeEnough(number(initialPanel.width), number(initialVideo.width))
    && closeEnough(number(initialPanel.height), number(initialVideo.height)),
  after_navigation_video_matches_panel: closeEnough(number(afterPanel.width), number(afterVideo.width))
    && closeEnough(number(afterPanel.height), number(afterVideo.height)),
  source_video_matches_panel: closeEnough(sourceVideoWidth, number(initialPanel.width))
    && closeEnough(sourceVideoHeight, number(initialPanel.height)),
};
const performanceOk = Object.values(performanceChecks).every(Boolean);
const zoomOk = Object.values(zoomChecks).every(Boolean);
const homeHttpOk = process.env.HOME_HTTP_CODE === "200";
const hashParity = Boolean(process.env.INSTALLED_HOME_SHA)
  && process.env.INSTALLED_HOME_SHA === process.env.SOURCE_HOME_SHA;
function supervisorStatusOk(status) {
  return status.ok === true
    && status.network_mode === "runtime_net_only"
    && status.direct_network === false
    && Number(status.active_pages || 0) === 0
    && Number(status.pending_launches || 0) === 0;
}

const controlBeforeCold = controlBefore.available === false;
const controlBeforeOk = controlBeforeCold || supervisorStatusOk(controlBefore);
const controlAfterOk = controlAfter.available !== false && supervisorStatusOk(controlAfter);
const controlAfterUptimeMs = number(controlAfter.uptime_ms, Infinity);
const controlFreshAfterRestartOk =
  controlAfterOk
  && controlAfterUptimeMs > 0
  && controlAfterUptimeMs <= thresholds.max_control_uptime_ms;
const controlOk = controlBeforeOk && controlAfterOk;
const embeddedOk = embedded.ok === true
  && embeddedUi.display_mode === "webrtc_remote_display"
  && runtimeMediaRelayOk
  && ready.present === true
  && ready.ready_state >= 2
  && decodedDelta > 0
  && navigation.input?.accepted === true
  && navigation.status?.actual_url === process.env.PROOF_URL
  && navigation.address_value === process.env.PROOF_URL;
const clickNavigationOk = !clickExpected || (
  clickNavigation
    && clickRegex.test(String(clickAddressValue))
    && clickRegex.test(String(clickActualUrl))
);
const diagnosticsOk = opened.ok === true
  && openPage.actual_url === process.env.PROOF_URL
  && openPage.direct_network === false
  && diagnostics.url === process.env.PROOF_URL
  && diagnostics.ready_state === "complete"
  && Number(diagnostics.image_count || 0) >= thresholds.min_images
  && Number(diagnostics.clickable_count || 0) >= thresholds.min_clickables
  && Number(diagnostics.broken_image_count || 0) === 0
  && visibleBrokenImageCount === 0
  && visiblePendingImageCount === 0
  && closeResults.every((entry) => entry.closed === true);
const profileResetRequested = process.env.ELASTOS_BROWSER_MAC_VM_PROFILE_RESET_PROOF === "1";
const profileResetOk = !profileResetRequested || (
  profileReset
    && profileReset.schema === "elastos.browser.profile-reset/v1"
    && profileReset.status === "ok"
    && profileReset.profile?.scope === "active_principal"
    && profileReset.profile?.storage === "principal_owned_profile_disk"
    && profileReset.profile?.storage_posture === "principal_owned_reset_scoped_unprotected"
    && profileReset.profile?.protected_storage === false
    && profileReset.profile?.encrypted === false
    && profileReset.profile?.recoverable === false
    && profileReset.profile?.recovery === "not_recovery_kit_packaged"
    && profileReset.profile?.reset === "whole_profile"
    && profileReset.profile?.profile_key == null
    && profileReset.profile?.principal_id == null
    && profileReset.removed_profile_disk === true
);
const engineIdentity =
  clickNavigation?.status?.engine_identity ||
  navigation.status?.engine_identity ||
  {};
const vmIsolation = {
  schema: engineIdentity.schema || null,
  adapter: engineIdentity.adapter || null,
  engine: engineIdentity.engine || null,
  display_mode: engineIdentity.display_mode || null,
  guarantee_level: engineIdentity.guarantee_level || null,
  engine_control: engineIdentity.engine_control || null,
  isolated_engine_session: engineIdentity.isolated_engine_session === true,
  isolation_kind: engineIdentity.isolation_kind || null,
};
const vmIsolationOk =
  vmIsolation.schema === "elastos.browser.engine.identity/v1"
  && vmIsolation.adapter === "browser-vm-product"
  && vmIsolation.engine === "chromium_microvm"
  && vmIsolation.display_mode === "webrtc_remote_display"
  && vmIsolation.guarantee_level === "mechanism_microvm"
  && vmIsolation.engine_control === "page_scoped"
  && vmIsolation.isolated_engine_session === true
  && vmIsolation.isolation_kind === "per_launch_vm_target";

const proof = {
  schema: "elastos.browser.mac-vm-proof/v1",
  ok: homeHttpOk && hashParity && controlOk && controlFreshAfterRestartOk && embeddedOk && clickNavigationOk && diagnosticsOk && performanceOk && zoomOk && profileResetOk && vmIsolationOk,
  target: "mac-source-home",
  generated_at: new Date().toISOString(),
  home: {
    url: process.env.HOME_URL,
    http_code: Number(process.env.HOME_HTTP_CODE),
    installed_index_sha256: process.env.INSTALLED_HOME_SHA || null,
    source_index_sha256: process.env.SOURCE_HOME_SHA || null,
    hash_parity: hashParity,
  },
  virtual_auth: {
    persistent_profile: process.env.VIRTUAL_AUTH_PERSISTENT === "1",
    cleanup_passkey: process.env.VIRTUAL_AUTH_CLEANUP !== "0",
  },
  browser_route: publicBrowserRoute(openStreamSession),
  vm_control: {
    socket: process.env.CONTROL_SOCKET,
    restart: {
      schema: "elastos.browser.mac-vm-control-restart/v1",
      fresh_after_restart: controlFreshAfterRestartOk,
      max_uptime_ms: thresholds.max_control_uptime_ms,
      actual_uptime_ms: Number.isFinite(controlAfterUptimeMs) ? controlAfterUptimeMs : null,
    },
    before: {
      ok: controlBeforeOk,
      available: controlBefore.available !== false,
      cold_start: controlBeforeCold,
      error: controlBefore.error || null,
      pid: controlBefore.pid || null,
      started_at: controlBefore.started_at || null,
      uptime_ms: Number.isFinite(number(controlBefore.uptime_ms, NaN)) ? number(controlBefore.uptime_ms) : null,
      active_pages: Number(controlBefore.active_pages || 0),
      pending_launches: Number(controlBefore.pending_launches || 0),
      network_mode: controlBefore.network_mode || null,
      direct_network: controlBefore.direct_network,
    },
    after: {
      ok: controlAfterOk,
      available: controlAfter.available !== false,
      error: controlAfter.error || null,
      pid: controlAfter.pid || null,
      started_at: controlAfter.started_at || null,
      uptime_ms: Number.isFinite(number(controlAfter.uptime_ms, NaN)) ? number(controlAfter.uptime_ms) : null,
      active_pages: Number(controlAfter.active_pages || 0),
      pending_launches: Number(controlAfter.pending_launches || 0),
      network_mode: controlAfter.network_mode || null,
      direct_network: controlAfter.direct_network,
    },
  },
  embedded_video_input: {
    ok: embeddedOk,
    page_id: embeddedUi.page_id || null,
    display_mode: embeddedUi.display_mode || null,
    display_session: displaySessionProof,
    vm_isolation: vmIsolation,
    remote_video_ready_ms: Number.isFinite(remoteVideoReadyMs) ? remoteVideoReadyMs : null,
    video_width: ready.video_width || null,
    video_height: ready.video_height || null,
    decoded_frames_before_click: beforeClickFrames,
    decoded_frames_after_click: afterClickFrames,
    decoded_frame_delta: decodedDelta,
    dropped_frames_before_click: beforeClickDropped,
    dropped_frames_after_click: droppedAfterClick,
    dropped_frame_delta: droppedDelta,
    click: embeddedUi.click || null,
    click_navigation: clickNavigation ? {
      ok: clickNavigationOk,
      expected_url_re: clickNavigation.expected_url_re || clickExpectPattern || null,
      address_value: clickAddressValue || null,
      duration_ms: clickNavigation.duration_ms || null,
      status: clickNavigation.status || null,
      input: clickNavigation.input || null,
    } : {
      ok: clickNavigationOk,
      expected_url_re: clickExpectPattern || null,
      skipped: !clickExpected,
    },
    dom_clicks: embeddedUi.dom_clicks || [],
    navigation: {
      requested_url: navigation.requested_url || null,
      actual_url: navigation.status?.actual_url || null,
      title: navigation.status?.title || null,
      address_value: navigation.address_value || null,
      duration_ms: navigation.duration_ms || null,
      input_response_ms: navigation.input_response_ms || null,
      status_match_ms: navigation.status_match_ms || null,
      address_match_ms: navigation.address_match_ms || null,
    },
    geometry: embeddedUi.geometry || null,
  },
  page_diagnostics: {
    ...publicPageDiagnostics(diagnostics),
    ok: diagnosticsOk,
    page_id: openPage.page_id || null,
    url: diagnostics.url || openPage.actual_url || null,
    title: diagnostics.title || openPage.status?.title || null,
    visible_broken_image_count: visibleBrokenImageCount,
    visible_pending_image_count: visiblePendingImageCount,
    pending_image_samples: pendingImageSamples,
    broken_image_samples: brokenImageSamples,
    diagnostic_click_actions: Array.isArray(openPage.diagnostic_click_actions)
      ? openPage.diagnostic_click_actions.map((action) => ({
          ok: action.ok === true,
          error: action.error || null,
          expected_text_re: action.expected_text_re || null,
          click: action.click || null,
          target: action.target || null,
          input: action.input || null,
          diagnostics: action.diagnostics ? publicPageDiagnostics(action.diagnostics) : null,
        }))
      : [],
    close_results: closeResults.map((entry) => ({
      page_id: entry.page_id || null,
      closed: entry.closed === true,
      isolated_session: entry.isolated_session === true,
      shutdown_ok: entry.shutdown?.ok === true,
    })),
  },
  profile_reset: {
    requested: profileResetRequested,
    ok: profileResetOk,
    receipt: profileReset ? {
      schema: profileReset.schema || null,
      status: profileReset.status || null,
      profile: profileReset.profile || null,
      removed_profile_disk: profileReset.removed_profile_disk === true,
    } : null,
  },
  quality_gates: {
    ok: performanceOk && zoomOk,
    thresholds,
    performance: {
      ok: performanceOk,
      checks: performanceChecks,
      remote_video_ready_ms: Number.isFinite(remoteVideoReadyMs) ? remoteVideoReadyMs : null,
      embedded_navigation_ms: Number.isFinite(embeddedNavigationMs) ? embeddedNavigationMs : null,
      diagnostics_navigation_ms: Number.isFinite(diagnosticsNavigationMs) ? diagnosticsNavigationMs : null,
      decoded_frame_delta: decodedDelta,
      dropped_frames_before_click: beforeClickDropped,
      dropped_frames_after_click: droppedAfterClick,
      dropped_frame_delta: droppedDelta,
    },
    zoom: {
      ok: zoomOk,
      checks: zoomChecks,
      viewport_width: diagnostics.viewport_width || null,
      viewport_height: diagnostics.viewport_height || null,
      device_pixel_ratio: diagnostics.device_pixel_ratio || null,
      source_video_width: sourceVideoWidth || null,
      source_video_height: sourceVideoHeight || null,
      initial_panel: initialPanel,
      initial_video: initialVideo,
      after_navigation_panel: afterPanel,
      after_navigation_video: afterVideo,
    },
  },
  manual_acceptance: {
    status: "not_recorded",
    reason: "Machine proof does not replace hash-bound manual UX, product audio, or authenticated ela.city edit-profile acceptance.",
  },
};

fs.writeFileSync(outPath, `${JSON.stringify(proof, null, 2)}\n`);
NODE

if [[ -n "$artifact_out" ]]; then
  cp "$proof_json" "$artifact_out"
fi
cat "$proof_json"
"$node_bin" -e 'const fs = require("node:fs"); if (JSON.parse(fs.readFileSync(process.argv[1], "utf8")).ok !== true) process.exit(1);' "$proof_json"
