#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

run() {
  printf '\n==> %s\n' "$*"
  "$@"
}

single_test_filters=(
  test_metamask_can_link_multiple_accounts_and_wallet_can_remove_one
  test_wallet_app_can_delete_managed_account
  test_wallet_recovery_key_requires_passkey_step_up
  test_wallet_recovery_key_import_requires_passkey_step_up
  test_wallet_summary_reports_walletconnect_available_only_when_pinned
  test_walletconnect_connector_requires_pinned_config
)
browser_wallet_bridge_filter=browser_wallet_bridge_tests

printf '\n==> validating Wallet safety test filters\n'
count_library_tests() {
  cargo test --manifest-path elastos/Cargo.toml -p elastos-server \
    --lib "$1" -- --list |
    awk '/: test$/ { matches++ } END { print matches + 0 }'
}

for filter in "${single_test_filters[@]}"; do
  matches="$(count_library_tests "$filter")"
  if [ "$matches" -ne 1 ]; then
    printf 'Wallet safety filter %s resolved to %s library tests; expected exactly one\n' \
      "$filter" "$matches" >&2
    exit 1
  fi
  printf '[wallet-product-safety-smoke] %s -> 1 library test\n' "$filter"
done

browser_wallet_bridge_matches="$(count_library_tests "$browser_wallet_bridge_filter")"
if [ "$browser_wallet_bridge_matches" -eq 0 ]; then
  printf 'Wallet safety filter %s resolved to zero library tests\n' \
    "$browser_wallet_bridge_filter" >&2
  exit 1
fi
printf '[wallet-product-safety-smoke] %s -> %s library tests\n' \
  "$browser_wallet_bridge_filter" "$browser_wallet_bridge_matches"

for filter in "${single_test_filters[@]}"; do
  run cargo test --manifest-path elastos/Cargo.toml -p elastos-server \
    --lib "$filter" -- --nocapture
done
run cargo test --manifest-path elastos/Cargo.toml -p elastos-server \
  --lib "$browser_wallet_bridge_filter" -- --nocapture

node --input-type=module <<'NODE'
import { readFileSync } from "node:fs";

const read = (path) => readFileSync(path, "utf8");
const assert = (condition, message) => {
  if (!condition) {
    throw new Error(message);
  }
};

const walletIndex = read("capsules/wallet/browser/index.html");
const walletPreferences = read("capsules/wallet/browser/wallet-preferences.js");
const walletFormat = read("capsules/wallet/browser/wallet-format.js");
const browserSources = [
  "capsules/browser/browser/index.html",
  "capsules/browser/browser/browser.js",
  "capsules/browser/browser/browser-runtime-api.js",
  "capsules/browser/browser/browser-webrtc.js",
  "capsules/browser/browser/browser-remote-display.js",
].map(read).join("\n");

assert(
  !/\bledger\b|wallet-ledger/i.test(`${walletIndex}\n${walletPreferences}\n${walletFormat}`),
  "Ledger must stay hidden until a real connector is implemented and tested",
);
assert(
  walletPreferences.includes("walletConnectAvailable") &&
    walletPreferences.includes('method.id !== "wc" || walletConnectAvailable'),
  "WalletConnect must stay hidden unless the operator-pinned connector config is available",
);
assert(
  !/wallet-unisat|UniSat|window\.unisat/i.test(browserSources),
  "Hosted Browser must not advertise UniSat as an injected browser extension path",
);
assert(
  walletPreferences.includes('target: "wallet-metamask"') &&
    walletPreferences.includes('target: "wallet-unisat"') &&
    walletPreferences.includes("[data-wallet-remove-account]"),
  "Wallet approval methods must expose connector-owned add/open actions and Wallet-owned remove actions",
);

console.log("[wallet-product-safety-smoke] static UI/connector invariants OK");
NODE

echo "[wallet-product-safety-smoke] OK"
