#!/usr/bin/env bash
set -euo pipefail

FIXTURE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$FIXTURE_DIR/../../../../.." && pwd)"

"$ROOT/scripts/build-component-capsule.sh" "$FIXTURE_DIR"
