#!/usr/bin/env node
import assert from "node:assert/strict";
import {
  waitForWalletAccountAccess,
  walletRuntimeContextAllowsBinding,
  walletRuntimeGrantedAccount,
  walletRuntimeRecordAccountAccess,
  walletRuntimeRequestedAccount,
} from "./browser-selkies-control-service.mjs";

const now = Math.floor(Date.now() / 1000);
const accountId = "wallet:eip155:20:0x1111111111111111111111111111111111111111";
const address = "0x1111111111111111111111111111111111111111";
const runtime = {
  wallet: {
    principal_id: "person:local:browser-consent",
    session_id: "session:browser-consent",
    launch_id: "launch:browser-consent",
    default_account_id: accountId,
    default_chain_namespace: "eip155:20",
    accounts: [
      {
        account_id: accountId,
        chain_namespace: "eip155:20",
        address,
        proof_type: "managed_evm",
      },
      {
        account_id: accountId,
        chain_namespace: "eip155:8453",
        address,
        proof_type: "managed_evm",
      },
    ],
  },
  accountPermissions: new Map(),
  pendingAccountAccess: new Map(),
  revokedAccountAccess: new Set(),
};
const context = {
  executionContextId: 7,
  pageUrl: "https://dapp.example/connect",
  pageOrigin: "https://dapp.example",
  protocol: "https:",
  isDocument: true,
  isTopLevel: true,
};

assert.equal(walletRuntimeContextAllowsBinding(context), true);
assert.equal(walletRuntimeContextAllowsBinding({ ...context, isTopLevel: false }), false);
assert.equal(
  walletRuntimeGrantedAccount(runtime, context, "eip155:20", { required: false }),
  null,
  "eth_accounts must be empty before review",
);
assert.equal(
  walletRuntimeRequestedAccount(runtime, { chain_namespace: "eip155:8453" }).account_id,
  accountId,
);

runtime.pendingAccountAccess.set("wallet-approval:consent", {
  executionContextId: context.executionContextId,
  origin: context.pageOrigin,
  requestedChainNamespace: "eip155:20",
  chainNamespaces: ["eip155:20", "eip155:8453"],
  accountId,
  address,
});
walletRuntimeRecordAccountAccess(runtime, context, "wallet-approval:consent", {
  permission: "eth_accounts",
  principal_id: runtime.wallet.principal_id,
  session_id: runtime.wallet.session_id,
  launch_id: runtime.wallet.launch_id,
  origin: context.pageOrigin,
  requested_chain_namespace: "eip155:20",
  chain_namespaces: ["eip155:20", "eip155:8453"],
  account_id: accountId,
  address,
  grant_expires_at: now + 600,
});
assert.equal(walletRuntimeGrantedAccount(runtime, context, "eip155:20").address, address);
assert.equal(
  walletRuntimeGrantedAccount(runtime, { ...context, executionContextId: 8 }, "eip155:8453")
    .address,
  address,
  "same-origin reload keeps the reviewed grant",
);
assert.equal(
  walletRuntimeGrantedAccount(
    runtime,
    {
      ...context,
      executionContextId: 9,
      pageUrl: "https://other.example/",
      pageOrigin: "https://other.example",
    },
    "eip155:20",
    { required: false },
  ),
  null,
  "a different origin must not inherit the grant",
);

runtime.pendingAccountAccess.set("wallet-approval:tampered", {
  executionContextId: context.executionContextId,
  origin: context.pageOrigin,
  requestedChainNamespace: "eip155:20",
  chainNamespaces: ["eip155:20", "eip155:8453"],
  accountId,
  address,
});
assert.throws(
  () =>
    walletRuntimeRecordAccountAccess(runtime, context, "wallet-approval:tampered", {
      permission: "eth_accounts",
      principal_id: runtime.wallet.principal_id,
      session_id: runtime.wallet.session_id,
      launch_id: runtime.wallet.launch_id,
      origin: "https://attacker.example",
      requested_chain_namespace: "eip155:20",
      chain_namespaces: ["eip155:20", "eip155:8453"],
      account_id: accountId,
      address,
      grant_expires_at: now + 600,
    }),
  /did not match this Browser context/,
);

assert.deepEqual(
  await waitForWalletAccountAccess("wallet-approval:completed", now + 60, {
    getStatus: async () => ({ status: "completed", accounts: [address] }),
    now: () => now * 1000,
    wait: async () => {},
  }),
  [address],
);
await assert.rejects(
  waitForWalletAccountAccess("wallet-approval:rejected", now + 60, {
    getStatus: async () => ({ status: "rejected" }),
    now: () => now * 1000,
    wait: async () => {},
  }),
  (error) => error.code === 4001,
);

console.log("browser wallet account consent smoke: ok");
