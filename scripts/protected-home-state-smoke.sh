#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GATEWAY_URL="${ELASTOS_GATEWAY_URL:-https://elastos.elacitylabs.com}"
GATEWAY_URL="${GATEWAY_URL%/}"

echo "[protected-home-state] source protected-root HTTP regression"
cargo test \
    --manifest-path "$ROOT/elastos/Cargo.toml" \
    -p elastos-server \
    home_browser_state \
    -- --nocapture

echo "[protected-home-state] live unsigned Home summary"
summary="$(
    curl -fsS "${GATEWAY_URL}/api/apps/home/summary"
)"
printf '%s\n' "$summary" | jq -e '
    .browser_state.schema == "elastos.home.browser-state/v1"
    and (.browser_state.session == null or (.browser_state.session | type == "object"))
    and (.authority.wallet_connected == false or .authority.wallet_connected == null)
' >/dev/null

if [[ -n "${ELASTOS_HOME_TOKEN:-}" ]]; then
    echo "[protected-home-state] live signed Home state"
    signed_state="$(
        curl -fsS \
            -H "x-elastos-home-token: ${ELASTOS_HOME_TOKEN}" \
            "${GATEWAY_URL}/api/apps/home/state"
    )"
    printf '%s\n' "$signed_state" | jq -e '
        .schema == "elastos.home.browser-state/v1"
        and (.principal_id | type == "string")
        and (.localhost_root | startswith("localhost://Users/"))
    ' >/dev/null
fi

echo "[protected-home-state] OK"
