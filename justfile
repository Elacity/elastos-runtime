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

# Auto-format code
fmt:
    cd elastos && cargo fmt --all

# Pre-commit gate: alignment, entropy, smoke tests, fmt/lint/test
verify:
    git diff --check
    just alignment-check
    node scripts/check-elastos-bus-wit.mjs
    node scripts/check-capsule-templates.mjs
    node scripts/home-entropy-check.mjs
    node scripts/home-clipboard-source-gate.mjs
    node scripts/browser-entropy-check.mjs
    node --test scripts/browser-window-close-handshake.test.mjs
    python3 scripts/source-home-capsule-inventory-smoke.py
    ./scripts/command-smoke.sh
    just candidate-command-audit
    cd elastos && cargo fmt --all -- --check
    cd elastos && cargo clippy --workspace --all-targets -- -D warnings
    cd elastos && cargo test --workspace

# Release-trust gate: requires canonical publisher signer, not the dev signer
verify-release:
    just verify
    just local-carrier-setup-smoke
    just home-frontdoor-smoke

# CI gate: the full `verify` MINUS the Carrier-network setup smoke (`local-carrier-setup-smoke`),
# which a stock GitHub runner cannot reach. Everything else a clean runner CAN verify runs here;
# the carrier smoke is covered separately on a Carrier-capable Linux box / self-hosted runner.
verify-ci:
    just alignment-check
    just _verify-tail

# (hidden) gate steps shared by `verify` and `verify-ci` — everything after the alignment-check
# + (verify-only) carrier-smoke preamble.
_verify-tail:
    ./scripts/command-smoke.sh
    just candidate-command-audit
    cd elastos && cargo fmt --all -- --check
    cd elastos && cargo clippy --workspace --all-targets -- -D warnings
    cd elastos && cargo test --workspace
    # The dev-modes lane (Sprint 46, council S46 guardian F3): the money-path construction
    # ratchets (S43 typed BuyError, the S46 prepare-leg deadline, chain-mock buys) are
    # `#[cfg(feature = "dev-modes")]` — without this lane the gate never compiles them and a
    # "ratchet" outside the gate cannot ratchet. Shares the workspace build cache; lib-only.
    cd elastos && cargo test -p elastos-server --lib --features dev-modes
    just verify-capsules

# Build + test the dDRM capsule crates the elastos-workspace gate does not reach. These crates
# carry the protected-content surface (watermark codec, grant-digest envelope, media-authority), are
# exercised under their CANONICAL feature sets (matching scripts/dev/run-creator-gateway.sh), and
# gated by build+test only (clippy -D warnings is held back for now: pre-existing lint debt).
verify-capsules:
    cd capsules/decrypt-provider && cargo test --features rail-stream,rail-mint,pdf-render,pq-envelope
    cd capsules/ddrm-envelope && cargo test --features access-grant,av-variants
    # The dKMS EXTERNAL key-authority node: the full remediation regression suite (owner-only
    # entitlement, RPC agreement, bounded replay, lifecycle-manifest v2, durable revocation,
    # offline provisioning, allow-list posture). The elastos workspace does NOT include this crate,
    # so without this line the whole dKMS regression suite (70+ tests) never runs in CI. Both the
    # default (secure, legacy path fenced OUT) and the `dev-modes` lane (legacy-receipt-authz
    # migration scaffold) are exercised so a ratchet under one config cannot silently rot the other.
    cd capsules/dkms-authority && cargo test
    cd capsules/dkms-authority && cargo test --features dev-modes
    cd scripts/dev/ddrm-media-authority && cargo test
    # AV forensic cross-language weld: the Python extractor's canonical codeword must match the Rust
    # serve selector byte-for-byte (golden vectors on both sides). Pure stdlib — no numpy/ffmpeg.
    python3 tools/av-forensics/test_canonical.py

# Fail-closed check for rooted-localhost and Home-first contract drift
alignment-check:
    ./scripts/check-wci-alignment.sh

# Verify the checked-in ElastOS Bus contract and real Component fixture
bus-conformance:
    #!/usr/bin/env bash
    set -euo pipefail
    node scripts/check-elastos-bus-wit.mjs
    cd elastos
    # Full module path + --exact so a rename can't silently drift the filter to zero matches.
    # cargo test still exits 0 on a zero-match filter (a vacuous pass), so we explicitly assert
    # exactly one test ran and passed rather than trusting the exit code alone.
    out=$(cargo test -p elastos-server --lib \
        runtime::tests::component_conformance_exercises_bus_authorization_dispatch_and_audit \
        -- --exact --nocapture 2>&1) && status=0 || status=$?
    echo "$out"
    if [ "$status" -ne 0 ]; then
        echo "::error::bus-conformance: test run failed" >&2
        exit "$status"
    fi
    if ! echo "$out" | grep -qE "test result: ok\. 1 passed"; then
        echo "::error::bus-conformance: expected exactly 1 test to run and pass (filter matched 0 — check the test name/path)" >&2
        exit 1
    fi

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
    ELASTOS_AUDIT_BIN="$PWD/elastos/target/release/elastos" ./scripts/installed-command-audit.sh

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

# Run P2P chat demo
chat *args:
    ./scripts/chat.sh {{args}}

# Run GBA emulator demo
gba *args:
    ./scripts/gba.sh {{args}}
