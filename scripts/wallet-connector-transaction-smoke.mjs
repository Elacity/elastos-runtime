#!/usr/bin/env node
import { pathToFileURL } from "node:url";
import { resolve } from "node:path";

const ADDRESS = "0x3333333333333333333333333333333333333333";
const TX_HASH = "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

class FakeElement {
  constructor(tagName, id = "") {
    this.tagName = tagName;
    this.id = id;
    this.children = [];
    this.dataset = {};
    this.listeners = new Map();
    this.parentNode = null;
    this.disabled = false;
    this.hidden = false;
    this.textContent = "";
    this.className = "";
    this.type = "";
  }

  addEventListener(type, callback) {
    this.listeners.set(type, callback);
  }

  append(...children) {
    for (const child of children) {
      if (child && typeof child === "object") {
        child.parentNode = this;
      }
      this.children.push(child);
    }
  }

  replaceChildren(...children) {
    this.children = [];
    this.append(...children);
  }

  closest(selector) {
    if (selector === "[data-wallet-request-sign]" && this.dataset.walletRequestSign) {
      return this;
    }
    if (selector === "[data-wallet-copy-address]" && this.dataset.walletCopyAddress) {
      return this;
    }
    return this.parentNode && typeof this.parentNode.closest === "function"
      ? this.parentNode.closest(selector)
      : null;
  }

  async click() {
    const callback = this.listeners.get("click");
    if (callback) {
      await callback({ target: this });
    }
  }
}

class FakeDocument {
  constructor() {
    this.nodes = new Map([
      ["#wallet-connect", new FakeElement("button", "wallet-connect")],
      ["#wallet-status", new FakeElement("div", "wallet-status")],
      ["#wallet-state", new FakeElement("div", "wallet-state")],
      ["#wallet-accounts", new FakeElement("div", "wallet-accounts")],
      ["#wallet-requests", new FakeElement("div", "wallet-requests")],
    ]);
  }

  querySelector(selector) {
    return this.nodes.get(selector) || null;
  }

  createElement(tagName) {
    return new FakeElement(tagName);
  }
}

function findReviewButton(root) {
  const stack = [...root.children];
  while (stack.length > 0) {
    const node = stack.shift();
    if (!node || typeof node !== "object") {
      continue;
    }
    if (node.dataset && node.dataset.walletRequestSign) {
      return node;
    }
    if (Array.isArray(node.children)) {
      stack.push(...node.children);
    }
  }
  return null;
}

function createFakeProvider() {
  const calls = [];
  let chainId = "0x1";
  let chainAdded = false;
  return {
    calls,
    isMetaMask: true,
    async request(payload) {
      calls.push(payload);
      switch (payload.method) {
        case "eth_chainId":
          return chainId;
        case "wallet_switchEthereumChain":
          if (!chainAdded && payload.params?.[0]?.chainId === "0x14") {
            const error = new Error("Unrecognized chain");
            error.code = 4902;
            throw error;
          }
          chainId = payload.params?.[0]?.chainId || chainId;
          return null;
        case "wallet_addEthereumChain":
          if (payload.params?.[0]?.chainId !== "0x14") {
            throw new Error("connector tried to add the wrong chain");
          }
          if (payload.params?.[0]?.rpcUrls?.[0] !== "https://api.elastos.io/esc") {
            throw new Error("connector did not use the typed ESC RPC metadata");
          }
          chainAdded = true;
          chainId = "0x14";
          return null;
        case "eth_accounts":
        case "eth_requestAccounts":
          return [ADDRESS];
        case "personal_sign":
          return "0xsigned";
        case "eth_sendTransaction":
          if (chainId !== "0x14") {
            throw new Error(`expected connector to switch to 0x14 before sending, got ${chainId}`);
          }
          if (payload.params?.[0]?.from?.toLowerCase() !== ADDRESS.toLowerCase()) {
            throw new Error("connector sent transaction for the wrong account");
          }
          return TX_HASH;
        default:
          throw new Error(`unexpected provider request: ${payload.method}`);
      }
    },
  };
}

function createResponse(body, ok = true, status = 200) {
  return {
    ok,
    status,
    statusText: ok ? "OK" : "Error",
    async json() {
      return body;
    },
    async text() {
      return typeof body === "string" ? body : JSON.stringify(body);
    },
  };
}

function connectorRequest(connectorId) {
  return {
    request_id: `wallet-approval:${connectorId}`,
    status: "pending",
    intent: "transaction_intent",
    capsule_id: "browser",
    resource: "elastos://chain/esc-mainnet/broadcast_transaction",
    account_id: `wallet:eip155:20:${ADDRESS}`,
    chain_namespace: "eip155:20",
    address: ADDRESS,
    proof_type: "siwe",
    connector_id: connectorId,
    reason: "Browser page requests eth_sendTransaction on esc-mainnet",
  };
}

function connectorHandoff() {
  return {
    handoff: {
      schema: "elastos.wallet.webconnect_handoff/v1",
      intent: "transaction_intent",
      signer: ADDRESS,
      payload_hash: "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
      transaction: {
        from: ADDRESS,
        to: "0x2222222222222222222222222222222222222222",
        value: "0x1",
        data: "0x",
        chainId: "0x14",
        gas: "0x5208",
        gasPrice: "0x3b9aca00",
        nonce: "0x1",
      },
      status: "awaiting_wallet_transaction",
    },
  };
}

async function runConnectorSmoke({ connectorId, scriptPath, installProvider }) {
  const document = new FakeDocument();
  const provider = createFakeProvider();
  const completeBodies = [];
  const fetchCalls = [];
  const sdkModule = `data:text/javascript,${encodeURIComponent(`
    export async function connectWalletConnectEvm() {
      return globalThis.__walletConnectProvider;
    }
  `)}`;

  globalThis.document = document;
  Object.defineProperty(globalThis, "navigator", {
    configurable: true,
    value: { clipboard: { async writeText() {} } },
  });
  globalThis.Event = class Event {
    constructor(type) {
      this.type = type;
    }
  };
  globalThis.__walletConnectProvider = provider;
  globalThis.window = {
    location: {
      search: "?home_token=test-token",
      origin: "https://elastos.elacitylabs.com",
    },
    parent: {
      postMessage() {},
    },
    addEventListener() {},
    dispatchEvent() {},
  };
  globalThis.window.window = globalThis.window;
  installProvider(globalThis.window, provider);

  globalThis.fetch = async (url, init = {}) => {
    fetchCalls.push({ url, init });
    if (!init.headers || init.headers["x-elastos-home-token"] !== "test-token") {
      throw new Error(`missing connector Runtime launch token for ${url}`);
    }
    if (url.endsWith("/wallet/config")) {
      return createResponse({
        evm_chains: [{
          chainId: "0x14",
          chainName: "Elastos Smart Chain",
          nativeCurrency: { name: "ELA", symbol: "ELA", decimals: 18 },
          rpcUrls: ["https://api.elastos.io/esc"],
        }],
        walletconnect: {
          sdk_asset_path: sdkModule,
          project_id: "runtime-test-project",
        },
      });
    }
    if (url.endsWith("/wallet/accounts")) {
      return createResponse({
        accounts: [{
          account_id: `wallet:eip155:20:${ADDRESS}`,
          chain_namespace: "eip155:20",
          address: ADDRESS,
          proof_type: "siwe",
          connector_id: connectorId,
        }],
      });
    }
    if (url.endsWith("/wallet/approvals")) {
      return createResponse({
        approval_requests: [connectorRequest(connectorId)],
      });
    }
    if (url.endsWith(`/wallet/approvals/${encodeURIComponent(`wallet-approval:${connectorId}`)}/approve`)) {
      return createResponse(connectorHandoff());
    }
    if (url.endsWith(`/wallet/approvals/${encodeURIComponent(`wallet-approval:${connectorId}`)}/complete`)) {
      const body = JSON.parse(init.body || "{}");
      completeBodies.push(body);
      return createResponse({ ok: true });
    }
    throw new Error(`unexpected Runtime endpoint: ${url}`);
  };

  await import(`${pathToFileURL(resolve(scriptPath)).href}?smoke=${Date.now()}-${connectorId}`);
  await new Promise((resolveTick) => setImmediate(resolveTick));

  const requestRoot = document.querySelector("#wallet-requests");
  const review = findReviewButton(requestRoot);
  if (!review) {
    throw new Error(`${connectorId} did not render a transaction approval request`);
  }
  const requestClick = requestRoot.listeners.get("click");
  if (!requestClick) {
    throw new Error(`${connectorId} did not register request review handling`);
  }
  await requestClick({ target: review });

  const methods = provider.calls.map((call) => call.method);
  if (!methods.includes("wallet_switchEthereumChain")) {
    throw new Error(`${connectorId} did not switch to the transaction chain before send: ${methods.join(", ")}`);
  }
  if (!methods.includes("wallet_addEthereumChain")) {
    throw new Error(`${connectorId} did not add known Runtime chain metadata after unknown-chain switch failure`);
  }
  if (!methods.includes("eth_sendTransaction")) {
    throw new Error(`${connectorId} did not call the external wallet transaction sender`);
  }
  if (completeBodies.length !== 1) {
    throw new Error(`${connectorId} did not complete exactly one Runtime approval`);
  }
  const completion = completeBodies[0];
  if (completion.transaction_hash !== TX_HASH) {
    throw new Error(`${connectorId} completion did not include the external transaction hash`);
  }
  if (Object.prototype.hasOwnProperty.call(completion, "signature")) {
    throw new Error(`${connectorId} transaction completion must not send a signature field`);
  }

  return {
    connectorId,
    providerMethods: methods,
    runtimeEndpoints: fetchCalls.map((call) => call.url),
  };
}

const results = [];
results.push(await runConnectorSmoke({
  connectorId: "wallet-metamask",
  scriptPath: "capsules/wallet-metamask/browser/wallet-metamask.js",
  installProvider(windowObject, provider) {
    windowObject.ethereum = provider;
  },
}));
results.push(await runConnectorSmoke({
  connectorId: "wallet-walletconnect",
  scriptPath: "capsules/wallet-walletconnect/browser/wallet-walletconnect.js",
  installProvider() {},
}));

console.log(JSON.stringify({ ok: true, results }, null, 2));
