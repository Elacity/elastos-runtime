#!/usr/bin/env node
import { readFileSync } from "node:fs";
import { pathToFileURL } from "node:url";
import { resolve } from "node:path";

const ADDRESS = "0x3333333333333333333333333333333333333333";
const TX_HASH = `0x${"aa".repeat(32)}`;
const PAYLOAD_HASH = `0x${"bb".repeat(32)}`;
const HOME_ORIGIN = "https://elastos.elacitylabs.com";
const CONNECTOR_TOKEN = "test-connector-token";
const homeClipboardClientUrl = pathToFileURL(
  resolve("capsules/home/browser/home-clipboard-client.js"),
).href;

async function importConnectorFixture(path) {
  const source = readFileSync(resolve(path), "utf8").replace(
    '"/apps/home/home-clipboard-client.js?v=home-20260807a"',
    JSON.stringify(homeClipboardClientUrl),
  );
  return import(
    `data:text/javascript;base64,${Buffer.from(source).toString("base64")}#${Date.now()}`
  );
}

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
      if (child && typeof child === "object") child.parentNode = this;
      this.children.push(child);
    }
  }

  replaceChildren(...children) {
    this.children = [];
    this.append(...children);
  }

  closest(selector) {
    if (selector === "[data-wallet-request-sign]" && this.dataset.walletRequestSign) return this;
    if (selector === "[data-wallet-copy-address]" && this.dataset.walletCopyAddress) return this;
    return this.parentNode?.closest?.(selector) || null;
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
    if (node?.dataset?.walletRequestSign) return node;
    if (Array.isArray(node?.children)) stack.push(...node.children);
  }
  return null;
}

function createFakeProvider() {
  const calls = [];
  let chainId = "0x1";
  let chainAdded = false;
  return {
    calls,
    async request(payload) {
      calls.push(payload);
      switch (payload.method) {
        case "eth_chainId":
          return chainId;
        case "wallet_switchEthereumChain": {
          if (!chainAdded && payload.params?.[0]?.chainId === "0x14") {
            const error = new Error("Unrecognized chain");
            error.code = 4902;
            throw error;
          }
          chainId = payload.params?.[0]?.chainId || chainId;
          return null;
        }
        case "wallet_addEthereumChain":
          if (
            payload.params?.[0]?.chainId !== "0x14"
            || payload.params?.[0]?.rpcUrls?.[0] !== "https://api.elastos.io/esc"
          ) {
            throw new Error("host did not use the Runtime-selected ESC chain metadata");
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
          if (
            chainId !== "0x14"
            || payload.params?.[0]?.from?.toLowerCase() !== ADDRESS.toLowerCase()
          ) {
            throw new Error("host sent the transaction with the wrong chain or account");
          }
          return TX_HASH;
        default:
          throw new Error(`unexpected provider request: ${payload.method}`);
      }
    },
  };
}

function response(body) {
  return {
    ok: true,
    status: 200,
    statusText: "OK",
    async json() {
      return body;
    },
    async text() {
      return JSON.stringify(body);
    },
  };
}

function approvalRequest(connectorId) {
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

function handoffEnvelope(connectorId) {
  const requestId = `wallet-approval:${connectorId}`;
  return {
    schema: "elastos.home.wallet-connector.effect.result/v1",
    action: "approval_handoff",
    connector_id: connectorId,
    request_id: requestId,
    handoff: {
      schema: "elastos.wallet.webconnect_handoff/v1",
      request_id: requestId,
      intent: "transaction_intent",
      signer: ADDRESS,
      payload_hash: PAYLOAD_HASH,
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
    evm_chains: [{
      chainId: "0x14",
      chainName: "Elastos Smart Chain",
      nativeCurrency: { name: "ELA", symbol: "ELA", decimals: 18 },
      rpcUrls: ["https://api.elastos.io/esc"],
    }],
  };
}

function eventHarness() {
  const listeners = new Map();
  return {
    add(type, callback) {
      if (!listeners.has(type)) listeners.set(type, []);
      listeners.get(type).push(callback);
    },
    remove(type, callback) {
      listeners.set(type, (listeners.get(type) || []).filter((entry) => entry !== callback));
    },
    dispatch(type, event) {
      for (const callback of listeners.get(type) || []) callback(event);
    },
  };
}

async function runMetaMaskHostBridgeSmoke() {
  const connectorId = "wallet-metamask";
  const document = new FakeDocument();
  const provider = createFakeProvider();
  const events = eventHarness();
  const fetchCalls = [];
  const completionBodies = [];
  let openCount = 0;
  let frameWindow;

  const homeHost = {
    postMessage(message, targetOrigin) {
      if (message.type === "home:app-ready") return;
      if (
        targetOrigin !== HOME_ORIGIN
        || Object.keys(message).sort().join(",")
          !== "action,connectorId,connectorToken,requestId,schema,type"
        || message.type !== "home:wallet-connector-effect"
        || message.schema !== "elastos.home.wallet-connector-effect/v1"
        || message.connectorId !== connectorId
        || message.connectorToken !== CONNECTOR_TOKEN
        || Object.keys(message.action || {}).sort().join(",") !== "approvalRequestId,kind"
        || message.action.kind !== "approve"
        || message.action.approvalRequestId !== `wallet-approval:${connectorId}`
      ) {
        throw new Error("connector sent an open or malformed Home wallet effect");
      }
      performTrustedHomeTransaction(message)
        .then((result) => {
          events.dispatch("message", {
            source: homeHost,
            origin: HOME_ORIGIN,
            data: {
              type: "home:wallet-connector-effect-result",
              schema: "elastos.home.wallet-connector-effect-result/v1",
              requestId: message.requestId,
              connectorId,
              result,
            },
          });
        })
        .catch((error) => {
          events.dispatch("message", {
            source: homeHost,
            origin: HOME_ORIGIN,
            data: {
              type: "home:wallet-connector-effect-result",
              schema: "elastos.home.wallet-connector-effect-result/v1",
              requestId: message.requestId,
              connectorId,
              error: error.message,
            },
          });
        });
    },
  };

  async function performTrustedHomeTransaction(message) {
    const requestId = message.action.approvalRequestId;
    const handoff = await fetchJson(
      `/api/apps/home/wallet-connector/approvals/${encodeURIComponent(requestId)}/handoff`,
      {
        method: "POST",
        body: JSON.stringify({
          schema: "elastos.home.wallet-connector.effect.request/v1",
          connector_id: connectorId,
          connector_token: message.connectorToken,
        }),
      },
    );
    const transaction = handoff.handoff.transaction;
    const targetChain = transaction.chainId;
    if (await provider.request({ method: "eth_chainId" }) !== targetChain) {
      try {
        await provider.request({
          method: "wallet_switchEthereumChain",
          params: [{ chainId: targetChain }],
        });
      } catch (error) {
        if (Number(error.code) !== 4902) throw error;
        await provider.request({
          method: "wallet_addEthereumChain",
          params: [handoff.evm_chains[0]],
        });
      }
    }
    const [signer] = await provider.request({ method: "eth_accounts" });
    if (signer.toLowerCase() !== handoff.handoff.signer.toLowerCase()) {
      throw new Error("Home host signer mismatch");
    }
    const transactionHash = await provider.request({
      method: "eth_sendTransaction",
      params: [transaction],
    });
    await fetchJson(
      `/api/apps/home/wallet-connector/approvals/${encodeURIComponent(requestId)}/complete`,
      {
        method: "POST",
        body: JSON.stringify({
          schema: "elastos.home.wallet-connector.effect.request/v1",
          connector_id: connectorId,
          connector_token: message.connectorToken,
          payload_hash: handoff.handoff.payload_hash,
          signer,
          transaction_hash: transactionHash,
        }),
      },
    );
    return { status: "completed", effect: "transaction" };
  }

  async function fetchJson(url, init) {
    const result = await globalThis.fetch(url, init);
    return result.json();
  }

  globalThis.document = document;
  Object.defineProperty(globalThis, "navigator", {
    configurable: true,
    value: { clipboard: { async writeText() {} } },
  });
  frameWindow = {
    location: {
      search: `?home_origin=${encodeURIComponent(HOME_ORIGIN)}`,
      hash: `#home_token=${CONNECTOR_TOKEN}`,
      origin: "null",
    },
    crypto: { randomUUID: () => "transaction-smoke" },
    top: homeHost,
    parent: homeHost,
    addEventListener: (type, callback) => events.add(type, callback),
    removeEventListener: (type, callback) => events.remove(type, callback),
    setTimeout: globalThis.setTimeout,
    clearTimeout: globalThis.clearTimeout,
    open() {
      openCount += 1;
      throw new Error("connector opened an extra window");
    },
  };
  frameWindow.window = frameWindow;
  globalThis.window = frameWindow;

  globalThis.fetch = async (url, init = {}) => {
    const body = init.body ? JSON.parse(init.body) : null;
    fetchCalls.push({ url: String(url), init, body });
    if (String(url).endsWith("/wallet/accounts")) {
      if (init.headers?.["x-elastos-home-token"] !== CONNECTOR_TOKEN) {
        throw new Error("connector read did not use its fragment launch token");
      }
      return response({
        accounts: [{
          account_id: `wallet:eip155:20:${ADDRESS}`,
          chain_namespace: "eip155:20",
          address: ADDRESS,
          proof_type: "siwe",
          connector_id: connectorId,
        }],
      });
    }
    if (String(url).endsWith("/wallet/approvals")) {
      if (init.headers?.["x-elastos-home-token"] !== CONNECTOR_TOKEN) {
        throw new Error("connector approval read did not use its fragment launch token");
      }
      return response({ approval_requests: [approvalRequest(connectorId)] });
    }
    if (String(url).endsWith("/handoff")) {
      if (
        init.headers?.["x-elastos-home-token"]
        || body?.connector_token !== CONNECTOR_TOKEN
      ) {
        throw new Error("Home bridge used connector ambient authority instead of carried authority");
      }
      return response(handoffEnvelope(connectorId));
    }
    if (String(url).endsWith("/complete")) {
      completionBodies.push(body);
      return response({
        schema: "elastos.home.wallet-connector.effect.result/v1",
        action: "approval_complete",
        connector_id: connectorId,
        request_id: `wallet-approval:${connectorId}`,
        status: "completed",
      });
    }
    throw new Error(`unexpected Runtime endpoint: ${url}`);
  };

  await importConnectorFixture(
    "capsules/wallet-metamask/browser/wallet-metamask.js",
  );
  await new Promise((resolveTick) => setImmediate(resolveTick));
  const requestRoot = document.querySelector("#wallet-requests");
  const review = findReviewButton(requestRoot);
  await requestRoot.listeners.get("click")({ target: review });

  const methods = provider.calls.map((call) => call.method);
  if (
    !methods.includes("wallet_switchEthereumChain")
    || !methods.includes("wallet_addEthereumChain")
    || !methods.includes("eth_sendTransaction")
  ) {
    throw new Error(`Home host did not execute the closed transaction path: ${methods.join(", ")}`);
  }
  if (
    frameWindow.ethereum
    || fetchCalls.some((call) =>
      call.url.includes("/wallet/approvals/") && /\/(approve|complete)$/.test(call.url)
      && !call.url.includes("/api/apps/home/wallet-connector/")
    )
  ) {
    throw new Error("opaque MetaMask frame retained direct injected-provider or effect access");
  }
  if (
    completionBodies.length !== 1
    || completionBodies[0].transaction_hash !== TX_HASH
    || Object.hasOwn(completionBodies[0], "signature")
    || openCount !== 0
    || frameWindow.location.search.includes("home_token")
    || !frameWindow.location.hash.includes(`home_token=${CONNECTOR_TOKEN}`)
  ) {
    throw new Error("MetaMask Home bridge lost fragment authority or bounded completion semantics");
  }
  return {
    connectorId,
    providerMethods: methods,
    runtimeEndpoints: fetchCalls.map((call) => call.url),
    authorityTransport: "fragment",
    effectOwner: "home-host",
  };
}

async function runWalletConnectSmoke() {
  const connectorId = "wallet-walletconnect";
  const document = new FakeDocument();
  const provider = createFakeProvider();
  const fetchCalls = [];
  const completionBodies = [];
  const sdkModule = `data:text/javascript,${encodeURIComponent(`
    export async function connectWalletConnectEvm() {
      return globalThis.__walletConnectProvider;
    }
  `)}`;
  const top = { postMessage() {} };
  globalThis.document = document;
  globalThis.__walletConnectProvider = provider;
  globalThis.window = {
    location: {
      search: `?home_origin=${encodeURIComponent(HOME_ORIGIN)}`,
      hash: `#home_token=${CONNECTOR_TOKEN}`,
      origin: "null",
    },
    top,
    parent: top,
    addEventListener() {},
    dispatchEvent() {},
  };
  globalThis.window.window = globalThis.window;
  globalThis.fetch = async (url, init = {}) => {
    fetchCalls.push({ url: String(url), init });
    if (init.headers?.["x-elastos-home-token"] !== CONNECTOR_TOKEN) {
      throw new Error(`WalletConnect lost its fragment launch token for ${url}`);
    }
    if (String(url).endsWith("/wallet/config")) {
      return response({
        evm_chains: handoffEnvelope(connectorId).evm_chains,
        walletconnect: {
          sdk_asset_path: sdkModule,
          project_id: "runtime-test-project",
        },
      });
    }
    if (String(url).endsWith("/wallet/accounts")) {
      return response({
        accounts: [{
          account_id: `wallet:eip155:20:${ADDRESS}`,
          chain_namespace: "eip155:20",
          address: ADDRESS,
          proof_type: "siwe",
          connector_id: connectorId,
        }],
      });
    }
    if (String(url).endsWith("/wallet/approvals")) {
      return response({ approval_requests: [approvalRequest(connectorId)] });
    }
    if (String(url).endsWith("/approve")) {
      return response({ handoff: handoffEnvelope(connectorId).handoff });
    }
    if (String(url).endsWith("/complete")) {
      completionBodies.push(JSON.parse(init.body || "{}"));
      return response({ ok: true });
    }
    throw new Error(`unexpected Runtime endpoint: ${url}`);
  };
  await importConnectorFixture(
    "capsules/wallet-walletconnect/browser/wallet-walletconnect.js",
  );
  await new Promise((resolveTick) => setImmediate(resolveTick));
  const requestRoot = document.querySelector("#wallet-requests");
  const review = findReviewButton(requestRoot);
  await requestRoot.listeners.get("click")({ target: review });
  const methods = provider.calls.map((call) => call.method);
  if (
    !methods.includes("eth_sendTransaction")
    || completionBodies.length !== 1
    || completionBodies[0].transaction_hash !== TX_HASH
    || globalThis.window.location.search.includes("home_token")
  ) {
    throw new Error("WalletConnect configured path or fragment authority regressed");
  }
  return {
    connectorId,
    providerMethods: methods,
    runtimeEndpoints: fetchCalls.map((call) => call.url),
    authorityTransport: "fragment",
    effectOwner: "walletconnect-configured-path",
  };
}

const results = [
  await runMetaMaskHostBridgeSmoke(),
  await runWalletConnectSmoke(),
];

console.log(JSON.stringify({ ok: true, results }, null, 2));
