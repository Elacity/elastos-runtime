#!/usr/bin/env bash
# The one behaviour suite: CI test-behaviour, `just test-behaviour` and the
# local pre-push gate all run this file.
#   test-behaviour.sh inputs  installs the pinned Browser test inputs (network)
#   test-behaviour.sh         runs the suite offline; the spec reporter ends
#                             with the failing tests by name
set -euo pipefail
cd "$(dirname "$0")/.."

inputs="${BEHAVIOUR_INPUTS:-${RUNNER_TEMP:-${HOME}/.cache/elastos-runtime}/behaviour}"
browser="${inputs}/browser" core="${inputs}/core"
installed() { # prefix package version
  [ "$(node -p "try { require('$1/node_modules/$2/package.json').version } catch { '' }")" = "$3" ]
}

if [ "${1:-}" = inputs ]; then
  installed "$browser" playwright 1.60.0 ||
    npm install --prefix "$browser" --no-save --package-lock=false playwright@1.60.0
  if ! installed "$core" playwright-core 1.59.1 || ! installed "$core" node 24.21.0; then
    npm install --prefix "$core" --no-save --package-lock=false playwright-core@1.59.1 node@24.21.0
  fi
  # Tests use both SDK pins; keep both browser builds in the shared cache.
  export PLAYWRIGHT_SKIP_BROWSER_GC=1
  # --with-deps installs Linux system libraries and needs root; CI only.
  "${browser}/node_modules/.bin/playwright" install chromium --only-shell ${CI:+--with-deps}
  # The 1.59.1 installer stalls extracting Chromium under Node 26.
  "${core}/node_modules/node/bin/node" "${core}/node_modules/playwright-core/cli.js" install chromium-headless-shell
  exit 0
fi

if ! installed "$browser" playwright 1.60.0 || ! installed "$core" playwright-core 1.59.1; then
  echo "Browser test inputs are missing: run \`just test-behaviour-inputs\` first." >&2
  exit 1
fi
export NODE_PATH="${browser}/node_modules"
export BROWSER_OPERATOR_PLAYWRIGHT_CORE="${core}/node_modules/playwright-core"
exec node --test --test-force-exit --test-timeout=60000 --test-reporter=spec elastos/esp/projections.test.mjs scripts/*.test.mjs scripts/build/*.test.mjs scripts/lib/*.test.mjs capsules/*/browser/*.test.mjs capsules/*/browser/src/*.test.mjs scripts/home-fixture-contracts-smoke.mjs scripts/documents-save-conflict-smoke.mjs scripts/gba-save-conflict-smoke.mjs scripts/assistant-shell-smoke.mjs scripts/model-preparation-failure-smoke.mjs scripts/model-management-browser-smoke.mjs
