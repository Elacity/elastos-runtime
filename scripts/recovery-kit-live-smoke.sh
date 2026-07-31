#!/usr/bin/env bash
set -euo pipefail

GATEWAY_URL="${ELASTOS_GATEWAY_URL:-https://elastos.elacitylabs.com}"
GATEWAY_URL="${GATEWAY_URL%/}"
HOME_TOKEN="${ELASTOS_HOME_TOKEN:-${ELASTOS_SYSTEM_TOKEN:-}}"
HOME_COOKIE="${ELASTOS_HOME_COOKIE:-${ELASTOS_COOKIE:-}}"
HOME_COOKIE_JAR="${ELASTOS_HOME_COOKIE_JAR:-${ELASTOS_COOKIE_JAR:-}}"
PASSWORD="${ELASTOS_RECOVERY_KIT_PASSWORD:-}"
FRESH_PASSKEY_TOKEN="${ELASTOS_FRESH_PASSKEY_HOME_TOKEN:-}"
ALLOW_IMPORT="${ELASTOS_RECOVERY_KIT_IMPORT:-0}"
CURL_AUTH_ARGS=()

usage() {
    cat <<EOF
Usage: ELASTOS_HOME_TOKEN=<signed-token> $(basename "$0")

Live Full Recovery Bundle proof for a signed Home/System session. Export needs
a fresh request-bound passkey token from System. Import remains opt-in. Auth can
use a copied token, a copied Cookie header, or a curl-compatible cookie jar.

Environment:
  ELASTOS_GATEWAY_URL             Default: https://elastos.elacitylabs.com
  ELASTOS_HOME_TOKEN              Signed Home/System token for the active user
  ELASTOS_SYSTEM_TOKEN            Accepted alias for ELASTOS_HOME_TOKEN
  ELASTOS_HOME_COOKIE             Cookie header containing home-session=<token>
  ELASTOS_HOME_COOKIE_JAR         curl cookie jar containing home-session
  ELASTOS_FRESH_PASSKEY_HOME_TOKEN  Fresh System passkey token for this export
  ELASTOS_RECOVERY_KIT_PASSWORD   Optional package password for export/import
  ELASTOS_RECOVERY_KIT_IMPORT=1   Import the exported full bundle into same root
EOF
}

need_cmd() {
    command -v "$1" >/dev/null 2>&1 || {
        echo "[recovery-kit-live-smoke] missing required command: $1" >&2
        exit 2
    }
}

configure_auth() {
    if [[ -n "$HOME_TOKEN" ]]; then
        CURL_AUTH_ARGS=(-H "x-elastos-home-token: ${HOME_TOKEN}")
        return 0
    fi
    if [[ -n "$HOME_COOKIE" ]]; then
        CURL_AUTH_ARGS=(-H "Cookie: ${HOME_COOKIE}")
        return 0
    fi
    if [[ -n "$HOME_COOKIE_JAR" ]]; then
        if [[ ! -f "$HOME_COOKIE_JAR" ]]; then
            echo "[recovery-kit-live-smoke] cookie jar does not exist: $HOME_COOKIE_JAR" >&2
            exit 2
        fi
        CURL_AUTH_ARGS=(-b "$HOME_COOKIE_JAR")
        return 0
    fi
    return 1
}

post_json() {
    local path="$1"
    local body="$2"
    curl -fsS \
        "${CURL_AUTH_ARGS[@]}" \
        -H "content-type: application/json" \
        -d "$body" \
        "${GATEWAY_URL}${path}"
}

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
    usage
    exit 0
fi

need_cmd curl
need_cmd jq

if ! configure_auth; then
    echo "[recovery-kit-live-smoke] SKIP: set ELASTOS_HOME_TOKEN, ELASTOS_HOME_COOKIE, or ELASTOS_HOME_COOKIE_JAR from a signed Home/System session"
    exit 0
fi

echo "[recovery-kit-live-smoke] signed recovery status"
status="$(
    curl -fsS \
        "${CURL_AUTH_ARGS[@]}" \
        "${GATEWAY_URL}/api/auth/recovery/status"
)"

printf '%s\n' "$status" | jq -e '
    .schema == "elastos.principal.root-recovery.status/v1"
    and (.principal_id | type == "string" and length > 0)
    and (.localhost_root | startswith("localhost://Users/"))
    and (.crypto.cipher | type == "string" and length > 0)
' >/dev/null

principal="$(jq -r '.principal_id' <<<"$status")"
localhost_root="$(jq -r '.localhost_root' <<<"$status")"
if [[ -z "$FRESH_PASSKEY_TOKEN" ]]; then
    echo "[recovery-kit-live-smoke] SKIP: set ELASTOS_FRESH_PASSKEY_HOME_TOKEN from a fresh System passkey verification"
    exit 0
fi

request_common="$(
    jq -nc \
        --arg principal_id "$principal" \
        --arg localhost_root "$localhost_root" \
        '{principal_id: $principal_id, localhost_root: $localhost_root}'
)"

echo "[recovery-kit-live-smoke] export full recovery bundle"
body="$(
    jq -nc \
        --argjson common "$request_common" \
        --arg home_token "$FRESH_PASSKEY_TOKEN" \
        --arg password "$PASSWORD" \
        '$common + {
          schema: "elastos.full-recovery-bundle.export.request/v1",
          label: "Recovery Kit",
          home_token: $home_token
        } + (if $password == "" then {} else {download_password: $password} end)'
)"
kit="$(post_json "/api/auth/recovery/full-export" "$body")"

expected_schema="elastos.full-recovery-bundle/v1"
if [[ -n "$PASSWORD" ]]; then
    expected_schema="elastos.full-recovery-bundle.package/v1"
fi

printf '%s\n' "$kit" | jq -e \
    --arg schema "$expected_schema" \
    --arg principal_id "$principal" \
    --arg localhost_root "$localhost_root" \
    '.schema == $schema
     and .principal_id == $principal_id
     and .localhost_root == $localhost_root
     and (.bundle_id | type == "string" and length > 0)' >/dev/null

if [[ "$ALLOW_IMPORT" == "1" ]]; then
    echo "[recovery-kit-live-smoke] import exported full bundle into same root"
    kit_schema="$(jq -r '.schema' <<<"$kit")"
    if [[ "$kit_schema" == "elastos.full-recovery-bundle.package/v1" ]]; then
        import_body="$(
            jq -nc \
                --argjson common "$request_common" \
                --argjson package "$kit" \
                --arg password "$PASSWORD" \
                '$common + {
                  schema: "elastos.full-recovery-bundle.import.request/v1",
                  reassign_to_current_principal: false,
                  package: $package
                } + (if $password == "" then {} else {password: $password} end)'
        )"
    else
        import_body="$(
            jq -nc \
                --argjson common "$request_common" \
                --argjson bundle "$kit" \
                '$common + {
                  schema: "elastos.full-recovery-bundle.import.request/v1",
                  reassign_to_current_principal: false,
                  bundle: $bundle
                }'
        )"
    fi
    imported="$(post_json "/api/auth/recovery/full-import" "$import_body")"
    printf '%s\n' "$imported" | jq -e \
        --arg principal_id "$principal" \
        --arg localhost_root "$localhost_root" \
        '.schema == "elastos.full-recovery-bundle.import.response/v2"
         and .status == "imported"
         and .principal_id == $principal_id
         and .localhost_root == $localhost_root
         and .wallet_restore.status == "complete"
         and (.wallet_restore.expected_count | type == "number")
         and .wallet_restore.imported_count == .wallet_restore.expected_count
         and .wallet_restore.reason_code == "none"
         and .runtime_audit.status == "complete"
         and .runtime_audit.reason_code == "none"' >/dev/null
fi

echo "[recovery-kit-live-smoke] OK"
