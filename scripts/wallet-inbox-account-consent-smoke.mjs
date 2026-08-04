#!/usr/bin/env node
import assert from "node:assert/strict";
import fs from "node:fs";
import { createWalletRequests } from "../capsules/wallet/browser/wallet-requests.js";

class FakeClassList {
  constructor(owner) {
    this.owner = owner;
  }

  toggle(name, enabled) {
    const names = new Set(String(this.owner.className || "").split(/\s+/).filter(Boolean));
    enabled ? names.add(name) : names.delete(name);
    this.owner.className = [...names].join(" ");
  }
}

class FakeElement {
  constructor(tagName) {
    this.tagName = String(tagName).toUpperCase();
    this.children = [];
    this.className = "";
    this.classList = new FakeClassList(this);
    this.dataset = {};
    this.hidden = false;
    this.open = false;
    this.textContent = "";
  }

  append(...children) {
    this.children.push(...children);
  }

  replaceChildren(...children) {
    this.children = [...children];
  }

  scrollIntoView() {}
}

globalThis.document = {
  createElement: (tagName) => new FakeElement(tagName),
};
globalThis.window = {
  setTimeout: (callback) => callback(),
};

const requestsNode = new FakeElement("section");
const requestsPanelNode = new FakeElement("section");
const walletRequests = createWalletRequests({
  fetchJson: async () => ({}),
  notifyHomeSummaryChanged: () => {},
  openApprovalMethod: () => {},
  requestPasskeyStepUp: async () => "step-up:test",
  refreshWalletState: async () => {},
  requestsNode,
  requestsPanelNode,
  shellHeaders: (headers) => headers,
  showStatus: () => {},
});

const request = {
  request_id: "wallet-approval:account-consent",
  status: "pending",
  intent: "browser_account_access",
  capsule_id: "browser",
  reason: "Browser origin requests exact account access",
  account_id: "wallet:eip155:20:0x1111111111111111111111111111111111111111",
  address: "0x1111111111111111111111111111111111111111",
  proof_type: "managed_evm",
  created_at: Math.floor(Date.now() / 1000),
  expires_at: Math.floor(Date.now() / 1000) + 600,
  review: {
    kind: "account_access",
    permission: "eth_accounts",
    origin: "https://dapp.example",
    page_url: "https://dapp.example/connect",
    account_id: "wallet:eip155:20:0x1111111111111111111111111111111111111111",
    requested_chain_namespace: "eip155:20",
    chain_namespaces: ["eip155:20", "eip155:8453"],
    address: "0x1111111111111111111111111111111111111111",
    grant_expires_at: Math.floor(Date.now() / 1000) + 600,
  },
};

walletRequests.renderRequests([request]);

function descendants(node) {
  return [node, ...(node.children || []).flatMap(descendants)];
}

const rendered = descendants(requestsNode);
assert(rendered.some((node) => node.textContent === "Browser account access"));
assert(rendered.some((node) => node.textContent === "Review exact account access"));
assert(rendered.some((node) => node.textContent === "https://dapp.example"));
assert(rendered.some((node) => node.textContent === "eip155:20, eip155:8453"));
assert(rendered.some((node) => node.tagName === "BUTTON" && node.textContent === "Allow"));

walletRequests.renderRequests([{ ...request, review: null }]);
assert(
  !descendants(requestsNode).some(
    (node) => node.tagName === "BUTTON" && node.textContent === "Allow",
  ),
  "Wallet must not offer Allow without exact review details",
);

const inbox = fs.readFileSync(new URL("../capsules/inbox/browser/index.html", import.meta.url), "utf8");
const reviewBranch = inbox.slice(
  inbox.indexOf('actionId.startsWith("wallet-review-request:")'),
  inbox.indexOf('actionId.startsWith("capability-approve-request:")'),
);
assert(reviewBranch.includes('createButton("Review in Wallet"'));
assert(reviewBranch.includes('wallet_request: requestId'));
assert(!reviewBranch.includes("approveWalletRequest"));
assert(!reviewBranch.includes('createActionButton("Approve"'));

console.log("wallet/inbox account consent smoke: ok");
