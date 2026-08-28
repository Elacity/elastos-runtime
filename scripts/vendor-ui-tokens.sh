#!/usr/bin/env bash
# Vendors the shared UI token sheet (capsules/_shared/elastos-ui.css) and the
# Inter font into each participating capsule's browser dir. Capsules stay
# self-contained: no gateway route, no runtime sharing — just identical files
# stamped from one source (Principle 10: one canonical path; Principle 12:
# drift is contract drift).
#
#   ./scripts/vendor-ui-tokens.sh          # sync copies from the source
#   ./scripts/vendor-ui-tokens.sh --check  # fail (exit 1) if any copy drifted
set -euo pipefail

cd "$(dirname "$0")/.."

SOURCE_CSS="capsules/_shared/elastos-ui.css"
SOURCE_JS="capsules/_shared/elastos-theme.js"
SOURCE_PICKER_JS="capsules/_shared/elastos-accent-picker.js"
SOURCE_FONT="capsules/_shared/fonts/Inter-latin-var.woff2"
HEADER="/* GENERATED from ${SOURCE_CSS} — do not edit. Run \`just vendor-ui\`. */"
JS_HEADER="/* GENERATED from ${SOURCE_JS} — do not edit. Run \`just vendor-ui\`. */"
PICKER_JS_HEADER="/* GENERATED from ${SOURCE_PICKER_JS} — do not edit. Run \`just vendor-ui\`. */"

# Capsules that consume the shared tokens today. Extend as apps migrate: a
# capsule joins this list in the same change that makes it reference the tokens,
# so no capsule ships assets it does not use.
# Entries are the browser-serving dir relative to capsules/ — most apps serve
# from a browser/ subdir; viewer-style capsules serve straight from their root.
# home/browser is the shell host (unlock surface); home-gui/browser is the GUI
# shell package. home-cli stays out: its terminal surface is capsule-local xterm
# rendering by contract. The vendored theme runtime owns no browser-profile
# storage, so it is safe in an opaque capsule; only the host installs a
# persistence adapter, in its own non-vendored file.
TARGETS=(
  home/browser
  home-gui/browser
  people/browser
  inbox/browser
  chat-room/browser
)

MODE="${1:-sync}"
FAILED=0

stamped_source() {
  printf '%s\n' "$HEADER"
  cat "$SOURCE_CSS"
}

stamped_js_source() {
  printf '%s\n' "$JS_HEADER"
  cat "$SOURCE_JS"
}

stamped_picker_js_source() {
  printf '%s\n' "$PICKER_JS_HEADER"
  cat "$SOURCE_PICKER_JS"
}

for target_dir in "${TARGETS[@]}"; do
  browser_dir="capsules/${target_dir}"
  css_target="${browser_dir}/elastos-ui.css"
  font_dir="${browser_dir}/assets/fonts"
  font_target="${font_dir}/Inter-latin-var.woff2"

  if [[ ! -d "$browser_dir" ]]; then
    echo "[vendor-ui] MISSING capsule browser dir: ${browser_dir}" >&2
    exit 1
  fi

  js_target="${browser_dir}/elastos-theme.js"
  picker_js_target="${browser_dir}/elastos-accent-picker.js"

  if [[ "$MODE" == "--check" ]]; then
    if [[ ! -f "$css_target" ]] || ! diff -q <(stamped_source) "$css_target" >/dev/null 2>&1; then
      echo "[vendor-ui] DRIFT: ${css_target} does not match ${SOURCE_CSS}" >&2
      FAILED=1
    fi
    if [[ ! -f "$js_target" ]] || ! diff -q <(stamped_js_source) "$js_target" >/dev/null 2>&1; then
      echo "[vendor-ui] DRIFT: ${js_target} does not match ${SOURCE_JS}" >&2
      FAILED=1
    fi
    if [[ ! -f "$picker_js_target" ]] || ! diff -q <(stamped_picker_js_source) "$picker_js_target" >/dev/null 2>&1; then
      echo "[vendor-ui] DRIFT: ${picker_js_target} does not match ${SOURCE_PICKER_JS}" >&2
      FAILED=1
    fi
    if [[ ! -f "$font_target" ]] || ! cmp -s "$SOURCE_FONT" "$font_target"; then
      echo "[vendor-ui] DRIFT: ${font_target} does not match ${SOURCE_FONT}" >&2
      FAILED=1
    fi
  else
    mkdir -p "$font_dir"
    stamped_source > "$css_target"
    stamped_js_source > "$js_target"
    stamped_picker_js_source > "$picker_js_target"
    cp "$SOURCE_FONT" "$font_target"
    echo "[vendor-ui] stamped ${css_target}"
  fi
done

if [[ "$MODE" == "--check" ]]; then
  if [[ "$FAILED" -ne 0 ]]; then
    echo "[vendor-ui] FAIL — run \`just vendor-ui\` and commit the result" >&2
    exit 1
  fi
  echo "[vendor-ui] OK"
fi
