#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
UI_DIR="${ROOT}/capsules/chat-room-ui"
OUT_DIR="${ROOT}/capsules/chat-room/browser"
TARGET_WASM="${CARGO_TARGET_DIR:-${UI_DIR}/target}/wasm32-unknown-unknown/release/chat_room_ui.wasm"
TARGET_JS="${OUT_DIR}/chat_room_ui.js"
TARGET_BINDGEN_WASM="${OUT_DIR}/chat_room_ui_bg.wasm"

# Remap builder-specific paths so the wasm is byte-identical across machines
# and checkout locations (panic/debug strings otherwise embed absolute paths).
CARGO_HOME_DIR="${CARGO_HOME:-${HOME}/.cargo}"
SYSROOT="$(rustc --print sysroot)"
REMAP_FLAGS=(
    "--remap-path-prefix=${ROOT}=/elastos-runtime"
    "--remap-path-prefix=${CARGO_HOME_DIR}=/cargo"
    "--remap-path-prefix=${SYSROOT}=/rustc-sysroot"
)
ENCODED_FLAGS="$(printf '%s\x1f' "${REMAP_FLAGS[@]}")"
CARGO_ENCODED_RUSTFLAGS="${ENCODED_FLAGS%$'\x1f'}" RUSTFLAGS='' \
    cargo build --locked --manifest-path "${UI_DIR}/Cargo.toml" --target wasm32-unknown-unknown --release

if node "${ROOT}/scripts/check-chat-ui-artifacts.mjs" compiler-match "${TARGET_WASM}"; then
    echo "chat-room source and browser assets match the compiler output; skipping wasm-bindgen"
    exit 0
fi

TMP_DIR="$(mktemp -d)"
trap 'rm -rf "${TMP_DIR}"' EXIT

# wasm-bindgen 0.2.105 reorders generated closure shims between identical runs.
# Reuse requires the source fingerprint, compiler hash and browser hashes to
# match, so verification leaves current committed browser artifacts alone.
wasm-bindgen --target web --out-dir "${TMP_DIR}" --out-name chat_room_ui "${TARGET_WASM}"
rm -f "${TMP_DIR}/chat_room_ui.d.ts" "${TMP_DIR}/chat_room_ui_bg.wasm.d.ts"
mv "${TMP_DIR}/chat_room_ui.js" "${TARGET_JS}"
mv "${TMP_DIR}/chat_room_ui_bg.wasm" "${TARGET_BINDGEN_WASM}"
node "${ROOT}/scripts/check-chat-ui-artifacts.mjs" stamp "${TARGET_WASM}"
