#!/usr/bin/env bash
set -euo pipefail

# Python owns subprocess groups and deadlines on native macOS.
exec python3 "$(cd "$(dirname "$0")" && pwd)/update-hop-compare.py" "$@"
