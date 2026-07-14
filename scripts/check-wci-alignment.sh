#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

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
  --glob '!**/target/**'
  --glob '!**/node_modules/**'
)

failed=0
tmp="$(mktemp)"
trap 'rm -f "$tmp"' EXIT

rg_search() {
  local pattern="$1"
  shift
  if [[ "${ELASTOS_FORCE_GREP_FALLBACK:-0}" != "1" ]] && command -v rg >/dev/null 2>&1; then
    rg -n "$pattern" "$@"
    return
  fi
  local paths=()
  local globs=()
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --glob)
        shift
        globs+=("${1:-}")
        ;;
      *)
        paths+=("$1")
        ;;
    esac
    shift || true
  done
  local python_args=("$pattern")
  if [[ "${#paths[@]}" -gt 0 ]]; then
    python_args+=("${paths[@]}")
  fi
  python_args+=(--)
  if [[ "${#globs[@]}" -gt 0 ]]; then
    python_args+=("${globs[@]}")
  fi
  python3 - "${python_args[@]}" <<'PY'
import fnmatch
import os
import re
import sys
from pathlib import Path

pattern = sys.argv[1]
args = sys.argv[2:]
try:
    split = args.index("--")
except ValueError:
    split = len(args)
paths = args[:split] or ["."]
globs = args[split + 1:]
includes = [glob for glob in globs if glob and not glob.startswith("!")]
excludes = [
    "target/**",
    "**/target/**",
    "node_modules/**",
    "**/node_modules/**",
    ".git/**",
    "**/.git/**",
]
excludes.extend(glob[1:] for glob in globs if glob.startswith("!"))
cwd = Path.cwd()

for source, replacement in {
    "[:space:]": r"\s",
    "[:digit:]": r"\d",
    "[:alnum:]": r"A-Za-z0-9",
    "[:alpha:]": r"A-Za-z",
}.items():
    pattern = pattern.replace(source, replacement)
regex = re.compile(pattern)

def rel_name(path):
    try:
        return path.resolve().relative_to(cwd.resolve()).as_posix()
    except ValueError:
        return path.as_posix()

def matches(pattern, rel):
    return (
        fnmatch.fnmatch(rel, pattern)
        or fnmatch.fnmatch(f"./{rel}", pattern)
        or fnmatch.fnmatch(Path(rel).name, pattern)
    )

def accepted(path):
    rel = rel_name(path)
    if includes and not any(matches(pattern, rel) for pattern in includes):
        return False
    if any(matches(pattern, rel) for pattern in excludes):
        return False
    return True

def excluded_dir(path):
    rel = rel_name(path)
    probe = f"{rel}/__elastos_probe__"
    return any(matches(pattern, rel) or matches(pattern, probe) for pattern in excludes)

def iter_files(root):
    if root.is_file():
        yield root
        return
    for dirpath, dirnames, filenames in os.walk(root):
        current = Path(dirpath)
        dirnames[:] = [
            dirname for dirname in dirnames
            if dirname not in {"target", "node_modules", ".git"}
            and not excluded_dir(current / dirname)
        ]
        for filename in filenames:
            yield current / filename

seen = set()
found = False
for raw in paths:
    root = Path(raw)
    if not root.exists():
        continue
    for path in iter_files(root):
        rel = rel_name(path)
        if rel in seen or not accepted(path):
            continue
        seen.add(rel)
        try:
            with path.open("rb") as handle:
                sample = handle.read(8192)
                if b"\0" in sample:
                    continue
                handle.seek(0)
                for line_number, raw_line in enumerate(handle, 1):
                    line = raw_line.decode("utf-8", errors="ignore")
                    if regex.search(line):
                        print(f"{rel}:{line_number}:{line.rstrip()}")
                        found = True
        except OSError:
            continue
sys.exit(0 if found else 1)
PY
}

check_forbidden() {
  local pattern="$1"
  local label="$2"
  local search_status=0
  rg_search "$pattern" "${scope[@]}" "${exclude_globs[@]}" >"$tmp" 2>/dev/null || search_status=$?
  if [[ "$search_status" -eq 0 ]]; then
    echo "[alignment] forbidden pattern found: $label"
    cat "$tmp"
    echo
    failed=1
  fi
  return 0
}

check_required() {
  local pattern="$1"
  local path="$2"
  local label="$3"
  local search_status=0
  rg_search "$pattern" "$path" >"$tmp" 2>/dev/null || search_status=$?
  if [[ "$search_status" -ne 0 ]]; then
    echo "[alignment] required pattern missing: $label"
    echo "  file: $path"
    echo
    failed=1
  fi
  return 0
}

check_forbidden_in_path() {
  local pattern="$1"
  local path="$2"
  local label="$3"
  local search_status=0
  rg_search "$pattern" "$path" >"$tmp" 2>/dev/null || search_status=$?
  if [[ "$search_status" -eq 0 ]]; then
    echo "[alignment] forbidden pattern found: $label"
    cat "$tmp"
    echo
    failed=1
  fi
  return 0
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
check_required '"private": true' elastos/esp/package.json 'ESP type package must remain private'
check_required 'ESP_TRANSPORT_SCOPE = "local_runtime_adapter"' elastos/esp/esp_v0.ts 'ESP type package must mark HTTP as local adapter scope'
check_required 'elastos\.inspect\.gate-preview/v1' elastos/esp/esp_v0.ts 'ESP type package must include current Inspect gate preview schema'
check_required 'elastos\.esp\.request-binding/v1' elastos/esp/esp_v0.ts 'ESP type package must include current request binding schema'
check_required 'elastos\.inspect\.dispatch-result/v1' elastos/esp/esp_v0.ts 'ESP type package must include current Inspect dispatch result schema'
check_required 'ESP_FACT_DESCRIPTORS' elastos/esp/esp_v0.ts 'ESP type package must expose current fact descriptor identities'
check_required 'ESP_VERB_DESCRIPTORS' elastos/esp/esp_v0.ts 'ESP type package must expose current verb descriptor identities'
check_required 'parseDocSupportedSchemas' elastos/esp/check-esp-v0.mjs 'ESP package check must compare docs supported schemas to the served descriptor'
check_required 'parseDocProjectionFacts' elastos/esp/check-esp-v0.mjs 'ESP package check must compare docs projection facts to the served descriptor'
check_required 'parseDocVerbTable' elastos/esp/check-esp-v0.mjs 'ESP package check must compare docs verb table to the served descriptor'
check_required 'gateway\.rs must route' elastos/esp/check-esp-v0.mjs 'ESP package check must prove descriptor routes are wired in the gateway'
check_required 'Standing grants are not implemented or exposed by ESP v0' docs/ESP_V0.md 'ESP docs must not claim standing grants'
check_required 'Reach enforcement and reach halos are not implemented by ESP v0' docs/ESP_V0.md 'ESP docs must not claim reach enforcement'
check_required 'SSE ESP projection streams are not product-ready' docs/ESP_V0.md 'ESP docs must not claim SSE projection streams'
check_required 'Shell marketplace is not implemented' docs/ESP_V0.md 'ESP docs must not claim shell marketplace'
check_required 'Full second-shell product UX is not complete' docs/ESP_V0.md 'ESP docs must not claim full second-shell product UX'
check_forbidden_in_path 'affordance-consent-pending|elastos\.reach|ReachFact|AffordanceGrantReceipt|RequestCapabilityInput|ValidateAndConsume|validate-and-consume|standing grant|shell marketplace|EventSource|SSE|projection stream|full second-shell|fetch\(' elastos/esp/esp_v0.ts 'ESP type package must not import unsupported Flint authority or future fact surfaces'
check_required 'trustMaterial' elastos/esp/trust.ts 'ESP projection helpers must include trust projection'
check_required 'custodyView' elastos/esp/custody.ts 'ESP projection helpers must include custody projection'
check_required 'shellPicker' elastos/esp/shell_picker.ts 'ESP projection helpers must include shell picker projection'
check_required 'inspectActionRequestValidation' elastos/esp/consent.ts 'ESP projection helpers must include current consent validation'
check_required 'capsuleDetailView' elastos/esp/capsule_detail.ts 'ESP projection helpers must include capsule detail projection'
check_required 'homeFleetView' elastos/esp/home_fleet.ts 'ESP projection helpers must include Home fleet projection'
check_required 'auditCountsView' elastos/esp/audit_views.ts 'ESP projection helpers must include audit view projection'
check_required 'plain ES modules or native Web Components' elastos/esp/README.md 'ESP README must keep the no-framework-first visual UI rule'
check_required 'Svelte may be used later only as an optional capsule-local UI compiler' elastos/esp/README.md 'ESP README must keep Svelte optional and capsule-local'
check_forbidden_in_path '"capsule-inspector"' components.json 'standalone capsule-inspector must not be packaged before the optional extraction decision'
check_forbidden_in_path '"svelte"|@sveltejs|vite|rollup' elastos/esp/package.json 'headless ESP package must not carry UI framework/compiler dependencies'
check_forbidden_in_path '\.svelte|svelte/compiler|svelte/server|from "svelte|from '\''svelte' elastos/esp 'headless ESP package must not contain framework UI sources or render tests'
check_forbidden_in_path 'spend_audit|HomeCustodyView|spendBudgetView|auditChainView|intentProofView' elastos/esp 'headless ESP must keep current custody/audit projection canonical instead of duplicating Flint spend-audit logic'
check_forbidden_in_path 'spend_audit|HomeCustodyView|spendBudgetView|auditChainView|intentProofView|dispatch_approved' capsules/capsule-inspector 'standalone capsule-inspector must not vendor spend-audit logic or direct dispatch authority'
check_forbidden_in_path 'fetch\(|XMLHttpRequest|WebSocket|localStorage|sessionStorage|indexedDB|crypto\.|privateKey|secret|home_token|dispatch_approved|invoke_provider|send_raw|ProviderRegistry' elastos/esp/audit_views.ts 'ESP audit view helper must stay pure'
check_forbidden_in_path 'fetch\(|XMLHttpRequest|WebSocket|localStorage|sessionStorage|indexedDB|crypto\.|privateKey|secret|home_token|dispatch_approved|invoke_provider|send_raw|ProviderRegistry' elastos/esp/capsule_detail.ts 'ESP capsule detail helper must stay pure'
check_forbidden_in_path 'fetch\(|XMLHttpRequest|WebSocket|localStorage|sessionStorage|indexedDB|crypto\.|privateKey|secret|home_token|dispatch_approved|invoke_provider|send_raw|ProviderRegistry' elastos/esp/consent.ts 'ESP consent helper must stay pure'
check_forbidden_in_path 'fetch\(|XMLHttpRequest|WebSocket|localStorage|sessionStorage|indexedDB|crypto\.|privateKey|secret|home_token|dispatch_approved|invoke_provider|send_raw|ProviderRegistry' elastos/esp/custody.ts 'ESP custody helper must stay pure'
check_forbidden_in_path 'fetch\(|XMLHttpRequest|WebSocket|localStorage|sessionStorage|indexedDB|crypto\.|privateKey|secret|home_token|dispatch_approved|invoke_provider|send_raw|ProviderRegistry' elastos/esp/home_fleet.ts 'ESP Home fleet helper must stay pure'
check_forbidden_in_path 'fetch\(|XMLHttpRequest|WebSocket|localStorage|sessionStorage|indexedDB|crypto\.|privateKey|secret|home_token|dispatch_approved|invoke_provider|send_raw|ProviderRegistry' elastos/esp/shell_picker.ts 'ESP shell picker helper must stay pure'
check_forbidden_in_path 'fetch\(|XMLHttpRequest|WebSocket|localStorage|sessionStorage|indexedDB|crypto\.|privateKey|secret|home_token|dispatch_approved|invoke_provider|send_raw|ProviderRegistry' elastos/esp/trust.ts 'ESP trust helper must stay pure'
check_required '"role": "shell"' capsules/home-cli/capsule.json 'Home CLI capsule must remain a shell-role capsule'
check_required 'home-cli\.js' capsules/home-cli/browser/index.html 'Home CLI browser shell must load its terminal surface'
check_required 'id="xterm-terminal"' capsules/home-cli/browser/index.html 'Home CLI browser shell must mount an xterm terminal'
check_required '/api/apps/home-cli/terminal/sessions' capsules/home-cli/browser/home-cli.js 'Home CLI browser shell must attach a Runtime-owned terminal session'
check_required 'elastos\.home-cli\.terminal-start/v1' capsules/home-cli/browser/home-cli.js 'Home CLI browser shell must use the typed terminal start schema'
check_required 'EventSource' capsules/home-cli/browser/home-cli.js 'Home CLI browser shell must receive terminal events from Runtime'
check_required 'queueRuntimeTerminalInput' capsules/home-cli/browser/home-cli.js 'Home CLI browser shell must send raw terminal input through Runtime'
check_required 'resizeRuntimeTerminal' capsules/home-cli/browser/home-cli.js 'Home CLI browser shell must report terminal resize through Runtime'
check_required 'elastos\.home\.terminal-host-intent/v1' capsules/home-cli/browser/home-cli.js 'Home CLI browser shell must forward Home CLI TUI host intents without direct app launch'
check_required 'home:open-target' capsules/home-cli/browser/home-cli.js 'Home CLI must ask Home to open visible capsules through the signed Home message channel'
check_required 'print_cli_inspect' capsules/home-cli/src/line_views.rs 'Home CLI must render Runtime-derived capsule projection facts'
check_required 'print_cli_contract' capsules/home-cli/src/line_views.rs 'Home CLI must render the shared capsule interface contract'
check_required 'home-shell-boot-mask' capsules/home/browser/index.html 'Home shell host must neutral-mask first paint until Runtime selects the active shell'
check_required 'print_cli_gates' capsules/home-cli/src/line_views.rs 'Home CLI must render Runtime gate facts'
check_required 'print_cli_affordances' capsules/home-cli/src/line_views.rs 'Home CLI must render capsule affordance facts'
check_required 'print_cli_wallet' capsules/home-cli/src/line_views.rs 'Home CLI must render wallet approval hints from Runtime facts'
check_required 'print_cli_browser' capsules/home-cli/src/line_views.rs 'Home CLI must render Browser Engine and Exit facts'
check_forbidden_in_path 'ProviderRegistry|dispatch_approved|/api/provider/|/api/apps/system|window\.ethereum|personal_sign|eth_requestAccounts|localStorage|sessionStorage|indexedDB' capsules/home-cli/browser 'Home CLI browser shell must stay a pure Runtime terminal client'
check_required 'elastos\.capsule\.projection/v1' elastos/crates/elastos-server/src/api/gateway_capsule_catalog/read_model.rs 'Runtime catalog must derive shared capsule projection facts for shells'
check_required 'elastos\.capsule\.projection/v1' docs/CAPSULE_INTERFACE_CONTRACT.md 'Capsule interface contract must document the derived projection facts'
check_required 'active-shell-options' capsules/system/browser/index.html 'System Settings must expose active shell selection'
check_required '/api/apps/home/active-shell' capsules/system/browser/system.js 'System Settings must use the Runtime-owned active-shell route'
check_required 'home:refresh-summary' capsules/system/browser/system.js 'System shell setting must ask Home to swap the root shell after changes'
check_forbidden_in_path 'esp-shell' components.json 'obsolete ESP Shell capsule must not be packaged'

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
check_required 'wallet-metamask' capsules/home/browser/home-shell-host.js 'System must be able to open the dedicated MetaMask connector instead of signing in place'
check_required 'wallet-unisat' capsules/home/browser/home-shell-host.js 'Wallet must be able to open the dedicated UniSat connector instead of signing Bitcoin proofs in place'
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
check_required 'presentation: "prompt"' capsules/home/browser/home-shell-host.js 'unsigned Home must encourage sign-in without blocking the standard desktop'
check_required '/api/auth/sessions/refresh' capsules/home/browser/shell-auth.js 'Home must refresh proof-bound browser sessions through runtime auth'
check_required 'home state save failed' capsules/home-gui/browser/shell-core.js 'Home GUI browser state writes must stay explicit and observable'
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
check_required 'max_active_streams_per_principal' capsules/exit-provider/src/main.rs 'remote Carrier Exit grants must support per-principal stream quotas'
check_required 'browser_reserve_stream_session' elastos/crates/elastos-server/src/api/gateway_browser_stream.rs 'Browser streams must route Browser -> Net validation -> internal Exit handoff'
check_required 'remote_exit_id' elastos/crates/elastos-server/src/api/gateway_browser_stream.rs 'Browser stream reservation must preserve selected remote Exit identity'
check_required 'stream_nonce' elastos/crates/elastos-server/src/api/gateway_browser_stream.rs 'Browser stream reservation must preserve per-open stream nonce'
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
check_required 'read_browser_relay_open_line' elastos/crates/elastos-server/src/api/gateway_browser_stream.rs 'Browser runtime stream relay must forward bounded typed Exit relay-open handshakes before forwarding bytes'
check_required 'BROWSER_RUNTIME_RELAY_OPEN_MAX_BYTES' elastos/crates/elastos-server/src/api/gateway_browser_stream.rs 'Browser runtime stream relay must bound relay-open handshakes before forwarding bytes'
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
  --glob '!capsules/home/**' \
  --glob '!capsules/system/browser/*' \
  --glob '!capsules/inbox/browser/*' \
  --glob '!capsules/wallet/*' \
  --glob '!capsules/*-provider/**' \
  --glob '!**/target/**' >"$tmp" 2>/dev/null; then
  echo "[alignment] forbidden pattern found: passkey ceremonies must stay in Home/System/Inbox/Wallet/runtime auth surfaces"
  cat "$tmp"
  echo
  failed=1
fi
check_forbidden_in_path 'credentials\.create|passkey/register|webauthn/register' capsules/inbox 'Inbox may request fresh passkey authentication for wallet or Inspector approval, but must not register passkeys'
check_forbidden_in_path 'credentials\.create|passkey/register|webauthn/register' capsules/wallet 'Wallet may request fresh passkey authentication for protected recovery, but must not register passkeys'
if rg_search $'fetch[[:space:]]*\\([[:space:]]*["\\\'`]https?://|\\.open[[:space:]]*\\([[:space:]]*["\\\'`][A-Z]+["\\\'`][[:space:]]*,[[:space:]]*["\\\'`]https?://|new[[:space:]]+WebSocket[[:space:]]*\\([[:space:]]*["\\\'`]wss?://|new[[:space:]]+EventSource[[:space:]]*\\([[:space:]]*["\\\'`]https?://|sendBeacon[[:space:]]*\\([[:space:]]*["\\\'`]https?://' capsules \
  --glob '*/browser/**' \
  --glob '!capsules/*-provider/**' \
  --glob '!**/target/**' >"$tmp" 2>/dev/null; then
  echo "[alignment] forbidden pattern found: app capsules must not open absolute external network URLs directly"
  cat "$tmp"
  echo
  failed=1
fi
if rg_search 'WalletConnect|walletconnect|MetaMask|metamask|UniSat|unisat|window\.ethereum|window\.unisat|ethereum\.request|personal_sign|eth_requestAccounts|eth_sendTransaction|wallet_switchEthereumChain|signMessage' capsules \
  --glob '!capsules/home/browser/*' \
  --glob '!capsules/home-gui/browser/*' \
  --glob '!capsules/system/browser/*' \
  --glob '!capsules/browser/*' \
  --glob '!capsules/wallet-metamask/*' \
  --glob '!capsules/wallet-unisat/*' \
  --glob '!capsules/wallet/*' \
  --glob '!capsules/wallet-walletconnect/*' \
  --glob '!capsules/*-provider/**' \
  --glob '!elastos/capsules/*-provider/**' \
  --glob '!**/capsule.json' \
  --glob '!**/tests/**' \
  --glob '!**/*test*' \
  --glob '!**/*spec*' \
  --glob '!**/target/**' >"$tmp" 2>/dev/null; then
  echo "[alignment] forbidden pattern found: app capsules must not touch browser wallet authority directly"
  cat "$tmp"
  echo
  failed=1
fi
if rg_search 'elastos://chain|/api/provider/chain|chain-provider|blockchain provider|rpc_url|RPC_URL|JSON-RPC|jsonrpc|eth_call|eth_chainId|bitcoin-cli|bitcoind|Bitcoin Core RPC' capsules \
  --glob '!capsules/*-provider/**' \
  --glob '!elastos/capsules/*-provider/**' \
  --glob '!capsules/system/browser/*' \
  --glob '!capsules/wallet-metamask/*' \
  --glob '!capsules/wallet-unisat/*' \
  --glob '!capsules/wallet/*' \
  --glob '!capsules/wallet-walletconnect/*' \
  --glob '!**/capsule.json' \
  --glob '!**/tests/**' \
  --glob '!**/*test*' \
  --glob '!**/*spec*' \
  --glob '!**/target/**' >"$tmp" 2>/dev/null; then
  echo "[alignment] forbidden pattern found: app capsules must not touch raw chain/node authority directly"
  cat "$tmp"
  echo
  failed=1
fi
if rg_search 'elastos://wallet|/api/provider/wallet|wallet-provider' capsules \
  --glob '!capsules/system/browser/*' \
  --glob '!capsules/browser/*' \
  --glob '!capsules/*-provider/**' \
  --glob '!elastos/capsules/*-provider/**' \
  --glob '!**/capsule.json' \
  --glob '!**/tests/**' \
  --glob '!**/*test*' \
  --glob '!**/*spec*' \
  --glob '!**/target/**' >"$tmp" 2>/dev/null; then
  echo "[alignment] forbidden pattern found: app capsules must not reference raw wallet provider authority directly"
  cat "$tmp"
  echo
  failed=1
fi
check_forbidden_in_path 'home_session_cookie_header|home_session_cookie_is_valid|SET_COOKIE' elastos/crates/elastos-server/src/api/browser_capsules.rs 'Home static route must not auto-mint a local session cookie'
check_forbidden_in_path 'default chat profile' docs/GETTING_STARTED.md 'onboarding must teach the default Home profile, not the old chat profile'
check_forbidden_in_path 'darwin\)' scripts/install.sh 'public installer must stay Linux-only until update/install support macOS coherently'
check_required 'Current public install preview: Linux x86_64/aarch64' scripts/install.sh 'installer help must label public install as Linux preview'
check_required 'Current public install preview is Linux-only' scripts/install.sh 'installer must fail cleanly on non-Linux hosts'
check_required 'if \[\[ \$\{#GATEWAYS\[@\]\} -gt 0 \]\]; then' scripts/install.sh 'installer must safely prepend publisher gateway without expanding an empty Bash array'
check_required 'current Linux `x86_64`/`aarch64` preview' README.md 'README install path must be scoped to Linux preview'
check_required 'current Linux `x86_64`/`aarch64` preview' docs/INSTALL.md 'install docs must scope public installer to Linux preview'
check_required 'current Linux `x86_64`/`aarch64`' docs/GETTING_STARTED.md 'getting started must scope binary install to Linux preview'
check_required 'System, People, Services, Browser, Wallet, Documents, Library,' docs/INSTALL.md 'install docs must list the current default Home visible surfaces'
check_required 'Marketplace, Archive, and Inbox' docs/INSTALL.md 'install docs must list Marketplace and Archive in the default Home visible surfaces'
check_required 'People is Home-owned state and UI' docs/INSTALL.md 'install docs must not describe People as a separate installed capsule'
check_required 'separate capsule' docs/INSTALL.md 'install docs must explicitly distinguish People from installed capsules'
check_required 'System, People, Services, Browser, Wallet, Documents, Library, Marketplace,' docs/GETTING_STARTED.md 'getting started must list the current default Home visible surfaces'
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
import json, re, sys
from pathlib import Path

components = json.loads(Path("components.json").read_text())

allowed_roles = {"shell", "app", "viewer", "provider", "content"}
ordinary_roles = {"app", "viewer", "content"}
# System is the runtime-owned approval/diagnostic surface. Dedicated wallet
# connector capsules and the Browser shell are privileged adapter UIs, not
# general app authority.
ordinary_capsules_with_privileged_authority_ui = {"home", "system", "wallet-metamask", "wallet-unisat", "wallet", "wallet-walletconnect", "browser", "library"}
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

def is_supported_resource_scheme(resource):
    return isinstance(resource, str) and (
        resource.startswith("elastos://") or resource.startswith("localhost://")
    )

def require_supported_resource_scheme(path, label, resource):
    if not is_supported_resource_scheme(resource):
        print(f"[alignment] {path} {label} uses unsupported resource scheme: {resource}")
        sys.exit(1)

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

direct_external_network_patterns = [
    (
        re.compile(r"""fetch\s*\(\s*["'`]https?://"""),
        "absolute external fetch",
    ),
    (
        re.compile(r"""\.open\s*\(\s*["'`][A-Z]+["'`]\s*,\s*["'`]https?://"""),
        "absolute external XMLHttpRequest",
    ),
    (
        re.compile(r"""new\s+WebSocket\s*\(\s*["'`]wss?://"""),
        "absolute external WebSocket",
    ),
    (
        re.compile(r"""new\s+EventSource\s*\(\s*["'`]https?://"""),
        "absolute external EventSource",
    ),
    (
        re.compile(r"""sendBeacon\s*\(\s*["'`]https?://"""),
        "absolute external beacon",
    ),
]

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
    if provides:
        require_supported_resource_scheme(path, "provides namespace", provides)
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
        for index, capability in enumerate(authority.get("capabilities") or []):
            if isinstance(capability, dict):
                require_supported_resource_scheme(
                    path,
                    f"authority capability {index} resource",
                    capability.get("resource"),
                )
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
            "chain-provider": "raw chain backend provider",
            "net-provider": "raw Browser/Net backend provider",
            "exit-provider": "raw Browser Exit backend provider",
            "browser-engine-adapter": "raw Browser Engine backend adapter",
            "browser-engine-supervisor": "raw Browser Engine host supervisor",
            "browser-stream-bridge": "raw Browser Engine byte transport bridge",
            "browser-local-exit": "raw Browser local Exit daemon",
            "wallet-provider": "raw wallet backend provider",
            "object-provider": "raw object backend provider",
            "ipfs-provider": "raw IPFS backend provider",
            "availability-provider": "raw availability backend provider",
            "drm-provider": "raw protected-content backend provider",
            "rights-provider": "raw protected-content rights backend provider",
            "key-provider": "raw protected-content key backend provider",
            "decrypt-provider": "raw protected-content decrypt/render backend provider",
            "/api/provider/chain": "direct chain provider route",
            "/api/provider/net": "direct Browser/Net provider route",
            "/api/provider/exit": "direct Browser Exit provider route",
            "/api/provider/browser-engine": "direct Browser Engine provider route",
            "/api/provider/wallet": "direct wallet provider route",
            "ipfs-cluster": "raw IPFS Cluster backend",
            "elacity-sdk": "raw Elacity SDK backend",
            "elacity": "raw Elacity backend",
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
                text = source.read_text(errors="ignore")
                for pattern, reason in direct_external_network_patterns:
                    if pattern.search(text):
                        print(f"[alignment] ordinary capsule {manifest.get('name', path)} opens direct external network in {source}: {reason}")
                        sys.exit(1)
                for pattern, reason in forbidden_source_patterns.items():
                    if pattern in text:
                        print(f"[alignment] ordinary capsule {manifest.get('name', path)} leaks host topology in {source}: {reason}")
                        sys.exit(1)

def platform_info(component, platform):
    platforms = component.get("platforms") or {}
    return platforms.get(platform) or platforms.get("*")

home = components["profiles"]["home"]["components"]
forbidden = {"kubo", "ipfs-provider", "availability-provider", "site-provider", "tunnel-provider", "cloudflared", "drm-provider", "rights-provider", "key-provider", "decrypt-provider"}
bad = sorted(forbidden.intersection(home))
if bad:
    print("[alignment] home profile includes non-default off-box/public-edge/protected-content components:", ", ".join(bad))
    sys.exit(1)
wallet_browser_surfaces = {"wallet", "wallet-metamask", "wallet-unisat", "wallet-walletconnect", "browser", "inbox"}
wallet_browser_providers = {"chain-provider", "wallet-provider"}
for profile_name, profile in sorted(components["profiles"].items()):
    profile_components = set(profile.get("components") or [])
    if wallet_browser_surfaces.intersection(profile_components):
        missing_providers = sorted(wallet_browser_providers.difference(profile_components))
        if missing_providers:
            print(f"[alignment] {profile_name} profile installs Wallet/Browser surfaces without providers: {', '.join(missing_providers)}")
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
def shell_array_items(text, name):
    match = re.search(rf"^{re.escape(name)}=\((.*?)^\)", text, re.MULTILINE | re.DOTALL)
    if not match:
        print(f"[alignment] missing shell array {name}")
        sys.exit(1)
    return set(re.findall(r"^\s*([A-Za-z0-9_-]+)\s*$", match.group(1), re.MULTILINE))

def rust_const_items(text, name):
    match = re.search(rf"const\s+{re.escape(name)}:\s*&\[\&str\]\s*=\s*&\[(.*?)\];", text, re.DOTALL)
    if not match:
        print(f"[alignment] missing Rust const {name}")
        sys.exit(1)
    return set(re.findall(r'"([^"]+)"', match.group(1)))

publish_release = Path("scripts/publish-release.sh").read_text()
publish_rs = Path("elastos/crates/elastos-server/src/publish.rs").read_text()
if "components-release-integrity-check.py" not in publish_release or "validate_generated_components_json" not in publish_release:
    print("[alignment] publish-release must validate generated components.json release checksums")
    sys.exit(1)
publish_release_default = shell_array_items(publish_release, "DEFAULT_CAPSULES")
publish_release_required = shell_array_items(publish_release, "REQUIRED_SUPPORTED_CAPSULES")
publish_rust_home = rust_const_items(publish_rs, "HOME_PUBLISH_CAPSULES")
publish_rust_demo = rust_const_items(publish_rs, "DEMO_PUBLISH_CAPSULES")
publish_rust_required = rust_const_items(publish_rs, "REQUIRED_SUPPORTED_PUBLISH_CAPSULES")
home_profile_capsules = {
    name for name in components["profiles"]["home"]["components"]
    if name in components.get("external", {}) and Path("capsules", name, "capsule.json").exists()
}
legacy_capsules = {
    name for name in components["profiles"]["home"]["components"]
    if name in components.get("external", {}) and Path("elastos/capsules", name, "capsule.json").exists()
}
home_profile_capsules |= legacy_capsules
if publish_release_default != home_profile_capsules:
    print("[alignment] publish-release default capsules must match setup home capsule surface")
    print("[alignment] missing from publish-release:", ", ".join(sorted(home_profile_capsules - publish_release_default)) or "(none)")
    print("[alignment] extra in publish-release:", ", ".join(sorted(publish_release_default - home_profile_capsules)) or "(none)")
    sys.exit(1)
if publish_release_required != home_profile_capsules:
    print("[alignment] publish-release required capsules must match setup home capsule surface")
    print("[alignment] missing from required:", ", ".join(sorted(home_profile_capsules - publish_release_required)) or "(none)")
    print("[alignment] extra in required:", ", ".join(sorted(publish_release_required - home_profile_capsules)) or "(none)")
    sys.exit(1)
if publish_rust_home != home_profile_capsules or publish_rust_required != home_profile_capsules:
    print("[alignment] Rust home/required publish capsules must match setup home capsule surface")
    print("[alignment] missing from Rust home:", ", ".join(sorted(home_profile_capsules - publish_rust_home)) or "(none)")
    print("[alignment] extra in Rust home:", ", ".join(sorted(publish_rust_home - home_profile_capsules)) or "(none)")
    sys.exit(1)
for demo_capsule in ["chat", "gba-emulator", "gba-ucity", "chat-room", "ipfs-provider", "tunnel-provider"]:
    if demo_capsule not in publish_rust_demo:
        print(f"[alignment] Rust demo publish profile missing {demo_capsule}")
        sys.exit(1)
for provider in sorted(wallet_browser_providers):
    if provider not in publish_release_default:
        print(f"[alignment] publish-release default capsule set missing {provider}")
        sys.exit(1)
    if provider not in publish_release_required:
        print(f"[alignment] publish-release required supported capsule set missing {provider}")
        sys.exit(1)
    if provider not in publish_rust_home:
        print(f"[alignment] Rust home publish capsule set missing {provider}")
        sys.exit(1)
    if provider not in publish_rust_required:
        print(f"[alignment] Rust required supported publish capsule set missing {provider}")
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
for platform in ("linux-amd64", "linux-arm64", "darwin-arm64"):
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

if [[ "$failed" -ne 0 ]]; then
  echo "[alignment] FAILED"
  exit 1
fi

echo "[alignment] OK"
