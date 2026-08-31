#!/usr/bin/env node

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

function assert(condition, message, details = undefined) {
  if (condition) {
    return;
  }
  const suffix = details === undefined ? "" : `\n${JSON.stringify(details, null, 2)}`;
  throw new Error(`${message}${suffix}`);
}

const repoRoot = resolve(fileURLToPath(new URL("../", import.meta.url)));
const walletRoot = resolve(repoRoot, "capsules/wallet/browser");
const indexHtml = readFileSync(resolve(walletRoot, "index.html"), "utf8");
const styleCss = readFileSync(resolve(walletRoot, "style.css"), "utf8");
const walletJs = readFileSync(resolve(walletRoot, "wallet.js"), "utf8");
const walletPreferencesJs = readFileSync(resolve(walletRoot, "wallet-preferences.js"), "utf8");
const walletSendFlowJs = readFileSync(resolve(walletRoot, "wallet-send-flow.js"), "utf8");
const walletReceiveFlowJs = readFileSync(resolve(walletRoot, "wallet-receive-flow.js"), "utf8");
const walletAccountActionsJs = readFileSync(resolve(walletRoot, "wallet-account-actions.js"), "utf8");
const walletFormatJs = readFileSync(resolve(walletRoot, "wallet-format.js"), "utf8");
const vendorScript = readFileSync(resolve(repoRoot, "scripts/vendor-ui-tokens.sh"), "utf8");

const letterSpacingValues = [...styleCss.matchAll(/letter-spacing:\s*([^;]+);/g)].map((match) => match[1].trim());

assert(
  indexHtml.includes('<script src="./elastos-theme.js"></script>')
    && indexHtml.includes('<link rel="stylesheet" href="./elastos-ui.css">')
    && indexHtml.includes('src="./elastos-logo.svg"')
    && indexHtml.includes('id="wallet-get-started"')
    && indexHtml.includes('id="wallet-hero-pending"'),
  "Wallet must load the canonical shared theme assets and the reviewed hero surface.",
);
assert(
  vendorScript.includes("wallet/browser")
    && vendorScript.includes("wallet-metamask/browser")
    && vendorScript.includes("wallet-unisat/browser")
    && vendorScript.includes("wallet-walletconnect/browser"),
  "Wallet family must participate in the canonical shared token vendoring list.",
);
assert(
  letterSpacingValues.length > 0 && letterSpacingValues.every((value) => value === "0"),
  "Wallet UIUX must keep every letter-spacing declaration at 0.",
  { letterSpacingValues },
);
assert(
  walletJs.includes("homeClipboard.start();")
    && !walletJs.includes('window.top.postMessage({ type: "home:app-ready", homeToken: activeHomeToken }, homeParentOrigin);')
    && walletJs.includes('if (event.origin !== homeParentOrigin || event.source !== window.top) {')
    && walletJs.includes('if (event.origin !== "null" || event.source !== window.parent) {')
    && walletJs.includes('type: "wallet:pending-count"')
    && walletPreferencesJs.includes('type: "wallet:privacy-state"'),
  "Wallet must keep one canonical Home app-ready path plus exact Home top/runtime and opaque parent chrome boundaries.",
);
assert(
  walletPreferencesJs.includes('type: "home:open-target"')
    && !walletPreferencesJs.includes("window.location.origin")
    && !walletPreferencesJs.includes("navigator.clipboard")
    && !walletPreferencesJs.includes("execCommand"),
  "Wallet connector launch must stay on one Home open-target path with no clipboard fallback.",
);
assert(
  !walletJs.includes("navigator.clipboard")
    && !walletJs.includes("execCommand")
    && !walletJs.includes("sessionStorage")
    && !walletJs.includes("indexedDB")
    && !walletPreferencesJs.includes("sessionStorage")
    && !walletAccountActionsJs.includes("window.confirm"),
  "Wallet must not add browser authority or browser confirm fallbacks.",
);
assert(
  walletFormatJs.includes('localStorage.getItem(key)')
    && walletFormatJs.includes('localStorage.setItem(key, value)')
    && walletPreferencesJs.includes('readStoredBoolean("wallet.privacy")')
    && walletPreferencesJs.includes('DISPLAY_CURRENCY_STORAGE_KEY')
    && !walletJs.includes("localStorage"),
  "Wallet browser storage must stay limited to display-only currency and privacy preferences.",
);
assert(
  walletJs.includes('return `${method}:eip155:${address}`;')
    && walletJs.includes("account.account_ids.includes(defaultAccount.account_id)")
    && walletSendFlowJs.includes("account_id: asset.account_id || account.account_id")
    && walletReceiveFlowJs.includes('body: JSON.stringify({ address: account.address })')
    && walletReceiveFlowJs.includes("This EVM address can receive assets on supported EVM networks."),
  "Grouped EVM cards must stay display-only while Send and Receive bind the exact reviewed address or asset record.",
);
assert(
  walletAccountActionsJs.includes("Choose the exact network")
    && walletAccountActionsJs.includes("This grouped card updates")
    && walletAccountActionsJs.includes("This grouped card removes")
    && walletAccountActionsJs.includes("Promise.all(accountIds(account).map((accountId) =>")
    && walletAccountActionsJs.includes("for (const accountId of accountIds(account))")
    && walletAccountActionsJs.includes("defaultIntentLabel(record)")
    && walletAccountActionsJs.includes("showRecoveryKey(record)"),
  "Grouped EVM account actions must make grouped scope explicit and bind default or recovery to exact underlying records.",
);

const walletFormatModule = await import(pathToFileURL(resolve(walletRoot, "wallet-format.js")).href);

assert(
  walletFormatModule.accountDisplayBalance(
    { balanceAvailable: true, amount: 1.5, priceAvailable: false, usd: 0, symbol: "ETH" },
    {},
    "usd",
  ) === "1.5 ETH",
  "Wallet must keep the zero-price native balance fallback for funded assets.",
);
assert(
  walletFormatModule.isEvmChainNamespace("eip155:20")
    && walletFormatModule.isEvmChainNamespace("eip155")
    && !walletFormatModule.isEvmChainNamespace("bip122:000000000019d6689c085ae165831e93"),
  "Wallet must keep the reviewed EVM namespace detection.",
);
assert(
  walletFormatModule.cardNetworkLabel({
    proof_type: "managed_evm",
    chain_namespace: "eip155:20",
  }) === "Built-in · EVM",
  "Wallet card network labels must keep the reviewed built-in EVM grouping language.",
);

console.log("wallet-product-behavior-smoke: OK");
