# ElastOS — single entry point for build, test, and publish.
# Install: cargo install just

# Default: show recipes
default:
    @just --list

# Build runtime + core capsules
build:
    ./scripts/build.sh

# Build everything (runtime + all capsules)
build-all:
    ./scripts/build.sh --all

# Build runtime only
build-runtime:
    ./scripts/build.sh --runtime

# Build a specific capsule by name
build-capsule name:
    ./scripts/build.sh --capsule {{name}}

# List all buildable capsules
list-capsules:
    ./scripts/build.sh --list

# Fast check after editing (single crate)
check crate="elastos-server":
    cd elastos && cargo check -p {{crate}}

# --target-dir is pinned so the binary paths below stay correct even when the
# environment sets a global CARGO_TARGET_DIR.
# Build the provider process binaries the protected-content process tests spawn
prepare-providers:
    cd capsules/protected-content-protect-provider && cargo build --release --target-dir target
    cd capsules/protected-content-decrypt-provider && cargo build --release --target-dir target
    cd capsules/custody-provider && cargo build --release --target-dir target

# Run workspace tests (provider process tests need the capsule binaries)
test-elastos *args: prepare-providers
    cd elastos && \
      ELASTOS_TEST_PROTECT_PROVIDER_BIN="$(pwd)/../capsules/protected-content-protect-provider/target/release/protected-content-protect-provider" \
      ELASTOS_TEST_DECRYPT_PROVIDER_BIN="$(pwd)/../capsules/protected-content-decrypt-provider/target/release/protected-content-decrypt-provider" \
      ELASTOS_TEST_CUSTODY_PROVIDER_BIN="$(pwd)/../capsules/custody-provider/target/release/custody-provider" \
      cargo test --workspace {{args}}

# The elastos workspace suite (and CI's `cargo test --workspace`) never
# builds or tests own-workspace capsules; this covers them.
# Test every capsule that is its own cargo workspace
test-capsules:
    #!/usr/bin/env bash
    set -euo pipefail
    # Dependency artifacts are shared across all workspaces via the root
    # .cargo/config.toml build-dir (<repo>/target-build), so each dep compiles
    # once instead of ~25 times. Deliberately NO --target-dir pin: with a
    # shared build-dir, cargo reuses compiled test executables across
    # invocations without re-baking their CARGO_BIN_EXE_* paths, so every
    # invocation style (just, plain cargo test, rust-analyzer, CI) must agree
    # on each workspace's default target dir or process tests spawn stale
    # binary paths.
    for lock in capsules/*/Cargo.lock; do
        capsule="$(dirname "$lock")"
        echo "== testing $capsule =="
        (cd "$capsule" && cargo test)
    done

# Run the workspace suite and every own-workspace capsule suite
test: test-elastos test-capsules

# Accurate local replica of the CI test-elastos job: Linux container, cold
# caches, pristine copy of the working tree (tracked + modified files).
# nodejs matches the ubuntu-latest runner, where node is preinstalled and
# elastos-server integration tests spawn it.
ci-test-elastos:
    tar --no-xattrs --no-mac-metadata --no-fflags --exclude='.git' --exclude='target' --exclude='target-capsules' --exclude='target-build' --exclude='*/target' --exclude='capsules/*/target' -cf - . | \
      docker run --rm -i -e RUSTFLAGS="-D warnings" -e CARGO_TERM_COLOR=always rust:1.91-bookworm bash -c '\
        mkdir /w && tar -xf - -C /w && cd /w && cargo --version >/dev/null && \
        apt-get update -qq >/dev/null && apt-get install -y -qq nodejs >/dev/null && node --version && \
        useradd -m ci && chown -R ci:ci /w && \
        su -s /bin/bash ci -c "export PATH=/usr/local/cargo/bin:\$PATH RUSTUP_HOME=/usr/local/rustup CARGO_HOME=/home/ci/.cargo RUSTFLAGS=\"-D warnings\" && cd /w && \
        cargo build --release --manifest-path capsules/protected-content-protect-provider/Cargo.toml --target-dir capsules/protected-content-protect-provider/target && \
        cargo build --release --manifest-path capsules/protected-content-decrypt-provider/Cargo.toml --target-dir capsules/protected-content-decrypt-provider/target && \
        cargo build --release --manifest-path capsules/custody-provider/Cargo.toml --target-dir capsules/custody-provider/target && \
        cd elastos && \
        ELASTOS_TEST_PROTECT_PROVIDER_BIN=/w/capsules/protected-content-protect-provider/target/release/protected-content-protect-provider \
        ELASTOS_TEST_DECRYPT_PROVIDER_BIN=/w/capsules/protected-content-decrypt-provider/target/release/protected-content-decrypt-provider \
        ELASTOS_TEST_CUSTODY_PROVIDER_BIN=/w/capsules/custody-provider/target/release/custody-provider \
        cargo test --workspace"'

# Accurate local replica of the CI test-capsules job (same container recipe).
ci-test-capsules:
    tar --no-xattrs --no-mac-metadata --no-fflags --exclude='.git' --exclude='target' --exclude='target-capsules' --exclude='target-build' --exclude='*/target' --exclude='capsules/*/target' -cf - . | \
      docker run --rm -i -e RUSTFLAGS="-D warnings" -e CARGO_TERM_COLOR=always rust:1.91-bookworm bash -c '\
        mkdir /w && tar -xf - -C /w && cd /w && cargo --version >/dev/null && \
        useradd -m ci && chown -R ci:ci /w && \
        su -s /bin/bash ci -c "export PATH=/usr/local/cargo/bin:\$PATH RUSTUP_HOME=/usr/local/rustup CARGO_HOME=/home/ci/.cargo RUSTFLAGS=\"-D warnings\" && cd /w && \
        for lock in capsules/*/Cargo.lock; do \
            capsule=\$(dirname \$lock); \
            echo \"== testing \$capsule ==\"; \
            (cd \$capsule && cargo test) || exit 1; \
        done"'

# Accurate local replica of the CI source-home-linux job. arch selects the
# matrix leg: arm64 = ubuntu-24.04-arm (native on Apple silicon),
# amd64 = ubuntu-latest (emulated, much slower).
ci-source-home-linux arch='arm64':
    tar --no-xattrs --no-mac-metadata --no-fflags --exclude='.git' --exclude='target' --exclude='target-capsules' --exclude='target-build' --exclude='*/target' --exclude='capsules/*/target' -cf - . | \
      docker run --rm -i --platform linux/{{arch}} -e CARGO_TERM_COLOR=never rust:1.91-bookworm bash -c '\
        mkdir /w && tar -xf - -C /w && cd /w && \
        apt-get update -qq >/dev/null && apt-get install -y -qq coturn e2fsprogs ffmpeg nodejs git jq >/dev/null && \
        rustup target add wasm32-unknown-unknown "$(uname -m)-unknown-linux-musl" >/dev/null 2>&1 && \
        useradd -m ci && chown -R ci:ci /w && \
        su -s /bin/bash ci -c "set -euo pipefail && \
        export PATH=/usr/local/cargo/bin:\$PATH RUSTUP_HOME=/usr/local/rustup CARGO_HOME=/home/ci/.cargo && \
        cd /w && git init -q && git add -A && git -c user.email=ci@local -c user.name=ci commit -qm ci-replica && \
        export ELASTOS_COLLABORATION_STARTUP_MODE=isolated && \
        SOURCE_HOME=/home/ci/rtemp/elastos-source-home && mkdir -p \$SOURCE_HOME && \
        export HOME=\$SOURCE_HOME XDG_DATA_HOME=\$SOURCE_HOME/.local/share && \
        scripts/setup-source-home.sh && \
        ELASTOS_DATA_DIR=\$XDG_DATA_HOME/elastos scripts/installed-provider-verify.sh && \
        scripts/local-carrier-setup-smoke.sh"'

# Test a single crate (fastest iteration)
test-crate crate *args:
    cd elastos && cargo test -p {{crate}} {{args}}

# Run clippy + fmt check
lint:
    cd elastos && cargo clippy --workspace --all-targets -- -D warnings
    cd elastos && cargo fmt --all -- --check

# Stamp the shared UI token sheet into each consuming capsule
vendor-ui:
    ./scripts/vendor-ui-tokens.sh

# Auto-format code
fmt:
    cd elastos && cargo fmt --all

# Pre-commit gate: alignment, entropy, smoke tests, fmt/lint/test
verify:
    git --no-pager diff --check
    just alignment-check
    node scripts/check-elastos-bus-wit.mjs
    node scripts/check-capsule-templates.mjs
    ./scripts/vendor-ui-tokens.sh --check
    node scripts/home-entropy-check.mjs
    node scripts/carrier-dependency-generation-check.mjs
    just product-ui-source
    node scripts/home-clipboard-source-gate.mjs
    node scripts/browser-entropy-check.mjs
    node --test scripts/browser-window-close-handshake.test.mjs
    node --test scripts/home-two-runtime-acceptance.test.mjs
    python3 scripts/source-home-capsule-inventory-smoke.py
    ./scripts/command-smoke.sh
    ./scripts/browser-local-exit-orphan-cleanup-smoke.sh
    just candidate-command-audit
    cd elastos && cargo fmt --all -- --check
    cd elastos && cargo clippy --workspace --all-targets -- -D warnings
    just test
    # browser-local-exit has its own workspace under elastos/tools, so neither
    # the elastos workspace test nor test-capsules reaches it
    cd elastos/tools/browser-local-exit && cargo fmt -- --check
    cd elastos/tools/browser-local-exit && cargo clippy --all-targets -- -D warnings
    cd elastos/tools/browser-local-exit && cargo test

# Simulate CI (github actions), especially the test parts for
# both main steps (`test-capsules` and `test-elastos`)
verify-ci: ci-test-capsules ci-test-elastos    

product-ui-source:
    node scripts/home-shell-regression-smoke.mjs
    node scripts/people-discovery-smoke.mjs
    node scripts/inbox-product-behavior-smoke.mjs
    node scripts/archive-product-behavior-smoke.mjs
    node scripts/marketplace-product-behavior-smoke.mjs
    node scripts/documents-product-behavior-smoke.mjs
    node scripts/library-product-behavior-smoke.mjs
    node scripts/chat-room-product-behavior-smoke.mjs

product-ui-browser:
    node scripts/people-product-layout-smoke.mjs
    node scripts/inbox-product-layout-smoke.mjs
    node scripts/archive-product-layout-smoke.mjs
    node scripts/marketplace-product-layout-smoke.mjs
    node scripts/documents-product-layout-smoke.mjs
    node scripts/library-product-layout-smoke.mjs
    node scripts/chat-room-configured-layout-smoke.mjs

product-ui-virtual-auth:
    HOME_VIRTUAL_AUTH_APP_MATRIX=1 node scripts/home-passkey-virtual-auth-smoke.mjs

# Release-trust gate: requires canonical publisher signer, not the dev signer
verify-release:
    just verify
    just product-ui-browser
    just local-carrier-setup-smoke
    just home-frontdoor-smoke

# Fail-closed check for rooted-localhost and Home-first contract drift
alignment-check:
    ./scripts/check-wci-alignment.sh

# Verify the checked-in ElastOS Bus contract and real Component fixture
bus-conformance:
    node scripts/check-elastos-bus-wit.mjs
    cd elastos && cargo test -p elastos-server component_runs_through_real_bus_authority_provider_and_audit_paths -- --nocapture

# Validate canonical capsule scaffolds against the current manifest and WIT contracts
capsule-templates:
    node scripts/check-capsule-templates.mjs
    cd elastos && cargo test -p elastos-common capsule_authoring_templates_validate -- --nocapture

# Real-PTY source proof: current target-built elastos + current home-cli.wasm against clean-home data
home-frontdoor-smoke:
    ./scripts/home-frontdoor-smoke.sh

# Clean-home setup proof for the current local trusted-source path
local-carrier-setup-smoke:
    ./scripts/local-carrier-setup-smoke.sh

# Prepare and launch a clean temp-home local Home demo from source
home-demo-local *args:
    ./scripts/home-demo-local.sh {{args}}

# Audit an installed-style elastos binary on a clean home
installed-command-audit bin="":
    #!/usr/bin/env bash
    set -euo pipefail
    if [[ -n "{{bin}}" ]]; then
        ELASTOS_AUDIT_BIN="{{bin}}" ./scripts/installed-command-audit.sh
    else
        ./scripts/installed-command-audit.sh
    fi

# Build the release binary and audit its installed-style command surface
candidate-command-audit:
    #!/usr/bin/env bash
    set -euo pipefail
    cargo build -p elastos-server --release --manifest-path elastos/Cargo.toml
    audit_target_root="${CARGO_TARGET_DIR:-$PWD/elastos/target}"
    if [[ "$audit_target_root" != /* ]]; then
        audit_target_root="$PWD/$audit_target_root"
    fi
    ELASTOS_AUDIT_BIN="$audit_target_root/release/elastos" ./scripts/installed-command-audit.sh

# Clean build artifacts
clean:
    ./scripts/build/clean.sh

# Clean everything (artifacts + runtime data + caches)
clean-all:
    ./scripts/build/clean.sh --all

# Build rootfs for a single capsule
rootfs name:
    ./scripts/build/build-rootfs.sh {{name}}

# Build rootfs for all publish capsules
rootfs-all:
    #!/usr/bin/env bash
    set -euo pipefail
    capsules=(shell localhost-provider chat did-provider ipfs-provider tunnel-provider)
    for c in "${capsules[@]}"; do
        ./scripts/build/build-rootfs.sh "$c" --output artifacts/
    done
    echo "All rootfs builds complete."

# Full publish: build + rootfs + sign + upload
publish version key:
    ./scripts/publish-release.sh --version {{version}} --key {{key}}

# Quick re-publish: skip build + rootfs (re-sign and re-upload only)
publish-quick version key:
    ./scripts/publish-release.sh --version {{version}} --key {{key}} --skip-build --skip-rootfs

# Local publish: skip build + rootfs + no public URL (fastest)
publish-local version key:
    ./scripts/publish-release.sh --version {{version}} --key {{key}} --skip-build --skip-rootfs --no-public-url

# Run GBA emulator demo
gba *args:
    ./scripts/gba.sh {{args}}
