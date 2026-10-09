#!/usr/bin/env bash
# The one behaviour suite: CI test-behaviour, `just test-behaviour` and the
# local pre-push gate all run this file. It installs the pinned Browser test
# inputs once per directory, then runs the suite; the spec reporter ends with
# the failing tests by name.
set -euo pipefail
cd "$(dirname "$0")/.."

inputs="${BEHAVIOUR_INPUTS:-${RUNNER_TEMP:-${HOME}/.cache/elastos-runtime}/behaviour}"
install() { # prefix package version
  if [ "$(node -p "try { require('$1/node_modules/$2/package.json').version } catch { '' }")" != "$3" ]; then
    npm install --prefix "$1" --no-save --package-lock=false "$2@$3"
  fi
}
install "${inputs}/browser" playwright 1.60.0
install "${inputs}/core" playwright-core 1.59.1
# --with-deps installs Linux system libraries and needs root; CI only.
"${inputs}/browser/node_modules/.bin/playwright" install chromium --only-shell ${CI:+--with-deps}

export NODE_PATH="${inputs}/browser/node_modules"
export BROWSER_OPERATOR_PLAYWRIGHT_CORE="${inputs}/core/node_modules/playwright-core"
exec node --test --test-timeout=60000 --test-reporter=spec elastos/esp/projections.test.mjs scripts/*.test.mjs scripts/build/*.test.mjs scripts/lib/*.test.mjs capsules/*/browser/*.test.mjs capsules/*/browser/src/*.test.mjs scripts/home-fixture-contracts-smoke.mjs scripts/documents-save-conflict-smoke.mjs scripts/gba-save-conflict-smoke.mjs
