#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

protected_url="${ELASTOS_PROTECTED_CONTENT_URL:-https://ela.city/cinema/view/0x366273ab04d95a98499ecf7eece8d25a7fca944e/93407450701306779960081980475876347092895931815671991187705114984854614190341}"

export HOME_URL="${HOME_URL:-https://elastos.elacitylabs.com/apps/home/}"
export HOME_VIRTUAL_AUTH_ALLOW_REMOTE="${HOME_VIRTUAL_AUTH_ALLOW_REMOTE:-1}"
export HOME_VIRTUAL_AUTH_BROWSER=1
export HOME_VIRTUAL_AUTH_BROWSER_OPEN=1
export HOME_VIRTUAL_AUTH_BROWSER_OPEN_CONCURRENT=1
export HOME_VIRTUAL_AUTH_BROWSER_OPEN_HOLD_MS="${HOME_VIRTUAL_AUTH_BROWSER_OPEN_HOLD_MS:-30000}"
export HOME_VIRTUAL_AUTH_BROWSER_OPEN_URLS="$protected_url"

node scripts/home-passkey-virtual-auth-smoke.mjs
