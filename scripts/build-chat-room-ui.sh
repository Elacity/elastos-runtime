#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
UI_DIR="${ROOT}/capsules/chat-room-ui"
OUT_DIR="${ROOT}/capsules/chat-room/browser"
TARGET_WASM="${UI_DIR}/target/wasm32-unknown-unknown/release/chat_room_ui.wasm"
STAMP_FILE="${UI_DIR}/.chat_room_ui_source.sha256"
TARGET_JS="${OUT_DIR}/chat_room_ui.js"
TARGET_BINDGEN_WASM="${OUT_DIR}/chat_room_ui_bg.wasm"

cargo build --manifest-path "${UI_DIR}/Cargo.toml" --target wasm32-unknown-unknown --release

SOURCE_HASH="$(sha256sum "${TARGET_WASM}" | awk '{print $1}')"
if [[ -f "${STAMP_FILE}" ]] \
    && [[ -f "${TARGET_JS}" ]] \
    && [[ -f "${TARGET_BINDGEN_WASM}" ]] \
    && [[ "$(cat "${STAMP_FILE}")" == "${SOURCE_HASH}" ]]; then
    echo "chat-room browser assets already match ${SOURCE_HASH}; skipping wasm-bindgen"
    exit 0
fi

TMP_DIR="$(mktemp -d)"
trap 'rm -rf "${TMP_DIR}"' EXIT

# wasm-bindgen 0.2.105 reorders generated closure shims between identical runs.
# Cache on the stable compiler-output hash so unchanged sources do not rewrite
# the committed browser artifacts during verification.
wasm-bindgen --target web --out-dir "${TMP_DIR}" --out-name chat_room_ui "${TARGET_WASM}"
rm -f "${TMP_DIR}/chat_room_ui.d.ts" "${TMP_DIR}/chat_room_ui_bg.wasm.d.ts"
mv "${TMP_DIR}/chat_room_ui.js" "${TARGET_JS}"
mv "${TMP_DIR}/chat_room_ui_bg.wasm" "${TARGET_BINDGEN_WASM}"
printf '%s\n' "${SOURCE_HASH}" > "${STAMP_FILE}"
