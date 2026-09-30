#!/usr/bin/env bash
# Collect the local CI checks without hiding later failures.
set -uo pipefail

cd "$(dirname "$0")/.."

source_commit="$(git rev-parse HEAD)"
source_tree="$(git rev-parse HEAD^{tree})"
host_target="$(rustc -vV | sed -n 's/^host: //p')"
toolchain="$(rustc --version)"
printf 'source=%s tree=%s\n' "$source_commit" "$source_tree"
printf 'environment=%s %s; %s; node=%s\n' "$(uname -s)" "$(uname -m)" "$toolchain" "$(node --version)"
printf 'persistent_cache=%s (toolchain=%s target=%s)\n' "$PWD/target-build" "$toolchain" "$host_target"
git status --short

failed=0
results=()
run() {
    local name="$1"
    shift
    printf '\n== %s ==\n' "$name"
    if "$@"; then
        results+=("$name:pass")
    else
        results+=("$name:fail")
        failed=1
    fi
}

disk_safe() {
    df -Pk . | awk 'NR == 2 { exit !($4 * 10 >= $2) }'
}

run node-26 bash -c '[[ "$(node -p "process.versions.node.split(\".\")[0]")" == 26 ]]'
run rust-1.91 bash -c '[[ "$(rustc --version)" == "rustc 1.91.0 "* ]]'
run diff-check git diff --check
run rust-format bash -c 'cd elastos && cargo fmt --all -- --check'
run chain-format cargo fmt --manifest-path capsules/chain-provider/Cargo.toml -- --check
run alignment bash scripts/check-wci-alignment.sh
run home-entropy node scripts/home-entropy-check.mjs
run browser-entropy node scripts/browser-entropy-check.mjs
run home-shell node scripts/home-shell-regression-smoke.mjs
run agent-shell node scripts/home-agent-shell-smoke.mjs
run agent-cancellation node --test scripts/home-agent-cancellation.test.mjs
run people-discovery node scripts/people-discovery-smoke.mjs
run components python3 scripts/components-release-integrity-check.py --self-test
run publish-platform python3 scripts/publish-platform-artifacts-test.py
run release-input python3 scripts/release-platform-input-test.py
run prepare-platform python3 scripts/prepare-release-platform-test.py
run media-tools python3 scripts/media-tools-build-test.py
run update-hop python3 scripts/update-hop-compare-test.py
run browser-close node --test scripts/browser-window-close-handshake.test.mjs

if ! disk_safe; then
    printf '\nDisk reserve below 10%%; Rust checks unavailable.\n'
    results+=("lint:unavailable" "test-elastos:unavailable" "test-capsules:unavailable")
    failed=1
else
    export RUSTFLAGS='-D warnings'
    run lint just lint
    if disk_safe; then
        run test-elastos just test-elastos
    else
        results+=("test-elastos:unavailable")
        failed=1
    fi
    if disk_safe; then
        run test-capsules env RUST_TEST_THREADS=1 just test-capsules
    else
        results+=("test-capsules:unavailable")
        failed=1
    fi
fi

if [[ "$(uname -s)" != Linux ]]; then
    printf '\nLinux source checks: unavailable on this host. GitHub CI must verify them.\n'
    results+=("linux:unavailable")
    failed=1
fi

printf '\n== summary ==\n'
printf '%s\n' "${results[@]}"
exit "$failed"
