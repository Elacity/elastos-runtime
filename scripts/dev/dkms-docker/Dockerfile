# syntax=docker/dockerfile:1
# dkms-docker — ONE image that plays a full dKMS quorum node for local simulation:
#   dkms-authority     the node daemon (owns a master seed; recovers + re-seals in-boundary)
#   dkms-carrier-node  the untrusted Carrier (iroh) byte relay fronting the daemon
#   dkms-keygen        operator-console identity tool (used once, to mint the caller identity)
#
# Build context is the REPO ROOT (see docker-compose.yml). Only the standalone crates and
# their path deps are copied in — never the whole workspace.

FROM rust:1-bookworm AS build
WORKDIR /src

COPY capsules/ddrm-envelope capsules/ddrm-envelope
COPY capsules/dkms-authority capsules/dkms-authority
COPY capsules/dkms-keygen capsules/dkms-keygen
COPY elastos/crates/elastos-common elastos/crates/elastos-common
# elastos-common embeds the bus contract at compile time (include_bytes! in manifest.rs)
COPY elastos/wit elastos/wit
COPY scripts/dev/dkms-carrier-node scripts/dev/dkms-carrier-node

# elastos-common inherits package fields from the elastos workspace root, which is
# deliberately NOT copied into this image. Pin the same values the workspace declares.
RUN sed -i \
      -e 's/^version\.workspace = true/version = "0.5.0"/' \
      -e 's/^edition\.workspace = true/edition = "2021"/' \
      -e 's/^license\.workspace = true/license = "MIT"/' \
      elastos/crates/elastos-common/Cargo.toml

# SECURE-BY-DEFAULT node build (NO dev-modes): a recover without a wallet-signed
# AccessGrant fails closed — exactly the posture of the live geo nodes.
RUN cargo build --release --manifest-path capsules/dkms-authority/Cargo.toml \
 && cargo build --release --manifest-path capsules/dkms-keygen/Cargo.toml \
 && cargo build --release --manifest-path scripts/dev/dkms-carrier-node/Cargo.toml

FROM debian:bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/*

COPY --from=build /src/capsules/dkms-authority/target/release/dkms-authority /usr/local/bin/
COPY --from=build /src/capsules/dkms-keygen/target/release/dkms-keygen /usr/local/bin/
COPY --from=build /src/scripts/dev/dkms-carrier-node/target/release/dkms-carrier-node /usr/local/bin/
COPY scripts/dev/dkms-docker/entrypoint.sh /usr/local/bin/entrypoint.sh
RUN chmod +x /usr/local/bin/entrypoint.sh

ENTRYPOINT ["/usr/local/bin/entrypoint.sh"]
