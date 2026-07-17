#!/usr/bin/env node

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const parentOrigin = "http://localhost:61180";
let messageListener = null;
let delegatedRequest = null;
const parent = {
  postMessage(message, origin) {
    delegatedRequest = { message, origin };
    queueMicrotask(() => messageListener({
      source: parent,
      origin: parentOrigin,
      data: {
        type: "home:passkey-authority-result",
        requestId: message.requestId,
        homeToken: "fresh-wallet-token",
      },
    }));
  },
};

globalThis.document = { referrer: `${parentOrigin}/apps/home/` };
globalThis.window = {
  location: {
    origin: "null",
    search: `?home_origin=${encodeURIComponent(parentOrigin)}`,
  },
  parent,
  top: parent,
  crypto: { randomUUID: () => "fresh-authority-request" },
  setTimeout,
  clearTimeout,
  addEventListener(type, listener) {
    if (type === "message") messageListener = listener;
  },
  removeEventListener(type, listener) {
    if (type === "message" && messageListener === listener) messageListener = null;
  },
};

const { createWalletApi } = await import("../capsules/wallet/browser/wallet-api.js");
const walletApi = createWalletApi({ getHomeToken: () => "existing-wallet-token" });
const freshToken = await walletApi.requestFreshPasskeyHomeToken(
  "wallet.send",
  { account_id: "wallet:test", to: "0x1", amount: "1" },
);

assert.equal(freshToken, "fresh-wallet-token");
assert.equal(delegatedRequest.origin, parentOrigin);
assert.equal(delegatedRequest.message.homeToken, "existing-wallet-token");
assert.equal(
  walletApi.shellHeaders()["x-elastos-home-token"],
  "existing-wallet-token",
  "ordinary Wallet requests must retain their launch authority",
);
assert.equal(
  walletApi.shellHeaders({}, freshToken)["x-elastos-home-token"],
  "fresh-wallet-token",
  "the passkey-bound operation must use the fresh scoped token as request authority",
);

const source = (path) => readFileSync(new URL(`../${path}`, import.meta.url), "utf8");
const freshOperations = [
  ["capsules/wallet/browser/wallet-send-flow.js", ["wallet.send"], "headers"],
  [
    "capsules/wallet/browser/wallet-account-actions.js",
    ["wallet.account.delete", "wallet.recovery-key.export"],
    "headers",
  ],
  [
    "capsules/wallet/browser/wallet-create-account-flow.js",
    ["wallet.recovery-key.import"],
    "headers",
  ],
  ["capsules/wallet/browser/wallet-preferences.js", ["wallet.account.delete"], "headers"],
  ["capsules/wallet/browser/wallet-requests.js", ["wallet.approve"], "headers"],
  ["capsules/system/browser/system.js", ["auth.full-recovery-bundle.export"], "headers"],
  [
    "capsules/inbox/browser/index.html",
    ["wallet.approve", "inspect.approve"],
    "inbox-action",
  ],
];

for (const [path, operations, transport] of freshOperations) {
  const code = source(path);
  const calls = [...code.matchAll(/\bawait\s+requestFreshPasskeyHomeToken\s*\(/g)];
  assert.equal(
    calls.length,
    operations.length,
    `${path} must enumerate every fresh-passkey operation in this regression`,
  );
  calls.forEach((call, index) => {
    const end = calls[index + 1]?.index ?? code.length;
    const operation = operations[index];
    const segment = code.slice(call.index, end);
    assert.match(
      segment,
      new RegExp(`requestFreshPasskeyHomeToken\\s*\\(\\s*["']${operation.replaceAll(".", "\\.")}["']`),
      `${path} fresh-passkey call ${index + 1} must remain ${operation}`,
    );
    assert.match(
      segment,
      /home_token:\s*homeToken/,
      `${path} ${operation} must bind the fresh token in its request body`,
    );
    if (transport === "headers") {
      assert.match(
        segment,
        /headers:\s*shellHeaders\([\s\S]{0,160}?homeToken\)/,
        `${path} ${operation} must send the fresh token as request authority`,
      );
    } else {
      assert.match(
        segment,
        /inboxAction\([\s\S]{0,300}?},\s*homeToken\)/,
        `${path} ${operation} must send the fresh token through Inbox request authority`,
      );
    }
  });
}

console.log("[home-fresh-passkey-authority] PASS");
