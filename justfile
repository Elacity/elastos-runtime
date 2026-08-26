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

# Run workspace tests
test *args:
    cd elastos && cargo test --workspace {{args}}

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
    git diff --check
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
    just candidate-command-audit
    cd elastos && cargo fmt --all -- --check
    cd elastos && cargo clippy --workspace --all-targets -- -D warnings
    cd elastos && cargo test --workspace

product-ui-source:
    node scripts/home-shell-regression-smoke.mjs
    node scripts/people-discovery-smoke.mjs
    node scripts/inbox-product-behavior-smoke.mjs
    node scripts/documents-product-behavior-smoke.mjs
    node scripts/library-product-behavior-smoke.mjs
    node scripts/chat-room-product-behavior-smoke.mjs

product-ui-browser:
    node scripts/people-product-layout-smoke.mjs
    node scripts/inbox-product-layout-smoke.mjs
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
