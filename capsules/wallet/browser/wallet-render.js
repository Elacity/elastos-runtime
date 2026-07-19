import {
  accountDisplayBalance,
  assetColor,
  formatAmount,
  shortAddress,
} from "./wallet-format.js?v=wallet-20260719w";

const METHOD_ICON_SRC = Object.freeze({
  metamask: "./icons/metamask.png",
  btc: "./icons/unisat.png",
  ethereum: "./icons/ethereum.png",
  bitcoin: "./icons/bitcoin.png",
  passkey: "./icons/passkey.png",
});

export function createWalletRender({ statusNode }) {
  let statusClearTimer = 0;

  function showStatus(message, tone = "muted") {
    if (!statusNode) {
      return;
    }
    if (statusClearTimer) {
      window.clearTimeout(statusClearTimer);
      statusClearTimer = 0;
    }
    const raw = typeof message === "string" ? message.trim() : String(message ?? "").trim();
    if (!raw) {
      statusNode.hidden = true;
      statusNode.textContent = "";
      delete statusNode.dataset.tone;
      return;
    }
    const text = publicWalletText(raw);
    statusNode.hidden = text.length === 0;
    statusNode.textContent = text;
    statusNode.dataset.tone = tone || "muted";
    if (!text) {
      return;
    }
    // Brief confirmations shouldn't pin the hero open; errors linger longer.
    const dismissMs = tone === "error" ? 8000 : tone === "success" ? 3200 : 4000;
    statusClearTimer = window.setTimeout(() => {
      if (statusNode.textContent === text) {
        statusNode.hidden = true;
        statusNode.textContent = "";
        delete statusNode.dataset.tone;
      }
      statusClearTimer = 0;
    }, dismissMs);
  }

  return {
    showStatus,
    setBusy,
  };
}

export function publicWalletText(value, fallback = "Wallet action could not be completed.") {
  const message = String(value || "").trim();
  if (!message || /\b(schema|projection|provider|adapter|capability|affordance|runtime-owned|launch token|hostcall|request failed|failed to fetch|unauthorized|forbidden|[45]\d\d)\b|engine_[a-z_]+/i.test(message)) {
    return fallback;
  }
  return message;
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
  const identity = document.createElement("div");
  identity.className = "wallet-account-identity";
  const title = document.createElement("div");
  title.className = "wallet-card-title";
  title.append(
    textNode("strong", account.name, "wallet-card-name"),
    textNode("span", shortAddress(account.address), "wallet-card-address"),
  );
  identity.append(
    methodMark(account.method, account.monogram, false, account.chain_namespace),
    title,
  );

  const balance = document.createElement("div");
  balance.className = "wallet-card-balance";
  balance.textContent = privacyMode ? "••••••" : accountDisplayBalance(account, prices, displayCurrency);

  const more = document.createElement("button");
  more.className = "wallet-more-button";
  more.type = "button";
  more.textContent = "⋯";
  more.dataset.walletAccountMenu = account.account_id;
  more.setAttribute("aria-label", `Account actions for ${account.name}`);

  top.append(identity, balance, more);

  const meta = document.createElement("div");
  meta.className = "wallet-account-meta";
  meta.append(textNode("span", account.network, "wallet-account-network"));
  for (const asset of account.assets.slice(0, 3)) {
    meta.append(assetChip(asset));
  }

  card.append(top, meta);
  return card;
}

export function emptyHero() {
  const empty = document.createElement("div");
  empty.className = "wallet-empty";
  empty.innerHTML = `
    <p class="wallet-state">No accounts yet. Create a built-in account, or connect MetaMask / UniSat below.</p>
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

export function methodMarkIconSrc(method, chainNamespace = "") {
  if (method === "metamask") {
    return METHOD_ICON_SRC.metamask;
  }
  if (method === "btc") {
    return METHOD_ICON_SRC.btc;
  }
  if (method === "passkey") {
    const namespace = String(chainNamespace || "");
    if (namespace.startsWith("eip155:")) {
      return METHOD_ICON_SRC.ethereum;
    }
    if (namespace.startsWith("bip122:")) {
      return METHOD_ICON_SRC.bitcoin;
    }
    // Built-in accounts header / passkey without a chain → passkey mark.
    return METHOD_ICON_SRC.passkey;
  }
  return "";
}

export function methodMark(method, monogram, large = false, chainNamespace = "") {
  const mark = document.createElement("span");
  const iconSrc = methodMarkIconSrc(method, chainNamespace);
  const markKind = iconSrc
    ? method === "passkey" && String(chainNamespace).startsWith("bip122:")
      ? "bitcoin"
      : method === "passkey" && String(chainNamespace).startsWith("eip155:")
        ? "ethereum"
        : method
    : method;
  mark.className = [
    "wallet-method-mark",
    `wallet-method-${markKind}`,
    large ? "wallet-method-mark-large" : "",
    iconSrc ? "wallet-method-mark-icon" : "",
  ].filter(Boolean).join(" ");
  mark.setAttribute("aria-hidden", "true");
  if (iconSrc) {
    const img = document.createElement("img");
    img.src = iconSrc;
    img.alt = "";
    img.draggable = false;
    mark.append(img);
  } else {
    mark.textContent = monogram || "?";
  }
  return mark;
}

export const COPY_ICON_SVG =
  '<svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true"><rect x="5.5" y="5.5" width="8" height="8" rx="1.5" fill="none" stroke="currentColor" stroke-width="1.4"/><path d="M3.5 10.5V3.5A1 1 0 0 1 4.5 2.5h7" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round"/></svg>';

export const CHECK_ICON_SVG =
  '<svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true"><path d="M3.25 8.25l3 3 6.5-6.5" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"/></svg>';

export function copyButton(value) {
  const button = document.createElement("button");
  button.className = "wallet-copy-button";
  button.type = "button";
  button.textContent = shortAddress(value);
  button.dataset.walletCopyAddress = value;
  return button;
}

export function copyIconButton(address, ariaLabel = "Copy address") {
  const copy = document.createElement("button");
  copy.className = "wallet-copy-icon";
  copy.type = "button";
  copy.setAttribute("aria-label", ariaLabel);
  copy.title = "Copy address";
  copy.dataset.walletCopyAddress = address;
  copy.innerHTML = COPY_ICON_SVG;
  return copy;
}

export function pulseCopied(button) {
  button.classList.add("is-copied");
  if (button.classList.contains("wallet-copy-icon")) {
    const previousLabel = button.getAttribute("aria-label") || "Copy address";
    const previousTitle = button.title || "Copy address";
    const previousHtml = button.innerHTML;
    button.innerHTML = CHECK_ICON_SVG;
    button.setAttribute("aria-label", "Copied");
    button.title = "Copied";
    window.setTimeout(() => {
      button.classList.remove("is-copied");
      button.innerHTML = previousHtml || COPY_ICON_SVG;
      button.setAttribute("aria-label", previousLabel);
      button.title = previousTitle;
    }, 1200);
    return;
  }
  const previous = button.textContent;
  button.textContent = "Copied";
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
