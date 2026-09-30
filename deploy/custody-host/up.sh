#!/usr/bin/env bash
# 3-node custody-host compose harness lifecycle (ELACITY-2298 Task 6).
#
# Subcommands:
#   up.sh up [CLIENT_DATA_DIR]   Bring up custody-a/b/c, verify 3 distinct
#                                DID-keyed descriptors landed in shared/,
#                                export a per-node Carrier connect ticket,
#                                and print the exact host-client commands to
#                                register them (`elastos node peer add`) and
#                                assemble the pool (`generate-custody-composition`).
#   up.sh sync-chain-config [CLIENT_DATA_DIR]
#                                Re-derive shared/chain-provider.json (the
#                                client's protected-content network config
#                                with host-loopback RPC URLs rewritten for
#                                the containers); `up` does this itself.
#                                Restart the nodes to apply a changed file.
#   up.sh down                   Stop the 3 services; named volumes (each
#                                node's provisioned custody state) persist,
#                                so a following `up` restarts already-provisioned
#                                nodes (entrypoint.sh's zero-env restart path).
#   up.sh destroy                `down -v`: also drops the 3 named volumes and
#                                shared/*, then prints the builder-cache
#                                cleanup command (not run automatically --
#                                that cache is shared with any other image
#                                built from this repo).
#
# CLIENT_DATA_DIR selects which host Runtime data dir this harness treats as
# the client whose CUSTODY_TRUSTED_RUNTIME_ISSUER the 3 nodes will trust (env
# var, or the `up` subcommand's first positional argument); default is the
# real macOS Runtime data dir. If that data dir has no device key yet,
# `show-runtime-issuer` fails closed by design (it never auto-provisions an
# identity a caller didn't ask for) -- this script then falls back to a
# throwaway data dir made just for this harness run, noted on stdout.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
cd "${SCRIPT_DIR}"

SERVICES=(custody-a custody-b custody-c)
declare -A HOST_PORT=([custody-a]=14431 [custody-b]=14432 [custody-c]=14433)
CONTAINER_DATA_ROOT=/home/custody/.local/share/elastos
CONTAINER_ELASTOS_BIN="${CONTAINER_DATA_ROOT}/bin/elastos"
POLL_BUDGET_SECS=120
POLL_INTERVAL_SECS=2

log() { printf '%s\n' "$*" >&2; }
fail() {
    printf 'FAIL: %s\n' "$*" >&2
    exit 1
}
compose() { docker compose -f "${SCRIPT_DIR}/docker-compose.yml" "$@"; }
logs_hint() { printf 'docker compose -f %s logs %s' "${SCRIPT_DIR}/docker-compose.yml" "$*"; }

# --- host elastos binary -----------------------------------------------

resolve_host_elastos_bin() {
    local bin="${REPO_ROOT}/elastos/target/release/elastos"
    # `elastos/target/release/` is a shared, repo-relative cargo output path
    # (per .cargo/config.toml's shared build-dir) -- this repo also has other
    # agents/processes building container images and extracting artifacts
    # from them into the same tree, and empirically (this task's own live
    # run) that has clobbered this path with a Linux ELF built for a
    # container. `-x` alone can't tell a same-arch cross-target binary from
    # a real host-native one, so actually execute it before trusting it.
    if [ ! -x "${bin}" ] || ! "${bin}" --version >/dev/null 2>&1; then
        log "host elastos binary missing or not runnable on this host at ${bin}; building (cargo build --release -p elastos-server)..."
        (cd "${REPO_ROOT}" && cargo build --release --manifest-path elastos/Cargo.toml -p elastos-server)
    fi
    printf '%s\n' "${bin}"
}

macos_default_client_data_dir() {
    printf '%s/Library/Application Support/elastos\n' "${HOME}"
}

# --- ticket construction --------------------------------------------------
#
# `elastos node info --json` (the brief's assumed ticket source) reads its
# connect ticket from an HTTP admin API coordinate file
# (operator_control::gather_local_node_status -> read_runtime_coords) that
# only `elastos serve` writes. The custody-host containers run standalone
# via `elastos run custody-provider --carrier-addr ...` (provider_host.rs),
# which never writes that coordinate file -- confirmed empirically below in
# `up` (the command runs and is logged, not skipped) and by source
# inspection (no `write_runtime_coords` call anywhere in run_cmd.rs /
# provider_host.rs). So `node info --json` genuinely returns no ticket for
# these nodes; there is no self-reported ticket to rewrite.
#
# Instead this constructs a ticket by hand, byte-compatible with
# carrier.rs::decode_ticket_endpoints: base32(nopad, lowercased) of
# `{"topic":null,"endpoints":[{"id":"<hex ed25519 pubkey>","addrs":[{"Ip":"<host>:<port>"}]}]}`.
# The `id` is the same Ed25519 public key the DID already encodes (did:key
# multicodec 0xed01 + 32-byte key, base58btc) -- iroh's EndpointId, per
# iroh-base's `PublicKey` Display impl, serializes as lowercase hex; DID and
# Carrier endpoint identity are the same device key (entrypoint.sh's
# provision step signs the descriptor's transport.peer_did with this exact
# DID). This is docker-simulation glue: it hands the client the published
# 127.0.0.1:1443N mapping directly, standing in for whatever real
# address-discovery a non-simulated deployment would use.
build_ticket() {
    local did="$1" port="$2"
    python3 - "${did}" "${port}" <<'PY'
import base64
import json
import sys

ALPHABET = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"


def b58decode(s: str) -> bytes:
    num = 0
    for ch in s:
        num = num * 58 + ALPHABET.index(ch)
    body = num.to_bytes((num.bit_length() + 7) // 8, "big") if num else b""
    n_pad = len(s) - len(s.lstrip("1"))
    return b"\x00" * n_pad + body


did, port = sys.argv[1], sys.argv[2]
prefix = "did:key:z"
if not did.startswith(prefix):
    raise SystemExit(f"not a did:key: {did}")
raw = b58decode(did[len(prefix):])
if len(raw) != 34 or raw[:2] != b"\xed\x01":
    raise SystemExit(f"not a canonical Ed25519 did:key: {did}")
pubkey_hex = raw[2:].hex()

ticket_obj = {
    "topic": None,
    "endpoints": [{"id": pubkey_hex, "addrs": [{"Ip": f"127.0.0.1:{port}"}]}],
}
payload = json.dumps(ticket_obj, separators=(",", ":")).encode()
ticket = base64.b32encode(payload).decode().rstrip("=").lower()
print(ticket)
PY
}

json_field() {
    # json_field <json-on-stdin> <dotted.field.path>
    python3 -c '
import json, sys
value = json.load(sys.stdin)
for part in sys.argv[1].split("."):
    value = value[part]
print(value)
' "$1"
}

# --- subcommands ------------------------------------------------------

cmd_up() {
    local client_data_dir="${1:-${CLIENT_DATA_DIR:-$(macos_default_client_data_dir)}}"
    local elastos_bin
    elastos_bin="$(resolve_host_elastos_bin)"

    log "== host client data dir: ${client_data_dir} =="
    log "== elastos protected-content-config show-runtime-issuer --data-dir '${client_data_dir}' =="
    local issuer_json=""
    if ! issuer_json="$("${elastos_bin}" protected-content-config show-runtime-issuer --data-dir "${client_data_dir}" 2>&1)"; then
        log "show-runtime-issuer failed against '${client_data_dir}' (${issuer_json});"
        log "falling back to a throwaway data dir for this harness run only"
        local throwaway_home
        throwaway_home="$(mktemp -d)"
        client_data_dir="${throwaway_home}/Library/Application Support/elastos"
        log "throwaway HOME: ${throwaway_home}"
        log "throwaway client data dir: ${client_data_dir}"
        # `identity show`/`node *` always read HOME-derived default_data_dir()
        # (no --data-dir flag exists for them); this first call creates the
        # device key that show-runtime-issuer below requires to already exist.
        HOME="${throwaway_home}" "${elastos_bin}" identity show >/dev/null
        issuer_json="$("${elastos_bin}" protected-content-config show-runtime-issuer --data-dir "${client_data_dir}")"
        log "NOTE: subsequent host-client commands (node peer add, node info) act on"
        log "default_data_dir(), which is HOME-derived -- run them with HOME=${throwaway_home}"
        log "to keep targeting this same throwaway identity."
    fi
    local issuer
    issuer="$(printf '%s' "${issuer_json}" | json_field trusted_runtime_issuer)"
    [ -n "${issuer}" ] || fail "could not parse trusted_runtime_issuer from: ${issuer_json}"
    log "CUSTODY_TRUSTED_RUNTIME_ISSUER=${issuer}"

    printf 'CUSTODY_TRUSTED_RUNTIME_ISSUER=%s\n' "${issuer}" >.env
    mkdir -p shared
    # shared/ is bind-mounted into the containers, which run as an
    # unprivileged uid (10001) that does not exist on the host. A bind mount
    # keeps the host directory's owner and mode: Docker Desktop on macOS maps
    # ownership so the nodes can write regardless, a Linux host does not, and
    # the descriptor export then fails on every boot. World-writable with the
    # sticky bit (this host user owns the directory, so it can still remove
    # the nodes' files); the only things that ever land here are the nodes'
    # public descriptors, the host-written tickets and the chain config.
    chmod 1777 shared || fail "could not make shared/ writable for the containers' custody user (chmod 1777 shared)"
    sync_chain_config "${client_data_dir}" yes

    log "== docker compose up -d --build =="
    compose up -d --build

    log "== polling up to ${POLL_BUDGET_SECS}s for 3 distinct DID-keyed descriptors in shared/ =="
    local waited=0 count=0
    while :; do
        count="$(find shared -maxdepth 1 -name '*.descriptor.json' 2>/dev/null | wc -l | tr -d ' ')"
        [ "${count}" -ge 3 ] && break
        if [ "${waited}" -ge "${POLL_BUDGET_SECS}" ]; then
            # The nodes' own words are the only evidence of WHY the export
            # never landed (a first-boot failure restarts the container, so
            # the state alone says little); print them before failing so a
            # CI log carries the cause and not just the symptom.
            log "-- container state and last log lines (descriptor export never landed) --"
            compose ps -a >&2 || true
            compose logs --no-color --tail=40 "${SERVICES[@]}" >&2 || true
            fail "only ${count}/3 descriptor files after ${POLL_BUDGET_SECS}s; inspect with: $(logs_hint "${SERVICES[*]}")"
        fi
        sleep "${POLL_INTERVAL_SECS}"
        waited=$((waited + POLL_INTERVAL_SECS))
    done
    local descriptor_dids=()
    local f base
    for f in shared/*.descriptor.json; do
        base="$(basename "${f}" .descriptor.json)"
        descriptor_dids+=("${base}")
    done
    local uniq_count
    uniq_count="$(printf '%s\n' "${descriptor_dids[@]}" | sort -u | wc -l | tr -d ' ')"
    [ "${uniq_count}" -eq 3 ] || fail "descriptor DIDs are not distinct (${descriptor_dids[*]}); inspect with: $(logs_hint "${SERVICES[*]}")"
    log "3 distinct descriptors: ${descriptor_dids[*]}"

    log "== per-node ready receipt, ticket export, and node peer add commands =="
    local svc did carrier_bound ready_json state ticket node_info_json
    local peer_add_lines=()
    local descriptor_paths=()
    for svc in "${SERVICES[@]}"; do
        state="$(compose ps --format '{{.State}}' "${svc}" 2>/dev/null || true)"
        [ "${state}" = "running" ] || fail "${svc} is not running (state='${state}'); inspect with: $(logs_hint "${svc}")"

        # The container reaching "running" only means entrypoint.sh's exec'd
        # process started; provisioning + all 3 providers registering +
        # Carrier coming online still take a moment after that, so the
        # readiness receipt is polled for too, on the same overall budget.
        ready_json=""
        waited=0
        while :; do
            if ready_json="$(compose exec -T "${svc}" cat "${CONTAINER_DATA_ROOT}/run/provider-host.ready.json" 2>&1)"; then
                break
            fi
            if [ "${waited}" -ge "${POLL_BUDGET_SECS}" ]; then
                fail "${svc} has no readiness receipt after ${POLL_BUDGET_SECS}s (${ready_json}); inspect with: $(logs_hint "${svc}")"
            fi
            sleep "${POLL_INTERVAL_SECS}"
            waited=$((waited + POLL_INTERVAL_SECS))
        done
        did="$(printf '%s' "${ready_json}" | json_field did)"
        carrier_bound="$(printf '%s' "${ready_json}" | json_field carrier_bound)"
        case "${carrier_bound}" in
        *:4433) ;;
        *) fail "${svc} ready receipt carrier_bound='${carrier_bound}' does not end :4433 (stale receipt from a dead process?); inspect with: $(logs_hint "${svc}")" ;;
        esac
        [ -f "shared/${did}.descriptor.json" ] || fail "${svc}'s ready DID ${did} has no matching shared/${did}.descriptor.json; inspect with: $(logs_hint "${svc}")"
        # The node wrote its descriptor owner-only as ITS uid (10001), and the
        # composition ceremony insists on owner-only input, so the host needs
        # its own owner-only copy: on a Linux host the bind-mounted file is
        # unreadable to this user (Docker Desktop maps ownership away, which
        # is why macOS never noticed). Stream it out through an exec as the
        # node's own user into a file this user creates owner-only (umask
        # 077 in the subshell; the temp name carries no colon because
        # `compose cp` would read the DID's colons as a service separator),
        # then replace the node's copy (this user owns shared/, so the sticky
        # bit permits it).
        (umask 077 && compose exec -T "${svc}" cat "/shared/${did}.descriptor.json" >"shared/.${svc}.descriptor.host") \
            || fail "could not read ${svc}'s descriptor out of the container; inspect with: $(logs_hint "${svc}")"
        [ -s "shared/.${svc}.descriptor.host" ] \
            || fail "${svc}'s descriptor read back empty from the container; inspect with: $(logs_hint "${svc}")"
        mv -f "shared/.${svc}.descriptor.host" "shared/${did}.descriptor.json" \
            || fail "could not install the host-owned copy of shared/${did}.descriptor.json"
        log "${svc}: did=${did} carrier_bound=${carrier_bound} state=${state} (live process confirmed via docker compose ps + a fresh exec; descriptor pulled as a host-owned owner-only copy)"

        # Empirical confirmation of the finding documented above build_ticket():
        # standalone `elastos run` never writes the runtime-coords file
        # `node info --json` reads, so this always reports no active local
        # runtime / no connect_ticket for these containers -- logged, not
        # treated as a failure.
        node_info_json="$(compose exec -T "${svc}" "${CONTAINER_ELASTOS_BIN}" node info --json 2>&1 || true)"
        log "${svc}: elastos node info --json => ${node_info_json}"

        ticket="$(build_ticket "${did}" "${HOST_PORT[${svc}]}")"
        printf '%s\n' "${ticket}" >"shared/${did}.ticket"
        log "${svc}: wrote shared/${did}.ticket (host route 127.0.0.1:${HOST_PORT[${svc}]}/udp, docker-simulation glue -- see build_ticket())"

        peer_add_lines+=("elastos node peer add --did ${did} --label ${svc} --ticket \"\$(cat '${SCRIPT_DIR}/shared/${did}.ticket')\"")
        descriptor_paths+=("${SCRIPT_DIR}/shared/${did}.descriptor.json")
    done

    log ""
    log "== next: register the 3 nodes and assemble the pool from the host client =="
    local line
    for line in "${peer_add_lines[@]}"; do
        log "${line}"
    done
    log ""
    log "elastos protected-content-config generate-custody-composition \\"
    log "  --authority-key <policy-authority-key-path> \\"
    for f in "${descriptor_paths[@]}"; do
        log "  --node '${f}' \\"
    done
    log "  --data-dir '${client_data_dir}'"
}

# sync_chain_config CLIENT_DATA_DIR -- derive shared/chain-provider.json from
# the client's protected-content/chain-provider.json. A custody committee
# member settles every release through its own chain rights evidence, so each
# node needs the same network config as the client, with evidence RPC URLs it
# can reach FROM ITS CONTAINER: host-loopback URLs (127.0.0.1 / localhost,
# e.g. a local Anvil fork and its distinct-origin alias) are rewritten to
# host.docker.internal (Docker Desktop's host gateway; docker-simulation
# glue exactly like build_ticket()). Every other URL passes through
# unchanged. entrypoint.sh syncs the file into each node's private data
# root on every boot, so re-running this and restarting the nodes applies a
# changed config. Fails closed when the client has no chain config yet
# (run the proof driver's chain-config-real phase first).
sync_chain_config() {
    local client_data_dir="$1" allow_placeholder="${2:-no}"
    local src="${client_data_dir}/protected-content/chain-provider.json"
    if [ ! -f "${src}" ]; then
        if [ "${allow_placeholder}" != "yes" ]; then
            fail "no protected-content chain configuration at '${src}'; provision the client's chain config first (protected-content-installed-e2e-proof.sh --phase chain-config-real), then re-run: $0 sync-chain-config '${client_data_dir}'"
        fi
        # `up` before the client has any chain config (the CI ceremony smoke
        # brings the nodes up first, then provisions the client): boot the
        # nodes on the same placeholder network the proof driver's
        # `provision` phase installs on the client, so the chain plane has a
        # valid config to register with. Placeholder nodes can dial nothing
        # real, so they settle no release -- re-run `sync-chain-config` +
        # restart once the client has its real config (chain-config-real).
        local placeholder_home
        placeholder_home="$(mktemp -d)"
        log "NOTE: '${src}' does not exist yet; deriving shared/chain-provider.json from the"
        log "      proof driver's PLACEHOLDER chain config (loopback RPCs, placeholder mint"
        log "      addresses). Nodes booted on it cannot settle releases: after the client"
        log "      has its real chain config, run: $0 sync-chain-config '${client_data_dir}'"
        log "      and restart the nodes."
        "${elastos_bin}" protected-content-config generate-chain-config \
            --data-dir "${placeholder_home}" \
            --rpc-url http://127.0.0.1:8545 \
            --evidence-rpc-url http://127.0.0.1:8545 \
            --evidence-rpc-url http://127.0.0.1:8546 \
            --mint-ledger 0x0000000000000000000000000000000000000022 \
            --mint-pay-token 0x0000000000000000000000000000000000000033 \
            --mint-asset-created-emitter 0x0000000000000000000000000000000000000044 \
            >/dev/null || fail "could not generate the placeholder chain config under '${placeholder_home}'"
        src="${placeholder_home}/protected-content/chain-provider.json"
    fi
    mkdir -p shared
    python3 - "${src}" shared/chain-provider.json <<'PY' || fail "could not derive shared/chain-provider.json from '${src}'"
import json
import re
import sys

src, dst = sys.argv[1], sys.argv[2]
with open(src) as handle:
    config = json.load(handle)

HOST_LOOPBACK = re.compile(r"^(https?://)(127\.0\.0\.1|localhost)(:|/|$)")
rewritten = []

def rewrite(url):
    new = HOST_LOOPBACK.sub(r"\1host.docker.internal\3", url)
    if new != url:
        rewritten.append(f"{url} -> {new}")
    return new

def walk(node):
    if isinstance(node, dict):
        return {key: walk(value) for key, value in node.items()}
    if isinstance(node, list):
        return [walk(value) for value in node]
    if isinstance(node, str) and node.startswith(("http://", "https://")):
        return rewrite(node)
    return node

derived = walk(config)
if rewritten:
    # The chain-provider admits plain http only against loopback unless the
    # host is allowlisted explicitly on the network; name the one host the
    # rewrite introduced, nothing wider.
    network = derived.get("protected_content_network")
    if not isinstance(network, dict):
        raise SystemExit("client chain config has no protected_content_network object")
    hosts = list(network.get("plain_http_rpc_hosts") or [])
    if "host.docker.internal" not in hosts:
        hosts.append("host.docker.internal")
    network["plain_http_rpc_hosts"] = hosts
with open(dst, "w") as handle:
    json.dump(derived, handle, indent=2, sort_keys=True)
    handle.write("\n")
for line in rewritten:
    print(f"rewrote {line}", file=sys.stderr)
PY
    # Read by the containers' unprivileged user through the bind mount: on a
    # Linux host an owner-only file is unreadable to it (Docker Desktop maps
    # that away, which is how 0600 survived here). The content is network
    # ids, contract addresses and RPC URLs -- nothing secret belongs in a
    # simulation harness's shared/ -- so world-readable is the right mode.
    chmod 0644 shared/chain-provider.json
    log "wrote shared/chain-provider.json (from ${src}; host-loopback RPC URLs rewritten to host.docker.internal for the containers; mode 0644 so the nodes can read it)"
}

cmd_sync_chain_config() {
    local client_data_dir="${1:-${CLIENT_DATA_DIR:-$(macos_default_client_data_dir)}}"
    sync_chain_config "${client_data_dir}"
    log "restart the nodes to apply it: docker compose -f ${SCRIPT_DIR}/docker-compose.yml restart"
}

cmd_down() {
    log "== docker compose down (named volumes kept; restart resumes provisioned nodes) =="
    compose down
}

cmd_destroy() {
    log "== docker compose down -v (named volumes + containers removed) =="
    compose down -v
    rm -f shared/*.descriptor.json shared/*.ticket shared/chain-provider.json
    log ""
    log "Builder cache and the elastos-custody-host image were left in place"
    log "(shared across any other image built from this repo). To reclaim that"
    log "space too, run:"
    log "  docker image rm elastos-custody-host:latest"
    log "  docker buildx prune --filter type=exec.cachemount"
}

main() {
    local sub="${1:-}"
    case "${sub}" in
    up)
        shift
        cmd_up "$@"
        ;;
    down)
        cmd_down
        ;;
    destroy)
        cmd_destroy
        ;;
    sync-chain-config)
        shift
        cmd_sync_chain_config "$@"
        ;;
    *)
        fail "usage: $(basename "$0") up [CLIENT_DATA_DIR] | sync-chain-config [CLIENT_DATA_DIR] | down | destroy"
        ;;
    esac
}

main "$@"
