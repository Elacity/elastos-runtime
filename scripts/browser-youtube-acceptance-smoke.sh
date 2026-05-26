#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

youtube_url="${BROWSER_YOUTUBE_URL:-https://www.youtube.com/embed/dQw4w9WgXcQ?autoplay=1&mute=0&controls=1&origin=https%3A%2F%2Felastos.elacitylabs.com}"
referer="${BROWSER_YOUTUBE_REFERER:-https://elastos.elacitylabs.com/apps/home/}"

if [[ -z "${DISPLAY:-}" && -x "$(command -v xvfb-run || true)" ]]; then
  xvfb-run -a env \
    BROWSER_SMOKE_HEADLESS=0 \
    BROWSER_SMOKE_URL="$youtube_url" \
    BROWSER_SMOKE_REFERER="$referer" \
    BROWSER_SMOKE_ALLOWED_HOSTS="${BROWSER_SMOKE_ALLOWED_HOSTS:-*}" \
    BROWSER_SMOKE_UPSTREAM_HTTP_PROXY="${BROWSER_SMOKE_UPSTREAM_HTTP_PROXY:-}" \
    BROWSER_SMOKE_UPSTREAM_PROXY_AUTHORIZATION="${BROWSER_SMOKE_UPSTREAM_PROXY_AUTHORIZATION:-}" \
    BROWSER_SMOKE_ADDRESS_FAMILY="${BROWSER_SMOKE_ADDRESS_FAMILY:-prefer_ipv4}" \
    BROWSER_SMOKE_REQUIRE_MEDIA=1 \
    scripts/browser-runtime-proxy-smoke.sh
else
  BROWSER_SMOKE_HEADLESS=0 \
  BROWSER_SMOKE_URL="$youtube_url" \
  BROWSER_SMOKE_REFERER="$referer" \
  BROWSER_SMOKE_ALLOWED_HOSTS="${BROWSER_SMOKE_ALLOWED_HOSTS:-*}" \
  BROWSER_SMOKE_UPSTREAM_HTTP_PROXY="${BROWSER_SMOKE_UPSTREAM_HTTP_PROXY:-}" \
  BROWSER_SMOKE_UPSTREAM_PROXY_AUTHORIZATION="${BROWSER_SMOKE_UPSTREAM_PROXY_AUTHORIZATION:-}" \
  BROWSER_SMOKE_ADDRESS_FAMILY="${BROWSER_SMOKE_ADDRESS_FAMILY:-prefer_ipv4}" \
  BROWSER_SMOKE_REQUIRE_MEDIA=1 \
  scripts/browser-runtime-proxy-smoke.sh
fi
