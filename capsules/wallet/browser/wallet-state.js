import {
  BALANCE_NETWORKS,
  parseBalanceValue,
  readText,
  unitsToNumber,
} from "./wallet-format.js?v=wallet-20260718a";

export function createWalletStateLoader({ fetchJson, shellHeaders }) {
  async function loadPrices() {
    try {
      const payload = await fetchJson("/api/wallet/prices", { headers: shellHeaders() });
      return {
        stale: Boolean(payload.stale),
        prices: payload.prices || {},
        unavailable: Boolean(payload.unavailable),
      };
    } catch (_) {
      return { stale: true, prices: {}, unavailable: true };
    }
  }

  async function loadBalanceRows(accounts) {
    const balanceTargets = balanceTargetsForAccounts(accounts);
    if (balanceTargets.length === 0) {
      return [];
    }
    return Promise.all(balanceTargets.map(loadAccountBalance));
  }

  function balanceTargetsForAccounts(accounts) {
    const targets = new Map();
    for (const account of accounts) {
      const address = readText(account.address);
      if (!address) {
        continue;
      }
      const namespaces = account.chain_namespace?.startsWith("eip155:")
        ? Object.keys(BALANCE_NETWORKS).filter((namespace) => namespace.startsWith("eip155:"))
        : [account.chain_namespace];
      for (const namespace of namespaces) {
        if (!BALANCE_NETWORKS[namespace]) {
          continue;
        }
        const key = `${namespace}:${address.toLowerCase()}`;
        if (!targets.has(key)) {
          targets.set(key, { ...account, chain_namespace: namespace });
        }
      }
    }
    return [...targets.values()];
  }

  async function loadAccountBalance(account) {
    const config = BALANCE_NETWORKS[account.chain_namespace];
    if (!config) {
      return {
        account,
        symbol: "",
        decimals: 0,
        amount: 0,
        rawValue: 0n,
        available: false,
        note: "No balance source for this network yet",
      };
    }
    try {
      const payload = await fetchJson("/api/provider/chain/balance", {
        method: "POST",
        headers: shellHeaders({ "content-type": "application/json" }),
        body: JSON.stringify({
          network: config.network,
          address: account.address,
        }),
      });
      if (payload.status !== "ok") {
        throw new Error(readText(payload.message) || "balance unavailable");
      }
      const data = payload.data || {};
      const rawValue = parseBalanceValue(data.balance_hex ?? data.balance_sats);
      return {
        account,
        balance_key: balanceKey(account.chain_namespace, account.address),
        symbol: config.symbol,
        decimals: config.decimals,
        amount: unitsToNumber(rawValue, config.decimals),
        rawValue,
        available: true,
        note: "",
      };
    } catch (error) {
      return {
        account,
        balance_key: balanceKey(account.chain_namespace, account.address),
        symbol: config.symbol,
        decimals: config.decimals,
        amount: 0,
        rawValue: 0n,
        available: false,
        note: String(error.message || error),
      };
    }
  }

  function balanceKey(namespace, address) {
    return `${readText(namespace)}:${readText(address).toLowerCase()}`;
  }

  return {
    loadBalanceRows,
    loadPrices,
  };
}
