#!/usr/bin/env node
import crypto from "node:crypto";
import fs from "node:fs";
import http from "node:http";
import net from "node:net";
import process from "node:process";
import tls from "node:tls";
import { spawn } from "node:child_process";
import { URL } from "node:url";

const CONFIG_ENV = "ELASTOS_BROWSER_SELKIES_CONTROL_CONFIG";

function fail(message) {
  console.error(message);
  process.exit(1);
}

function safeId(value) {
  return typeof value === "string" && /^[A-Za-z0-9:_-]+$/.test(value);
}

function readConfig() {
  const raw = process.env[CONFIG_ENV];
  if (!raw) {
    fail(`${CONFIG_ENV} is required`);
  }
  let config;
  try {
    config = JSON.parse(raw);
  } catch (error) {
    fail(`${CONFIG_ENV} is invalid JSON: ${error.message}`);
  }
  if (config.schema !== "elastos.browser.selkies-control.config/v1") {
    fail("unsupported Selkies control config schema");
  }
  if (typeof config.control_socket_path !== "string" || !config.control_socket_path.startsWith("/") || /[\s\0]/.test(config.control_socket_path)) {
    fail("control_socket_path must be an absolute Unix socket path without whitespace");
  }
  const wsUrl = new URL(config.selkies_ws_url || "");
  if (!["ws:", "wss:"].includes(wsUrl.protocol)) {
    fail("selkies_ws_url must use ws or wss");
  }
  const browserControl = readBrowserControlConfig(config.browser_control);
  const basicAuth = readBasicAuthConfig(config.basic_auth);
  const iceServers = readIceServersConfig(config.ice_servers);
  const displaySurface = readDisplaySurfaceConfig(config.display_surface);
  const targetContainerName = readTargetContainerName(config.target_container_name);
  return {
    schema: config.schema,
    controlSocketPath: config.control_socket_path,
    replaceExistingSocket: config.replace_existing_socket === true,
    selkiesWsUrl: wsUrl,
    browserControl,
    adapterId: config.adapter_id || "hosted-product",
    connectTimeoutMs: numberOr(config.connect_timeout_ms, 10_000),
    signalTimeoutMs: numberOr(config.signal_timeout_ms, 10_000),
    sessionCooldownMs: numberOr(config.session_cooldown_ms, 1_500),
    basicAuth,
    iceServers,
    displaySurface,
    targetContainerName,
  };
}

function readTargetContainerName(value) {
  if (value == null || value === "") {
    return "";
  }
  if (typeof value !== "string" || !/^elastos-selkies-runtime-exit-target-[0-9]+$/.test(value)) {
    fail("target_container_name must be an ElastOS Selkies target container name");
  }
  return value;
}

function readBrowserControlConfig(value) {
  if (!value || value.kind !== "cdp_http") {
    fail("browser_control.kind=cdp_http is required");
  }
  const endpoint = new URL(value.endpoint || "");
  if (!["http:", "https:"].includes(endpoint.protocol)) {
    fail("browser_control.endpoint must use http or https");
  }
  if (!["127.0.0.1", "::1", "localhost"].includes(endpoint.hostname)) {
    fail("browser_control.endpoint must be loopback/private to the operator service");
  }
  return {
    kind: "cdp_http",
    endpoint,
    timeoutMs: numberOr(value.timeout_ms, 5_000),
  };
}

function readBasicAuthConfig(value) {
  if (value == null) {
    return null;
  }
  if (typeof value !== "object" || Array.isArray(value)) {
    fail("basic_auth must be an object when provided");
  }
  const user = value.user;
  const password = value.password;
  if (typeof user !== "string" || user.length === 0 || /[\r\n\0]/.test(user)) {
    fail("basic_auth.user must be a non-empty string without control characters");
  }
  if (typeof password !== "string" || password.length === 0 || /[\r\n\0]/.test(password)) {
    fail("basic_auth.password must be a non-empty string without control characters");
  }
  return { user, password };
}

function readIceServersConfig(value) {
  if (value == null) {
    return [];
  }
  if (!Array.isArray(value)) {
    fail("ice_servers must be an array when provided");
  }
  if (value.length > 8) {
    fail("ice_servers may contain at most 8 entries");
  }
  return value.map((entry, index) => readIceServerConfig(entry, index));
}

function readDisplaySurfaceConfig(value) {
  const streamWidth = numberOr(value?.stream_width, 1920);
  const streamHeight = numberOr(value?.stream_height, 1080);
  const cssWidth = numberOr(value?.css_width, Math.max(320, Math.round(streamWidth / 1.5)));
  const cssHeight = numberOr(value?.css_height, Math.max(240, Math.round(streamHeight / 1.5)));
  if (streamWidth < 640 || streamWidth > 3840 || streamHeight < 360 || streamHeight > 2160) {
    fail("display_surface stream size must be within 640x360 and 3840x2160");
  }
  if (cssWidth < 320 || cssWidth > 3840 || cssHeight < 240 || cssHeight > 2160) {
    fail("display_surface CSS viewport must be within 320x240 and 3840x2160");
  }
  return {
    stream: { width: streamWidth, height: streamHeight },
    css: { width: cssWidth, height: cssHeight },
    deviceScaleFactor: streamWidth / cssWidth,
  };
}

function readIceServerConfig(value, index) {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    fail(`ice_servers[${index}] must be an object`);
  }
  const urls = readIceUrls(value.urls, index);
  const server = { urls };
  if (value.username != null) {
    if (typeof value.username !== "string" || value.username.length === 0 || /[\r\n\0]/.test(value.username)) {
      fail(`ice_servers[${index}].username must be a non-empty string without control characters`);
    }
    server.username = value.username;
  }
  if (value.credential != null) {
    if (typeof value.credential !== "string" || value.credential.length === 0 || /[\r\n\0]/.test(value.credential)) {
      fail(`ice_servers[${index}].credential must be a non-empty string without control characters`);
    }
    server.credential = value.credential;
  }
  return server;
}

function readIceUrls(value, index) {
  const urls = Array.isArray(value) ? value : [value];
  if (urls.length === 0 || urls.length > 8) {
    fail(`ice_servers[${index}].urls must contain 1..8 URLs`);
  }
  return urls.map((url, urlIndex) => {
    if (typeof url !== "string") {
      fail(`ice_servers[${index}].urls[${urlIndex}] must be a string`);
    }
    const trimmed = url.trim();
    if (!/^(stun|turns?):/i.test(trimmed) || /[\r\n\0]/.test(trimmed) || trimmed.length > 512) {
      fail(`ice_servers[${index}].urls[${urlIndex}] must be a stun:, turn:, or turns: URL without control characters`);
    }
    return trimmed;
  });
}

function numberOr(value, defaultValue) {
  return Number.isInteger(value) && value > 0 ? value : defaultValue;
}

function validateLaunchViewport(launch) {
  const viewport = launch?.viewport;
  if (viewport == null) {
    return;
  }
  if (
    !Number.isInteger(viewport.width) ||
    !Number.isInteger(viewport.height) ||
    viewport.width < 320 ||
    viewport.width > 3840 ||
    viewport.height < 240 ||
    viewport.height > 2160
  ) {
    throw new Error("launch viewport must be within 320x240 and 3840x2160");
  }
}

function displaySizeForLaunch(launch, config) {
  validateLaunchViewport(launch);
  if (launch?.viewport) {
    return launch.viewport;
  }
  return config.displaySurface.css;
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function pageIdFor() {
  return `page:selkies-${crypto.randomBytes(8).toString("hex")}`;
}

function normalizeWalletBridge(wallet) {
  const accounts = Array.isArray(wallet?.accounts)
    ? wallet.accounts
        .map((account) => ({
          account_id: String(account?.account_id || ""),
          chain_namespace: String(account?.chain_namespace || ""),
          address: String(account?.address || "").toLowerCase(),
          label: account?.label ? String(account.label) : null,
        }))
        .filter((account) => safeId(account.account_id) && /^eip155:\d+$/.test(account.chain_namespace) && /^0x[0-9a-f]{40}$/.test(account.address))
    : [];
  const defaultChain =
    typeof wallet?.default_chain_namespace === "string" &&
    accounts.some((account) => account.chain_namespace === wallet.default_chain_namespace)
      ? wallet.default_chain_namespace
      : accounts[0]?.chain_namespace || "";
  const defaultAccountId =
    typeof wallet?.default_account_id === "string" &&
    accounts.some((account) => account.account_id === wallet.default_account_id)
      ? wallet.default_account_id
      : accounts[0]?.account_id || "";
  return {
    accounts,
    default_chain_namespace: defaultChain,
    default_account_id: defaultAccountId,
    bridge_url: typeof wallet?.bridge_url === "string" ? wallet.bridge_url : "",
    approval_url: typeof wallet?.approval_url === "string" ? wallet.approval_url : "",
    transaction_url: typeof wallet?.transaction_url === "string" ? wallet.transaction_url : "",
    read_url: typeof wallet?.read_url === "string" ? wallet.read_url : "",
    transaction_broadcast_url:
      typeof wallet?.transaction_broadcast_url === "string" ? wallet.transaction_broadcast_url : "",
    approval_status_url: typeof wallet?.approval_status_url === "string" ? wallet.approval_status_url : "",
    home_token: typeof wallet?.home_token === "string" ? wallet.home_token : "",
  };
}

function chainNamespaceToDecimal(namespace) {
  const [, value] = String(namespace || "").split(":");
  const parsed = Number(value || "");
  return Number.isFinite(parsed) && parsed > 0 ? parsed : null;
}

function chainNamespaceToHex(namespace) {
  const decimal = chainNamespaceToDecimal(namespace);
  return decimal == null ? null : `0x${decimal.toString(16)}`;
}

function walletInitScript(wallet) {
  const current =
    wallet.accounts.find(
      (account) =>
        account.account_id === wallet.default_account_id &&
        account.chain_namespace === wallet.default_chain_namespace,
    ) ||
    wallet.accounts.find((account) => account.account_id === wallet.default_account_id) ||
    wallet.accounts.find((account) => account.chain_namespace === wallet.default_chain_namespace) ||
    wallet.accounts[0] ||
    null;
  const initialState = {
    chainId: chainNamespaceToHex(wallet.default_chain_namespace),
    selectedAddress: current?.address || null,
    accounts: wallet.accounts,
    defaultChainNamespace: wallet.default_chain_namespace,
    defaultAccountId: wallet.default_account_id,
    bridgeUrl: wallet.bridge_url,
    approvalUrl: wallet.approval_url,
    transactionUrl: wallet.transaction_url,
    readUrl: wallet.read_url,
    transactionBroadcastUrl: wallet.transaction_broadcast_url,
    approvalStatusUrl: wallet.approval_status_url,
    homeToken: wallet.home_token,
  };
	  return `
(() => {
		  if (!globalThis.__elastosBrowserNavigationPolicyInstalled) {
		    Object.defineProperty(globalThis, "__elastosBrowserNavigationPolicyInstalled", {
		      value: true,
		      configurable: false,
		      enumerable: false
		    });
		    const resolveElastosBrowserNavigationUrl = (value) => {
		      try {
		        if (value == null || String(value).trim() === "") {
		          return "";
		        }
		        const url = new URL(String(value), window.location.href);
		        return url.protocol === "http:" || url.protocol === "https:" ? url.href : "";
		      } catch {
		        return "";
		      }
		    };
		    const navigateElastosBrowserInPlace = (value) => {
		      const href = resolveElastosBrowserNavigationUrl(value);
		      if (!href) {
		        return false;
		      }
		      window.location.assign(href);
		      return true;
		    };
		    window.open = (url) => {
		      return navigateElastosBrowserInPlace(url) ? window : null;
		    };
		    document.addEventListener("click", (event) => {
		      if (
		        event.defaultPrevented ||
		        event.button !== 0 ||
		        event.metaKey ||
		        event.ctrlKey ||
		        event.shiftKey ||
		        event.altKey
		      ) {
		        return;
		      }
		      const anchor =
		        event.target && typeof event.target.closest === "function"
		          ? event.target.closest("a[target]")
		          : null;
		      if (!anchor || anchor.download) {
		        return;
		      }
		      const target = String(anchor.target || "").toLowerCase();
		      if (target !== "_blank" && target !== "blank") {
		        return;
		      }
		      if (!navigateElastosBrowserInPlace(anchor.href)) {
		        return;
		      }
		      event.preventDefault();
		      event.stopImmediatePropagation();
		    }, true);
		  }
		  const nextState = ${JSON.stringify(initialState)};
		  if (globalThis.ethereum?.isElastOS) {
		    if (typeof globalThis.ethereum.__elastosRefreshWallet === "function") {
		      globalThis.ethereum.__elastosRefreshWallet({ force: true }).catch(() => {
		        if (typeof globalThis.ethereum.__elastosUpdateWallet === "function") {
		          globalThis.ethereum.__elastosUpdateWallet(nextState);
		        }
		      });
		    } else if (typeof globalThis.ethereum.__elastosUpdateWallet === "function") {
		      globalThis.ethereum.__elastosUpdateWallet(nextState);
		    }
		    if (typeof globalThis.ethereum.__elastosAnnounce === "function") {
		      globalThis.ethereum.__elastosAnnounce();
		    }
		    return;
		  }
		  const state = nextState;
		  const runtimeFetch = window.fetch.bind(window);
		  const listeners = new Map();
	  const providerInfo = {
	    uuid: "9a9a76a8-f36e-4a1f-9a3d-3d9d0f4b4e1a",
	    name: "ElastOS Wallet",
	    icon: "data:image/svg+xml,<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 64 64'><rect width='64' height='64' rx='16' fill='%230E0E0C'/><text x='32' y='41' text-anchor='middle' font-family='Arial' font-size='34' font-weight='700' fill='white'>e</text></svg>",
	    rdns: "com.elacitylabs.elastos.wallet"
	  };
	  const emit = (event, payload) => {
	    for (const handler of listeners.get(event) || []) {
	      try { handler(payload); } catch {}
	    }
	  };
  const chainNamespaceToDecimal = (namespace) => {
    const parsed = Number(String(namespace || "").split(":")[1] || "");
    return Number.isFinite(parsed) && parsed > 0 ? parsed : null;
  };
  const chainNamespaceToHex = (namespace) => {
    const decimal = chainNamespaceToDecimal(namespace);
    return decimal == null ? null : "0x" + decimal.toString(16);
  };
	  const accountForChain = (namespace) => state.accounts.find((account) => account.chain_namespace === namespace) || null;
	  const accountForId = (accountId) => state.accounts.find((account) => account.account_id === accountId) || null;
	  const currentAccount = () =>
	    state.accounts.find(
	      (account) =>
	        account.account_id === state.defaultAccountId &&
	        account.chain_namespace === state.defaultChainNamespace
	    ) ||
	    accountForId(state.defaultAccountId) ||
	    accountForChain(state.defaultChainNamespace) ||
	    state.accounts[0] ||
	    null;
	  const currentAccounts = () => {
	    const account = currentAccount();
	    return account ? [account.address] : [];
	  };
	  const runtimeError = (message, code = 4100) => {
	    const error = new Error(message);
	    error.code = code;
	    return error;
	  };
	  const runtimePost = async (url, body) => {
	    if (!url || !state.homeToken) {
	      throw runtimeError("Runtime wallet approval endpoint is unavailable for this Browser session.");
	    }
	    const response = await runtimeFetch(url, {
	      method: "POST",
	      headers: {
	        "content-type": "application/json",
	        "x-elastos-home-token": state.homeToken
	      },
	      body: JSON.stringify(body)
	    });
	    const text = await response.text();
	    let payload = null;
	    if (text) {
	      try { payload = JSON.parse(text); } catch { payload = text; }
	    }
	    if (!response.ok) {
	      throw runtimeError(typeof payload === "string" ? payload : payload?.message || payload?.error || "Runtime wallet request failed.", response.status === 400 ? 4001 : 4100);
	    }
	    return payload;
	  };
	  let walletRefreshInFlight = null;
	  let lastWalletRefreshAt = 0;
	  const refreshWalletState = async (options = {}) => {
	    if (!state.bridgeUrl || !state.homeToken) {
	      return;
	    }
	    const force = options.force === true;
	    if (!force && Date.now() - lastWalletRefreshAt < 1500) {
	      return;
	    }
	    if (walletRefreshInFlight) {
	      return walletRefreshInFlight;
	    }
	    walletRefreshInFlight = (async () => {
	      const response = await runtimeFetch(state.bridgeUrl, {
	        headers: { "x-elastos-home-token": state.homeToken }
	      });
	      const text = await response.text();
	      const payload = text ? JSON.parse(text) : {};
	      if (!response.ok || payload?.schema !== "elastos.browser.wallet-bridge/v1") {
	        throw runtimeError("Runtime wallet bridge refresh failed.");
	      }
	      provider.__elastosUpdateWallet({
	        accounts: Array.isArray(payload.accounts) ? payload.accounts : [],
	        defaultChainNamespace: typeof payload.default_chain_namespace === "string" ? payload.default_chain_namespace : "",
	        defaultAccountId: typeof payload.default_account_id === "string" ? payload.default_account_id : "",
	        bridgeUrl: typeof payload.bridge_url === "string" ? payload.bridge_url : state.bridgeUrl,
	        approvalUrl: typeof payload.approval_url === "string" ? payload.approval_url : state.approvalUrl,
	        transactionUrl: typeof payload.transaction_url === "string" ? payload.transaction_url : state.transactionUrl,
	        readUrl: typeof payload.read_url === "string" ? payload.read_url : state.readUrl,
	        transactionBroadcastUrl: typeof payload.transaction_broadcast_url === "string" ? payload.transaction_broadcast_url : state.transactionBroadcastUrl,
	        approvalStatusUrl: typeof payload.approval_status_url === "string" ? payload.approval_status_url : state.approvalStatusUrl,
	        homeToken: typeof payload.home_token === "string" ? payload.home_token : state.homeToken
	      });
	      lastWalletRefreshAt = Date.now();
	    })().finally(() => {
	      walletRefreshInFlight = null;
	    });
	    return walletRefreshInFlight;
	  };
	  const runtimeRead = async (method, params) => {
	    const account = selectedEvmAccount();
	    const payload = await runtimePost(state.readUrl, {
	      method,
	      params,
	      chain_namespace: account.chain_namespace,
	      address: account.address,
	      page_url: pageUrl(),
	      origin: pageOrigin()
	    });
	    if (payload?.schema !== "elastos.browser.wallet-read-result/v1" || payload.requires_approval !== false) {
	      throw runtimeError("Runtime chain provider returned an invalid Browser wallet read response.");
	    }
	    return payload.result;
	  };
	  const runtimeGetApproval = async (requestId) => {
	    const base = String(state.approvalStatusUrl || "").replace(/\\/+$/, "");
	    if (!base || !state.homeToken) {
	      throw runtimeError("Runtime wallet approval status endpoint is unavailable for this Browser session.");
	    }
	    const response = await runtimeFetch(base + "/" + encodeURIComponent(requestId), {
	      headers: { "x-elastos-home-token": state.homeToken }
	    });
	    const text = await response.text();
	    let payload = null;
	    if (text) {
	      try { payload = JSON.parse(text); } catch { payload = text; }
	    }
	    if (!response.ok) {
	      throw runtimeError(typeof payload === "string" ? payload : payload?.message || payload?.error || "Runtime wallet approval status failed.");
	    }
	    return payload;
	  };
	  const waitForApproval = async (requestId, { transaction = false } = {}) => {
	    const deadline = Date.now() + 5 * 60 * 1000;
	    while (Date.now() < deadline) {
	      const status = await runtimeGetApproval(requestId);
	      if (status?.status === "completed") {
	        if (transaction) {
	          if (status.transaction_hash) return status.transaction_hash;
	          const broadcast = await runtimePost(state.transactionBroadcastUrl, { request_id: requestId });
	          return broadcast.transaction_hash || status.transaction_hash;
	        }
	        if (status.signature) return status.signature;
	        throw runtimeError("Runtime wallet approval completed without a signature.");
	      }
	      if (status?.status === "rejected" || status?.status === "expired") {
	        throw runtimeError("Runtime wallet approval was " + status.status + ".", 4001);
	      }
	      await new Promise((resolve) => setTimeout(resolve, 1200));
	    }
	    throw runtimeError("Runtime wallet approval timed out.", 4001);
	  };
	  const pageUrl = () => {
	    try { return window.location.href; } catch { return ""; }
	  };
	  const pageOrigin = () => {
	    try { return window.location.origin; } catch { return null; }
	  };
	  const selectedEvmAccount = () => {
	    const account = currentAccount();
	    if (!account) {
	      throw runtimeError("No ElastOS Wallet EVM account is available for this Runtime principal. Open Wallet to create or link an EVM account first.");
	    }
	    return account;
	  };
	  const normalizePersonalSignParams = (params) => {
	    const account = selectedEvmAccount();
	    const first = typeof params[0] === "string" ? params[0] : "";
	    const second = typeof params[1] === "string" ? params[1] : "";
	    if (first.toLowerCase() === account.address.toLowerCase() && second) {
	      return [second, account.address];
	    }
	    return [first, second || account.address];
	  };
	  const applyChain = (account) => {
	    state.defaultChainNamespace = account.chain_namespace;
	    state.chainId = chainNamespaceToHex(account.chain_namespace);
	    state.selectedAddress = account.address;
	    provider.chainId = state.chainId;
	    provider.networkVersion = String(chainNamespaceToDecimal(account.chain_namespace) || "");
	    provider.selectedAddress = account.address;
	    emit("chainChanged", provider.chainId);
	    emit("accountsChanged", [account.address]);
	  };
	  const switchToChainId = (chainIdValue) => {
	    const chainId = String(chainIdValue || "").toLowerCase();
	    if (!/^0x[0-9a-f]+$/.test(chainId)) {
	      const error = new Error("Wallet network switch requires a hex EIP-155 chain id.");
	      error.code = 4902;
	      throw error;
	    }
	    const next =
	      state.accounts.find(
	        (account) =>
	          account.account_id === state.defaultAccountId &&
	          chainNamespaceToHex(account.chain_namespace) === chainId
	      ) ||
	      state.accounts.find((account) => chainNamespaceToHex(account.chain_namespace) === chainId);
	    if (!next) {
	      const decimal = parseInt(chainId, 16);
	      const error = new Error("No ElastOS Wallet account is available for eip155:" + decimal + ". Open Wallet to create or link this network first.");
	      error.code = 4902;
	      throw error;
	    }
	    applyChain(next);
	    return null;
	  };
	  const forceRefreshIfNoAccounts = async () => {
	    if (currentAccounts().length === 0) {
	      await refreshWalletState({ force: true }).catch(() => {});
	    }
	    return currentAccounts();
	  };
	  const request = async (payload = {}) => {
	    const method = payload && payload.method;
	    const params = Array.isArray(payload && payload.params) ? payload.params : [];
	    await refreshWalletState().catch(() => {});
	    if (method === "eth_accounts") {
	      const accounts = await forceRefreshIfNoAccounts();
	      provider.selectedAddress = accounts[0] || null;
	      return accounts;
	    }
	    if (method === "eth_requestAccounts") {
	      const accounts = await forceRefreshIfNoAccounts();
	      if (accounts.length === 0) {
	        const error = new Error("No ElastOS Wallet EVM account is available for this Runtime principal. Open Wallet to create or link an EVM account first.");
	        error.code = 4100;
	        throw error;
	      }
	      provider.selectedAddress = accounts[0];
	      emit("accountsChanged", accounts);
	      emit("connect", { chainId: provider.chainId });
	      return accounts;
	    }
	    if (method === "eth_coinbase") return currentAccounts()[0] || null;
	    if (method === "eth_chainId") {
	      if (!provider.chainId) {
	        const error = new Error("No ElastOS Wallet EVM chain is selected for this Runtime principal.");
	        error.code = 4900;
	        throw error;
	      }
	      return provider.chainId;
	    }
	    if (method === "net_version") return provider.chainId ? String(parseInt(provider.chainId, 16)) : "";
	    if (method === "wallet_getPermissions" || method === "wallet_requestPermissions") {
	      return currentAccounts().length > 0 ? [{ parentCapability: "eth_accounts", caveats: [] }] : [];
	    }
		    if (method === "wallet_switchEthereumChain") {
		      return switchToChainId(params[0] && params[0].chainId);
	    }
	    if (method === "wallet_addEthereumChain") {
	      return switchToChainId(params[0] && params[0].chainId);
	    }
	    if (
	      method === "eth_blockNumber" ||
	      method === "eth_getBalance" ||
	      method === "eth_call" ||
	      method === "eth_estimateGas" ||
	      method === "eth_getTransactionCount" ||
	      method === "eth_gasPrice" ||
	      method === "eth_feeHistory" ||
	      method === "eth_getCode" ||
	      method === "eth_getLogs" ||
	      method === "eth_getTransactionByHash" ||
	      method === "eth_getTransactionReceipt"
	    ) {
	      return runtimeRead(method, params);
	    }
	    if (method === "personal_sign" || method === "eth_sign") {
	      const account = selectedEvmAccount();
	      const approval = await runtimePost(state.approvalUrl, {
	        method: "personal_sign",
	        params: normalizePersonalSignParams(params),
	        account_id: account.account_id,
	        chain_namespace: account.chain_namespace,
	        address: account.address,
	        page_url: pageUrl(),
	        origin: pageOrigin()
	      });
	      const requestId = approval?.approval_request?.request_id;
	      if (!requestId) {
	        throw runtimeError("Runtime wallet approval request was not created.");
	      }
	      return waitForApproval(requestId);
	    }
	    if (method === "eth_signTypedData" || method === "eth_signTypedData_v3" || method === "eth_signTypedData_v4") {
	      const account = selectedEvmAccount();
	      const first = params[0];
	      const second = params[1];
	      const firstIsAccount = typeof first === "string" && first.toLowerCase() === account.address.toLowerCase();
	      const secondIsAccount = typeof second === "string" && second.toLowerCase() === account.address.toLowerCase();
	      const normalizedParams = firstIsAccount
	        ? [account.address, second]
	        : secondIsAccount
	          ? [account.address, first]
	          : params;
	      const approval = await runtimePost(state.approvalUrl, {
	        method,
	        params: normalizedParams,
	        account_id: account.account_id,
	        chain_namespace: account.chain_namespace,
	        address: account.address,
	        page_url: pageUrl(),
	        origin: pageOrigin()
	      });
	      const requestId = approval?.approval_request?.request_id;
	      if (!requestId) {
	        throw runtimeError("Runtime wallet typed-data approval request was not created.");
	      }
	      return waitForApproval(requestId);
	    }
	    if (method === "eth_sendTransaction") {
	      const account = selectedEvmAccount();
	      const approval = await runtimePost(state.transactionUrl, {
	        method,
	        params,
	        account_id: account.account_id,
	        chain_namespace: account.chain_namespace,
	        address: account.address,
	        page_url: pageUrl(),
	        origin: pageOrigin()
	      });
	      const requestId = approval?.approval_request?.request_id;
	      if (!requestId) {
	        throw runtimeError("Runtime transaction approval request was not created.");
	      }
	      return waitForApproval(requestId, { transaction: true });
	    }
    const error = new Error(method + " requires Runtime Wallet/Inbox approval and is not exposed by this hosted Browser adapter yet.");
    error.code = 4100;
    throw error;
	  };
		  const provider = {
	    isElastOS: true,
	    isMetaMask: true,
	    isConnected: () => Boolean(provider.chainId),
	    request,
    selectedAddress: state.selectedAddress,
    chainId: state.chainId,
    networkVersion: state.chainId ? String(parseInt(state.chainId, 16)) : "",
    on(event, handler) {
      if (typeof handler !== "function") return this;
      const handlers = listeners.get(event) || [];
      handlers.push(handler);
      listeners.set(event, handlers);
      return this;
    },
    removeListener(event, handler) {
      const handlers = listeners.get(event) || [];
      listeners.set(event, handlers.filter((item) => item !== handler));
      return this;
	    },
	    enable: async () => request({ method: "eth_requestAccounts" }),
	    _metamask: { isUnlocked: async () => true }
	  };
	  provider.__elastosUpdateWallet = (next) => {
	    state.accounts = Array.isArray(next?.accounts) ? next.accounts : [];
	    state.defaultAccountId = typeof next?.defaultAccountId === "string" ? next.defaultAccountId : state.defaultAccountId;
	    state.bridgeUrl = typeof next?.bridgeUrl === "string" ? next.bridgeUrl : state.bridgeUrl;
	    state.approvalUrl = typeof next?.approvalUrl === "string" ? next.approvalUrl : state.approvalUrl;
	    state.transactionUrl = typeof next?.transactionUrl === "string" ? next.transactionUrl : state.transactionUrl;
	    state.readUrl = typeof next?.readUrl === "string" ? next.readUrl : state.readUrl;
	    state.transactionBroadcastUrl = typeof next?.transactionBroadcastUrl === "string" ? next.transactionBroadcastUrl : state.transactionBroadcastUrl;
	    state.approvalStatusUrl = typeof next?.approvalStatusUrl === "string" ? next.approvalStatusUrl : state.approvalStatusUrl;
	    state.homeToken = typeof next?.homeToken === "string" ? next.homeToken : state.homeToken;
	    const preferred = typeof next?.defaultChainNamespace === "string"
	      ? state.accounts.find((account) => account.account_id === state.defaultAccountId && account.chain_namespace === next.defaultChainNamespace) || accountForChain(next.defaultChainNamespace)
	      : null;
	    const retainedOrFirst = currentAccount();
	    const account = preferred || retainedOrFirst;
	    if (account) {
	      applyChain(account);
	    } else {
	      state.defaultChainNamespace = typeof next?.defaultChainNamespace === "string" ? next.defaultChainNamespace : "";
	      state.chainId = chainNamespaceToHex(state.defaultChainNamespace);
	      state.selectedAddress = null;
	      provider.chainId = state.chainId;
	      provider.networkVersion = state.chainId ? String(chainNamespaceToDecimal(state.defaultChainNamespace) || "") : "";
	      provider.selectedAddress = null;
	      emit("accountsChanged", []);
	    }
	  };
	  provider.__elastosRefreshWallet = refreshWalletState;
	  window.setInterval(() => {
	    refreshWalletState().catch(() => {});
	  }, 3000);
	  provider.providers = [provider];
	  provider.send = (methodOrPayload, paramsOrCallback) => {
	    if (typeof methodOrPayload === "string") {
	      return request({ method: methodOrPayload, params: Array.isArray(paramsOrCallback) ? paramsOrCallback : [] });
	    }
	    const payload = methodOrPayload || {};
	    const callback = typeof paramsOrCallback === "function" ? paramsOrCallback : null;
	    const promise = request(payload);
	    if (callback) {
	      promise.then((result) => callback(null, { id: payload.id, jsonrpc: "2.0", result })).catch((error) => callback(error));
	    }
	    return promise;
	  };
	  provider.sendAsync = (payload, callback) => {
	    request(payload || {}).then((result) => callback(null, { id: payload?.id, jsonrpc: "2.0", result })).catch((error) => callback(error));
	  };
		  Object.defineProperty(window, "ethereum", {
		    value: provider,
		    configurable: false,
	    enumerable: false,
	    writable: false
	  });
		  const announceProvider = () => {
		    window.dispatchEvent(new CustomEvent("eip6963:announceProvider", {
		      detail: { info: providerInfo, provider }
		    }));
		  };
		  provider.__elastosAnnounce = announceProvider;
	  window.addEventListener("eip6963:requestProvider", announceProvider);
	  const announceProviderWhenWalletReady = () => {
	    refreshWalletState({ force: true }).catch(() => {}).finally(() => {
	      announceProvider();
	      window.dispatchEvent(new Event("ethereum#initialized"));
	    });
	  };
	  queueMicrotask(announceProviderWhenWalletReady);
	})();
`;
}

function httpJson(res, status, body) {
  const data = Buffer.from(JSON.stringify(body));
  res.writeHead(status, {
    "content-type": "application/json",
    "content-length": data.length,
  });
  res.end(data);
}

function stopTargetContainer(config) {
  if (!config.targetContainerName) {
    return;
  }
  const child = spawn("docker", ["rm", "-f", config.targetContainerName], {
    detached: true,
    stdio: "ignore",
  });
  child.unref();
}

function readJsonRequest(req) {
  return new Promise((resolve, reject) => {
    const chunks = [];
    req.on("data", (chunk) => chunks.push(chunk));
    req.on("end", () => {
      try {
        const raw = Buffer.concat(chunks).toString("utf8");
        resolve(raw ? JSON.parse(raw) : {});
      } catch (error) {
        reject(new Error(`invalid JSON request: ${error.message}`));
      }
    });
    req.on("error", reject);
  });
}

class MinimalWebSocketClient {
  constructor(url, { basicAuth } = {}) {
    this.url = url;
    this.basicAuth = basicAuth;
    this.socket = null;
    this.buffer = Buffer.alloc(0);
    this.textHandler = () => {};
    this.closeHandler = () => {};
  }

  async connect(timeoutMs) {
    const port = Number(this.url.port || (this.url.protocol === "wss:" ? 443 : 80));
    const host = this.url.hostname;
    const path = `${this.url.pathname || "/"}${this.url.search || ""}`;
    this.socket = await new Promise((resolve, reject) => {
      const connect = this.url.protocol === "wss:" ? tls.connect : net.connect;
      const socket = connect({ host, port, servername: host });
      const timer = setTimeout(() => {
        socket.destroy(new Error("Selkies WebSocket connect timed out"));
      }, timeoutMs);
      socket.once("connect", () => {
        clearTimeout(timer);
        resolve(socket);
      });
      socket.once("error", reject);
    });

    const key = crypto.randomBytes(16).toString("base64");
    const headers = [
      `GET ${path} HTTP/1.1`,
      `Host: ${host}:${port}`,
      "Upgrade: websocket",
      "Connection: Upgrade",
      `Sec-WebSocket-Key: ${key}`,
      "Sec-WebSocket-Version: 13",
    ];
    if (this.basicAuth?.user && this.basicAuth?.password) {
      const value = Buffer.from(`${this.basicAuth.user}:${this.basicAuth.password}`).toString("base64");
      headers.push(`Authorization: Basic ${value}`);
    }
    this.socket.write(`${headers.join("\r\n")}\r\n\r\n`);
    await this.readHandshake(timeoutMs);
    this.socket.on("data", (chunk) => this.handleData(chunk));
    this.socket.on("close", () => this.closeHandler());
  }

  readHandshake(timeoutMs) {
    return new Promise((resolve, reject) => {
      let data = Buffer.alloc(0);
      const timer = setTimeout(() => {
        cleanup();
        reject(new Error("Selkies WebSocket handshake timed out"));
      }, timeoutMs);
      const cleanup = () => {
        clearTimeout(timer);
        this.socket.off("data", onData);
        this.socket.off("error", onError);
      };
      const onError = (error) => {
        cleanup();
        reject(error);
      };
      const onData = (chunk) => {
        data = Buffer.concat([data, chunk]);
        const end = data.indexOf("\r\n\r\n");
        if (end < 0) {
          return;
        }
        const head = data.subarray(0, end).toString("utf8");
        if (!head.startsWith("HTTP/1.1 101") && !head.startsWith("HTTP/1.0 101")) {
          cleanup();
          reject(new Error(`Selkies WebSocket handshake failed: ${head.split("\r\n")[0]}`));
          return;
        }
        this.buffer = data.subarray(end + 4);
        cleanup();
        resolve();
      };
      this.socket.on("data", onData);
      this.socket.on("error", onError);
    });
  }

  onText(handler) {
    this.textHandler = handler;
  }

  onClose(handler) {
    this.closeHandler = handler;
  }

  sendText(text) {
    this.sendFrame(0x1, Buffer.from(text));
  }

  sendFrame(opcode, payload) {
    if (!this.socket || this.socket.destroyed) {
      throw new Error("Selkies WebSocket is closed");
    }
    const header = [];
    header.push(0x80 | opcode);
    if (payload.length < 126) {
      header.push(0x80 | payload.length);
    } else if (payload.length <= 0xffff) {
      header.push(0x80 | 126, (payload.length >> 8) & 0xff, payload.length & 0xff);
    } else {
      throw new Error("Selkies WebSocket message is too large");
    }
    const mask = crypto.randomBytes(4);
    const masked = Buffer.alloc(payload.length);
    for (let index = 0; index < payload.length; index += 1) {
      masked[index] = payload[index] ^ mask[index % 4];
    }
    this.socket.write(Buffer.concat([Buffer.from(header), mask, masked]));
  }

  close() {
    if (!this.socket || this.socket.destroyed) {
      return;
    }
    try {
      this.sendFrame(0x8, Buffer.alloc(0));
    } catch {
      // The socket may already be closing; TCP teardown below is still required.
    }
    this.socket.end();
  }

  handleData(chunk) {
    this.buffer = Buffer.concat([this.buffer, chunk]);
    for (;;) {
      const frame = readFrame(this.buffer);
      if (!frame) {
        return;
      }
      this.buffer = this.buffer.subarray(frame.consumed);
      if (frame.opcode === 0x1) {
      this.textHandler(frame.payload.toString("utf8"));
    } else if (frame.opcode === 0x8) {
      this.close();
      return;
    } else if (frame.opcode === 0x9) {
      this.sendFrame(0xa, frame.payload);
    }
  }
}
}

function readFrame(buffer) {
  if (buffer.length < 2) {
    return null;
  }
  const opcode = buffer[0] & 0x0f;
  const masked = (buffer[1] & 0x80) !== 0;
  let length = buffer[1] & 0x7f;
  let offset = 2;
  if (length === 126) {
    if (buffer.length < offset + 2) return null;
    length = buffer.readUInt16BE(offset);
    offset += 2;
  } else if (length === 127) {
    throw new Error("Selkies WebSocket frame is too large");
  }
  let mask;
  if (masked) {
    if (buffer.length < offset + 4) return null;
    mask = buffer.subarray(offset, offset + 4);
    offset += 4;
  }
  if (buffer.length < offset + length) {
    return null;
  }
  const payload = Buffer.from(buffer.subarray(offset, offset + length));
  if (mask) {
    for (let index = 0; index < payload.length; index += 1) {
      payload[index] ^= mask[index % 4];
    }
  }
  return { opcode, payload, consumed: offset + length };
}

class SelkiesPage {
  constructor(config, launchRequest, onClosed = () => {}) {
    this.config = config;
    this.launchRequest = launchRequest;
    this.pageId = pageIdFor(launchRequest.url, launchRequest.stream_id);
    this.onClosed = onClosed;
    this.ws = new MinimalWebSocketClient(config.selkiesWsUrl, { basicAuth: config.basicAuth });
    this.serverPeerId = null;
    this.messages = [];
    this.waiters = [];
    this.remoteCandidates = [];
    this.closed = false;
    this.ws.onText((message) => this.handleMessage(message));
    this.ws.onClose(() => {
      this.markClosed();
    });
  }

  async open() {
    const wallet = normalizeWalletBridge(this.launchRequest.wallet || {});
    this.wallet = wallet;
    const displaySize = displaySizeForLaunch(this.launchRequest, this.config);
    const browserPage = await openBrowserPage(this.config, this.launchRequest.url, wallet, this.launchRequest);
    browserPage._stopWalletBridgeWatch = startWalletBridgeWatch(
      browserPage,
      wallet,
      this.config.browserControl.timeoutMs,
    );
    this.browserPage = browserPage;
    await this.ws.connect(this.config.connectTimeoutMs);
    this.ws.sendText("HELLO client " + JSON.stringify({
      client_type: "controller",
      client_slot: 1,
      client_strict_viewer: false,
    }));
    await this.waitFor((message) => message.kind === "hello", "Selkies HELLO");
    this.ws.sendText("SESSION server");
    const session = await this.waitFor((message) => message.kind === "session_ok", "Selkies SESSION_OK");
    this.serverPeerId = session.serverPeerId;
    const offer = await this.waitFor(
      (message) => message.sdp?.type === "offer" && typeof message.sdp.sdp === "string",
      "Selkies SDP offer",
    );
    return this.supervisorResult(offer.sdp.sdp, browserPage, wallet);
  }

  supervisorResult(sdp, browserPage, wallet) {
    const displaySize = displaySizeForLaunch(this.launchRequest, this.config);
    return {
      schema: "elastos.browser.engine.supervisor-result/v1",
      page_id: this.pageId,
      adapter: this.launchRequest.adapter,
      engine: this.launchRequest.engine,
      stream_id: this.launchRequest.stream_id,
      actual_url: browserPage.url || this.launchRequest.url,
      title: browserPage.title || "Selkies Browser",
      network_mode: "runtime_net_only",
      direct_network: false,
      wallet_injection: false,
      wallet_bridge: {
        schema: "elastos.browser.wallet-bridge/v1",
        mode: "runtime_mediated_eip1193",
        accounts: wallet.accounts.length,
        default_chain_namespace: wallet.default_chain_namespace,
        signing: "approval_required",
      },
      view: {
        schema: "elastos.browser.view/v1",
        mode: "webrtc_remote_display",
        width: displaySize.width,
        height: displaySize.height,
      },
      display_session: {
        schema: "elastos.browser.display-session/v1",
        session_id: `display:${this.launchRequest.stream_id}`,
        mode: "webrtc_remote_display",
        width: this.config.displaySurface.stream.width,
        height: this.config.displaySurface.stream.height,
        input: "datachannel",
        input_protocol: "selkies_v1",
        offerer: "engine",
        initial_offer: {
          schema: "elastos.browser.webrtc-offer/v1",
          type: "offer",
          sdp,
        },
        display_backend: "selkies_gstreamer_webrtc",
        backend_class: "product_compositor",
        audio: true,
        video: true,
        ice_servers: this.config.iceServers,
        network_mode: "runtime_net_only",
        direct_network: false,
        signaling_url: `/api/apps/browser/pages/${encodeURIComponent(this.pageId)}/webrtc`,
      },
    };
  }

  handleMessage(raw) {
    const parsed = parseSelkiesMessage(raw);
    if (parsed?.ice) {
      this.remoteCandidates.push(parsed.ice);
    }
    if (parsed) {
      this.messages.push(parsed);
      this.flushWaiters();
    }
  }

  flushWaiters() {
    for (const waiter of [...this.waiters]) {
      const matchIndex = this.messages.findIndex(waiter.predicate);
      if (matchIndex >= 0) {
        const [message] = this.messages.splice(matchIndex, 1);
        this.waiters = this.waiters.filter((entry) => entry !== waiter);
        clearTimeout(waiter.timer);
        waiter.resolve(message);
      } else if (this.closed) {
        this.waiters = this.waiters.filter((entry) => entry !== waiter);
        clearTimeout(waiter.timer);
        waiter.reject(new Error(`Selkies WebSocket closed while waiting for ${waiter.label}`));
      }
    }
  }

  waitFor(predicate, label) {
    const matchIndex = this.messages.findIndex(predicate);
    if (matchIndex >= 0) {
      const [message] = this.messages.splice(matchIndex, 1);
      return Promise.resolve(message);
    }
    return new Promise((resolve, reject) => {
      const waiter = {
        predicate,
        label,
        resolve,
        reject,
        timer: setTimeout(() => {
          this.waiters = this.waiters.filter((entry) => entry !== waiter);
          reject(new Error(`timed out waiting for ${label}`));
        }, this.config.signalTimeoutMs),
      };
      this.waiters.push(waiter);
    });
  }

  signal(signal) {
    if (!this.serverPeerId) {
      throw new Error("Selkies server peer is unavailable");
    }
    if (signal.schema === "elastos.browser.webrtc-answer/v1") {
      this.ws.sendText(`${this.serverPeerId} ${JSON.stringify({ sdp: { type: "answer", sdp: signal.sdp } })}`);
      return this.ack("answer");
    }
    if (signal.schema === "elastos.browser.webrtc-candidate/v1") {
      this.ws.sendText(`${this.serverPeerId} ${JSON.stringify({ ice: signal.candidate })}`);
      return this.ack("candidate");
    }
    if (signal.schema === "elastos.browser.webrtc-end-of-candidates/v1") {
      return this.ack("end_of_candidates");
    }
    throw new Error("unsupported WebRTC signal for Selkies control service");
  }

  ack(type) {
    const candidates = this.remoteCandidates.splice(0);
    return {
      schema: "elastos.browser.webrtc-signal-ack/v1",
      page_id: this.pageId,
      type,
      accepted: true,
      candidates,
      end_of_candidates: false,
    };
  }

  close() {
    this.markClosed();
    this.ws.close();
    if (this.browserPage?.target_id) {
      closeBrowserPage(this.config.browserControl, this.browserPage).catch(() => {});
    }
  }

  markClosed() {
    if (this.closed) {
      return;
    }
    this.closed = true;
    this.onClosed(this);
    this.flushWaiters();
  }
}

class CdpClient {
  constructor(webSocketUrl) {
    this.ws = new MinimalWebSocketClient(new URL(webSocketUrl));
    this.nextId = 1;
    this.pending = new Map();
    this.events = [];
    this.waiters = [];
  }

  async connect(timeoutMs) {
    await this.ws.connect(timeoutMs);
    this.ws.onText((message) => this.handleMessage(message));
    this.ws.onClose(() => {
      for (const pending of this.pending.values()) {
        pending.reject(new Error("browser CDP WebSocket closed"));
      }
      this.pending.clear();
      for (const waiter of this.waiters.splice(0)) {
        clearTimeout(waiter.timer);
        waiter.reject(new Error(`browser CDP WebSocket closed while waiting for ${waiter.label}`));
      }
    });
  }

  request(method, params = {}) {
    const id = this.nextId++;
    this.ws.sendText(JSON.stringify({ id, method, params }));
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
    });
  }

  waitForEvent(method, timeoutMs, label = method) {
    const existingIndex = this.events.findIndex((event) => event.method === method);
    if (existingIndex >= 0) {
      const [event] = this.events.splice(existingIndex, 1);
      return Promise.resolve(event.params || {});
    }
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.waiters = this.waiters.filter((entry) => entry.timer !== timer);
        reject(new Error(`timed out waiting for browser CDP event ${label}`));
      }, timeoutMs);
      this.waiters.push({ method, label, timer, resolve, reject });
    });
  }

  handleMessage(raw) {
    let message;
    try {
      message = JSON.parse(raw);
    } catch {
      return;
    }
    if (Number.isInteger(message.id)) {
      const pending = this.pending.get(message.id);
      if (!pending) {
        return;
      }
      this.pending.delete(message.id);
      if (message.error) {
        pending.reject(new Error(message.error.message || "browser CDP request failed"));
      } else {
        pending.resolve(message.result || {});
      }
      return;
    }
    if (typeof message.method !== "string") {
      return;
    }
    const waiter = this.waiters.find((entry) => entry.method === message.method);
    if (waiter) {
      this.waiters = this.waiters.filter((entry) => entry !== waiter);
      clearTimeout(waiter.timer);
      waiter.resolve(message.params || {});
      return;
    }
    this.events.push(message);
    if (this.events.length > 100) {
      this.events.splice(0, this.events.length - 100);
    }
  }

  close() {
    this.ws.close();
  }
}

async function fetchBrowserControlJson(browserControl, path, { method = "GET" } = {}) {
  if (browserControl.kind !== "cdp_http") {
    throw new Error("unsupported browser control kind");
  }
  const target = new URL(path, browserControl.endpoint);
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), browserControl.timeoutMs);
  let response;
  try {
    response = await fetch(target, { method, signal: controller.signal });
  } catch (error) {
    throw new Error(`browser CDP control ${method} ${target.pathname} failed: ${error instanceof Error ? error.message : String(error)}`);
  } finally {
    clearTimeout(timer);
  }
  if (!response.ok) {
    throw new Error(`browser CDP control ${method} ${target.pathname} failed: HTTP ${response.status}`);
  }
  try {
    return await response.json();
  } catch (error) {
    throw new Error(`browser CDP control ${method} ${target.pathname} returned invalid JSON: ${error instanceof Error ? error.message : String(error)}`);
  }
}

async function fetchBrowserControl(browserControl, path, { method = "GET" } = {}) {
  if (browserControl.kind !== "cdp_http") {
    throw new Error("unsupported browser control kind");
  }
  const target = new URL(path, browserControl.endpoint);
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), browserControl.timeoutMs);
  try {
    await fetch(target, { method, signal: controller.signal });
  } catch {
    // Best-effort CDP browser-control endpoints such as /json/activate are not authority-critical.
  } finally {
    clearTimeout(timer);
  }
}

function usableBrowserTarget(value) {
  return (
    value &&
    value.type === "page" &&
    typeof value.id === "string" &&
    value.id.length > 0 &&
    typeof value.webSocketDebuggerUrl === "string" &&
    value.webSocketDebuggerUrl.length > 0
  );
}

async function browserPageTarget(browserControl, preferredTargetId = "") {
  const targets = await fetchBrowserControlJson(browserControl, "/json/list");
  const pages = Array.isArray(targets) ? targets.filter(usableBrowserTarget) : [];
  const preferred = pages.find((page) => page.id === preferredTargetId);
  if (preferred) {
    return preferred;
  }
  if (pages[0]) {
    return pages[0];
  }
  const created = await fetchBrowserControlJson(browserControl, "/json/new?about:blank", { method: "PUT" });
  if (!usableBrowserTarget(created)) {
    throw new Error("browser CDP navigation did not return a page debugger URL");
  }
  return created;
}

async function activateBrowserTarget(browserControl, targetId) {
  if (targetId && !/[\s\0/]/.test(targetId)) {
    await fetchBrowserControl(browserControl, `/json/activate/${encodeURIComponent(targetId)}`);
  }
}

async function applyBrowserViewport(cdp, launch, config) {
  const displaySize = displaySizeForLaunch(launch, config);
  await cdp.request("Emulation.setDeviceMetricsOverride", {
    width: displaySize.width,
    height: displaySize.height,
    deviceScaleFactor: config.displaySurface.deviceScaleFactor,
    mobile: false,
    screenWidth: displaySize.width,
    screenHeight: displaySize.height,
  });
  return displaySize;
}

async function installWalletBridge(cdp, wallet) {
  const source = walletInitScript(wallet);
  const initScript = await cdp.request("Page.addScriptToEvaluateOnNewDocument", { source });
  const evaluated = await cdp.request("Runtime.evaluate", { expression: source, awaitPromise: false });
  if (evaluated.exceptionDetails) {
    throw new Error("Browser wallet bridge injection failed");
  }
  return typeof initScript.identifier === "string" ? initScript.identifier : "";
}

function startWalletBridgeWatch(browserPage, wallet, timeoutMs) {
  const cdp = browserPage?._cdp;
  if (!cdp) {
    return null;
  }
  const source = walletInitScript(wallet);
  let stopped = false;
  const ensure = () => {
    if (stopped) {
      return;
    }
    cdp
      .request("Runtime.evaluate", { expression: source, awaitPromise: false })
      .catch(() => {
        stopped = true;
      });
  };
  const timer = setInterval(ensure, Math.max(750, Math.min(timeoutMs, 2000)));
  return () => {
    stopped = true;
    clearInterval(timer);
  };
}

async function openBrowserPage(config, url, wallet, launch) {
  const browserControl = config.browserControl;
  const body = await browserPageTarget(browserControl);
  if (typeof body.webSocketDebuggerUrl !== "string" || !body.webSocketDebuggerUrl) {
    throw new Error("browser CDP navigation did not return a page debugger URL");
  }
  await activateBrowserTarget(browserControl, body.id);
  const cdp = new CdpClient(body.webSocketDebuggerUrl);
  let initScriptId = "";
  let keepCdp = false;
  try {
    await cdp.connect(browserControl.timeoutMs);
	    await cdp.request("Page.enable");
	    await cdp.request("Runtime.enable");
	    await applyBrowserViewport(cdp, launch, config);
	    initScriptId = await installWalletBridge(cdp, wallet);
	    await cdp.request("Page.navigate", { url });
    await cdp.waitForEvent("Page.domContentEventFired", browserControl.timeoutMs, "domcontent");
    const current = await cdp.request("Runtime.evaluate", {
      expression: "JSON.stringify({ url: window.location.href, title: document.title })",
      returnByValue: true,
    });
    let page = {};
    try {
      page = JSON.parse(current?.result?.value || "{}");
    } catch {
      page = {};
    }
    keepCdp = true;
    return {
      target_id: typeof body.id === "string" ? body.id : "",
      debugger_url: body.webSocketDebuggerUrl,
	      init_script_id: initScriptId,
	      wallet,
	      url: typeof page.url === "string" ? page.url : url,
      title: typeof page.title === "string" ? page.title : "Selkies Browser",
      _cdp: cdp,
    };
  } finally {
    if (!keepCdp) {
      cdp.close();
    }
  }
}

async function resizeBrowserPage(config, browserPage, event, timeoutMs) {
  if (!browserPage?.debugger_url) {
    throw new Error("browser page debugger URL is unavailable");
  }
  validateLaunchViewport({ viewport: event?.viewport });
  const cdp = new CdpClient(browserPage.debugger_url);
  try {
    await cdp.connect(timeoutMs);
    await cdp.request("Page.enable");
    await cdp.request("Runtime.enable");
    const displaySize = await applyBrowserViewport(cdp, { viewport: event?.viewport }, config);
    return {
      url: browserPage.url || "",
      title: browserPage.title || "Selkies Browser",
      can_go_back: browserPage.can_go_back === true,
      can_go_forward: browserPage.can_go_forward === true,
      width: displaySize.width,
      height: displaySize.height,
    };
  } finally {
    cdp.close();
  }
}

function validatePasteText(value) {
  const text = String(value ?? "");
  if (!text) {
    throw new Error("browser paste_text input requires non-empty text");
  }
  if (text.length > 65536) {
    throw new Error("browser paste_text input is too large");
  }
  return text;
}

async function pasteTextIntoBrowserPage(browserPage, event, timeoutMs) {
  if (!browserPage?.debugger_url) {
    throw new Error("browser page debugger URL is unavailable");
  }
  const text = validatePasteText(event?.text);
  const cdp = new CdpClient(browserPage.debugger_url);
  try {
    await cdp.connect(timeoutMs);
    await cdp.request("Input.insertText", { text });
  } finally {
    cdp.close();
  }
  return refreshBrowserPageState(browserPage, timeoutMs);
}

async function refreshBrowserPageState(browserPage, timeoutMs) {
  if (!browserPage?.debugger_url) {
    return {
      url: browserPage?.url || "",
      title: browserPage?.title || "Selkies Browser",
      can_go_back: false,
      can_go_forward: false,
    };
  }
  const cdp = new CdpClient(browserPage.debugger_url);
  try {
    await cdp.connect(timeoutMs);
    await cdp.request("Page.enable");
    await cdp.request("Runtime.enable");
    const current = await cdp.request("Runtime.evaluate", {
      expression: "JSON.stringify({ url: window.location.href, title: document.title })",
      returnByValue: true,
    });
    const history = await cdp.request("Page.getNavigationHistory").catch(() => ({}));
    let page = {};
    try {
      page = JSON.parse(current?.result?.value || "{}");
    } catch {
      page = {};
    }
    const currentIndex = Number(history.currentIndex ?? -1);
    const entryCount = Array.isArray(history.entries) ? history.entries.length : 0;
    browserPage.url = typeof page.url === "string" ? page.url : browserPage.url;
    browserPage.title = typeof page.title === "string" ? page.title : browserPage.title;
    browserPage.can_go_back = currentIndex > 0;
    browserPage.can_go_forward = currentIndex >= 0 && currentIndex < entryCount - 1;
    return {
      url: browserPage.url,
      title: browserPage.title,
      can_go_back: browserPage.can_go_back,
      can_go_forward: browserPage.can_go_forward,
    };
  } finally {
    cdp.close();
  }
}

function validateBrowserNavigationUrl(value) {
  let parsed;
  try {
    parsed = new URL(String(value || ""));
  } catch {
    throw new Error("browser navigate command requires a valid URL");
  }
  if (!["http:", "https:"].includes(parsed.protocol)) {
    throw new Error("browser navigate command only supports http and https URLs");
  }
  return parsed.href;
}

async function applyBrowserCommand(browserPage, event, timeoutMs) {
  if (!browserPage?.debugger_url) {
    throw new Error("browser page debugger URL is unavailable");
  }
  const command = String(event?.command || "");
  const cdp = new CdpClient(browserPage.debugger_url);
  try {
	    await cdp.connect(timeoutMs);
	    await cdp.request("Page.enable");
	    await cdp.request("Runtime.enable");
	    await installWalletBridge(cdp, normalizeWalletBridge(browserPage.wallet || {}));
	    if (command === "navigate") {
      await cdp.request("Page.navigate", { url: validateBrowserNavigationUrl(event?.url) });
    } else if (command === "reload") {
      await cdp.request("Page.reload", { ignoreCache: false });
    } else if (command === "back" || command === "forward") {
      const history = await cdp.request("Page.getNavigationHistory");
      const currentIndex = Number(history.currentIndex ?? -1);
      const entries = Array.isArray(history.entries) ? history.entries : [];
      const nextIndex = command === "back" ? currentIndex - 1 : currentIndex + 1;
      const entry = entries[nextIndex];
      if (entry?.id != null) {
        await cdp.request("Page.navigateToHistoryEntry", { entryId: entry.id });
      }
    } else {
      throw new Error("unsupported browser command");
    }
    await cdp.waitForEvent("Page.domContentEventFired", Math.min(timeoutMs, 15000), "domcontent").catch(() => {});
  } finally {
    cdp.close();
  }
  return refreshBrowserPageState(browserPage, timeoutMs);
}

async function closeBrowserPage(browserControl, browserPage) {
  if (typeof browserPage?._stopWalletBridgeWatch === "function") {
    browserPage._stopWalletBridgeWatch();
    browserPage._stopWalletBridgeWatch = null;
  }
  if (browserPage?._cdp) {
    try {
      if (browserPage.init_script_id) {
        await browserPage._cdp
          .request("Page.removeScriptToEvaluateOnNewDocument", {
            identifier: browserPage.init_script_id,
          })
          .catch(() => {});
      }
      await browserPage._cdp.request("Page.navigate", { url: "about:blank" }).catch(() => {});
    } finally {
      browserPage._cdp.close();
      browserPage._cdp = null;
    }
    return;
  }
  const targetId = browserPage?.target_id;
  if (!targetId || /[\s\0/]/.test(targetId)) {
    return;
  }
  const body = await browserPageTarget(browserControl, targetId);
  if (typeof body.webSocketDebuggerUrl !== "string" || !body.webSocketDebuggerUrl) {
    return;
  }
  const cdp = new CdpClient(body.webSocketDebuggerUrl);
  try {
    await cdp.connect(browserControl.timeoutMs);
    await cdp.request("Page.enable");
    if (browserPage.init_script_id) {
      await cdp.request("Page.removeScriptToEvaluateOnNewDocument", {
        identifier: browserPage.init_script_id,
      }).catch(() => {});
    }
    await cdp.request("Page.navigate", { url: "about:blank" }).catch(() => {});
  } finally {
    cdp.close();
  }
}

function parseSelkiesMessage(raw) {
  if (raw === "HELLO") {
    return { kind: "hello", raw };
  }
  if (raw.startsWith("SESSION_OK ")) {
    return { kind: "session_ok", raw, serverPeerId: raw.split(/\s+/)[1] };
  }
  if (raw.startsWith("ERROR")) {
    return { kind: "error", raw };
  }
  const separator = raw.indexOf(" ");
  if (separator < 0) {
    return { kind: "unknown", raw };
  }
  const from = raw.slice(0, separator);
  const payload = raw.slice(separator + 1);
  try {
    const message = JSON.parse(payload);
    return { kind: "peer_message", raw, from, ...message };
  } catch {
    return { kind: "unknown", raw };
  }
}

function validateOpenRequest(body) {
  if (body.schema !== "elastos.browser.hosted-product.open/v1") {
    throw new Error("unsupported hosted product open schema");
  }
  const launch = body.launch_request;
  if (!launch || launch.schema !== "elastos.browser.engine.launch-request/v1") {
    throw new Error("missing Browser Engine launch request");
  }
  if (launch.engine !== "selkies_gstreamer") {
    throw new Error("Selkies control service requires engine=selkies_gstreamer");
  }
  if (launch.display_mode !== "webrtc_remote_display") {
    throw new Error("Selkies control service requires webrtc_remote_display");
  }
  if (launch.network_mode !== "runtime_net_only" || launch.direct_network !== false) {
    throw new Error("Selkies control service requires runtime_net_only and direct_network=false");
  }
  if (!safeId(launch.adapter) || !safeId(launch.stream_id)) {
    throw new Error("launch request adapter and stream_id must be safe identifiers");
  }
  return launch;
}

async function main() {
  const config = readConfig();
  if (fs.existsSync(config.controlSocketPath)) {
    if (!config.replaceExistingSocket) {
      fail(`control socket already exists: ${config.controlSocketPath}`);
    }
    fs.unlinkSync(config.controlSocketPath);
  }
  const pages = new Map();
  let lastSessionClosedAt = 0;
  const markSessionClosed = () => {
    lastSessionClosedAt = Date.now();
  };
  const closeActivePages = () => {
    const pageIds = [...pages.keys()];
    for (const page of pages.values()) {
      page.close();
    }
    pages.clear();
    if (pageIds.length > 0) {
      markSessionClosed();
    }
    return pageIds;
  };
  const waitForSessionCooldown = async () => {
    const remaining = config.sessionCooldownMs - (Date.now() - lastSessionClosedAt);
    if (remaining > 0) {
      await sleep(remaining);
    }
  };
  const server = http.createServer(async (req, res) => {
    try {
      const url = new URL(req.url, "http://browser-engine");
      if (req.method === "POST" && url.pathname === "/shutdown") {
        closeActivePages();
        httpJson(res, 200, {
          schema: "elastos.browser.selkies-control.shutdown/v1",
          ok: true,
        });
        stopTargetContainer(config);
        setTimeout(() => {
          server.close(() => {
            try {
              fs.unlinkSync(config.controlSocketPath);
            } catch {}
            process.exit(0);
          });
        }, 25);
        return;
      }
      if (req.method === "GET" && url.pathname === "/status") {
        httpJson(res, 200, {
          schema: "elastos.browser.selkies-control.status/v1",
          display_backend: "selkies_gstreamer_webrtc",
          backend_class: "product_compositor",
          active_pages: pages.size,
          page_ids: [...pages.keys()],
          single_session: true,
          direct_network: false,
        });
        return;
      }
      if (req.method === "POST" && url.pathname === "/pages") {
        const body = await readJsonRequest(req);
        const launch = validateOpenRequest(body);
        if (pages.size > 0) {
          closeActivePages();
        }
        await waitForSessionCooldown();
        const page = new SelkiesPage(config, launch, (closedPage) => {
          if (pages.get(closedPage.pageId) === closedPage) {
            pages.delete(closedPage.pageId);
            markSessionClosed();
          }
        });
        try {
          const result = await page.open();
          pages.set(page.pageId, page);
          httpJson(res, 200, result);
        } catch (error) {
          page.close();
          markSessionClosed();
          throw error;
        }
        return;
      }
      const pageMatch = url.pathname.match(/^\/pages\/([^/]+)\/(webrtc|input|close|status)$/);
      if (!pageMatch) {
        httpJson(res, 404, { error: "not found" });
        return;
      }
      const pageId = decodeURIComponent(pageMatch[1]);
      const op = pageMatch[2];
      const page = pages.get(pageId);
      if (!page) {
        httpJson(res, 404, { error: "browser page not found" });
        return;
      }
      if (req.method === "GET" && op === "status") {
        const browserState = await refreshBrowserPageState(page.browserPage, config.signalTimeoutMs).catch(() => ({}));
        httpJson(res, 200, {
          schema: "elastos.browser.page-status/v1",
          page_id: pageId,
          display_backend: "selkies_gstreamer_webrtc",
          backend_class: "product_compositor",
          input_protocol: "selkies_v1",
          audio: true,
          video: true,
          direct_network: false,
          webrtc_connection_state: page.closed ? "closed" : "signaling",
          actual_url: browserState.url || page.browserPage?.url,
          title: browserState.title || page.browserPage?.title,
          can_go_back: browserState.can_go_back === true,
          can_go_forward: browserState.can_go_forward === true,
          principal_id: page.launchRequest.principal_id || null,
          wallet_bridge: {
            schema: "elastos.browser.wallet-bridge/v1",
            mode: "runtime_mediated_eip1193",
            accounts: page.wallet?.accounts?.length || 0,
            default_chain_namespace: page.wallet?.default_chain_namespace || null,
            signing: "approval_required",
          },
        });
        return;
      }
      const body = await readJsonRequest(req);
      if (req.method === "POST" && op === "webrtc") {
        httpJson(res, 200, page.signal(body.signal));
        return;
      }
      if (req.method === "POST" && op === "input") {
        if (body?.event?.type === "browser_command") {
          const state = await applyBrowserCommand(
            page.browserPage,
            body.event,
            config.signalTimeoutMs,
          );
          httpJson(res, 200, {
            schema: "elastos.browser.input-result/v1",
            page_id: pageId,
            accepted: true,
            actual_url: state.url,
            title: state.title,
            can_go_back: state.can_go_back,
            can_go_forward: state.can_go_forward,
            direct_network: false,
          });
          return;
        }
        if (body?.event?.type === "resize") {
          const state = await resizeBrowserPage(
            config,
            page.browserPage,
            body.event,
            config.signalTimeoutMs,
          );
          httpJson(res, 200, {
            schema: "elastos.browser.input-result/v1",
            page_id: pageId,
            accepted: true,
            actual_url: state.url,
            title: state.title,
            can_go_back: state.can_go_back,
            can_go_forward: state.can_go_forward,
            width: state.width,
            height: state.height,
            direct_network: false,
          });
          return;
        }
        if (body?.event?.type === "paste_text") {
          const state = await pasteTextIntoBrowserPage(
            page.browserPage,
            body.event,
            config.signalTimeoutMs,
          );
          httpJson(res, 200, {
            schema: "elastos.browser.input-result/v1",
            page_id: pageId,
            accepted: true,
            actual_url: state.url,
            title: state.title,
            can_go_back: state.can_go_back,
            can_go_forward: state.can_go_forward,
            direct_network: false,
          });
          return;
        }
        httpJson(res, 200, {
          schema: "elastos.browser.input-result/v1",
          page_id: pageId,
          accepted: false,
          reason: "Selkies input is carried by the WebRTC data channel",
        });
        return;
      }
      if (req.method === "POST" && op === "close") {
        page.close();
        pages.delete(pageId);
        markSessionClosed();
        httpJson(res, 200, { schema: "elastos.browser.close-result/v1", page_id: pageId, closed: true });
        return;
      }
      httpJson(res, 405, { error: "method not allowed" });
    } catch (error) {
      httpJson(res, 500, { error: error instanceof Error ? error.message : String(error) });
    }
  });
  server.listen(config.controlSocketPath, () => {
    console.log(JSON.stringify({
      ok: true,
      schema: "elastos.browser.selkies-control.ready/v1",
      control_socket: config.controlSocketPath,
      selkies_ws_url: config.selkiesWsUrl.toString(),
      display_backend: "selkies_gstreamer_webrtc",
      backend_class: "product_compositor",
      audio: true,
      video: true,
      direct_network: false,
    }));
  });
}

main().catch((error) => fail(error instanceof Error ? error.message : String(error)));
