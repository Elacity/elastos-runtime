// The market listing contract, as Runtime publishes it, and the buy_offer
// answer contract, as Runtime answers it.
//
// This module is the consumer half of two written agreements: the
// `elastos.marketplace.listing/v1` object Runtime publishes for one market
// item, and the answers of the `buy_offer` object provider operation. The
// producer writes a fixture of each (`fixtures/`), and the fixture tests
// beside this file hold the two sides together.
//
// `parseMarketListing` follows the same discipline `listing.js` established
// for `parseRuntimeCustodyListing`: every field the producer may send is
// named here, and an unnamed field -- at the top level or inside `item`,
// `asset`, an offer, or `source` -- is refused rather than ignored, because a
// field this parser does not know about is a field it cannot have decided is
// safe. Unlike that parser, this one never throws: a market listing is shown
// item by item, one bad listing is one row the caller declines to render, so
// the return shape is `{ok, listing}` or `{ok: false, reason}`.
//
// `parseBuyOfferAnswer` reads the same wait/fail envelope `buyOutcomeFromAnswer`
// already reads (`buy_progress` carrying `schema`, `stage`, `resumable`,
// `awaits_person`, `connector_id`), plus the typed codes `buy_offer` adds:
// `terms_changed` and `attempt_in_progress` (each carrying `current`, an
// offer or null), `own_offer`, `already_owned`, `asset_mismatch`, and the
// unavailable refusal (R48: Runtime could not reach the market). `complete`
// is never a default: it is returned only for the exact
// `elastos.marketplace.buy-offer-complete/v1` envelope -- `schema`, the
// `item` bought, `asset_uri`, and `adoption` -- because a market purchase of
// a foreign item has no mint to report. Everything else -- `{}`, `null`, a
// non-object, an unrecognised `code`, `buy_progress` that fails its own
// shape check, the mint-based terminal shape of `buy` -- is `failed`. This
// matters because `{}` is exactly what `postObjectProvider` can hand this
// parser on a genuine failure (its body parse falls back to `{}`, and
// `providerFailure` coerces a non-object answer to `{}` too), so `{}`
// reaching here is not evidence of anything and must never be read as a
// purchase having gone through.
//
// uint256 values stay canonical lowercase hex strings exactly as Runtime
// sends them, never JavaScript numbers. Free text tied to
// `bounded_directory_text` (`api/gateway_marketplace_directory.rs`) --
// `asset.title/description/category/mime_type` -- is bounded the same way
// that function is: by Unicode scalar value count, not UTF-8 byte length.

export const MARKET_LISTING_SCHEMA_V1 = "elastos.marketplace.listing/v1";
export const BUY_OFFER_PROGRESS_SCHEMA_V1 = "elastos.protected-content.buy-progress/v1";
// `buy_offer`'s terminal success envelope: the item bought, the asset it
// names, and how far adoption got. Exported so
// a caller that wants to recognise this shape outside `parseBuyOfferAnswer`
// names the same schema rather than a copy of it.
export const BUY_OFFER_COMPLETE_SCHEMA_V1 = "elastos.marketplace.buy-offer-complete/v1";
export const BUY_OFFER_ADOPTION_STATES = ["adopted", "foreign", "pending"];
// `unknown` (R46): Runtime could not read the asset's metadata or its KID
// binding when it answered. A purchase rests on the chain's terms alone, so
// an unknown readability is advisory like the others and never a gate.
export const MARKET_READABILITY_STATES = ["verified", "unverified", "foreign", "unknown"];
export const MARKET_ACCESS_STATES = ["available", "purchased", "creator"];
export const MARKET_SOURCE_STARTS = ["token_uri", "kid", "item"];
export const MAX_MARKET_OFFERS = 32; // Runtime's PROTECTED_CONTENT_OFFERS_MAX

const MAX_TEXT_BYTES = 256;
// `bounded_directory_text`'s own cap, in Unicode scalar values
// (`api/gateway_marketplace_directory.rs` MARKET_DIRECTORY_MAX_TEXT_CHARS).
const MAX_DIRECTORY_TEXT_CHARS = 256;

// uint256 in hex: lowercase, no leading zero except the value zero itself.
const UINT256_HEX = /^0x(?:0|[1-9a-f][0-9a-f]{0,63})$/;
const ADDRESS_HEX = /^0x[0-9a-f]{40}$/;
// `0x` plus the 32 hex digits of a content id (protected_content_elacity_metadata.rs).
const KID_HEX = /^0x[0-9a-f]{32}$/;
const CONTENT_CID = /^(Qm[1-9A-HJ-NP-Za-km-z]{44}|b[a-z2-7]{45,95})$/;
// `elastos://` plus exactly the same CIDv0/CIDv1 shape `CONTENT_CID` accepts
// -- an asset's URI names a content-addressed folder, never an arbitrary
// alphanumeric string.
const ASSET_URI = /^elastos:\/\/(?:Qm[1-9A-HJ-NP-Za-km-z]{44}|b[a-z2-7]{45,95})$/;

const TOP_KEYS = ["schema", "item", "asset", "offers", "access_state", "source"];
const TOP_OPTIONAL_KEYS = ["offers_truncated", "access_unknown"];
const ITEM_KEYS = ["chain_namespace", "network", "ledger", "token_id", "operative"];
const ITEM_OPTIONAL_KEYS = ["kid"];
const ASSET_KEYS = ["uri", "title", "description", "cover_cid", "category", "mime_type", "readability"];
const OFFER_KEYS = ["seller", "quantity", "price", "pay_token", "payment_processor"];
const SOURCE_KEYS = ["start", "read_at_block"];
const BUY_OFFER_COMPLETE_KEYS = ["schema", "item", "asset_uri", "adoption"];

const BUY_OFFER_STAGES = [
  "allowance_approval",
  "purchase_approval",
  "chain_settlement",
  "access_evidence",
  "declined",
];
// The typed answers that carry `current`: an offer, or `null`.
// `terms_changed`: the seller's fresh offer, `null` once it is gone.
// `attempt_in_progress`: the offer an earlier purchase of this item was
// recorded against, `null` when that purchase went through another path.
const BUY_OFFER_OFFER_CODES = ["terms_changed", "attempt_in_progress"];
// The typed refusals carrying nothing past their own name.
const BUY_OFFER_BARE_CODES = ["own_offer", "already_owned", "asset_mismatch"];
// R48: a `buy_offer` refused because Runtime could not reach the market or
// the chain. `provider_error_from` carries it under the generic
// `library_error` code with this stable sentence
// (`RUNTIME_CUSTODY_PURCHASE_UNAVAILABLE_MESSAGE`, protected_content_runtime.rs)
// and no typed field of its own, so this exact pair is what names it; a
// typed `unavailable` code, as the listing endpoint uses, names it too.
const BUY_OFFER_UNAVAILABLE_CODE = "library_error";
const BUY_OFFER_UNAVAILABLE_MESSAGE = "Runtime custody purchase is unavailable";

/**
 * Parse one `ListingObject`, exactly as Runtime published it.
 *
 * @returns {{ok: true, listing: object} | {ok: false, reason: string}}
 */
export function parseMarketListing(value) {
  const topError = keysError(value, TOP_KEYS, TOP_OPTIONAL_KEYS);
  if (topError) {
    return fail(topError);
  }
  if (value.schema !== MARKET_LISTING_SCHEMA_V1) {
    return fail("invalid market listing schema");
  }
  const item = parseMarketItem(value.item);
  if (!item.ok) {
    return item;
  }
  const asset = parseMarketAsset(value.asset);
  if (!asset.ok) {
    return asset;
  }
  if (!Array.isArray(value.offers) || value.offers.length > MAX_MARKET_OFFERS) {
    return fail("invalid market listing offers");
  }
  const offers = [];
  for (const rawOffer of value.offers) {
    const offer = parseMarketOffer(rawOffer);
    if (!offer.ok) {
      return offer;
    }
    offers.push(offer.offer);
  }
  if (!MARKET_ACCESS_STATES.includes(value.access_state)) {
    return fail("invalid market listing access state");
  }
  const source = parseMarketSource(value.source);
  if (!source.ok) {
    return source;
  }
  if ("offers_truncated" in value && value.offers_truncated !== true) {
    return fail("invalid market listing truncation flag");
  }
  if ("access_unknown" in value && value.access_unknown !== true) {
    return fail("invalid market listing access-unknown flag");
  }

  const listing = {
    schema: value.schema,
    item: item.item,
    asset: asset.asset,
    offers,
    accessState: value.access_state,
    source: source.source,
  };
  if (value.offers_truncated === true) {
    listing.offersTruncated = true;
  }
  if (value.access_unknown === true) {
    listing.accessUnknown = true;
  }
  return { ok: true, listing };
}

/**
 * The typed answer to `buy_offer`, read from either the success payload or
 * the failure `error.answer` `postObjectProvider` throws (see
 * `marketplace.js`'s `postObjectProvider` / `providerFailure`).
 *
 * @returns {{kind: "complete"|"wait"|"terms_changed"|"attempt_in_progress"|"own_offer"|"already_owned"|"asset_mismatch"|"unavailable"|"failed", ...}}
 */
export function parseBuyOfferAnswer(value) {
  const answer = value && typeof value === "object" && !Array.isArray(value) ? value : {};
  const code = typeof answer.code === "string" ? answer.code : "";

  if (BUY_OFFER_OFFER_CODES.includes(code)) {
    if (answer.current === null) {
      return { kind: code, current: null };
    }
    const current = parseMarketOffer(answer.current);
    // `current` is still an offer, validated like one. Terms this parser
    // cannot read are not terms it can show, and showing nothing trustworthy
    // as "the new terms" or "the purchase in progress" would be worse than
    // refusing the whole answer. A missing `current` is refused the same way.
    if (!current.ok) {
      return failedAnswer();
    }
    return { kind: code, current: current.offer };
  }
  if (BUY_OFFER_BARE_CODES.includes(code)) {
    return { kind: code };
  }

  const progress = answer.buy_progress;
  if (
    progress
    && typeof progress === "object"
    && progress.schema === BUY_OFFER_PROGRESS_SCHEMA_V1
    && BUY_OFFER_STAGES.includes(progress.stage)
    && typeof progress.resumable === "boolean"
    && typeof progress.awaits_person === "boolean"
  ) {
    const connectorId = typeof progress.connector_id === "string" ? progress.connector_id : "";
    if (progress.resumable) {
      return { kind: "wait", stage: progress.stage, awaitsPerson: progress.awaits_person, connectorId };
    }
    return { kind: "failed", stage: progress.stage, awaitsPerson: false, connectorId };
  }

  if (
    code === "unavailable"
    || (code === BUY_OFFER_UNAVAILABLE_CODE && answer.message === BUY_OFFER_UNAVAILABLE_MESSAGE)
  ) {
    return { kind: "unavailable" };
  }

  const complete = parseBuyOfferComplete(answer);
  if (complete) {
    return { kind: "complete", item: complete.item, assetUri: complete.assetUri, adoption: complete.adoption };
  }
  // Nothing positively recognised this answer as the terminal success
  // envelope. That covers `{}`, `null`, a non-object, an unrecognised
  // `code`, `buy_progress` that failed its own shape check, and the old
  // mint-based terminal shape -- none of which is evidence a purchase went
  // through.
  return failedAnswer();
}

// `buy_offer`'s terminal success answer: `schema`, the `item` bought,
// `asset_uri`, and `adoption`. Returns `{item, assetUri, adoption}` on a
// match, or `null` -- never throws, never guesses.
function parseBuyOfferComplete(answer) {
  const keyError = keysError(answer, BUY_OFFER_COMPLETE_KEYS, []);
  if (keyError || answer.schema !== BUY_OFFER_COMPLETE_SCHEMA_V1) {
    return null;
  }
  const item = buyOfferCompleteItem(answer.item);
  if (!item) {
    return null;
  }
  if (!ASSET_URI.test(String(answer.asset_uri ?? ""))) {
    return null;
  }
  if (!BUY_OFFER_ADOPTION_STATES.includes(answer.adoption)) {
    return null;
  }
  return { item, assetUri: answer.asset_uri, adoption: answer.adoption };
}

// The item a completed `buy_offer` names: a listing's item, read by the same
// `parseMarketItem`. Its `kid` may be null or absent (R46): a purchase rests
// on the chain's terms alone, and a paid purchase must never read as a
// failed one because Runtime could not learn the KID. Returns the parsed
// item or `null`.
function buyOfferCompleteItem(value) {
  const item = parseMarketItem(value);
  return item.ok ? item.item : null;
}

/** Stable per-item key for page state: `"<chain>|<ledger>|<token_id>"`. */
export function marketItemKey(item) {
  if (!item || typeof item !== "object") {
    return "||";
  }
  const chain = String(item.chainNamespace ?? item.chain_namespace ?? "");
  const ledger = String(item.ledger ?? "");
  const tokenId = String(item.tokenId ?? item.token_id ?? "");
  return `${chain}|${ledger}|${tokenId}`;
}

/**
 * The item as a request names it (Runtime's `RuntimeMarketBuyItemClaim`):
 * `buy_offer` and `download_owned_copy` both send exactly this. There is no
 * `operative`, because Runtime derives it, and `kid` appears only when the
 * listing bound one -- a null kid (R46) is left out, never sent as null.
 */
export function marketItemClaim(item) {
  const claim = {
    chain_namespace: item.chainNamespace,
    network: item.network,
    ledger: item.ledger,
    token_id: item.tokenId,
  };
  if (item.kid) {
    claim.kid = item.kid;
  }
  return claim;
}

/**
 * The offer rows a sheet shows while Runtime holds a purchase recorded on
 * `recorded`'s terms (`attempt_in_progress` with a `current` offer). That
 * attempt can move on only when the person presses Continue on its exact
 * terms, so it keeps a row even after its seller's live offer is gone or
 * changed: the recorded offer replaces a row marked `_gone`, or is added
 * after the live rows when its seller has none. `recorded` null (Runtime
 * named no seller) adds nothing. The listing's own `offers` stay unchanged.
 */
export function offersWithRecordedAttempt(offers, recorded) {
  if (!recorded) {
    return offers;
  }
  const index = offers.findIndex((entry) => entry.seller === recorded.seller);
  if (index === -1) {
    return [...offers, recorded];
  }
  if (offers[index]._gone) {
    return offers.map((entry, at) => (at === index ? recorded : entry));
  }
  return offers;
}

function parseMarketItem(value) {
  const keyError = keysError(value, ITEM_KEYS, ITEM_OPTIONAL_KEYS);
  if (keyError) {
    return fail(keyError);
  }
  const chainNamespace = boundedText(value.chain_namespace, MAX_TEXT_BYTES);
  if (chainNamespace === null) {
    return fail("invalid market listing item chain namespace");
  }
  const network = boundedText(value.network, MAX_TEXT_BYTES);
  if (network === null) {
    return fail("invalid market listing item network");
  }
  if (!ADDRESS_HEX.test(String(value.ledger ?? ""))) {
    return fail("invalid market listing item ledger");
  }
  if (!UINT256_HEX.test(String(value.token_id ?? ""))) {
    return fail("invalid market listing item token id");
  }
  if (!ADDRESS_HEX.test(String(value.operative ?? ""))) {
    return fail("invalid market listing item operative");
  }
  const item = {
    chainNamespace,
    network,
    ledger: value.ledger,
    tokenId: value.token_id,
    operative: value.operative,
  };
  // `kid` may be absent, or null when Runtime could not learn it (R46).
  if (value.kid === null) {
    item.kid = null;
  } else if ("kid" in value) {
    if (!KID_HEX.test(String(value.kid ?? ""))) {
      return fail("invalid market listing item kid");
    }
    item.kid = value.kid;
  }
  return { ok: true, item };
}

function parseMarketAsset(value) {
  const keyError = keysError(value, ASSET_KEYS, []);
  if (keyError) {
    return fail(keyError);
  }
  if (!ASSET_URI.test(String(value.uri ?? ""))) {
    return fail("invalid market listing asset uri");
  }
  const title = boundedDirectoryText(value.title, MAX_DIRECTORY_TEXT_CHARS);
  if (title === null) {
    return fail("invalid market listing asset title");
  }
  const description = boundedDirectoryText(value.description, MAX_DIRECTORY_TEXT_CHARS);
  if (description === null) {
    return fail("invalid market listing asset description");
  }
  let coverCid = null;
  if (value.cover_cid !== null) {
    if (typeof value.cover_cid !== "string" || !CONTENT_CID.test(value.cover_cid)) {
      return fail("invalid market listing asset cover");
    }
    coverCid = value.cover_cid;
  }
  const category = boundedDirectoryText(value.category, MAX_DIRECTORY_TEXT_CHARS);
  if (category === null) {
    return fail("invalid market listing asset category");
  }
  const mimeType = boundedDirectoryText(value.mime_type, MAX_DIRECTORY_TEXT_CHARS);
  if (mimeType === null) {
    return fail("invalid market listing asset mime type");
  }
  if (!MARKET_READABILITY_STATES.includes(value.readability)) {
    return fail("invalid market listing asset readability");
  }
  return {
    ok: true,
    asset: {
      uri: value.uri,
      title,
      description,
      coverCid,
      category,
      mimeType,
      readability: value.readability,
    },
  };
}

function parseMarketOffer(value) {
  const keyError = keysError(value, OFFER_KEYS, []);
  if (keyError) {
    return fail(keyError);
  }
  if (!ADDRESS_HEX.test(String(value.seller ?? ""))) {
    return fail("invalid market listing offer seller");
  }
  if (!UINT256_HEX.test(String(value.quantity ?? ""))) {
    return fail("invalid market listing offer quantity");
  }
  if (!UINT256_HEX.test(String(value.price ?? ""))) {
    return fail("invalid market listing offer price");
  }
  if (!ADDRESS_HEX.test(String(value.pay_token ?? ""))) {
    return fail("invalid market listing offer pay token");
  }
  // Always present (R16): `null` for a native-token offer, which pays
  // through no processor, and a canonical address otherwise. A missing key
  // is refused by `keysError` above, like every other required field.
  if (
    value.payment_processor !== null
    && (typeof value.payment_processor !== "string" || !ADDRESS_HEX.test(value.payment_processor))
  ) {
    return fail("invalid market listing offer payment processor");
  }
  return {
    ok: true,
    offer: {
      seller: value.seller,
      quantity: value.quantity,
      price: value.price,
      payToken: value.pay_token,
      paymentProcessor: value.payment_processor,
    },
  };
}

function parseMarketSource(value) {
  const keyError = keysError(value, SOURCE_KEYS, []);
  if (keyError) {
    return fail(keyError);
  }
  if (!MARKET_SOURCE_STARTS.includes(value.start)) {
    return fail("invalid market listing source start");
  }
  if (!UINT256_HEX.test(String(value.read_at_block ?? ""))) {
    return fail("invalid market listing source block");
  }
  return { ok: true, source: { start: value.start, readAtBlock: value.read_at_block } };
}

function keysError(value, required, optional) {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    return "invalid market listing object";
  }
  const known = new Set([...required, ...optional]);
  for (const key of Object.keys(value)) {
    if (!known.has(key)) {
      return `invalid market listing field: ${key}`;
    }
  }
  for (const key of required) {
    if (!(key in value)) {
      return `invalid market listing field: ${key}`;
    }
  }
  return null;
}

function boundedText(value, maxBytes) {
  if (typeof value !== "string") {
    return null;
  }
  if (new TextEncoder().encode(value).length > maxBytes) {
    return null;
  }
  if (/[\x00-\x1f\x7f]/.test(value)) {
    return null;
  }
  return value;
}

// `bounded_directory_text` counts Unicode scalar values, not bytes -- a
// producer-side truncation rule this parser mirrors as a refusal instead,
// consistent with every other field here: a value outside the bound is
// declined, never silently cut down to fit.
function boundedDirectoryText(value, maxChars) {
  if (typeof value !== "string") {
    return null;
  }
  if (/[\x00-\x1f\x7f]/.test(value)) {
    return null;
  }
  if ([...value].length > maxChars) {
    return null;
  }
  return value;
}

function failedAnswer() {
  return { kind: "failed", stage: "", awaitsPerson: false, connectorId: "" };
}

function fail(reason) {
  return { ok: false, reason };
}
