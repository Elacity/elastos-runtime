#!/usr/bin/env bash
# ELACITY-2298 installed e2e proof: client composition ceremony + preflight.
#
# This drives the CLIENT side of the protected-content installed proof: the
# offline "composition ceremony" (`elastos protected-content-config ...`)
# against the three real custody-host nodes brought up by
# deploy/custody-host/up.sh, an installed-artifact client restart, and a
# preflight block proving the client can reach all three nodes over Carrier
# before the mint/journey phases (added by a later task) drive the real
# custody-plane crossing.
#
# Phases (invoke one per run; `--phase <name>` selects the case arm below --
# adding a later phase is one more case arm + one more function, nothing
# else changes):
#   provision         - offline ceremony: policy authority key, generate-
#                        custody-composition against the 3 live descriptors,
#                        generate-chain-config (placeholder RPCs), verify-
#                        custody-composition, verify/repair the client's
#                        installed-artifact state, restart the client
#                        runtime, run the --role home static audit.
#   preflight          - register the 3 custody peers (idempotent), assert 3
#                        peer entries, prove a transport-level Carrier dial
#                        per node, cross-check the 3 live containers'
#                        readiness receipts, re-verify the composition, and
#                        record preflight_ok.
#   chain-config-real  - regenerate chain-provider.json with the ceremony's
#                        PROVEN Base defaults (authority gateway
#                        0x09dBe796f40ECEffEAccf243c3d758C4c1d8D87D, selector
#                        0x54d42821) plus operator-supplied REAL RPC URLs
#                        (--real-rpc-url once, --real-evidence-rpc-url 2..=5x
#                        distinct origins, validated client-side before the
#                        ceremony runs), replacing provision's loopback
#                        placeholders. Re-run provision (or restart the
#                        client) after this so the running client picks up
#                        the new config.
#   wallet-setup       - creates/imports a managed wallet account over HTTP
#                        for the creator and buyer principals (each needs a
#                        pre-obtained signed Home session -- see "Home-token
#                        login sequence" below), waits for the operator's
#                        one-time funding transfer (polled via the chain
#                        evidence RPCs), then sets each principal's default
#                        eip155 transaction-intent account so the mint/buy
#                        flow can sign.
#   mint               - creator: publish the --content-path file with
#                        protection.mode=runtime_custody, poll and approve
#                        the resulting wallet approval, assert the mint
#                        journal reaches a terminal state and the fresh
#                        availability receipt shows replicas>=3 and
#                        live_multi_peer_proof=true. Records mint_id for
#                        later phases.
#   availability       - standalone `elastos content status --cid` proof
#                        against the minted (or --cid-supplied) content.
#   buy                - buyer: buy the minted listing (pending -> headless
#                        wallet approval -> confirmed), replay-idempotent.
#   open               - buyer: open_viewer -> read_viewer (init + one
#                        segment) -> close_viewer against elacity-player.
#   drill-custody      - stop custody-b, prove open still settles on the
#                        2-of-3 committee (attempt 1, custody-a + custody-c
#                        release), restart custody-b, prove open settles on
#                        3-of-3 again (attempt 2) with custody-b's DID and
#                        state intact. Fail-closed below quorum is the
#                        negative phase's job (two nodes down).
#   drill-replica      - stop the container holding a proven replica, capture
#                        the cached `content status` (last stored receipt),
#                        prove buy/open fail closed while degraded (they
#                        re-ensure the CID live), run
#                        `content repair-worker --force`, prove status heals
#                        and open succeeds again.
#   negative           - 5 scripted assertions in one receipt block: below-
#                        quorum (2 custody nodes stopped -> open fails
#                        closed), denial (a third, non-purchasing principal
#                        -> RUNTIME_CUSTODY_OPEN_DENIED_MESSAGE), foreign (a
#                        second non-purchasing principal replays the buyer's
#                        own captured viewer_session_handle via read_viewer
#                        -> rejected; see below), stale (the buyer replays
#                        their own captured read_viewer op, same
#                        token/handle, after close_viewer already settled it
#                        -> rejected), tamper (flip one byte in a stored
#                        replica file on one custody node -> the whole open
#                        fails, byte restored after).
#                        NOTE: the brief's original "foreign/expired binding"
#                        case (splice a stale token's launch_id/session_id/
#                        grant_id/proof_binding_id into a fresh token's
#                        request body) is NOT implemented -- the gateway
#                        proxy unconditionally strips exactly those 4 fields
#                        from every is_protected_viewer_op request body and
#                        re-injects its own verified context values
#                        (api/gateway_provider_proxy.rs:1653-1685), so a
#                        client-supplied splice can never reach the handler;
#                        that case would pass against a correct product for
#                        the wrong reason. Replaced per fix-round ruling with
#                        the two cases above, which exercise real,
#                        server-enforced denial paths instead.
#   restart            - mints its own throwaway item, initiates buy,
#                        SIGKILLs the client gateway between the wallet
#                        approval and its confirmation, restarts via
#                        mac-source-home-restart.sh, re-launches a FRESH
#                        capsule token (pre-kill tokens are not assumed to
#                        survive a restart), re-issues buy, asserts an exact
#                        replay (no duplicate transaction).
#   cleanup            - explicit close_viewer settles; then SIGKILL the
#                        client mid-open, restart, assert the boot sweeper
#                        settles the CleanupPending viewer lease.
#   finalize           - aggregates every REQUIRED journey phase block
#                        already written to the receipt (provision through
#                        cleanup; chain-config-real is optional, recorded
#                        separately) into one overall_ok verdict -- true
#                        only when every required phase is BOTH present
#                        AND ok:true, never on absence (a receipt missing
#                        a phase entirely records it under
#                        missing_required_phases and overall_ok stays
#                        false). Also captures per-container (host + each
#                        of the 3 custody nodes) elastos/custody-provider
#                        sha256 evidence and records the receipt/
#                        transcript/commands-log paths. Run last; reads,
#                        never re-runs, the other phases.
#   all                - provision, preflight, wallet-setup, mint,
#                        availability, buy, open, drill-custody,
#                        drill-replica, negative, restart, cleanup,
#                        finalize, in that order (the only order that keeps
#                        every later phase's prerequisites satisfied).
#
# Home-token login sequence (read before running any HTTP phase):
# there is no headless/curl-only way to mint the FIRST Home session --
# `require_home_token_context` (api/gateway_home_token.rs:392) demands a
# same-origin browser request carrying a token whose signature chains back
# to a real authenticated session grant, and the only routes that ever mint
# that first grant are the passkey/WebAuthn ceremony endpoints
# (`/api/auth/passkey/register/*` and `/api/auth/passkey/authenticate/*`,
# wired in api/gateway.rs:614-629) -- a real hardware/platform authenticator
# ceremony, not scriptable from this file. So: the OPERATOR logs into Home
# once in a real browser (first passkey registration bootstraps the initial
# Admin), then supplies the resulting Home session to this script exactly
# the way scripts/library-live-smoke.sh already does it: --home-token (or
# $ELASTOS_HOME_TOKEN) for the raw `x-elastos-home-token` value, or
# --home-cookie/$ELASTOS_HOME_COOKIE for a `home-session=<token>` Cookie
# header, or --home-cookie-jar/$ELASTOS_HOME_COOKIE_JAR for a curl cookie
# jar holding it. Every phase past that point is headless: it calls
# `POST /api/apps/home/launch {"target": <capsule>}` with that base
# credential (api/gateway.rs:925, handler api/gateway_home_runtime.rs:3) to
# mint a capsule-scoped projection token, which the handler appends to the
# response `route` as a URL FRAGMENT, not a query parameter --
# `#home_token=<token>` (api/gateway_home_runtime.rs:154-177
# `append_home_launch_token_to_route`). This script extracts it from the
# fragment; note library-live-smoke.sh's `url.searchParams.get("home_token")`
# extraction reads the query string and would silently return empty against
# this handler's current fragment-based shape.
#
# import-recovery-key finding: `POST /api/apps/wallet/wallet/accounts/
# import-recovery-key` (gateway_wallet_app.rs:174) does accept raw key
# material (`recovery_key`, arbitrary JSON) as an alternative to the managed-
# account-plus-transfer flow, but its handler unconditionally calls
# `consume_passkey_step_up_token` first -- it needs a FRESH interactive
# passkey step-up ceremony per call, same as account deletion or recovery-
# key export. It is not a headless shortcut past the transfer flow; it is
# documented here as the alternative wallet-setup can take (via
# --creator-recovery-key/--buyer-recovery-key plus an operator-supplied
# --*-step-up-token) when the operator prefers importing an existing funded
# key over transferring into a fresh managed account.
#
# Both phases read/write one evidence receipt (schema
# elastos.protected-content.installed-e2e-proof/v1) at --receipt-out
# (default <client-data-dir>/receipts/protected-content-installed-e2e-proof.json),
# merging their own top-level block so provision and preflight can run in
# separate invocations and still produce one combined receipt.
#
# --valid-days (default 365, mirrors the CLI's own default): the signed
# custody epoch this ceremony mints expires after this many days from now,
# and an expired epoch fails CLOSED on the CLIENT by design (see
# protected_content_config.rs's `valid_days` and the Runtime loader) -- this
# is a deliberate fail-closed choice, not a node defect. A long-idle
# deployment (content archived and not touched for longer than this window)
# needs a conscious re-ceremony before it can serve again, so pick this
# value deliberately for the deployment's expected access cadence rather
# than accepting the default blindly.
#
# The chain-provider.json this ceremony installs uses local placeholder RPC
# endpoints (loopback HTTP, which validate_private_rpc_url permits) and
# placeholder mint contract addresses (the exact ones the Rust test suite
# itself uses, in protected_content_config.rs's `chain_config_command`
# fixture) -- GenerateChainConfig only validates URL/address *shape*, it
# never dials out, so this stays a purely structural proof exactly like the
# custody composition ceremony. Override --chain-rpc-url /
# --chain-evidence-rpc-url / --chain-mint-* before pointing this at a real
# deployment.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# Tracked so a hard failure anywhere in a phase can still write an
# `"ok": false` block naming exactly which step it was in -- see the EXIT
# trap below. CURRENT_PHASE is set only once dispatch actually picks a
# phase, so a bare --help/usage/arg-parse exit never writes anything.
CURRENT_PHASE=""
CURRENT_STEP="startup"
LAST_FAIL_MESSAGE=""

log() {
    printf '%s\n' "$*" >&2
    # Every echoed real command ("+ ...", the convention every phase in this
    # file already follows) is also captured to COMMANDS_PATH -- the
    # brief's "full command list" evidence artifact. Best-effort, silent on
    # its own errors, and a no-op before COMMANDS_PATH's default is
    # computed (arg-parsing's own log() calls never start with "+").
    case "$1" in
    '+'*)
        if [ -n "${COMMANDS_PATH:-}" ]; then
            mkdir -p -m 700 "$(dirname "$COMMANDS_PATH")" 2>/dev/null || true
            printf '%s\n' "$*" >>"$COMMANDS_PATH" 2>/dev/null || true
        fi
        ;;
    esac
}
fail() {
    LAST_FAIL_MESSAGE="$*"
    printf 'FAIL: %s\n' "$*" >&2
    exit 1
}

# On ANY nonzero exit while a phase is active, write `{"ok": false,
# "failed_step": ..., "error": ..., "exit_code": ...}` (plus a best-effort
# git identity) into that phase's receipt block, so a stale `"ok": true`
# from a previous successful run can never survive a later regression --
# the receipt always describes the LATEST run of each phase, success or
# failure. Best-effort and silent on its own errors: this fires during
# teardown of an already-failing script, so it must never mask or replace
# the original failure.
write_failure_receipt() {
    local exit_code="$1"
    local commit="" tree="" clean="unknown"
    if git -C "$ROOT" rev-parse --verify HEAD >/dev/null 2>&1; then
        commit="$(git -C "$ROOT" rev-parse --verify HEAD 2>/dev/null || true)"
        tree="$(git -C "$ROOT" rev-parse --verify 'HEAD^{tree}' 2>/dev/null || true)"
        if [ -z "$(git -C "$ROOT" status --porcelain=v1 --untracked-files=all 2>/dev/null)" ]; then
            clean="true"
        else
            clean="false"
        fi
    fi
    local block
    block="$(python3 -c '
import json
import sys

commit, tree, clean, step, message, exit_code = sys.argv[1:7]
print(json.dumps({
    "ok": False,
    "failed_step": step,
    "error": message,
    "exit_code": int(exit_code),
    "git": {
        "commit": commit or None,
        "tree": tree or None,
        "clean": {"true": True, "false": False}.get(clean),
    },
}))
' "$commit" "$tree" "$clean" "$CURRENT_STEP" "${LAST_FAIL_MESSAGE:-unknown failure (see stderr above)}" "$exit_code" 2>/dev/null)" || return 0
    [ -n "$block" ] || return 0
    write_receipt_block "$CURRENT_PHASE" "$block" 2>/dev/null || true
}

on_exit() {
    local exit_code=$?
    if [ "$exit_code" -ne 0 ] && [ -n "$CURRENT_PHASE" ]; then
        write_failure_receipt "$exit_code"
    fi
    # Safety net for the drill/negative phases: every docker_stop_service
    # call and every negative_tamper byte-flip pushes its own inverse onto
    # RESTORE_KINDS/ARG1/ARG2/ARG3 (see push_restore_docker_start/
    # push_restore_tamper below); a designed success path pops its own
    # entry once it has already restored things itself, so this is a no-op
    # on a clean run. On ANY unanticipated failure while a container is
    # stopped or a replica is tampered (not just the specific assertion
    # paths that already called docker_start_service/restored the byte
    # inline), this still runs here, so a failed run never leaves the
    # harness broken. Best-effort by design (see run_restore_stack).
    run_restore_stack
}

# Forward declaration, overwritten by the real definition further down
# (Important 4's restore stack, next to docker_stop_service/
# docker_start_service). on_exit (just above) unconditionally calls
# run_restore_stack, but the EXIT trap can fire before the script ever
# reaches that later definition -- a bare --help, a missing --phase, or a
# validate_positive_int failure all `exit` long before dispatch, and would
# otherwise hit "run_restore_stack: command not found" (exit 127) instead
# of their real exit code (fix round 2, same bug class as Critical 1: a
# name the EXIT trap can reach before it exists). A true no-op is always
# correct here: nothing can have been pushed onto the restore stack before
# any phase has actually run.
run_restore_stack() { :; }

trap on_exit EXIT

# --- defaults ---------------------------------------------------------

CLIENT_DATA_DIR="${HOME}/Library/Application Support/elastos"
VALID_DAYS=365
POLICY_AUTHORITY_KEY="${HOME}/.elastos-protected-content/policy-authority.key"
SHARED_DIR="${ROOT}/deploy/custody-host/shared"
COMPOSE_FILE="${ROOT}/deploy/custody-host/docker-compose.yml"
PROFILE="home"
ALLOW_DIRTY=0
RECEIPT_PATH=""
CHAIN_RPC_URL="http://127.0.0.1:8545"
CHAIN_EVIDENCE_RPC_URLS=()
CHAIN_MINT_LEDGER="0x0000000000000000000000000000000000000022"
CHAIN_MINT_PAY_TOKEN="0x0000000000000000000000000000000000000033"
CHAIN_MINT_ASSET_CREATED_EMITTER="0x0000000000000000000000000000000000000044"
PHASE=""

SERVICES=(custody-a custody-b custody-c)
CONTAINER_READY_PATH=/home/custody/.local/share/elastos/run/provider-host.ready.json

# Restore stack for the drill/negative phases (Important 4 in the fix
# round): every docker_stop_service call and every negative_tamper
# byte-flip pushes its own inverse action here (parallel arrays -- bash 3.2
# has no associative arrays); on_exit's run_restore_stack (see above) runs
# every pending entry, most-recent-first, so a failed run never leaves a
# container stopped or a replica corrupted. A designed success path pops
# its own entry once it has already restored things itself.
RESTORE_KINDS=()
RESTORE_ARG1=()
RESTORE_ARG2=()
RESTORE_ARG3=()

# Fail-closed message constants pinned verbatim from
# elastos/crates/elastos-server/src/protected_content_runtime.rs (kept as
# plain strings here, not re-derived at runtime, so a source rename shows
# up as a script assertion failure rather than a silently-vacuous check).
RUNTIME_CUSTODY_OPEN_DENIED_MESSAGE="Runtime custody open is denied before purchase"                    # protected_content_runtime.rs:144-145
RUNTIME_CUSTODY_RELEASE_APPROVAL_UNAVAILABLE_MESSAGE="Runtime custody viewer release approval is unavailable"  # protected_content_runtime.rs RUNTIME_CUSTODY_RELEASE_APPROVAL_UNAVAILABLE_MESSAGE
RUNTIME_CUSTODY_AVAILABILITY_UNAVAILABLE_MESSAGE="Runtime custody content availability is unavailable"  # protected_content_runtime.rs:146-147
RUNTIME_CUSTODY_DECRYPT_UNAVAILABLE_MESSAGE="Runtime custody decrypt provider is unavailable"            # protected_content_runtime.rs:148-149
RUNTIME_CUSTODY_PURCHASE_DENIED_MESSAGE="Runtime custody purchase is denied before buy"                  # protected_content_runtime.rs:138-139; also the buy path's own map_err of a failed fresh-availability check (api/gateway_provider_proxy.rs:2986-2999)
RUNTIME_CUSTODY_VIEWER_SESSION_UNAVAILABLE_MESSAGE="Runtime custody viewer session is unavailable"       # protected_content_runtime.rs (27 call sites; e.g. :5152, and every denial branch of read/close_runtime_custody_viewer around :6045-6210)

# --- HTTP/journey defaults ---------------------------------------------

GATEWAY_URL="${ELASTOS_GATEWAY_URL:-http://localhost:61180}"
GATEWAY_URL="${GATEWAY_URL%/}"

CREATOR_HOME_TOKEN="${ELASTOS_HOME_TOKEN:-}"
CREATOR_HOME_COOKIE="${ELASTOS_HOME_COOKIE:-}"
CREATOR_HOME_COOKIE_JAR="${ELASTOS_HOME_COOKIE_JAR:-}"
BUYER_HOME_TOKEN="${ELASTOS_BUYER_HOME_TOKEN:-}"
BUYER_HOME_COOKIE="${ELASTOS_BUYER_HOME_COOKIE:-}"
BUYER_HOME_COOKIE_JAR="${ELASTOS_BUYER_HOME_COOKIE_JAR:-}"
DENIAL_HOME_TOKEN="${ELASTOS_DENIAL_HOME_TOKEN:-}"
DENIAL_HOME_COOKIE="${ELASTOS_DENIAL_HOME_COOKIE:-}"
DENIAL_HOME_COOKIE_JAR="${ELASTOS_DENIAL_HOME_COOKIE_JAR:-}"

CREATOR_RECOVERY_KEY=""
CREATOR_STEP_UP_TOKEN=""
BUYER_RECOVERY_KEY=""
BUYER_STEP_UP_TOKEN=""
DENIAL_RECOVERY_KEY=""
DENIAL_STEP_UP_TOKEN=""

CONTENT_PATH=""
COPIES="3"
PRICE_WEI="1000000000000000"
# Managed-wallet approvals (mint, buy, restart) need a fresh passkey
# step-up token bound to the exact approval request; only the passkey
# holder can produce one, so the driver delegates to a hook command:
#   <hook> <principal: creator|buyer> <operation> <app-launch-token>
# with the canonical step-up request JSON on stdin, printing the token on
# stdout. Without a hook the driver fails closed with the Wallet UI action.
STEP_UP_HOOK="${ELASTOS_STEP_UP_HOOK:-}"
# After a wallet approval the Wallet signs/broadcasts and the Chain confirms;
# the pending call is re-issued until it settles, bounded by these.
SETTLE_TIMEOUT_SECONDS=300
SETTLE_POLL_SECONDS=5
MINT_ID=""
CID=""
CHAIN_NAMESPACE="eip155:8453"

FUNDING_TIMEOUT_SECONDS=1800
FUNDING_POLL_SECONDS=15
FUNDING_RPC_URL=""

REAL_RPC_URL=""
REAL_EVIDENCE_RPC_URLS=()

# Every HTTP request/response this driver makes is appended here as one
# JSON line (owner-only 0600). Redacted exactly like the terminal log
# redacts the home-token header value (see redact_token()): recovery_key/
# step_up_token/home_token field values, and any home_token=... URL
# fragment embedded in a route string, are replaced with a
# <redacted:N-chars> marker (see record_transcript()) before the line is
# written -- this is the evidence artifact meant to be reviewed/attached,
# so it must never carry raw secret material even though it captures full
# request/response shape otherwise. Default sits beside the evidence
# receipt.
TRANSCRIPT_PATH=""

# Every "+ ..." command this driver echoes to the terminal (log()) is also
# appended here, verbatim, as the brief's "full command list" -- the
# ordered set of commands a human re-runs to reproduce a run. Owner-only
# 0600, append-only, default sits beside the evidence receipt.
COMMANDS_PATH=""

GATEWAY_HOST=""

usage() {
    cat >&2 <<'USAGE'
Usage:
  scripts/protected-content-installed-e2e-proof.sh --phase <name> [options]

Phases: provision | preflight | chain-config-real | wallet-setup | mint |
        availability | buy | open | drill-custody | drill-replica |
        negative | restart | cleanup | finalize | all

Runbook order for a real, user-driven live session (see the header comment
for the full rationale of each step):
  1. provision            (client artifact/composition/chain-config ceremony)
  2. preflight             (Carrier dial + readiness proof against the 3 nodes)
  3. chain-config-real     (only if pointing at a real chain deployment)
  4. log into Home once in a real browser, for THREE principals (creator,
     buyer, and a denial principal used by drill-replica/negative); capture
     each resulting Home session (see "Home-token login sequence" in the
     header comment)
  5. wallet-setup           (needs --home-token/--buyer-home-token/
                             --denial-home-token; the operator must complete
                             the funding transfer this phase waits for, for
                             all three principals -- the denial principal
                             needs its own small funded balance too, or
                             drill-replica's degraded-buy assertion would
                             fail for wallet/settlement reasons instead of
                             the availability reason it is meant to prove)
  6. mint, availability, buy, open   (the positive journey)
  7. drill-custody, drill-replica, negative, restart, cleanup, finalize

Shared options:
  --phase <name>                  Required; one of the phases above.
  --client-data-dir <dir>         Client Runtime data dir. Default: $HOME/Library/Application Support/elastos
                                   (must end in "/Library/Application Support/elastos" -- that's
                                   the fixed suffix mac-source-home-restart.sh's --test-home implies)
  --valid-days <n>                 Custody epoch validity window. Default: 365
  --policy-authority-key <path>   Owner-only path outside the repo. Default: ~/.elastos-protected-content/policy-authority.key
  --shared-dir <dir>               Custody node descriptor/ticket handoff dir. Default: deploy/custody-host/shared
  --compose-file <path>           Custody-host compose file. Default: deploy/custody-host/docker-compose.yml
  --profile <name>                 components.json profile for the static audit. Default: home
  --receipt-out <path>             Evidence receipt path. Default: <client-data-dir>/receipts/protected-content-installed-e2e-proof.json
  --transcript-out <path>          HTTP transcript JSONL path (secrets redacted). Default: <client-data-dir>/receipts/protected-content-installed-e2e-proof-transcript.jsonl
  --commands-out <path>            Full command list (every echoed "+ ..." line). Default: <client-data-dir>/receipts/protected-content-installed-e2e-proof-commands.log
  --allow-dirty                    DEVELOPMENT ONLY: skip the clean-working-tree gate
  -h, --help                       Print this help

provision-only (placeholder chain config; see header comment):
  --chain-rpc-url <url>             Placeholder primary chain RPC (loopback default)
  --chain-evidence-rpc-url <url>   Placeholder evidence RPC (repeat 2..=5x; default: two loopback placeholders)

chain-config-real only:
  --real-rpc-url <url>              Real primary chain RPC (required)
  --real-evidence-rpc-url <url>    Real independent evidence RPC, distinct origin (repeat 2..=5x, required)
  --chain-mint-ledger <addr>        Real mint ledger contract (required)
  --chain-mint-pay-token <addr>     Real mint pay-token contract (required)
  --chain-mint-asset-created-emitter <addr>  Real AssetCreated emitter (required)

HTTP/journey phases (wallet-setup, mint, availability, buy, open,
drill-*, negative, restart, cleanup, all):
  --gateway-url <url>               Installed client gateway base URL. Default: $ELASTOS_GATEWAY_URL or http://localhost:61180
  --home-token <token>              Creator's signed Home session token (x-elastos-home-token). Default: $ELASTOS_HOME_TOKEN
  --home-cookie <cookie>            Creator's `home-session=<token>` Cookie header. Default: $ELASTOS_HOME_COOKIE
  --home-cookie-jar <path>          Creator's curl cookie jar holding home-session. Default: $ELASTOS_HOME_COOKIE_JAR
  --buyer-home-token/--buyer-home-cookie/--buyer-home-cookie-jar   Same, for the buyer principal ($ELASTOS_BUYER_HOME_*)
  --denial-home-token/--denial-home-cookie/--denial-home-cookie-jar   Same, for the negative-case denial principal ($ELASTOS_DENIAL_HOME_*)
  --content-path <path>             File to mint (required for --phase mint)
  --copies <n>                      Runtime custody copies. Default: 3
  --price-wei <n>                   Runtime custody price in wei (string). Default: 1000000000000000
  --step-up-hook <cmd>              Command producing passkey step-up tokens for wallet approvals (see header). Default: $ELASTOS_STEP_UP_HOOK
  --mint-id <hex>                   Mint id override; default: read from the receipt's mint.mint_id (set by --phase mint)
  --cid <cid>                       Content id override for --phase availability; default: read from the receipt
  --chain-namespace <ns>            eip155:<chain_id> for wallet defaults/balance polling. Default: eip155:8453
  --creator-recovery-key <json>     Alternative to a managed account: import an existing key (needs --creator-step-up-token)
  --creator-step-up-token <token>   Fresh passkey step-up token for the creator's import-recovery-key call
  --buyer-recovery-key <json>       Same, for the buyer
  --buyer-step-up-token <token>     Same, for the buyer
  --denial-recovery-key <json>      Same, for the negative/drill-replica denial principal
  --denial-step-up-token <token>    Same, for the denial principal
  --funding-timeout <seconds>       Wallet-setup funding poll timeout. Default: 1800 (must be a positive integer)
  --funding-poll-interval <seconds> Wallet-setup funding poll interval. Default: 15 (must be a positive integer)
  --funding-rpc-url <url>           RPC used to poll balances. Default: --chain-rpc-url's value
USAGE
}

while [ $# -gt 0 ]; do
    case "$1" in
    --phase)
        PHASE="${2:-}"
        shift 2
        ;;
    --client-data-dir)
        CLIENT_DATA_DIR="${2:-}"
        shift 2
        ;;
    --valid-days)
        VALID_DAYS="${2:-}"
        shift 2
        ;;
    --policy-authority-key)
        POLICY_AUTHORITY_KEY="${2:-}"
        shift 2
        ;;
    --shared-dir)
        SHARED_DIR="${2:-}"
        shift 2
        ;;
    --compose-file)
        COMPOSE_FILE="${2:-}"
        shift 2
        ;;
    --profile)
        PROFILE="${2:-}"
        shift 2
        ;;
    --receipt-out)
        RECEIPT_PATH="${2:-}"
        shift 2
        ;;
    --chain-rpc-url)
        CHAIN_RPC_URL="${2:-}"
        shift 2
        ;;
    --chain-evidence-rpc-url)
        CHAIN_EVIDENCE_RPC_URLS+=("${2:-}")
        shift 2
        ;;
    --allow-dirty)
        ALLOW_DIRTY=1
        shift
        ;;
    --transcript-out)
        TRANSCRIPT_PATH="${2:-}"
        shift 2
        ;;
    --gateway-url)
        GATEWAY_URL="${2%/}"
        shift 2
        ;;
    --home-token)
        CREATOR_HOME_TOKEN="${2:-}"
        shift 2
        ;;
    --home-cookie)
        CREATOR_HOME_COOKIE="${2:-}"
        shift 2
        ;;
    --home-cookie-jar)
        CREATOR_HOME_COOKIE_JAR="${2:-}"
        shift 2
        ;;
    --buyer-home-token)
        BUYER_HOME_TOKEN="${2:-}"
        shift 2
        ;;
    --buyer-home-cookie)
        BUYER_HOME_COOKIE="${2:-}"
        shift 2
        ;;
    --buyer-home-cookie-jar)
        BUYER_HOME_COOKIE_JAR="${2:-}"
        shift 2
        ;;
    --denial-home-token)
        DENIAL_HOME_TOKEN="${2:-}"
        shift 2
        ;;
    --denial-home-cookie)
        DENIAL_HOME_COOKIE="${2:-}"
        shift 2
        ;;
    --denial-home-cookie-jar)
        DENIAL_HOME_COOKIE_JAR="${2:-}"
        shift 2
        ;;
    --content-path)
        CONTENT_PATH="${2:-}"
        shift 2
        ;;
    --copies)
        COPIES="${2:-}"
        shift 2
        ;;
    --step-up-hook)
        STEP_UP_HOOK="${2:-}"
        shift 2
        ;;
    --price-wei)
        PRICE_WEI="${2:-}"
        shift 2
        ;;
    --mint-id)
        MINT_ID="${2:-}"
        shift 2
        ;;
    --cid)
        CID="${2:-}"
        shift 2
        ;;
    --chain-namespace)
        CHAIN_NAMESPACE="${2:-}"
        shift 2
        ;;
    --creator-recovery-key)
        CREATOR_RECOVERY_KEY="${2:-}"
        shift 2
        ;;
    --creator-step-up-token)
        CREATOR_STEP_UP_TOKEN="${2:-}"
        shift 2
        ;;
    --buyer-recovery-key)
        BUYER_RECOVERY_KEY="${2:-}"
        shift 2
        ;;
    --buyer-step-up-token)
        BUYER_STEP_UP_TOKEN="${2:-}"
        shift 2
        ;;
    --denial-recovery-key)
        DENIAL_RECOVERY_KEY="${2:-}"
        shift 2
        ;;
    --denial-step-up-token)
        DENIAL_STEP_UP_TOKEN="${2:-}"
        shift 2
        ;;
    --commands-out)
        COMMANDS_PATH="${2:-}"
        shift 2
        ;;
    --funding-timeout)
        FUNDING_TIMEOUT_SECONDS="${2:-}"
        shift 2
        ;;
    --funding-poll-interval)
        FUNDING_POLL_SECONDS="${2:-}"
        shift 2
        ;;
    --funding-rpc-url)
        FUNDING_RPC_URL="${2:-}"
        shift 2
        ;;
    --real-rpc-url)
        REAL_RPC_URL="${2:-}"
        shift 2
        ;;
    --real-evidence-rpc-url)
        REAL_EVIDENCE_RPC_URLS+=("${2:-}")
        shift 2
        ;;
    --chain-mint-ledger)
        CHAIN_MINT_LEDGER="${2:-}"
        shift 2
        ;;
    --chain-mint-pay-token)
        CHAIN_MINT_PAY_TOKEN="${2:-}"
        shift 2
        ;;
    --chain-mint-asset-created-emitter)
        CHAIN_MINT_ASSET_CREATED_EMITTER="${2:-}"
        shift 2
        ;;
    --help | -h)
        usage
        exit 0
        ;;
    *)
        log "unknown argument: $1"
        usage
        exit 2
        ;;
    esac
done

[ -n "$PHASE" ] || {
    log "--phase is required"
    usage
    exit 2
}

if [ "${#CHAIN_EVIDENCE_RPC_URLS[@]}" -eq 0 ]; then
    CHAIN_EVIDENCE_RPC_URLS=(http://127.0.0.1:8546 http://127.0.0.1:8547)
fi

[ -n "$RECEIPT_PATH" ] || RECEIPT_PATH="${CLIENT_DATA_DIR}/receipts/protected-content-installed-e2e-proof.json"
[ -n "$TRANSCRIPT_PATH" ] || TRANSCRIPT_PATH="${CLIENT_DATA_DIR}/receipts/protected-content-installed-e2e-proof-transcript.jsonl"
[ -n "$COMMANDS_PATH" ] || COMMANDS_PATH="${CLIENT_DATA_DIR}/receipts/protected-content-installed-e2e-proof-commands.log"
[ -n "$FUNDING_RPC_URL" ] || FUNDING_RPC_URL="$CHAIN_RPC_URL"

# Poll-interval/timeout flags must be positive integers -- a 0 (or
# non-numeric) value makes poll_funding's `waited=$((waited + N))` loop
# spin forever, since it can never reach the timeout (minor 14 in the fix
# round). Checked here, once, right after every flag has its final value,
# rather than deep inside the polling loop itself.
validate_positive_int() {
    local value="$1" flag="$2"
    if ! [[ "$value" =~ ^[1-9][0-9]*$ ]]; then
        fail "$flag must be a positive integer, got '$value'"
    fi
}
validate_positive_int "$FUNDING_POLL_SECONDS" "--funding-poll-interval"
validate_positive_int "$FUNDING_TIMEOUT_SECONDS" "--funding-timeout"

# --- shared helpers -----------------------------------------------------

check_git_clean() {
    local status
    status="$(git -C "$ROOT" status --porcelain=v1 --untracked-files=all)"
    if [ -n "$status" ] && [ "$ALLOW_DIRTY" != "1" ]; then
        fail "working tree is dirty; refusing to run. diagnostic: git -C '$ROOT' status --porcelain=v1 --untracked-files=all -- pass --allow-dirty for development only"
    fi
}

git_identity() {
    GIT_COMMIT="$(git -C "$ROOT" rev-parse --verify HEAD)"
    GIT_TREE="$(git -C "$ROOT" rev-parse --verify 'HEAD^{tree}')"
    local status
    status="$(git -C "$ROOT" status --porcelain=v1 --untracked-files=all)"
    if [ -z "$status" ]; then
        GIT_CLEAN=true
    else
        GIT_CLEAN=false
    fi
}

sha256_of() {
    # Emits "sha256:<hex>", or empty string when the file is absent -- the
    # empty-string sentinel is converted to JSON null at the call site
    # rather than baked in here, so this stays a plain string helper.
    if [ -f "$1" ]; then
        printf 'sha256:%s' "$(shasum -a 256 "$1" | awk '{print $1}')"
    fi
}

resolve_elastos_bin() {
    ELASTOS_BIN="${ROOT}/elastos/target/release/elastos"
    # This is a shared, repo-relative cargo output path; this session has
    # already observed it silently clobbered with a non-runnable binary by
    # a concurrent process building container images from the same repo
    # (see Task 6's report). `-x` alone cannot tell that apart from a real
    # host-native binary, so actually execute it before trusting it.
    if [ ! -x "$ELASTOS_BIN" ] || ! "$ELASTOS_BIN" --version >/dev/null 2>&1; then
        log "host elastos binary missing or not runnable at $ELASTOS_BIN; building (cargo build --release -p elastos-server)"
        (cd "$ROOT" && cargo build --release --manifest-path elastos/Cargo.toml -p elastos-server)
    fi
}

detect_platform() {
    case "$(uname -s)-$(uname -m)" in
    Darwin-arm64) PLATFORM="darwin-arm64" ;;
    Linux-x86_64) PLATFORM="linux-amd64" ;;
    Linux-aarch64 | Linux-arm64) PLATFORM="linux-arm64" ;;
    *) fail "unsupported platform: $(uname -s)-$(uname -m)" ;;
    esac
}

discover_descriptors() {
    DESCRIPTOR_PATHS=()
    DESCRIPTOR_DIDS=()
    local f base
    for f in "$SHARED_DIR"/*.descriptor.json; do
        [ -e "$f" ] || continue
        base="$(basename "$f" .descriptor.json)"
        DESCRIPTOR_PATHS+=("$f")
        DESCRIPTOR_DIDS+=("$base")
    done
    local count="${#DESCRIPTOR_PATHS[@]}"
    [ "$count" -eq 3 ] || fail "expected exactly 3 custody node descriptors in '$SHARED_DIR', found $count; diagnostic: ls '$SHARED_DIR'/*.descriptor.json"
}

compute_artifact_hashes() {
    SOURCE_ELASTOS_SHA256="$(sha256_of "${ROOT}/elastos/target/release/elastos")"
    INSTALLED_ELASTOS_SHA256="$(sha256_of "${CLIENT_DATA_DIR}/bin/elastos")"
    if [ -n "$SOURCE_ELASTOS_SHA256" ] && [ "$SOURCE_ELASTOS_SHA256" = "$INSTALLED_ELASTOS_SHA256" ]; then
        ARTIFACT_PARITY=true
    else
        ARTIFACT_PARITY=false
    fi
}

# Merge a JSON block (passed as $2, a JSON text) under receipt[block_name],
# creating the receipt file (schema
# elastos.protected-content.installed-e2e-proof/v1) if it doesn't exist yet.
# Owner-only (0600), atomic replace. The block is handed over through an
# env var, not stdin -- stdin is where the inline python script itself is
# fed via `python3 -`, so reusing it for data too would race the script
# read against the data read.
write_receipt_block() {
    local block_name="$1"
    local block_json="$2"
    mkdir -p -m 700 "$(dirname "$RECEIPT_PATH")"
    RECEIPT_BLOCK_JSON="$block_json" python3 - "$RECEIPT_PATH" "$block_name" <<'PY'
import json
import os
import sys

path, block_name = sys.argv[1], sys.argv[2]
block = json.loads(os.environ["RECEIPT_BLOCK_JSON"])
try:
    with open(path) as handle:
        receipt = json.load(handle)
except (FileNotFoundError, json.JSONDecodeError):
    receipt = {
        "schema": "elastos.protected-content.installed-e2e-proof/v1",
        "version": 1,
    }
receipt[block_name] = block
tmp = path + ".tmp"
fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
with os.fdopen(fd, "w") as handle:
    json.dump(receipt, handle, indent=2, sort_keys=True)
    handle.write("\n")
os.replace(tmp, path)
os.chmod(path, 0o600)
PY
}

# --- shared HTTP/journey helpers ----------------------------------------
#
# Every HTTP-driving phase (wallet-setup onward) shares these: a redacted
# curl wrapper, a full-detail transcript writer, Home-session credential
# handling per principal (creator/buyer/denial -- kept as three explicit
# call sites rather than one indirected-by-name helper because this file
# targets macOS's stock bash 3.2, which has neither namerefs nor
# associative arrays), the Home-launch -> capsule-token mint sequence, and
# thin provider/wallet JSON-RPC wrappers.

# Redacts a bearer-style secret for terminal logs: never the transcript
# file, which keeps the real value for genuine evidence review.
redact_token() {
    local value="$1"
    if [ -z "$value" ]; then
        printf '<empty>'
    else
        printf '<redacted:%d-chars>' "${#value}"
    fi
}

gateway_host() {
    [ -n "$GATEWAY_HOST" ] || GATEWAY_HOST="$(python3 -c 'import sys,urllib.parse; print(urllib.parse.urlsplit(sys.argv[1]).netloc)' "$GATEWAY_URL")"
    printf '%s' "$GATEWAY_HOST"
}

# Appends one full-detail HTTP exchange to the transcript JSONL (owner-only
# 0600, append-only). Best-effort like write_failure_receipt: a transcript
# write must never itself fail an otherwise-successful call.
# Redacts recovery_key/step_up_token/home_token values, and any
# "home_token=..." fragment embedded in a route/URL string, exactly the way
# the terminal log already redacts a home-token header (redact_token()'s
# "<redacted:N-chars>" shape) -- Important 5 in the fix round. Applied to
# BOTH the request and response before the JSONL line is ever written, so
# the evidence artifact itself never carries raw secret material even
# though every other field (shape, ids, error messages) survives intact.
record_transcript() {
    local method="$1" path="$2" request_body="$3" http_code="$4" response_body="$5"
    mkdir -p -m 700 "$(dirname "$TRANSCRIPT_PATH")" 2>/dev/null || true
    RECORD_METHOD="$method" RECORD_PATH="$path" RECORD_REQUEST="$request_body" \
        RECORD_HTTP_CODE="$http_code" RECORD_RESPONSE="$response_body" \
        python3 -c '
import json
import os
import re
import sys
import time

REDACT_KEYS = {"recovery_key", "step_up_token", "home_token"}
HOME_TOKEN_FRAGMENT_RE = re.compile(r"(home_token=)([^&\s\"]+)")

def redact_scalar(value):
    try:
        length = len(value) if isinstance(value, str) else len(json.dumps(value))
    except Exception:
        length = 0
    return "<redacted:%d-chars>" % length

def redact_fragment(text):
    def _sub(match):
        return match.group(1) + "<redacted:%d-chars>" % len(match.group(2))
    return HOME_TOKEN_FRAGMENT_RE.sub(_sub, text)

def walk(node):
    if isinstance(node, dict):
        out = {}
        for key, value in node.items():
            if key in REDACT_KEYS:
                out[key] = redact_scalar(value)
            elif isinstance(value, str):
                out[key] = redact_fragment(value)
            else:
                out[key] = walk(value)
        return out
    if isinstance(node, list):
        return [walk(item) for item in node]
    if isinstance(node, str):
        return redact_fragment(node)
    return node

path = sys.argv[1]
entry = {
    "at": int(time.time()),
    "method": os.environ["RECORD_METHOD"],
    "path": os.environ["RECORD_PATH"],
    "http_code": os.environ["RECORD_HTTP_CODE"],
}
for key, env_name in (("request", "RECORD_REQUEST"), ("response", "RECORD_RESPONSE")):
    raw = os.environ.get(env_name, "")
    try:
        parsed = json.loads(raw) if raw else None
        entry[key] = walk(parsed) if parsed is not None else None
    except Exception:
        entry[key] = redact_fragment(raw)
line = json.dumps(entry, sort_keys=True)
fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_APPEND, 0o600)
with os.fdopen(fd, "a") as handle:
    handle.write(line + "\n")
os.chmod(path, 0o600)
' "$TRANSCRIPT_PATH" 2>/dev/null || true
}

# curl_call METHOD URL [curl-args...] -- issues the request, sets
# CURL_HTTP_CODE/CURL_BODY. A transport-level failure (no response at all)
# is a hard fail(); an HTTP-level error status is NOT -- callers decide
# what a given status/JSON-status means for their assertion (fail-closed
# negative cases expect exactly this).
curl_call() {
    local method="$1" url="$2"
    shift 2
    local tmp
    tmp="$(mktemp)"
    local code
    if ! code="$(curl -sS -o "$tmp" -w '%{http_code}' -X "$method" "$@" "$url" 2>"${tmp}.stderr")"; then
        local transport_err
        transport_err="$(cat "${tmp}.stderr" 2>/dev/null || true)"
        rm -f "$tmp" "${tmp}.stderr"
        fail "curl transport error calling $method $url: $transport_err"
    fi
    CURL_HTTP_CODE="$code"
    CURL_BODY="$(cat "$tmp")"
    rm -f "$tmp" "${tmp}.stderr"
}

# Sets HOME_AUTH_CURL_ARGS (an array of curl -H/-b args) from one
# principal's token/cookie/cookie-jar triple, or fail()s naming exactly
# which flag (or matching env var) the operator needs to supply -- see the
# header comment's "Home-token login sequence". flag_infix/env_infix are
# "" for the base creator (--home-token / $ELASTOS_HOME_TOKEN) or
# "buyer-"/"BUYER_", "denial-"/"DENIAL_" for the other two principals.
compute_home_auth_args() {
    local token="$1" cookie="$2" jar="$3" human_label="$4" flag_infix="$5" env_infix="$6"
    HOME_AUTH_CURL_ARGS=()
    if [ -n "$token" ]; then
        HOME_AUTH_CURL_ARGS=(-H "x-elastos-home-token: ${token}")
        return 0
    fi
    if [ -n "$cookie" ]; then
        HOME_AUTH_CURL_ARGS=(-H "Cookie: ${cookie}")
        return 0
    fi
    if [ -n "$jar" ]; then
        [ -f "$jar" ] || fail "$human_label cookie jar does not exist: $jar"
        HOME_AUTH_CURL_ARGS=(-b "$jar")
        return 0
    fi
    fail "$human_label has no signed Home session; pass --${flag_infix}home-token / --${flag_infix}home-cookie / --${flag_infix}home-cookie-jar (or \$ELASTOS_${env_infix}HOME_TOKEN / \$ELASTOS_${env_infix}HOME_COOKIE / \$ELASTOS_${env_infix}HOME_COOKIE_JAR), obtained per the header comment's Home-token login sequence"
}

compute_creator_auth_args() {
    compute_home_auth_args "$CREATOR_HOME_TOKEN" "$CREATOR_HOME_COOKIE" "$CREATOR_HOME_COOKIE_JAR" "creator" "" ""
    CREATOR_AUTH_ARGS=("${HOME_AUTH_CURL_ARGS[@]}")
}

compute_buyer_auth_args() {
    compute_home_auth_args "$BUYER_HOME_TOKEN" "$BUYER_HOME_COOKIE" "$BUYER_HOME_COOKIE_JAR" "buyer" "buyer-" "BUYER_"
    BUYER_AUTH_ARGS=("${HOME_AUTH_CURL_ARGS[@]}")
}

compute_denial_auth_args() {
    compute_home_auth_args "$DENIAL_HOME_TOKEN" "$DENIAL_HOME_COOKIE" "$DENIAL_HOME_COOKIE_JAR" "denial" "denial-" "DENIAL_"
    DENIAL_AUTH_ARGS=("${HOME_AUTH_CURL_ARGS[@]}")
}

# gateway_launch TARGET [home-auth curl-args...] -- POSTs
# /api/apps/home/launch, extracts the capsule-scoped projection token from
# the response route's #home_token=... FRAGMENT (never the query string;
# see the header comment), sets LAUNCH_TOKEN. sec-fetch-site: same-origin
# alone satisfies require_exact_home_browser_origin's same-origin-
# provenance check without needing to match GATEWAY_URL's scheme exactly.
gateway_launch() {
    local target="$1"
    shift
    local body
    body="$(python3 -c 'import json,sys; print(json.dumps({"target": sys.argv[1], "query": {}}))' "$target")"
    log "+ curl -X POST ${GATEWAY_URL}/api/apps/home/launch --data '${body}' [home-auth headers redacted]"
    curl_call POST "${GATEWAY_URL}/api/apps/home/launch" \
        "$@" \
        -H "content-type: application/json" \
        -H "Host: $(gateway_host)" \
        -H "sec-fetch-site: same-origin" \
        --data "$body"
    record_transcript POST "/api/apps/home/launch" "$body" "$CURL_HTTP_CODE" "$CURL_BODY"
    case "$CURL_HTTP_CODE" in
    2??) ;;
    *) fail "Home launch of '$target' failed (http $CURL_HTTP_CODE): $CURL_BODY" ;;
    esac
    local route
    route="$(printf '%s' "$CURL_BODY" | python3 -c 'import json,sys
try:
    print(json.load(sys.stdin).get("route",""))
except Exception:
    print("")')"
    LAUNCH_TOKEN="$(python3 -c '
import sys
import urllib.parse
route = sys.argv[1]
frag = urllib.parse.urlsplit(route).fragment
params = urllib.parse.parse_qs(frag)
print((params.get("home_token") or [""])[0])
' "$route")"
    [ -n "$LAUNCH_TOKEN" ] || fail "Home launch of '$target' did not return a capsule launch token in route='$route'"
}

# provider_call SCHEME OP TOKEN BODY_JSON -- POSTs
# /api/provider/<scheme>/<op>. Sets CURL_HTTP_CODE/CURL_BODY plus
# PROVIDER_STATUS (the response JSON's own "status" field, "ok"/"error"/
# "" when unparsable) so callers can assert on either HTTP or app-level
# status, exactly what the fail-closed negative cases need.
provider_call() {
    local scheme="$1" op="$2" token="$3" body="$4"
    log "+ curl -X POST ${GATEWAY_URL}/api/provider/${scheme}/${op} -H 'x-elastos-home-token: $(redact_token "$token")' --data '${body}'"
    local __body_file; __body_file="$(mktemp)"; printf '%s' "$body" > "$__body_file"
    curl_call POST "${GATEWAY_URL}/api/provider/${scheme}/${op}" \
        -H "x-elastos-home-token: ${token}" \
        -H "Origin: null" \
        -H "content-type: application/json" \
        --data "@${__body_file}"
    rm -f "$__body_file"
    record_transcript POST "/api/provider/${scheme}/${op}" "$body" "$CURL_HTTP_CODE" "$CURL_BODY"
    PROVIDER_STATUS="$(printf '%s' "$CURL_BODY" | python3 -c 'import json,sys
try:
    print(json.load(sys.stdin).get("status",""))
except Exception:
    print("")' 2>/dev/null || true)"
}

# wallet_call METHOD PATH TOKEN BODY_JSON -- calls a $GATEWAY_URL$PATH
# wallet/system endpoint (managed account create, default-account update,
# approvals list/approve, import-recovery-key). BODY_JSON may be empty for
# a bodyless GET.
wallet_call() {
    local method="$1" path="$2" token="$3" body="${4:-}"
    local extra=() __wbody_file=""
    if [ -n "$body" ]; then __wbody_file="$(mktemp)"; printf '%s' "$body" > "$__wbody_file"; extra=(-H "content-type: application/json" --data "@${__wbody_file}"); fi
    log "+ curl -X ${method} ${GATEWAY_URL}${path} -H 'x-elastos-home-token: $(redact_token "$token")'${body:+ --data '<redacted-body>'}"
    curl_call "$method" "${GATEWAY_URL}${path}" \
        -H "x-elastos-home-token: ${token}" \
        -H "Origin: null" \
        ${extra[@]+"${extra[@]}"}
    record_transcript "$method" "$path" "$body" "$CURL_HTTP_CODE" "$CURL_BODY"
    # Not `[ -n ] && rm`: on a bodyless call that list evaluates to 1 and
    # becomes the function's exit status, which aborts the driver under
    # `set -e` and trips the smoke's empty-body assertion.
    if [ -n "$__wbody_file" ]; then
        rm -f "$__wbody_file"
    fi
}

# eth_get_balance RPC_URL ADDRESS -- raw eth_getBalance JSON-RPC against
# one of the chain-provider's own evidence RPC endpoints; sets BALANCE_HEX
# ("0x0" on any RPC-level error, never treated as a hard failure since
# wallet-setup's whole point is to poll this until it stops being zero).
eth_get_balance() {
    local rpc_url="$1" address="$2"
    local body
    body="$(python3 -c 'import json,sys; print(json.dumps({"jsonrpc":"2.0","id":1,"method":"eth_getBalance","params":[sys.argv[1],"latest"]}))' "$address")"
    curl_call POST "$rpc_url" -H "content-type: application/json" --data "$body"
    BALANCE_HEX="$(printf '%s' "$CURL_BODY" | python3 -c 'import json,sys
try:
    d = json.load(sys.stdin)
    print(d.get("result") or "0x0")
except Exception:
    print("0x0")' 2>/dev/null || echo 0x0)"
}

docker_stop_service() {
    local svc="$1"
    log "+ docker compose -f '$COMPOSE_FILE' stop $svc"
    docker compose -f "$COMPOSE_FILE" stop "$svc" \
        || fail "docker compose stop failed for $svc; diagnostic: docker compose -f '$COMPOSE_FILE' ps $svc"
    push_restore_docker_start "$svc"
}

docker_start_service() {
    local svc="$1"
    log "+ docker compose -f '$COMPOSE_FILE' start $svc"
    docker compose -f "$COMPOSE_FILE" start "$svc" \
        || fail "docker compose start failed for $svc; diagnostic: docker compose -f '$COMPOSE_FILE' ps $svc"
    remove_restore_docker_start "$svc"
}

# --- restore stack (Important 4 in the fix round) -----------------------
#
# Parallel arrays, not an array-of-structs or associative array (bash 3.2
# has neither). Every docker_stop_service call pushes a "docker_start" entry
# here automatically; docker_start_service pops the matching entry when it
# runs (harmless if it doesn't -- `docker compose start` on an
# already-running container is a no-op). negative_tamper pushes/pops a
# "tamper" entry around its own byte-flip the same way. on_exit's
# run_restore_stack (see above) sweeps whatever is left, most-recent-first,
# so a failure on ANY path -- not just the specific assertion branches that
# already restore things inline -- still leaves the harness intact.

push_restore_docker_start() {
    RESTORE_KINDS+=("docker_start")
    RESTORE_ARG1+=("$1")
    RESTORE_ARG2+=("")
    RESTORE_ARG3+=("")
}

# Removes the most recently pushed matching entry (bash 3.2 array element
# removal: unset the index, then re-pack with a self-assignment -- unset
# alone leaves a hole in the index sequence).
remove_restore_docker_start() {
    local svc="$1" idx
    idx="${#RESTORE_KINDS[@]}"
    while [ "$idx" -gt 0 ]; do
        idx=$((idx - 1))
        if [ "${RESTORE_KINDS[$idx]}" = "docker_start" ] && [ "${RESTORE_ARG1[$idx]}" = "$svc" ]; then
            unset "RESTORE_KINDS[$idx]" "RESTORE_ARG1[$idx]" "RESTORE_ARG2[$idx]" "RESTORE_ARG3[$idx]"
            # Guarded expansion, not a bare "${arr[@]}" (Critical 1's own
            # class of bug): if this was the LAST remaining entry, the
            # array is now empty, and an unguarded "${RESTORE_KINDS[@]}"
            # here would itself be an unbound-variable error under bash
            # 3.2's `set -u` -- caught by dry-running this exact re-pack
            # down to zero elements during the fix round.
            RESTORE_KINDS=(${RESTORE_KINDS[@]+"${RESTORE_KINDS[@]}"})
            RESTORE_ARG1=(${RESTORE_ARG1[@]+"${RESTORE_ARG1[@]}"})
            RESTORE_ARG2=(${RESTORE_ARG2[@]+"${RESTORE_ARG2[@]}"})
            RESTORE_ARG3=(${RESTORE_ARG3[@]+"${RESTORE_ARG3[@]}"})
            return 0
        fi
    done
}

push_restore_tamper() {
    RESTORE_KINDS+=("tamper")
    RESTORE_ARG1+=("$1") # service
    RESTORE_ARG2+=("$2") # target path inside the container
    RESTORE_ARG3+=("$3") # local backup file holding the original bytes
}

remove_restore_tamper() {
    local svc="$1" target="$2" idx
    idx="${#RESTORE_KINDS[@]}"
    while [ "$idx" -gt 0 ]; do
        idx=$((idx - 1))
        if [ "${RESTORE_KINDS[$idx]}" = "tamper" ] && [ "${RESTORE_ARG1[$idx]}" = "$svc" ] && [ "${RESTORE_ARG2[$idx]}" = "$target" ]; then
            unset "RESTORE_KINDS[$idx]" "RESTORE_ARG1[$idx]" "RESTORE_ARG2[$idx]" "RESTORE_ARG3[$idx]"
            # Guarded expansion, not a bare "${arr[@]}" (Critical 1's own
            # class of bug): if this was the LAST remaining entry, the
            # array is now empty, and an unguarded "${RESTORE_KINDS[@]}"
            # here would itself be an unbound-variable error under bash
            # 3.2's `set -u` -- caught by dry-running this exact re-pack
            # down to zero elements during the fix round.
            RESTORE_KINDS=(${RESTORE_KINDS[@]+"${RESTORE_KINDS[@]}"})
            RESTORE_ARG1=(${RESTORE_ARG1[@]+"${RESTORE_ARG1[@]}"})
            RESTORE_ARG2=(${RESTORE_ARG2[@]+"${RESTORE_ARG2[@]}"})
            RESTORE_ARG3=(${RESTORE_ARG3[@]+"${RESTORE_ARG3[@]}"})
            return 0
        fi
    done
}

# Best-effort by design, same as write_failure_receipt: this runs from the
# EXIT trap, possibly during teardown of an already-failing script, so a
# restore failure is logged loudly (WARNING) but must never itself abort or
# mask the original failure.
run_restore_stack() {
    local n="${#RESTORE_KINDS[@]}"
    [ "$n" -gt 0 ] || return 0
    log "running $n pending restore action(s) left by an interrupted drill/tamper phase"
    local idx
    idx="$n"
    while [ "$idx" -gt 0 ]; do
        idx=$((idx - 1))
        case "${RESTORE_KINDS[$idx]}" in
        docker_start)
            log "+ restore: docker compose -f '$COMPOSE_FILE' start ${RESTORE_ARG1[$idx]}"
            docker compose -f "$COMPOSE_FILE" start "${RESTORE_ARG1[$idx]}" 2>/dev/null \
                || log "WARNING: restore failed to start ${RESTORE_ARG1[$idx]} -- manual intervention required: docker compose -f '$COMPOSE_FILE' start ${RESTORE_ARG1[$idx]}"
            ;;
        tamper)
            local svc="${RESTORE_ARG1[$idx]}" target="${RESTORE_ARG2[$idx]}" backup="${RESTORE_ARG3[$idx]}"
            if [ -f "$backup" ]; then
                log "+ restore: write back original bytes to $svc:$target from $backup"
                docker compose -f "$COMPOSE_FILE" exec -T "$svc" sh -c "cat > '$target'" <"$backup" 2>/dev/null \
                    || log "WARNING: restore failed to write back $target on $svc from $backup -- manual intervention required"
            else
                log "WARNING: tamper backup $backup is gone -- cannot restore $target on $svc; manual intervention required"
            fi
            ;;
        *)
            log "WARNING: unknown restore-stack entry kind '${RESTORE_KINDS[$idx]}', skipping"
            ;;
        esac
    done
    RESTORE_KINDS=()
    RESTORE_ARG1=()
    RESTORE_ARG2=()
    RESTORE_ARG3=()
}

# Later phases (buy/open/drills/negative/restart/cleanup) reuse the mint_id
# the mint phase recorded, so an operator never has to retype a 32-byte hex
# id across separate invocations. --mint-id always wins when given.
resolve_mint_id() {
    if [ -n "$MINT_ID" ]; then
        return 0
    fi
    [ -f "$RECEIPT_PATH" ] || fail "no --mint-id given and no receipt at '$RECEIPT_PATH' to read one from; run --phase mint first or pass --mint-id"
    MINT_ID="$(python3 -c '
import json
import sys
try:
    with open(sys.argv[1]) as handle:
        receipt = json.load(handle)
    print(receipt.get("mint", {}).get("mint_id") or "")
except Exception:
    print("")
' "$RECEIPT_PATH")"
    [ -n "$MINT_ID" ] || fail "receipt at '$RECEIPT_PATH' has no mint.mint_id; run --phase mint first or pass --mint-id"
}

resolve_cid() {
    if [ -n "$CID" ]; then
        return 0
    fi
    [ -f "$RECEIPT_PATH" ] || fail "no --cid given and no receipt at '$RECEIPT_PATH' to read one from; run --phase mint first or pass --cid"
    CID="$(python3 -c '
import json
import sys
try:
    with open(sys.argv[1]) as handle:
        receipt = json.load(handle)
    print(receipt.get("mint", {}).get("cid") or "")
except Exception:
    print("")
' "$RECEIPT_PATH")"
    [ -n "$CID" ] || fail "receipt at '$RECEIPT_PATH' has no mint.cid; run --phase mint first or pass --cid"
}

# find_availability_fields JSON_TEXT -- exact key path pinned from source
# (content.rs's content-provider "status" op handler, content_cmd.rs's
# Status CLI arm just wraps it verbatim): for a --cid whose receipt exists,
# `elastos content status --cid <cid>` always prints
# {"status": "ok"/"error", "data": {..., "availability": {"replicas": <int>,
# "peer_selection": {"live_multi_peer_proof": <bool>, ...}, ...}}}
# (content.rs:4100-4116's `status()` builds exactly this "availability"
# object from the same AvailabilityReceipt fields the "publish" response
# uses -- confirmed against content.rs's own test assertions at
# response["data"]["availability"]["replicas"] /
# response["data"]["availability"]["peer_selection"]["live_multi_peer_proof"],
# e.g. :9230-9237/:9556). No longer a tolerant whole-document walk (Important
# 8 in the fix round): missing/non-numeric/non-boolean here is a genuine
# schema-drift or degraded-availability signal, never something to paper
# over, so this only ever emits an integer or the literal "null" (never a
# generic error status masked by `2>/dev/null` truthiness) -- callers MUST
# hard-fail on "null" in both the healthy and the degraded direction (see
# assert_content_status_healthy and phase_drill_replica).
find_availability_fields() {
    local json_text="$1"
    printf '%s' "$json_text" | python3 -c '
import json
import sys

try:
    doc = json.loads(sys.stdin.read())
except Exception:
    print("null null")
    raise SystemExit(0)

availability = {}
if isinstance(doc, dict):
    data = doc.get("data")
    if isinstance(data, dict):
        candidate = data.get("availability")
        if isinstance(candidate, dict):
            availability = candidate

replicas = availability.get("replicas")
peer_selection = availability.get("peer_selection")
live = peer_selection.get("live_multi_peer_proof") if isinstance(peer_selection, dict) else None

replicas_out = replicas if isinstance(replicas, int) and not isinstance(replicas, bool) else "null"
live_out = "true" if live is True else ("false" if live is False else "null")
print(f"{replicas_out} {live_out}")
'
}

# find_string_field JSON_TEXT KEY -- first string value found anywhere in
# the document under KEY (depth-first), or empty. Used where a response
# field's exact nesting was not pinned to one path from source (mint_id in
# the publish response, in particular).
find_string_field() {
    local json_text="$1" key="$2"
    printf '%s' "$json_text" | python3 -c '
import json
import sys

key = sys.argv[1]

def walk(node):
    if isinstance(node, dict):
        if key in node and isinstance(node[key], str) and node[key]:
            return node[key]
        for value in node.values():
            found = walk(value)
            if found:
                return found
    elif isinstance(node, list):
        for item in node:
            found = walk(item)
            if found:
                return found
    return None

try:
    doc = json.loads(sys.stdin.read())
except Exception:
    print("")
    raise SystemExit(0)
print(walk(doc) or "")
' "$key"
}

# elastos content status --cid <cid> (CLI, not HTTP -- content_cmd.rs's
# Status arm) then asserts replicas>=3 and live_multi_peer_proof=true via
# find_availability_fields. Sets CONTENT_STATUS_JSON.
# assert_content_status_healthy_or_report CID -- same checks as
# assert_content_status_healthy but returns 1 instead of failing, so a
# caller holding a stopped node can restore it before aborting.
assert_content_status_healthy_or_report() {
    local cid="$1"
    log "+ $ELASTOS_BIN content status --cid $cid"
    CONTENT_STATUS_JSON="$("$ELASTOS_BIN" content status --cid "$cid")" || return 1
    local fields replicas live
    fields="$(find_availability_fields "$CONTENT_STATUS_JSON")"
    replicas="$(printf '%s' "$fields" | awk '{print $1}')"
    live="$(printf '%s' "$fields" | awk '{print $2}')"
    [ "$replicas" != "null" ] || return 1
    [ "$replicas" -ge 3 ] || return 1
    [ "$live" = "true" ] || return 1
    log "content status for $cid healthy: replicas=$replicas live_multi_peer_proof=$live"
    return 0
}

assert_content_status_healthy() {
    local cid="$1"
    log "+ $ELASTOS_BIN content status --cid $cid"
    CONTENT_STATUS_JSON="$("$ELASTOS_BIN" content status --cid "$cid")" \
        || fail "elastos content status --cid $cid failed; diagnostic: $ELASTOS_BIN content status --cid $cid"
    local fields replicas live
    fields="$(find_availability_fields "$CONTENT_STATUS_JSON")"
    replicas="$(printf '%s' "$fields" | awk '{print $1}')"
    live="$(printf '%s' "$fields" | awk '{print $2}')"
    if [ "$replicas" = "null" ]; then
        fail "content status for $cid has no numeric data.availability.replicas (schema drift or missing receipt?); diagnostic: $ELASTOS_BIN content status --cid $cid -- $CONTENT_STATUS_JSON"
    fi
    if [ "$replicas" -lt 3 ]; then
        fail "content status for $cid does not show replicas>=3 (got '$replicas'); diagnostic: $ELASTOS_BIN content status --cid $cid -- $CONTENT_STATUS_JSON"
    fi
    if [ "$live" != "true" ]; then
        fail "content status for $cid does not show data.availability.peer_selection.live_multi_peer_proof=true (got '$live'); diagnostic: $ELASTOS_BIN content status --cid $cid -- $CONTENT_STATUS_JSON"
    fi
    log "content status for $cid healthy: replicas=$replicas live_multi_peer_proof=$live"
}

# --- provision phase ------------------------------------------------------

verify_install_state() {
    local receipt="${CLIENT_DATA_DIR}/receipts/source-home-installation.json"
    local head_commit
    head_commit="$(git -C "$ROOT" rev-parse --verify HEAD)"
    INSTALLED_COMMIT=""
    if [ -f "$receipt" ]; then
        INSTALLED_COMMIT="$(python3 -c '
import json
import sys
try:
    with open(sys.argv[1]) as handle:
        data = json.load(handle)
    print(data.get("source", {}).get("commit") or "")
except Exception:
    print("")
' "$receipt" 2>/dev/null || true)"
    fi
    if [ -n "$INSTALLED_COMMIT" ] && [ "$INSTALLED_COMMIT" = "$head_commit" ]; then
        INSTALL_FROM_TREE=1
    else
        INSTALL_FROM_TREE=0
    fi
}

ensure_installed_from_this_tree() {
    verify_install_state
    if [ "$INSTALL_FROM_TREE" = "1" ]; then
        log "installed source-home already matches this working tree (commit ${INSTALLED_COMMIT})"
        INSTALL_OK=1
        SETUP_RAN=false
        SETUP_WALL_SECONDS=0
        return 0
    fi
    log "installed source-home commit ('${INSTALLED_COMMIT:-<none>}') does not match this working tree HEAD; running scripts/setup-source-home.sh"
    SETUP_RAN=true
    local start end
    start="$(date +%s)"
    # This proof provisions protected-content prerequisites only, not a
    # multi-node collaboration cluster, so "isolated" (no shared startup
    # config) is the correct default; respect an operator override if one
    # is already exported.
    if ELASTOS_COLLABORATION_STARTUP_MODE="${ELASTOS_COLLABORATION_STARTUP_MODE:-isolated}" \
        "${ROOT}/scripts/setup-source-home.sh"; then
        SETUP_EXIT=0
    else
        SETUP_EXIT=$?
    fi
    end="$(date +%s)"
    SETUP_WALL_SECONDS=$((end - start))
    log "scripts/setup-source-home.sh wall time: ${SETUP_WALL_SECONDS}s (exit ${SETUP_EXIT})"
    verify_install_state
    if [ "$INSTALL_FROM_TREE" = "1" ]; then
        INSTALL_OK=1
    else
        INSTALL_OK=0
        log "GAP: installed source-home still does not match this working tree after setup-source-home.sh (installed commit: '${INSTALLED_COMMIT:-<none>}', HEAD: ${head_commit:-$(git -C "$ROOT" rev-parse --verify HEAD)}); diagnostic: scripts/setup-source-home.sh; skipping client restart"
    fi
}

ensure_policy_authority_key() {
    local dir
    dir="$(dirname "$POLICY_AUTHORITY_KEY")"
    mkdir -p "$dir"
    chmod 700 "$dir"
    if [ -e "$POLICY_AUTHORITY_KEY" ]; then
        log "reusing existing policy authority key at $POLICY_AUTHORITY_KEY"
        POLICY_AUTHORITY_KEY_CREATED=false
    else
        log "+ $ELASTOS_BIN protected-content-config create-policy-authority-key --key $POLICY_AUTHORITY_KEY"
        "$ELASTOS_BIN" protected-content-config create-policy-authority-key --key "$POLICY_AUTHORITY_KEY" \
            || fail "create-policy-authority-key failed; diagnostic: $ELASTOS_BIN protected-content-config create-policy-authority-key --key '$POLICY_AUTHORITY_KEY'"
        POLICY_AUTHORITY_KEY_CREATED=true
    fi
}

register_peers() {
    local i did ticket_path ticket label
    for i in "${!DESCRIPTOR_DIDS[@]}"; do
        did="${DESCRIPTOR_DIDS[$i]}"
        ticket_path="${SHARED_DIR}/${did}.ticket"
        [ -f "$ticket_path" ] || fail "missing ticket for $did; diagnostic: ls '$SHARED_DIR'/${did}.ticket"
        ticket="$(cat "$ticket_path")"
        label="custody-node-$((i + 1))"
        log "+ $ELASTOS_BIN node peer add --did $did --label $label --ticket <redacted> --json"
        # `node peer add` is documented "Add or update a known operator
        # peer" -- re-running for an already-registered DID updates in
        # place rather than failing, so this loop is safe to repeat.
        "$ELASTOS_BIN" node peer add --did "$did" --label "$label" --ticket "$ticket" --json >/dev/null \
            || fail "node peer add failed for $did; diagnostic: $ELASTOS_BIN node peer add --did $did --label $label --ticket <ticket-from ${ticket_path}> --json"
    done
}

generate_composition() {
    local composition_path="${CLIENT_DATA_DIR}/protected-content/custody-composition.json"
    if [ -e "$composition_path" ]; then
        log "custody composition already installed at $composition_path; skipping ceremony (remove it to force re-generation)"
        return 0
    fi
    local node_args=() p
    for p in "${DESCRIPTOR_PATHS[@]}"; do node_args+=(--node "$p"); done
    log "+ $ELASTOS_BIN protected-content-config generate-custody-composition --authority-key $POLICY_AUTHORITY_KEY ${node_args[*]} --data-dir '$CLIENT_DATA_DIR' --valid-days $VALID_DAYS"
    "$ELASTOS_BIN" protected-content-config generate-custody-composition \
        --authority-key "$POLICY_AUTHORITY_KEY" \
        "${node_args[@]}" \
        --data-dir "$CLIENT_DATA_DIR" \
        --valid-days "$VALID_DAYS" \
        || fail "generate-custody-composition failed; diagnostic: $ELASTOS_BIN protected-content-config generate-custody-composition --authority-key '$POLICY_AUTHORITY_KEY' ${node_args[*]} --data-dir '$CLIENT_DATA_DIR' --valid-days $VALID_DAYS"
}

generate_chain_config() {
    local chain_path="${CLIENT_DATA_DIR}/protected-content/chain-provider.json"
    if [ -e "$chain_path" ]; then
        log "chain provider config already installed at $chain_path; skipping generation (remove it to force re-generation)"
        return 0
    fi
    local evidence_args=() u
    for u in "${CHAIN_EVIDENCE_RPC_URLS[@]}"; do evidence_args+=(--evidence-rpc-url "$u"); done
    log "+ $ELASTOS_BIN protected-content-config generate-chain-config --data-dir '$CLIENT_DATA_DIR' --rpc-url $CHAIN_RPC_URL ${evidence_args[*]} --mint-ledger $CHAIN_MINT_LEDGER --mint-pay-token $CHAIN_MINT_PAY_TOKEN --mint-asset-created-emitter $CHAIN_MINT_ASSET_CREATED_EMITTER"
    "$ELASTOS_BIN" protected-content-config generate-chain-config \
        --data-dir "$CLIENT_DATA_DIR" \
        --rpc-url "$CHAIN_RPC_URL" \
        "${evidence_args[@]}" \
        --mint-ledger "$CHAIN_MINT_LEDGER" \
        --mint-pay-token "$CHAIN_MINT_PAY_TOKEN" \
        --mint-asset-created-emitter "$CHAIN_MINT_ASSET_CREATED_EMITTER" \
        || fail "generate-chain-config failed; diagnostic: $ELASTOS_BIN protected-content-config generate-chain-config --data-dir '$CLIENT_DATA_DIR' --rpc-url '$CHAIN_RPC_URL' ${evidence_args[*]} --mint-ledger $CHAIN_MINT_LEDGER --mint-pay-token $CHAIN_MINT_PAY_TOKEN --mint-asset-created-emitter $CHAIN_MINT_ASSET_CREATED_EMITTER"
}

verify_composition() {
    log "+ $ELASTOS_BIN protected-content-config verify-custody-composition --data-dir '$CLIENT_DATA_DIR'"
    VERIFY_COMPOSITION_JSON="$("$ELASTOS_BIN" protected-content-config verify-custody-composition --data-dir "$CLIENT_DATA_DIR")" \
        || fail "verify-custody-composition failed; diagnostic: $ELASTOS_BIN protected-content-config verify-custody-composition --data-dir '$CLIENT_DATA_DIR'"
    log "verify-custody-composition: $VERIFY_COMPOSITION_JSON"
}

# mac-source-home-restart.sh always computes its own data dir as
# "<--test-home>/Library/Application Support/elastos" -- it has no direct
# --data-dir flag. So --test-home must be CLIENT_DATA_DIR with exactly that
# fixed suffix stripped back off, not always $HOME: a caller who passed a
# custom --client-data-dir but got the default $HOME here would have
# provisioning and restart silently target two different data dirs.
CLIENT_DATA_DIR_SUFFIX="/Library/Application Support/elastos"

restart_client() {
    if [ "$INSTALL_OK" != "1" ]; then
        log "skipping client restart: installed source-home does not match this working tree (see install-state GAP above)"
        RESTART_ATTEMPTED=false
        RESTART_OK=false
        return 0
    fi
    local test_home
    case "$CLIENT_DATA_DIR" in
    *"$CLIENT_DATA_DIR_SUFFIX")
        test_home="${CLIENT_DATA_DIR%"$CLIENT_DATA_DIR_SUFFIX"}"
        ;;
    *)
        fail "--client-data-dir '$CLIENT_DATA_DIR' does not end with '$CLIENT_DATA_DIR_SUFFIX'; scripts/mac-source-home-restart.sh always restarts <--test-home>${CLIENT_DATA_DIR_SUFFIX}, so a client data dir outside that fixed shape can never be the one it actually restarts. Pass --client-data-dir ending in '${CLIENT_DATA_DIR_SUFFIX}', or restart manually."
        ;;
    esac
    RESTART_ATTEMPTED=true
    log "+ ${ROOT}/scripts/mac-source-home-restart.sh --test-home '$test_home'"
    if "${ROOT}/scripts/mac-source-home-restart.sh" --test-home "$test_home"; then
        RESTART_OK=true
    else
        RESTART_OK=false
        log "GAP: mac-source-home-restart.sh failed; diagnostic: ${ROOT}/scripts/mac-source-home-restart.sh --test-home '$test_home' --dry-run"
    fi
}

run_static_audit() {
    detect_platform
    log "+ python3 ${ROOT}/scripts/protected-content-installed-static-audit.py --source-root '$ROOT' --installed-data-root '$CLIENT_DATA_DIR' --installed-runtime '$CLIENT_DATA_DIR/bin/elastos' --platform $PLATFORM --profile $PROFILE --role home"
    local stderr_tmp
    stderr_tmp="$(mktemp)"
    set +e
    STATIC_AUDIT_JSON="$(python3 "${ROOT}/scripts/protected-content-installed-static-audit.py" \
        --source-root "$ROOT" \
        --installed-data-root "$CLIENT_DATA_DIR" \
        --installed-runtime "${CLIENT_DATA_DIR}/bin/elastos" \
        --platform "$PLATFORM" \
        --profile "$PROFILE" \
        --role home 2>"$stderr_tmp")"
    STATIC_AUDIT_EXIT=$?
    STATIC_AUDIT_STDERR="$(cat "$stderr_tmp" 2>/dev/null || true)"
    rm -f "$stderr_tmp"
    set -e
    STATIC_AUDIT_READY="$(printf '%s' "$STATIC_AUDIT_JSON" | python3 -c 'import json,sys
try:
    d = json.load(sys.stdin)
    print("true" if d.get("ready_for_active_proof") else "false")
except Exception:
    print("false")' 2>/dev/null || echo false)"
    log "static audit exit=${STATIC_AUDIT_EXIT} ready_for_active_proof=${STATIC_AUDIT_READY}"
    if [ "$STATIC_AUDIT_READY" != "true" ]; then
        log "static audit receipt (verbatim): $STATIC_AUDIT_JSON"
        [ -z "$STATIC_AUDIT_STDERR" ] || log "static audit stderr: $STATIC_AUDIT_STDERR"
    fi
}

phase_provision() {
    CURRENT_STEP="check_git_clean"
    check_git_clean
    CURRENT_STEP="resolve_elastos_bin"
    resolve_elastos_bin
    CURRENT_STEP="git_identity"
    git_identity
    CURRENT_STEP="discover_descriptors"
    discover_descriptors

    CURRENT_STEP="ensure_installed_from_this_tree"
    ensure_installed_from_this_tree
    CURRENT_STEP="ensure_policy_authority_key"
    ensure_policy_authority_key
    CURRENT_STEP="register_peers"
    register_peers
    CURRENT_STEP="generate_composition"
    generate_composition
    CURRENT_STEP="generate_chain_config"
    generate_chain_config
    CURRENT_STEP="verify_composition"
    verify_composition
    CURRENT_STEP="restart_client"
    restart_client
    CURRENT_STEP="run_static_audit"
    run_static_audit
    CURRENT_STEP="compute_artifact_hashes"
    compute_artifact_hashes

    CURRENT_STEP="write_receipt_block:provision"
    # Reached only if every step above succeeded (each one calls fail() and
    # exits on a real problem) -- computed, not a bare literal, so it stays
    # wired to the same "ok" shape the EXIT trap's failure block uses.
    local provision_ok=true

    # Same argv + json.dumps pattern phase_preflight already used below:
    # every value crosses the bash/python boundary as a plain argument,
    # never spliced as raw text into a JSON literal, so a malformed value
    # (an unescaped quote in a path, say) surfaces as a normal Python
    # traceback under fail()'s diagnostic rather than a silently broken
    # receipt.
    local provision_block_json
    provision_block_json="$(python3 -c '
import json
import sys

(
    commit, tree, clean,
    client_data_dir,
    valid_days,
    policy_authority_key_path, policy_authority_key_created,
    descriptor_dids_json,
    install_from_this_tree, installed_commit,
    setup_source_home_ran, setup_source_home_wall_seconds,
    restart_attempted, restart_ok,
    verify_composition_json,
    static_audit_ready, static_audit_json,
    source_sha256, installed_sha256, artifact_parity,
    ok,
) = sys.argv[1:22]

print(json.dumps({
    "ok": json.loads(ok),
    "git": {"commit": commit, "tree": tree, "clean": json.loads(clean)},
    "client_data_dir": client_data_dir,
    "valid_days": int(valid_days),
    "policy_authority_key_path": policy_authority_key_path,
    "policy_authority_key_created": json.loads(policy_authority_key_created),
    "descriptor_dids": json.loads(descriptor_dids_json),
    "install_from_this_tree": json.loads(install_from_this_tree),
    "installed_commit": installed_commit or None,
    "setup_source_home_ran": json.loads(setup_source_home_ran),
    "setup_source_home_wall_seconds": int(setup_source_home_wall_seconds),
    "restart_attempted": json.loads(restart_attempted),
    "restart_ok": json.loads(restart_ok),
    "verify_composition": json.loads(verify_composition_json),
    "static_audit_ready_for_active_proof": json.loads(static_audit_ready),
    "static_audit_receipt": json.loads(static_audit_json),
    "artifacts": {
        "source_elastos_sha256": source_sha256 or None,
        "installed_elastos_sha256": installed_sha256 or None,
        "installed_artifact_parity": json.loads(artifact_parity),
    },
}))
' \
        "$GIT_COMMIT" "$GIT_TREE" "$GIT_CLEAN" \
        "$CLIENT_DATA_DIR" \
        "$VALID_DAYS" \
        "$POLICY_AUTHORITY_KEY" "$POLICY_AUTHORITY_KEY_CREATED" \
        "$(python3 -c 'import json,sys; print(json.dumps(sys.argv[1:]))' "${DESCRIPTOR_DIDS[@]}")" \
        "$([ "$INSTALL_FROM_TREE" = "1" ] && echo true || echo false)" "${INSTALLED_COMMIT:-}" \
        "$SETUP_RAN" "$SETUP_WALL_SECONDS" \
        "${RESTART_ATTEMPTED:-false}" "${RESTART_OK:-false}" \
        "$VERIFY_COMPOSITION_JSON" \
        "$STATIC_AUDIT_READY" "$STATIC_AUDIT_JSON" \
        "$SOURCE_ELASTOS_SHA256" "$INSTALLED_ELASTOS_SHA256" "$ARTIFACT_PARITY" \
        "$provision_ok")"
    write_receipt_block provision "$provision_block_json"

    log "provision phase complete"
}

# --- preflight phase --------------------------------------------------

preflight_assert_peers() {
    log "+ $ELASTOS_BIN node peer list --json"
    PEER_LIST_JSON="$("$ELASTOS_BIN" node peer list --json)" \
        || fail "node peer list failed; diagnostic: $ELASTOS_BIN node peer list --json"
    local count
    count="$(printf '%s' "$PEER_LIST_JSON" | python3 -c 'import json,sys; print(len(json.load(sys.stdin)))')"
    [ "$count" -eq 3 ] || fail "expected 3 registered operator peers, found $count; diagnostic: $ELASTOS_BIN node peer list --json"
    local did
    for did in "${DESCRIPTOR_DIDS[@]}"; do
        printf '%s' "$PEER_LIST_JSON" | python3 -c '
import json
import sys
peers = json.load(sys.stdin)
sys.exit(0 if any(p.get("did") == sys.argv[1] for p in peers) else 1)
' "$did" || fail "descriptor DID $did is not among the registered peers; diagnostic: $ELASTOS_BIN node peer list --json"
    done
    log "3 peer entries confirmed, matching the 3 live custody node descriptors"
}

# A dial that reaches the peer and gets a *signed* response back proves
# dial+dispatch at the transport level, even when that response is a
# denial (this client's requester DID isn't on the custody node's operator
# allowlist -- expected, since the real custody-plane crossing is Task 8's
# mint, not this preflight). Only a connection-level failure (never reached
# the peer) is a genuine preflight failure. Per Task 6's adjudication and
# verified live against this exact harness before writing this script.
DENIAL_RE='is not configured for operator control|is not allowed to perform|operator request was denied|response request_id mismatch'
CONNECT_FAIL_RE='operator connect failed|operator connection timed out|failed to bind operator client endpoint|invalid connect ticket'

dial_proof_one() {
    local did="$1" out rc
    log "+ $ELASTOS_BIN node status --peer $did --json"
    set +e
    out="$("$ELASTOS_BIN" node status --peer "$did" --json 2>&1)"
    rc=$?
    set -e
    if [ "$rc" -eq 0 ]; then
        log "dial proof $did -> pass (success): $out"
        DIAL_PROOF_RESULT="pass:success"
        DIAL_PROOF_DETAIL="$out"
        return 0
    fi
    if printf '%s' "$out" | grep -Eq "$DENIAL_RE"; then
        log "dial proof $did -> pass (signed denial, dial+dispatch proven): $out"
        DIAL_PROOF_RESULT="pass:denial"
        DIAL_PROOF_DETAIL="$out"
        return 0
    fi
    if printf '%s' "$out" | grep -Eq "$CONNECT_FAIL_RE"; then
        fail "dial proof for $did did not reach the peer (connection-level failure): $out ; diagnostic: $ELASTOS_BIN node status --peer $did --json"
    fi
    fail "dial proof for $did returned an unrecognized error (treating as failure -- neither a known signed denial nor a known connect failure): $out ; diagnostic: $ELASTOS_BIN node status --peer $did --json"
}

check_ready_receipt() {
    local svc="$1" out
    log "+ docker compose -f '$COMPOSE_FILE' exec -T $svc cat $CONTAINER_READY_PATH"
    out="$(docker compose -f "$COMPOSE_FILE" exec -T "$svc" cat "$CONTAINER_READY_PATH" 2>&1)" \
        || fail "$svc has no readiness receipt; diagnostic: docker compose -f '$COMPOSE_FILE' logs $svc"
    local carrier_bound
    carrier_bound="$(printf '%s' "$out" | python3 -c 'import json,sys
try:
    print(json.load(sys.stdin).get("carrier_bound",""))
except Exception:
    print("")' 2>/dev/null || true)"
    case "$carrier_bound" in
    *:4433) ;;
    *) fail "$svc ready receipt carrier_bound='$carrier_bound' does not end :4433; diagnostic: docker compose -f '$COMPOSE_FILE' exec -T $svc cat $CONTAINER_READY_PATH" ;;
    esac
    local providers_ok
    providers_ok="$(printf '%s' "$out" | python3 -c '
import json
import sys
try:
    d = json.load(sys.stdin)
    providers = set(d.get("providers", []))
    expected = {"custody-provider", "availability-provider", "ipfs-provider"}
    print("true" if expected.issubset(providers) else "false")
except Exception:
    print("false")
' 2>/dev/null || echo false)"
    [ "$providers_ok" = "true" ] || fail "$svc ready receipt is missing an expected provider; diagnostic: docker compose -f '$COMPOSE_FILE' exec -T $svc cat $CONTAINER_READY_PATH"
    READY_RECEIPT_JSON="$out"
    log "$svc ready receipt ok: $out"
}

phase_preflight() {
    CURRENT_STEP="check_git_clean"
    check_git_clean
    CURRENT_STEP="resolve_elastos_bin"
    resolve_elastos_bin
    CURRENT_STEP="git_identity"
    git_identity
    CURRENT_STEP="discover_descriptors"
    discover_descriptors
    CURRENT_STEP="compute_artifact_hashes"
    compute_artifact_hashes

    # Registration is provision's job (an explicit ceremony step, run once,
    # idempotently); preflight only asserts the pool it left behind.
    CURRENT_STEP="preflight_assert_peers"
    preflight_assert_peers

    local dial_entries=() did
    for did in "${DESCRIPTOR_DIDS[@]}"; do
        CURRENT_STEP="dial_proof:${did}"
        dial_proof_one "$did"
        dial_entries+=("$(python3 -c 'import json,sys; print(json.dumps({"did": sys.argv[1], "result": sys.argv[2], "detail": sys.argv[3]}))' "$did" "$DIAL_PROOF_RESULT" "$DIAL_PROOF_DETAIL")")
    done

    local ready_entries=() svc
    for svc in "${SERVICES[@]}"; do
        CURRENT_STEP="ready_receipt:${svc}"
        check_ready_receipt "$svc"
        ready_entries+=("$(python3 -c 'import json,sys; print(json.dumps({"service": sys.argv[1], "receipt": json.loads(sys.argv[2])}))' "$svc" "$READY_RECEIPT_JSON")")
    done

    CURRENT_STEP="verify_composition"
    verify_composition

    CURRENT_STEP="write_receipt_block:preflight"
    # Reached only if every step above succeeded (each one calls fail() and
    # exits on a real problem) -- a computed value, not a bare literal.
    PREFLIGHT_OK=true

    local preflight_block_json
    preflight_block_json="$(python3 -c '
import json
import sys
print(json.dumps({
    "ok": json.loads(sys.argv[12]),
    "git": {"commit": sys.argv[1], "tree": sys.argv[2], "clean": json.loads(sys.argv[3])},
    "client_data_dir": sys.argv[4],
    "peer_list": json.loads(sys.argv[5]),
    "dial_proofs": [json.loads(e) for e in json.loads(sys.argv[6])],
    "ready_receipts": [json.loads(e) for e in json.loads(sys.argv[7])],
    "verify_composition": json.loads(sys.argv[8]),
    "artifacts": {
        "source_elastos_sha256": sys.argv[9] or None,
        "installed_elastos_sha256": sys.argv[10] or None,
        "installed_artifact_parity": json.loads(sys.argv[11]),
    },
    "preflight_ok": json.loads(sys.argv[12]),
}))
' \
        "$GIT_COMMIT" "$GIT_TREE" "$GIT_CLEAN" "$CLIENT_DATA_DIR" \
        "$PEER_LIST_JSON" \
        "$(python3 -c 'import json,sys; print(json.dumps(sys.argv[1:]))' "${dial_entries[@]}")" \
        "$(python3 -c 'import json,sys; print(json.dumps(sys.argv[1:]))' "${ready_entries[@]}")" \
        "$VERIFY_COMPOSITION_JSON" \
        "$SOURCE_ELASTOS_SHA256" "$INSTALLED_ELASTOS_SHA256" "$ARTIFACT_PARITY" \
        "$PREFLIGHT_OK")"
    write_receipt_block preflight "$preflight_block_json"

    log "preflight phase complete: preflight_ok=true"
}

# --- chain-config-real phase --------------------------------------------
#
# Placeholder loopback RPCs/mint addresses that provision installed only
# ever prove *shape*, never dial out (see the module header). Pointing this
# harness at a real deployment needs this separate phase: it validates the
# operator-supplied real RPC URLs client-side (2..=5 distinct origins,
# exactly mirroring the server-side MIN/MAX_EVIDENCE_RPC_SOURCES check in
# protected_content_config.rs so a malformed --real-evidence-rpc-url list
# fails fast here instead of after a partial ceremony), removes the
# placeholder chain-provider.json (GenerateChainConfig is create-only, so a
# pre-existing file must be removed first to regenerate), and re-runs the
# ceremony's own PROVEN Base defaults (authority gateway
# 0x09dBe796f40ECEffEAccf243c3d758C4c1d8D87D, has_access_by_content_id
# selector 0x54d42821 -- protected_content_config.rs's
# PROVEN_BASE_AUTHORITY_GATEWAY/PROVEN_HAS_ACCESS_SELECTOR, which the Rust
# CLI already defaults `--rights-contract`/`--rights-selector`/
# `--authority-gateway-contract` to, so this script never needs to pass
# them) against the real RPC endpoints and real mint contract addresses.

PLACEHOLDER_CHAIN_MINT_LEDGER="0x0000000000000000000000000000000000000022"
PLACEHOLDER_CHAIN_MINT_PAY_TOKEN="0x0000000000000000000000000000000000000033"
PLACEHOLDER_CHAIN_MINT_ASSET_CREATED_EMITTER="0x0000000000000000000000000000000000000044"

# Pure argument-shape validation -- no network, no docker, no gateway. Runs
# before the ceremony is ever invoked, which is exactly what makes it
# smoke-testable without a live environment.
validate_real_chain_config_inputs() {
    [ -n "$REAL_RPC_URL" ] || fail "--real-rpc-url is required for --phase chain-config-real"
    local count="${#REAL_EVIDENCE_RPC_URLS[@]}"
    if [ "$count" -lt 2 ] || [ "$count" -gt 5 ]; then
        fail "--real-evidence-rpc-url must be given 2..=5 times (got $count); diagnostic: pass --real-evidence-rpc-url twice at minimum, up to 5 times, each a distinct origin"
    fi
    python3 -c '
import sys
import urllib.parse

urls = sys.argv[1:]
origins = []
for url in urls:
    parsed = urllib.parse.urlsplit(url)
    if not parsed.scheme or not parsed.netloc:
        print(f"invalid RPC URL (needs scheme and host): {url}", file=sys.stderr)
        raise SystemExit(1)
    origins.append(f"{parsed.scheme}://{parsed.netloc}")
if len(set(origins)) != len(origins):
    print("--real-rpc-url / --real-evidence-rpc-url origins must be pairwise distinct", file=sys.stderr)
    raise SystemExit(1)
' "$REAL_RPC_URL" "${REAL_EVIDENCE_RPC_URLS[@]}" \
        || fail "real RPC URL validation failed (see diagnostic above): --real-rpc-url '$REAL_RPC_URL' --real-evidence-rpc-url ${REAL_EVIDENCE_RPC_URLS[*]}"
    if [ "$CHAIN_MINT_LEDGER" = "$PLACEHOLDER_CHAIN_MINT_LEDGER" ] \
        || [ "$CHAIN_MINT_PAY_TOKEN" = "$PLACEHOLDER_CHAIN_MINT_PAY_TOKEN" ] \
        || [ "$CHAIN_MINT_ASSET_CREATED_EMITTER" = "$PLACEHOLDER_CHAIN_MINT_ASSET_CREATED_EMITTER" ]; then
        fail "--phase chain-config-real refuses provision's placeholder mint addresses; pass real --chain-mint-ledger / --chain-mint-pay-token / --chain-mint-asset-created-emitter"
    fi
}

phase_chain_config_real() {
    CURRENT_STEP="check_git_clean"
    check_git_clean
    CURRENT_STEP="validate_real_chain_config_inputs"
    validate_real_chain_config_inputs
    CURRENT_STEP="resolve_elastos_bin"
    resolve_elastos_bin
    CURRENT_STEP="git_identity"
    git_identity

    local chain_path="${CLIENT_DATA_DIR}/protected-content/chain-provider.json"
    local replaced_placeholder=false
    if [ -e "$chain_path" ]; then
        replaced_placeholder=true
        rm -f "$chain_path"
    fi

    CURRENT_STEP="generate_chain_config_real"
    local evidence_args=() u
    for u in "${REAL_EVIDENCE_RPC_URLS[@]}"; do evidence_args+=(--evidence-rpc-url "$u"); done
    log "+ $ELASTOS_BIN protected-content-config generate-chain-config --data-dir '$CLIENT_DATA_DIR' --rpc-url $REAL_RPC_URL ${evidence_args[*]} --mint-ledger $CHAIN_MINT_LEDGER --mint-pay-token $CHAIN_MINT_PAY_TOKEN --mint-asset-created-emitter $CHAIN_MINT_ASSET_CREATED_EMITTER"
    CHAIN_CONFIG_REAL_JSON="$("$ELASTOS_BIN" protected-content-config generate-chain-config \
        --data-dir "$CLIENT_DATA_DIR" \
        --rpc-url "$REAL_RPC_URL" \
        "${evidence_args[@]}" \
        --mint-ledger "$CHAIN_MINT_LEDGER" \
        --mint-pay-token "$CHAIN_MINT_PAY_TOKEN" \
        --mint-asset-created-emitter "$CHAIN_MINT_ASSET_CREATED_EMITTER")" \
        || fail "generate-chain-config (real) failed; diagnostic: $ELASTOS_BIN protected-content-config generate-chain-config --data-dir '$CLIENT_DATA_DIR' --rpc-url '$REAL_RPC_URL' ${evidence_args[*]} --mint-ledger $CHAIN_MINT_LEDGER --mint-pay-token $CHAIN_MINT_PAY_TOKEN --mint-asset-created-emitter $CHAIN_MINT_ASSET_CREATED_EMITTER"
    log "generate-chain-config (real): $CHAIN_CONFIG_REAL_JSON"

    CURRENT_STEP="verify_composition"
    verify_composition

    CURRENT_STEP="write_receipt_block:chain-config-real"
    local ok=true
    local block_json
    block_json="$(python3 -c '
import json
import sys

(commit, tree, clean, client_data_dir, rpc_url, evidence_count,
 replaced_placeholder, chain_config_json, verify_composition_json, ok) = sys.argv[1:11]
print(json.dumps({
    "ok": json.loads(ok),
    "git": {"commit": commit, "tree": tree, "clean": json.loads(clean)},
    "client_data_dir": client_data_dir,
    "rpc_url": rpc_url,
    "evidence_rpc_sources": int(evidence_count),
    "replaced_placeholder": json.loads(replaced_placeholder),
    "chain_config": json.loads(chain_config_json),
    "verify_composition": json.loads(verify_composition_json),
}))
' \
        "$GIT_COMMIT" "$GIT_TREE" "$GIT_CLEAN" "$CLIENT_DATA_DIR" "$REAL_RPC_URL" \
        "${#REAL_EVIDENCE_RPC_URLS[@]}" "$replaced_placeholder" \
        "$CHAIN_CONFIG_REAL_JSON" "$VERIFY_COMPOSITION_JSON" "$ok")"
    write_receipt_block chain_config_real "$block_json"

    log "chain-config-real phase complete; re-run --phase provision (or restart the client) so the running client picks up the new config"
}

# --- wallet-setup phase --------------------------------------------------

# setup_wallet_account LABEL TOKEN RECOVERY_KEY_JSON STEP_UP_TOKEN -- either
# imports RECOVERY_KEY_JSON (needs a fresh interactive STEP_UP_TOKEN; see
# the header comment's import-recovery-key finding) or, when no recovery
# key was given, creates a managed account for CHAIN_NAMESPACE. Sets
# WALLET_SETUP_ACCOUNT_ID/WALLET_SETUP_ADDRESS from the response's account
# matching CHAIN_NAMESPACE.
setup_wallet_account() {
    local label="$1" token="$2" recovery_key="$3" step_up_token="$4"
    local body
    if [ -n "$recovery_key" ]; then
        [ -n "$step_up_token" ] || fail "--${label}-recovery-key was given without --${label}-step-up-token; import-recovery-key needs a fresh interactive passkey step-up (see header comment)"
        body="$(python3 -c 'import json,sys; print(json.dumps({"step_up_token": sys.argv[1], "recovery_key": json.loads(sys.argv[2]), "label": sys.argv[3]}))' "$step_up_token" "$recovery_key" "${label}-imported")" \
            || fail "--${label}-recovery-key is not valid JSON"
        wallet_call POST "/api/apps/wallet/wallet/accounts/import-recovery-key" "$token" "$body"
    else
        body="$(python3 -c 'import json,sys; print(json.dumps({"chain_namespace": sys.argv[1], "label": sys.argv[2], "create_new": False}))' "$CHAIN_NAMESPACE" "${label}-managed")"
        wallet_call POST "/api/apps/wallet/wallet/managed" "$token" "$body"
    fi
    case "$CURL_HTTP_CODE" in
    2??) ;;
    *) fail "$label wallet account setup failed (http $CURL_HTTP_CODE): $CURL_BODY" ;;
    esac
    WALLET_SETUP_ACCOUNT_ID="$(printf '%s' "$CURL_BODY" | python3 -c '
import json
import sys
try:
    accounts = json.load(sys.stdin).get("accounts") or []
    matches = [a for a in accounts if a.get("chain_namespace") == sys.argv[1]]
    print(matches[-1]["account_id"] if matches else "")
except Exception:
    print("")
' "$CHAIN_NAMESPACE" 2>/dev/null || true)"
    WALLET_SETUP_ADDRESS="$(printf '%s' "$CURL_BODY" | python3 -c '
import json
import sys
try:
    accounts = json.load(sys.stdin).get("accounts") or []
    matches = [a for a in accounts if a.get("chain_namespace") == sys.argv[1]]
    print(matches[-1]["address"] if matches else "")
except Exception:
    print("")
' "$CHAIN_NAMESPACE" 2>/dev/null || true)"
    [ -n "$WALLET_SETUP_ACCOUNT_ID" ] && [ -n "$WALLET_SETUP_ADDRESS" ] \
        || fail "$label wallet account setup did not return an account for chain_namespace=$CHAIN_NAMESPACE: $CURL_BODY"
    log "$label managed wallet account: account_id=$WALLET_SETUP_ACCOUNT_ID address=$WALLET_SETUP_ADDRESS"
}

# require_principal_onboarded LABEL [home-auth curl-args...] -- launches the
# People capsule as that principal and reads GET /api/apps/people/summary
# (people_summary, gateway_home_system.rs): identity.recovery_readiness and
# identity.profile_readiness must both be "ready". Fails closed with the
# Home UI actions otherwise (see the call site in phase_wallet_setup).
require_principal_onboarded() {
    local label="$1"
    shift
    gateway_launch "people" "$@"
    local people_token="$LAUNCH_TOKEN"
    wallet_call GET "/api/apps/people/summary" "$people_token" ""
    case "$CURL_HTTP_CODE" in
    2??) ;;
    *) fail "$label people summary failed (http $CURL_HTTP_CODE): $CURL_BODY" ;;
    esac
    local readiness
    readiness="$(printf '%s' "$CURL_BODY" | python3 -c '
import json
import sys
try:
    identity = json.load(sys.stdin).get("identity") or {}
    recovery = (identity.get("recovery_readiness") or {}).get("status") or ""
    profile = (identity.get("profile_readiness") or {}).get("status") or ""
    print(f"{recovery} {profile}")
except Exception:
    print(" ")
' 2>/dev/null || echo " ")"
    local recovery_status="${readiness%% *}" profile_status="${readiness##* }"
    if [ "$recovery_status" != "ready" ] || [ "$profile_status" != "ready" ]; then
        fail "$label principal is not onboarded (recovery_readiness=${recovery_status:-unknown}, profile_readiness=${profile_status:-unknown}); open the Home as that principal and complete System -> Recovery (export the Full Recovery Bundle, then verify it by importing it) and People -> Profile (set a display name), then re-run --phase wallet-setup"
    fi
    log "$label principal onboarded: recovery_readiness=$recovery_status profile_readiness=$profile_status"
}

# poll_funding LABEL ADDRESS -- polls FUNDING_RPC_URL's eth_getBalance for
# ADDRESS every FUNDING_POLL_SECONDS until it stops being 0x0 or
# FUNDING_TIMEOUT_SECONDS elapses. This is the one genuinely user-in-the-
# loop wait in the whole driver: the operator must send the one-time
# funding transfer while this loop is running. Sets FUNDING_BALANCE_HEX.
poll_funding() {
    local label="$1" address="$2"
    log "waiting for the operator to fund $label ($address) -- polling $FUNDING_RPC_URL every ${FUNDING_POLL_SECONDS}s, timeout ${FUNDING_TIMEOUT_SECONDS}s"
    local waited=0
    while :; do
        eth_get_balance "$FUNDING_RPC_URL" "$address"
        if [ -n "$BALANCE_HEX" ] && [ "$BALANCE_HEX" != "0x0" ] && [ "$BALANCE_HEX" != "0x" ]; then
            log "$label funded: balance=$BALANCE_HEX"
            FUNDING_BALANCE_HEX="$BALANCE_HEX"
            return 0
        fi
        if [ "$waited" -ge "$FUNDING_TIMEOUT_SECONDS" ]; then
            fail "$label ($address) was not funded within ${FUNDING_TIMEOUT_SECONDS}s; diagnostic: transfer funds to $address, or re-run with a longer --funding-timeout"
        fi
        sleep "$FUNDING_POLL_SECONDS"
        waited=$((waited + FUNDING_POLL_SECONDS))
    done
}

# set_default_account LABEL TOKEN ACCOUNT_ID -- sets the eip155
# transaction_intent default. update_default_wallet_account
# (gateway_wallet_accounts.rs:213-224) mirrors this into a browser_connect
# default for the same account automatically for eip155 namespaces; there
# is no separate "set elacity_mint_v1" step -- that ABI is resolved
# automatically server-side from chain-provider.json's mint_ledger/
# mint_pay_token/mint_asset_created_emitter once a transaction_intent
# default account exists (gateway_provider_proxy.rs's
# describe_protected_content_creator_mint_source).
set_default_account() {
    local label="$1" token="$2" account_id="$3"
    local body
    body="$(python3 -c 'import json,sys; print(json.dumps({"account_id": sys.argv[1], "chain_namespace": sys.argv[2], "intent": "transaction_intent"}))' "$account_id" "$CHAIN_NAMESPACE")"
    wallet_call POST "/api/apps/wallet/wallet/default" "$token" "$body"
    case "$CURL_HTTP_CODE" in
    2??) ;;
    *) fail "$label default-account update failed (http $CURL_HTTP_CODE): $CURL_BODY" ;;
    esac
}

phase_wallet_setup() {
    CURRENT_STEP="check_git_clean"
    check_git_clean
    CURRENT_STEP="git_identity"
    git_identity

    CURRENT_STEP="compute_creator_auth_args"
    compute_creator_auth_args
    CURRENT_STEP="compute_buyer_auth_args"
    compute_buyer_auth_args
    CURRENT_STEP="compute_denial_auth_args"
    compute_denial_auth_args

    # The Runtime needs each principal's profile DID (mint reads the
    # creator's, open reads the buyer's), and a profile can only exist on a
    # recovery-ready root. Both are one-time Home UI onboarding steps that
    # only the passkey holder can perform (recovery export needs a passkey
    # step-up), so the driver cannot do them on the operator's behalf: it
    # checks them up front and fails closed with the exact actions instead
    # of letting the mint fail later with an opaque
    # "Runtime custody viewer release approval is unavailable".
    CURRENT_STEP="require_principal_onboarded:creator"
    require_principal_onboarded "creator" "${CREATOR_AUTH_ARGS[@]}"
    CURRENT_STEP="require_principal_onboarded:buyer"
    require_principal_onboarded "buyer" "${BUYER_AUTH_ARGS[@]}"
    CURRENT_STEP="require_principal_onboarded:denial"
    require_principal_onboarded "denial" "${DENIAL_AUTH_ARGS[@]}"

    CURRENT_STEP="gateway_launch:creator:wallet"
    gateway_launch "wallet" "${CREATOR_AUTH_ARGS[@]}"
    local creator_wallet_token="$LAUNCH_TOKEN"
    CURRENT_STEP="gateway_launch:buyer:wallet"
    gateway_launch "wallet" "${BUYER_AUTH_ARGS[@]}"
    local buyer_wallet_token="$LAUNCH_TOKEN"
    CURRENT_STEP="gateway_launch:denial:wallet"
    gateway_launch "wallet" "${DENIAL_AUTH_ARGS[@]}"
    local denial_wallet_token="$LAUNCH_TOKEN"

    CURRENT_STEP="setup_wallet_account:creator"
    setup_wallet_account "creator" "$creator_wallet_token" "$CREATOR_RECOVERY_KEY" "$CREATOR_STEP_UP_TOKEN"
    local creator_account_id="$WALLET_SETUP_ACCOUNT_ID" creator_address="$WALLET_SETUP_ADDRESS"
    CURRENT_STEP="setup_wallet_account:buyer"
    setup_wallet_account "buyer" "$buyer_wallet_token" "$BUYER_RECOVERY_KEY" "$BUYER_STEP_UP_TOKEN"
    local buyer_account_id="$WALLET_SETUP_ACCOUNT_ID" buyer_address="$WALLET_SETUP_ADDRESS"
    CURRENT_STEP="setup_wallet_account:denial"
    # The denial principal needs its own funded managed account too
    # (Important 3 in the fix round): drill-replica's degraded-buy
    # assertion runs `buy` from the denial principal while custody-a is
    # stopped, and that assertion is only a real proof of
    # verify_fresh_runtime_custody_availability's fail-closed behavior
    # (api/gateway_provider_proxy.rs:2986-2999) if the denial principal is
    # otherwise capable of a normal buy -- an unfunded principal's buy would
    # fail for wallet/settlement reasons on a HEALTHY topology too, which
    # would make that drill vacuous.
    setup_wallet_account "denial" "$denial_wallet_token" "$DENIAL_RECOVERY_KEY" "$DENIAL_STEP_UP_TOKEN"
    local denial_account_id="$WALLET_SETUP_ACCOUNT_ID" denial_address="$WALLET_SETUP_ADDRESS"

    log "CREATOR ADDRESS: $creator_address"
    log "BUYER ADDRESS:   $buyer_address"
    log "DENIAL ADDRESS:  $denial_address"
    log "fund all three addresses now (one-time transfer from the operator's existing funded account; the denial principal only needs dust) -- this phase waits until all three are funded"

    CURRENT_STEP="poll_funding:creator"
    poll_funding "creator" "$creator_address"
    local creator_balance_hex="$FUNDING_BALANCE_HEX"
    CURRENT_STEP="poll_funding:buyer"
    poll_funding "buyer" "$buyer_address"
    local buyer_balance_hex="$FUNDING_BALANCE_HEX"
    CURRENT_STEP="poll_funding:denial"
    poll_funding "denial" "$denial_address"
    local denial_balance_hex="$FUNDING_BALANCE_HEX"

    CURRENT_STEP="set_default_account:creator"
    set_default_account "creator" "$creator_wallet_token" "$creator_account_id"
    CURRENT_STEP="set_default_account:buyer"
    set_default_account "buyer" "$buyer_wallet_token" "$buyer_account_id"
    CURRENT_STEP="set_default_account:denial"
    set_default_account "denial" "$denial_wallet_token" "$denial_account_id"

    CURRENT_STEP="write_receipt_block:wallet-setup"
    local ok=true
    local block_json
    block_json="$(python3 -c '
import json
import sys

(commit, tree, clean, chain_namespace,
 creator_account_id, creator_address, creator_balance_hex,
 buyer_account_id, buyer_address, buyer_balance_hex,
 denial_account_id, denial_address, denial_balance_hex, ok) = sys.argv[1:15]
print(json.dumps({
    "ok": json.loads(ok),
    "git": {"commit": commit, "tree": tree, "clean": json.loads(clean)},
    "chain_namespace": chain_namespace,
    "creator": {"account_id": creator_account_id, "address": creator_address, "funded_balance_hex": creator_balance_hex},
    "buyer": {"account_id": buyer_account_id, "address": buyer_address, "funded_balance_hex": buyer_balance_hex},
    "denial": {"account_id": denial_account_id, "address": denial_address, "funded_balance_hex": denial_balance_hex},
}))
' \
        "$GIT_COMMIT" "$GIT_TREE" "$GIT_CLEAN" "$CHAIN_NAMESPACE" \
        "$creator_account_id" "$creator_address" "$creator_balance_hex" \
        "$buyer_account_id" "$buyer_address" "$buyer_balance_hex" \
        "$denial_account_id" "$denial_address" "$denial_balance_hex" "$ok")"
    write_receipt_block wallet_setup "$block_json"

    log "wallet-setup phase complete"
}

# --- shared wallet-approval helper (mint + buy) -------------------------

# settle_pending_call SCHEME OP TOKEN BODY PENDING_MARKER LABEL SYSTEM_TOKEN
# WALLET_TOKEN ACCOUNT_ID SINCE -- re-issues an idempotent provider call
# until it stops answering with PENDING_MARKER (status=ok, or any other
# error, ends the loop), bounded by SETTLE_TIMEOUT_SECONDS. A pending
# settlement can raise FURTHER wallet requests on the way (the creator tail
# adds the operative's operator approval for the market gateway after the
# mint lands), so every round also approves any new pending request for
# ACCOUNT_ID created after SINCE, through the same step-up hook. Leaves
# CURL_BODY/PROVIDER_STATUS as the last call.
settle_pending_call() {
    local scheme="$1" op="$2" token="$3" body="$4" marker="$5"
    local label="$6" system_token="$7" wallet_token="$8" account_id="$9" since="${10}"
    local waited=0
    while :; do
        provider_call "$scheme" "$op" "$token" "$body"
        [ "$PROVIDER_STATUS" = "ok" ] && return 0
        case "$CURL_BODY" in
        *"$marker"*) ;;
        *) return 0 ;;
        esac
        if approve_new_wallet_approval_if_any "$label" "$system_token" "$wallet_token" "$account_id" "$since"; then
            since="$(date +%s)"
            continue
        fi
        [ "$waited" -lt "$SETTLE_TIMEOUT_SECONDS" ] \
            || fail "$scheme/$op did not settle within ${SETTLE_TIMEOUT_SECONDS}s after wallet approval: $CURL_BODY"
        sleep "$SETTLE_POLL_SECONDS"
        waited=$((waited + SETTLE_POLL_SECONDS))
    done
}

# approve_new_wallet_approval_if_any LABEL SYSTEM_TOKEN WALLET_TOKEN
# ACCOUNT_ID SINCE -- one non-blocking look at the approvals list: approves
# (via the step-up hook) the newest pending request for ACCOUNT_ID created
# at or after SINCE and returns 0; returns 1 when there is none.
approve_new_wallet_approval_if_any() {
    local label="$1" system_token="$2" wallet_token="$3" account_id="$4" since="$5"
    wallet_call GET "/api/apps/system/wallet/approvals" "$system_token" ""
    case "$CURL_HTTP_CODE" in
    2??) ;;
    *) return 1 ;;
    esac
    local request_id
    request_id="$(printf '%s' "$CURL_BODY" | python3 -c '
import json
import sys
try:
    reqs = json.load(sys.stdin).get("approval_requests") or []
    matches = [r for r in reqs
               if r.get("status") == "pending" and r.get("account_id") == sys.argv[1]
               and int(r.get("created_at") or 0) >= int(sys.argv[2])]
    matches.sort(key=lambda r: int(r.get("created_at") or 0), reverse=True)
    print(matches[0]["request_id"] if matches else "")
except Exception:
    print("")
' "$account_id" "$since" 2>/dev/null || true)"
    [ -n "$request_id" ] || return 1
    approve_wallet_request "$label" "$wallet_token" "$request_id"
    return 0
}

# poll_and_approve_wallet_approval LABEL SYSTEM_TOKEN WALLET_TOKEN
# ACCOUNT_ID [TIMEOUT_SECONDS] [POLL_SECONDS] -- polls
# GET /api/apps/system/wallet/approvals (system_wallet_approvals,
# gateway_wallet_approvals.rs:3-13, needs a SYSTEM_CAPSULE_ID token) for a
# pending request whose account_id matches, then approves it via
# POST /api/apps/wallet/wallet/managed-approvals/:request_id/approve
# (wallet_app_managed_approval_approve, needs a WALLET_CAPSULE_ID token).
# Sets APPROVED_REQUEST_ID.
poll_and_approve_wallet_approval() {
    local label="$1" system_token="$2" wallet_token="$3" account_id="$4"
    local timeout="${5:-300}" poll_interval="${6:-5}" min_created_at="${7:-0}"
    local waited=0 request_id=""
    while :; do
        wallet_call GET "/api/apps/system/wallet/approvals" "$system_token" ""
        case "$CURL_HTTP_CODE" in
        2??) ;;
        *) fail "$label approvals list failed (http $CURL_HTTP_CODE): $CURL_BODY" ;;
        esac
        request_id="$(printf '%s' "$CURL_BODY" | python3 -c '
import json
import sys
try:
    reqs = json.load(sys.stdin).get("approval_requests") or []
    # Only requests raised by THIS operation: the approval store outlives
    # the mint journal, so an older pending request for the same account
    # (an aborted earlier run) must not be the one we approve. Newest wins.
    matches = [r for r in reqs
               if r.get("status") == "pending" and r.get("account_id") == sys.argv[1]
               and int(r.get("created_at") or 0) >= int(sys.argv[2])]
    matches.sort(key=lambda r: int(r.get("created_at") or 0), reverse=True)
    print(matches[0]["request_id"] if matches else "")
except Exception:
    print("")
' "$account_id" "$min_created_at" 2>/dev/null || true)"
        [ -n "$request_id" ] && break
        if [ "$waited" -ge "$timeout" ]; then
            fail "$label: no pending wallet approval for account_id=$account_id appeared within ${timeout}s: $CURL_BODY"
        fi
        sleep "$poll_interval"
        waited=$((waited + poll_interval))
    done
    approve_wallet_request "$label" "$wallet_token" "$request_id"
    APPROVED_REQUEST_ID="$request_id"
}

# approve_wallet_request LABEL WALLET_TOKEN REQUEST_ID -- approves one
# pending managed-wallet request with a fresh passkey step-up from the hook.
approve_wallet_request() {
    local label="$1" wallet_token="$2" request_id="$3"
    log "$label: approving wallet request $request_id"
    # approve_wallet_managed_request (gateway_wallet_approvals.rs) consumes a
    # passkey step-up bound to operation "wallet.approve" and the canonical
    # request {"request_id", "reason"}, begun from the same Wallet launch
    # that performs the approval -- so the hook gets this wallet token.
    local reason="protected-content-installed-e2e-proof" principal step_up_request step_up_token
    case "$label" in
    mint) principal="creator" ;;
    buy | open | restart) principal="buyer" ;;
    *) principal="$label" ;;
    esac
    [ -n "$STEP_UP_HOOK" ] || fail "$label: approving wallet request $request_id needs a fresh passkey step-up (\"fresh passkey verification is required to sign with a built-in wallet\"); either approve it in the Wallet app as the $principal principal while this phase polls, or pass --step-up-hook <cmd> (see the header comment)"
    step_up_request="$(python3 -c 'import json,sys; print(json.dumps({"request_id": sys.argv[1], "reason": sys.argv[2]}))' "$request_id" "$reason")"
    log "+ step-up hook: $STEP_UP_HOOK $principal wallet.approve <wallet-token> (request on stdin)"
    step_up_token="$(printf '%s' "$step_up_request" | $STEP_UP_HOOK "$principal" "wallet.approve" "$wallet_token")" \
        || fail "$label: step-up hook failed for wallet request $request_id"
    [ -n "$step_up_token" ] || fail "$label: step-up hook printed no token for wallet request $request_id"
    local approve_body
    approve_body="$(python3 -c 'import json,sys; print(json.dumps({"reason": sys.argv[1], "step_up_token": sys.argv[2]}))' "$reason" "$step_up_token")"
    wallet_call POST "/api/apps/wallet/wallet/managed-approvals/${request_id}/approve" "$wallet_token" "$approve_body"
    case "$CURL_HTTP_CODE" in
    2??) ;;
    *) fail "$label approval of $request_id failed (http $CURL_HTTP_CODE): $CURL_BODY" ;;
    esac
}

# --- mint phase ------------------------------------------------------

# The publish call that carries protection.mode=runtime_custody needs a
# signed on-chain mint transaction (elacity_mint_v1, resolved automatically
# from chain-provider.json -- see set_default_account's comment), so its
# first response may come back pending a wallet approval rather than
# already "ok". This helper issues publish once, and if the response isn't
# immediately "ok" with a cid, approves the pending request and re-issues
# the IDENTICAL publish call (same uri/if_revision/protection -- create-
# only and idempotent by design, exactly like the buy flow's documented
# replay-safety) to fetch the terminal response. This two-shot shape is a
# documented design choice, not verified against a live server this
# session -- see the task report.
library_publish_runtime_custody() {
    local library_token="$1" system_token="$2" wallet_token="$3" account_id="$4"
    local uri="$5" if_revision="$6" copies="$7" price="$8"
    local body
    # The Runtime's mint intent takes copies/price as canonical hex
    # quantities ("0x3", "0x38d7ea4c68000"); the Library app converts the
    # person's decimal input with decimalIntegerToHexQuantity before
    # publishing (capsules/library/browser/src/actions.js), so do the same
    # here -- a decimal string is rejected as an invalid selection.
    body="$(python3 -c 'import json,sys; print(json.dumps({"uri": sys.argv[1], "if_revision": sys.argv[2], "protection": {"mode": "runtime_custody", "copies": hex(int(sys.argv[3])), "price": hex(int(sys.argv[4]))}}))' "$uri" "$if_revision" "$copies" "$price")" \
        || fail "copies/price must be decimal integers (got copies='$copies' price='$price')"
    local publish_started_at
    publish_started_at="$(date +%s)"
    provider_call object publish "$library_token" "$body"
    if [ "$PROVIDER_STATUS" != "ok" ]; then
        # The two-shot approve+re-publish is ONLY valid for the creator-tail
        # pending state (Phase B: the on-chain mint awaits a managed-wallet
        # approval). Any other error -- notably a Phase-A media-preparation
        # failure ("settlement reconciliation", EffectPending) -- must fail
        # fast: no approval is ever enqueued for it, so polling would just
        # spin until timeout.
        case "$CURL_BODY" in
        *"pending exact Wallet or Chain settlement"*)
            log "mint publish pending creator-mint settlement; approving the wallet request and re-publishing"
            poll_and_approve_wallet_approval "mint" "$system_token" "$wallet_token" "$account_id" 300 5 "$publish_started_at"
            # Approval only unblocks the creator tail: the Wallet still has
            # to sign and broadcast, and the Chain to confirm. The publish
            # is idempotent for the same uri/if_revision/protection, so
            # re-issue it until it settles (bounded).
            settle_pending_call object publish "$library_token" "$body" "pending exact Wallet or Chain settlement" \
                "mint" "$system_token" "$wallet_token" "$account_id" "$publish_started_at"
            ;;
        *)
            fail "library publish (runtime_custody) failed and is not a pending-approval response (won't poll): $CURL_BODY"
            ;;
        esac
    fi
    [ "$PROVIDER_STATUS" = "ok" ] || fail "library publish (protection=runtime_custody) did not reach status=ok: $CURL_BODY"
    PUBLISH_RESPONSE_JSON="$CURL_BODY"
}

# creator_mint_content_item URI_SUFFIX -- shared by phase_mint and
# phase_restart (which mints its own throwaway item so the restart-mid-
# purchase drill has a genuinely fresh, not-yet-purchased mint to
# interrupt). Requires resolve_elastos_bin, compute_creator_auth_args, and
# a wallet-setup receipt to already be in effect. Sets MINTED_CID/
# MINTED_MINT_ID/MINTED_URI/MINTED_PUBLISH_JSON.
creator_mint_content_item() {
    local uri_suffix="$1"
    [ -n "$CONTENT_PATH" ] || fail "--content-path is required to mint a content item"
    [ -f "$CONTENT_PATH" ] || fail "--content-path '$CONTENT_PATH' does not exist"

    CURRENT_STEP="gateway_launch:creator:library"
    gateway_launch "library" "${CREATOR_AUTH_ARGS[@]}"
    local library_token="$LAUNCH_TOKEN"
    CURRENT_STEP="gateway_launch:creator:system"
    gateway_launch "system" "${CREATOR_AUTH_ARGS[@]}"
    local system_token="$LAUNCH_TOKEN"
    CURRENT_STEP="gateway_launch:creator:wallet"
    gateway_launch "wallet" "${CREATOR_AUTH_ARGS[@]}"
    local wallet_token="$LAUNCH_TOKEN"

    CURRENT_STEP="library_roots"
    provider_call object roots "$library_token" '{}'
    [ "$PROVIDER_STATUS" = "ok" ] || fail "library roots failed: $CURL_BODY"
    local public_uri
    public_uri="$(printf '%s' "$CURL_BODY" | python3 -c '
import json
import sys
try:
    roots = json.load(sys.stdin).get("data", {}).get("roots") or []
    matches = [r for r in roots if r.get("id") == "public"]
    print(matches[0]["uri"] if matches else "")
except Exception:
    print("")
' 2>/dev/null || true)"
    [ -n "$public_uri" ] || fail "library roots did not expose Public: $CURL_BODY"

    CURRENT_STEP="library_write"
    local stamp file_uri data_b64
    stamp="$(date -u +%Y%m%dT%H%M%SZ)-$$"
    file_uri="${public_uri}/protected-content-installed-e2e-proof-${uri_suffix}${stamp}"
    data_b64="$(base64 <"$CONTENT_PATH" | tr -d '\n')"
    local write_body
    write_body="$(printf '%s' "$data_b64" | python3 -c 'import json,sys; print(json.dumps({"uri": sys.argv[1], "data": sys.stdin.read()}))' "$file_uri")"
    provider_call object write "$library_token" "$write_body"
    [ "$PROVIDER_STATUS" = "ok" ] || fail "library write failed: $CURL_BODY"
    local revision
    revision="$(find_string_field "$CURL_BODY" revision)"
    [ -n "$revision" ] || fail "library write did not return a revision: $CURL_BODY"

    CURRENT_STEP="resolve_creator_account_id"
    # account_id used to disambiguate pending wallet approvals: read it
    # back from the receipt wallet-setup wrote (creator.account_id).
    [ -f "$RECEIPT_PATH" ] || fail "no wallet-setup receipt at '$RECEIPT_PATH'; run --phase wallet-setup first"
    local creator_account_id
    creator_account_id="$(python3 -c '
import json
import sys
try:
    with open(sys.argv[1]) as handle:
        receipt = json.load(handle)
    print(receipt.get("wallet_setup", {}).get("creator", {}).get("account_id") or "")
except Exception:
    print("")
' "$RECEIPT_PATH")"
    [ -n "$creator_account_id" ] || fail "receipt at '$RECEIPT_PATH' has no wallet_setup.creator.account_id; run --phase wallet-setup first"

    CURRENT_STEP="library_publish_runtime_custody"
    library_publish_runtime_custody "$library_token" "$system_token" "$wallet_token" "$creator_account_id" \
        "$file_uri" "$revision" "$COPIES" "$PRICE_WEI"

    MINTED_CID="$(find_string_field "$PUBLISH_RESPONSE_JSON" cid)"
    MINTED_MINT_ID="$(find_string_field "$PUBLISH_RESPONSE_JSON" mint_id)"
    [ -n "$MINTED_CID" ] || fail "mint publish did not return a cid: $PUBLISH_RESPONSE_JSON"
    [ -n "$MINTED_MINT_ID" ] || fail "mint publish did not return a mint_id: $PUBLISH_RESPONSE_JSON"
    MINTED_URI="$file_uri"
    MINTED_PUBLISH_JSON="$PUBLISH_RESPONSE_JSON"
    log "minted cid=$MINTED_CID mint_id=$MINTED_MINT_ID uri=$MINTED_URI"
}

phase_mint() {
    CURRENT_STEP="check_git_clean"
    check_git_clean
    CURRENT_STEP="require_content_path"
    [ -n "$CONTENT_PATH" ] || fail "--content-path is required for --phase mint"
    [ -f "$CONTENT_PATH" ] || fail "--content-path '$CONTENT_PATH' does not exist"
    CURRENT_STEP="resolve_elastos_bin"
    resolve_elastos_bin
    CURRENT_STEP="git_identity"
    git_identity
    CURRENT_STEP="compute_creator_auth_args"
    compute_creator_auth_args

    CURRENT_STEP="creator_mint_content_item"
    creator_mint_content_item ""

    CURRENT_STEP="assert_content_status_healthy"
    assert_content_status_healthy "$MINTED_CID"

    CURRENT_STEP="write_receipt_block:mint"
    local ok=true
    local block_json
    block_json="$(python3 -c '
import json
import sys

(commit, tree, clean, uri, cid, mint_id, copies, price, publish_json, status_json, ok) = sys.argv[1:12]
print(json.dumps({
    "ok": json.loads(ok),
    "git": {"commit": commit, "tree": tree, "clean": json.loads(clean)},
    "uri": uri,
    "cid": cid,
    "mint_id": mint_id,
    "copies": copies,
    "price_wei": price,
    "publish_response": json.loads(publish_json),
    "content_status": json.loads(status_json),
}))
' \
        "$GIT_COMMIT" "$GIT_TREE" "$GIT_CLEAN" "$MINTED_URI" "$MINTED_CID" "$MINTED_MINT_ID" "$COPIES" "$PRICE_WEI" \
        "$MINTED_PUBLISH_JSON" "$CONTENT_STATUS_JSON" "$ok")"
    write_receipt_block mint "$block_json"

    log "mint phase complete: cid=$MINTED_CID mint_id=$MINTED_MINT_ID"
}

# --- availability phase --------------------------------------------------

phase_availability() {
    CURRENT_STEP="check_git_clean"
    check_git_clean
    CURRENT_STEP="resolve_cid"
    resolve_cid
    CURRENT_STEP="resolve_elastos_bin"
    resolve_elastos_bin
    CURRENT_STEP="git_identity"
    git_identity

    CURRENT_STEP="assert_content_status_healthy"
    assert_content_status_healthy "$CID"

    CURRENT_STEP="write_receipt_block:availability"
    local ok=true
    local block_json
    block_json="$(python3 -c '
import json
import sys

(commit, tree, clean, cid, status_json, ok) = sys.argv[1:7]
print(json.dumps({
    "ok": json.loads(ok),
    "git": {"commit": commit, "tree": tree, "clean": json.loads(clean)},
    "cid": cid,
    "content_status": json.loads(status_json),
}))
' "$GIT_COMMIT" "$GIT_TREE" "$GIT_CLEAN" "$CID" "$CONTENT_STATUS_JSON" "$ok")"
    write_receipt_block availability "$block_json"

    log "availability phase complete: cid=$CID"
}

# --- buy phase -------------------------------------------------------

# buy routes through either LIBRARY_CAPSULE_ID or MARKETPLACE_CAPSULE_ID
# per the op table (gateway_provider_proxy.rs:1509-1540); this driver
# always launches "library" for it, matching the mint phase's own capsule
# and scripts/library-live-smoke.sh's precedent.
phase_buy() {
    CURRENT_STEP="check_git_clean"
    check_git_clean
    CURRENT_STEP="git_identity"
    git_identity
    CURRENT_STEP="resolve_mint_id"
    resolve_mint_id
    CURRENT_STEP="compute_buyer_auth_args"
    compute_buyer_auth_args

    CURRENT_STEP="gateway_launch:buyer:library"
    gateway_launch "library" "${BUYER_AUTH_ARGS[@]}"
    local library_token="$LAUNCH_TOKEN"
    CURRENT_STEP="gateway_launch:buyer:system"
    gateway_launch "system" "${BUYER_AUTH_ARGS[@]}"
    local system_token="$LAUNCH_TOKEN"
    CURRENT_STEP="gateway_launch:buyer:wallet"
    gateway_launch "wallet" "${BUYER_AUTH_ARGS[@]}"
    local wallet_token="$LAUNCH_TOKEN"

    CURRENT_STEP="resolve_buyer_account_id"
    [ -f "$RECEIPT_PATH" ] || fail "no wallet-setup receipt at '$RECEIPT_PATH'; run --phase wallet-setup first"
    local buyer_account_id
    buyer_account_id="$(python3 -c '
import json
import sys
try:
    with open(sys.argv[1]) as handle:
        receipt = json.load(handle)
    print(receipt.get("wallet_setup", {}).get("buyer", {}).get("account_id") or "")
except Exception:
    print("")
' "$RECEIPT_PATH")"
    [ -n "$buyer_account_id" ] || fail "receipt at '$RECEIPT_PATH' has no wallet_setup.buyer.account_id; run --phase wallet-setup first"

    CURRENT_STEP="buy"
    local buy_body
    buy_body="$(python3 -c 'import json,sys; print(json.dumps({"mint_id": sys.argv[1]}))' "$MINT_ID")"
    local buy_started_at
    buy_started_at="$(date +%s)"
    provider_call object buy "$library_token" "$buy_body"
    if [ "$PROVIDER_STATUS" != "ok" ]; then
        # Only the pending-settlement state enqueues a wallet approval; any
        # other error (denied before buy, availability unavailable) must fail
        # fast rather than poll approvals for the full timeout.
        case "$CURL_BODY" in
        *"pending exact Wallet or Chain settlement"*) ;;
        *) fail "buy failed and is not a pending-approval response (won't poll): $CURL_BODY" ;;
        esac
        log "buy pending purchase settlement; approving the wallet request and re-issuing"
        poll_and_approve_wallet_approval "buy" "$system_token" "$wallet_token" "$buyer_account_id" 300 5 "$buy_started_at"
        settle_pending_call object buy "$library_token" "$buy_body" "pending exact Wallet or Chain settlement" \
            "buy" "$system_token" "$wallet_token" "$buyer_account_id" "$buy_started_at"
    fi
    [ "$PROVIDER_STATUS" = "ok" ] || fail "buy did not reach status=ok: $CURL_BODY"
    local bought_json="$CURL_BODY"

    CURRENT_STEP="buy_replay"
    # Re-issuing the identical buy is the documented idempotency contract
    # (gateway_tests/library.rs's typed publish/buy/open test asserts the
    # replay equals the original response byte-for-byte); confirm no
    # duplicate transaction/charge happens on a second call.
    provider_call object buy "$library_token" "$buy_body"
    [ "$PROVIDER_STATUS" = "ok" ] || fail "buy replay did not reach status=ok: $CURL_BODY"
    local replayed_json="$CURL_BODY"
    if [ "$bought_json" != "$replayed_json" ]; then
        fail "buy replay is not byte-identical to the original buy response (possible duplicate transaction); original=$bought_json replayed=$replayed_json"
    fi

    CURRENT_STEP="list_runtime_custody"
    provider_call object list_runtime_custody "$library_token" '{}'
    [ "$PROVIDER_STATUS" = "ok" ] || fail "list_runtime_custody failed: $CURL_BODY"
    local access_state
    access_state="$(printf '%s' "$CURL_BODY" | python3 -c '
import json
import sys
try:
    listings = json.load(sys.stdin).get("data", {}).get("listings") or []
    matches = [l for l in listings if l.get("mint_id") == sys.argv[1]]
    print(matches[0].get("access_state", "") if matches else "")
except Exception:
    print("")
' "$MINT_ID" 2>/dev/null || true)"
    [ "$access_state" = "purchased" ] || fail "list_runtime_custody shows access_state='$access_state' for mint_id=$MINT_ID (expected 'purchased'): $CURL_BODY"

    CURRENT_STEP="write_receipt_block:buy"
    local ok=true
    local block_json
    block_json="$(python3 -c '
import json
import sys

(commit, tree, clean, mint_id, bought_json, replayed_json, listing_json, ok) = sys.argv[1:9]
print(json.dumps({
    "ok": json.loads(ok),
    "git": {"commit": commit, "tree": tree, "clean": json.loads(clean)},
    "mint_id": mint_id,
    "buy_response": json.loads(bought_json),
    "buy_replay_matches": json.loads(bought_json) == json.loads(replayed_json),
    "list_runtime_custody": json.loads(listing_json),
}))
' "$GIT_COMMIT" "$GIT_TREE" "$GIT_CLEAN" "$MINT_ID" "$bought_json" "$replayed_json" "$CURL_BODY" "$ok")"
    write_receipt_block buy "$block_json"

    log "buy phase complete: mint_id=$MINT_ID access_state=purchased"
}

# --- shared viewer-open helper (open, drills, negative, restart) --------
#
# A managed buyer account holds the viewer release rights-signature request
# as a pending Wallet approval (step-up bound); the Runtime answers the open
# with RUNTIME_CUSTODY_OPEN_PENDING_MESSAGE and resumes the persisted release
# when the identical open is re-issued after approval. Every buyer-side open
# goes through here so each phase sees the settled answer (ok, or the real
# fail-closed message) rather than the pending state. The buyer's System
# (list approvals) and Wallet (approve) tokens plus the wallet account id are
# resolved lazily, once, from BUYER_AUTH_ARGS and the wallet-setup receipt.
# Leaves CURL_BODY/PROVIDER_STATUS as the last open_viewer call.
RUNTIME_CUSTODY_OPEN_PENDING_MESSAGE="Runtime custody viewer release is pending exact Wallet approval"
BUYER_SYSTEM_TOKEN=""
BUYER_WALLET_TOKEN=""
BUYER_ACCOUNT_ID=""
resolve_buyer_wallet_context() {
    [ -n "$BUYER_ACCOUNT_ID" ] && return 0
    local step="$CURRENT_STEP"
    CURRENT_STEP="gateway_launch:buyer:system"
    gateway_launch "system" "${BUYER_AUTH_ARGS[@]}"
    BUYER_SYSTEM_TOKEN="$LAUNCH_TOKEN"
    CURRENT_STEP="gateway_launch:buyer:wallet"
    gateway_launch "wallet" "${BUYER_AUTH_ARGS[@]}"
    BUYER_WALLET_TOKEN="$LAUNCH_TOKEN"
    CURRENT_STEP="resolve_buyer_account_id"
    [ -f "$RECEIPT_PATH" ] || fail "no wallet-setup receipt at '$RECEIPT_PATH'; run --phase wallet-setup first"
    BUYER_ACCOUNT_ID="$(python3 -c '
import json
import sys
try:
    with open(sys.argv[1]) as handle:
        receipt = json.load(handle)
    print(receipt.get("wallet_setup", {}).get("buyer", {}).get("account_id") or "")
except Exception:
    print("")
' "$RECEIPT_PATH")"
    [ -n "$BUYER_ACCOUNT_ID" ] || fail "receipt at '$RECEIPT_PATH' has no wallet_setup.buyer.account_id; run --phase wallet-setup first"
    CURRENT_STEP="$step"
}
# open_viewer_settling PLAYER_TOKEN OPEN_BODY LABEL
open_viewer_settling() {
    local player_token="$1" open_body="$2" label="$3"
    local started_at
    started_at="$(date +%s)"
    provider_call object open_viewer "$player_token" "$open_body"
    [ "$PROVIDER_STATUS" = "ok" ] && return 0
    case "$CURL_BODY" in
    *"$RUNTIME_CUSTODY_OPEN_PENDING_MESSAGE"*) ;;
    *) return 0 ;;
    esac
    resolve_buyer_wallet_context
    log "$label: open pending viewer release approval; approving the wallet request and re-issuing"
    poll_and_approve_wallet_approval "open" "$BUYER_SYSTEM_TOKEN" "$BUYER_WALLET_TOKEN" "$BUYER_ACCOUNT_ID" 300 5 "$started_at"
    settle_pending_call object open_viewer "$player_token" "$open_body" "$RUNTIME_CUSTODY_OPEN_PENDING_MESSAGE" \
        "$label" "$BUYER_SYSTEM_TOKEN" "$BUYER_WALLET_TOKEN" "$BUYER_ACCOUNT_ID" "$started_at"
}

# --- open phase ------------------------------------------------------

# open_viewer/read_viewer/close_viewer route ONLY to ELACITY_PLAYER_CAPSULE_ID
# (gateway_provider_proxy.rs:1509-1540) -- never library or marketplace.
phase_open() {
    CURRENT_STEP="check_git_clean"
    check_git_clean
    CURRENT_STEP="git_identity"
    git_identity
    CURRENT_STEP="resolve_mint_id"
    resolve_mint_id
    CURRENT_STEP="compute_buyer_auth_args"
    compute_buyer_auth_args

    CURRENT_STEP="gateway_launch:buyer:elacity-player"
    gateway_launch "elacity-player" "${BUYER_AUTH_ARGS[@]}"
    local player_token="$LAUNCH_TOKEN"

    CURRENT_STEP="open_viewer"
    local open_body
    open_body="$(python3 -c 'import json,sys; print(json.dumps({"mint_id": sys.argv[1]}))' "$MINT_ID")"
    open_viewer_settling "$player_token" "$open_body" "open"
    [ "$PROVIDER_STATUS" = "ok" ] || fail "open_viewer did not reach status=ok: $CURL_BODY"
    local handle
    handle="$(find_string_field "$CURL_BODY" viewer_session_handle)"
    [ -n "$handle" ] || fail "open_viewer did not return a viewer_session_handle: $CURL_BODY"
    local open_json="$CURL_BODY"

    # Viewer media parts are released strictly in order: the init segment
    # first (no segment_index), then segment 0, 1, ... -- the Runtime refuses
    # any other order as "viewer media part is invalid".
    CURRENT_STEP="read_viewer:init"
    local read_init_body
    read_init_body="$(python3 -c 'import json,sys; print(json.dumps({"mint_id": sys.argv[1], "viewer_session_handle": sys.argv[2]}))' "$MINT_ID" "$handle")"
    provider_call object read_viewer "$player_token" "$read_init_body"
    [ "$PROVIDER_STATUS" = "ok" ] || fail "read_viewer (init segment) did not reach status=ok: $CURL_BODY"
    local read_init_json="$CURL_BODY"

    CURRENT_STEP="read_viewer"
    local read_body
    read_body="$(python3 -c 'import json,sys; print(json.dumps({"mint_id": sys.argv[1], "viewer_session_handle": sys.argv[2], "segment_index": 0}))' "$MINT_ID" "$handle")"
    provider_call object read_viewer "$player_token" "$read_body"
    [ "$PROVIDER_STATUS" = "ok" ] || fail "read_viewer (segment 0) did not reach status=ok: $CURL_BODY"
    local read_json="$CURL_BODY"

    CURRENT_STEP="close_viewer"
    local close_body
    close_body="$(python3 -c 'import json,sys; print(json.dumps({"mint_id": sys.argv[1], "viewer_session_handle": sys.argv[2]}))' "$MINT_ID" "$handle")"
    provider_call object close_viewer "$player_token" "$close_body"
    [ "$PROVIDER_STATUS" = "ok" ] || fail "close_viewer did not reach status=ok: $CURL_BODY"
    local close_json="$CURL_BODY"

    CURRENT_STEP="write_receipt_block:open"
    local ok=true
    local block_json
    block_json="$(python3 -c '
import json
import sys

(commit, tree, clean, mint_id, handle, open_json, read_init_json, read_json, close_json, ok) = sys.argv[1:11]
print(json.dumps({
    "ok": json.loads(ok),
    "git": {"commit": commit, "tree": tree, "clean": json.loads(clean)},
    "mint_id": mint_id,
    "viewer_session_handle": handle,
    "open_viewer": json.loads(open_json),
    "read_viewer_init": json.loads(read_init_json),
    "read_viewer_segment_0": json.loads(read_json),
    "close_viewer": json.loads(close_json),
}))
' "$GIT_COMMIT" "$GIT_TREE" "$GIT_CLEAN" "$MINT_ID" "$handle" "$open_json" "$read_init_json" "$read_json" "$close_json" "$ok")"
    write_receipt_block open "$block_json"

    log "open phase complete: mint_id=$MINT_ID viewer_session_handle=$handle"
}

# --- drill-custody phase (single custody-loss, D2 k-of-n durable service) --
#
# docker stop/start is issued ONLY inside this phase (and drill-replica
# below), clearly logged; this session never runs it (see the task
# report), but the sequence is exactly what a live run executes.

docker_service_ps_json() {
    local svc="$1"
    docker compose -f "$COMPOSE_FILE" ps --format json "$svc" 2>/dev/null | head -1 || true
}

phase_drill_custody() {
    CURRENT_STEP="check_git_clean"
    check_git_clean
    CURRENT_STEP="git_identity"
    git_identity
    CURRENT_STEP="resolve_mint_id"
    resolve_mint_id
    CURRENT_STEP="resolve_cid"
    resolve_cid
    CURRENT_STEP="resolve_elastos_bin"
    resolve_elastos_bin
    CURRENT_STEP="compute_buyer_auth_args"
    compute_buyer_auth_args

    CURRENT_STEP="docker_stop:custody-b"
    docker_stop_service custody-b
    local stopped_ps
    stopped_ps="$(docker_service_ps_json custody-b)"

    CURRENT_STEP="gateway_launch:buyer:elacity-player:attempt1"
    gateway_launch "elacity-player" "${BUYER_AUTH_ARGS[@]}"
    local player_token_1="$LAUNCH_TOKEN"
    CURRENT_STEP="open_viewer:attempt1_expect_two_of_three"
    local open_body
    open_body="$(python3 -c 'import json,sys; print(json.dumps({"mint_id": sys.argv[1]}))' "$MINT_ID")"
    open_viewer_settling "$player_token_1" "$open_body" "drill-custody:attempt1"
    # Product contract, confirmed live 2026-09-06: the custody composition
    # is 2-of-3, so ONE stopped committee member must not deny the viewer --
    # the release settles on the two remaining nodes. The FIRST fresh
    # availability ensure right after the loss is order-dependent, though
    # (bounded candidate attempts: it can spend one of its two attempts on
    # the dead node and come up short), so exactly one availability
    # fail-closed answer is admissible here, after which the retry -- with
    # the dead node deprioritized -- must serve on the surviving replicas.
    # Fail-closed below quorum is the negative phase's two-nodes-down case.
    local attempt1_first_json="$CURL_BODY" attempt1_first_status="$PROVIDER_STATUS"
    if [ "$PROVIDER_STATUS" != "ok" ]; then
        if ! printf '%s' "$CURL_BODY" | grep -Fq "$RUNTIME_CUSTODY_AVAILABILITY_UNAVAILABLE_MESSAGE"; then
            docker_start_service custody-b
            fail "drill-custody attempt 1 (custody-b stopped) failed with an unexpected message (expected ok or '$RUNTIME_CUSTODY_AVAILABILITY_UNAVAILABLE_MESSAGE'): $CURL_BODY"
        fi
        # Replication at mint is bounded to two node candidates, so the stopped
        # node may have held one of the only two node replicas: local + one
        # live replica is below the 3-replica availability requirement and
        # the fail-closed answer is legitimate. The product's recovery path
        # is the repair worker re-replicating to the surviving node -- run
        # it with custody-b STILL STOPPED, require a healthy status, and the
        # viewer must then serve on the 2-of-3 committee.
        log "drill-custody attempt 1: fresh ensure after the loss failed closed on availability (recorded); repairing onto the surviving nodes with custody-b still stopped"
        # One forced pass is bounded to two candidate attempts and the dead
        # node still ranks among them until its failure lowers its
        # reputation, so a pass can end local-only; the product's worker
        # loops periodically. Bound the drill to three passes.
        local drill_repair_json drill_repair_pass=0 drill_repair_healthy=false
        while [ "$drill_repair_pass" -lt 3 ]; do
            drill_repair_pass=$((drill_repair_pass + 1))
            log "+ $ELASTOS_BIN content repair-worker --force (pass $drill_repair_pass, custody-b stopped)"
            drill_repair_json="$("$ELASTOS_BIN" content repair-worker --force)" \
                || { docker_start_service custody-b; fail "drill-custody: elastos content repair-worker --force failed with custody-b stopped; diagnostic: $ELASTOS_BIN content repair-worker --force"; }
            log "repair-worker pass $drill_repair_pass (custody-b stopped): $drill_repair_json"
            if assert_content_status_healthy_or_report "$CID"; then
                drill_repair_healthy=true
                break
            fi
            log "content status not healthy yet after repair pass $drill_repair_pass: $(printf '%s' "$(find_availability_fields "$CONTENT_STATUS_JSON")")"
        done
        if [ "$drill_repair_healthy" != true ]; then
            docker_start_service custody-b
            fail "drill-custody: content status did not read healthy after $drill_repair_pass repair passes with custody-b stopped: $CONTENT_STATUS_JSON"
        fi
        open_viewer_settling "$player_token_1" "$open_body" "drill-custody:attempt1-after-repair"
        if [ "$PROVIDER_STATUS" != "ok" ]; then
            docker_start_service custody-b
            fail "drill-custody attempt 1 after repair (custody-b stopped, 2-of-3 must serve on the repaired replica set) did not reach status=ok: $CURL_BODY"
        fi
    fi
    local attempt1_json="$CURL_BODY"
    local handle_1
    handle_1="$(find_string_field "$attempt1_json" viewer_session_handle)"
    [ -n "$handle_1" ] || { docker_start_service custody-b; fail "drill-custody attempt 1 returned no viewer_session_handle: $attempt1_json"; }
    log "drill-custody attempt 1 served on 2-of-3 with custody-b stopped: $attempt1_json"
    local close_body_1
    close_body_1="$(python3 -c 'import json,sys; print(json.dumps({"mint_id": sys.argv[1], "viewer_session_handle": sys.argv[2]}))' "$MINT_ID" "$handle_1")"
    provider_call object close_viewer "$player_token_1" "$close_body_1"
    [ "$PROVIDER_STATUS" = "ok" ] || { docker_start_service custody-b; fail "drill-custody attempt 1 close_viewer did not reach status=ok: $CURL_BODY"; }

    CURRENT_STEP="docker_start:custody-b"
    docker_start_service custody-b
    local restarted_ps
    restarted_ps="$(docker_service_ps_json custody-b)"

    CURRENT_STEP="open_viewer:attempt2_expect_three_of_three"
    gateway_launch "elacity-player" "${BUYER_AUTH_ARGS[@]}"
    local player_token_2="$LAUNCH_TOKEN"
    open_viewer_settling "$player_token_2" "$open_body" "drill-custody:attempt2"
    [ "$PROVIDER_STATUS" = "ok" ] || fail "drill-custody attempt 2 (custody-b restarted, 3-of-3) did not reach status=ok: $CURL_BODY"
    local attempt2_json="$CURL_BODY"
    local handle
    handle="$(find_string_field "$attempt2_json" viewer_session_handle)"
    log "drill-custody attempt 2 served again with custody-b back (3-of-3): $attempt2_json"
    if [ -n "$handle" ]; then
        local close_body
        close_body="$(python3 -c 'import json,sys; print(json.dumps({"mint_id": sys.argv[1], "viewer_session_handle": sys.argv[2]}))' "$MINT_ID" "$handle")"
        provider_call object close_viewer "$player_token_2" "$close_body"
    fi

    CURRENT_STEP="write_receipt_block:drill-custody"
    local ok=true
    local block_json
    block_json="$(python3 -c '
import json
import sys

(commit, tree, clean, mint_id, stopped_ps, restarted_ps, attempt1_first_status, attempt1_first_json, attempt1_json, attempt2_json, ok) = sys.argv[1:12]
def parse_ps(text):
    try:
        return json.loads(text) if text else None
    except Exception:
        return text or None
print(json.dumps({
    "ok": json.loads(ok),
    "git": {"commit": commit, "tree": tree, "clean": json.loads(clean)},
    "mint_id": mint_id,
    "custody_b_stopped_ps": parse_ps(stopped_ps),
    "custody_b_restarted_ps": parse_ps(restarted_ps),
    "attempt_1_first_answer_status": attempt1_first_status,
    "attempt_1_first_answer": json.loads(attempt1_first_json) if attempt1_first_json.strip().startswith("{") else attempt1_first_json,
    "attempt_1_two_of_three_with_custody_b_stopped": json.loads(attempt1_json),
    "attempt_2_three_of_three_after_restart": json.loads(attempt2_json),
}))
' "$GIT_COMMIT" "$GIT_TREE" "$GIT_CLEAN" "$MINT_ID" "$stopped_ps" "$restarted_ps" "$attempt1_first_status" "$attempt1_first_json" "$attempt1_json" "$attempt2_json" "$ok")"
    write_receipt_block drill_custody "$block_json"

    log "drill-custody phase complete: served on 2-of-3 with custody-b stopped (attempt 1), served on 3-of-3 after its restart (attempt 2)"
}

# --- drill-replica phase (replica-loss, repair-worker recovery) --------
#
# Uses custody-a (distinct from drill-custody's custody-b) as the stopped
# replica-holding container -- an arbitrary but documented choice, since
# each of the 3 SERVICES containers also runs availability-provider/
# ipfs-provider alongside custody-provider (preflight's ready-receipt
# check). The degraded-buy assertion uses the DENIAL principal (a fresh,
# not-yet-purchasing principal) rather than the buyer: a second buy() call
# from an already-Complete purchase returns its terminal response before
# ever touching availability (protected_content_runtime.rs), so only a
# FRESH purchase attempt actually exercises the fresh-availability check
# this drill is proving.
phase_drill_replica() {
    CURRENT_STEP="check_git_clean"
    check_git_clean
    CURRENT_STEP="resolve_mint_id"
    resolve_mint_id
    CURRENT_STEP="resolve_cid"
    resolve_cid
    CURRENT_STEP="resolve_elastos_bin"
    resolve_elastos_bin
    CURRENT_STEP="git_identity"
    git_identity
    CURRENT_STEP="compute_buyer_auth_args"
    compute_buyer_auth_args
    CURRENT_STEP="compute_denial_auth_args"
    compute_denial_auth_args

    CURRENT_STEP="docker_stop:custody-a"
    docker_stop_service custody-a

    CURRENT_STEP="capture_cached_status_while_degraded"
    # `elastos content status` reports the LATEST STORED availability receipt
    # (content.rs latest_receipt_for_cid), never a live re-observation, so
    # right after stopping custody-a it still echoes the healthy 3-replica
    # receipt the last ensure wrote. The live degraded evidence is the buy
    # and open below: both re-`ensure` the CID under the listing's bindings
    # (fresh availability) and must fail closed. The cached reading is
    # captured here for the receipt, not asserted degraded.
    log "+ $ELASTOS_BIN content status --cid $CID (cached receipt; expected to still read healthy)"
    local degraded_json
    degraded_json="$("$ELASTOS_BIN" content status --cid "$CID")" \
        || fail "elastos content status --cid $CID failed while degraded; diagnostic: $ELASTOS_BIN content status --cid $CID"
    local fields replicas
    fields="$(find_availability_fields "$degraded_json")"
    replicas="$(printf '%s' "$fields" | awk '{print $1}')"
    if [ "$replicas" = "null" ]; then
        docker_start_service custody-a
        fail "content status for $CID has no numeric data.availability.replicas while degraded (schema drift?); diagnostic: $ELASTOS_BIN content status --cid $CID -- $degraded_json"
    fi
    log "cached content status with custody-a stopped (last stored receipt, replicas=$replicas): $degraded_json"

    CURRENT_STEP="gateway_launch:denial:library"
    gateway_launch "library" "${DENIAL_AUTH_ARGS[@]}"
    local denial_library_token="$LAUNCH_TOKEN"
    CURRENT_STEP="buy_expect_fail_while_degraded"
    local buy_body degraded_buy_json
    buy_body="$(python3 -c 'import json,sys; print(json.dumps({"mint_id": sys.argv[1]}))' "$MINT_ID")"
    provider_call object buy "$denial_library_token" "$buy_body"
    # Live-run finding (2026-09-06): buy and open both re-ensure the CID
    # live with the 3-replica requirement, and the outcome right after a
    # single replica loss is ORDER-DEPENDENT, not a stable contract: the
    # first ensure after the loss can spend one of its bounded candidate
    # attempts (content abuse_controls: attempt_limit 2 of 3 candidates) on
    # the dead node and come up short, while the next ensure, with that
    # node deprioritized, reaches local + two live replicas and passes.
    # So the degraded buy/open are RECORDED, and only two answers are
    # admissible: ok, or the availability fail-closed message. The stable
    # contract asserted below is repair -> healthy status -> open serves.
    # Once the fresh ensure passes, the buy proceeds to its exact Wallet
    # settlement and answers "pending" -- the denial principal's wallet
    # request is deliberately never approved here, so that pending answer is
    # the third admissible (recorded) outcome.
    if [ "$PROVIDER_STATUS" != "ok" ] \
        && ! printf '%s' "$CURL_BODY" | grep -Fq "$RUNTIME_CUSTODY_PURCHASE_DENIED_MESSAGE" \
        && ! printf '%s' "$CURL_BODY" | grep -Fq "$RUNTIME_CUSTODY_AVAILABILITY_UNAVAILABLE_MESSAGE" \
        && ! printf '%s' "$CURL_BODY" | grep -Fq "pending exact Wallet or Chain settlement"; then
        docker_start_service custody-a
        fail "buy while degraded failed with an unexpected message (expected ok, pending settlement, '$RUNTIME_CUSTODY_PURCHASE_DENIED_MESSAGE' or '$RUNTIME_CUSTODY_AVAILABILITY_UNAVAILABLE_MESSAGE'): $CURL_BODY"
    fi
    degraded_buy_json="$CURL_BODY"
    log "buy while degraded (recorded, status=$PROVIDER_STATUS): $CURL_BODY"

    CURRENT_STEP="open_expect_fail_while_degraded"
    gateway_launch "elacity-player" "${BUYER_AUTH_ARGS[@]}"
    local player_token="$LAUNCH_TOKEN"
    local open_body degraded_open_json
    open_body="$(python3 -c 'import json,sys; print(json.dumps({"mint_id": sys.argv[1]}))' "$MINT_ID")"
    open_viewer_settling "$player_token" "$open_body" "drill-replica:degraded"
    if [ "$PROVIDER_STATUS" = "ok" ]; then
        # Served on the surviving replicas (see the buy note above): close
        # the session so the after-repair open below starts clean.
        local degraded_handle
        degraded_handle="$(find_string_field "$CURL_BODY" viewer_session_handle)"
        degraded_open_json="$CURL_BODY"
        if [ -n "$degraded_handle" ]; then
            local degraded_close_body
            degraded_close_body="$(python3 -c 'import json,sys; print(json.dumps({"mint_id": sys.argv[1], "viewer_session_handle": sys.argv[2]}))' "$MINT_ID" "$degraded_handle")"
            provider_call object close_viewer "$player_token" "$degraded_close_body"
        fi
        log "open_viewer while degraded served on the surviving replicas (recorded): $degraded_open_json"
    else
        if ! printf '%s' "$CURL_BODY" | grep -Fq "$RUNTIME_CUSTODY_AVAILABILITY_UNAVAILABLE_MESSAGE"; then
            docker_start_service custody-a
            fail "open_viewer while degraded failed with an unexpected message (expected ok or '$RUNTIME_CUSTODY_AVAILABILITY_UNAVAILABLE_MESSAGE'): $CURL_BODY"
        fi
        degraded_open_json="$CURL_BODY"
        log "open_viewer while degraded failed closed on fresh availability (recorded): $CURL_BODY"
    fi

    CURRENT_STEP="repair_worker_force"
    log "+ $ELASTOS_BIN content repair-worker --force"
    local repair_json
    repair_json="$("$ELASTOS_BIN" content repair-worker --force)" \
        || fail "elastos content repair-worker --force failed; diagnostic: $ELASTOS_BIN content repair-worker --force"
    log "repair-worker: $repair_json"

    CURRENT_STEP="assert_content_status_healthy_after_repair"
    assert_content_status_healthy "$CID"

    CURRENT_STEP="open_succeeds_after_repair"
    gateway_launch "elacity-player" "${BUYER_AUTH_ARGS[@]}"
    player_token="$LAUNCH_TOKEN"
    open_viewer_settling "$player_token" "$open_body" "drill-replica:after-repair"
    [ "$PROVIDER_STATUS" = "ok" ] || fail "open_viewer did not reach status=ok after repair: $CURL_BODY"
    local healed_open_json="$CURL_BODY"
    local handle
    handle="$(find_string_field "$healed_open_json" viewer_session_handle)"
    if [ -n "$handle" ]; then
        local close_body
        close_body="$(python3 -c 'import json,sys; print(json.dumps({"mint_id": sys.argv[1], "viewer_session_handle": sys.argv[2]}))' "$MINT_ID" "$handle")"
        provider_call object close_viewer "$player_token" "$close_body"
    fi

    CURRENT_STEP="docker_start:custody-a"
    docker_start_service custody-a

    CURRENT_STEP="write_receipt_block:drill-replica"
    local ok=true
    local block_json
    block_json="$(python3 -c '
import json
import sys

(commit, tree, clean, cid, mint_id, degraded_json, degraded_buy_json,
 degraded_open_json, repair_json, healed_status_json, healed_open_json, ok) = sys.argv[1:13]
print(json.dumps({
    "ok": json.loads(ok),
    "git": {"commit": commit, "tree": tree, "clean": json.loads(clean)},
    "cid": cid,
    "mint_id": mint_id,
    "degraded_content_status": json.loads(degraded_json),
    "degraded_buy_failed": json.loads(degraded_buy_json) if degraded_buy_json.strip().startswith("{") else degraded_buy_json,
    "degraded_open_failed": json.loads(degraded_open_json) if degraded_open_json.strip().startswith("{") else degraded_open_json,
    "repair_worker": json.loads(repair_json),
    "healed_content_status": json.loads(healed_status_json),
    "healed_open": json.loads(healed_open_json),
}))
' \
        "$GIT_COMMIT" "$GIT_TREE" "$GIT_CLEAN" "$CID" "$MINT_ID" "$degraded_json" "$degraded_buy_json" \
        "$degraded_open_json" "$repair_json" "$CONTENT_STATUS_JSON" "$healed_open_json" "$ok")"
    write_receipt_block drill_replica "$block_json"

    log "drill-replica phase complete: degraded buy/open recorded, repair-worker healed, status healthy, open succeeds"
}

# --- negative phase (5 scripted fail-closed assertions) -----------------

negative_below_quorum() {
    docker_stop_service custody-a
    docker_stop_service custody-b
    gateway_launch "elacity-player" "${BUYER_AUTH_ARGS[@]}"
    local player_token="$LAUNCH_TOKEN"
    local open_body
    open_body="$(python3 -c 'import json,sys; print(json.dumps({"mint_id": sys.argv[1]}))' "$MINT_ID")"
    open_viewer_settling "$player_token" "$open_body" "negative:below-quorum"
    local status="$PROVIDER_STATUS" body="$CURL_BODY"
    docker_start_service custody-a
    docker_start_service custody-b
    if [ "$status" = "ok" ]; then
        fail "open_viewer unexpectedly succeeded below quorum (custody-a and custody-b both stopped): $body"
    fi
    # Specific fail-closed message, not a bare non-"ok" check (Important 7
    # in the fix round): 2-of-3 custody nodes down is unambiguously below
    # any viable quorum, so open_runtime_custody_viewer's fresh-availability
    # re-check (verify_fresh_runtime_custody_availability,
    # protected_content_runtime.rs:5559/5677) is the path that fails here,
    # not the decrypt-share path drill-custody's single-node-loss case
    # might also hit.
    if ! printf '%s' "$body" | grep -Fq "$RUNTIME_CUSTODY_AVAILABILITY_UNAVAILABLE_MESSAGE"; then
        fail "open_viewer failed below quorum, but not with the expected message ('$RUNTIME_CUSTODY_AVAILABILITY_UNAVAILABLE_MESSAGE'): $body"
    fi
    log "open_viewer correctly failed closed below quorum: $body"
    BELOW_QUORUM_RESULT_JSON="$body"
}

negative_denial() {
    compute_denial_auth_args
    gateway_launch "elacity-player" "${DENIAL_AUTH_ARGS[@]}"
    local player_token="$LAUNCH_TOKEN"
    local open_body
    open_body="$(python3 -c 'import json,sys; print(json.dumps({"mint_id": sys.argv[1]}))' "$MINT_ID")"
    provider_call object open_viewer "$player_token" "$open_body"
    if [ "$PROVIDER_STATUS" = "ok" ]; then
        fail "open_viewer unexpectedly succeeded for the denial principal, who never purchased: $CURL_BODY"
    fi
    if ! printf '%s' "$CURL_BODY" | grep -Fq "$RUNTIME_CUSTODY_OPEN_DENIED_MESSAGE"; then
        fail "open_viewer failed for the denial principal but not with RUNTIME_CUSTODY_OPEN_DENIED_MESSAGE ('$RUNTIME_CUSTODY_OPEN_DENIED_MESSAGE'): $CURL_BODY"
    fi
    log "open_viewer correctly denied the non-purchasing principal: $CURL_BODY"
    DENIAL_RESULT_JSON="$CURL_BODY"
}

# negative_foreign -- FOREIGN sub-case (Critical 2 ruling): principal B
# (the denial principal, who never purchased) attempts read_viewer against
# a viewer session/open that principal A (the buyer) created, using A's own
# real captured viewer_session_handle. gateway_provider_proxy.rs:1653
# always injects the CALLER's own verified principal_id server-side
# (`request["principal_id"] = ...`), never a client-supplied one, so
# read_runtime_custody_viewer's load_runtime_custody_purchase(data_dir,
# B_principal_id, mint_id) can never find A's purchase however B's request
# body predeclares mint_id/handle -- proving storage/lookup is
# principal-scoped, not merely that the caller must be signed-in.
negative_foreign() {
    gateway_launch "elacity-player" "${BUYER_AUTH_ARGS[@]}"
    local owner_token="$LAUNCH_TOKEN"
    local open_body
    open_body="$(python3 -c 'import json,sys; print(json.dumps({"mint_id": sys.argv[1]}))' "$MINT_ID")"
    open_viewer_settling "$owner_token" "$open_body" "negative:cross-principal:owner"
    [ "$PROVIDER_STATUS" = "ok" ] || fail "negative:foreign setup: buyer's own open_viewer did not reach status=ok: $CURL_BODY"
    local owner_handle
    owner_handle="$(find_string_field "$CURL_BODY" viewer_session_handle)"
    [ -n "$owner_handle" ] || fail "negative:foreign setup: buyer's open_viewer did not return a viewer_session_handle: $CURL_BODY"

    compute_denial_auth_args
    gateway_launch "elacity-player" "${DENIAL_AUTH_ARGS[@]}"
    local foreign_token="$LAUNCH_TOKEN"
    local foreign_read_body
    foreign_read_body="$(python3 -c 'import json,sys; print(json.dumps({"mint_id": sys.argv[1], "viewer_session_handle": sys.argv[2], "segment_index": 0}))' "$MINT_ID" "$owner_handle")"
    provider_call object read_viewer "$foreign_token" "$foreign_read_body"
    local foreign_status="$PROVIDER_STATUS" foreign_json="$CURL_BODY"

    local owner_close_body
    owner_close_body="$(python3 -c 'import json,sys; print(json.dumps({"mint_id": sys.argv[1], "viewer_session_handle": sys.argv[2]}))' "$MINT_ID" "$owner_handle")"
    provider_call object close_viewer "$owner_token" "$owner_close_body"

    if [ "$foreign_status" = "ok" ]; then
        fail "read_viewer unexpectedly succeeded for the denial principal against the buyer's own viewer session (cross-principal access): $foreign_json"
    fi
    # A foreign principal is refused before the session gate: read_viewer
    # loads the CALLER's purchase first, so the non-purchasing principal is
    # denied "before purchase" (confirmed live); a principal with its own
    # purchase but a foreign handle hits the session gate instead. Both are
    # specific fail-closed answers.
    if ! printf '%s' "$foreign_json" | grep -Fq "$RUNTIME_CUSTODY_VIEWER_SESSION_UNAVAILABLE_MESSAGE" \
        && ! printf '%s' "$foreign_json" | grep -Fq "$RUNTIME_CUSTODY_OPEN_DENIED_MESSAGE"; then
        fail "read_viewer correctly denied cross-principal access, but not with an expected message ('$RUNTIME_CUSTODY_VIEWER_SESSION_UNAVAILABLE_MESSAGE' or '$RUNTIME_CUSTODY_OPEN_DENIED_MESSAGE'): $foreign_json"
    fi
    log "read_viewer correctly denied cross-principal access to another principal's viewer session: $foreign_json"
    FOREIGN_RESULT_JSON="$foreign_json"
}

# negative_stale -- STALE sub-case (Critical 2 ruling): the buyer replays
# their own captured read_viewer op -- SAME token, SAME handle -- after
# close_viewer already settled the session. Deliberately reuses the
# original token (not a freshly launched one) so runtime_session_binding
# still matches (protected_content_runtime.rs:6069-6075); this isolates
# the Closed-lifecycle bail specifically, rather than a binding mismatch
# that a fresh launch would also trigger for an unrelated reason.
negative_stale() {
    gateway_launch "elacity-player" "${BUYER_AUTH_ARGS[@]}"
    local player_token="$LAUNCH_TOKEN"
    local open_body
    open_body="$(python3 -c 'import json,sys; print(json.dumps({"mint_id": sys.argv[1]}))' "$MINT_ID")"
    open_viewer_settling "$player_token" "$open_body" "negative:stale"
    [ "$PROVIDER_STATUS" = "ok" ] || fail "negative:stale setup: open_viewer did not reach status=ok: $CURL_BODY"
    local handle
    handle="$(find_string_field "$CURL_BODY" viewer_session_handle)"
    [ -n "$handle" ] || fail "negative:stale setup: open_viewer did not return a viewer_session_handle: $CURL_BODY"

    local close_body
    close_body="$(python3 -c 'import json,sys; print(json.dumps({"mint_id": sys.argv[1], "viewer_session_handle": sys.argv[2]}))' "$MINT_ID" "$handle")"
    provider_call object close_viewer "$player_token" "$close_body"
    [ "$PROVIDER_STATUS" = "ok" ] || fail "negative:stale setup: close_viewer did not reach status=ok: $CURL_BODY"

    local replay_body
    replay_body="$(python3 -c 'import json,sys; print(json.dumps({"mint_id": sys.argv[1], "viewer_session_handle": sys.argv[2], "segment_index": 0}))' "$MINT_ID" "$handle")"
    provider_call object read_viewer "$player_token" "$replay_body"
    if [ "$PROVIDER_STATUS" = "ok" ]; then
        fail "read_viewer unexpectedly succeeded replaying a captured op (same token/handle) after close_viewer already settled the session: $CURL_BODY"
    fi
    if ! printf '%s' "$CURL_BODY" | grep -Fq "$RUNTIME_CUSTODY_VIEWER_SESSION_UNAVAILABLE_MESSAGE"; then
        fail "post-close read_viewer replay failed as expected, but not with the expected message ('$RUNTIME_CUSTODY_VIEWER_SESSION_UNAVAILABLE_MESSAGE'): $CURL_BODY"
    fi
    log "read_viewer correctly rejected a stale replay after close_viewer settled the session: $CURL_BODY"
    STALE_RESULT_JSON="$CURL_BODY"
}

# find_tamper_target SERVICE CID -- best-effort search of the container's
# whole data-dir tree for a file whose path contains CID (content-addressed
# storage places the CID in the path/filename on every store this codebase
# has; the exact custody-node replica layout was not pinned to one path
# from source, so this searches broadly rather than guessing a brittle
# exact path). Sets TAMPER_TARGET (empty if nothing matched).
find_tamper_target() {
    local svc="$1" cid="$2"
    # On the custody-host image a replica lives in kubo's block store (base32
    # multihash file names, never the CID) and, for the custody plane, in the
    # node's share store (protected-content/custody-provider/inactive/data/
    # node-shares/<slot-hash>). The share is the byte a shard-holder's
    # integrity actually rides on, and this journey provisions exactly one
    # mint, so its share is the newest file there. $cid is kept for the log.
    log "+ docker compose -f '$COMPOSE_FILE' exec -T $svc ls -t .../node-shares (newest custody share; cid=$cid)"
    # `|| true` at the end (matching docker_service_ps_json's existing
    # idiom in this file) so `head -1` closing the pipe early can never
    # SIGPIPE the `docker exec find` stage into a spurious failure under
    # `set -o pipefail` (minor 13 in the fix round) -- fails in the safe
    # direction either way (empty TAMPER_TARGET aborts before tampering),
    # this just removes an avoidable flake.
    TAMPER_TARGET="$(docker compose -f "$COMPOSE_FILE" exec -T "$svc" sh -c 'd=/home/custody/.local/share/elastos/protected-content/custody-provider/inactive/data/node-shares; ls -t "$d" 2>/dev/null | grep -vE "^\\.|\\.lock$|\\.tmp$" | head -1 | sed "s|^|$d/|"' 2>/dev/null | head -1 | tr -d '\r' || true)"
}

negative_tamper() {
    local svc="custody-a"
    find_tamper_target "$svc" "$CID"
    [ -n "$TAMPER_TARGET" ] || fail "tamper: no stored replica file found on $svc matching cid=$CID under /home/custody/.local/share/elastos; diagnostic: docker compose -f '$COMPOSE_FILE' exec -T $svc find /home/custody/.local/share/elastos -type f"

    local backup_file flipped_file
    backup_file="$(mktemp)"
    flipped_file="$(mktemp)"
    docker compose -f "$COMPOSE_FILE" exec -T "$svc" cat "$TAMPER_TARGET" >"$backup_file" \
        || fail "tamper: could not read $TAMPER_TARGET from $svc"
    python3 -c '
import sys
with open(sys.argv[1], "rb") as handle:
    data = bytearray(handle.read())
if not data:
    raise SystemExit("tamper target file is empty")
data[0] ^= 0xFF
with open(sys.argv[2], "wb") as handle:
    handle.write(data)
' "$backup_file" "$flipped_file" || fail "tamper: failed to build a flipped copy of $TAMPER_TARGET (empty file?)"
    docker compose -f "$COMPOSE_FILE" exec -T "$svc" sh -c "cat > '$TAMPER_TARGET'" <"$flipped_file" \
        || { rm -f "$backup_file" "$flipped_file"; fail "tamper: failed to write the flipped copy back to $svc:$TAMPER_TARGET"; }
    # Corruption is now live on $svc -- register the inverse on the restore
    # stack (Important 4 in the fix round) BEFORE doing anything else, so
    # ANY failure from here on (gateway_launch, a transport error, etc.)
    # still gets the byte restored by on_exit's run_restore_stack, not just
    # the designed success path below.
    push_restore_tamper "$svc" "$TAMPER_TARGET" "$backup_file"

    gateway_launch "elacity-player" "${BUYER_AUTH_ARGS[@]}"
    local player_token="$LAUNCH_TOKEN"
    local open_body
    open_body="$(python3 -c 'import json,sys; print(json.dumps({"mint_id": sys.argv[1]}))' "$MINT_ID")"
    open_viewer_settling "$player_token" "$open_body" "negative:tamper"
    local tamper_status="$PROVIDER_STATUS" tamper_json="$CURL_BODY"

    # Restore unconditionally, before asserting anything -- a stuck-
    # tampered node must never survive an assertion failure. Pop the
    # restore-stack entry only once the write-back actually succeeded; on
    # failure, leave both the entry and backup_file in place so on_exit's
    # sweep (or a future run) can still retry the restore from the same
    # backup.
    if docker compose -f "$COMPOSE_FILE" exec -T "$svc" sh -c "cat > '$TAMPER_TARGET'" <"$backup_file"; then
        remove_restore_tamper "$svc" "$TAMPER_TARGET"
        rm -f "$backup_file"
    else
        log "WARNING: failed to restore $TAMPER_TARGET on $svc from backup $backup_file -- the EXIT-time restore stack will retry; manual restore required if that also fails"
    fi
    rm -f "$flipped_file"

    # k-of-n contract (confirmed live 2026-09-06 by drill-custody): a
    # committee member whose share is corrupt answers BackendUnavailable
    # for its contribution and the release settles on the two intact nodes,
    # exactly like a stopped node -- so the tampered node must NEVER make
    # the viewer serve on ITS contribution, and the only admissible answers
    # are: served (2-of-3 on the intact nodes; close it) or the release
    # fail-closed message. Anything else is a defect.
    if [ "$tamper_status" = "ok" ]; then
        local tamper_handle
        tamper_handle="$(find_string_field "$tamper_json" viewer_session_handle)"
        if [ -n "$tamper_handle" ]; then
            local tamper_close_body
            tamper_close_body="$(python3 -c 'import json,sys; print(json.dumps({"mint_id": sys.argv[1], "viewer_session_handle": sys.argv[2]}))' "$MINT_ID" "$tamper_handle")"
            provider_call object close_viewer "$player_token" "$tamper_close_body"
        fi
        log "open_viewer served on the intact 2-of-3 with one byte of $svc's custody share tampered (recorded): $tamper_json"
    elif printf '%s' "$tamper_json" | grep -Fq "$RUNTIME_CUSTODY_RELEASE_APPROVAL_UNAVAILABLE_MESSAGE"; then
        log "open_viewer failed closed with one byte of $svc's custody share tampered (recorded): $tamper_json"
    else
        fail "open_viewer after tampering one byte of $TAMPER_TARGET on $svc answered neither ok (2-of-3) nor '$RUNTIME_CUSTODY_RELEASE_APPROVAL_UNAVAILABLE_MESSAGE': $tamper_json"
    fi
    TAMPER_RESULT_JSON="$tamper_json"
    TAMPER_TARGET_PATH="$TAMPER_TARGET"
}

phase_negative() {
    CURRENT_STEP="check_git_clean"
    check_git_clean
    CURRENT_STEP="git_identity"
    git_identity
    CURRENT_STEP="resolve_mint_id"
    resolve_mint_id
    CURRENT_STEP="resolve_cid"
    resolve_cid
    CURRENT_STEP="compute_buyer_auth_args"
    compute_buyer_auth_args

    CURRENT_STEP="negative:below_quorum"
    negative_below_quorum
    CURRENT_STEP="negative:denial"
    negative_denial
    CURRENT_STEP="negative:foreign"
    negative_foreign
    CURRENT_STEP="negative:stale"
    negative_stale
    CURRENT_STEP="negative:tamper"
    negative_tamper

    CURRENT_STEP="write_receipt_block:negative"
    local ok=true
    local block_json
    block_json="$(python3 -c '
import json
import sys

(commit, tree, clean, below_quorum, denial, foreign, stale, tamper, tamper_target, ok) = sys.argv[1:11]

def parse(text):
    try:
        return json.loads(text)
    except Exception:
        return text

print(json.dumps({
    "ok": json.loads(ok),
    "git": {"commit": commit, "tree": tree, "clean": json.loads(clean)},
    "below_quorum": parse(below_quorum),
    "denial": parse(denial),
    "foreign": parse(foreign),
    "stale": parse(stale),
    "tamper": {"target": tamper_target, "open_response": parse(tamper)},
}))
' \
        "$GIT_COMMIT" "$GIT_TREE" "$GIT_CLEAN" \
        "$BELOW_QUORUM_RESULT_JSON" "$DENIAL_RESULT_JSON" "$FOREIGN_RESULT_JSON" "$STALE_RESULT_JSON" \
        "$TAMPER_RESULT_JSON" "$TAMPER_TARGET_PATH" "$ok")"
    write_receipt_block negative "$block_json"

    log "negative phase complete: below-quorum, denial, foreign, stale, and tamper all correctly rejected"
}

# --- restart phase (SIGKILL between wallet approval and buy confirmation) -

CLIENT_GATEWAY_PID_PATH_SUFFIX="run/gateway.pid"

sigkill_client_gateway() {
    local pid_path="${CLIENT_DATA_DIR}/${CLIENT_GATEWAY_PID_PATH_SUFFIX}"
    [ -f "$pid_path" ] || fail "no gateway pid file at '$pid_path'; is the installed client running?"
    local pid
    pid="$(tr -d '[:space:]' <"$pid_path")"
    [ -n "$pid" ] || fail "gateway pid file at '$pid_path' is empty"
    log "+ kill -KILL $pid (client gateway, pid from $pid_path)"
    if ! kill -KILL "$pid" 2>/dev/null; then
        log "kill -KILL $pid: process already gone (fine -- proceeding to restart)"
    fi
    sleep 1
}

# Mirrors restart_client()'s --test-home derivation but always restarts
# (no INSTALL_OK skip-guard -- mid-drill, we always intend to restart).
restart_installed_client() {
    local test_home
    case "$CLIENT_DATA_DIR" in
    *"$CLIENT_DATA_DIR_SUFFIX")
        test_home="${CLIENT_DATA_DIR%"$CLIENT_DATA_DIR_SUFFIX"}"
        ;;
    *)
        fail "--client-data-dir '$CLIENT_DATA_DIR' does not end with '$CLIENT_DATA_DIR_SUFFIX'; cannot derive --test-home for mac-source-home-restart.sh"
        ;;
    esac
    log "+ ${ROOT}/scripts/mac-source-home-restart.sh --test-home '$test_home'"
    "${ROOT}/scripts/mac-source-home-restart.sh" --test-home "$test_home" \
        || fail "mac-source-home-restart.sh failed; diagnostic: ${ROOT}/scripts/mac-source-home-restart.sh --test-home '$test_home' --dry-run"
}

# Mints its own throwaway content item (rather than reusing the journey's
# mint_id) so there is a genuinely fresh, not-yet-purchased purchase to
# interrupt -- re-buying an already-Complete purchase returns its terminal
# response before ever reaching a wallet approval (protected_content_
# runtime.rs), so it could never actually land inside the approval-to-
# confirmation window this drill needs to interrupt.
phase_restart() {
    CURRENT_STEP="check_git_clean"
    check_git_clean
    CURRENT_STEP="compute_creator_auth_args"
    compute_creator_auth_args
    CURRENT_STEP="resolve_elastos_bin"
    resolve_elastos_bin
    CURRENT_STEP="git_identity"
    git_identity
    CURRENT_STEP="creator_mint_content_item:restart"
    creator_mint_content_item "restart-"
    local mint_id="$MINTED_MINT_ID"
    CURRENT_STEP="assert_content_status_healthy"
    assert_content_status_healthy "$MINTED_CID"

    CURRENT_STEP="compute_buyer_auth_args"
    compute_buyer_auth_args
    CURRENT_STEP="gateway_launch:buyer:library"
    gateway_launch "library" "${BUYER_AUTH_ARGS[@]}"
    local library_token="$LAUNCH_TOKEN"
    CURRENT_STEP="gateway_launch:buyer:system"
    gateway_launch "system" "${BUYER_AUTH_ARGS[@]}"
    local system_token="$LAUNCH_TOKEN"
    CURRENT_STEP="gateway_launch:buyer:wallet"
    gateway_launch "wallet" "${BUYER_AUTH_ARGS[@]}"
    local wallet_token="$LAUNCH_TOKEN"

    CURRENT_STEP="resolve_buyer_account_id"
    [ -f "$RECEIPT_PATH" ] || fail "no wallet-setup receipt at '$RECEIPT_PATH'; run --phase wallet-setup first"
    local buyer_account_id
    buyer_account_id="$(python3 -c '
import json
import sys
try:
    with open(sys.argv[1]) as handle:
        receipt = json.load(handle)
    print(receipt.get("wallet_setup", {}).get("buyer", {}).get("account_id") or "")
except Exception:
    print("")
' "$RECEIPT_PATH")"
    [ -n "$buyer_account_id" ] || fail "receipt at '$RECEIPT_PATH' has no wallet_setup.buyer.account_id; run --phase wallet-setup first"

    CURRENT_STEP="buy_initiate"
    local buy_body
    buy_body="$(python3 -c 'import json,sys; print(json.dumps({"mint_id": sys.argv[1]}))' "$mint_id")"
    provider_call object buy "$library_token" "$buy_body"
    [ "$PROVIDER_STATUS" != "ok" ] || fail "buy reached status=ok before any wallet approval -- restart drill needs a genuinely in-flight purchase to interrupt: $CURL_BODY"
    local pending_json="$CURL_BODY"

    CURRENT_STEP="poll_and_approve_wallet_approval"
    poll_and_approve_wallet_approval "restart" "$system_token" "$wallet_token" "$buyer_account_id"

    CURRENT_STEP="sigkill_client_gateway"
    log "SIGKILLing the client gateway now -- approval request $APPROVED_REQUEST_ID is approved but buy() has not yet been re-issued to confirm it"
    sigkill_client_gateway

    CURRENT_STEP="restart_installed_client"
    restart_installed_client

    # Re-launch a FRESH capsule token after the restart (Important 6 in the
    # fix round) -- do not assume the pre-kill token still validates
    # against the restarted gateway. Reusing it would risk attributing a
    # token-lifetime detail (an unrelated 401) to a purchase-durability
    # regression, exactly the failure mode phase_cleanup's own post-restart
    # re-launch already avoids; this now matches that pattern.
    CURRENT_STEP="gateway_launch:buyer:library:post-restart"
    gateway_launch "library" "${BUYER_AUTH_ARGS[@]}"
    library_token="$LAUNCH_TOKEN"

    CURRENT_STEP="buy_confirm_after_restart"
    # The approval survived the kill, but the purchase still needs its exact
    # Chain settlement (finality on both evidence sources), which the kill
    # interrupted mid-flight: the re-issued buy legitimately answers the
    # pending-settlement state until the receipt finalizes (observed live
    # 2026-09-07). Settle it the same bounded way the buy phase does, with
    # fresh post-restart System/Wallet tokens for any follow-up request.
    local restart_confirm_started_at
    restart_confirm_started_at="$(date +%s)"
    provider_call object buy "$library_token" "$buy_body"
    if [ "$PROVIDER_STATUS" != "ok" ]; then
        case "$CURL_BODY" in
        *"pending exact Wallet or Chain settlement"*) ;;
        *) fail "buy did not reach status=ok after restart and is not the pending-settlement state (approval $APPROVED_REQUEST_ID was already approved before the kill): $CURL_BODY" ;;
        esac
        CURRENT_STEP="gateway_launch:buyer:system:post-restart"
        gateway_launch "system" "${BUYER_AUTH_ARGS[@]}"
        system_token="$LAUNCH_TOKEN"
        CURRENT_STEP="gateway_launch:buyer:wallet:post-restart"
        gateway_launch "wallet" "${BUYER_AUTH_ARGS[@]}"
        wallet_token="$LAUNCH_TOKEN"
        CURRENT_STEP="buy_confirm_after_restart"
        log "buy after restart pending exact settlement (approval already granted); re-issuing until the purchase settles"
        settle_pending_call object buy "$library_token" "$buy_body" "pending exact Wallet or Chain settlement" \
            "restart" "$system_token" "$wallet_token" "$buyer_account_id" "$restart_confirm_started_at"
    fi
    [ "$PROVIDER_STATUS" = "ok" ] || fail "buy did not reach status=ok after restart (approval $APPROVED_REQUEST_ID was already approved before the kill): $CURL_BODY"
    local confirmed_json="$CURL_BODY"

    CURRENT_STEP="buy_replay_after_restart"
    provider_call object buy "$library_token" "$buy_body"
    [ "$PROVIDER_STATUS" = "ok" ] || fail "buy replay after restart did not reach status=ok: $CURL_BODY"
    local replayed_json="$CURL_BODY"
    if [ "$confirmed_json" != "$replayed_json" ]; then
        fail "post-restart buy replay is not byte-identical (possible duplicate transaction); confirmed=$confirmed_json replayed=$replayed_json"
    fi

    CURRENT_STEP="write_receipt_block:restart"
    local ok=true
    local block_json
    block_json="$(python3 -c '
import json
import sys

(commit, tree, clean, mint_id, request_id, pending_json, confirmed_json, replay_matches, ok) = sys.argv[1:10]
print(json.dumps({
    "ok": json.loads(ok),
    "git": {"commit": commit, "tree": tree, "clean": json.loads(clean)},
    "mint_id": mint_id,
    "approved_request_id": request_id,
    "buy_pending_before_kill": json.loads(pending_json),
    "buy_confirmed_after_restart": json.loads(confirmed_json),
    "buy_replay_matches": json.loads(replay_matches),
}))
' \
        "$GIT_COMMIT" "$GIT_TREE" "$GIT_CLEAN" "$mint_id" "$APPROVED_REQUEST_ID" \
        "$pending_json" "$confirmed_json" "$([ "$confirmed_json" = "$replayed_json" ] && echo true || echo false)" "$ok")"
    write_receipt_block restart "$block_json"

    log "restart phase complete: mint_id=$mint_id survived a SIGKILL between approval and confirmation, no duplicate transaction"
}

# --- cleanup phase (explicit close settles; boot sweeper settles a kill) --
#
# Part 1: an explicit open -> read -> close settles normally (evidence
# only -- reuses --mint-id from the receipt, the buyer's already-purchased
# item, exactly like --phase open).
# Part 2: open again, SIGKILL the client BEFORE closing (leaving the
# viewer lease CleanupPending -- protected_content_runtime.rs's
# RuntimeCustodyViewerLifecycleStatus::CleanupPending), restart, and prove
# the boot sweeper settled it: reconcile_runtime_custody_viewers_after_
# decrypt_boot (protected_content_runtime.rs:6791, invoked from
# server_infra.rs:1379 on every Runtime boot -- NOT line 1314 as originally
# briefed; the call site moved since, see the task report) runs
# automatically before the gateway ever serves a request, so a FRESH
# open_viewer for the same principal/mint_id succeeding again is the
# scriptable proof that nothing stayed stuck.
phase_cleanup() {
    CURRENT_STEP="check_git_clean"
    check_git_clean
    CURRENT_STEP="resolve_mint_id"
    resolve_mint_id
    CURRENT_STEP="resolve_elastos_bin"
    resolve_elastos_bin
    CURRENT_STEP="git_identity"
    git_identity
    CURRENT_STEP="compute_buyer_auth_args"
    compute_buyer_auth_args

    CURRENT_STEP="gateway_launch:buyer:elacity-player:explicit-close"
    gateway_launch "elacity-player" "${BUYER_AUTH_ARGS[@]}"
    local player_token="$LAUNCH_TOKEN"
    local open_body
    open_body="$(python3 -c 'import json,sys; print(json.dumps({"mint_id": sys.argv[1]}))' "$MINT_ID")"

    CURRENT_STEP="explicit_open"
    open_viewer_settling "$player_token" "$open_body" "restart:explicit-close"
    [ "$PROVIDER_STATUS" = "ok" ] || fail "explicit-close: open_viewer did not reach status=ok: $CURL_BODY"
    local handle
    handle="$(find_string_field "$CURL_BODY" viewer_session_handle)"
    [ -n "$handle" ] || fail "explicit-close: open_viewer did not return a viewer_session_handle: $CURL_BODY"

    CURRENT_STEP="explicit_read"
    local read_body
    # Init segment first, then segment 0: media parts are released in order.
    read_body="$(python3 -c 'import json,sys; print(json.dumps({"mint_id": sys.argv[1], "viewer_session_handle": sys.argv[2]}))' "$MINT_ID" "$handle")"
    provider_call object read_viewer "$player_token" "$read_body"
    [ "$PROVIDER_STATUS" = "ok" ] || fail "explicit-close: read_viewer (init segment) did not reach status=ok: $CURL_BODY"
    read_body="$(python3 -c 'import json,sys; print(json.dumps({"mint_id": sys.argv[1], "viewer_session_handle": sys.argv[2], "segment_index": 0}))' "$MINT_ID" "$handle")"
    provider_call object read_viewer "$player_token" "$read_body"
    [ "$PROVIDER_STATUS" = "ok" ] || fail "explicit-close: read_viewer did not reach status=ok: $CURL_BODY"

    CURRENT_STEP="explicit_close"
    local close_body
    close_body="$(python3 -c 'import json,sys; print(json.dumps({"mint_id": sys.argv[1], "viewer_session_handle": sys.argv[2]}))' "$MINT_ID" "$handle")"
    provider_call object close_viewer "$player_token" "$close_body"
    [ "$PROVIDER_STATUS" = "ok" ] || fail "explicit close_viewer did not reach status=ok: $CURL_BODY"
    local explicit_close_json="$CURL_BODY"
    log "explicit close_viewer settled: $explicit_close_json"

    CURRENT_STEP="gateway_launch:buyer:elacity-player:kill-mid-open"
    gateway_launch "elacity-player" "${BUYER_AUTH_ARGS[@]}"
    player_token="$LAUNCH_TOKEN"

    CURRENT_STEP="open_before_kill"
    open_viewer_settling "$player_token" "$open_body" "restart:before-kill"
    [ "$PROVIDER_STATUS" = "ok" ] || fail "open_viewer (before the mid-open kill) did not reach status=ok: $CURL_BODY"
    local abandoned_open_json="$CURL_BODY"
    log "opened a viewer session and abandoning it (no close) -- killing the client now: $abandoned_open_json"

    CURRENT_STEP="sigkill_client_gateway"
    sigkill_client_gateway

    CURRENT_STEP="restart_installed_client"
    restart_installed_client

    CURRENT_STEP="open_after_boot_sweep"
    gateway_launch "elacity-player" "${BUYER_AUTH_ARGS[@]}"
    player_token="$LAUNCH_TOKEN"
    open_viewer_settling "$player_token" "$open_body" "restart:after-boot-sweep"
    [ "$PROVIDER_STATUS" = "ok" ] || fail "open_viewer failed after restart -- the boot sweeper did not settle the abandoned CleanupPending viewer lease: $CURL_BODY"
    local resweep_open_json="$CURL_BODY"
    local resweep_handle
    resweep_handle="$(find_string_field "$resweep_open_json" viewer_session_handle)"
    log "open_viewer succeeded after restart -- boot sweeper settled the abandoned lease: $resweep_open_json"

    CURRENT_STEP="close_after_boot_sweep"
    local final_close_json=""
    if [ -n "$resweep_handle" ]; then
        local final_close_body
        final_close_body="$(python3 -c 'import json,sys; print(json.dumps({"mint_id": sys.argv[1], "viewer_session_handle": sys.argv[2]}))' "$MINT_ID" "$resweep_handle")"
        provider_call object close_viewer "$player_token" "$final_close_body"
        [ "$PROVIDER_STATUS" = "ok" ] || fail "final close_viewer after boot sweep did not reach status=ok: $CURL_BODY"
        final_close_json="$CURL_BODY"
    fi

    CURRENT_STEP="write_receipt_block:cleanup"
    local ok=true
    local block_json
    block_json="$(python3 -c '
import json
import sys

(commit, tree, clean, mint_id, explicit_close_json, abandoned_open_json,
 resweep_open_json, final_close_json, ok) = sys.argv[1:10]
print(json.dumps({
    "ok": json.loads(ok),
    "git": {"commit": commit, "tree": tree, "clean": json.loads(clean)},
    "mint_id": mint_id,
    "explicit_close_settles": json.loads(explicit_close_json),
    "abandoned_open_before_kill": json.loads(abandoned_open_json),
    "open_after_boot_sweep": json.loads(resweep_open_json),
    "final_close_after_boot_sweep": json.loads(final_close_json) if final_close_json else None,
}))
' \
        "$GIT_COMMIT" "$GIT_TREE" "$GIT_CLEAN" "$MINT_ID" "$explicit_close_json" "$abandoned_open_json" \
        "$resweep_open_json" "$final_close_json" "$ok")"
    write_receipt_block cleanup "$block_json"

    log "cleanup phase complete: explicit close settles; boot sweeper settled a mid-open kill"
}

# --- finalize phase (aggregate verdict, per-container hashes, evidence) --
#
# Reads the receipt every prior phase already wrote to (never re-runs a
# phase), so it can run any time after at least one other phase has -- but
# is only a meaningful "overall_ok" once run last. Brief Step 6 / Important
# 9 in the fix round.

CUSTODY_ELASTOS_CONTAINER_PATH=/home/custody/.local/share/elastos/bin/elastos
CUSTODY_PROVIDER_CONTAINER_PATH=/home/custody/.local/share/elastos/bin/custody-provider

# container_sha256_of SERVICE PATH -- best-effort "sha256:<hex>" of a file
# inside a running custody-host container (deploy/custody-host/Dockerfile's
# own bin/ layout), or empty if the container/file is unreachable. Never a
# hard failure: finalize's job is to record whatever evidence is available,
# not to require every container running.
container_sha256_of() {
    local svc="$1" path="$2"
    local out
    out="$(docker compose -f "$COMPOSE_FILE" exec -T "$svc" sha256sum "$path" 2>/dev/null | awk '{print $1}')" || true
    if [ -n "$out" ]; then
        printf 'sha256:%s' "$out"
    fi
    # Explicit, unconditional success: this is best-effort evidence, and a
    # missing container/binary must never abort finalize under `set -e`
    # (a bare `[ -n "$out" ] && printf ...` as this function's last
    # statement would otherwise return 1 whenever out is empty, which
    # set -e treats as this function's own failure at the call site's
    # `var="$(container_sha256_of ...)"` assignment).
    return 0
}

phase_finalize() {
    CURRENT_STEP="check_git_clean"
    check_git_clean
    CURRENT_STEP="git_identity"
    git_identity
    # Cheapest gate first (matching the ordering fix already applied to
    # phase_availability/phase_drill_replica/phase_restart/phase_cleanup):
    # a scratch dir with no prior receipt fails here, before ever touching
    # resolve_elastos_bin's self-healing `cargo build --release`.
    CURRENT_STEP="require_receipt"
    [ -f "$RECEIPT_PATH" ] || fail "no receipt at '$RECEIPT_PATH' to finalize; run at least one other phase first"

    CURRENT_STEP="resolve_elastos_bin"
    resolve_elastos_bin
    CURRENT_STEP="compute_artifact_hashes"
    compute_artifact_hashes

    CURRENT_STEP="aggregate_phase_verdicts"
    # overall_ok is the top-level verdict, so it must never pass on
    # ABSENCE (fix round 2, minor B): a receipt with only e.g.
    # provision:ok present would otherwise vacuously read as overall_ok:
    # true. required_phases is phase_all's own step list minus
    # chain-config-real, which phase_all itself already documents as
    # deliberately excluded (needs real operator-supplied RPC/contract
    # input, only relevant when pointing at a real chain deployment) --
    # so a legitimate placeholder/structural-only proof that never ran it
    # is not penalized, but any journey phase phase_all DOES always run
    # must be both present AND ok:true for overall_ok to be true.
    # chain-config-real's own verdict, when present, is still recorded
    # separately (optional_phase_verdicts) so it's never silently dropped.
    local required_phases=(provision preflight wallet_setup mint \
        availability buy open drill_custody drill_replica negative restart cleanup)
    local aggregate_json
    aggregate_json="$(RECEIPT_PATH="$RECEIPT_PATH" python3 -c '
import json
import os
import sys

path = os.environ["RECEIPT_PATH"]
required = sys.argv[1:]
with open(path) as handle:
    receipt = json.load(handle)

verdicts = {}
missing = []
for name in required:
    block = receipt.get(name)
    if block is None:
        missing.append(name)
        continue
    verdicts[name] = bool(block.get("ok"))

optional = {}
chain_config_block = receipt.get("chain_config_real")
if chain_config_block is not None:
    optional["chain_config_real"] = bool(chain_config_block.get("ok"))

overall_ok = (not missing) and bool(verdicts) and all(verdicts.values())
print(json.dumps({
    "required_phases": required,
    "phase_verdicts": verdicts,
    "missing_required_phases": missing,
    "optional_phase_verdicts": optional,
    "overall_ok": overall_ok,
}))
' "${required_phases[@]}")"
    local overall_ok
    overall_ok="$(printf '%s' "$aggregate_json" | python3 -c 'import json,sys; print("true" if json.load(sys.stdin)["overall_ok"] else "false")')"

    CURRENT_STEP="container_artifact_hashes"
    local container_entries=() svc elastos_sha custody_sha
    for svc in "${SERVICES[@]}"; do
        elastos_sha="$(container_sha256_of "$svc" "$CUSTODY_ELASTOS_CONTAINER_PATH")"
        custody_sha="$(container_sha256_of "$svc" "$CUSTODY_PROVIDER_CONTAINER_PATH")"
        container_entries+=("$(python3 -c 'import json,sys; print(json.dumps({"service": sys.argv[1], "elastos_sha256": sys.argv[2] or None, "custody_provider_sha256": sys.argv[3] or None}))' "$svc" "$elastos_sha" "$custody_sha")")
    done

    CURRENT_STEP="write_receipt_block:finalize"
    local ok=true
    local block_json
    block_json="$(python3 -c '
import json
import sys

(commit, tree, clean, aggregate_json,
 host_elastos_sha, container_entries_json,
 receipt_path, transcript_path, commands_path, ok) = sys.argv[1:11]
result = {
    "ok": json.loads(ok),
    "git": {"commit": commit, "tree": tree, "clean": json.loads(clean)},
    "artifacts": {
        "host_elastos_sha256": host_elastos_sha or None,
        "containers": [json.loads(e) for e in json.loads(container_entries_json)],
    },
    "evidence_paths": {
        "receipt": receipt_path,
        "transcript": transcript_path,
        "commands": commands_path,
    },
}
# required_phases/phase_verdicts/missing_required_phases/
# optional_phase_verdicts/overall_ok, computed above.
result.update(json.loads(aggregate_json))
print(json.dumps(result))
' \
        "$GIT_COMMIT" "$GIT_TREE" "$GIT_CLEAN" "$aggregate_json" \
        "$SOURCE_ELASTOS_SHA256" \
        "$(python3 -c 'import json,sys; print(json.dumps(sys.argv[1:]))' "${container_entries[@]}")" \
        "$RECEIPT_PATH" "$TRANSCRIPT_PATH" "$COMMANDS_PATH" "$ok")"
    write_receipt_block finalize "$block_json"

    log "finalize phase complete: overall_ok=$overall_ok"
}

# --- all phase (documented safe order) -----------------------------------
#
# provision, preflight, wallet-setup, mint, availability, buy, open,
# drill-custody, drill-replica, negative, restart, cleanup, finalize -- the
# only order that keeps every later phase's prerequisites satisfied (mint
# needs wallet-setup's accounts; buy/open/drills/negative need mint's
# mint_id and buy's purchase; negative's foreign/stale/tamper cases assume
# a healthy 3-node topology, so they run after the drills have already put
# every container back; restart mints its own throwaway item so it never
# disturbs the shared mint_id; cleanup runs before finalize because it
# deliberately leaves the client freshly restarted and finalize only reads
# the receipt every earlier phase already wrote). CURRENT_PHASE is re-set
# before each sub-phase so a failure partway through "all" still attributes
# the EXIT-trap failure receipt to the right block, exactly like running
# that one phase standalone would.
phase_all() {
    local step
    for step in provision:phase_provision preflight:phase_preflight \
        wallet_setup:phase_wallet_setup mint:phase_mint \
        availability:phase_availability buy:phase_buy open:phase_open \
        drill_custody:phase_drill_custody drill_replica:phase_drill_replica \
        negative:phase_negative restart:phase_restart cleanup:phase_cleanup \
        finalize:phase_finalize; do
        local phase_name="${step%%:*}" phase_fn="${step#*:}"
        log "=== all: ${phase_name} ==="
        CURRENT_PHASE="$phase_name"
        "$phase_fn"
    done
    log "all phases complete"
}

# --- dispatch -----------------------------------------------------------

case "$PHASE" in
provision)
    CURRENT_PHASE="provision"
    phase_provision
    ;;
preflight)
    CURRENT_PHASE="preflight"
    phase_preflight
    ;;
chain-config-real)
    CURRENT_PHASE="chain_config_real"
    phase_chain_config_real
    ;;
wallet-setup)
    CURRENT_PHASE="wallet_setup"
    phase_wallet_setup
    ;;
mint)
    CURRENT_PHASE="mint"
    phase_mint
    ;;
availability)
    CURRENT_PHASE="availability"
    phase_availability
    ;;
buy)
    CURRENT_PHASE="buy"
    phase_buy
    ;;
open)
    CURRENT_PHASE="open"
    phase_open
    ;;
drill-custody)
    CURRENT_PHASE="drill_custody"
    phase_drill_custody
    ;;
drill-replica)
    CURRENT_PHASE="drill_replica"
    phase_drill_replica
    ;;
negative)
    CURRENT_PHASE="negative"
    phase_negative
    ;;
restart)
    CURRENT_PHASE="restart"
    phase_restart
    ;;
cleanup)
    CURRENT_PHASE="cleanup"
    phase_cleanup
    ;;
finalize)
    CURRENT_PHASE="finalize"
    phase_finalize
    ;;
all)
    phase_all
    ;;
*)
    fail "unknown --phase '$PHASE' (expected provision|preflight|chain-config-real|wallet-setup|mint|availability|buy|open|drill-custody|drill-replica|negative|restart|cleanup|finalize|all)"
    ;;
esac

log "receipt: $RECEIPT_PATH"
# The transcript only exists once an HTTP-driving phase has run
# (record_transcript writes lazily); don't advertise a file that isn't there.
if [ -f "$TRANSCRIPT_PATH" ]; then
    log "transcript: $TRANSCRIPT_PATH"
else
    log "transcript: (none yet -- written by the HTTP phases, wallet-setup onward)"
fi
