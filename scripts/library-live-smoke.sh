#!/usr/bin/env bash
set -euo pipefail

GATEWAY_URL="${ELASTOS_GATEWAY_URL:-https://elastos.elacitylabs.com}"
GATEWAY_URL="${GATEWAY_URL%/}"
HOME_TOKEN="${ELASTOS_HOME_TOKEN:-}"
HOME_COOKIE="${ELASTOS_HOME_COOKIE:-${ELASTOS_COOKIE:-}}"
HOME_COOKIE_JAR="${ELASTOS_HOME_COOKIE_JAR:-${ELASTOS_COOKIE_JAR:-}}"
CURL_HOME_AUTH_ARGS=()

usage() {
    cat <<EOF
Usage: ELASTOS_HOME_TOKEN=<signed-home-token> $(basename "$0")

Live Library proof for a signed Home session. The script asks Home to launch
Library, extracts the Library-scoped launch token, then verifies the app-facing
provider path: roots, write, publish, status, share, trash, and cleanup.
Without a signed session it still verifies the public Library shell/modules so
stale static deployments are caught.

Environment:
  ELASTOS_GATEWAY_URL       Default: https://elastos.elacitylabs.com
  ELASTOS_HOME_TOKEN        Signed Home token for the active user
  ELASTOS_HOME_COOKIE       Cookie header containing home-session=<token>
  ELASTOS_HOME_COOKIE_JAR   curl cookie jar containing home-session
EOF
}

need_cmd() {
    command -v "$1" >/dev/null 2>&1 || {
        echo "[library-live-smoke] missing required command: $1" >&2
        exit 2
    }
}

configure_home_auth() {
    if [[ -n "$HOME_TOKEN" ]]; then
        CURL_HOME_AUTH_ARGS=(-H "x-elastos-home-token: ${HOME_TOKEN}")
        return 0
    fi
    if [[ -n "$HOME_COOKIE" ]]; then
        CURL_HOME_AUTH_ARGS=(-H "Cookie: ${HOME_COOKIE}")
        return 0
    fi
    if [[ -n "$HOME_COOKIE_JAR" ]]; then
        if [[ ! -f "$HOME_COOKIE_JAR" ]]; then
            echo "[library-live-smoke] cookie jar does not exist: $HOME_COOKIE_JAR" >&2
            exit 2
        fi
        CURL_HOME_AUTH_ARGS=(-b "$HOME_COOKIE_JAR")
        return 0
    fi
    return 1
}

post_home_json() {
    local path="$1"
    local body="$2"
    curl -fsS \
        "${CURL_HOME_AUTH_ARGS[@]}" \
        -H "content-type: application/json" \
        -d "$body" \
        "${GATEWAY_URL}${path}"
}

post_library_json() {
    local op="$1"
    local body="$2"
    curl -fsS \
        -H "x-elastos-home-token: ${LIBRARY_TOKEN}" \
        -H "content-type: application/json" \
        -d "$body" \
        "${GATEWAY_URL}/api/provider/object/${op}"
}

put_library_upload() {
    local uri="$1"
    local file="$2"
    curl -fsS \
        -X PUT \
        -H "x-elastos-home-token: ${LIBRARY_TOKEN}" \
        -H "content-type: text/plain" \
        --data-binary "@${file}" \
        "${GATEWAY_URL}/api/provider/object/upload?uri=$(node -e 'process.stdout.write(encodeURIComponent(process.argv[1]))' "$uri")"
}

get_library_download() {
    local uri="$1"
    curl -fsS \
        -H "x-elastos-home-token: ${LIBRARY_TOKEN}" \
        "${GATEWAY_URL}/api/provider/object/download/raw?uri=$(node -e 'process.stdout.write(encodeURIComponent(process.argv[1]))' "$uri")"
}

get_library_download_with_headers() {
    local uri="$1"
    local headers_file="$2"
    curl -fsS \
        -D "$headers_file" \
        -H "x-elastos-home-token: ${LIBRARY_TOKEN}" \
        "${GATEWAY_URL}/api/provider/object/download/raw?uri=$(node -e 'process.stdout.write(encodeURIComponent(process.argv[1]))' "$uri")"
}

get_library_download_range() {
    local uri="$1"
    local range="$2"
    curl -fsS \
        -H "x-elastos-home-token: ${LIBRARY_TOKEN}" \
        -H "Range: ${range}" \
        "${GATEWAY_URL}/api/provider/object/download/raw?uri=$(node -e 'process.stdout.write(encodeURIComponent(process.argv[1]))' "$uri")"
}

get() {
    curl -fsS "${GATEWAY_URL}${1}"
}

assert_contains() {
    local label="$1"
    local text="$2"
    local needle="$3"
    if ! grep -Fq "$needle" <<<"$text"; then
        echo "[library-live-smoke] ${label} missing expected marker: ${needle}" >&2
        exit 1
    fi
}

assert_not_contains() {
    local label="$1"
    local text="$2"
    local needle="$3"
    if grep -Fq "$needle" <<<"$text"; then
        echo "[library-live-smoke] ${label} still contains stale marker: ${needle}" >&2
        exit 1
    fi
}

verify_public_library_assets() {
    echo "[library-live-smoke] verify public Library shell/modules"
    local shell
    local library_css
    local app_js
    local api_js
    local actions_js
    local events_js
    local render_js
    local uploads_js
    local state_js
    shell="$(get "/apps/library/")"
    library_css="$(get "/apps/library/library.css")"
    app_js="$(get "/apps/library/src/app.js")"
    api_js="$(get "/apps/library/src/api.js")"
    actions_js="$(get "/apps/library/src/actions.js")"
    events_js="$(get "/apps/library/src/events.js")"
    render_js="$(get "/apps/library/src/render.js")"
    uploads_js="$(get "/apps/library/src/uploads.js")"
    state_js="$(get "/apps/library/src/state.js")"
    assert_contains "Library shell" "$shell" 'rel="stylesheet" href="library.css"'
    assert_contains "Library shell" "$shell" 'type="module" src="src/app.js?v=library-20260711b"'
    assert_contains "Library CSS" "$library_css" "grid-template-rows: 45px auto 18px;"
    assert_contains "Library CSS" "$library_css" '.content[data-view="grid"] .badges'
    assert_contains "Library CSS" "$library_css" '.content[data-view="list"] .badges'
    assert_contains "Library app.js" "$app_js" 'from "./uploads.js"'
    assert_contains "Library app.js" "$app_js" "createLibraryUploads({"
    assert_contains "Library app.js" "$app_js" "function activeRootForUri(uri)"
    assert_contains "Library app.js" "$app_js" ".sort((left, right) => right.uri.length - left.uri.length)"
    assert_contains "Library app.js" "$app_js" 'sidebar: document.querySelector(".sidebar")'
    assert_contains "Library app.js" "$app_js" "function showPlaceMenu(uri, x, y)"
    assert_contains "Library app.js" "$app_js" "Open in New Window"
    assert_contains "Library app.js" "$app_js" "Download Selected"
    assert_contains "Library app.js" "$app_js" "Extract Here"
    assert_contains "Library api.js" "$api_js" "/api/provider/object/upload"
    assert_contains "Library api.js" "$api_js" "/api/provider/object/upload/start"
    assert_contains "Library api.js" "$api_js" "CHUNKED_UPLOAD_THRESHOLD_BYTES"
    assert_contains "Library api.js" "$api_js" "http-chunk-session"
    assert_contains "Library api.js" "$api_js" "/api/provider/object/download/raw"
    assert_contains "Library api.js" "$api_js" "x-elastos-transfer-receipt"
    assert_contains "Library api.js" "$api_js" "XMLHttpRequest"
    assert_contains "Library api.js" "$api_js" "too large for the current upload service"
    assert_not_contains "Library api.js" "$api_js" "/api/provider/library/upload"
    assert_not_contains "Library api.js" "$api_js" "/api/provider/library/download/raw"
    assert_contains "Library actions.js" "$actions_js" "uploadObject({"
    assert_contains "Library actions.js" "$actions_js" "downloadObjectRaw({"
    assert_contains "Library actions.js" "$actions_js" "async function downloadSelectedObjects()"
    assert_contains "Library actions.js" "$actions_js" "async function extractArchiveObject(object)"
    if grep -Fq "fileToBase64" <<<"$actions_js"; then
        echo "[library-live-smoke] Library actions.js still contains fileToBase64 upload path" >&2
        exit 1
    fi
    assert_contains "Library events.js" "$events_js" 'elements.places.addEventListener("contextmenu"'
    assert_contains "Library events.js" "$events_js" 'elements.sidebar?.addEventListener("contextmenu"'
    assert_contains "Library events.js" "$events_js" "showPlaceMenu(button.dataset.uri, event.clientX, event.clientY)"
    assert_contains "Library events.js" "$events_js" "event.stopPropagation();"
    assert_contains "Library events.js" "$events_js" "function isNameEditorTarget(target)"
    assert_contains "Library events.js" "$events_js" "isNameEditorTarget(event.target)"
    assert_contains "Library events.js" "$events_js" "selectRangeTo(item.dataset.uri"
    assert_contains "Library events.js" "$events_js" 'event.key === "Enter"'
    assert_contains "Library events.js" "$events_js" "openSelectedObjects(objects, openObject, showError)"
    assert_contains "Library events.js" "$events_js" 'event.key === "ContextMenu"'
    assert_contains "Library events.js" "$events_js" 'event.shiftKey && event.key === "F10"'
    assert_not_contains "Library events.js" "$events_js" "NAME_CLICK_RENAME_DELAY_MS"
    assert_not_contains "Library events.js" "$events_js" "clickedName"
    assert_not_contains "Library events.js" "$events_js" "cancelPendingNameClickRename"
    assert_contains "Library render.js" "$render_js" "emptyStateCopy(state)"
    assert_contains "Library render.js" "$render_js" "No connected spaces"
    assert_contains "Library render.js" "$render_js" "Provider-backed spaces"
    assert_contains "Library render.js" "$render_js" "writable spaces use provider-owned storage"
    assert_contains "Library render.js" "$render_js" "const badgesMarkup = badges ?"
    assert_contains "Library uploads.js" "$uploads_js" "function scheduleUploadRender("
    assert_contains "Library uploads.js" "$uploads_js" "window.requestAnimationFrame("
    assert_contains "Library uploads.js" "$uploads_js" "perf.uploadRenderCount"
    assert_contains "Library uploads.js" "$uploads_js" "perf.uploadRenderScheduledCount"
    assert_contains "Library state.js" "$state_js" "uploadRenderCount"
    assert_contains "Library state.js" "$state_js" "uploadRenderScheduledCount"
    assert_contains "Library state.js" "$state_js" "extract_archive"
}

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
    usage
    exit 0
fi

need_cmd curl
need_cmd grep

verify_public_library_assets

if ! configure_home_auth; then
    echo "[library-live-smoke] signed provider path skipped: set ELASTOS_HOME_TOKEN, ELASTOS_HOME_COOKIE, or ELASTOS_HOME_COOKIE_JAR from a signed Home session"
    exit 0
fi

need_cmd base64
need_cmd jq
need_cmd node
need_cmd python3

echo "[library-live-smoke] launch Library from signed Home session"
launch="$(post_home_json "/api/apps/home/launch" '{"target":"library","query":{}}')"
route="$(jq -r '.route // empty' <<<"$launch")"
LIBRARY_TOKEN="$(
    node -e 'const route = process.argv[1] || ""; const url = new URL(route, "https://example.invalid"); process.stdout.write(url.searchParams.get("home_token") || "");' "$route"
)"
if [[ -z "$LIBRARY_TOKEN" ]]; then
    echo "[library-live-smoke] Home launch did not return a Library token" >&2
    exit 1
fi

echo "[library-live-smoke] resolve Library roots"
roots="$(post_library_json "roots" '{}')"
public_uri="$(jq -r '.data.roots[] | select(.id == "public") | .uri' <<<"$roots")"
if [[ -z "$public_uri" || "$public_uri" == "null" ]]; then
    echo "[library-live-smoke] Library roots did not expose Public" >&2
    exit 1
fi

stamp="$(date -u +%Y%m%dT%H%M%SZ)-$$"
file_uri="${public_uri}/library-live-smoke-${stamp}.txt"
archive_uri="${public_uri}/library-live-archive-${stamp}.tar"
body_text="Library live smoke ${stamp}"
body_b64="$(printf '%s' "$body_text" | base64 -w0)"

cleanup() {
    rm -f "${upload_file:-}" "${download_headers_file:-}" 2>/dev/null || true
    if [[ -n "${extracted_trash_uri:-}" ]]; then
        post_library_json "delete_permanently" "$(jq -nc --arg uri "$extracted_trash_uri" '{uri: $uri}')" >/dev/null 2>&1 || true
    elif [[ -n "${extracted_uri:-}" ]]; then
        local extracted_trash_response
        extracted_trash_response="$(post_library_json "trash" "$(jq -nc --arg uri "$extracted_uri" '{uri: $uri}')" 2>/dev/null || true)"
        local cleanup_extracted_trash_uri
        cleanup_extracted_trash_uri="$(jq -r '.data.object.uri // empty' <<<"${extracted_trash_response:-}" 2>/dev/null || true)"
        if [[ -n "$cleanup_extracted_trash_uri" ]]; then
            post_library_json "delete_permanently" "$(jq -nc --arg uri "$cleanup_extracted_trash_uri" '{uri: $uri}')" >/dev/null 2>&1 || true
        fi
    fi
    if [[ -n "${archive_trash_uri:-}" ]]; then
        post_library_json "delete_permanently" "$(jq -nc --arg uri "$archive_trash_uri" '{uri: $uri}')" >/dev/null 2>&1 || true
    elif [[ -n "${archive_uri:-}" ]]; then
        local archive_trash_response
        archive_trash_response="$(post_library_json "trash" "$(jq -nc --arg uri "$archive_uri" '{uri: $uri}')" 2>/dev/null || true)"
        local cleanup_archive_trash_uri
        cleanup_archive_trash_uri="$(jq -r '.data.object.uri // empty' <<<"${archive_trash_response:-}" 2>/dev/null || true)"
        if [[ -n "$cleanup_archive_trash_uri" ]]; then
            post_library_json "delete_permanently" "$(jq -nc --arg uri "$cleanup_archive_trash_uri" '{uri: $uri}')" >/dev/null 2>&1 || true
        fi
    fi
    if [[ -n "${upload_trash_uri:-}" ]]; then
        post_library_json "delete_permanently" "$(jq -nc --arg uri "$upload_trash_uri" '{uri: $uri}')" >/dev/null 2>&1 || true
    elif [[ -n "${upload_uri:-}" ]]; then
        local upload_trash_response
        upload_trash_response="$(post_library_json "trash" "$(jq -nc --arg uri "$upload_uri" '{uri: $uri}')" 2>/dev/null || true)"
        local cleanup_upload_trash_uri
        cleanup_upload_trash_uri="$(jq -r '.data.object.uri // empty' <<<"${upload_trash_response:-}" 2>/dev/null || true)"
        if [[ -n "$cleanup_upload_trash_uri" ]]; then
            post_library_json "delete_permanently" "$(jq -nc --arg uri "$cleanup_upload_trash_uri" '{uri: $uri}')" >/dev/null 2>&1 || true
        fi
    fi
    if [[ -n "${trash_uri:-}" ]]; then
        post_library_json "delete_permanently" "$(jq -nc --arg uri "$trash_uri" '{uri: $uri}')" >/dev/null 2>&1 || true
        return
    fi
    if [[ -n "${file_uri:-}" ]]; then
        local trash_response
        trash_response="$(post_library_json "trash" "$(jq -nc --arg uri "$file_uri" '{uri: $uri}')" 2>/dev/null || true)"
        local cleanup_trash_uri
        cleanup_trash_uri="$(jq -r '.data.object.uri // empty' <<<"${trash_response:-}" 2>/dev/null || true)"
        if [[ -n "$cleanup_trash_uri" ]]; then
            post_library_json "delete_permanently" "$(jq -nc --arg uri "$cleanup_trash_uri" '{uri: $uri}')" >/dev/null 2>&1 || true
        fi
    fi
}
trap cleanup EXIT

echo "[library-live-smoke] write Public smoke object"
write="$(
    post_library_json "write" "$(
        jq -nc \
            --arg uri "$file_uri" \
            --arg data "$body_b64" \
            '{uri: $uri, mime: "text/plain", data: $data}'
    )"
)"
revision="$(jq -r '.data.object.revision' <<<"$write")"
jq -e --arg uri "$file_uri" '.status == "ok" and .data.object.uri == $uri' <<<"$write" >/dev/null

upload_uri="${public_uri}/library-live-upload-${stamp}.txt"
upload_file="$(mktemp)"
upload_text="Library live raw upload ${stamp}"
printf '%s' "$upload_text" >"$upload_file"
echo "[library-live-smoke] raw upload Public smoke object"
upload="$(put_library_upload "$upload_uri" "$upload_file")"
rm -f "$upload_file"
jq -e --arg uri "$upload_uri" '.status == "ok" and .data.object.uri == $uri and .data.transport == "raw-body"' <<<"$upload" >/dev/null
jq -e '.data.receipt.schema == "elastos.object.transfer.receipt/v1" and .data.receipt.op == "upload" and .data.receipt.status == "completed" and (.data.receipt.bytes > 0)' <<<"$upload" >/dev/null
read_upload="$(post_library_json "read" "$(jq -nc --arg uri "$upload_uri" '{uri: $uri}')")"
jq -e '.status == "ok" and (.data.data | type == "string" and length > 0)' <<<"$read_upload" >/dev/null
upload_read_text="$(jq -r '.data.data' <<<"$read_upload" | base64 -d)"
if [[ "$upload_read_text" != "$upload_text" ]]; then
    echo "[library-live-smoke] raw upload readback mismatch" >&2
    exit 1
fi
download_headers_file="$(mktemp)"
download_text="$(get_library_download_with_headers "$upload_uri" "$download_headers_file")"
if [[ "$download_text" != "$upload_text" ]]; then
    echo "[library-live-smoke] raw download readback mismatch" >&2
    exit 1
fi
grep -Fiq "x-elastos-request-id:" "$download_headers_file" || {
    echo "[library-live-smoke] raw download missing request id header" >&2
    exit 1
}
grep -Fiq "x-elastos-transfer-receipt:" "$download_headers_file" || {
    echo "[library-live-smoke] raw download missing transfer receipt header" >&2
    exit 1
}
range_text="$(get_library_download_range "$upload_uri" "bytes=0-6")"
if [[ "$range_text" != "${upload_text:0:7}" ]]; then
    echo "[library-live-smoke] raw range download readback mismatch" >&2
    exit 1
fi

archive_b64="$(
    python3 - <<'PY'
import base64
import io
import tarfile

payload = b"Library live archive extraction"
buffer = io.BytesIO()
with tarfile.open(fileobj=buffer, mode="w") as archive:
    info = tarfile.TarInfo("inside.txt")
    info.size = len(payload)
    info.mode = 0o644
    archive.addfile(info, io.BytesIO(payload))
print(base64.b64encode(buffer.getvalue()).decode("ascii"))
PY
)"
echo "[library-live-smoke] extract plain tar through object-provider route"
archive_write="$(
    post_library_json "write" "$(
        jq -nc \
            --arg uri "$archive_uri" \
            --arg data "$archive_b64" \
            '{uri: $uri, mime: "application/x-tar", data: $data}'
    )"
)"
jq -e --arg uri "$archive_uri" '.status == "ok" and .data.object.uri == $uri and .data.object.mime == "application/x-tar" and (.data.object.capabilities | index("extract_archive"))' <<<"$archive_write" >/dev/null
extract="$(
    post_library_json "extract_archive" "$(
        jq -nc --arg uri "$archive_uri" '{uri: $uri}'
    )"
)"
extracted_uri="$(jq -r '.data.object.uri' <<<"$extract")"
jq -e --arg expected "${public_uri}/library-live-archive-${stamp}" '.status == "ok" and .data.object.uri == $expected and .data.object.kind == "directory"' <<<"$extract" >/dev/null
extracted_read="$(post_library_json "read" "$(jq -nc --arg uri "${extracted_uri}/inside.txt" '{uri: $uri}')")"
extracted_text="$(jq -r '.data.data' <<<"$extracted_read" | base64 -d)"
if [[ "$extracted_text" != "Library live archive extraction" ]]; then
    echo "[library-live-smoke] plain tar extracted file readback mismatch" >&2
    exit 1
fi
extracted_trash="$(post_library_json "trash" "$(jq -nc --arg uri "$extracted_uri" '{uri: $uri}')")"
extracted_trash_uri="$(jq -r '.data.object.uri' <<<"$extracted_trash")"
post_library_json "delete_permanently" "$(jq -nc --arg uri "$extracted_trash_uri" '{uri: $uri}')" >/dev/null
extracted_trash_uri=""
extracted_uri=""
archive_trash="$(post_library_json "trash" "$(jq -nc --arg uri "$archive_uri" '{uri: $uri}')")"
archive_trash_uri="$(jq -r '.data.object.uri' <<<"$archive_trash")"
post_library_json "delete_permanently" "$(jq -nc --arg uri "$archive_trash_uri" '{uri: $uri}')" >/dev/null
archive_trash_uri=""
archive_uri=""

upload_trash="$(post_library_json "trash" "$(jq -nc --arg uri "$upload_uri" '{uri: $uri}')")"
upload_trash_uri="$(jq -r '.data.object.uri' <<<"$upload_trash")"
post_library_json "delete_permanently" "$(jq -nc --arg uri "$upload_trash_uri" '{uri: $uri}')" >/dev/null
upload_trash_uri=""
upload_uri=""

echo "[library-live-smoke] publish through content-provider"
publish="$(
    post_library_json "publish" "$(
        jq -nc \
            --arg uri "$file_uri" \
            --arg revision "$revision" \
            '{uri: $uri, if_revision: $revision}'
    )"
)"
jq -e '.status == "ok" and (.data.cid | type == "string" and length > 0) and (.data.uri | startswith("elastos://"))' <<<"$publish" >/dev/null
cid="$(jq -r '.data.cid' <<<"$publish")"

echo "[library-live-smoke] status and share published object"
status="$(post_library_json "status" "$(jq -nc --arg uri "$file_uri" '{uri: $uri}')")"
jq -e --arg cid "$cid" '.status == "ok" and .data.published.cid == $cid' <<<"$status" >/dev/null
share="$(post_library_json "share" "$(jq -nc --arg uri "$file_uri" '{uri: $uri}')")"
jq -e --arg cid "$cid" '.status == "ok" and .data.cid == $cid and (.data.uri | startswith("elastos://"))' <<<"$share" >/dev/null

echo "[library-live-smoke] cleanup smoke object"
trash="$(post_library_json "trash" "$(jq -nc --arg uri "$file_uri" '{uri: $uri}')")"
trash_uri="$(jq -r '.data.object.uri' <<<"$trash")"
jq -e '.status == "ok" and (.data.object.uri | contains("/.Trash/"))' <<<"$trash" >/dev/null
delete="$(post_library_json "delete_permanently" "$(jq -nc --arg uri "$trash_uri" '{uri: $uri}')")"
jq -e --arg uri "$trash_uri" '.status == "ok" and .data.deleted_uri == $uri' <<<"$delete" >/dev/null
trash_uri=""

echo "[library-live-smoke] PASS Library live publish/share smoke cid=${cid}"
