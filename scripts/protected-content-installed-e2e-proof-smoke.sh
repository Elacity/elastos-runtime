#!/usr/bin/env bash
# Smoke test for scripts/protected-content-installed-e2e-proof.sh.
#
# No docker, no live gateway, no elastos binary build, no chain calls, and
# no destructive git operations -- every assertion here runs against
# scratch directories this script owns, plus one bounded read of the
# driver's own --help output and (for the dirty-tree gate) one transient,
# immediately-removed untracked file under the repo root. Follows the
# scripts/ smoke harness idiom in
# the retired provisional provider-contract smoke (mktemp -d +
# trap-guarded cleanup, plain PASS/FAIL lines, exit non-zero on the first
# failed assertion).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DRIVER="${ROOT}/scripts/protected-content-installed-e2e-proof.sh"
DIRTY_MARKER="${ROOT}/.protected-content-installed-e2e-proof-smoke-dirty-marker"

TMP_ROOT="$(mktemp -d)"
cleanup_smoke() {
    rm -f "$DIRTY_MARKER" 2>/dev/null || true
    rm -rf "$TMP_ROOT" 2>/dev/null || true
}
trap cleanup_smoke EXIT

pass() { printf '[protected-content-installed-e2e-proof-smoke] PASS: %s\n' "$*"; }
fail_smoke() {
    printf '[protected-content-installed-e2e-proof-smoke] FAIL: %s\n' "$*" >&2
    exit 1
}

[ -x "$DRIVER" ] || fail_smoke "driver is not executable: $DRIVER"

# --- 1. bash -n on the driver --------------------------------------------

bash -n "$DRIVER" || fail_smoke "bash -n failed on $DRIVER"
pass "bash -n on the driver"

# --- 2. usage lists every phase; unknown --phase fails naming them -------
#
# Exit codes are checked EXACTLY, not just "nonzero" (fix round 2): a bare
# `[ "$rc" -ne 0 ]` would happily accept 127 ("command not found") in place
# of the driver's real 0/1/2, which is exactly the shape of bug that let
# the on_exit/run_restore_stack forward-reference regression (see
# assertion 11 below) slip past the original fix round's smoke suite.

set +e
help_output="$("$DRIVER" --help 2>&1)"
help_rc=$?
set -e
[ "$help_rc" -eq 0 ] || fail_smoke "--help exited $help_rc, expected 0 (a nonzero/127 here means the EXIT trap hit something undefined before dispatch): $help_output"
grep -q "^Phases:" <<<"$help_output" || fail_smoke "--help did not print a 'Phases:' line"
for phase_name in provision preflight chain-config-real wallet-setup mint \
    availability buy open drill-custody drill-replica negative restart cleanup \
    finalize all; do
    grep -q "$phase_name" <<<"$help_output" \
        || fail_smoke "--help does not mention phase '$phase_name'"
done
pass "usage lists all 15 phases; --help exits 0"

unknown_scratch="$TMP_ROOT/unknown-phase/client-data"
mkdir -p "$unknown_scratch"
set +e
unknown_output="$("$DRIVER" --phase not-a-real-phase --client-data-dir "$unknown_scratch" --allow-dirty 2>&1)"
unknown_rc=$?
set -e
[ "$unknown_rc" -eq 1 ] || fail_smoke "unknown --phase exited $unknown_rc, expected 1 (fail()'s own exit code): $unknown_output"
grep -q "unknown --phase 'not-a-real-phase'" <<<"$unknown_output" \
    || fail_smoke "unknown --phase did not print the expected error: $unknown_output"
grep -q "provision" <<<"$unknown_output" || fail_smoke "unknown --phase error did not name the known phases: $unknown_output"
grep -q "drill-custody" <<<"$unknown_output" || fail_smoke "unknown --phase error did not name drill-custody: $unknown_output"
pass "unknown --phase fails (rc=1) and names the known phases"

no_phase_scratch="$TMP_ROOT/no-phase"
mkdir -p "$no_phase_scratch"
set +e
no_phase_output="$("$DRIVER" 2>&1)"
no_phase_rc=$?
set -e
[ "$no_phase_rc" -eq 2 ] || fail_smoke "missing --phase exited $no_phase_rc, expected 2 (the arg-parse usage/exit-2 path): $no_phase_output"
grep -q -- "--phase is required" <<<"$no_phase_output" \
    || fail_smoke "missing --phase did not print the expected error: $no_phase_output"
pass "missing --phase fails (rc=2) with a clear message"

poll0_scratch="$TMP_ROOT/poll0/client-data"
mkdir -p "$poll0_scratch"
set +e
poll0_output="$("$DRIVER" --phase wallet-setup --client-data-dir "$poll0_scratch" --allow-dirty --funding-poll-interval 0 2>&1)"
poll0_rc=$?
set -e
[ "$poll0_rc" -eq 1 ] || fail_smoke "--funding-poll-interval 0 exited $poll0_rc, expected 1: $poll0_output"
grep -q -- "--funding-poll-interval must be a positive integer" <<<"$poll0_output" \
    || fail_smoke "--funding-poll-interval 0 did not print the expected validation error: $poll0_output"
pass "--funding-poll-interval 0 fails (rc=1) at parse time"

# --- 3. refusal on a dirty tree without --allow-dirty ---------------------
#
# check_git_clean always targets $ROOT itself (not overridable per
# invocation), so genuinely proving the gate fires needs the real tree to
# be dirty at test time. This adds exactly one small untracked file under
# the repo root for the duration of a single driver invocation, and
# removes it immediately after (also guarded by the EXIT trap above, so it
# is removed even if this block fails).

echo "protected-content-installed-e2e-proof-smoke transient marker" >"$DIRTY_MARKER"
dirty_scratch="$TMP_ROOT/dirty-check/client-data"
mkdir -p "$dirty_scratch"
set +e
dirty_output="$("$DRIVER" --phase provision --client-data-dir "$dirty_scratch" 2>&1)"
dirty_rc=$?
set -e
rm -f "$DIRTY_MARKER"
[ "$dirty_rc" -ne 0 ] || fail_smoke "--phase provision unexpectedly succeeded against a dirty tree without --allow-dirty"
grep -q "working tree is dirty" <<<"$dirty_output" \
    || fail_smoke "dirty-tree run did not print the expected refusal: $dirty_output"
grep -q -- "--allow-dirty" <<<"$dirty_output" \
    || fail_smoke "dirty-tree refusal did not mention the --allow-dirty escape hatch: $dirty_output"
pass "refuses to run against a dirty tree without --allow-dirty"

# --- 4. receipt schema shape via a deliberate, fast, no-docker failure ----
#
# --phase chain-config-real without --real-rpc-url fails inside
# validate_real_chain_config_inputs, which runs BEFORE resolve_elastos_bin
# -- so this exercises the exact fail-closed receipt machinery (EXIT trap
# -> write_failure_receipt) without ever touching the elastos binary,
# docker, or a network call. Mirrors Task 7's deliberate-failure demo
# pattern (scratch --client-data-dir / --receipt-out, --allow-dirty).

receipt_scratch="$TMP_ROOT/receipt-shape"
mkdir -p "$receipt_scratch"
receipt_path="$receipt_scratch/receipt.json"
set +e
"$DRIVER" --phase chain-config-real \
    --allow-dirty \
    --client-data-dir "$receipt_scratch/client-data" \
    --receipt-out "$receipt_path" \
    >"$receipt_scratch/stdout.log" 2>"$receipt_scratch/stderr.log"
receipt_demo_rc=$?
set -e
[ "$receipt_demo_rc" -ne 0 ] || fail_smoke "chain-config-real without --real-rpc-url unexpectedly succeeded"
[ -f "$receipt_path" ] || fail_smoke "deliberate failure did not write a receipt at $receipt_path"

python3 - "$receipt_path" <<'PY'
import json
import sys

path = sys.argv[1]
with open(path) as handle:
    receipt = json.load(handle)

assert receipt.get("schema") == "elastos.protected-content.installed-e2e-proof/v1", receipt.get("schema")
assert receipt.get("version") == 1, receipt.get("version")

block = receipt.get("chain_config_real")
assert block is not None, "receipt is missing its chain_config_real block"
assert block.get("ok") is False, block.get("ok")
assert block.get("failed_step") == "validate_real_chain_config_inputs", block.get("failed_step")
assert "--real-rpc-url" in (block.get("error") or ""), block.get("error")
assert isinstance(block.get("exit_code"), int) and block["exit_code"] != 0, block.get("exit_code")

git_block = block.get("git") or {}
for key in ("commit", "tree", "clean"):
    assert key in git_block, f"receipt git block missing {key}: {git_block}"
PY
pass "deliberate failure writes an ok:false receipt with failed_step/error/exit_code/git"

# --- 5. chain-config-real argument validation (2..=5 distinct origins) ---
#
# All three cases fail inside validate_real_chain_config_inputs, the same
# no-docker/no-gateway/no-chain-call gate exercised above.

assert_chain_config_real_rejects() {
    local label="$1"
    shift
    local scratch="$TMP_ROOT/chain-config-real-${label}"
    mkdir -p "$scratch"
    local out rc
    set +e
    out="$("$DRIVER" --phase chain-config-real --allow-dirty \
        --client-data-dir "${scratch}/client-data" \
        --receipt-out "${scratch}/receipt.json" \
        "$@" 2>&1)"
    rc=$?
    set -e
    [ "$rc" -ne 0 ] || fail_smoke "chain-config-real (${label}) unexpectedly succeeded: $out"
    printf '%s' "$out"
}

too_few_output="$(assert_chain_config_real_rejects too-few-evidence \
    --real-rpc-url http://a.smoke.invalid \
    --real-evidence-rpc-url http://b.smoke.invalid)"
grep -q "2..=5" <<<"$too_few_output" \
    || fail_smoke "too-few-evidence did not name the 2..=5 requirement: $too_few_output"

too_many_output="$(assert_chain_config_real_rejects too-many-evidence \
    --real-rpc-url http://a.smoke.invalid \
    --real-evidence-rpc-url http://b.smoke.invalid \
    --real-evidence-rpc-url http://c.smoke.invalid \
    --real-evidence-rpc-url http://d.smoke.invalid \
    --real-evidence-rpc-url http://e.smoke.invalid \
    --real-evidence-rpc-url http://f.smoke.invalid \
    --real-evidence-rpc-url http://g.smoke.invalid)"
grep -q "2..=5" <<<"$too_many_output" \
    || fail_smoke "too-many-evidence did not name the 2..=5 requirement: $too_many_output"

duplicate_output="$(assert_chain_config_real_rejects duplicate-origin \
    --real-rpc-url http://a.smoke.invalid \
    --real-evidence-rpc-url http://b.smoke.invalid \
    --real-evidence-rpc-url http://b.smoke.invalid)"
grep -q "distinct" <<<"$duplicate_output" \
    || fail_smoke "duplicate-origin did not name the distinct-origins requirement: $duplicate_output"

missing_mint_output="$(assert_chain_config_real_rejects missing-mint-addresses \
    --real-rpc-url http://a.smoke.invalid \
    --real-evidence-rpc-url http://b.smoke.invalid \
    --real-evidence-rpc-url http://c.smoke.invalid)"
grep -q "placeholder mint addresses" <<<"$missing_mint_output" \
    || fail_smoke "missing real mint addresses did not refuse provision's placeholders: $missing_mint_output"

pass "chain-config-real validates 2..=5 distinct evidence RPC origins and real mint addresses client-side"

# --- 6. wallet_call is bash-3.2 `set -u` safe on its empty-body path -----
#
# Critical 1 in the fix round: on bash < 4.4, expanding an empty array as
# "${arr[@]}" under `set -u` is an "unbound variable" error -- the ONE
# empty-array call site this driver has (wallet_call's bodyless-GET
# curl-arg path) was fixed to `${extra[@]+"${extra[@]}"}`, but the modern
# Homebrew `bash` this repo's own shebang/PATH usually resolves to is 4.4+
# and would never have caught the regression. This extracts the REAL
# wallet_call() function body from the driver (not a copy) and exercises
# it under macOS's stock /bin/bash (3.2) when available, so a future
# regression of this exact class fails HERE instead of silently passing
# smoke and only breaking on a real macOS run (per reviewer minor 17).

SMOKE_BASH="/bin/bash"
[ -x "$SMOKE_BASH" ] || SMOKE_BASH="bash"

# extract_bash_function NAME SRC_FILE -- prints NAME() { ... } verbatim from
# SRC_FILE. NOT a naive `/^}/` sed range: several functions in the driver
# embed a `python3 -c '...'` body whose own dict/set literals close on a
# line that is exactly "}" (e.g. record_transcript's `entry = {...}`),
# which would truncate a naive range mid-string. Counts brace characters
# instead (python's embedded braces are always self-balanced across the
# function body, so a brace count still lands on that occurrence's own
# closing "}" first).
#
# Selects the LAST brace-balanced occurrence of NAME() { ... } in the
# file, not the first: bash itself resolves a function name to whichever
# definition it most recently executed, and a name can legitimately have
# an earlier throwaway/forward-declared definition before its real one
# (e.g. run_restore_stack's own bash-3.2 forward-declaration stub,
# `run_restore_stack() { :; }`, ahead of `trap on_exit EXIT` -- see fix
# round 2). A first-match extractor would silently grab that one-line stub
# instead of the real, later body and make every assertion built on it
# pass trivially without exercising any real logic. Scans the WHOLE file
# (no early exit) and keeps only the most recently completed match.
extract_bash_function() {
    local fn_name="$1" src_file="$2"
    awk -v fn="$fn_name" '
    BEGIN { depth = 0; capturing = 0; last_n = 0; buf_n = 0 }
    {
        if (!capturing) {
            if ($0 ~ "^" fn "\\(\\) \\{") {
                capturing = 1
                depth = 0
                buf_n = 0
            } else {
                next
            }
        }
        buf_n++
        buf[buf_n] = $0
        o = gsub(/\{/, "{")
        depth += o
        c = gsub(/\}/, "}")
        depth -= c
        if (capturing && depth == 0) {
            capturing = 0
            last_n = buf_n
            for (i = 1; i <= buf_n; i++) {
                last_buf[i] = buf[i]
            }
        }
    }
    END {
        for (i = 1; i <= last_n; i++) {
            print last_buf[i]
        }
    }
    ' "$src_file"
}

wallet_call_fn="$TMP_ROOT/wallet_call.fn.sh"
extract_bash_function wallet_call "$DRIVER" >"$wallet_call_fn"
[ -s "$wallet_call_fn" ] || fail_smoke "could not extract wallet_call() from the driver for the bash -u regression check"

wallet_call_harness="$TMP_ROOT/wallet_call_harness.sh"
cat >"$wallet_call_harness" <<'HARNESS'
set -euo pipefail
log() { :; }
redact_token() { printf '<redacted>'; }
curl_call() { CURL_HTTP_CODE=200; CURL_BODY='{}'; }
record_transcript() { :; }
GATEWAY_URL="http://gateway.smoke.invalid"
HARNESS
{
    printf '. %q\n' "$wallet_call_fn"
    printf 'wallet_call GET "/api/apps/system/wallet/approvals" "sometoken" ""\n'
    printf 'echo WALLET_CALL_OK\n'
} >>"$wallet_call_harness"

set +e
wallet_call_out="$("$SMOKE_BASH" "$wallet_call_harness" 2>&1)"
wallet_call_rc=$?
set -e
{ [ "$wallet_call_rc" -eq 0 ] && grep -q "WALLET_CALL_OK" <<<"$wallet_call_out"; } \
    || fail_smoke "wallet_call's empty-body curl-arg path failed under $SMOKE_BASH (bash-3.2 unbound-array regression?): $wallet_call_out"
pass "wallet_call's empty-body curl-arg path is bash-3.2 set -u safe ($SMOKE_BASH: $("$SMOKE_BASH" --version | head -1))"

# --- 7. record_transcript redacts secrets ---------------------------------
#
# Important 5 in the fix round: the transcript is the evidence artifact
# meant to be reviewed/attached, so it must never carry raw recovery_key/
# step_up_token/home_token values. Extracts the REAL record_transcript()
# function and exercises it directly with fake secret-bearing request/
# response JSON, then asserts none of the raw secret values survive in the
# written line and a redaction marker is present instead.

record_transcript_fn="$TMP_ROOT/record_transcript.fn.sh"
extract_bash_function record_transcript "$DRIVER" >"$record_transcript_fn"
[ -s "$record_transcript_fn" ] || fail_smoke "could not extract record_transcript() from the driver for the redaction check"

transcript_scratch="$TMP_ROOT/transcript-redaction"
mkdir -p "$transcript_scratch"
transcript_test_path="$transcript_scratch/transcript.jsonl"

secret_recovery_key="SUPER-SECRET-RECOVERY-KEY-MATERIAL-$$"
secret_step_up="SUPER-SECRET-STEP-UP-TOKEN-$$"
secret_home_token="SUPER-SECRET-HOME-TOKEN-VALUE-$$"

request_body="$(python3 -c 'import json,sys; print(json.dumps({"step_up_token": sys.argv[1], "recovery_key": sys.argv[2], "label": "smoke"}))' "$secret_step_up" "$secret_recovery_key")"
response_body="$(python3 -c 'import json,sys; print(json.dumps({"route": "/apps/elacity-player/?a=b#home_token=" + sys.argv[1] + "&x=1"}))' "$secret_home_token")"

record_transcript_harness="$TMP_ROOT/record_transcript_harness.sh"
cat >"$record_transcript_harness" <<'HARNESS'
set -euo pipefail
HARNESS
{
    printf 'TRANSCRIPT_PATH=%q\n' "$transcript_test_path"
    printf '. %q\n' "$record_transcript_fn"
    printf 'record_transcript POST %q %q 200 %q\n' \
        "/api/apps/wallet/wallet/accounts/import-recovery-key" "$request_body" "$response_body"
} >>"$record_transcript_harness"

set +e
"$SMOKE_BASH" "$record_transcript_harness" >/dev/null 2>"$transcript_scratch/stderr.log"
record_transcript_rc=$?
set -e
[ "$record_transcript_rc" -eq 0 ] || fail_smoke "record_transcript harness failed: $(cat "$transcript_scratch/stderr.log")"
[ -f "$transcript_test_path" ] || fail_smoke "record_transcript did not write $transcript_test_path"

transcript_line="$(cat "$transcript_test_path")"
if grep -qF "$secret_recovery_key" <<<"$transcript_line"; then
    fail_smoke "transcript leaked the raw recovery_key value: $transcript_line"
fi
if grep -qF "$secret_step_up" <<<"$transcript_line"; then
    fail_smoke "transcript leaked the raw step_up_token value: $transcript_line"
fi
if grep -qF "$secret_home_token" <<<"$transcript_line"; then
    fail_smoke "transcript leaked the raw home_token fragment value: $transcript_line"
fi
grep -q '<redacted:' <<<"$transcript_line" \
    || fail_smoke "transcript did not redact any secret field (expected a <redacted:N-chars> marker): $transcript_line"
python3 -c "
import json
with open('$transcript_test_path') as handle:
    json.loads(handle.read().strip())
" || fail_smoke "transcript line is not valid JSON: $transcript_line"
pass "record_transcript redacts recovery_key/step_up_token/home_token fragments"

# --- 8. restore-stack push/remove/run is bash-3.2 set -u safe ------------
#
# Important 4's restore stack (push_restore_docker_start/
# remove_restore_docker_start/push_restore_tamper/remove_restore_tamper/
# run_restore_stack) re-packs its parallel arrays after removing an entry
# ("RESTORE_KINDS=(\"${RESTORE_KINDS[@]}\")" and 3 siblings) -- when the
# removed entry was the LAST one, that re-pack is itself exactly Critical
# 1's bug class (an unguarded "${arr[@]}" of a now-empty array under
# bash-3.2 `set -u`). This was caught and fixed during this same fix round
# by dry-running the pack-to-zero-elements case directly; this assertion
# keeps that regression covered going forward.

restore_stack_fns="$TMP_ROOT/restore_stack.fn.sh"
: >"$restore_stack_fns"
for fn_name in push_restore_docker_start remove_restore_docker_start \
    push_restore_tamper remove_restore_tamper run_restore_stack; do
    fn_extracted="$(extract_bash_function "$fn_name" "$DRIVER")"
    [ -n "$fn_extracted" ] || fail_smoke "could not extract ${fn_name}() from the driver"
    # Every one of these 5 real bodies is multi-line; a 1-line extraction
    # means the extractor grabbed a stub/forward-declaration instead of
    # the real definition (exactly the run_restore_stack regression this
    # smoke script itself introduced and is now guarding against) --
    # fail loudly here rather than letting a later assertion pass
    # trivially against dead code.
    fn_line_count="$(printf '%s\n' "$fn_extracted" | wc -l | tr -d ' ')"
    [ "$fn_line_count" -gt 1 ] || fail_smoke "${fn_name}() extracted as only $fn_line_count line(s) -- extractor likely grabbed a stub, not the real body: $fn_extracted"
    printf '%s\n\n' "$fn_extracted" >>"$restore_stack_fns"
done
[ -s "$restore_stack_fns" ] || fail_smoke "could not extract the restore-stack functions from the driver"

restore_stack_harness="$TMP_ROOT/restore_stack_harness.sh"
cat >"$restore_stack_harness" <<'HARNESS'
set -euo pipefail
log() { :; }
COMPOSE_FILE="/nonexistent/compose.yml"
docker() { :; }
RESTORE_KINDS=()
RESTORE_ARG1=()
RESTORE_ARG2=()
RESTORE_ARG3=()
HARNESS
{
    printf '. %q\n' "$restore_stack_fns"
    cat <<'HARNESS'
push_restore_docker_start custody-a
[ "${#RESTORE_KINDS[@]}" -eq 1 ] || { echo "push did not add an entry"; exit 1; }
remove_restore_docker_start custody-a
[ "${#RESTORE_KINDS[@]}" -eq 0 ] || { echo "remove of the last entry did not empty the stack"; exit 1; }
run_restore_stack
[ "${#RESTORE_KINDS[@]}" -eq 0 ] || { echo "run_restore_stack left entries behind"; exit 1; }
echo RESTORE_STACK_OK
HARNESS
} >>"$restore_stack_harness"

set +e
restore_stack_out="$("$SMOKE_BASH" "$restore_stack_harness" 2>&1)"
restore_stack_rc=$?
set -e
{ [ "$restore_stack_rc" -eq 0 ] && grep -q "RESTORE_STACK_OK" <<<"$restore_stack_out"; } \
    || fail_smoke "restore-stack push/remove/run failed under $SMOKE_BASH (bash-3.2 unbound-array regression on empty re-pack?): $restore_stack_out"
pass "restore-stack push/remove-to-empty/run is bash-3.2 set -u safe"

printf '[protected-content-installed-e2e-proof-smoke] all checks passed\n'
