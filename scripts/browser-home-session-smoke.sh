#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

export HOME_URL="${HOME_URL:-http://localhost:8090/apps/home/}"
export HOME_VIRTUAL_AUTH_BROWSER=1
export HOME_VIRTUAL_AUTH_BROWSER_OPEN=1
export HOME_VIRTUAL_AUTH_BROWSER_OPEN_CONCURRENT="${HOME_VIRTUAL_AUTH_BROWSER_OPEN_CONCURRENT:-2}"
export HOME_VIRTUAL_AUTH_BROWSER_OPEN_HOLD_MS="${HOME_VIRTUAL_AUTH_BROWSER_OPEN_HOLD_MS:-30000}"

node scripts/home-passkey-virtual-auth-smoke.mjs
