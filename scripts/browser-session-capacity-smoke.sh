#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

export HOME_URL="${HOME_URL:-http://localhost:8090/apps/home/}"
export HOME_VIRTUAL_AUTH_BROWSER=1
export HOME_VIRTUAL_AUTH_BROWSER_SUMMARY=1
export HOME_VIRTUAL_AUTH_BROWSER_OPEN=0

node scripts/home-passkey-virtual-auth-smoke.mjs
