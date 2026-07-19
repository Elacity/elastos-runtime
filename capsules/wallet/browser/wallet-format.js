export const MANAGED_CHAIN_NAMESPACES = Object.freeze([
  "eip155:20",
  "eip155:8453",
  "bip122:000000000019d6689c085ae165831e93",
]);

export const DEFAULT_ACCOUNT_NAMES = Object.freeze({
  "eip155:20": "ELA Wallet",
  "eip155:8453": "Spending",
  "bip122:000000000019d6689c085ae165831e93": "Savings",
});

export const BALANCE_NETWORKS = Object.freeze({
  "eip155:20": { network: "esc-mainnet", symbol: "ELA", decimals: 18 },
  "eip155:8453": { network: "base-mainnet", symbol: "ETH", decimals: 18 },
  "bip122:000000000019d6689c085ae165831e93": { network: "btc-mainnet", symbol: "BTC", decimals: 8 },
});

export const CHAIN_LABELS = Object.freeze({
  "eip155:1": "Ethereum",
  "eip155:20": "Elastos Smart Chain",
  "eip155:8453": "Base",
  "bip122:000000000019d6689c085ae165831e93": "Bitcoin",
});

export const METHOD_LABELS = Object.freeze({
  passkey: "Passkey",
  metamask: "MetaMask",
  btc: "Bitcoin",
  wc: "WalletConnect",
});

export const METHOD_MONOGRAMS = Object.freeze({
  passkey: "P",
  metamask: "M",
  btc: "₿",
  wc: "W",
});

export const DISPLAY_CURRENCY_STORAGE_KEY = "wallet.displayCurrency";

export function readText(value) {
  return typeof value === "string" ? value.trim() : "";
}

export function readStoredValue(key, defaultValue, allowed) {
  try {
    const value = localStorage.getItem(key);
    return allowed.includes(value) ? value : defaultValue;
  } catch (_) {
    return defaultValue;
  }
}

export function readStoredBoolean(key) {
  try {
    return localStorage.getItem(key) === "1";
  } catch (_) {
    return false;
  }
}

export function storeValue(key, value) {
  try {
    localStorage.setItem(key, value);
  } catch (_) {}
}

export function accountName(account, index) {
  const label = cleanAccountLabel(account.label);
  if (label) {
    return label;
  }
  const connectorId = readText(account.connector_id);
  if (connectorId === "wallet-walletconnect") {
    return index === 0 ? "Connected Account" : "WalletConnect Account";
  }
  if (connectorId === "wallet-metamask" || (!connectorId && account.proof_type === "siwe")) {
    return index === 0 ? "Family" : "MetaMask Account";
  }
  if (connectorId === "wallet-unisat") {
    return "Cold Storage";
  }
  if (account.chain_namespace === "eip155:8453") {
    return "Spending";
  }
  if (account.chain_namespace === "eip155:20") {
    return "ELA Wallet";
  }
  if (account.chain_namespace === "bip122:000000000019d6689c085ae165831e93") {
    return "Savings";
  }
  return `${chainLabel(account.chain_namespace)} Account`;
}

export function cleanAccountLabel(label) {
  const value = readText(label);
  if (!value || value === "External account") {
    return "";
  }
  const oldManagedPrefix = `${"Pass"}key approval (`;
  if (value.startsWith(oldManagedPrefix) && value.endsWith(")")) {
    return value.slice(oldManagedPrefix.length, -1);
  }
  if (value === "Elastos Smart Chain" || value === "Base" || value === "Bitcoin") {
    return "";
  }
  return value;
}

export function methodForAccount(account) {
  const connectorId = readText(account.connector_id);
  if (connectorId === "wallet-metamask") {
    return "metamask";
  }
  if (connectorId === "wallet-unisat") {
    return "btc";
  }
  if (connectorId === "wallet-walletconnect") {
    return "wc";
  }
  if (!connectorId && account.proof_type === "siwe") {
    return "metamask";
  }
  if (account.proof_type === "managed_btc_p2wpkh" || account.proof_type === "managed_evm") {
    return "passkey";
  }
  return "unknown";
}

export function namesForMethod(accounts, method) {
  return accounts.filter((account) => account.method === method).map((account) => account.name);
}

export function chainLabel(namespace) {
  return CHAIN_LABELS[namespace] || namespace || "Network";
}

export function accountDisplayBalance(account, prices, displayCurrency) {
  if (!account.balanceAvailable) {
    return "—";
  }
  if (account.priceAvailable || Object.keys(prices).length > 0) {
    return formatMoney(account.usd, displayCurrency, prices);
  }
  return `${formatAmount(account.amount)} ${account.symbol}`;
}

export function formatMoney(usd, currency, prices) {
  if (!Number.isFinite(usd)) {
    return "—";
  }
  if (currency === "ela") {
    const price = prices.ELA?.usd;
    return price ? `${formatAmount(usd / price, 4)} ELA` : "—";
  }
  if (currency === "btc") {
    const price = prices.BTC?.usd;
    return price ? `₿ ${formatAmount(usd / price, usd / price < 0.001 ? 6 : 4)}` : "—";
  }
  return new Intl.NumberFormat("en-US", {
    style: "currency",
    currency: "USD",
    maximumFractionDigits: usd > 1000 ? 0 : 2,
  }).format(usd);
}

export function formatAmount(value, maxFractionDigits = 6) {
  if (!Number.isFinite(value)) {
    return "0";
  }
  return value.toLocaleString("en-US", {
    maximumFractionDigits: maxFractionDigits,
  });
}

export function delta24h(accounts, prices) {
  let nowTotal = 0;
  let prevTotal = 0;
  for (const account of accounts) {
    const assets = Array.isArray(account.assets) && account.assets.length > 0
      ? account.assets
      : [{ symbol: account.symbol, amount: account.amount }];
    for (const asset of assets) {
      const price = prices[asset.symbol];
      if (!price || !asset.amount) {
        continue;
      }
      nowTotal += asset.amount * price.usd;
      prevTotal += (asset.amount * price.usd) / (1 + ((price.change24h || 0) / 100));
    }
  }
  return prevTotal === 0 ? 0 : ((nowTotal - prevTotal) / prevTotal) * 100;
}

export function shortAddress(value) {
  const address = readText(value);
  if (address.length <= 13) {
    return address || "account";
  }
  return `${address.slice(0, 6)}…${address.slice(-4)}`;
}

/** Card-number rhythm for the hero identity card. Hex gets PAN groups; others truncate.
    Privacy mode hides balances, not identity — the PAN display already exposes
    only head/tail, so hex addresses render the same either way. */
export function cardAddressDisplay(value, { privacy = false } = {}) {
  const address = readText(value);
  if (!address) {
    return "—";
  }
  if (address.startsWith("0x") && address.length >= 12) {
    return `${address.slice(0, 6)}  ····  ····  ${address.slice(-4)}`;
  }
  if (privacy && address.length > 10) {
    return `${address.slice(0, 4)}  ····  ${address.slice(-4)}`;
  }
  return shortAddress(address);
}

/** Bottom-right card lockup: connector primary, short chain cue secondary. */
export function cardNetworkLabel(account) {
  const method = methodForAccount(account);
  const connector =
    method === "passkey"
      ? "Built-in"
      : method === "btc"
        ? "UniSat"
        : METHOD_LABELS[method] || "Wallet";
  const namespace = readText(account?.chain_namespace);
  let chain = "";
  if (namespace.startsWith("eip155:")) {
    chain = "EVM";
  } else if (namespace.startsWith("bip122:")) {
    chain = "BTC";
  } else if (namespace) {
    chain = chainLabel(namespace);
  }
  return chain ? `${connector} · ${chain}` : connector;
}

export function relativeTime(value) {
  const timestamp = Number(value || 0);
  if (!Number.isFinite(timestamp) || timestamp <= 0) {
    return "";
  }
  const diffSeconds = Math.round(timestamp - Date.now() / 1000);
  const absSeconds = Math.abs(diffSeconds);
  const suffix = diffSeconds < 0 ? "ago" : "";
  const prefix = diffSeconds >= 0 ? "in " : "";
  if (absSeconds < 60) {
    return diffSeconds < 0 ? "just now" : "in <1m";
  }
  const units = [
    [86400, "d"],
    [3600, "h"],
    [60, "m"],
  ];
  const [unitSeconds, unitLabel] = units.find(([seconds]) => absSeconds >= seconds) || units[2];
  const amount = Math.floor(absSeconds / unitSeconds);
  return `${prefix}${amount}${unitLabel}${suffix ? ` ${suffix}` : ""}`;
}

export function requestTiming(request) {
  const parts = [];
  const created = relativeTime(request?.created_at);
  const expires = relativeTime(request?.expires_at);
  if (created) {
    parts.push(`requested ${created}`);
  }
  if (expires && readText(request?.status) === "pending") {
    parts.push(`expires ${expires}`);
  }
  return parts.join(" · ");
}

export function requestTitle(request) {
  if (request.intent === "browser_personal_sign") {
    return "Browser signature";
  }
  if (request.intent === "browser_typed_data_sign") {
    return "Browser typed data";
  }
  if (isManagedRequest(request)) {
    return "Built-in approval";
  }
  if (isBitcoinProofRequest(request)) {
    return "Bitcoin proof";
  }
  if (request.connector_id === "wallet-metamask") {
    return "MetaMask approval";
  }
  if (request.connector_id === "wallet-unisat") {
    return "Bitcoin approval";
  }
  return "Wallet approval";
}

export function isPasskeyManagedAccount(account) {
  return account.proof_type === "managed_evm" || account.proof_type === "managed_btc_p2wpkh";
}

export function isManagedRequest(request) {
  return request.proof_type === "managed_evm" || request.proof_type === "managed_btc_p2wpkh";
}

export function isBitcoinProofRequest(request) {
  const connectorId = readText(request.connector_id);
  const proofType = readText(request.proof_type);
  return (connectorId === "wallet" || connectorId === "wallet-unisat")
    && request.intent === "bitcoin_bip322_proof"
    && (proofType === "bip322_simple" || proofType === "bitcoin_signed_message");
}

export function nextAccountName(chainNamespace, existingNames = []) {
  const baseName = DEFAULT_ACCOUNT_NAMES[chainNamespace] || `${chainLabel(chainNamespace)} Account`;
  const existing = new Set(existingNames);
  if (!existing.has(baseName)) {
    return baseName;
  }
  let index = 2;
  while (existing.has(`${baseName} ${index}`)) {
    index += 1;
  }
  return `${baseName} ${index}`;
}

export function defaultIntentForAccount(account) {
  return account.chain_namespace === "bip122:000000000019d6689c085ae165831e93"
    ? "bitcoin_bip322_proof"
    : "transaction_intent";
}

export function defaultIntentLabel(account) {
  return defaultIntentForAccount(account) === "bitcoin_bip322_proof"
    ? "Bitcoin signing"
    : "transactions";
}

export function balanceRowForAccount(account, rows) {
  return rows.find((row) => {
    const rowAccount = row.account || {};
    return (account.account_id && rowAccount.account_id === account.account_id)
      || (account.address && rowAccount.address === account.address);
  });
}

export function parseBalanceValue(raw) {
  if (typeof raw === "string" && raw.startsWith("0x")) {
    return BigInt(raw);
  }
  if (typeof raw === "string" && /^[0-9]+$/.test(raw)) {
    return BigInt(raw);
  }
  if (typeof raw === "number" && Number.isFinite(raw)) {
    return BigInt(Math.trunc(raw));
  }
  throw new Error("balance response is missing");
}

export function unitsToNumber(value, decimals) {
  const whole = value / (10n ** BigInt(decimals));
  const fraction = value % (10n ** BigInt(decimals));
  const raw = fraction.toString().padStart(decimals, "0").slice(0, 12).replace(/0+$/, "");
  return Number(raw ? `${whole}.${raw}` : whole.toString());
}

export function validateAddress(address, namespace) {
  if (namespace.startsWith("eip155:")) {
    return /^0x[a-fA-F0-9]{40}$/.test(address);
  }
  if (namespace.startsWith("bip122:")) {
    return /^(bc1|[13])[a-zA-HJ-NP-Z0-9]{20,90}$/.test(address);
  }
  return address.length > 3;
}

export function assetColor(symbol) {
  if (symbol === "BTC") return "var(--btc)";
  if (symbol === "ETH") return "var(--eth)";
  if (symbol === "ELA") return "var(--ela)";
  if (symbol === "USDC") return "var(--usdc)";
  return "var(--ink-soft)";
}

export function cssEscape(value) {
  if (window.CSS && typeof window.CSS.escape === "function") {
    return window.CSS.escape(value);
  }
  return String(value).replace(/["\\]/g, "\\$&");
}
