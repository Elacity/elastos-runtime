# Scanner image for Anthropic's OSS Scanner (https://github.com/anthropics/oss-scanner).
#
# The scanner builds this file with the repository root as the build context and network access, then runs the
# audit offline. This build therefore installs the pinned toolchain, fetches every crate that the Runtime
# workspace, the capsule workspaces and the helper tools lock, and builds the Runtime workspace and every
# capsule workspace with its test binaries in the dev profile (debug assertions and overflow checks on).
# .oss-scanner/threat_model.md says what to attack and how to run it.

FROM rust:1.91-bookworm
ENV DEBIAN_FRONTEND=noninteractive
# aws-lc-sys and zstd-sys build native code. jq and curl help when poking the local gateway.
RUN apt-get update && apt-get install -y --no-install-recommends \
      clang cmake pkg-config python3 jq curl \
    && rm -rf /var/lib/apt/lists/*

# rust-toolchain.toml pins the exact toolchain. Install it, with the wasm target the capsules use, while the network
# is still available.
RUN rustup toolchain install 1.91.0 --profile minimal \
      --component rustfmt --component clippy --target wasm32-unknown-unknown

# scanner contract: the checkout lives inside the image, at /src
COPY . /src
WORKDIR /src

ENV RUST_BACKTRACE=1 \
    CARGO_TERM_COLOR=never

# Every crate in every lockfile, so later builds with other features, targets or tests stay offline.
RUN set -eu; \
    cargo fetch --locked --manifest-path elastos/Cargo.toml; \
    for lock in capsules/*/Cargo.lock elastos/tools/*/Cargo.lock; do \
      cargo fetch --locked --manifest-path "$(dirname "$lock")/Cargo.toml"; \
    done

# The Runtime workspace: the `elastos` binary, the localhost-provider and every test binary. Intermediate artifacts
# land in /src/target-build (.cargo/config.toml), final binaries in elastos/target/debug. A step that fails fails
# the build, and primary_contact gets the log.
RUN cd elastos && cargo build --locked --workspace --all-targets \
    && cargo test --locked --workspace --no-run

# Every capsule workspace with its test binaries. The three native providers below are also the process binaries
# that the Runtime integration tests start.
RUN set -eu; \
    for lock in capsules/*/Cargo.lock; do \
      capsule="$(dirname "$lock")"; \
      echo "== building $capsule =="; \
      (cd "$capsule" && cargo build --locked --all-targets && cargo test --locked --no-run); \
    done

ENV ELASTOS_TEST_PROTECT_PROVIDER_BIN=/src/capsules/protected-content-protect-provider/target/debug/protected-content-protect-provider \
    ELASTOS_TEST_DECRYPT_PROVIDER_BIN=/src/capsules/protected-content-decrypt-provider/target/debug/protected-content-decrypt-provider \
    ELASTOS_TEST_CUSTODY_PROVIDER_BIN=/src/capsules/custody-provider/target/debug/custody-provider

# Run the tests so the log shows the image works, without letting a failure (or a test that needs the Internet) abort
# the build. Tests that need the network are #[ignore]d.
RUN (cd elastos && timeout 2400 cargo test --locked --workspace --no-fail-fast) \
    || echo "WARNING: Runtime workspace tests reported failures"
RUN for lock in capsules/*/Cargo.lock; do \
      capsule="$(dirname "$lock")"; \
      (cd "$capsule" && timeout 900 cargo test --locked --no-fail-fast) \
        || echo "WARNING: $capsule tests reported failures"; \
    done

# Everything is fetched. From here on cargo must not try to reach a registry.
ENV CARGO_NET_OFFLINE=true
