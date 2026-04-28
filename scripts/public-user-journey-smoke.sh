#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)

bash "$SCRIPT_DIR/public-root-site-smoke.sh"
bash "$SCRIPT_DIR/system-camofox-smoke.sh"
bash "$SCRIPT_DIR/home-camofox-smoke.sh"
bash "$SCRIPT_DIR/chat-room-runtime-activity-smoke.sh"
bash "$SCRIPT_DIR/chat-room-session-reuse-camofox-smoke.sh"
bash "$SCRIPT_DIR/chat-room-guest-identity-camofox-smoke.sh"
bash "$SCRIPT_DIR/chat-room-gateway-camofox-smoke.sh"
