#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

run() {
  printf '\n==> %s\n' "$*"
  "$@"
}

run cargo test --manifest-path elastos/Cargo.toml -p elastos-server \
  test_metamask_can_link_multiple_accounts_and_wallet_can_remove_one -- --nocapture
run cargo test --manifest-path elastos/Cargo.toml -p elastos-server \
  test_wallet_app_can_delete_managed_account -- --nocapture
run cargo test --manifest-path elastos/Cargo.toml -p elastos-server \
  test_wallet_recovery_key_requires_fresh_passkey_home_token -- --nocapture
run cargo test --manifest-path elastos/Cargo.toml -p elastos-server \
  test_wallet_recovery_key_import_requires_fresh_passkey_home_token -- --nocapture
run cargo test --manifest-path elastos/Cargo.toml -p elastos-server \
  test_wallet_summary_reports_walletconnect_available_only_when_pinned -- --nocapture
run cargo test --manifest-path elastos/Cargo.toml -p elastos-server \
  test_walletconnect_connector_requires_pinned_config -- --nocapture
run cargo test --manifest-path elastos/Cargo.toml -p elastos-server \
  browser_wallet_bridge_tests --lib -- --nocapture

node --input-type=module <<'NODE'
import { readFileSync } from "node:fs";

const read = (path) => readFileSync(path, "utf8");
const assert = (condition, message) => {
  if (!condition) {
    throw new Error(message);
  }
};

const walletIndex = read("capsules/wallet/index.html");
const walletPreferences = read("capsules/wallet/wallet-preferences.js");
const walletFormat = read("capsules/wallet/wallet-format.js");
const browserSources = [
  "capsules/browser/index.html",
  "capsules/browser/browser.js",
  "capsules/browser/browser-runtime-api.js",
  "capsules/browser/browser-webrtc.js",
  "capsules/browser/browser-remote-display.js",
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
