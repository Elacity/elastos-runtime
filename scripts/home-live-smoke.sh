#!/usr/bin/env bash
set -euo pipefail

GATEWAY_URL="${ELASTOS_GATEWAY_URL:-https://elastos.elacitylabs.com}"
GATEWAY_URL="${GATEWAY_URL%/}"
HOME_COOKIE="${ELASTOS_HOME_COOKIE:-${ELASTOS_COOKIE:-}}"
HOME_COOKIE_JAR="${ELASTOS_HOME_COOKIE_JAR:-${ELASTOS_COOKIE_JAR:-}}"
HOME_VERSION="${ELASTOS_HOME_ASSET_VERSION:-home-20260603c}"
CURL_HOME_AUTH_ARGS=()

need_cmd() {
    command -v "$1" >/dev/null 2>&1 || {
        echo "[home-live-smoke] missing required command: $1" >&2
        exit 2
    }
}

configure_home_auth() {
    if [[ -n "$HOME_COOKIE" ]]; then
        CURL_HOME_AUTH_ARGS=(-H "Cookie: ${HOME_COOKIE}")
        return 0
    fi
    if [[ -n "$HOME_COOKIE_JAR" ]]; then
        if [[ ! -f "$HOME_COOKIE_JAR" ]]; then
            echo "[home-live-smoke] cookie jar does not exist: $HOME_COOKIE_JAR" >&2
            exit 2
        fi
        CURL_HOME_AUTH_ARGS=(-b "$HOME_COOKIE_JAR")
        return 0
    fi
    return 1
}

get() {
    curl -fsS "$GATEWAY_URL$1"
}

get_authed() {
    curl -fsS "${CURL_HOME_AUTH_ARGS[@]}" "$GATEWAY_URL$1"
}

assert_contains() {
    local label="$1"
    local text="$2"
    local needle="$3"
    if ! grep -Fq "$needle" <<<"$text"; then
        echo "[home-live-smoke] ${label} missing expected marker: ${needle}" >&2
        exit 1
    fi
}

assert_not_contains() {
    local label="$1"
    local text="$2"
    local needle="$3"
    if grep -Fq "$needle" <<<"$text"; then
        echo "[home-live-smoke] ${label} contains stale marker: ${needle}" >&2
        exit 1
    fi
}

need_cmd curl
need_cmd grep

echo "[home-live-smoke] fetch live Home shell"
home_html="$(get "/apps/home/")"
assert_contains "Home HTML" "$home_html" "shell.js?v=${HOME_VERSION}"
assert_contains "Home HTML" "$home_html" "style.css?v=${HOME_VERSION}"
assert_contains "Home HTML" "$home_html" "manifest.webmanifest?v=${HOME_VERSION}"
assert_not_contains "Home HTML" "$home_html" "home-20260603a"
assert_not_contains "Home HTML" "$home_html" "home-20260601a"

echo "[home-live-smoke] verify live Home module graph"
shell_js="$(get "/apps/home/shell.js?v=${HOME_VERSION}")"
assert_contains "Home shell.js" "$shell_js" "shell-core.js?v=${HOME_VERSION}"
assert_contains "Home shell.js" "$shell_js" "shell-auth.js?v=${HOME_VERSION}"
assert_contains "Home shell.js" "$shell_js" "registration is intentionally disabled for now"
assert_not_contains "Home shell.js" "$shell_js" "navigator.serviceWorker.register"
assert_not_contains "Home shell.js" "$shell_js" "home-20260603a"

echo "[home-live-smoke] verify cleanup service worker"
service_worker="$(get "/apps/home/service-worker.js")"
assert_contains "Home service worker" "$service_worker" "self.registration.unregister()"
assert_contains "Home service worker" "$service_worker" "key.startsWith(CACHE_PREFIX)"
assert_not_contains "Home service worker" "$service_worker" "caches.open("

for module in shell-core shell-auth shell-chrome shell-surface shell-windows shell-window-geometry; do
    curl -fsS -o /dev/null "$GATEWAY_URL/apps/home/${module}.js?v=${HOME_VERSION}"
done

if configure_home_auth; then
    echo "[home-live-smoke] verify signed Home summary"
    summary="$(get_authed "/api/apps/home/summary")"
    assert_contains "Home summary" "$summary" '"signed_in":true'
else
    echo "[home-live-smoke] signed summary skipped: set ELASTOS_HOME_COOKIE or ELASTOS_HOME_COOKIE_JAR to verify session state"
fi

echo "[home-live-smoke] PASS Home live shell smoke version=${HOME_VERSION}"
