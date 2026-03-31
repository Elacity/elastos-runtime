#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TARGET_DIR="$ROOT/capsules/gba-emulator"
PACKAGE_SPEC="${1:-@thenick775/mgba-wasm@2.4.1}"

tmpdir="$(mktemp -d /tmp/elastos-gba-runtime-XXXXXX)"
trap 'rm -rf "$tmpdir"' EXIT

cd "$tmpdir"
npm pack "$PACKAGE_SPEC" >/dev/null
tar -xzf ./*.tgz

install -m 0644 package/dist/mgba.js "$TARGET_DIR/mgba.js"
install -m 0644 package/dist/mgba.wasm "$TARGET_DIR/mgba.wasm"

cat >"$TARGET_DIR/UPSTREAM.md" <<EOF
# Upstream Runtime

This capsule vendors the browser runtime from:

- npm: \`$PACKAGE_SPEC\`
- repository: <https://github.com/thenick775/mgba.git>
- license: MPL-2.0

These files are vendored here:

- \`mgba.js\`
- \`mgba.wasm\`

Refresh with:

\`\`\`bash
bash scripts/vendor-gba-runtime.sh
\`\`\`

This viewer expects COOP/COEP headers so SharedArrayBuffer and threaded WASM remain available.
ElastOS already applies those headers when serving web capsules.
EOF

echo "Vendored GBA runtime into $TARGET_DIR"
