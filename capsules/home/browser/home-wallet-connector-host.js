import { fetchJson } from "./shell-core.js?v=home-20260802a";

export const WALLET_CONNECTOR_EFFECT_TYPE = "home:wallet-connector-effect";
export const WALLET_CONNECTOR_EFFECT_SCHEMA = "elastos.home.wallet-connector-effect/v1";
const WALLET_CONNECTOR_EFFECT_RESULT_TYPE = "home:wallet-connector-effect-result";
const WALLET_CONNECTOR_EFFECT_RESULT_SCHEMA =
  "elastos.home.wallet-connector-effect-result/v1";
const RUNTIME_REQUEST_SCHEMA = "elastos.home.wallet-connector.effect.request/v1";
const RUNTIME_RESULT_SCHEMA = "elastos.home.wallet-connector.effect.result/v1";
const METAMASK_CONNECTOR_ID = "wallet-metamask";
const UNISAT_CONNECTOR_ID = "wallet-unisat";
const CONNECTOR_IDS = new Set([METAMASK_CONNECTOR_ID, UNISAT_CONNECTOR_ID]);
const METAMASK_RDNS = "io.metamask";
const BRAVE_RDNS = "com.brave.wallet";
const MAX_EIP6963_PROVIDERS = 32;
const MAX_EFFECT_REQUESTS_PER_FRAME = 64;
const OPAQUE_FRAME_TARGET = "*";
const eip6963Providers = [];
let eip6963Overflow = false;

installEip6963Discovery();

export function handleHomeWalletConnectorEffect(event, context, data) {
  if (data?.type !== WALLET_CONNECTOR_EFFECT_TYPE) {
    return false;
  }
  const requestId = readText(data.requestId);
  const connectorId = readText(data.connectorId);
  const action = data.action;
  const state = context?.walletEffectState;
  if (
    context?.kind !== "app-frame"
    || !CONNECTOR_IDS.has(context.targetId)
    || data.requestId !== requestId
    || data.connectorId !== connectorId
    || connectorId !== context.targetId
    || !hasExactKeys(data, [
      "type",
      "schema",
      "requestId",
      "connectorId",
      "connectorToken",
      "action",
    ])
    || data.schema !== WALLET_CONNECTOR_EFFECT_SCHEMA
    || data.connectorToken !== context.homeToken
    || !validRequestId(requestId)
    || !validAction(action)
    || !state
  ) {
    replyToConnector(event, requestId, connectorId, null, "Home denied the wallet request.");
    return true;
  }
  if (state.inFlight || state.requestIds.has(requestId)) {
    replyToConnector(
      event,
      requestId,
      connectorId,
      null,
      state.inFlight ? "A wallet request is already active." : "Wallet request replay denied.",
    );
    return true;
  }
  while (state.requestIds.size >= MAX_EFFECT_REQUESTS_PER_FRAME) {
    state.requestIds.delete(state.requestIds.values().next().value);
  }
  state.requestIds.add(requestId);
  state.inFlight = true;
  performConnectorEffect(connectorId, context.homeToken, action)
    .then((result) => replyToConnector(event, requestId, connectorId, result))
    .catch((error) => {
      replyToConnector(
        event,
        requestId,
        connectorId,
        null,
        error instanceof Error ? error.message : "Wallet request failed.",
      );
    })
    .finally(() => {
      state.inFlight = false;
    });
  return true;
}

function installEip6963Discovery() {
  window.addEventListener("eip6963:announceProvider", (event) => {
    const info = event?.detail?.info;
    const provider = event?.detail?.provider;
    const rdns = readText(info?.rdns).toLowerCase();
    const uuid = readText(info?.uuid);
    if (
      ![METAMASK_RDNS, BRAVE_RDNS].includes(rdns)
      || !uuid
      || uuid.length > 128
      || !provider
      || typeof provider.request !== "function"
    ) {
      return;
    }
    if (eip6963Providers.length >= MAX_EIP6963_PROVIDERS) {
      eip6963Overflow = true;
      return;
    }
    if (!eip6963Providers.some((entry) => (
      entry.provider === provider && entry.rdns === rdns && entry.uuid === uuid
    ))) {
      eip6963Providers.push({ provider, rdns, uuid });
    }
  });
  requestEip6963Providers();
}

function requestEip6963Providers() {
  if (typeof window.dispatchEvent !== "function") {
    return;
  }
  window.dispatchEvent(new Event("eip6963:requestProvider"));
}

function selectMetaMaskCompatibleProvider() {
  requestEip6963Providers();
  if (eip6963Overflow) {
    throw new Error("Injected wallet provider discovery is ambiguous.");
  }
  const matching = eip6963Providers.filter((entry) => (
    entry.rdns === METAMASK_RDNS || entry.rdns === BRAVE_RDNS
  ));
  for (const entry of matching) {
    const identities = new Set(
      matching.filter((candidate) => candidate.provider === entry.provider)
        .map((candidate) => candidate.rdns),
    );
    const uuids = new Set(
      matching
        .filter((candidate) => (
          candidate.provider === entry.provider && candidate.rdns === entry.rdns
        ))
        .map((candidate) => candidate.uuid),
    );
    if (identities.size !== 1 || uuids.size !== 1) {
      throw new Error("Injected wallet provider identities conflict.");
    }
  }
  const providersFor = (rdns) => [
    ...new Set(matching.filter((entry) => entry.rdns === rdns).map((entry) => entry.provider)),
  ];
  const metamask = providersFor(METAMASK_RDNS);
  const brave = providersFor(BRAVE_RDNS);
  if (metamask.length > 1 || brave.length > 1) {
    throw new Error("Injected wallet provider discovery is ambiguous.");
  }
  if (metamask.length === 1) {
    return metamask[0];
  }
  if (brave.length === 1) {
    return brave[0];
  }
  throw new Error("MetaMask or Brave Wallet was not found in this browser profile.");
}

async function performConnectorEffect(connectorId, connectorToken, action) {
  if (connectorId === METAMASK_CONNECTOR_ID && action.kind === "link") {
    return linkEvmWallet(connectorToken);
  }
  if (connectorId === UNISAT_CONNECTOR_ID && action.kind === "link") {
    return linkBitcoinWallet(connectorToken);
  }
  if (action.kind === "approve") {
    return completeApproval(connectorId, connectorToken, action.approvalRequestId);
  }
  throw new Error("Unsupported wallet request.");
}

async function linkEvmWallet(connectorToken) {
  const provider = selectMetaMaskCompatibleProvider();
  const accounts = await provider.request({ method: "eth_requestAccounts" });
  const address = requireEvmAddress(firstProviderAccount(accounts));
  const chainId = requireEvmChainId(await provider.request({ method: "eth_chainId" }));
  const challengeResult = await runtimePost(
    "/api/apps/home/wallet-connector/evm/link/challenge",
    connectorBody(METAMASK_CONNECTOR_ID, connectorToken, {
      address,
      chain_id: chainId,
    }),
  );
  const challenge = requireEvmChallenge(challengeResult);
  const signer = requireEvmAddress(await currentEvmAccount(provider));
  ensureSameAddress(signer, address, "Wallet account changed before signing.");
  const signature = requireProviderResult(await provider.request({
    method: "personal_sign",
    params: [challenge.message, signer],
  }), 16 * 1024, "Wallet returned an invalid signature.");
  const verified = await runtimePost(
    "/api/apps/home/wallet-connector/evm/link/verify",
    connectorBody(METAMASK_CONNECTOR_ID, connectorToken, {
      message: challenge.message,
      signature,
    }),
  );
  requireRuntimeCompletion(verified, "evm_link_complete", METAMASK_CONNECTOR_ID, "linked");
  return Object.freeze({ status: "linked" });
}

async function linkBitcoinWallet(connectorToken) {
  const provider = requireUniSatProvider();
  const account = await requestUniSatAccount(provider);
  const challengeResult = await runtimePost(
    "/api/apps/home/wallet-connector/bitcoin/link/challenge",
    connectorBody(UNISAT_CONNECTOR_ID, connectorToken, {
      address: account.address,
      network: "bitcoin",
    }),
  );
  const challenge = requireBitcoinChallenge(challengeResult, account.address);
  const signer = await currentUniSatAccount(provider);
  ensureSameAddress(signer.address, account.address, "UniSat account changed before signing.");
  const proof = await signBitcoinMessage(provider, signer, challenge.message, challenge.proof_type);
  const verified = await runtimePost(
    "/api/apps/home/wallet-connector/bitcoin/link/verify",
    connectorBody(UNISAT_CONNECTOR_ID, connectorToken, {
      message: challenge.message,
      signature: proof.signature,
      signature_type: proof.signatureType,
      public_key: proof.publicKey,
    }),
  );
  requireRuntimeCompletion(verified, "bitcoin_link_complete", UNISAT_CONNECTOR_ID, "linked");
  return Object.freeze({ status: "linked" });
}

async function completeApproval(connectorId, connectorToken, approvalRequestId) {
  const provider = connectorId === METAMASK_CONNECTOR_ID
    ? selectMetaMaskCompatibleProvider()
    : requireUniSatProvider();
  const encodedRequestId = encodeURIComponent(approvalRequestId);
  const response = await runtimePost(
    `/api/apps/home/wallet-connector/approvals/${encodedRequestId}/handoff`,
    connectorBody(connectorId, connectorToken),
  );
  const { handoff, evmChains } = requireHandoffResponse(
    response,
    connectorId,
    approvalRequestId,
  );
  if (connectorId === METAMASK_CONNECTOR_ID) {
    return completeEvmApproval(provider, connectorToken, handoff, evmChains);
  }
  return completeBitcoinApproval(provider, connectorToken, handoff);
}

async function completeEvmApproval(provider, connectorToken, handoff, evmChains) {
  const signer = requireEvmAddress(await currentEvmAccount(provider));
  ensureSameAddress(signer, handoff.signer, "Switch to the linked account before approving.");
  const path = `/api/apps/home/wallet-connector/approvals/${encodeURIComponent(handoff.request_id)}/complete`;
  if (handoff.intent === "transaction_intent") {
    await ensureProviderChain(provider, handoff.transaction.chainId, evmChains);
    const currentSigner = requireEvmAddress(await currentEvmAccount(provider));
    ensureSameAddress(
      currentSigner,
      handoff.signer,
      "Switch to the linked account before approving.",
    );
    const transactionHash = requireProviderResult(await provider.request({
      method: "eth_sendTransaction",
      params: [handoff.transaction],
    }), 512, "Wallet returned an invalid transaction hash.");
    const completed = await runtimePost(path, connectorBody(METAMASK_CONNECTOR_ID, connectorToken, {
      payload_hash: handoff.payload_hash,
      signer: currentSigner,
      transaction_hash: transactionHash,
    }));
    requireApprovalCompletion(completed, METAMASK_CONNECTOR_ID, handoff.request_id);
    return Object.freeze({ status: "completed", effect: "transaction" });
  }
  const method = handoff.intent === "browser_typed_data_sign"
    ? "eth_signTypedData_v4"
    : "personal_sign";
  const params = method === "eth_signTypedData_v4"
    ? [signer, handoff.message]
    : [handoff.message, signer];
  const signature = requireProviderResult(await provider.request({ method, params }), 16 * 1024,
    "Wallet returned an invalid signature.");
  const completed = await runtimePost(path, connectorBody(METAMASK_CONNECTOR_ID, connectorToken, {
    payload_hash: handoff.payload_hash,
    signature,
    signature_type: method === "eth_signTypedData_v4" ? "eip712" : "personal_sign",
    signer,
  }));
  requireApprovalCompletion(completed, METAMASK_CONNECTOR_ID, handoff.request_id);
  return Object.freeze({ status: "completed", effect: "signature" });
}

async function completeBitcoinApproval(provider, connectorToken, handoff) {
  const account = await currentUniSatAccount(provider);
  ensureSameAddress(
    account.address,
    handoff.signer,
    "Switch to the linked Bitcoin account before signing.",
  );
  const proof = await signBitcoinMessage(
    provider,
    account,
    handoff.message,
    handoff.signature_type,
  );
  const path = `/api/apps/home/wallet-connector/approvals/${encodeURIComponent(handoff.request_id)}/complete`;
  const completed = await runtimePost(path, connectorBody(UNISAT_CONNECTOR_ID, connectorToken, {
    payload_hash: handoff.payload_hash,
    signature: proof.signature,
    signature_type: proof.signatureType,
    public_key: proof.publicKey,
    signer: account.address,
  }));
  requireApprovalCompletion(completed, UNISAT_CONNECTOR_ID, handoff.request_id);
  return Object.freeze({ status: "completed", effect: "signature" });
}

async function ensureProviderChain(provider, chainId, chains) {
  const target = normalizeChainId(chainId);
  const active = normalizeChainId(await provider.request({ method: "eth_chainId" }));
  if (active === target) {
    return;
  }
  try {
    await provider.request({
      method: "wallet_switchEthereumChain",
      params: [{ chainId: target }],
    });
  } catch (error) {
    if (!isUnknownChainError(error)) {
      throw error;
    }
    const chain = requireEvmChain(chains, target);
    await provider.request({
      method: "wallet_addEthereumChain",
      params: [chain],
    });
  }
  if (normalizeChainId(await provider.request({ method: "eth_chainId" })) !== target) {
    throw new Error("Wallet did not switch to the Runtime-selected chain.");
  }
}

function requireEvmChain(chains, chainId) {
  const matches = chains.filter((chain) => normalizeChainId(chain?.chainId) === chainId);
  if (matches.length !== 1) {
    throw new Error("Runtime did not provide one exact chain configuration.");
  }
  const chain = matches[0];
  if (
    !hasExactKeys(chain, ["chainId", "chainName", "nativeCurrency", "rpcUrls"])
    || !hasExactKeys(chain.nativeCurrency, ["name", "symbol", "decimals"])
    || !boundedText(chain.chainName, 128)
    || !boundedText(chain.nativeCurrency.name, 128)
    || !boundedText(chain.nativeCurrency.symbol, 32)
    || !Number.isSafeInteger(chain.nativeCurrency.decimals)
    || chain.nativeCurrency.decimals < 0
    || chain.nativeCurrency.decimals > 255
    || !Array.isArray(chain.rpcUrls)
    || chain.rpcUrls.length < 1
    || chain.rpcUrls.length > 4
    || chain.rpcUrls.some((url) => !boundedText(url, 2048) || !url.startsWith("https://"))
  ) {
    throw new Error("Runtime returned an invalid chain configuration.");
  }
  return chain;
}

function requireHandoffResponse(response, connectorId, requestId) {
  if (
    !hasExactKeys(response, [
      "schema",
      "action",
      "connector_id",
      "request_id",
      "handoff",
      "evm_chains",
    ])
    || response.schema !== RUNTIME_RESULT_SCHEMA
    || response.action !== "approval_handoff"
    || response.connector_id !== connectorId
    || response.request_id !== requestId
    || !Array.isArray(response.evm_chains)
  ) {
    throw new Error("Runtime returned an invalid wallet handoff envelope.");
  }
  const handoff = response.handoff;
  const commonKeys = [
    "schema",
    "request_id",
    "intent",
    "payload_hash",
    "signer",
    "status",
  ];
  const transaction = handoff?.intent === "transaction_intent";
  const expectedKeys = transaction
    ? [...commonKeys, "transaction"]
    : [...commonKeys, "message", "signature_type"];
  if (
    !hasExactKeys(handoff, expectedKeys)
    || handoff.schema !== "elastos.wallet.webconnect_handoff/v1"
    || handoff.request_id !== requestId
    || !boundedText(handoff.intent, 128)
    || !/^0x[0-9a-fA-F]{64}$/.test(handoff.payload_hash || "")
    || !boundedText(handoff.signer, 256)
  ) {
    throw new Error("Runtime returned an invalid typed wallet handoff.");
  }
  if (connectorId === UNISAT_CONNECTOR_ID) {
    if (
      handoff.intent !== "bitcoin_bip322_proof"
      || handoff.status !== "awaiting_wallet_signature"
      || !boundedSignedMessage(handoff.message)
      || !["bip322_simple", "ecdsa"].includes(handoff.signature_type)
      || !supportedBitcoinAddress(handoff.signer)
    ) {
      throw new Error("Runtime returned an invalid UniSat handoff.");
    }
  } else if (transaction) {
    if (
      handoff.status !== "awaiting_wallet_transaction"
      || !validTransaction(handoff.transaction, handoff.signer)
    ) {
      throw new Error("Runtime returned an invalid EVM transaction handoff.");
    }
  } else if (
    handoff.intent === "bitcoin_bip322_proof"
    || handoff.status !== "awaiting_wallet_signature"
    || handoff.signature_type !== "personal_sign"
    || !boundedSignedMessage(handoff.message)
    || !/^0x[0-9a-fA-F]{40}$/.test(handoff.signer)
  ) {
    throw new Error("Runtime returned an invalid EVM signature handoff.");
  }
  return { handoff, evmChains: response.evm_chains };
}

function validTransaction(transaction, signer) {
  const keys = ["from", "to", "value", "data", "gas", "gasPrice", "nonce", "chainId"];
  return hasExactKeys(transaction, keys)
    && keys.every((key) => boundedText(transaction[key], key === "data" ? 65_536 : 512))
    && normalizeAddress(transaction.from) === normalizeAddress(signer)
    && /^0x[0-9a-fA-F]{40}$/.test(transaction.from)
    && /^0x[0-9a-fA-F]{40}$/.test(transaction.to)
    && /^0x[0-9a-fA-F]+$/.test(transaction.value)
    && /^0x[0-9a-fA-F]*$/.test(transaction.data)
    && /^0x[0-9a-fA-F]+$/.test(transaction.gas)
    && /^0x[0-9a-fA-F]+$/.test(transaction.gasPrice)
    && /^0x[0-9a-fA-F]+$/.test(transaction.nonce)
    && /^0x[0-9a-fA-F]+$/.test(transaction.chainId);
}

function requireEvmChallenge(response) {
  if (
    !hasExactKeys(response, ["schema", "action", "connector_id", "challenge"])
    || response.schema !== RUNTIME_RESULT_SCHEMA
    || response.action !== "evm_link_challenge"
    || response.connector_id !== METAMASK_CONNECTOR_ID
  ) {
    throw new Error("Runtime returned an invalid EVM challenge envelope.");
  }
  const challenge = response.challenge;
  if (
    !hasExactKeys(challenge, ["schema", "challenge_id", "message", "expires_at", "resources"])
    || challenge.schema !== "elastos.auth.challenge/v1"
    || !boundedText(challenge.challenge_id, 256)
    || !boundedSignedMessage(challenge.message)
    || !Number.isSafeInteger(challenge.expires_at)
    || challenge.expires_at <= 0
    || !Array.isArray(challenge.resources)
  ) {
    throw new Error("Runtime returned an invalid EVM challenge.");
  }
  return challenge;
}

function requireBitcoinChallenge(response, address) {
  if (
    !hasExactKeys(response, ["schema", "action", "connector_id", "challenge"])
    || response.schema !== RUNTIME_RESULT_SCHEMA
    || response.action !== "bitcoin_link_challenge"
    || response.connector_id !== UNISAT_CONNECTOR_ID
  ) {
    throw new Error("Runtime returned an invalid Bitcoin challenge envelope.");
  }
  const challenge = response.challenge;
  if (
    !hasExactKeys(challenge, [
      "schema",
      "challenge_id",
      "message",
      "expires_at",
      "network",
      "address",
      "resources",
      "proof_type",
    ])
    || challenge.schema !== "elastos.wallet.bitcoin_challenge/v1"
    || challenge.network !== "bitcoin"
    || normalizeAddress(challenge.address) !== normalizeAddress(address)
    || !["bip322_simple", "bitcoin_signed_message"].includes(challenge.proof_type)
    || !boundedText(challenge.challenge_id, 256)
    || !boundedSignedMessage(challenge.message)
    || !Number.isSafeInteger(challenge.expires_at)
    || challenge.expires_at <= 0
    || !Array.isArray(challenge.resources)
  ) {
    throw new Error("Runtime returned an invalid Bitcoin challenge.");
  }
  return challenge;
}

function requireRuntimeCompletion(response, action, connectorId, status) {
  if (
    !hasExactKeys(response, ["schema", "action", "connector_id", "status"])
    || response.schema !== RUNTIME_RESULT_SCHEMA
    || response.action !== action
    || response.connector_id !== connectorId
    || response.status !== status
  ) {
    throw new Error("Runtime returned an invalid wallet completion.");
  }
}

function requireApprovalCompletion(response, connectorId, requestId) {
  if (
    !hasExactKeys(response, ["schema", "action", "connector_id", "request_id", "status"])
    || response.schema !== RUNTIME_RESULT_SCHEMA
    || response.action !== "approval_complete"
    || response.connector_id !== connectorId
    || response.request_id !== requestId
    || response.status !== "completed"
  ) {
    throw new Error("Runtime returned an invalid approval completion.");
  }
}

function requireUniSatProvider() {
  const provider = window.unisat;
  if (
    !provider
    || typeof provider.requestAccounts !== "function"
    || typeof provider.getAccounts !== "function"
    || typeof provider.getNetwork !== "function"
    || typeof provider.signMessage !== "function"
  ) {
    throw new Error("UniSat was not found in this browser profile.");
  }
  return provider;
}

async function requestUniSatAccount(provider) {
  const account = accountFromProvider(await provider.requestAccounts());
  await requireUniSatMainnet(provider);
  return account;
}

async function currentUniSatAccount(provider) {
  const account = accountFromProvider(await provider.getAccounts());
  await requireUniSatMainnet(provider);
  return account;
}

async function requireUniSatMainnet(provider) {
  if (readText(await provider.getNetwork()).toLowerCase() !== "livenet") {
    throw new Error("Switch UniSat to Bitcoin mainnet.");
  }
}

function accountFromProvider(accounts) {
  const value = Array.isArray(accounts) ? accounts[0] : accounts;
  const address = typeof value === "string"
    ? value.trim()
    : ["address", "walletAddress", "account", "value"]
      .map((key) => readText(value?.[key]))
      .find(Boolean) || "";
  const type = bitcoinAddressType(address);
  if (type === "unsupported") {
    throw new Error("UniSat returned an unsupported Bitcoin address.");
  }
  return { address, type };
}

async function signBitcoinMessage(provider, account, message, signatureType) {
  if (signatureType === "bip322_simple") {
    if (!["native-segwit", "taproot"].includes(account.type)) {
      throw new Error("Runtime challenge does not match the selected Bitcoin address.");
    }
    return {
      signature: requireProviderResult(
        await provider.signMessage(message, "bip322-simple"),
        16 * 1024,
        "UniSat returned an invalid signature.",
      ),
      signatureType: "bip322_simple",
      publicKey: null,
    };
  }
  if (signatureType === "bitcoin_signed_message" || signatureType === "ecdsa") {
    if (
      !["nested-segwit", "legacy"].includes(account.type)
      || typeof provider.getPublicKey !== "function"
    ) {
      throw new Error("UniSat public-key signing is unavailable for this address.");
    }
    return {
      signature: requireProviderResult(
        await provider.signMessage(message, "ecdsa"),
        16 * 1024,
        "UniSat returned an invalid signature.",
      ),
      signatureType: "ecdsa",
      publicKey: requireProviderResult(
        await provider.getPublicKey(),
        4096,
        "UniSat returned an invalid public key.",
      ),
    };
  }
  throw new Error("Runtime returned an unsupported Bitcoin signature type.");
}

async function currentEvmAccount(provider) {
  return firstProviderAccount(await provider.request({ method: "eth_accounts" }));
}

function firstProviderAccount(accounts) {
  return Array.isArray(accounts) && typeof accounts[0] === "string" ? accounts[0].trim() : "";
}

function requireEvmAddress(value) {
  const address = readText(value);
  if (!/^0x[0-9a-fA-F]{40}$/.test(address)) {
    throw new Error("Wallet returned an invalid EVM account.");
  }
  return address;
}

function requireEvmChainId(value) {
  const chainId = normalizeChainId(value);
  const number = Number.parseInt(chainId.slice(2), 16);
  if (!Number.isSafeInteger(number) || number <= 0) {
    throw new Error("Wallet returned an invalid chain.");
  }
  return number;
}

function normalizeChainId(value) {
  const text = readText(value).toLowerCase();
  if (!/^0x[0-9a-f]+$/.test(text)) {
    throw new Error("Wallet returned an invalid chain.");
  }
  return `0x${BigInt(text).toString(16)}`;
}

function isUnknownChainError(error) {
  const message = String(error?.message || "").toLowerCase();
  return Number(error?.code) === 4902
    || message.includes("unrecognized chain")
    || message.includes("unknown chain");
}

function connectorBody(connectorId, connectorToken, extra = {}) {
  return {
    schema: RUNTIME_REQUEST_SCHEMA,
    connector_id: connectorId,
    connector_token: connectorToken,
    ...extra,
  };
}

async function runtimePost(url, body) {
  return fetchJson(url, {
    method: "POST",
    body: JSON.stringify(body),
  });
}

function validAction(action) {
  if (!action || typeof action !== "object" || Array.isArray(action)) {
    return false;
  }
  if (action.kind === "link") {
    return hasExactKeys(action, ["kind"]);
  }
  return action.kind === "approve"
    && hasExactKeys(action, ["kind", "approvalRequestId"])
    && validApprovalRequestId(action.approvalRequestId);
}

function validRequestId(value) {
  return /^[A-Za-z0-9][A-Za-z0-9._:-]{0,127}$/.test(value);
}

function validApprovalRequestId(value) {
  return boundedText(value, 256) && readText(value) === value;
}

function boundedText(value, maxLength) {
  return typeof value === "string"
    && value.length > 0
    && value.length <= maxLength
    && ![...value].some((character) => /\p{Cc}/u.test(character));
}

function boundedSignedMessage(value) {
  if (typeof value !== "string" || value.length === 0 || value.length > 65_536 || value.includes("\0")) {
    return false;
  }
  for (let index = 0; index < value.length; index += 1) {
    const code = value.charCodeAt(index);
    if (code === 10) {
      continue;
    }
    if (code === 13 && value.charCodeAt(index + 1) === 10) {
      index += 1;
      continue;
    }
    if (code < 32 || code === 127) {
      return false;
    }
  }
  return true;
}

function requireProviderResult(value, maxLength, message) {
  if (!boundedText(value, maxLength)) {
    throw new Error(message);
  }
  return value;
}

function supportedBitcoinAddress(value) {
  return bitcoinAddressType(value) !== "unsupported";
}

function bitcoinAddressType(value) {
  const address = normalizeAddress(value);
  if (address.startsWith("bc1q")) return "native-segwit";
  if (address.startsWith("bc1p")) return "taproot";
  if (address.startsWith("3")) return "nested-segwit";
  if (address.startsWith("1")) return "legacy";
  return "unsupported";
}

function ensureSameAddress(actual, expected, message) {
  if (normalizeAddress(actual) !== normalizeAddress(expected)) {
    throw new Error(message);
  }
}

function normalizeAddress(value) {
  return readText(value).toLowerCase();
}

function hasExactKeys(value, expectedKeys) {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    return false;
  }
  const actual = Object.keys(value).sort();
  const expected = [...expectedKeys].sort();
  return actual.length === expected.length
    && actual.every((key, index) => key === expected[index]);
}

function readText(value) {
  return typeof value === "string" ? value.trim() : "";
}

function replyToConnector(event, requestId, connectorId, result, error = "") {
  if (!event.source || !validRequestId(requestId) || !CONNECTOR_IDS.has(connectorId)) {
    return;
  }
  const errorText = readText(error);
  const safeError = boundedText(errorText, 512) ? errorText : "Wallet request failed.";
  const payload = error
    ? {
        type: WALLET_CONNECTOR_EFFECT_RESULT_TYPE,
        schema: WALLET_CONNECTOR_EFFECT_RESULT_SCHEMA,
        requestId,
        connectorId,
        error: safeError,
      }
    : {
        type: WALLET_CONNECTOR_EFFECT_RESULT_TYPE,
        schema: WALLET_CONNECTOR_EFFECT_RESULT_SCHEMA,
        requestId,
        connectorId,
        result,
      };
  event.source.postMessage(payload, OPAQUE_FRAME_TARGET);
}
