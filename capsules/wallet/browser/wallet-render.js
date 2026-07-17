import {
  accountDisplayBalance,
  assetColor,
  formatAmount,
  shortAddress,
} from "./wallet-format.js?v=wallet-20260717a";

export function createWalletRender({ statusNode }) {
  function showStatus(message, tone) {
    if (!statusNode) {
      return;
    }
    const text = typeof message === "string" ? message.trim() : "";
    statusNode.hidden = text.length === 0;
    statusNode.textContent = text;
    statusNode.dataset.tone = tone || "muted";
  }

  return {
    showStatus,
    setBusy,
  };
}

export function textNode(tag, text, className = "") {
  const node = document.createElement(tag);
  if (className) {
    node.className = className;
  }
  node.textContent = text;
  return node;
}

export function accountCard(account, displayedAccountId = "", { privacyMode, prices, displayCurrency }) {
  const card = document.createElement("article");
  card.className = "wallet-account";
  card.classList.toggle("is-selected", account.account_id === displayedAccountId);
  card.tabIndex = 0;
  card.dataset.walletAccountDetail = account.account_id;

  const top = document.createElement("div");
  top.className = "wallet-account-top";
  const title = document.createElement("div");
  title.className = "wallet-card-title";
  title.append(
    textNode("strong", account.name, "wallet-card-name"),
    textNode("span", shortAddress(account.address), "wallet-card-address"),
  );
  top.append(methodMark(account.method, account.monogram), title);

  const more = document.createElement("button");
  more.className = "wallet-more-button";
  more.type = "button";
  more.textContent = "⋯";
  more.dataset.walletAccountMenu = account.account_id;
  more.setAttribute("aria-label", `Account actions for ${account.name}`);
  top.append(more);

  const balance = document.createElement("div");
  balance.className = "wallet-card-balance";
  balance.textContent = privacyMode ? "••••••" : accountDisplayBalance(account, prices, displayCurrency);

  const network = document.createElement("div");
  network.className = "wallet-account-network";
  network.textContent = account.network;

  const footer = document.createElement("div");
  footer.className = "wallet-card-footer";
  for (const asset of account.assets.slice(0, 3)) {
    footer.append(assetChip(asset));
  }

  card.append(top, balance, network, footer);
  return card;
}

export function emptyHero() {
  const empty = document.createElement("div");
  empty.className = "wallet-empty";
  empty.innerHTML = `
    <p class="wallet-state">No accounts yet. Create an EVM or Bitcoin account from Accounts, or import a Wallet key.</p>
  `;
  return empty;
}

export function assetChip(asset) {
  const chip = document.createElement("span");
  chip.className = "wallet-chip";
  chip.append(assetGlyph(asset.symbol), document.createTextNode(`${formatAmount(asset.amount)} ${asset.symbol}`));
  return chip;
}

export function assetGlyph(symbol) {
  const glyph = document.createElement("span");
  glyph.className = "wallet-asset-glyph";
  glyph.textContent = symbol === "BTC" ? "₿" : symbol.slice(0, 1);
  glyph.style.color = assetColor(symbol);
  return glyph;
}

export function methodMark(method, monogram, large = false) {
  const mark = document.createElement("span");
  mark.className = `wallet-method-mark wallet-method-${method}${large ? " wallet-method-mark-large" : ""}`;
  mark.textContent = monogram || "?";
  mark.setAttribute("aria-hidden", "true");
  return mark;
}

export function copyButton(value) {
  const button = document.createElement("button");
  button.className = "wallet-copy-button";
  button.type = "button";
  button.textContent = shortAddress(value);
  button.dataset.walletCopyAddress = value;
  return button;
}

export function pulseCopied(button) {
  const previous = button.textContent;
  button.textContent = "Copied";
  button.classList.add("is-copied");
  window.setTimeout(() => {
    button.textContent = previous;
    button.classList.remove("is-copied");
  }, 1200);
}

export function actionButton(label, dataKey, dataValue, secondary = false) {
  const button = document.createElement("button");
  button.className = secondary ? "wallet-button wallet-button-secondary" : "wallet-button";
  button.type = "button";
  button.textContent = label;
  button.dataset[dataKey] = dataValue;
  return button;
}

export function setBusy(button, busy) {
  if (button) {
    button.disabled = Boolean(busy);
  }
}
