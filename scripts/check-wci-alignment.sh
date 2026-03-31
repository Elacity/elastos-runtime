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
)

failed=0
tmp="$(mktemp)"
trap 'rm -f "$tmp"' EXIT

rg_search() {
  local pattern="$1"
  shift
  if command -v rg >/dev/null 2>&1; then
    rg -n "$pattern" "$@"
    return
  fi
  local grep_args=(-R -n -E)
  local paths=()
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --glob)
        shift
        case "${1:-}" in
          '!archive/**') grep_args+=(--exclude-dir=archive) ;;
          '!docs/ANTI_DRIFT.md') grep_args+=(--exclude=ANTI_DRIFT.md) ;;
          '!plans/**') grep_args+=(--exclude-dir=plans) ;;
          '!scripts/check-wci-alignment.sh') grep_args+=(--exclude=check-wci-alignment.sh) ;;
          '!target/**') grep_args+=(--exclude-dir=target) ;;
        esac
        ;;
      *)
        paths+=("$1")
        ;;
    esac
    shift || true
  done
  grep "${grep_args[@]}" -- "$pattern" "${paths[@]}"
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
check_forbidden 'setup --profile chat' 'legacy setup profile guidance'
check_forbidden 'Start runtime:[[:space:]]+elastos serve' 'legacy install banner runtime hint'
check_forbidden 'Using publisher gateway' 'setup/update should not present publisher gateway as default ElastOS transport'
check_forbidden 'Checking publisher gateway' 'update should not default to publisher gateway transport'
check_forbidden 'IPFS gateway:[[:space:]]+https://' 'share should not print a default public web gateway'
check_forbidden 'DEFAULT_GATEWAYS=\(' 'installer should not bake in public IPFS gateway defaults'
check_forbidden 'canonical user-facing transport' 'installer/docs should not teach web transport as the normal post-install model'
check_forbidden 'Contacts publisher gateway directly' 'command docs should not describe update as gateway-first'
check_forbidden 'alias /var/www/elastos/' 'nginx should not own published application objects directly'
check_forbidden 'proxy_pass http://127\.0\.0\.1:8081' 'public nginx edge should not route the canonical site through the preview site service'

check_required 'PC2 home' README.md 'README must teach PC2 home'
check_required 'No Ambient Authority' PRINCIPLES.md 'principles file must codify explicit authority boundaries'
check_required 'Carrier First Off-Box' PRINCIPLES.md 'principles file must codify Carrier-first off-box transport'
check_required 'audit-linux-runtime-portability\.sh' scripts/publish-release.sh 'publish release must audit Linux runtime portability before publishing'
check_forbidden_in_path 'using default: \$ELASTOS' scripts/publish-release.sh 'public Linux runtime publish must not silently fall back to the glibc host binary'
check_required 'PC2 home' docs/GETTING_STARTED.md 'Getting Started must teach PC2 home'
check_required 'Open PC2 home:' scripts/install.sh 'installer banner must teach PC2 home'
check_required 'localhost://UsersAI' docs/NAMESPACES.md 'namespace docs must teach UsersAI rooted localhost'
check_required 'localhost://ElastOS' docs/NAMESPACES.md 'namespace docs must teach ElastOS rooted localhost'
check_required 'SharedByLocalUsersAndBots' elastos/crates/elastos-server/src/pc2_cmd.rs 'pc2 session code must use the shared Local workspace path'
check_required 'route\("/release\.json"' elastos/crates/elastos-server/src/api/gateway.rs 'gateway must serve release.json'
check_required 'route\("/artifacts/\*path"' elastos/crates/elastos-server/src/api/gateway.rs 'gateway must serve published artifacts'
check_required 'X-Elastos-Site-Origin' elastos/crates/elastos-server/src/api/gateway.rs 'gateway must stamp public site responses with rooted origin'
check_required 'X-Elastos-Site-Head-Release' elastos/crates/elastos-server/src/api/gateway.rs 'gateway must expose active named site releases when present'
check_required 'X-Elastos-Site-Head-Channel' elastos/crates/elastos-server/src/api/gateway.rs 'gateway must expose active release channels when present'
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
check_required 'public-install-update-smoke\.sh' docs/RUNTIME_REPO_USER_STORY_CHECKLIST.md 'runtime repo checklist must record the stamped-install update proof'
check_required 'public-install-identity-smoke\.sh' docs/RUNTIME_REPO_USER_STORY_CHECKLIST.md 'runtime repo checklist must record the DID/profile proof contract'
check_required 'public-linux-runtime-portability-smoke\.sh' docs/RUNTIME_REPO_USER_STORY_CHECKLIST.md 'runtime repo checklist must record the public Linux runtime portability proof'
check_required 'just verify-release' docs/RUNTIME_REPO_USER_STORY_CHECKLIST.md 'runtime repo checklist must record the canonical release-trust gate'
check_required 'public-install-update-smoke\.sh' state.md 'state ledger must record the explicit stamped-install update proof'
check_required 'local-identity-profile-smoke\.sh|public-install-identity-smoke\.sh' state.md 'state ledger must record the DID/profile proof path'
check_required 'public-linux-runtime-portability-smoke\.sh' state.md 'state ledger must record the explicit public Linux runtime portability proof'
check_required 'public-install-update-smoke\.sh' TASKS.md 'tasks must keep the stamped-install update proof in scope'
check_required 'public-install-identity-smoke\.sh|DID-backed People/profile contract' TASKS.md 'tasks must keep the DID/profile public proof in scope'
check_required 'public-linux-runtime-portability-smoke\.sh' TASKS.md 'tasks must keep the public Linux runtime portability proof in scope'
check_required 'BindDomain' elastos/crates/elastos-server/src/main.rs 'site command surface must expose bind-domain'
check_required 'Publish' elastos/crates/elastos-server/src/main.rs 'site command surface must expose publish'
check_required 'Releases' elastos/crates/elastos-server/src/main.rs 'site command surface must expose releases'
check_required 'Channels' elastos/crates/elastos-server/src/main.rs 'site command surface must expose channels'
check_required 'Activate' elastos/crates/elastos-server/src/main.rs 'site command surface must expose activate'
check_required 'History' elastos/crates/elastos-server/src/main.rs 'site command surface must expose history'
check_required 'Rollback' elastos/crates/elastos-server/src/main.rs 'site command surface must expose rollback'
check_required 'Promote' elastos/crates/elastos-server/src/main.rs 'site command surface must expose promote'
check_required 'edge_binding_path' elastos/crates/elastos-server/src/api/gateway.rs 'gateway must resolve Host bindings through Edge state'
check_required 'edge_site_head_path' elastos/crates/elastos-server/src/api/gateway.rs 'gateway must resolve signed site-head state through Edge'
check_required 'publisher_site_release_path' elastos/crates/elastos-server/src/site_cmd.rs 'site command surface must persist named releases under Publisher state'
check_required 'edge_release_channel_path' elastos/crates/elastos-server/src/site_cmd.rs 'site command surface must persist release channels under Edge state'
check_required 'publisher_release_manifest_path' elastos/crates/elastos-server/src/api/gateway.rs 'gateway must read release manifests from Publisher state'
check_forbidden_in_path 'default chat profile' docs/GETTING_STARTED.md 'onboarding must teach the default PC2 profile, not the old chat profile'
check_forbidden_in_path 'darwin\)' scripts/install.sh 'public installer must stay Linux-only until update/install support macOS coherently'
check_forbidden_in_path 'http://' elastos/crates/elastos-runtime/src/provider/registry.rs 'provider-registry tests/docs must not preserve http:// parity assumptions'
check_forbidden_in_path 'localhost:// = ' README.md 'public docs must not flatten localhost:// into a single-root slogan'
check_forbidden_in_path 'localhost:// = ' docs/OVERVIEW.md 'overview must describe rooted localhost spaces, not a flattened single-root slogan'
check_forbidden_in_path 'did-provider' capsules/chat/capsule.json 'chat capsule should use the host did bridge instead of bundling a stale did-provider dependency'
check_forbidden_in_path 'component\.as_os_str\(\) == "target"' elastos/crates/elastos-server/src/binaries.rs 'provider resolution must not auto-enable repo asset lookup just because the binary runs from target/'
check_forbidden_in_path 'component\.as_os_str\(\) == "target"' elastos/crates/elastos-server/src/ipfs.rs 'viewer resolution must not auto-enable repo asset lookup just because the binary runs from target/'
check_forbidden_in_path 'Legacy TCP fallback' elastos/crates/elastos-server/src/vm_provider.rs 'vm provider bridge must not describe generic TCP fallback as a normal contract'
check_forbidden_in_path 'guest_from_fallback' elastos/crates/elastos-server/src/init.rs 'init should name guest dependency source explicitly instead of treating registry dependency as an unnamed fallback'
check_required 'managed dashboard runtime' docs/OVERVIEW.md 'overview must teach PC2 home as the front door'

python3 - <<'PY'
import json, sys
from pathlib import Path

components = json.loads(Path("components.json").read_text())
pc2 = components["profiles"]["pc2"]["components"]
forbidden = {"kubo", "ipfs-provider", "site-provider", "tunnel-provider", "md-viewer", "cloudflared"}
bad = sorted(forbidden.intersection(pc2))
if bad:
    print("[alignment] pc2 profile includes non-default off-box/public-edge components:", ", ".join(bad))
    sys.exit(1)
required = {"shell", "localhost-provider", "did-provider", "webspace-provider", "pc2"}
missing = sorted(required.difference(pc2))
if missing:
    print("[alignment] pc2 profile missing required first-party core components:", ", ".join(missing))
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
    "webspace-provider",
    "pc2",
    "kubo",
    "ipfs-provider",
    "site-provider",
    "tunnel-provider",
    "md-viewer",
    "cloudflared",
}
missing_demo = sorted(required_demo.difference(demo_components))
if missing_demo:
    print("[alignment] demo profile missing required demo components:", ", ".join(missing_demo))
    sys.exit(1)
irc = components["profiles"].get("irc")
if not irc:
    print("[alignment] irc profile is missing")
    sys.exit(1)
irc_components = set(irc["components"])
required_irc = {
    "shell",
    "localhost-provider",
    "did-provider",
    "chat",
    "crosvm",
    "vmlinux",
}
missing_irc = sorted(required_irc.difference(irc_components))
if missing_irc:
    print("[alignment] irc profile missing required IRC components:", ", ".join(missing_irc))
    sys.exit(1)
forbidden_irc = {"kubo", "ipfs-provider", "site-provider", "tunnel-provider", "cloudflared"}
bad_irc = sorted(forbidden_irc.intersection(irc_components))
if bad_irc:
    print("[alignment] irc profile includes non-IRC transport/public components:", ", ".join(bad_irc))
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
pc2_component = components["external"].get("pc2")
if not pc2_component:
    print("[alignment] pc2 home capsule is missing from external components")
    sys.exit(1)
for platform in ("linux-amd64", "linux-arm64"):
    info = (pc2_component.get("platforms") or {}).get(platform)
    if not info:
        print(f"[alignment] pc2 home capsule missing {platform} release metadata")
        sys.exit(1)
    if not info.get("release_path"):
        print(f"[alignment] pc2 home capsule missing {platform} release_path")
        sys.exit(1)
PY

if [[ "$failed" -ne 0 ]]; then
  echo "[alignment] FAILED"
  exit 1
fi

echo "[alignment] OK"
