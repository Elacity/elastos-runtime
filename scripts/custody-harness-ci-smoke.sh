#!/usr/bin/env bash
# ELACITY-2298 Task 10: CI dKMS ceremony + custody harness smoke.
#
# Thin, fully self-contained rehearsal of the installed e2e proof's
# `provision` + `preflight` phases (scripts/protected-content-installed-e2e-proof.sh)
# against a *fresh* instance of the deploy/custody-host 3-node compose
# harness: build the image, bring the harness up, run the offline
# composition ceremony and the Carrier dial-proof preflight against a
# throwaway client identity, assert both receipt blocks report `ok: true`,
# then tear everything down. This is the machinery smoke test the CI job
# `custody-harness-smoke` runs on every PR touching the custody-host image,
# the proof driver, or the protected-content/carrier crates -- it is not a
# substitute for the full installed e2e proof (mint/buy/open/drill/negative
# phases), which needs a live installed client, real HTTP journey and Home
# session tokens, none of which belong in an automated CI job.
#
# Everything this script touches is throwaway: a fresh $(mktemp -d) client
# identity/data dir, a fresh policy authority key, a fresh compose project
# (COMPOSE_PROJECT_NAME=custody-ci-smoke, distinct from the harness's
# default "custody-host" project so its own named volumes/state never
# collide with an already-provisioned deployment). The real client data dir
# ($HOME/Library/Application Support/elastos on the machine running this)
# is never read or written by this script.
#
# --- Why a distinct compose project isn't full isolation on its own -----
#
# deploy/custody-host/docker-compose.yml hardcodes host port bindings
# (127.0.0.1:1443{1,2,3}:4433/udp) and up.sh hardcodes its shared/
# descriptor-and-ticket handoff directory and its .env
# (CUSTODY_TRUSTED_RUNTIME_ISSUER) file -- none of the three are scoped by
# compose project name, and this script is not permitted to edit up.sh or
# docker-compose.yml to add that scoping. So a second, concurrently *running*
# project cannot share the host with an already-up "custody-host" project:
# it would fail to bind the same UDP ports. This script therefore, when it
# finds the default project already running (the common case on a dev
# machine that already has a harness up from earlier manual testing):
#   1. stops (not destroys -- `docker compose down`, no `-v`) the default
#      project just long enough to free the ports, preserving its named
#      volumes (and therefore its already-provisioned custody state) intact,
#   2. runs its own project end to end on those now-free ports,
#   3. tears its own project down (`down -v`, its volumes only),
#   4. removes only the exact 3 descriptor/ticket files *this run* added to
#      the shared shared/ directory (recorded by DID as they're created --
#      never a blind glob-delete of that directory, which would also delete
#      the default project's own files),
#   5. restores deploy/custody-host/.env to what it held before this script
#      touched it,
#   6. and, only if it was the one that stopped the default project, brings
#      it back up (`docker compose up -d`, no rebuild -- the already-
#      provisioned volumes mean this resumes exactly where it left off).
# All of this runs from a single EXIT trap, so it happens on success,
# on a hard failure, and on Ctrl-C alike.
#
# --- Why the ceremony's own client identity must be $HOME-derived -------
#
# `elastos node peer add|list|status` and `elastos identity show` have no
# --data-dir flag; they always resolve the OS-native default data dir from
# $HOME (Darwin: "$HOME/Library/Application Support/elastos"; Linux:
# "${XDG_DATA_HOME:-$HOME/.local/share}/elastos" -- confirmed against the
# CLI's own behavior in Task 6's up.sh and reconfirmed here). So a genuinely
# fresh, isolated client identity for the peer-add/dial-proof steps needs
# $HOME itself thrown away for the whole proof-driver invocation, not just
# --client-data-dir passed as a flag -- and --client-data-dir is set to
# that exact same $HOME-derived path so the explicitly-flagged ceremony
# commands (generate-custody-composition, generate-chain-config, node
# peer add's target, etc.) and the identity-derived ones agree on one
# directory.
#
# --- Why provision's install/restart step is (honestly) skipped ---------
#
# `--phase provision` always attempts `ensure_installed_from_this_tree`,
# which runs `scripts/setup-source-home.sh` (full ~30-workspace,
# 33-app-capsule source-home build) whenever the throwaway client data dir
# has no installed-source-home receipt matching this tree's HEAD commit --
# which, for a brand new throwaway dir, is always. That build has nothing
# to do with what this smoke test needs to prove (the dKMS ceremony and the
# 3-node dial proof), and unlike Task 7's live run (which hit a real,
# incidental 16 GiB disk-space gate) this host and CI runners both may well
# have enough free space to actually attempt it -- an unbounded, unrelated
# multi-minute build is not an acceptable cost for a "smoke" job.
#
# The proof driver has no --skip-install/--skip-restart flag (checked: see
# its usage()); what it *does* have is the honest mechanism the task brief
# points at: `restart_client()` already, unconditionally, skips the actual
# client restart whenever the install step didn't land a tree-matching
# receipt (INSTALL_OK=0) -- logged as a GAP line, not a fail(), and
# provision's `ok` field does not depend on it (see phase_provision: `local
# provision_ok=true` is unconditional once every *earlier* step succeeds).
# This script leans on that existing mechanism rather than faking a receipt
# (which would be dishonest -- claiming something is installed when it
# isn't) or teaching itself a private skip switch. Concretely: it exports
# ELASTOS_DATA_DIR set to a value that deliberately does NOT match the
# throwaway $HOME's own default data dir. scripts/setup-source-home.sh
# already, and independently of this task, treats a mismatched
# ELASTOS_DATA_DIR as a hard configuration error and refuses to run --
# documented in its own --help text ("ELASTOS_DATA_DIR is intentionally
# not accepted as a gateway data-root override") and enforced as the very
# first check in the script, before it even looks at free disk space or
# resolves a toolchain. That's a real, accurate mismatch (this script
# genuinely did not intend ELASTOS_DATA_DIR to mean anything here), not a
# fabricated one, and it fails fast and cleanly -- no filesystem writes
# happen before that check exits. ensure_installed_from_this_tree treats
# the resulting nonzero exit exactly like Task 7's disk-space failure: log
# the GAP, set INSTALL_OK=0, move on. restart_client() then takes its
# already-built-in skip path. Verified empirically below (see the captured
# transcript in the task report): "GAP: installed source-home still does
# not match this working tree after setup-source-home.sh" appears, and
# "skipping client restart" follows it, and provision.ok is still true.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
COMPOSE_FILE="${ROOT}/deploy/custody-host/docker-compose.yml"
DEFAULT_COMPOSE_ARGS=(-f "$COMPOSE_FILE")
SMOKE_PROJECT="custody-ci-smoke"
SMOKE_COMPOSE_ARGS=(-f "$COMPOSE_FILE" -p "$SMOKE_PROJECT")
SHARED_LIVE_DIR="${ROOT}/deploy/custody-host/shared"
ENV_FILE="${ROOT}/deploy/custody-host/.env"

log() { printf '%s\n' "$*" >&2; }
fail() {
    printf 'FAIL: %s\n' "$*" >&2
    exit 1
}

# --- state the cleanup trap needs, set as the script progresses ---------
WORKDIR=""
STOPPED_DEFAULT_PROJECT=false
SMOKE_PROJECT_UP=false
ENV_BACKUP=""
ENV_EXISTED=false
NEW_DIDS=()
OVERALL_OK=false
# Set once known (see the provision-phase block below); declared here too
# (empty) so `set -u` never trips if cleanup fires before that point (e.g.
# a failure inside `up.sh up`, before any phase has even started).
RECEIPT_PATH=""
# A fixed, non-mktemp path so a CI step can find it by name after this
# trap has already deleted $WORKDIR (the receipt itself lives under
# $WORKDIR/home/.../elastos/receipts/, which is why it must be copied out
# *before* the unconditional `rm -rf "$WORKDIR"` below, on failure only --
# a passing run has nothing worth preserving here).
EVIDENCE_DIR="${TMPDIR:-/tmp}/custody-harness-ci-smoke-evidence"

cleanup() {
    local exit_code=$?
    log ""
    log "== cleanup (exit code so far: ${exit_code}) =="
    local passed=false
    if [ "$OVERALL_OK" = true ] && [ "$exit_code" -eq 0 ]; then
        passed=true
    fi

    # `set -e` is live inside this trap: every step below that isn't a
    # structural prerequisite for a LATER step must be `|| log "WARN: ..."`
    # guarded, never left to abort the trap outright -- an unguarded failure
    # here (a read-only .env, a full /tmp, bad dir perms) must never strand
    # the operator's real harness down. Ordering matters too, independent of
    # the guards: the default-project restore (the one step that actually
    # matters to the operator) runs before the purely-diagnostic evidence
    # copy, so even a future edit that drops a guard can't put the restore
    # behind a step that has no business blocking it. `.env` restore is the
    # one exception that must stay ahead of the default-project restore: the
    # compose file's `${CUSTODY_TRUSTED_RUNTIME_ISSUER:?...}` interpolation
    # is evaluated at `docker compose up` config-render time, unconditionally
    # (even though an already-provisioned restart never uses the value) --
    # an empty/missing .env fails that `up` outright, restore included.
    # Diagnostic, but it has to run BEFORE the teardown below erases the only
    # record of what the nodes did: on failure, keep the smoke project's
    # container state and full logs next to the evidence receipt (the CI job
    # uploads that directory). Guarded like everything else in this trap.
    if [ "$SMOKE_PROJECT_UP" = true ] && [ "$passed" = false ]; then
        log "-- run failed: preserving ${SMOKE_PROJECT} container state and logs at ${EVIDENCE_DIR} (before teardown) --"
        (mkdir -p "$EVIDENCE_DIR" \
            && docker compose "${SMOKE_COMPOSE_ARGS[@]}" ps -a >"${EVIDENCE_DIR}/compose-ps.txt" 2>&1 \
            && docker compose "${SMOKE_COMPOSE_ARGS[@]}" logs --no-color >"${EVIDENCE_DIR}/compose-logs.txt" 2>&1) \
            || log "WARN: failed to preserve the ${SMOKE_PROJECT} container logs at ${EVIDENCE_DIR}"
    fi

    if [ "$SMOKE_PROJECT_UP" = true ]; then
        log "-- tearing down ${SMOKE_PROJECT} project (containers + its own volumes) --"
        docker compose "${SMOKE_COMPOSE_ARGS[@]}" down -v || log "WARN: teardown of ${SMOKE_PROJECT} project reported an error; inspect with: docker compose ${SMOKE_COMPOSE_ARGS[*]} ps -a"
    fi

    # NEW_DIDS is only recorded once up.sh has returned successfully; an
    # up.sh that fails AFTER the nodes exported their descriptors (a
    # readiness or handoff failure) would otherwise leave this run's files in
    # the live shared/ and poison the next run's distinctness check. Recover
    # them from the before-snapshot the same way the success path does.
    if [ "${#NEW_DIDS[@]}" -eq 0 ] && [ "$SMOKE_PROJECT_UP" = true ]; then
        local orphan_paths orphan
        orphan_paths="$(comm -13 <(printf '%s\n' "$before_descriptors") \
            <(find "$SHARED_LIVE_DIR" -maxdepth 1 -name '*.descriptor.json' 2>/dev/null | sort))" || orphan_paths=""
        while IFS= read -r orphan; do
            [ -n "$orphan" ] || continue
            NEW_DIDS+=("$(basename "$orphan" .descriptor.json)")
        done <<<"$orphan_paths"
        [ "${#NEW_DIDS[@]}" -eq 0 ] || log "-- up.sh failed after ${#NEW_DIDS[@]} descriptor(s) landed; treating them as this run's --"
    fi

    if [ "${#NEW_DIDS[@]}" -gt 0 ]; then
        log "-- removing this run's ${#NEW_DIDS[@]} descriptor/ticket file(s) from ${SHARED_LIVE_DIR} (by DID, never a blind glob) --"
        local did
        for did in "${NEW_DIDS[@]}"; do
            rm -f "${SHARED_LIVE_DIR}/${did}.descriptor.json" "${SHARED_LIVE_DIR}/${did}.ticket" \
                || log "WARN: failed to remove one of ${did}'s descriptor/ticket files from ${SHARED_LIVE_DIR}; remove by hand if they're stale"
        done
    fi

    if [ -n "$ENV_BACKUP" ] && [ -f "$ENV_BACKUP" ]; then
        if [ "$ENV_EXISTED" = true ]; then
            log "-- restoring ${ENV_FILE} to its pre-run content --"
            cp "$ENV_BACKUP" "$ENV_FILE" \
                || log "WARN: failed to restore ${ENV_FILE} from ${ENV_BACKUP}; the default project's restore below may now fail too -- restore it by hand before retrying: cp '${ENV_BACKUP}' '${ENV_FILE}'"
        else
            log "-- ${ENV_FILE} did not exist before this run; removing the one this run wrote --"
            rm -f "$ENV_FILE" || log "WARN: failed to remove ${ENV_FILE} (didn't exist before this run)"
        fi
    fi

    if [ "$STOPPED_DEFAULT_PROJECT" = true ]; then
        log "-- restoring the default custody-host project this run stopped to free ports --"
        # Must be unset here: DEFAULT_COMPOSE_ARGS carries no -p flag, so it
        # relies on docker compose's directory-basename project-name
        # resolution, which COMPOSE_PROJECT_NAME (exported below, for the
        # proof driver's own unscoped `docker compose exec` calls to reach
        # the smoke project) would otherwise override.
        unset COMPOSE_PROJECT_NAME
        if docker compose "${DEFAULT_COMPOSE_ARGS[@]}" up -d; then
            log "default project restored (already-provisioned volumes resumed, no rebuild)"
        else
            log "WARN: failed to bring the default custody-host project back up; operator must run: docker compose -f ${COMPOSE_FILE} up -d"
        fi
    fi

    # Purely diagnostic from here down: nothing below this line is load-
    # bearing for the operator's harness, which is already fully restored
    # (or its restore already attempted and WARNed) above.
    if [ "$passed" = false ] && [ -n "$RECEIPT_PATH" ] && [ -f "$RECEIPT_PATH" ]; then
        log "-- run failed: preserving the evidence receipt at ${EVIDENCE_DIR} (before \$WORKDIR is removed below) --"
        (mkdir -p "$EVIDENCE_DIR" && cp "$RECEIPT_PATH" "${EVIDENCE_DIR}/protected-content-installed-e2e-proof.json") \
            || log "WARN: failed to preserve the evidence receipt at ${EVIDENCE_DIR}; it may still be readable at ${RECEIPT_PATH} until \$WORKDIR is removed next"
    fi

    if [ -n "$WORKDIR" ] && [ -d "$WORKDIR" ]; then
        rm -rf "$WORKDIR" || log "WARN: failed to remove throwaway \$WORKDIR ${WORKDIR}; remove it by hand"
    fi

    # Fold minor: a false pass must never carry a zero exit code -- makes an
    # "OVERALL_OK=false but exit_code=0" mismatch structurally impossible
    # (it was already unreachable given how OVERALL_OK/exit are set, but a
    # future edit shouldn't be able to reintroduce it silently).
    if [ "$passed" = false ] && [ "$exit_code" -eq 0 ]; then
        exit_code=1
    fi

    if [ "$passed" = true ]; then
        log ""
        log "custody-harness-ci-smoke: PASS"
    else
        log ""
        log "custody-harness-ci-smoke: FAIL (exit ${exit_code})"
    fi
    exit "$exit_code"
}
# INT/TERM too, not just EXIT: a SIGTERM mid-run (routine under CI's
# cancel-in-progress, or an operator's Ctrl-C twice) must still run this
# same restore path rather than leave .env rewritten and the default
# harness down with no cleanup at all.
trap cleanup EXIT INT TERM

# --- host elastos binary (same self-heal pattern as up.sh / the proof driver) ---

ELASTOS_BIN="${ROOT}/elastos/target/release/elastos"
if [ ! -x "$ELASTOS_BIN" ] || ! "$ELASTOS_BIN" --version >/dev/null 2>&1; then
    log "host elastos binary missing or not runnable at $ELASTOS_BIN; building (cargo build --release -p elastos-server)"
    (cd "$ROOT" && cargo build --release --manifest-path elastos/Cargo.toml -p elastos-server)
fi
log "host elastos binary: $("$ELASTOS_BIN" --version)"

# --- throwaway everything -------------------------------------------------

WORKDIR="$(mktemp -d)"
THROWAWAY_HOME="${WORKDIR}/home"
mkdir -p "$THROWAWAY_HOME"
case "$(uname -s)" in
Darwin) CLIENT_DATA_DIR="${THROWAWAY_HOME}/Library/Application Support/elastos" ;;
Linux) CLIENT_DATA_DIR="${THROWAWAY_HOME}/.local/share/elastos" ;;
*) fail "unsupported OS for this smoke test: $(uname -s)" ;;
esac
POLICY_AUTHORITY_KEY="${WORKDIR}/policy-authority.key"
SHARED_DIR="${WORKDIR}/shared"
mkdir -p "$SHARED_DIR"

# The driver's own preflight/provision phases shell out to `docker compose
# exec` directly (check_ready_receipt); when they're invoked below with
# HOME="$THROWAWAY_HOME" for identity isolation, the `docker` CLI would
# otherwise also lose its config/plugin lookup, which docker resolves from
# $HOME/.docker (Docker Desktop's `compose` subcommand is a CLI plugin
# discovered there, and its current *context* -- e.g. "desktop-linux", not
# the "default" unix:///var/run/docker.sock -- lives in that same
# config.json). Captured here, before HOME is ever overridden, and passed
# as DOCKER_CONFIG (which the docker CLI honors independently of HOME) on
# every throwaway-HOME invocation below, so `docker compose` keeps
# resolving against the real invoking user's config/context regardless of
# which identity's HOME the elastos CLI calls in the same command see.
# Empirically confirmed: without this, `docker compose exec` fails
# ("unknown shorthand flag: 'f' in -f" -- the compose plugin isn't found at
# all) on a machine using a non-default Docker context (Docker Desktop);
# harmless to set even where it isn't needed (a system-wide compose plugin,
# as most Linux CI runners install, needs no HOME-based discovery at all).
REAL_DOCKER_CONFIG="${DOCKER_CONFIG:-${HOME}/.docker}"

log "== throwaway client identity: HOME=${THROWAWAY_HOME} =="
log "== throwaway client data dir: ${CLIENT_DATA_DIR} =="
log "== throwaway policy authority key: ${POLICY_AUTHORITY_KEY} =="
log "== throwaway shared dir (this run's 3 descriptors only): ${SHARED_DIR} =="

# `identity show` auto-creates a fresh device key the first time it's asked
# for one; doing that now (rather than letting up.sh's own fallback do it
# under a *different*, script-generated throwaway HOME) means the exact
# CLIENT_DATA_DIR computed above already has a device key by the time
# up.sh's `show-runtime-issuer` call needs one.
HOME="$THROWAWAY_HOME" XDG_DATA_HOME="${THROWAWAY_HOME}/.local/share" \
    "$ELASTOS_BIN" identity show >/dev/null

# --- free the fixed host ports (14431-14433/udp) if the default project is up ---

if [ -n "$(docker compose "${DEFAULT_COMPOSE_ARGS[@]}" ps -q 2>/dev/null)" ]; then
    log "== default custody-host project is running; stopping it (volumes preserved) to free ports 14431-14433/udp for ${SMOKE_PROJECT} =="
    docker compose "${DEFAULT_COMPOSE_ARGS[@]}" down
    STOPPED_DEFAULT_PROJECT=true
fi

# --- snapshot .env and shared/ before this run's up.sh touches them ------

if [ -f "$ENV_FILE" ]; then
    ENV_EXISTED=true
    ENV_BACKUP="${WORKDIR}/env.backup"
    cp "$ENV_FILE" "$ENV_BACKUP"
else
    ENV_EXISTED=false
    ENV_BACKUP="${WORKDIR}/env.backup.sentinel"
    : >"$ENV_BACKUP"
fi

before_descriptors="$(find "$SHARED_LIVE_DIR" -maxdepth 1 -name '*.descriptor.json' 2>/dev/null | sort)"

# --- build + bring up a fresh, isolated harness instance ------------------
#
# Exported (not just prefixed onto the up.sh call below): the proof driver's
# own preflight phase calls `docker compose -f "$COMPOSE_FILE" exec ...`
# with no -p flag of its own (it has no notion of compose project names at
# all), so it needs COMPOSE_PROJECT_NAME in its inherited environment to
# reach this run's containers instead of the directory-basename-derived
# default ("custody-host") project.
export COMPOSE_PROJECT_NAME="$SMOKE_PROJECT"

log "== COMPOSE_PROJECT_NAME=${SMOKE_PROJECT} deploy/custody-host/up.sh up '${CLIENT_DATA_DIR}' =="
# Marked BEFORE the call: up.sh starts the containers and only then polls
# and verifies them, so a failure inside up.sh must still tear this
# project down (and reclaim its descriptors) -- a `down -v` on a project
# that never came up is harmless.
SMOKE_PROJECT_UP=true
"${ROOT}/deploy/custody-host/up.sh" up "$CLIENT_DATA_DIR"

after_descriptors="$(find "$SHARED_LIVE_DIR" -maxdepth 1 -name '*.descriptor.json' 2>/dev/null | sort)"
new_descriptor_paths="$(comm -13 <(printf '%s\n' "$before_descriptors") <(printf '%s\n' "$after_descriptors"))"
new_count="$(printf '%s\n' "$new_descriptor_paths" | grep -c . || true)"
[ "$new_count" -eq 3 ] || fail "expected exactly 3 new descriptor files from this run's up.sh, found ${new_count}; inspect ${SHARED_LIVE_DIR}"

log "== copying this run's 3 fresh descriptor/ticket files into the throwaway shared dir =="
while IFS= read -r f; do
    [ -n "$f" ] || continue
    base="$(basename "$f" .descriptor.json)"
    NEW_DIDS+=("$base")
    cp "${SHARED_LIVE_DIR}/${base}.descriptor.json" "${SHARED_DIR}/${base}.descriptor.json"
    cp "${SHARED_LIVE_DIR}/${base}.ticket" "${SHARED_DIR}/${base}.ticket"
    log "  ${base}"
done <<<"$new_descriptor_paths"

# --- run the offline ceremony (provision) ---------------------------------
#
# ELASTOS_DATA_DIR is deliberately set to a value that will never equal
# scripts/setup-source-home.sh's own $HOME-derived default -- see the
# header comment's "install/restart step" section for exactly why this is
# an honest, not a fabricated, skip.
log ""
log "== phase: provision =="
HOME="$THROWAWAY_HOME" XDG_DATA_HOME="${THROWAWAY_HOME}/.local/share" \
    ELASTOS_DATA_DIR="${CLIENT_DATA_DIR}.not-a-source-home-target" \
    DOCKER_CONFIG="$REAL_DOCKER_CONFIG" \
    "${ROOT}/scripts/protected-content-installed-e2e-proof.sh" \
    --phase provision \
    --client-data-dir "$CLIENT_DATA_DIR" \
    --policy-authority-key "$POLICY_AUTHORITY_KEY" \
    --shared-dir "$SHARED_DIR" \
    --compose-file "$COMPOSE_FILE" \
    --allow-dirty

RECEIPT_PATH="${CLIENT_DATA_DIR}/receipts/protected-content-installed-e2e-proof.json"
[ -f "$RECEIPT_PATH" ] || fail "expected evidence receipt at ${RECEIPT_PATH} after provision"

provision_ok="$(python3 -c '
import json, sys
with open(sys.argv[1]) as fh:
    data = json.load(fh)
print("true" if data.get("provision", {}).get("ok") is True else "false")
' "$RECEIPT_PATH")"
[ "$provision_ok" = "true" ] || fail "provision.ok is not true in ${RECEIPT_PATH}; diagnostic: python3 -c \"import json; print(json.load(open('${RECEIPT_PATH}'))['provision'])\""
log "provision.ok=true"

# --- run the Carrier dial-proof preflight ---------------------------------

log ""
log "== phase: preflight =="
HOME="$THROWAWAY_HOME" XDG_DATA_HOME="${THROWAWAY_HOME}/.local/share" \
    DOCKER_CONFIG="$REAL_DOCKER_CONFIG" \
    "${ROOT}/scripts/protected-content-installed-e2e-proof.sh" \
    --phase preflight \
    --client-data-dir "$CLIENT_DATA_DIR" \
    --policy-authority-key "$POLICY_AUTHORITY_KEY" \
    --shared-dir "$SHARED_DIR" \
    --compose-file "$COMPOSE_FILE" \
    --allow-dirty

preflight_ok="$(python3 -c '
import json, sys
with open(sys.argv[1]) as fh:
    data = json.load(fh)
print("true" if data.get("preflight", {}).get("preflight_ok") is True else "false")
' "$RECEIPT_PATH")"
[ "$preflight_ok" = "true" ] || fail "preflight.preflight_ok is not true in ${RECEIPT_PATH}; diagnostic: python3 -c \"import json; print(json.load(open('${RECEIPT_PATH}'))['preflight'])\""
log "preflight.preflight_ok=true"

log ""
log "NOTE: installed_artifact_parity:false and static_ok:false below are EXPECTED"
log "  in this receipt -- this smoke deliberately skips the full source-home"
log "  install (see this script's header comment, 'install/restart step is"
log "  (honestly) skipped'); they are not a sign this run failed."
log "== evidence receipt (verbatim): ${RECEIPT_PATH} =="
cat "$RECEIPT_PATH" >&2

OVERALL_OK=true
log ""
log "custody-harness-ci-smoke: provision.ok=true, preflight.preflight_ok=true"
