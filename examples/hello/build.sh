#!/usr/bin/env bash
set -euo pipefail

HELLO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${HELLO_DIR}/../.." && pwd)"

"${REPO_ROOT}/scripts/build-component-capsule.sh" "${HELLO_DIR}"

