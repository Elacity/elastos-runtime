#!/usr/bin/env bash
set -euo pipefail

BASE_URL="${ELASTOS_BASE_URL:-http://127.0.0.1:8090}"
ROOT_URL="${ELASTOS_PUBLIC_ROOT_URL:-${BASE_URL%/}/}"
HEADERS_FILE=$(mktemp)
BODY_FILE=$(mktemp)
trap 'rm -f "$HEADERS_FILE" "$BODY_FILE"' EXIT

curl -fsSL -D "$HEADERS_FILE" "$ROOT_URL" -o "$BODY_FILE"

grep -qi '^x-elastos-site-origin: localhost://MyWebSite' "$HEADERS_FILE" \
    || { echo "[public-root-site-smoke] missing X-Elastos-Site-Origin header" >&2; sed -n '1,40p' "$HEADERS_FILE" >&2; exit 1; }

grep -q '<title>ElastOS Runtime</title>' "$BODY_FILE" \
    || { echo "[public-root-site-smoke] root page did not render the expected site title" >&2; sed -n '1,80p' "$BODY_FILE" >&2; exit 1; }

if grep -q '<title>ElastOS Gateway</title>' "$BODY_FILE"; then
    echo "[public-root-site-smoke] root page fell back to the generic gateway landing page" >&2
    sed -n '1,80p' "$BODY_FILE" >&2
    exit 1
fi

echo "[public-root-site-smoke] pass url=${ROOT_URL}"
