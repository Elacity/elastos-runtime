# syntax=docker/dockerfile:1

# Minimal custody-host image: a slim dKMS shard-holder container.
#
# The dKMS custody node only verifies signed release operations/rights
# evidence and returns recipient-sealed contributions; CEK reconstruction
# happens on the playback runtime, never here. That keeps this image's own
# build needs to exactly 4 binaries: the `elastos` runtime plus the 3 native
# providers it hosts standalone (`elastos run custody-provider --with
# availability-provider --with ipfs-provider`) -- the latter two so this
# container also serves as a content-replica peer. The replica plane
# (ipfs-provider's Kubo-backed add/pin path) hard-depends on a `kubo` binary
# it does not build itself (capsules/ipfs-provider/src/main.rs:372
# ensure_kubo(); find_kubo_binary at :1296 has no in-process fallback), so a
# 5th binary -- kubo, fetched pre-built rather than compiled -- ships too;
# see the builder stage's "Kubo" section below.
#
# Deliberately NOT built via scripts/setup-source-home.sh: that installs
# ~30 unrelated binaries and 33 app capsules for the full-home topology.
# Direct `cargo build` of just the 4 workspaces this role needs instead.
FROM rust:1.91-bookworm AS builder

ARG APP_UID=10001
ARG APP_GID=10001
# Buildx-populated automatically (linux/amd64 -> "amd64", linux/arm64 ->
# "arm64"); must be re-declared per stage to be usable in that stage's
# instructions. Drives both the components.json platform key below and the
# libgcc triplet lookup, so this image builds correctly on CI's amd64
# runners as well as an arm64 dev machine -- neither is hardcoded.
ARG TARGETARCH

# jq is the one build-time tool this stage genuinely needs: it patches
# components.json's checksum/size fields with the real sha256 of the 3
# provider binaries this stage just built (the same field
# scripts/setup-source-home.sh's stamp_source_home_components_manifest step
# computes for a full install, scoped here to only what this stage builds),
# so scripts/installed-provider-verify.sh's --require-verified check has
# genuine, non-fabricated checksums to compare against instead of the
# unreleased "" placeholders components.json ships with. It never reaches
# the runtime stage. curl fetches the pinned kubo release tarball below
# (also build-time only; the runtime stage gets just the extracted binary).
RUN apt-get update -qq \
    && apt-get install -y -qq --no-install-recommends jq curl \
    && rm -rf /var/lib/apt/lists/*

RUN groupadd -g "${APP_GID}" custody \
    && useradd -m -u "${APP_UID}" -g "${APP_GID}" custody

ENV CARGO_TERM_COLOR=never

WORKDIR /src
COPY --chown=custody:custody . /src
# WORKDIR creates /src as root before COPY runs; --chown only covers the
# copied contents, not the pre-existing /src directory entry itself.
RUN chown custody:custody /src

USER custody

# Four direct builds, no wasm/musl targets. Path deps between capsules/* and
# elastos/crates/* resolve from the full checkout COPYed above (minus
# Dockerfile.dockerignore's build-artifact exclusions); intermediate build
# state lands in the repo's shared target-build/ per .cargo/config.toml,
# final binaries in each workspace's own target/release/.
#
# Dependency-cache layering: this repo's path-dependency graph runs each
# capsule straight into elastos/crates/* (not a self-contained crate a
# manifests-only COPY could prime in isolation), so the classic "COPY
# Cargo.{toml,lock} + dummy src, build, then COPY real source" trick doesn't
# apply cleanly here. A BuildKit cache mount gets the same result more
# directly: crates.io downloads and target-build/'s compiled dependency
# artifacts persist across builds keyed on this exact mount, independent of
# the source COPY layer above -- so touching any file elsewhere in the repo
# no longer forces every dependency to redownload and recompile from zero.
RUN --mount=type=cache,target=/usr/local/cargo/registry,uid=10001,gid=10001 \
    --mount=type=cache,target=/src/target-build,uid=10001,gid=10001 \
    cd elastos && cargo build --release -p elastos-server
RUN --mount=type=cache,target=/usr/local/cargo/registry,uid=10001,gid=10001 \
    --mount=type=cache,target=/src/target-build,uid=10001,gid=10001 \
    cargo build --release --manifest-path capsules/custody-provider/Cargo.toml
RUN --mount=type=cache,target=/usr/local/cargo/registry,uid=10001,gid=10001 \
    --mount=type=cache,target=/src/target-build,uid=10001,gid=10001 \
    cargo build --release --manifest-path capsules/availability-provider/Cargo.toml
RUN --mount=type=cache,target=/usr/local/cargo/registry,uid=10001,gid=10001 \
    --mount=type=cache,target=/src/target-build,uid=10001,gid=10001 \
    cargo build --release --manifest-path capsules/ipfs-provider/Cargo.toml
# chain-provider: a custody committee member settles every release through
# its own chain rights evidence (`protected_content_rights_evidence`), so the
# chain plane is part of this role, not an optional extra.
RUN --mount=type=cache,target=/usr/local/cargo/registry,uid=10001,gid=10001 \
    --mount=type=cache,target=/src/target-build,uid=10001,gid=10001 \
    cargo build --release --manifest-path capsules/chain-provider/Cargo.toml

# ── Kubo ─────────────────────────────────────────────────────────────
# ipfs-provider's replica plane hard-errors without a kubo binary
# (ensure_kubo() at capsules/ipfs-provider/src/main.rs:372); this repo has
# no Cargo path to it (kubo is a separate Go project), so the pinned
# official release tarball is fetched here instead, over TLS, and verified
# against a checksum hardcoded per architecture below (computed locally
# from a freshly downloaded copy of each tarball and cross-checked against
# the corresponding *.sha512 file dist.ipfs.tech publishes for this exact
# release) before anything is extracted -- fail closed on any mismatch,
# same posture as every other pinned artifact in this Dockerfile. The
# per-arch case statement mirrors the libgcc triplet lookup further below.
ARG KUBO_VERSION=v0.42.0
RUN <<'EOF'
set -eu
case "${TARGETARCH}" in
    amd64) kubo_sha256="284145534168b51fe980f73c90f0ce84b55ca293034836b2ba8ea8f93435116e" ;;
    arm64) kubo_sha256="edc6f485ab623f9327bf2ad7aa7a29d84c87c72f1e2584376a77240134a96e69" ;;
    *)
        echo "FAIL: unsupported TARGETARCH=${TARGETARCH} for kubo fetch (expected amd64 or arm64)" >&2
        exit 1
        ;;
esac
kubo_tarball="kubo_${KUBO_VERSION}_linux-${TARGETARCH}.tar.gz"
kubo_url="https://dist.ipfs.tech/kubo/${KUBO_VERSION}/${kubo_tarball}"
curl -fsSL -o "/tmp/${kubo_tarball}" "${kubo_url}"
echo "${kubo_sha256}  /tmp/${kubo_tarball}" | sha256sum -c -
tar -xzf "/tmp/${kubo_tarball}" -C /tmp kubo/ipfs
mv /tmp/kubo/ipfs /tmp/kubo-binary
# Same "strip after checksum-relevant steps are done" posture as the 4
# cargo-built binaries below: the upstream release ships full debug info
# (unstripped ~83MB on arm64, ~121MB on amd64); strip runs after the
# checksum verify above so the verified bytes are exactly what was
# downloaded, not a derivative the checksum never covered.
strip --strip-all /tmp/kubo-binary
chmod 0755 /tmp/kubo-binary
rm -rf "/tmp/${kubo_tarball}" /tmp/kubo
EOF

# Assemble the minimal artifact set the runtime stage will COPY wholesale:
# only the 4 binaries this role needs plus components.json, nothing else
# from the ~26-workspace source tree. `cp` always makes an independent inode
# (never a hardlink); the stat check after each copy is a defensive proof of
# that single-link property, not a workaround for cp itself.
#
# Stripped here, not at the cargo-output path: release builds carry full
# debug/symbol tables (elastos alone was ~95MB), and this is the one place
# both the checksum patch below and the host-side source-parity extraction
# read from, so there is exactly one stripped variant of each binary
# anywhere -- never a stripped-installed vs. unstripped-source mismatch for
# the audit's artifact_parity check to catch. NOTE ON WHAT THAT CHECK PROVES:
# the "source" side the audit compares against is this same builder stage's
# own output, extracted and placed at the repo's conventional
# elastos/target/release/* paths on the host running the audit -- not an
# independently rebuilt or third-party-attested copy. Byte-identical
# checksums here demonstrate internal consistency (the shipped binary is
# exactly the one this Dockerfile's cargo invocation produced), not external
# provenance of that build itself; see the task report's "self-referential
# parity" section for the full framing.
RUN <<'EOF'
set -eu
data_root="/home/custody/.local/share/elastos"
mkdir -p "${data_root}/bin"
for pair in \
    "elastos/target/release/elastos:elastos" \
    "capsules/custody-provider/target/release/custody-provider:custody-provider" \
    "capsules/availability-provider/target/release/availability-provider:availability-provider" \
    "capsules/ipfs-provider/target/release/ipfs-provider:ipfs-provider" \
    "capsules/chain-provider/target/release/chain-provider:chain-provider" \
; do
    src="${pair%%:*}"; name="${pair##*:}"
    cp "/src/${src}" "${data_root}/bin/${name}"
    nlink="$(stat -c %h "${data_root}/bin/${name}")"
    if [ "${nlink}" != "1" ]; then
        echo "FAIL: ${data_root}/bin/${name} has nlink=${nlink}, not a single-link copy: run 'stat ${data_root}/bin/${name}' to inspect" >&2
        exit 1
    fi
    strip --strip-all "${data_root}/bin/${name}"
    chmod 0700 "${data_root}/bin/${name}"
done
chmod 0700 "${data_root}/bin"

# libgcc_s.so.1 and libdl.so.2 live under an arch-triplet directory on
# Debian (bookworm's rust base image); stage both at fixed, arch-independent
# paths so the runtime stage's COPY --from doesn't need its own arch branch.
# libdl.so.2 is here for kubo specifically: empirically (verified live
# against both official v0.42.0 release tarballs, see task report) the
# amd64 build is dynamically linked and NEEDs libresolv.so.2, libpthread.so.0,
# libdl.so.2 and libc.so.6 -- busybox:glibc ships the first two and libc
# already (confirmed by `ls /lib` on both arch variants of the base image)
# but not libdl.so.2, so that one lands here explicitly, exactly like
# libgcc_s.so.1 above it. The arm64 build has no dynamic section at all
# (fully static, CGO_ENABLED=0 cross-build) and needs neither -- copying
# libdl.so.2 for arm64 too is harmless (unused) and keeps this one
# arch-branch the single source of both paths.
case "${TARGETARCH}" in
    amd64) libgcc_triplet="x86_64-linux-gnu" ;;
    arm64) libgcc_triplet="aarch64-linux-gnu" ;;
    *)
        echo "FAIL: unsupported TARGETARCH=${TARGETARCH} (expected amd64 or arm64)" >&2
        exit 1
        ;;
esac
cp "/usr/lib/${libgcc_triplet}/libgcc_s.so.1" /tmp/libgcc_s.so.1
cp "/usr/lib/${libgcc_triplet}/libdl.so.2" /tmp/libdl.so.2

jq \
    --arg platform "linux-${TARGETARCH}" \
    --arg cs_custody "sha256:$(sha256sum "${data_root}/bin/custody-provider" | cut -d' ' -f1)" \
    --arg cs_availability "sha256:$(sha256sum "${data_root}/bin/availability-provider" | cut -d' ' -f1)" \
    --arg cs_ipfs "sha256:$(sha256sum "${data_root}/bin/ipfs-provider" | cut -d' ' -f1)" \
    --arg cs_chain "sha256:$(sha256sum "${data_root}/bin/chain-provider" | cut -d' ' -f1)" \
    --argjson sz_custody "$(stat -c %s "${data_root}/bin/custody-provider")" \
    --argjson sz_availability "$(stat -c %s "${data_root}/bin/availability-provider")" \
    --argjson sz_ipfs "$(stat -c %s "${data_root}/bin/ipfs-provider")" \
    --argjson sz_chain "$(stat -c %s "${data_root}/bin/chain-provider")" \
    '
      .external["custody-provider"].platforms[$platform].checksum = $cs_custody
      | .external["custody-provider"].platforms[$platform].size = $sz_custody
      | .external["availability-provider"].platforms[$platform].checksum = $cs_availability
      | .external["availability-provider"].platforms[$platform].size = $sz_availability
      | .external["ipfs-provider"].platforms[$platform].checksum = $cs_ipfs
      | .external["ipfs-provider"].platforms[$platform].size = $sz_ipfs
      | .external["chain-provider"].platforms[$platform].checksum = $cs_chain
      | .external["chain-provider"].platforms[$platform].size = $sz_chain
      | .profiles["custody-host"] = {
          "description": "Components installed on a slim standalone custody-host container (shard-holder plus the chain rights-evidence plane it settles releases against; no media/decrypt planes)",
          "components": ["custody-provider", "availability-provider", "ipfs-provider", "chain-provider"]
        }
    ' /src/components.json > "${data_root}/components.json"
chmod 0600 "${data_root}/components.json"
chmod 0700 "${data_root}"
EOF

# Runtime stage: no Rust toolchain, no source tree, no target dirs, and no
# Debian userland at all -- busybox:glibc (~5MB) is the shell this stage
# needs (entrypoint.sh, rewritten strictly POSIX-sh: no [[ ]], no ${!name}
# indirection) plus the glibc our binaries are dynamically linked against,
# without debian:bookworm-slim's ~75MB of apt/dpkg/coreutils baggage.
# distroless/cc was considered and rejected: it ships no shell at all, and
# this stage's job is exactly to run entrypoint.sh.
FROM busybox:glibc AS runtime

ARG APP_UID=10001
ARG APP_GID=10001

# Empirically determined (see task report): `ldd` on all 4 stripped release
# binaries needs only libc.so.6, libm.so.6, libgcc_s.so.1 and the dynamic
# linker -- the rustls-only TLS stack in every one of these crates'
# Cargo.lock (no openssl/openssl-sys anywhere) needs no system TLS library.
# busybox:glibc already ships libc.so.6/libm.so.6/ld-linux-aarch64.so.1 (a
# newer glibc than the builder's; glibc's forward compatibility guarantee
# makes that safe) but not libgcc_s.so.1, so that one lands here explicitly.
# The CA bundle is for rustls-native-certs' outbound TLS at runtime; this
# stage has no apt to install ca-certificates with, so the single
# consolidated bundle file is copied straight from the builder instead of
# reinstalling the package here. libdl.so.2 is the 5th binary's (kubo's
# amd64 build) sole extra shared-library need beyond what's already here --
# see the builder stage's case statement for why.
COPY --from=builder /tmp/libgcc_s.so.1 /lib/libgcc_s.so.1
COPY --from=builder /tmp/libdl.so.2 /lib/libdl.so.2
COPY --from=builder /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt

RUN addgroup -g "${APP_GID}" custody \
    && adduser -D -u "${APP_UID}" -G custody -h /home/custody -s /bin/sh custody \
    && chown root:root /home \
    && mkdir -p /shared \
    && chown custody:custody /shared

ENV HOME=/home/custody \
    XDG_DATA_HOME=/home/custody/.local/share

# The image, not the volume, is the source of truth for these immutable
# artifacts: staged at /opt/elastos-image/, a plain image-layer path that is
# never declared VOLUME and so is always exactly what this image build
# produced, on every container start, regardless of what an older image
# version may have left in a long-lived named volume. entrypoint.sh compares
# this against the volume's copy on every boot and re-syncs bin/ +
# components.json (never state dirs) on any mismatch -- the fix for a
# straight `COPY ... /home/custody/.local/share/elastos` into the
# VOLUME-declared data root, which only ever populates an empty volume once
# and then never reaches it again across an image upgrade.
COPY --from=builder --chown=custody:custody /home/custody/.local/share/elastos/bin /opt/elastos-image/bin
COPY --from=builder --chown=custody:custody /home/custody/.local/share/elastos/components.json /opt/elastos-image/components.json
# The data root itself ships empty (just the directory, owner-only, no
# content) -- entrypoint.sh's sync step populates bin/+components.json into
# it on first boot exactly as it does on every later mismatch. Created here
# rather than left to Docker's implicit volume-mountpoint creation because
# busybox:glibc's adduser leaves /home/custody setgid (2755), and an
# implicitly-created mountpoint would inherit that (verified empirically);
# every directory the Runtime creates fresh under it at provisioning time
# would then inherit setgid too, failing the audit's exact-mode=0700 check
# on protected-content root. mkdir + explicit chmod 0700 here (not COPY of a
# pre-built tree, so there is no source mode for BuildKit to fail to
# re-assert) sidesteps that entirely.
RUN mkdir -p /home/custody/.local/share/elastos \
    && chown -R custody:custody /home/custody/.local/share \
    && chmod 0700 /home/custody/.local/share/elastos
COPY --chown=root:root --chmod=0755 deploy/custody-host/entrypoint.sh /entrypoint.sh

# kubo lands at a fixed, root-owned path outside the state volume entirely
# -- never under /opt/elastos-image or the data root's bin/ -- so it needs
# no entry in entrypoint.sh's image-vs-volume sync loop and survives an
# image upgrade the same way entrypoint.sh itself does. ELASTOS_IPFS_KUBO_PATH
# (find_kubo_binary's first, highest-priority lookup at
# capsules/ipfs-provider/src/main.rs:1298) is baked directly into the image
# rather than left to compose/docker-run -e, keeping the zero-required-env
# restart contract this image already makes intact.
COPY --from=builder --chown=root:root --chmod=0755 /tmp/kubo-binary /usr/local/bin/kubo
ENV ELASTOS_IPFS_KUBO_PATH=/usr/local/bin/kubo

USER custody
WORKDIR /home/custody
VOLUME ["/home/custody/.local/share/elastos", "/shared"]

ENTRYPOINT ["/entrypoint.sh"]
