#!/usr/bin/env bash
# The fast source checks: CI source-gate and the local pre-push gate both run
# this file, so a failure shows locally before it shows in CI.
#   test-source-gate.sh           runs every section
#   test-source-gate.sh SECTION   runs one section (CI names one per step):
#                                 product-data, prepush, product-ui, release
set -euo pipefail
cd "$(dirname "$0")/.."

product_data() {
  node scripts/check-product-data.mjs
}

prepush() {
  python3 scripts/ci-local-prepush-test.py
}

product_ui() {
  node --version
  node scripts/check-chat-ui-artifacts.mjs check
  node scripts/home-shell-regression-smoke.mjs
  node scripts/home-shell-bridge-smoke.mjs
  node scripts/home-agent-shell-smoke.mjs
  node scripts/people-discovery-smoke.mjs
}

release() {
  node --test scripts/ci-model-package.test.mjs scripts/ci-installed-home-failure.test.mjs
  python3 scripts/ci-release-policy-test.py
  python3 scripts/ci-download-cache-test.py
  python3 scripts/components-release-integrity-check.py --self-test
  python3 scripts/publish-platform-artifacts-test.py
  python3 scripts/release-platform-input-test.py
  python3 scripts/prepare-release-platform-test.py
  python3 scripts/media-tools-build-test.py
  python3 scripts/verify-linux-arm64-package-test.py
  python3 scripts/verify-linux-engine-test.py
  python3 scripts/update-hop-compare-test.py
  python3 .github/scripts/release-mac-capacity-test.py
}

case "${1:-all}" in
  product-data) product_data ;;
  prepush) prepush ;;
  product-ui) product_ui ;;
  release) release ;;
  all) product_data; prepush; product_ui; release ;;
  *) echo "usage: $0 [product-data|prepush|product-ui|release]" >&2; exit 2 ;;
esac
