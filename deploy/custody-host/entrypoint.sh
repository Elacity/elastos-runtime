#!/bin/sh
# Custody-host container entrypoint.
#
# Strict POSIX sh: the runtime stage is busybox:glibc, which provides only
# busybox ash as /bin/sh (no bash) -- no [[ ]], no ${!name} indirection, no
# `set -o pipefail`.
#
# One-shot-provisions this node's inactive custody state root and its public
# descriptor on first boot (guarded by a receipt file so restarts are a
# no-op), then execs the standalone provider host from Task 4
# (`elastos run custody-provider --with availability-provider --with
# ipfs-provider`). Only the descriptor -- a public artifact, and the sole
# thing that crosses into /shared -- carries the node's identity forward
# (its transport.peer_did field IS the DID; no separate .did file); all
# custody state stays under the private, owner-only data root.
#
# Exported filename is the node's own DID, not a caller-supplied index: this
# container's per-instance identity comes from CUSTODY_OPERATOR +
# CUSTODY_FAILURE_DOMAIN (product-meaningful, signed into the descriptor)
# plus its own generated device key, not from harness bookkeeping.
set -eu

data_root="${XDG_DATA_HOME:?}/elastos"
image_root="/opt/elastos-image"
elastos_bin="${data_root}/bin/elastos"
shared="/shared"
init_receipt="${data_root}/protected-content/custody-provider/container-init.json"

fail() {
    echo "FAIL: $*" >&2
    exit 1
}

# The image (${image_root}, never volume-shadowed) is the source of truth
# for bin/ + components.json; the volume only ever holds what an entrypoint
# run has synced into it. Runs on every boot, first included: a bare
# comparison naturally treats "nothing in the volume yet" as a mismatch, so
# this is also the only seeding step first boot needs -- there is no
# separate "first boot" code path for these artifacts, only for custody
# state further down. Never touches protected-content/, identity/, or run/.
sync_image_artifacts() {
    changed=0
    if [ ! -f "${data_root}/components.json" ] \
        || [ "$(sha256sum <"${image_root}/components.json")" != "$(sha256sum <"${data_root}/components.json")" ]; then
        changed=1
    fi
    if [ "${changed}" -eq 0 ]; then
        for name in elastos custody-provider availability-provider ipfs-provider chain-provider; do
            if [ ! -f "${data_root}/bin/${name}" ] \
                || [ "$(sha256sum <"${image_root}/bin/${name}")" != "$(sha256sum <"${data_root}/bin/${name}")" ]; then
                changed=1
                break
            fi
        done
    fi
    if [ "${changed}" -eq 0 ]; then
        return 0
    fi

    mkdir -p "${data_root}/bin"
    chmod 0700 "${data_root}/bin"
    for name in elastos custody-provider availability-provider ipfs-provider chain-provider; do
        cp "${image_root}/bin/${name}" "${data_root}/bin/${name}.tmp"
        chmod 0700 "${data_root}/bin/${name}.tmp"
        mv "${data_root}/bin/${name}.tmp" "${data_root}/bin/${name}"
    done
    cp "${image_root}/components.json" "${data_root}/components.json.tmp"
    chmod 0600 "${data_root}/components.json.tmp"
    mv "${data_root}/components.json.tmp" "${data_root}/components.json"
    echo "INFO: synced bin/ and components.json from the image into the data volume (new install or image upgrade)" >&2
}

sync_image_artifacts

if [ ! -x "${elastos_bin}" ]; then
    fail "installed runtime binary missing: ${elastos_bin} -- this image was not built from deploy/custody-host/Dockerfile; run: docker run --rm <image> ls -la \"${data_root}/bin\""
fi

if [ ! -d "${shared}" ]; then
    fail "${shared} is not mounted -- bind a host directory to ${shared} (e.g. -v \$(pwd)/shared:${shared}) so this node's descriptor can be published"
fi
# A bind mount keeps the HOST directory's owner and mode (the image's own
# chown of /shared is shadowed), and this container runs as the unprivileged
# custody user. Docker Desktop on macOS maps ownership so any container user
# can write; a Linux host does not, and the descriptor export below would
# fail on every boot with nothing in /shared to show for it. Probe once, up
# front, and name the remedy.
write_probe="${shared}/.write-probe.$$"
if ! ( : >"${write_probe}" ) 2>/dev/null; then
    fail "${shared} is not writable by uid $(id -u) (this container's custody user) -- on a Linux host a bind-mounted directory keeps the host owner/mode, so make the host directory world-writable before starting the node: chmod 1777 <host dir bound to ${shared}> (deploy/custody-host/up.sh up does this for its shared/)"
fi
rm -f "${write_probe}"

if [ ! -f "${init_receipt}" ]; then
    # CUSTODY_TRUSTED_RUNTIME_ISSUER / CUSTODY_OPERATOR / CUSTODY_FAILURE_DOMAIN
    # / ELASTOS_AVAILABILITY_ENSURE_URL are deployment-time configuration
    # (docker-compose.yaml, or -e on `docker run`) -- this image declares no
    # ENV/ARG default for any of them, and they are checked only here, at the
    # moment they are actually needed (first boot, before a state root
    # exists). ELASTOS_AVAILABILITY_ENSURE_URL is persisted into the receipt
    # below and re-exported from it on every later boot (see the `else`
    # branch), so a restart of an already-provisioned node needs none of the
    # four: zero required env vars.
    if [ -z "${CUSTODY_TRUSTED_RUNTIME_ISSUER:-}" ] || [ -z "${CUSTODY_OPERATOR:-}" ] \
        || [ -z "${CUSTODY_FAILURE_DOMAIN:-}" ] || [ -z "${ELASTOS_AVAILABILITY_ENSURE_URL:-}" ]; then
        fail "first boot needs CUSTODY_TRUSTED_RUNTIME_ISSUER, CUSTODY_OPERATOR, CUSTODY_FAILURE_DOMAIN, and ELASTOS_AVAILABILITY_ENSURE_URL to provision this node -- set them in docker-compose.yaml or pass -e to docker run"
    fi
    # This value is about to be hand-embedded in a plain JSON string field
    # below (see the printf comment); reject anything that field can't
    # safely hold rather than write corrupt JSON a later boot can't parse.
    case "${ELASTOS_AVAILABILITY_ENSURE_URL}" in
        *'"'* | *'\'*)
            fail "ELASTOS_AVAILABILITY_ENSURE_URL contains a double-quote or backslash, which this entrypoint's plain-JSON receipt writer cannot safely embed"
            ;;
    esac

    # `elastos identity show` has no --json output (verified against
    # elastos/crates/elastos-server/src/identity_cmd.rs); parse the plain
    # "DID:       <did>" line instead. The underlying identity load creates
    # the device key/DID on first call, so this is never empty once the
    # binary runs successfully.
    did="$("${elastos_bin}" identity show | sed -n 's/^DID:[[:space:]]*//p')"
    if [ -z "${did}" ] || [ "${did}" = "(not initialized yet)" ]; then
        fail "could not read this host's DID from 'elastos identity show'; run: docker exec <container> ${elastos_bin} identity show"
    fi
    # did:key:<base58> is always filename-safe, but this DID names a file
    # under /shared, so fail loudly rather than write somewhere unexpected
    # if that ever stops being true.
    case "${did}" in
        *[!A-Za-z0-9:]*)
            fail "DID '${did}' contains characters unsafe for a filename; run: ${elastos_bin} identity show"
            ;;
    esac

    "${elastos_bin}" protected-content-config provision-custody-node \
        --data-dir "${data_root}" \
        --trusted-runtime-issuer "${CUSTODY_TRUSTED_RUNTIME_ISSUER}" \
        --operator "${CUSTODY_OPERATOR}" \
        --failure-domain "${CUSTODY_FAILURE_DOMAIN}" \
        --transport-peer-did "${did}" \
        --output "${shared}/${did}.descriptor.json" \
        || fail "provision-custody-node failed; run: docker logs <container>"
    # The descriptor stays owner-only (0600, uid of this container's custody
    # user): the composition ceremony refuses any descriptor that is not, so
    # it is never relaxed here. A Linux host cannot read that file through
    # the bind mount; up.sh pulls its own owner-only copy out of the
    # container instead (docker compose exec cat) -- see "The /shared handoff".

    # A plain JSON object hand-built with printf rather than piped through
    # jq: this is the only JSON this entrypoint ever writes, every value in
    # it is already checked above for characters this format can't safely
    # embed, and adding a jq dependency to the runtime image just for these
    # few lines is not worth it. availability_ensure_url is persisted here
    # so a restart never needs ELASTOS_AVAILABILITY_ENSURE_URL supplied
    # again (see the `else` branch below).
    printf '{"did":"%s","provisioned_at":"%s","availability_ensure_url":"%s"}\n' \
        "${did}" "$(date -u +%FT%TZ)" "${ELASTOS_AVAILABILITY_ENSURE_URL}" >"${init_receipt}.tmp"
    chmod 0600 "${init_receipt}.tmp"
    mv "${init_receipt}.tmp" "${init_receipt}"
else
    # Already provisioned: restore the one env var the final exec still
    # needs from where first boot persisted it, so a restart requires
    # setting nothing at all (CUSTODY_* are never needed again either --
    # provision-custody-node above only runs once, in the branch above).
    ELASTOS_AVAILABILITY_ENSURE_URL="$(sed -n 's/.*"availability_ensure_url":"\([^"]*\)".*/\1/p' "${init_receipt}")"
    if [ -z "${ELASTOS_AVAILABILITY_ENSURE_URL}" ]; then
        fail "${init_receipt} has no availability_ensure_url field; run: docker exec <container> cat ${init_receipt}"
    fi
    export ELASTOS_AVAILABILITY_ENSURE_URL
fi

# A custody committee member settles every release through its own chain
# rights evidence (`protected_content_rights_evidence` on the chain plane),
# so the node needs the client's protected-content network configuration
# with evidence RPC URLs reachable FROM THIS CONTAINER. The deployment drops
# it at /shared/chain-provider.json (up.sh derives it from the client's
# config for the docker simulation); it is synced into the private data
# root on every boot, image-style, so an updated file takes effect on the
# next restart. Missing file: fail closed here with the remedy rather than
# let `elastos run` refuse the chain plane with a less specific message.
shared_chain_config="${shared}/chain-provider.json"
chain_config="${data_root}/protected-content/chain-provider.json"
if [ ! -f "${shared_chain_config}" ]; then
    fail "${shared_chain_config} is missing -- a custody node needs the protected-content chain configuration (network + evidence RPC URLs reachable from this container) to settle releases; for the docker simulation run: deploy/custody-host/up.sh sync-chain-config [CLIENT_DATA_DIR]"
fi
# Same bind-mount rule as the /shared write probe above, in the other
# direction: the host wrote this file as ITS user, and on a Linux host an
# owner-only mode leaves this container's custody user unable to read it
# (Docker Desktop maps that away). Name the remedy instead of dying on
# cp's one-line "Permission denied".
if [ ! -r "${shared_chain_config}" ]; then
    fail "${shared_chain_config} is not readable by uid $(id -u) (this container's custody user) -- the host must make it group/world-readable (chmod 0644; deploy/custody-host/up.sh sync-chain-config does this)"
fi
if [ ! -f "${chain_config}" ] \
    || [ "$(sha256sum <"${shared_chain_config}")" != "$(sha256sum <"${chain_config}")" ]; then
    mkdir -p "${data_root}/protected-content"
    chmod 0700 "${data_root}/protected-content"
    cp "${shared_chain_config}" "${chain_config}.tmp"
    chmod 0600 "${chain_config}.tmp"
    mv "${chain_config}.tmp" "${chain_config}"
    echo "INFO: synced protected-content/chain-provider.json from ${shared_chain_config}" >&2
fi

# --carrier-addr falls back to an ephemeral bind if 0.0.0.0:4433 is taken;
# callers must check the readiness receipt's carrier_bound rather than
# assume the requested port. ELASTOS_AVAILABILITY_ENSURE_URL is in the
# environment either way by this point (just-validated first-boot value, or
# restored from the receipt above) -- `elastos run --with
# availability-provider` reads it directly, there is no CLI flag for it.
exec "${elastos_bin}" run custody-provider \
    --with availability-provider \
    --with ipfs-provider \
    --with chain-provider \
    --carrier-addr 0.0.0.0:4433
