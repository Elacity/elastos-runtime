#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

for file in capsules/browser/*.js; do
  node --check "$file"
done

cargo test --manifest-path capsules/browser-engine-adapter/Cargo.toml
cargo test --manifest-path capsules/net-provider/Cargo.toml
cargo test --manifest-path capsules/exit-provider/Cargo.toml

(cd elastos && cargo test -p elastos-server browser --lib)

printf '{"schema":"elastos.browser.abi-provider-contract-smoke/v1","ok":true}\n'
