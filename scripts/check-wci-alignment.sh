#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

# ripgrep is REQUIRED. The forbidden-pattern checks below pass load-bearing
# `--glob '!...'` exclusions (provider/connector/test/capsule.json exemptions, plus
# `!target/**`). Those globs are not optional: without them a check matches the very
# capsules it is meant to exempt (e.g. wallet-provider, wallet-metamask) and reports
# false failures, and a path-scoped check scans compiled binaries under
# `capsules/*/target/`. A plain-grep fallback cannot faithfully reproduce ripgrep's
# gitignore-aware, multi-glob semantics, so we fail loudly here — before any caller
# redirects stderr to /dev/null — rather than silently produce wrong results.
if ! command -v rg >/dev/null 2>&1; then
  echo "[alignment] ERROR: ripgrep (rg) is required for alignment-check." >&2
  echo "[alignment]   the forbidden-pattern checks rely on rg --glob exclusions that a" >&2
  echo "[alignment]   grep fallback cannot honor; running without rg yields false results." >&2
  echo "[alignment]   install: 'brew install ripgrep' (macOS) or 'apt-get install ripgrep' (Debian/Ubuntu)." >&2
  exit 2
fi

# Capsules that declare `"role": "provider"` are part of the provider plane and are
# exempt from the "ordinary app capsule must not touch wallet/chain authority" checks —
# even when their directory name does not end in `-provider` (e.g. content-market,
# browser-engine-adapter, operator-drive-adapter). Build role-based exemption globs so
# the exemption tracks the declared role, not a `-provider` name convention.
provider_role_globs=()
for manifest in capsules/*/capsule.json; do
  if rg -q '"role"[[:space:]]*:[[:space:]]*"provider"' "$manifest" 2>/dev/null; then
    provider_dir="$(basename "$(dirname "$manifest")")"
    provider_role_globs+=( --glob "!capsules/${provider_dir}/**" )
  fi
done
# A directory under capsules/ with NO capsule.json is not a deployable app capsule — it is a
# shared library crate (e.g. ddrm-envelope) or a node/authority binary (e.g. dkms-authority),
# which legitimately holds crypto primitives and its own on-chain reads. The "app capsule must
# not touch X authority" checks target app capsules (manifest-bearing by definition), so exempt
# the manifest-less crates too — the exemption tracks "is an app capsule", not a name.
for capsule_dir in capsules/*/; do
  if [[ ! -f "${capsule_dir}capsule.json" ]]; then
    provider_role_globs+=( --glob "!${capsule_dir%/}/**" )
  fi
done

scope=(
  README.md
  docs
  elastos
  capsules
  scripts
  state.md
  TASKS.md
)

exclude_globs=(
  --glob '!archive/**'
  --glob '!docs/ANTI_DRIFT.md'
  --glob '!plans/**'
  --glob '!scripts/check-wci-alignment.sh'
  --glob '!target/**'
)

failed=0
tmp="$(mktemp)"
trap 'rm -f "$tmp"' EXIT

rg_search() {
  local pattern="$1"
  shift
  rg -n "$pattern" "$@"
}

check_forbidden() {
  local pattern="$1"
  local label="$2"
  if rg_search "$pattern" "${scope[@]}" "${exclude_globs[@]}" >"$tmp" 2>/dev/null; then
    echo "[alignment] forbidden pattern found: $label"
    cat "$tmp"
    echo
    failed=1
  fi
}

check_required() {
  local pattern="$1"
  local path="$2"
  local label="$3"
  if ! rg_search "$pattern" "$path" >"$tmp" 2>/dev/null; then
    echo "[alignment] required pattern missing: $label"
    echo "  file: $path"
    echo
    failed=1
  fi
}

check_forbidden_in_path() {
  local pattern="$1"
  local path="$2"
  local label="$3"
  if rg_search "$pattern" "$path" >"$tmp" 2>/dev/null; then
    echo "[alignment] forbidden pattern found: $label"
    cat "$tmp"
    echo
    failed=1
  fi
}

# Drop rg `-n` hits (path:line:content) whose matched line is comment-only. The scanned
# capsules are Rust/JS/TS/HTML, so the comment forms are `//`, `/* … */`, block-comment
# continuations (`* …`), and `<!-- … -->`. `#` (a Rust attribute) and a bare leading `*`
# (a deref) are deliberately NOT treated as comments. A documentation line that merely
# NAMES a provider/authority is not a code reference, so it must not trip these checks.
strip_comment_hits() {
  awk '{
    content = $0
    sub(/^[^:]*:[0-9]+:/, "", content)
    probe = content
    sub(/^[[:space:]]+/, "", probe)
    if (probe ~ /^(\/\/|\/\*|\*\/|\* |<!--)/) next
    print
  }'
}

# "Ordinary app capsule must not touch X authority" check: search `capsules` for
# forbidden CODE references (comment-only mentions are ignored via strip_comment_hits),
# always exempting the provider plane (by name and by declared role), capsule manifests,
# and test/spec/build files. Args: label, rg pattern, then any check-specific --glob args.
check_app_authority() {
  local label="$1" pattern="$2"
  shift 2
  rg_search "$pattern" capsules \
    --glob '!**/capsule.json' \
    --glob '!**/Cargo.toml' \
    --glob '!**/Cargo.lock' \
    --glob '!**/tests/**' \
    --glob '!**/*test*' \
    --glob '!**/*spec*' \
    --glob '!**/target/**' \
    --glob '!capsules/*-provider/**' \
    --glob '!elastos/capsules/*-provider/**' \
    "${provider_role_globs[@]}" \
    "$@" 2>/dev/null | strip_comment_hits >"$tmp" || true
  if [[ -s "$tmp" ]]; then
    echo "[alignment] forbidden pattern found: $label"
    cat "$tmp"
    echo
    failed=1
  fi
}

check_forbidden 'AgenticAI' 'legacy root AgenticAI'
check_forbidden 'localhost://Elastos' 'legacy localhost root name'
check_forbidden '\.DataCache' 'legacy per-user cache root'
check_forbidden 'LocalHost://' 'malformed rooted localhost cache path'
check_forbidden 'localhost://WebSpaces/[^[:space:]/`"]+://' 'nested :// inside rooted WebSpaces path'
check_forbidden 'GlobalRegistry' 'legacy registry name'
check_forbidden 'localhost://storage' 'legacy single-root localhost contract'
check_forbidden 'Local/PC2' 'legacy PC2 local session path'
check_forbidden 'join\("Local"\)\.join\("PC2"\)' 'legacy PC2 local session join path'
check_forbidden 'site stage\|list\|path\|serve' 'stale site command claim including removed list subcommand'
check_forbidden 'setup --profile irc' 'legacy setup profile guidance'
check_forbidden 'Start runtime:[[:space:]]+elastos serve' 'legacy install banner runtime hint'
check_forbidden 'Using publisher gateway' 'setup/update should not present publisher gateway as default ElastOS transport'
check_forbidden 'Checking publisher gateway' 'update should not default to publisher gateway transport'
check_forbidden 'IPFS gateway:[[:space:]]+https://' 'share should not print a default public web gateway'
check_forbidden 'DEFAULT_GATEWAYS=\(' 'installer should not bake in public IPFS gateway defaults'
check_forbidden 'canonical user-facing transport' 'installer/docs should not teach web transport as the normal post-install model'
check_forbidden 'Contacts publisher gateway directly' 'command docs should not describe update as gateway-first'
check_forbidden 'alias /var/www/elastos/' 'nginx should not own published application objects directly'
check_forbidden 'proxy_pass http://127\.0\.0\.1:8081' 'public nginx edge should not route the canonical site through the preview site service'
check_forbidden_in_path '/api/provider/did/sign([^_[:alnum:]]|$)' elastos/crates 'capsules must not call arbitrary DID signing routes'
check_forbidden_in_path '"operations":.*"sign"' capsules/did-provider/capsule.json 'did-provider must expose typed signing intents, not generic sign(data)'

check_required 'Home front door' README.md 'README must teach Home front door'
check_required 'No Ambient Authority' PRINCIPLES.md 'principles file must codify explicit authority boundaries'
check_required 'Carrier Plane For Local And Off-Box' PRINCIPLES.md 'principles file must codify the Carrier capability plane for local and off-box transport'
check_required 'audit-linux-runtime-portability\.sh' scripts/publish-release.sh 'publish release must audit Linux runtime portability before publishing'
check_forbidden_in_path 'using default: \$ELASTOS' scripts/publish-release.sh 'public Linux runtime publish must not silently fall back to the glibc host binary'
check_required 'opens Home' docs/GETTING_STARTED.md 'Getting Started must teach Home front door'
check_required 'Open Home:' scripts/install.sh 'installer banner must teach Home'
check_required 'localhost://UsersAI' docs/NAMESPACES.md 'namespace docs must teach UsersAI rooted localhost'
check_required 'localhost://ElastOS' docs/NAMESPACES.md 'namespace docs must teach ElastOS rooted localhost'
check_required 'SharedByLocalUsersAndBots' elastos/crates/elastos-server/src/home_cmd.rs 'Home session code must use the shared Local workspace path'
check_required 'route\("/release\.json"' elastos/crates/elastos-server/src/api/gateway.rs 'gateway must serve release.json'
check_required 'route\("/artifacts/\*path"' elastos/crates/elastos-server/src/api/gateway.rs 'gateway must serve published artifacts'
check_required 'X-Elastos-Site-Origin' elastos/crates/elastos-server/src/api/gateway_site.rs 'gateway must stamp public site responses with rooted origin'
check_required 'X-Elastos-Site-Head-Release' elastos/crates/elastos-server/src/api/gateway_site.rs 'gateway must expose active named site releases when present'
check_required 'X-Elastos-Site-Head-Channel' elastos/crates/elastos-server/src/api/gateway_site.rs 'gateway must expose active release channels when present'
check_required 'SystemServices/Publisher' docs/SITES.md 'site docs must teach the Publisher system-service root'
check_required 'SiteReleases' docs/SITES.md 'site docs must teach Publisher site release state'
check_required 'SystemServices/Edge' docs/SITES.md 'site docs must teach the Edge system-service root'
check_required 'ReleaseChannels' docs/SITES.md 'site docs must teach Edge release channel state'
check_required 'SiteHistory' docs/SITES.md 'site docs must teach Edge site history state'
check_required 'site publish' docs/SITES.md 'site docs must teach CID-backed site publish'
check_required 'site releases' docs/SITES.md 'site docs must teach named site releases'
check_required 'site channels' docs/SITES.md 'site docs must teach site channels'
check_required 'site activate' docs/SITES.md 'site docs must teach signed site activation'
check_required 'site history' docs/SITES.md 'site docs must teach site history'
check_required 'site rollback' docs/SITES.md 'site docs must teach site rollback'
check_required 'site promote' docs/SITES.md 'site docs must teach site promotion'
check_required 'public-install-operator-smoke\.sh' docs/RUNTIME_REPO_USER_STORY_CHECKLIST.md 'runtime repo checklist must record the installed operator/update proof'
check_required 'public-install-identity-smoke\.sh' docs/RUNTIME_REPO_USER_STORY_CHECKLIST.md 'runtime repo checklist must record the DID/profile proof contract'
check_required 'audit-linux-runtime-portability\.sh' docs/RUNTIME_REPO_USER_STORY_CHECKLIST.md 'runtime repo checklist must record the public Linux runtime portability proof'
check_required 'protected-content-provider-contract-smoke\.sh' docs/RUNTIME_REPO_USER_STORY_CHECKLIST.md 'runtime repo checklist must record the protected-content provider journey proof'
check_required 'just verify-release' docs/RUNTIME_REPO_USER_STORY_CHECKLIST.md 'runtime repo checklist must record the canonical release-trust gate'
check_required 'public-install-operator-smoke\.sh' state.md 'state ledger must record the explicit installed operator/update proof'
check_required 'local-identity-profile-smoke\.sh|public-install-identity-smoke\.sh' state.md 'state ledger must record the DID/profile proof path'
check_required 'audit-linux-runtime-portability\.sh' state.md 'state ledger must record the explicit public Linux runtime portability proof'
check_required 'protected-content-provider-contract-smoke\.sh' state.md 'state ledger must record the protected-content provider journey proof'
check_required 'public-install-operator-smoke\.sh' TASKS.md 'tasks must keep the installed operator/update proof in scope'
check_required 'public-install-identity-smoke\.sh|DID-backed People/profile contract' TASKS.md 'tasks must keep the DID/profile public proof in scope'
check_required 'audit-linux-runtime-portability\.sh' TASKS.md 'tasks must keep the public Linux runtime portability proof in scope'
check_required 'protected-content-provider-contract-smoke\.sh' TASKS.md 'tasks must keep the protected-content provider journey proof in scope'
check_required 'BindDomain' elastos/crates/elastos-server/src/main.rs 'site command surface must expose bind-domain'
check_required 'Publish' elastos/crates/elastos-server/src/main.rs 'site command surface must expose publish'
check_required 'Releases' elastos/crates/elastos-server/src/main.rs 'site command surface must expose releases'
check_required 'Channels' elastos/crates/elastos-server/src/main.rs 'site command surface must expose channels'
check_required 'Activate' elastos/crates/elastos-server/src/main.rs 'site command surface must expose activate'
check_required 'History' elastos/crates/elastos-server/src/main.rs 'site command surface must expose history'
check_required 'Rollback' elastos/crates/elastos-server/src/main.rs 'site command surface must expose rollback'
check_required 'Promote' elastos/crates/elastos-server/src/main.rs 'site command surface must expose promote'
check_required 'edge_binding_path' elastos/crates/elastos-server/src/api/gateway_site.rs 'gateway must resolve Host bindings through Edge state'
check_required 'edge_site_head_path' elastos/crates/elastos-server/src/api/gateway_site.rs 'gateway must resolve signed site-head state through Edge'
check_required 'publisher_site_release_path' elastos/crates/elastos-server/src/site_cmd.rs 'site command surface must persist named releases under Publisher state'
check_required 'edge_release_channel_path' elastos/crates/elastos-server/src/site_cmd.rs 'site command surface must persist release channels under Edge state'
check_required 'publisher_release_manifest_path' elastos/crates/elastos-server/src/api/gateway_site.rs 'gateway must read release manifests from Publisher state'
check_required 'publish_to_content_availability' elastos/crates/elastos-server/src/gateway_cmd.rs 'public gateway publish must route through content availability'
check_forbidden_in_path 'publish_to_ipfs|no CID in ipfs-provider response' elastos/crates/elastos-server/src/gateway_cmd.rs 'public gateway publish must not bind directly to ipfs-provider'
check_required 'ProviderInvocation' elastos/crates/elastos-server/src/content.rs 'content provider must use the provider invocation envelope'
check_required '"availability",' elastos/crates/elastos-server/src/content.rs 'content provider must keep the internal availability provider seam'
check_required '"availability",' elastos/crates/elastos-runtime/src/provider/registry.rs 'provider registry must reserve elastos://availability for the availability provider'
check_required '"wallet",' elastos/crates/elastos-runtime/src/provider/registry.rs 'provider registry must reserve elastos://wallet for the wallet provider'
check_required '"drm",' elastos/crates/elastos-runtime/src/provider/registry.rs 'provider registry must reserve elastos://drm for the protected-content provider'
check_required '"rights",' elastos/crates/elastos-runtime/src/provider/registry.rs 'provider registry must reserve elastos://rights for the protected-content rights provider'
check_required '"key",' elastos/crates/elastos-runtime/src/provider/registry.rs 'provider registry must reserve elastos://key for the protected-content key provider'
check_required 'elastos://drm/open' elastos/crates/elastos-server/src/provider_resource.rs 'DRM provider requests must map to narrow protected-content capability resources'
check_required 'elastos://content/fetch' elastos/crates/elastos-server/src/provider_resource.rs 'Content provider requests must map to narrow content capability resources'
check_forbidden_in_path '"content" \| "did" \| "peer" => Ok\(format!\("elastos://\{scheme\}/\*"\)\)' elastos/crates/elastos-server/src/provider_resource.rs 'content provider requests must not fall through to a broad wildcard capability resource'
check_required 'elastos://rights/access/has_access_by_content_id' elastos/crates/elastos-server/src/provider_resource.rs 'Rights provider requests must map to narrow protected-content capability resources'
check_required 'elastos://key/release' elastos/crates/elastos-server/src/provider_resource.rs 'Key provider requests must map to narrow protected-content capability resources'
check_required 'elastos://decrypt/session/open' elastos/crates/elastos-server/src/provider_resource.rs 'Decrypt provider requests must map to narrow protected-content capability resources'
check_required 'elastos-auth' capsules/wallet-provider/Cargo.toml 'wallet-provider must share proof primitives through elastos-auth'
check_forbidden_in_path 'elastos-runtime = ' capsules/wallet-provider/Cargo.toml 'wallet-provider must not depend on the full runtime execution stack'
check_required 'pub use elastos_auth' elastos/crates/elastos-runtime/src/auth.rs 'runtime auth module must re-export shared elastos-auth primitives'
check_required 'wallet_provider_data' elastos/crates/elastos-server/src/api/auth_gateway.rs 'EVM wallet auth must route proof operations through wallet-provider'
check_required 'ApprovalRequests' capsules/wallet-provider/src/main.rs 'wallet-provider must expose explicit approval request state instead of raw signing'
check_required 'CreateManagedAccount' capsules/wallet-provider/src/main.rs 'wallet-provider must expose provider-owned managed wallet creation behind runtime approval'
check_required 'ApproveApproval' capsules/wallet-provider/src/main.rs 'wallet-provider must expose explicit approval state instead of raw signing'
check_required 'CompleteApproval' capsules/wallet-provider/src/main.rs 'wallet-provider must expose signature completion receipts instead of raw app signing'
check_required 'SignApproved' capsules/wallet-provider/src/main.rs 'wallet-provider must execute managed signatures only after approval'
check_required 'recover_evm_address' capsules/wallet-provider/src/main.rs 'wallet-provider must verify external EVM approval signatures before completion receipts'
check_required '/api/apps/system/wallet/approvals' elastos/crates/elastos-server/src/api/gateway.rs 'System must expose wallet approval review through the runtime surface'
check_required '/api/apps/system/wallet/managed' elastos/crates/elastos-server/src/api/gateway.rs 'System must expose built-in wallet creation through the runtime surface'
check_required '/api/apps/:wallet_connector/wallet/approvals/:request_id/complete' elastos/crates/elastos-server/src/api/gateway.rs 'Wallet connector capsules must complete external wallet handoffs through the runtime surface'
check_forbidden_in_path 'personal_sign|eth_requestAccounts|window\.ethereum|selectedMetaMaskProvider' capsules/system/browser 'System must not hold injected browser wallet authority'
check_required 'wallet-metamask' capsules/home/browser/shell.js 'System must be able to open the dedicated MetaMask connector instead of signing in place'
check_required 'wallet-unisat' capsules/home/browser/shell.js 'Wallet must be able to open the dedicated UniSat connector instead of signing Bitcoin proofs in place'
check_required 'wallet-approve-request' elastos/crates/elastos-server/src/api/gateway_inbox.rs 'Inbox must expose wallet approval review through the runtime surface'
check_required 'sign_audit_event' elastos/crates/elastos-server/src/auth.rs 'runtime audit events must be signed by runtime authority'
check_required 'elastos://chain/\{network\}/\{op\}' elastos/crates/elastos-server/src/provider_resource.rs 'chain provider operations must map to network-scoped capability resources'
check_required 'SyncHealth' capsules/chain-provider/src/main.rs 'chain-provider must expose typed sync health without raw RPC passthrough'
check_required 'PrepareTransaction' capsules/chain-provider/src/main.rs 'chain-provider must expose typed transaction preparation without raw RPC passthrough'
check_required 'BroadcastTransaction' capsules/chain-provider/src/main.rs 'chain-provider must expose typed transaction broadcast without raw RPC passthrough'
check_required 'NodeLifecycle' capsules/chain-provider/src/main.rs 'chain-provider must expose node lifecycle through provider scope'
check_forbidden_in_path 'get_ipfs_bridge|get_content_registry_with_ipfs_bridge|start_public_share_tunnel' elastos/crates/elastos-server/src/main.rs 'main command dispatcher must not keep stale raw IPFS helper paths'
check_required 'home-unlock' capsules/home/browser/index.html 'Home must expose the passkey-first unlock surface'
check_required 'passkey/(register|authenticate)' capsules/home/browser/shell-auth.js 'Home unlock must use browser-gateway passkey routes'
check_required 'guest_registration_enabled' elastos/crates/elastos-server/src/auth.rs 'auth state must persist the admin-controlled guest enrollment gate'
check_required 'guest passkey registration is disabled' elastos/crates/elastos-server/src/api/auth_gateway.rs 'passkey registration must fail closed when guest enrollment is off'
check_required 'guest passkey registration is disabled' elastos/crates/elastos-server/src/api/handlers/identity.rs 'direct identity passkey registration must fail closed when guest enrollment is off'
check_required 'passkey_credential_principal_id' elastos/crates/elastos-server/src/api/auth_gateway.rs 'browser passkey auth must give each passkey its own principal root'
check_required 'passkey_credential_principal_id' elastos/crates/elastos-server/src/api/handlers/identity.rs 'direct passkey auth must give each passkey its own principal root'
check_forbidden_in_path 'passkey_person_principal_id' elastos/crates/elastos-server/src 'passkeys must not collapse multiple credentials into one identity-store user principal'
check_required '/api/apps/system/access/guest-registration' elastos/crates/elastos-server/src/api/gateway.rs 'System must expose the admin guest-enrollment control'
check_required 'Guest access' capsules/system/browser/index.html 'System must show guest passkey enrollment as access policy'
check_required 'guest_registration_enabled' capsules/home/browser/shell-auth.js 'Home unlock must respect the guest-enrollment gate'
check_required 'standard_home_identity_summary' elastos/crates/elastos-server/src/api/gateway_home_system.rs 'unsigned Home summary must use a standard non-user identity snapshot'
check_required 'standard_home_browser_state' elastos/crates/elastos-server/src/api/gateway_home_system.rs 'unsigned Home summary must not expose principal browser state'
check_required 'presentation: "prompt"' capsules/home/browser/shell.js 'unsigned Home must encourage sign-in without blocking the standard desktop'
check_required '/api/auth/sessions/refresh' capsules/home/browser/shell-auth.js 'Home must refresh proof-bound browser sessions through runtime auth'
check_required 'home state save failed' capsules/home/browser/shell-core.js 'Home browser state writes must stay explicit and observable'
check_required 'passkey the Home front door authority' elastos/CHANGELOG.md 'CHANGELOG must record the implemented passkey front-door authority model'
check_forbidden_in_path 'HOME_BROWSER_STATE_ROOT|ElastOS/System/HomeState' elastos/crates/elastos-server/src/api 'Home browser state must be rooted in the active principal user area, not a shared system bucket'
check_forbidden_in_path 'authority_id|authorityId|home_browser_authority_id' elastos/crates/elastos-server/src/api 'Home browser state must identify the runtime principal, not an ambiguous authority id'
check_required '.AppData/ElastOS/Home/browser-state.json' elastos/crates/elastos-server/src/api/gateway_home_system.rs 'Home browser state must materialize under the active principal localhost root'
check_required 'wallet proof is not bound to this runtime principal' elastos/crates/elastos-server/src/api/auth_gateway.rs 'wallet proof must link to an existing Runtime principal, not mint login'
check_required 'ELASTOS_WALLET_PRICE_HTTP_APPROVED' elastos/crates/elastos-server/src/api/gateway.rs 'wallet price HTTP access must require explicit operator approval'
check_required '/api/auth/passkeys' elastos/crates/elastos-server/src/api/gateway.rs 'browser gateway must expose passkey list/revoke routes through runtime auth'
check_required 'ELASTOS_EXIT_PROVIDER_CONFIG' elastos/crates/elastos-server/src/server_infra.rs 'exit-provider backends must require explicit operator configuration'
check_required 'allowed_hosts' capsules/exit-provider/src/main.rs 'exit-provider http_fetch backend must require host allowlists'
check_required 'max_body_bytes' capsules/exit-provider/src/main.rs 'exit-provider http_fetch backend must enforce body limits'
check_required 'gateway_browser_net_http' elastos/crates/elastos-server/src/api/gateway_browser_stream.rs 'Browser HTTP must route Browser -> Net validation -> internal Exit handoff'
check_required 'StreamRelay' capsules/exit-provider/src/main.rs 'exit-provider must model stream relay separately from HTTP fetch'
check_required 'elastos.exit.stream-session/v1' capsules/exit-provider/src/main.rs 'exit-provider stream relay must return typed stream-session receipts'
check_required 'elastos.adapter-ipc/v1' capsules/exit-provider/src/main.rs 'exit-provider stream relay must model private Browser Engine Adapter IPC descriptors'
check_required 'elastos.exit.relay-ipc/v1' capsules/exit-provider/src/main.rs 'exit-provider stream relay must model private Exit relay IPC descriptors'
check_required 'RelayIpcConfig' capsules/exit-provider/src/main.rs 'exit-provider stream relay config must separate engine adapter IPC from Exit relay IPC'
check_required 'gateway_browser_net_stream' elastos/crates/elastos-server/src/api/gateway_browser_stream.rs 'Browser streams must route Browser -> Net validation -> internal Exit handoff'
check_required 'browser-native-supervisor-smoke\.sh' TASKS.md 'native browser isolation proof must remain tracked in tasks'
check_required 'browser-native-supervisor-smoke\.sh' docs/BROWSER_CAPSULE.md 'browser docs must record the host-gated native supervisor proof'
check_required 'browser-native-proxy-engine-smoke\.sh' TASKS.md 'native browser proxy wrapper smoke must remain tracked in tasks'
check_required 'browser-native-proxy-engine-smoke\.sh' docs/BROWSER_CAPSULE.md 'browser docs must record the native proxy wrapper smoke'
check_required 'browser-native-supervisor-proxy-smoke\.sh' TASKS.md 'native browser supervisor/proxy smoke must remain tracked in tasks'
check_required 'browser-native-supervisor-proxy-smoke\.sh' docs/BROWSER_CAPSULE.md 'browser docs must record the native supervisor/proxy smoke'
check_required 'browser-native-operator-config\.mjs' TASKS.md 'native browser operator config generator must remain tracked in tasks'
check_required 'browser-native-operator-config\.mjs' docs/BROWSER_CAPSULE.md 'browser docs must record native operator config generation'
check_required 'browser-native-target-preflight\.sh' TASKS.md 'native browser target-host preflight must remain tracked in tasks'
check_required 'browser-native-target-preflight\.sh' docs/BROWSER_CAPSULE.md 'browser docs must record native target-host preflight'
check_required 'browser-engine-adapter\.json' scripts/browser-native-operator-config.mjs 'native operator config generator must emit browser-engine adapter config'
check_required 'exit-provider\.json' scripts/browser-native-operator-config.mjs 'native operator config generator must emit exit-provider config'
check_required 'browser-local-exit\.json' scripts/browser-native-operator-config.mjs 'native operator config generator must emit browser-local-exit config'
check_required 'ELASTOS_BROWSER_NATIVE_PROXY_ENGINE_CONFIG' scripts/browser-native-operator-config.mjs 'native operator config generator must wire proxy config through supervisor env'
check_required 'proxy-server=\{proxy_url\}' scripts/browser-native-operator-config.mjs 'native operator config generator must force Chromium/CEF through the Runtime proxy'
check_required 'skipped.*target host is not proven|target host is not proven' scripts/browser-native-target-preflight.sh 'native target preflight must fail closed when host-gated proof skips'
check_required 'wallet-connector-transaction-smoke\.mjs' TASKS.md 'Browser wallet connector transaction smoke must remain tracked in tasks'
check_required 'wallet-connector-transaction-smoke\.mjs' docs/BROWSER_CAPSULE.md 'Browser docs must keep connector transaction smoke as a regression gate'
check_required 'eth_sendTransaction' scripts/wallet-connector-transaction-smoke.mjs 'connector smoke must prove external transaction submission'
check_required 'wallet_addEthereumChain' scripts/wallet-connector-transaction-smoke.mjs 'connector smoke must prove known-chain add handling'
check_required 'transaction_hash' scripts/wallet-connector-transaction-smoke.mjs 'connector smoke must prove transaction-hash-only Runtime completion'
check_forbidden_in_path 'fallback|frame preview|showing Runtime frame preview' capsules/browser 'Browser UI must not silently downgrade to fallback frame previews'
check_forbidden_in_path 'fallback|showing Runtime frame preview' capsules/browser-engine-adapter 'Browser Engine Adapter must fail closed instead of fallback display modes'
check_forbidden_in_path 'fallback|showing Runtime frame preview' elastos/tools/browser-playwright-engine/src 'Browser proof engine must not present fallback display paths as normal browsing'
check_required 'cdp_screencast_i420' elastos/tools/browser-playwright-engine/src/supervisor.mjs 'Playwright proof backend must identify itself as CDP screencast, not final compositor'
check_required 'proof_surface' elastos/tools/browser-playwright-engine/src/supervisor.mjs 'Playwright proof backend must identify itself as proof surface'
check_required 'display_modes' capsules/browser-engine-adapter/src 'Browser Engine Adapter must enforce explicit display modes'
check_required 'ELASTOS_BROWSER_ENGINE_ADAPTER_CONFIG' elastos/crates/elastos-server/src/server_infra.rs 'browser-engine adapter must require explicit operator configuration'
check_required 'display_modes' capsules/browser-engine-adapter/src 'browser-engine adapter must require explicit display-mode declarations'
check_required 'elastos.browser.engine.page/v1' capsules/browser-engine-adapter/src 'browser-engine adapter must return typed page receipts'
check_required 'elastos.adapter-ipc/v1' capsules/browser-engine-adapter/src 'browser-engine adapter must validate private adapter_ipc descriptors'
check_required 'elastos.browser.engine.launch-request/v1' capsules/browser-engine-adapter/src 'browser-engine adapter must use a typed native supervisor launch request'
check_required 'elastos.browser.engine.supervisor-result/v1' capsules/browser-engine-adapter/src 'browser-engine adapter must require typed native supervisor launch results'
check_required 'webrtc_signal' capsules/browser-engine-adapter/src 'browser-engine adapter must route WebRTC signaling through Runtime provider operation'
check_required 'ELASTOS_BROWSER_ENGINE_REQUEST' capsules/browser-engine-adapter/src 'browser-engine adapter must pass native supervisor launch requests through explicit environment contract'
check_required 'ELASTOS_BROWSER_ENGINE_SUPERVISOR_CONFIG' elastos/tools/browser-engine-supervisor/src/main.rs 'browser-engine supervisor must require explicit operator config'
check_required 'ELASTOS_BROWSER_ENGINE_REQUEST' elastos/tools/browser-engine-supervisor/src/main.rs 'browser-engine supervisor must read typed launch requests only'
check_required 'Page.startScreencast' elastos/tools/browser-playwright-engine/src/supervisor.mjs 'hosted browser proof must use a real WebRTC display sender, not the HTTP frame route as product display'
check_required 'webrtc_remote_display' elastos/tools/browser-playwright-engine/src/supervisor.mjs 'hosted browser proof must declare WebRTC remote-display support'
check_required 'CLONE_NEWNET' elastos/tools/browser-engine-supervisor/src/main.rs 'browser-engine supervisor must deny ambient Linux networking'
check_required 'bring_loopback_up' elastos/tools/browser-engine-supervisor/src/main.rs 'browser-engine supervisor must bring loopback up for sandbox-local browser proxying'
check_required 'SIOCSIFFLAGS' elastos/tools/browser-engine-supervisor/src/main.rs 'browser-engine supervisor must explicitly configure loopback inside the native browser network namespace'
check_required 'elastos.browser.engine.supervisor-config/v1' elastos/tools/browser-engine-supervisor/src/main.rs 'browser-engine supervisor config must be typed'
check_required 'elastos.browser.engine.supervisor-result/v1' elastos/tools/browser-engine-supervisor/src/main.rs 'browser-engine supervisor result must be typed'
check_required 'ELASTOS_BROWSER_ENGINE_IPC' elastos/tools/browser-engine-supervisor/src/main.rs 'browser-engine supervisor must pass only explicit stream IPC to engines'
check_required 'ELASTOS_BROWSER_ENGINE_RELAY_IPC' elastos/tools/browser-engine-supervisor/src/main.rs 'browser-engine supervisor must pass only explicit Exit relay IPC to native proxy engines'
check_required 'ELASTOS_BROWSER_STREAM_BRIDGE_CONFIG' elastos/tools/browser-engine-supervisor/src/main.rs 'browser-engine supervisor must be able to start the local stream bridge without exposing host networking'
check_required 'stream_bridge_pid' elastos/tools/browser-engine-supervisor/src/main.rs 'browser-engine supervisor result must expose stream bridge process status when configured'
check_required 'ELASTOS_BROWSER_NATIVE_PROXY_ENGINE_CONFIG' elastos/tools/browser-native-proxy-engine/src/main.rs 'native browser proxy wrapper must require explicit operator config'
check_required 'ELASTOS_BROWSER_ENGINE_RELAY_IPC' elastos/tools/browser-native-proxy-engine/src/main.rs 'native browser proxy wrapper must receive Runtime Exit relay IPC from supervisor'
check_required 'elastos.exit.relay-open/v1' elastos/tools/browser-native-proxy-engine/src/main.rs 'native browser proxy wrapper must open typed Runtime Exit relay streams'
check_required 'CONNECT' elastos/tools/browser-native-proxy-engine/src/main.rs 'native browser proxy wrapper must support HTTPS CONNECT without becoming a content proxy'
check_required 'TcpListener::bind\("127.0.0.1:0"\)' elastos/tools/browser-native-proxy-engine/src/main.rs 'native browser proxy wrapper must expose only a loopback browser proxy inside the engine sandbox'
check_forbidden_in_path 'runtime_stream_path' capsules/exit-provider 'exit-provider must not own Runtime stream socket paths'
check_required 'runtime_stream_path' capsules/browser-engine-adapter/src 'browser-engine adapter must validate private Runtime stream socket descriptors'
check_required 'ELASTOS_BROWSER_STREAM_BRIDGE_CONFIG' elastos/tools/browser-stream-bridge/src/main.rs 'browser stream bridge must require explicit Runtime-owned stream config'
check_required 'elastos.browser.stream-bridge.config/v1' elastos/tools/browser-stream-bridge/src/main.rs 'browser stream bridge config must be typed'
check_required 'elastos.browser.stream-bridge.ready/v1' elastos/tools/browser-stream-bridge/src/main.rs 'browser stream bridge readiness must be typed'
check_required 'UnixListener' elastos/tools/browser-stream-bridge/src/main.rs 'browser stream bridge must accept only local Unix adapter sockets'
check_required 'UnixStream' elastos/tools/browser-stream-bridge/src/main.rs 'browser stream bridge must connect only local Runtime stream sockets'
check_forbidden_in_path 'TcpStream|ToSocketAddrs|UdpSocket|lookup_host' elastos/tools/browser-stream-bridge 'browser stream bridge must not contain raw host networking or DNS'
check_required 'ELASTOS_BROWSER_LOCAL_EXIT_CONFIG' elastos/tools/browser-local-exit/src/main.rs 'browser local Exit must require explicit operator config'
check_required 'elastos.browser.local-exit.config/v1' elastos/tools/browser-local-exit/src/main.rs 'browser local Exit config must be typed'
check_required 'elastos.exit.relay-open/v1' elastos/tools/browser-local-exit/src/main.rs 'browser local Exit must accept only typed Runtime relay-open handshakes'
check_required 'allowed_hosts' elastos/tools/browser-local-exit/src/main.rs 'browser local Exit must require host allowlists'
check_required 'address_family' elastos/tools/browser-local-exit/src/main.rs 'browser local Exit must make address-family routing an explicit Exit policy'
check_required 'allow_private_targets' elastos/tools/browser-local-exit/src/main.rs 'browser local Exit must block private targets unless the operator explicitly enables them'
check_required 'TcpStream' elastos/tools/browser-local-exit/src/main.rs 'browser local Exit is the only explicit server-side TCP relay in the Browser path'
check_required 'ToSocketAddrs' elastos/tools/browser-local-exit/src/main.rs 'browser local Exit is the only explicit server-side DNS resolver in the Browser path'
check_required 'private resolved IP blocked' elastos/tools/browser-local-exit/src/main.rs 'browser local Exit must block private resolved IPs by default'
check_required 'byte_transport_unavailable' capsules/browser-engine-adapter/src 'browser-engine adapter must fail closed without attached byte transport'
check_required 'validate_supervisor_result' capsules/browser-engine-adapter/src 'browser-engine adapter must validate native supervisor proof before returning a page receipt'
check_required 'browser_engine_summary' elastos/crates/elastos-server/src/api/gateway_browser_engine.rs 'Browser summary must report the internal Browser Engine Adapter contract'
check_required 'browser_app_open' elastos/crates/elastos-server/src/api/gateway_browser.rs 'Browser open must be a high-level Runtime route, not raw provider access from the UI'
check_required 'browser_provider_resource_call' elastos/crates/elastos-server/src/api/gateway_browser_response.rs 'Browser internal provider handoffs must validate Carrier/provider resource URIs'
check_required 'elastos://net/stream' elastos/crates/elastos-server/src/api/gateway_browser_stream.rs 'Browser open must reserve streams through the Net resource contract'
check_required 'elastos://exit/open_stream' elastos/crates/elastos-server/src/api/gateway_browser_stream.rs 'Browser open must hand approved egress to the Exit resource contract'
check_required 'elastos://browser-engine/launch' elastos/crates/elastos-server/src/api/gateway_browser.rs 'Browser open must bind streams through the Browser Engine resource contract'
check_required 'elastos://chain/\{network\}/broadcast_transaction' elastos/crates/elastos-server/src/api/gateway_browser_wallet.rs 'Browser transaction approvals must expose the chain broadcast effect resource'
check_required 'browser_visible_stream_session' elastos/crates/elastos-server/src/api/gateway_browser_stream.rs 'Browser open must strip private adapter_ipc descriptors from UI responses'
check_required 'browser_attach_runtime_stream_path' elastos/crates/elastos-server/src/api/gateway_browser_stream.rs 'Browser open must allocate Runtime-owned stream socket paths before engine launch'
check_required 'browser_stream_relay' elastos/crates/elastos-server/src/api/gateway_browser_stream.rs 'Browser open must validate private Exit relay IPC descriptors before relaying bytes'
check_required 'elastos.exit.relay-open/v1' elastos/crates/elastos-server/src/api/gateway_browser_stream.rs 'Browser open must send typed Exit relay-open handshakes before forwarding bytes'
check_required 'browser_engine_stream_session' elastos/crates/elastos-server/src/api/gateway_browser_stream.rs 'Browser open must build the private engine stream session separately from the visible Browser response'
check_required 'object.remove\("relay_ipc"\)' elastos/crates/elastos-server/src/api/gateway_browser_stream.rs 'Browser open must strip private Exit relay IPC descriptors from Browser UI responses'
check_required 'copy_bidirectional' elastos/crates/elastos-server/src/api/gateway_browser_stream.rs 'Browser runtime stream relay must use local byte forwarding, not raw host networking'
check_required 'spawn_browser_runtime_stream_listener' elastos/crates/elastos-server/src/api/gateway_browser_stream.rs 'Browser open must bind Runtime-owned stream sockets fail-closed until the real relay exists'
check_required 'UnixListener' elastos/crates/elastos-server/src/api/gateway_browser_stream.rs 'Browser runtime stream sockets must be local Unix sockets, not TCP host networking'
check_required 'BROWSER_RUNTIME_STREAM_TMP_DIR' elastos/crates/elastos-server/src/api/gateway_browser_stream.rs 'Browser runtime stream socket paths must use the short Runtime temp socket directory'
check_required 'test_browser_open_runtime_stream_socket_accepts_and_closes_fail_closed' elastos/crates/elastos-server/src/api/gateway_browser_route_tests.rs 'Browser open must test fail-closed Runtime stream socket attach'
check_required 'test_browser_open_runtime_stream_relays_to_exit_ipc_without_host_network' elastos/crates/elastos-server/src/api/gateway_browser_route_tests.rs 'Browser open must test local Runtime-to-Exit relay without raw host networking'
check_required 'browser\["attach_kind"\], "iframe"' elastos/crates/elastos-server/src/api/gateway_tests 'Home must open Browser as an ElastOS window, not a host tab'
check_required '/api/apps/browser/open' capsules/browser/browser/browser.js 'Browser UI must call the high-level Browser open route'
check_required '/api/auth/sessions/refresh' elastos/crates/elastos-server/src/api/gateway.rs 'browser gateway must expose proof-bound session refresh through runtime auth'
check_required 'invalid WebAuthn Origin header' elastos/crates/elastos-server/src/api/handlers/identity.rs 'WebAuthn RP derivation must fail closed on malformed browser origins'
if rg_search 'passkey|webauthn|PublicKeyCredential|credentials\.(create|get)' capsules \
  --glob '!capsules/home/browser/*' \
  --glob '!capsules/system/browser/*' \
  --glob '!capsules/wallet/*' \
  --glob '!capsules/*-provider/**' \
  --glob '!**/target/**' >"$tmp" 2>/dev/null; then
  echo "[alignment] forbidden pattern found: passkey ceremonies must stay in Home/System/runtime auth surfaces"
  cat "$tmp"
  echo
  failed=1
fi
check_forbidden_in_path 'credentials\.create|passkey/register|webauthn/register' capsules/wallet 'Wallet may request fresh passkey authentication for protected recovery, but must not register passkeys'
check_app_authority 'app capsules must not touch browser wallet authority directly' \
  'WalletConnect|walletconnect|MetaMask|metamask|UniSat|unisat|window\.ethereum|window\.unisat|ethereum\.request|personal_sign|eth_requestAccounts|eth_sendTransaction|wallet_switchEthereumChain|signMessage' \
  --glob '!capsules/home/browser/*' \
  --glob '!capsules/system/browser/*' \
  --glob '!capsules/browser/*' \
  --glob '!capsules/wallet-metamask/*' \
  --glob '!capsules/wallet-unisat/*' \
  --glob '!capsules/wallet/*' \
  --glob '!capsules/wallet-walletconnect/*'
check_app_authority 'app capsules must not touch raw chain/node authority directly' \
  'elastos://chain|/api/provider/chain|chain-provider|blockchain provider|rpc_url|RPC_URL|JSON-RPC|jsonrpc|eth_call|eth_chainId|bitcoin-cli|bitcoind|Bitcoin Core RPC' \
  --glob '!capsules/system/browser/*' \
  --glob '!capsules/wallet-metamask/*' \
  --glob '!capsules/wallet-unisat/*' \
  --glob '!capsules/wallet/*' \
  --glob '!capsules/wallet-walletconnect/*'
check_app_authority 'app capsules must not reference raw wallet provider authority directly' \
  'elastos://wallet|/api/provider/wallet|wallet-provider' \
  --glob '!capsules/system/browser/*' \
  --glob '!capsules/browser/*'
check_forbidden_in_path 'home_session_cookie_header|home_session_cookie_is_valid|SET_COOKIE' elastos/crates/elastos-server/src/api/browser_capsules.rs 'Home static route must not auto-mint a local session cookie'
check_forbidden_in_path 'default chat profile' docs/GETTING_STARTED.md 'onboarding must teach the default Home profile, not the old chat profile'
check_forbidden_in_path 'darwin\)' scripts/install.sh 'public installer must stay Linux-only until update/install support macOS coherently'
check_forbidden_in_path 'http://' elastos/crates/elastos-runtime/src/provider/registry.rs 'provider-registry tests/docs must not preserve http:// parity assumptions'
check_forbidden_in_path 'localhost:// = ' README.md 'public docs must not flatten localhost:// into a single-root slogan'
check_forbidden_in_path 'localhost:// = ' docs/OVERVIEW.md 'overview must describe rooted localhost spaces, not a flattened single-root slogan'
check_forbidden_in_path 'did-provider' capsules/chat/capsule.json 'chat capsule should use the host did bridge instead of bundling a stale did-provider dependency'
check_forbidden_in_path 'component\.as_os_str\(\) == "target"' elastos/crates/elastos-server/src/binaries.rs 'provider resolution must not auto-enable repo asset lookup just because the binary runs from target/'
check_forbidden_in_path 'component\.as_os_str\(\) == "target"' elastos/crates/elastos-server/src/ipfs.rs 'viewer resolution must not auto-enable repo asset lookup just because the binary runs from target/'
check_forbidden_in_path 'Legacy TCP fallback' elastos/crates/elastos-server/src/vm_provider.rs 'vm provider bridge must not describe generic TCP fallback as a normal contract'
check_forbidden_in_path 'guest_from_fallback' elastos/crates/elastos-server/src/init.rs 'init should name guest dependency source explicitly instead of treating registry dependency as an unnamed fallback'
check_forbidden_in_path 'ListCapsules|LaunchCapsule|StopCapsule|GrantCapability|RevokeCapability|SendMessage|ReceiveMessages|FetchContent|StorageRead|StorageWrite' elastos/crates/elastos-guest/src/runtime.rs 'guest SDK must expose capsule-kernel calls, not raw runtime control/storage/message APIs'
check_forbidden_in_path 'ProviderCall|ProviderResult|provider_call|provider_result' elastos/crates/elastos-guest/src/runtime.rs 'guest SDK must expose carrier_invoke, not provider_call'
check_forbidden_in_path 'provider_call|Provider call' capsules/chat/src 'chat capsule must use carrier_invoke instead of provider_call'
check_forbidden_in_path 'provider_call|Provider call' capsules/agent/src 'agent capsule must use carrier_invoke instead of provider_call'
check_forbidden_in_path 'provider_call|Provider call' capsules/home-cli/src 'home-cli capsule must use carrier_invoke instead of provider_call'
check_forbidden_in_path 'guest SDK|SDK request|SDK response|mirror the guest' elastos/crates/elastos-runtime/src/handler 'elastos-runtime handler must be named as internal shell/control, not public guest SDK'
check_forbidden_in_path 'get_ipfs_bridge|prepare_capsule_from_cid|send_raw\("ipfs"' elastos/crates/elastos-server/src/run_cmd.rs 'run --cid must materialize through elastos://content, not raw IPFS'
check_forbidden_in_path 'get_ipfs_bridge|prepare_capsule_from_cid|send_raw\("ipfs"' elastos/crates/elastos-server/src/serve_cmd.rs 'serve --cid must materialize through elastos://content, not raw IPFS'
check_forbidden_in_path 'send_raw\("ipfs"|ipfs_cat_via_provider|try_download_capsule_via_ipfs_provider' elastos/crates/elastos-server/src/supervisor.rs 'supervisor capsule downloads must use elastos://content/fetch, not raw IPFS'
check_required 'managed dashboard runtime' docs/OVERVIEW.md 'overview must teach Home as the front door'

python3 - <<'PY'
import json, sys
from pathlib import Path

components = json.loads(Path("components.json").read_text())

allowed_roles = {"shell", "app", "viewer", "provider", "content"}
ordinary_roles = {"app", "viewer", "content"}
# System is the runtime-owned approval/diagnostic surface. Dedicated wallet
# connector capsules and the Browser shell are privileged adapter UIs, not
# general app authority.
ordinary_capsules_with_privileged_authority_ui = {"system", "wallet-metamask", "wallet-unisat", "wallet", "wallet-walletconnect", "browser", "library"}
system_only_elastos_backends = {
    "elacity",
    "elacity-sdk",
    "gateway",
    "chain",
    "wallet",
    "library",
    "ipfs",
    "ipfs-cluster",
    "ipfs-provider",
    "kubo",
}

def is_system_only_backend(resource):
    if not isinstance(resource, str):
        return False
    if not resource.startswith("elastos://"):
        return False
    rest = resource[len("elastos://"):]
    head = rest.split("/", 1)[0].split("?", 1)[0].split("#", 1)[0]
    return head in system_only_elastos_backends

def is_test_or_generated_path(path):
    names = set(path.parts)
    lowered_name = path.name.lower()
    return (
        "target" in names
        or "tests" in names
        or "test" in names
        or "__tests__" in names
        or "test" in lowered_name
        or "spec" in lowered_name
    )

manifest_paths = sorted(Path("capsules").glob("*/capsule.json")) + sorted(Path("elastos/capsules").glob("*/capsule.json"))
for path in manifest_paths:
    manifest = json.loads(path.read_text())
    role = manifest.get("role")
    if role not in allowed_roles:
        print(f"[alignment] {path} has unknown capsule role: {role}")
        sys.exit(1)
    permissions = manifest.get("permissions") or {}
    if not isinstance(permissions, dict):
        print(f"[alignment] {path} permissions must be an object when present")
        sys.exit(1)
    provides = manifest.get("provides")
    if role in ordinary_roles and manifest.get("name") not in ordinary_capsules_with_privileged_authority_ui:
        problems = []
        if permissions.get("guest_network"):
            problems.append("guest_network")
        if permissions.get("carrier"):
            problems.append("carrier host execution")
        if provides:
            problems.append("provides namespace")
        if manifest.get("providers"):
            problems.append("provider implementation override")
        for capability in manifest.get("capabilities") or []:
            if is_system_only_backend(capability):
                problems.append(f"system-only backend capability {capability}")
        for storage in permissions.get("storage") or []:
            if is_system_only_backend(storage):
                problems.append(f"system-only backend storage {storage}")
        microvm = manifest.get("microvm") or {}
        if isinstance(microvm, dict) and microvm.get("http_port"):
            problems.append("microVM HTTP port")
        for req in manifest.get("requires") or []:
            if isinstance(req, dict) and req.get("kind") == "external":
                problems.append(f"external dependency {req.get('name', '<unnamed>')}")
        if problems:
            print(f"[alignment] ordinary capsule {manifest.get('name', path)} declares forbidden authority: {', '.join(problems)}")
            sys.exit(1)
    if provides and role != "provider":
        print(f"[alignment] {path} provides a namespace but is not a provider capsule")
        sys.exit(1)
    if role == "provider" and not provides:
        print(f"[alignment] provider capsule {manifest.get('name', path)} must declare a provides namespace")
        sys.exit(1)
    if role == "provider":
        authority = manifest.get("authority")
        if not isinstance(authority, dict):
            print(f"[alignment] provider capsule {manifest.get('name', path)} must declare authority metadata")
            sys.exit(1)
        if not str(authority.get("reason", "")).strip():
            print(f"[alignment] provider capsule {manifest.get('name', path)} authority reason is empty")
            sys.exit(1)
        if not authority.get("capabilities"):
            print(f"[alignment] provider capsule {manifest.get('name', path)} authority capability schema is empty")
            sys.exit(1)
        if not authority.get("audit_events"):
            print(f"[alignment] provider capsule {manifest.get('name', path)} authority audit events are empty")
            sys.exit(1)
    elif manifest.get("authority") is not None:
        print(f"[alignment] non-provider capsule {manifest.get('name', path)} declares provider authority metadata")
        sys.exit(1)
    if permissions.get("carrier") and (role != "provider" or not provides):
        print(f"[alignment] {path} uses carrier host execution without provider role and provides namespace")
        sys.exit(1)
    if permissions.get("guest_network") and (role != "provider" or not provides):
        print(f"[alignment] {path} uses guest_network without provider role and provides namespace")
        sys.exit(1)
    if role in ordinary_roles and manifest.get("name") not in ordinary_capsules_with_privileged_authority_ui:
        source_roots = [path.parent / "src", path.parent]
        forbidden_source_patterns = {
            "ELASTOS_API": "direct runtime API environment access",
            "elastos://chain": "raw chain provider namespace",
            "elastos://net": "raw Browser/Net provider namespace",
            "elastos://exit": "raw Browser Exit provider namespace",
            "elastos://browser-engine": "raw Browser Engine provider namespace",
            "elastos://wallet": "raw wallet provider namespace",
            "elastos://object": "raw object provider namespace",
            "elastos://ipfs": "raw IPFS backend namespace",
            "elastos://availability": "raw availability backend namespace",
            "elastos://drm": "raw protected-content backend namespace",
            "elastos://rights": "raw protected-content rights backend namespace",
            "elastos://key": "raw protected-content key backend namespace",
            "elastos://decrypt": "raw protected-content decrypt/render backend namespace",
            # Bare provider/adapter capsule *names* (chain-provider, object-provider,
            # wallet-provider, browser-engine-adapter, …) are intentionally NOT topology-leak
            # signals: an app capsule that merely names a provider — in a log prefix, a
            # classification list, or its own VIEWER_ID — is not touching that backend. The
            # real raw-access vectors are kept: the elastos:// namespaces above, the
            # /api/provider/* routes, and the concrete RPC/SDK/loopback tokens below.
            "/api/provider/chain": "direct chain provider route",
            "/api/provider/net": "direct Browser/Net provider route",
            "/api/provider/exit": "direct Browser Exit provider route",
            "/api/provider/browser-engine": "direct Browser Engine provider route",
            "/api/provider/wallet": "direct wallet provider route",
            "ipfs-cluster": "raw IPFS Cluster backend",
            "elacity-sdk": "raw Elacity SDK backend",
            "/api/provider/ipfs": "direct IPFS provider route",
            "WalletConnect": "direct browser wallet adapter authority",
            "walletconnect": "direct browser wallet adapter authority",
            "MetaMask": "direct browser wallet adapter authority",
            "metamask": "direct browser wallet adapter authority",
            "window.ethereum": "direct injected wallet authority",
            "ethereum.request": "direct injected wallet authority",
            "personal_sign": "direct wallet signing authority",
            "eth_requestAccounts": "direct wallet account authority",
            "eth_sendTransaction": "direct wallet transaction authority",
            "wallet_switchEthereumChain": "direct wallet chain-switch authority",
            "rpc_url": "raw RPC endpoint authority",
            "RPC_URL": "raw RPC endpoint authority",
            "JSON-RPC": "raw RPC protocol authority",
            "jsonrpc": "raw RPC protocol authority",
            "eth_call": "raw EVM RPC authority",
            "eth_chainId": "raw EVM RPC authority",
            "bitcoin-cli": "raw node CLI authority",
            "bitcoind": "raw node daemon authority",
            "Bitcoin Core RPC": "raw node RPC authority",
            "blockchain provider": "raw blockchain provider authority",
            "http://127.0.0.1": "direct loopback API topology",
            "http://localhost": "direct loopback API topology",
        }
        for source_root in source_roots:
            if not source_root.exists():
                continue
            for source in sorted(source_root.rglob("*")):
                if is_test_or_generated_path(source):
                    continue
                if source.name in {"mgba.js"} or source.suffix not in {".html", ".rs", ".ts", ".tsx", ".js", ".mjs"}:
                    continue
                # Ignore comment-only lines: a doc comment that merely names a provider
                # or route is not a code reference. Comment forms for the scanned
                # Rust/JS/TS/HTML files are //, /* … */, block continuations (* …), and
                # <!-- … -->; `#` (Rust attribute) and bare `*` (deref) are not comments.
                code_text = "\n".join(
                    ln for ln in source.read_text(errors="ignore").splitlines()
                    if not ln.lstrip().startswith(("//", "/*", "*/", "* ", "<!--"))
                )
                for pattern, reason in forbidden_source_patterns.items():
                    if pattern in code_text:
                        print(f"[alignment] ordinary capsule {manifest.get('name', path)} leaks host topology in {source}: {reason}")
                        sys.exit(1)

def platform_info(component, platform):
    platforms = component.get("platforms") or {}
    return platforms.get(platform) or platforms.get("*")

home = components["profiles"]["home"]["components"]
forbidden = {"kubo", "ipfs-provider", "availability-provider", "site-provider", "tunnel-provider", "cloudflared", "chain-provider", "wallet-provider", "drm-provider", "rights-provider", "key-provider", "decrypt-provider"}
bad = sorted(forbidden.intersection(home))
if bad:
    print("[alignment] home profile includes non-default off-box/public-edge/chain/wallet components:", ", ".join(bad))
    sys.exit(1)
required = {
    "shell",
    "localhost-provider",
    "did-provider",
    "net-provider",
    "exit-provider",
    "browser-engine-adapter",
    "browser-engine-supervisor",
    "browser-stream-bridge",
    "browser-local-exit",
    "webspace-provider",
    "object-provider",
    "home-cli",
    "home",
    "system",
    "documents",
    "library",
    "inbox",
}
missing = sorted(required.difference(home))
if missing:
    print("[alignment] home profile missing required first-party core components:", ", ".join(missing))
    sys.exit(1)
demo = components["profiles"].get("demo")
if not demo:
    print("[alignment] demo profile is missing")
    sys.exit(1)
demo_components = set(demo["components"])
required_demo = {
    "shell",
    "localhost-provider",
    "did-provider",
    "net-provider",
    "exit-provider",
    "browser-engine-adapter",
    "browser-engine-supervisor",
    "browser-stream-bridge",
    "browser-local-exit",
    "webspace-provider",
    "object-provider",
    "home-cli",
    "home",
    "system",
    "kubo",
    "ipfs-provider",
    "site-provider",
    "tunnel-provider",
    "documents",
    "library",
    "inbox",
    "chat-room",
    "cloudflared",
}
missing_demo = sorted(required_demo.difference(demo_components))
if missing_demo:
    print("[alignment] demo profile missing required demo components:", ", ".join(missing_demo))
    sys.exit(1)
chat = components["profiles"].get("chat")
if not chat:
    print("[alignment] chat profile is missing")
    sys.exit(1)
chat_components = set(chat["components"])
required_chat = {
    "shell",
    "localhost-provider",
    "did-provider",
    "chat",
    "crosvm",
    "vmlinux",
}
missing_chat = sorted(required_chat.difference(chat_components))
if missing_chat:
    print("[alignment] chat profile missing required microVM chat components:", ", ".join(missing_chat))
    sys.exit(1)
forbidden_chat = {"kubo", "ipfs-provider", "site-provider", "tunnel-provider", "cloudflared"}
bad_chat = sorted(forbidden_chat.intersection(chat_components))
if bad_chat:
    print("[alignment] chat profile includes non-chat transport/public components:", ", ".join(bad_chat))
    sys.exit(1)
blockchain = components["profiles"].get("blockchain")
if not blockchain:
    print("[alignment] blockchain profile is missing")
    sys.exit(1)
blockchain_components = set(blockchain["components"])
required_blockchain = {"shell", "localhost-provider", "did-provider", "chain-provider", "wallet-provider", "drm-provider", "rights-provider", "key-provider", "decrypt-provider"}
missing_blockchain = sorted(required_blockchain.difference(blockchain_components))
if missing_blockchain:
    print("[alignment] blockchain profile missing required components:", ", ".join(missing_blockchain))
    sys.exit(1)
webspace_component = components["external"].get("webspace-provider")
if not webspace_component:
    print("[alignment] webspace-provider is missing from external components")
    sys.exit(1)
for platform in ("linux-amd64", "linux-arm64"):
    info = (webspace_component.get("platforms") or {}).get(platform)
    if not info:
        print(f"[alignment] webspace-provider missing {platform} release metadata")
        sys.exit(1)
    if not info.get("release_path"):
        print(f"[alignment] webspace-provider missing {platform} release_path")
        sys.exit(1)
chain_component = components["external"].get("chain-provider")
if not chain_component:
    print("[alignment] chain-provider is missing from external components")
    sys.exit(1)
for platform in ("linux-amd64", "linux-arm64"):
    info = (chain_component.get("platforms") or {}).get(platform)
    if not info:
        print(f"[alignment] chain-provider missing {platform} release metadata")
        sys.exit(1)
    if not info.get("release_path"):
        print(f"[alignment] chain-provider missing {platform} release_path")
        sys.exit(1)
net_component = components["external"].get("net-provider")
if not net_component:
    print("[alignment] net-provider is missing from external components")
    sys.exit(1)
for platform in ("linux-amd64", "linux-arm64"):
    info = (net_component.get("platforms") or {}).get(platform)
    if not info:
        print(f"[alignment] net-provider missing {platform} release metadata")
        sys.exit(1)
    if not info.get("release_path"):
        print(f"[alignment] net-provider missing {platform} release_path")
        sys.exit(1)
exit_component = components["external"].get("exit-provider")
if not exit_component:
    print("[alignment] exit-provider is missing from external components")
    sys.exit(1)
for platform in ("linux-amd64", "linux-arm64"):
    info = (exit_component.get("platforms") or {}).get(platform)
    if not info:
        print(f"[alignment] exit-provider missing {platform} release metadata")
        sys.exit(1)
    if not info.get("release_path"):
        print(f"[alignment] exit-provider missing {platform} release_path")
        sys.exit(1)
browser_engine_component = components["external"].get("browser-engine-adapter")
if not browser_engine_component:
    print("[alignment] browser-engine-adapter is missing from external components")
    sys.exit(1)
for platform in ("linux-amd64", "linux-arm64"):
    info = (browser_engine_component.get("platforms") or {}).get(platform)
    if not info:
        print(f"[alignment] browser-engine-adapter missing {platform} release metadata")
        sys.exit(1)
    if not info.get("release_path"):
        print(f"[alignment] browser-engine-adapter missing {platform} release_path")
        sys.exit(1)
browser_engine_supervisor_component = components["external"].get("browser-engine-supervisor")
if not browser_engine_supervisor_component:
    print("[alignment] browser-engine-supervisor is missing from external components")
    sys.exit(1)
for platform in ("linux-amd64", "linux-arm64"):
    info = (browser_engine_supervisor_component.get("platforms") or {}).get(platform)
    if not info:
        print(f"[alignment] browser-engine-supervisor missing {platform} release metadata")
        sys.exit(1)
    if not info.get("release_path"):
        print(f"[alignment] browser-engine-supervisor missing {platform} release_path")
        sys.exit(1)
browser_stream_bridge_component = components["external"].get("browser-stream-bridge")
if not browser_stream_bridge_component:
    print("[alignment] browser-stream-bridge is missing from external components")
    sys.exit(1)
for platform in ("linux-amd64", "linux-arm64"):
    info = (browser_stream_bridge_component.get("platforms") or {}).get(platform)
    if not info:
        print(f"[alignment] browser-stream-bridge missing {platform} release metadata")
        sys.exit(1)
    if not info.get("release_path"):
        print(f"[alignment] browser-stream-bridge missing {platform} release_path")
        sys.exit(1)
browser_local_exit_component = components["external"].get("browser-local-exit")
if not browser_local_exit_component:
    print("[alignment] browser-local-exit is missing from external components")
    sys.exit(1)
for platform in ("linux-amd64", "linux-arm64"):
    info = (browser_local_exit_component.get("platforms") or {}).get(platform)
    if not info:
        print(f"[alignment] browser-local-exit missing {platform} release metadata")
        sys.exit(1)
    if not info.get("release_path"):
        print(f"[alignment] browser-local-exit missing {platform} release_path")
        sys.exit(1)
wallet_component = components["external"].get("wallet-provider")
if not wallet_component:
    print("[alignment] wallet-provider is missing from external components")
    sys.exit(1)
for platform in ("linux-amd64", "linux-arm64"):
    info = (wallet_component.get("platforms") or {}).get(platform)
    if not info:
        print(f"[alignment] wallet-provider missing {platform} release metadata")
        sys.exit(1)
    if not info.get("release_path"):
        print(f"[alignment] wallet-provider missing {platform} release_path")
        sys.exit(1)
drm_component = components["external"].get("drm-provider")
if not drm_component:
    print("[alignment] drm-provider is missing from external components")
    sys.exit(1)
for platform in ("linux-amd64", "linux-arm64"):
    info = (drm_component.get("platforms") or {}).get(platform)
    if not info:
        print(f"[alignment] drm-provider missing {platform} release metadata")
        sys.exit(1)
    if not info.get("release_path"):
        print(f"[alignment] drm-provider missing {platform} release_path")
        sys.exit(1)
rights_component = components["external"].get("rights-provider")
if not rights_component:
    print("[alignment] rights-provider is missing from external components")
    sys.exit(1)
for platform in ("linux-amd64", "linux-arm64"):
    info = (rights_component.get("platforms") or {}).get(platform)
    if not info:
        print(f"[alignment] rights-provider missing {platform} release metadata")
        sys.exit(1)
    if not info.get("release_path"):
        print(f"[alignment] rights-provider missing {platform} release_path")
        sys.exit(1)
key_component = components["external"].get("key-provider")
if not key_component:
    print("[alignment] key-provider is missing from external components")
    sys.exit(1)
for platform in ("linux-amd64", "linux-arm64"):
    info = (key_component.get("platforms") or {}).get(platform)
    if not info:
        print(f"[alignment] key-provider missing {platform} release metadata")
        sys.exit(1)
    if not info.get("release_path"):
        print(f"[alignment] key-provider missing {platform} release_path")
        sys.exit(1)
decrypt_component = components["external"].get("decrypt-provider")
if not decrypt_component:
    print("[alignment] decrypt-provider is missing from external components")
    sys.exit(1)
for platform in ("linux-amd64", "linux-arm64"):
    info = (decrypt_component.get("platforms") or {}).get(platform)
    if not info:
        print(f"[alignment] decrypt-provider missing {platform} release metadata")
        sys.exit(1)
    if not info.get("release_path"):
        print(f"[alignment] decrypt-provider missing {platform} release_path")
        sys.exit(1)
availability_component = components["external"].get("availability-provider")
if not availability_component:
    print("[alignment] availability-provider is missing from external components")
    sys.exit(1)
for platform in ("linux-amd64", "linux-arm64"):
    info = (availability_component.get("platforms") or {}).get(platform)
    if not info:
        print(f"[alignment] availability-provider missing {platform} release metadata")
        sys.exit(1)
    if not info.get("release_path"):
        print(f"[alignment] availability-provider missing {platform} release_path")
        sys.exit(1)
home_cli_component = components["external"].get("home-cli")
if not home_cli_component:
    print("[alignment] home-cli capsule is missing from external components")
    sys.exit(1)
for platform in ("linux-amd64", "linux-arm64"):
    info = platform_info(home_cli_component, platform)
    if not info:
        print(f"[alignment] home-cli capsule missing {platform} release metadata")
        sys.exit(1)
    if not info.get("release_path"):
        print(f"[alignment] home-cli capsule missing {platform} release_path")
        sys.exit(1)
for name in ("home", "system", "documents", "library", "marketplace", "inbox"):
    component = components["external"].get(name)
    if not component:
        print(f"[alignment] {name} capsule is missing from external components")
        sys.exit(1)
    for platform in ("linux-amd64", "linux-arm64"):
        info = platform_info(component, platform)
        if not info:
            print(f"[alignment] {name} capsule missing {platform} release metadata")
            sys.exit(1)
        if not info.get("release_path") or not info.get("extract_path"):
            print(f"[alignment] {name} capsule missing {platform} archive metadata")
            sys.exit(1)
PY

# ── Trusted-core freeze (ADR 0001 Phase 0) ───────────────────────────────────
# These elastos-server files hold app/service logic that ADR 0001 says belongs in
# capsules (content-market/availability, chat-room, library, documents). Until the
# extraction lands they are FROZEN: each may only SHRINK. The ceilings are a
# no-ballooning band — a little above today's size, so ordinary maintenance edits
# pass but a whole new service concern (hundreds of lines) fails. Policy: ratchet
# these numbers DOWN as ADR phases land; NEVER raise them. (A per-file freeze can be
# evaded by adding a new file — that is what the ADR + review catch; this gate stops
# the known offenders from growing.)
check_core_freeze() {
  local rel="$1" ceiling="$2"
  local path="elastos/crates/elastos-server/src/$rel"
  [[ -f "$path" ]] || return 0
  local lines
  lines=$(wc -l < "$path" | tr -d ' ')
  if [[ "$lines" -gt "$ceiling" ]]; then
    echo "[alignment] trusted-core freeze: $rel grew to $lines lines (ceiling $ceiling) — ADR 0001 Phase 0."
    echo "  This file holds app/service logic that belongs in its capsule, not the trusted core."
    echo "  Move logic out (shrink it), or — if this commit IS the extraction — lower the ceiling."
    failed=1
  fi
}
check_core_freeze content.rs 13200
check_core_freeze room_service.rs 5550
check_core_freeze documents.rs 1700
check_core_freeze library.rs 7300   # extra headroom: in-flight dDRM work touches this; tighten after it lands

if [[ "$failed" -ne 0 ]]; then
  echo "[alignment] FAILED"
  exit 1
fi

echo "[alignment] OK"
