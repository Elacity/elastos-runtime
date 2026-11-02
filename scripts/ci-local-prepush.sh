#!/usr/bin/env bash
# Keep this entry point compatible with macOS Bash 3.
set -euo pipefail
exec python3 "$(dirname "$0")/ci-local-prepush.py" "$@"
