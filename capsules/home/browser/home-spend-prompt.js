// Home-chrome confirmation for node-signed money verbs.
//
// WHY THIS EXISTS. `runMoneyVerb` signs the body a capsule frame handed it. The step-up ceremony
// binds that body cryptographically, so nothing can be altered AFTER the ceremony — but on its own
// a ceremony proves only "a human touched the authenticator", not "a human agreed to THIS spend".
// A compromised marketplace frame could render one asset and post a different `content_id` or
// `seller`; the browser's own verification UI names no terms, so the user would confirm a purchase
// they never saw. `expected_price` bounds price drift, and nothing bounds substitution.
//
// So Home shows the intent itself, in DOM Home owns, before the ceremony starts.
//
// THE INVARIANT THAT MAKES THIS WORTH ANYTHING: what is displayed is what is signed. The signed
// body is the WHOLE request object, so this renders EVERY key in it — known fields first, with
// friendly labels and a fixed order, then any remaining key under its own raw name. A frame cannot
// smuggle a term past the dialog by inventing a field name the display does not know, because
// unknown fields are shown, not skipped.
//
// Rendering goes through `textContent` and `createElement` ONLY — never any markup-parsing sink.
// The frame supplies these strings, so it may contribute text and nothing else. The entropy check
// `moneyVerbIntentConfirmation.escapedTermsOnly` greps this file for those sinks by name, which is
// also why none of them is named here.

const MAX_VALUE_CHARS = 160;
const MAX_ROWS = 24;

// Fields Home understands, in display order. Everything else still renders, under its raw key.
const KNOWN_FIELDS = [
  ["content_id", "Asset"],
  ["uri", "Item"],
  ["seller", "Seller"],
  ["quantity", "Quantity"],
  ["expected_price", "Price"],
  ["expected_pay_token", "Pay with"],
  ["ledger", "Channel"],
  ["operative", "Operative"],
  ["token_id", "Token id"],
  ["to", "Mint to"],
  ["token_uri", "Metadata"],
  ["op_type_code", "Operation type"],
];

const OPERATION_COPY = Object.freeze({
  "market.buy": {
    title: "Confirm purchase",
    summary:
      "This node will sign and broadcast a purchase with your managed wallet. Check the terms "
      + "below — they are exactly what will be signed.",
    action: "Confirm purchase",
  },
  "create.mint": {
    title: "Confirm mint",
    summary:
      "This node will sign and broadcast a mint with your managed wallet. Check the terms below "
      + "— they are exactly what will be signed.",
    action: "Confirm mint",
  },
});

/// The refusal a cancelled confirmation produces. Distinct from every step-up refusal so the
/// calling frame can tell "you declined" from "verification failed" and say so.
export const SPEND_DECLINED_MESSAGE = "You declined this spend in Home.";

export function moneyVerbIntentRows(request) {
  const source = request && typeof request === "object" && !Array.isArray(request) ? request : {};
  const seen = new Set();
  const rows = [];
  for (const [key, label] of KNOWN_FIELDS) {
    if (Object.prototype.hasOwnProperty.call(source, key)) {
      seen.add(key);
      rows.push([label, displayValue(source[key])]);
    }
  }
  for (const key of Object.keys(source)) {
    if (seen.has(key)) {
      continue;
    }
    // Deliberately shown, not hidden: an unrecognized field is still signed, so the user must be
    // able to see it. The raw key is the honest label — Home does not know what it means.
    rows.push([key, displayValue(source[key])]);
  }
  return rows.slice(0, MAX_ROWS);
}

function displayValue(value) {
  if (value === null || value === undefined) {
    return "—";
  }
  const text = typeof value === "string" ? value : safeStringify(value);
  const collapsed = text.replace(/\s+/g, " ").trim();
  if (collapsed === "") {
    return "—";
  }
  return collapsed.length > MAX_VALUE_CHARS
    ? `${collapsed.slice(0, MAX_VALUE_CHARS)}…`
    : collapsed;
}

function safeStringify(value) {
  try {
    return JSON.stringify(value) ?? String(value);
  } catch (_error) {
    return "(unreadable)";
  }
}

export function createHomeSpendPrompt({
  root,
  title,
  summary,
  terms,
  allowButton,
  cancelButton,
} = {}) {
  if (!root || !title || !summary || !terms || !allowButton || !cancelButton) {
    throw new TypeError("Home spend prompt nodes are required");
  }
  let active = null;

  function hide() {
    root.hidden = true;
    root.setAttribute("aria-hidden", "true");
    terms.replaceChildren();
  }

  function settle(confirmed) {
    const request = active;
    if (!request) {
      return;
    }
    active = null;
    hide();
    request.resolve(confirmed);
  }

  allowButton.addEventListener("click", () => settle(true));
  cancelButton.addEventListener("click", () => settle(false));
  // Escape cancels, matching every other Home dialog. Cancel is the safe direction, so this can
  // only ever refuse a spend.
  root.addEventListener("keydown", (event) => {
    if (event.key === "Escape") {
      settle(false);
    }
  });

  return {
    /// Resolves `true` only on an explicit click of the confirm button.
    confirm(operation, request) {
      const copy = OPERATION_COPY[operation];
      if (!copy) {
        // An operation with no written confirmation copy cannot be confirmed. Fail closed rather
        // than show a spend the user cannot read.
        return Promise.resolve(false);
      }
      if (active) {
        // One spend at a time. A second request while a confirmation is open is refused outright
        // rather than queued — queuing lets a frame stack dialogs until one is clicked through.
        return Promise.resolve(false);
      }
      title.textContent = copy.title;
      summary.textContent = copy.summary;
      allowButton.textContent = copy.action;
      terms.replaceChildren();
      for (const [label, value] of moneyVerbIntentRows(request)) {
        const term = document.createElement("dt");
        term.textContent = label;
        const detail = document.createElement("dd");
        detail.textContent = value;
        terms.append(term, detail);
      }
      root.hidden = false;
      root.setAttribute("aria-hidden", "false");
      try {
        cancelButton.focus({ preventScroll: true });
      } catch (_error) {
        /* focus is a nicety, not a gate */
      }
      return new Promise((resolve) => {
        active = { resolve };
      });
    },
  };
}
