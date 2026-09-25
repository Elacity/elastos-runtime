import assert from "node:assert/strict";
import test from "node:test";

import {
  BUY_OFFER_COMPLETE_SCHEMA_V1,
  BUY_OFFER_PROGRESS_SCHEMA_V1,
  MARKET_LISTING_SCHEMA_V1,
  marketItemClaim,
  marketItemKey,
  offersWithRecordedAttempt,
  parseBuyOfferAnswer,
  parseMarketListing,
} from "./market-listing.js";

// Built inline from the written contract (the listing object and the
// buy_offer answers). The producer-written fixtures are exercised by the
// *.fixture.test.mjs files beside this one; this file does not depend on them.

// A canonical 40-hex-character address, built rather than hand-counted, so a
// fixture can never silently drift a digit short of what the parser demands.
function addr(id) {
  return `0x${String(id).padStart(40, "0")}`;
}

// A canonical 32-hex-character kid, same reasoning as `addr`.
function kidHex(id) {
  return `0x${String(id).padStart(32, "0")}`;
}

function offerFixture(overrides = {}) {
  return {
    seller: addr(1),
    quantity: "0x3",
    price: "0x2dc6c0",
    pay_token: addr(0),
    payment_processor: addr(2),
    ...overrides,
  };
}

function itemFixture(overrides = {}) {
  return {
    chain_namespace: "eip155:8453",
    network: "base",
    ledger: addr(3),
    token_id: "0x1f4",
    operative: addr(4),
    kid: kidHex("102030405060708090a0b0c0d0e0f10"),
    ...overrides,
  };
}

function assetFixture(overrides = {}) {
  return {
    uri: "elastos://QmcHh9eQuLiisfxZo4m4TxaVTSYzs4AF4LaVcmkd2o2Y76",
    title: "A protected item",
    description: "",
    cover_cid: "QmX1KSGkt3fLX5GsuN7wVWrCj55PSnfbk4JJ2BTY2QRJ7F",
    category: "video",
    mime_type: "video/mp4",
    readability: "verified",
    ...overrides,
  };
}

function sourceFixture(overrides = {}) {
  return { start: "item", read_at_block: "0x2a", ...overrides };
}

function listingFixture(overrides = {}) {
  return {
    schema: MARKET_LISTING_SCHEMA_V1,
    item: itemFixture(),
    asset: assetFixture(),
    offers: [offerFixture()],
    access_state: "available",
    source: sourceFixture(),
    ...overrides,
  };
}

function progressFixture(overrides = {}) {
  return {
    schema: BUY_OFFER_PROGRESS_SCHEMA_V1,
    stage: "chain_settlement",
    resumable: true,
    awaits_person: false,
    connector_id: "wallet-metamask",
    ...overrides,
  };
}

// `buy_offer`'s terminal success answer: the item bought, the asset it names, and how far adoption got.
function buyOfferCompleteFixture(overrides = {}) {
  return {
    schema: BUY_OFFER_COMPLETE_SCHEMA_V1,
    item: itemFixture(),
    asset_uri: "elastos://QmcHh9eQuLiisfxZo4m4TxaVTSYzs4AF4LaVcmkd2o2Y76",
    adoption: "adopted",
    ...overrides,
  };
}

// The mint-based terminal shape `runtime_custody_buy_terminal_response`
// builds for `buy`. `buy_offer` never answers with
// this -- a market purchase of a foreign item has no mint_id/content_id/cid.
function oldMintBasedTerminalFixture(overrides = {}) {
  const mintId = "a".repeat(64);
  const contentId = "b".repeat(64);
  const cid = "QmX1KSGkt3fLX5GsuN7wVWrCj55PSnfbk4JJ2BTY2QRJ7F";
  return {
    schema: "elastos.library.runtime-custody-purchase/v1",
    mint_id: mintId,
    content_id: contentId,
    cid,
    availability: {
      schema: "elastos.library.runtime-custody-availability/v1",
      status: "buyer_owned",
      cid,
      content_id: contentId,
      mint_id: mintId,
    },
    ...overrides,
  };
}

test("a listing built exactly as the contract describes parses whole", () => {
  const result = parseMarketListing(listingFixture());
  assert.equal(result.ok, true);
  assert.equal(result.listing.item.chainNamespace, "eip155:8453");
  assert.equal(result.listing.item.kid, "0x0102030405060708090a0b0c0d0e0f10");
  assert.equal(result.listing.asset.readability, "verified");
  assert.equal(result.listing.offers.length, 1);
  assert.equal(result.listing.offers[0].seller, addr(1));
  assert.equal(result.listing.accessState, "available");
  assert.equal(result.listing.source.readAtBlock, "0x2a");
  assert.equal(Object.prototype.hasOwnProperty.call(result.listing, "offersTruncated"), false);
});

test("item.kid is optional -- a listing that has not bound one still parses", () => {
  const { kid, ...itemWithoutKid } = itemFixture();
  const result = parseMarketListing(listingFixture({ item: itemWithoutKid }));
  assert.equal(result.ok, true);
  assert.equal(Object.prototype.hasOwnProperty.call(result.listing.item, "kid"), false);
});

test("item.kid may be null (R46) -- a listing whose KID Runtime could not learn still parses", () => {
  const result = parseMarketListing(listingFixture({ item: itemFixture({ kid: null }) }));
  assert.equal(result.ok, true);
  assert.equal(result.listing.item.kid, null);
});

test("readability unknown (R46) parses, with a null kid, and keeps its offers to buy", () => {
  const result = parseMarketListing(listingFixture({
    item: itemFixture({ kid: null }),
    asset: assetFixture({ readability: "unknown" }),
  }));
  assert.equal(result.ok, true);
  assert.equal(result.listing.asset.readability, "unknown");
  assert.equal(result.listing.offers.length, 1);
});

test("offers_truncated and access_unknown are accepted only as true", () => {
  const truncated = parseMarketListing(listingFixture({ offers_truncated: true }));
  assert.equal(truncated.ok, true);
  assert.equal(truncated.listing.offersTruncated, true);

  const unknown = parseMarketListing(listingFixture({ access_unknown: true }));
  assert.equal(unknown.ok, true);
  assert.equal(unknown.listing.accessUnknown, true);

  const falseFlag = parseMarketListing(listingFixture({ offers_truncated: false }));
  assert.equal(falseFlag.ok, false);
});

test("a wrong schema is refused", () => {
  const result = parseMarketListing(listingFixture({ schema: "elastos.marketplace.listing/v2" }));
  assert.equal(result.ok, false);
  assert.match(result.reason, /schema/);
});

test("never throws on a value that is not an object", () => {
  for (const bad of [null, undefined, "listing", 42, [], []]) {
    const result = parseMarketListing(bad);
    assert.equal(result.ok, false);
  }
});

test("an unknown top-level key is refused", () => {
  const result = parseMarketListing(listingFixture({ extra_field: "x" }));
  assert.equal(result.ok, false);
  assert.match(result.reason, /field/);
});

test("a missing required top-level key is refused", () => {
  const { source: _source, ...withoutSource } = listingFixture();
  const result = parseMarketListing(withoutSource);
  assert.equal(result.ok, false);
});

test("an unknown key inside item is refused", () => {
  const result = parseMarketListing(listingFixture({ item: itemFixture({ mint_id: "x" }) }));
  assert.equal(result.ok, false);
});

test("an unknown key inside asset is refused", () => {
  const result = parseMarketListing(listingFixture({ asset: assetFixture({ extra: "x" }) }));
  assert.equal(result.ok, false);
});

test("an unknown key inside an offer is refused", () => {
  const result = parseMarketListing(listingFixture({ offers: [offerFixture({ extra: "x" })] }));
  assert.equal(result.ok, false);
});

test("an unknown key inside source is refused", () => {
  const result = parseMarketListing(listingFixture({ source: sourceFixture({ extra: "x" }) }));
  assert.equal(result.ok, false);
});

test("a uint256 field sent as a JSON number is refused", () => {
  const result = parseMarketListing(listingFixture({ offers: [offerFixture({ price: 3000000 })] }));
  assert.equal(result.ok, false);
});

test("a uint256 field with uppercase hex digits is refused", () => {
  const result = parseMarketListing(listingFixture({ offers: [offerFixture({ price: "0x2DC6C0" })] }));
  assert.equal(result.ok, false);
});

test("a uint256 field with a leading zero is refused, except the value zero", () => {
  const leadingZero = parseMarketListing(listingFixture({ offers: [offerFixture({ quantity: "0x03" })] }));
  assert.equal(leadingZero.ok, false);

  const zero = parseMarketListing(listingFixture({ offers: [offerFixture({ quantity: "0x0" })] }));
  assert.equal(zero.ok, true);
});

test("every uint256 field is checked: token_id and read_at_block too", () => {
  const badTokenId = parseMarketListing(listingFixture({ item: itemFixture({ token_id: "0x0F4" }) }));
  assert.equal(badTokenId.ok, false);

  const badBlock = parseMarketListing(listingFixture({ source: sourceFixture({ read_at_block: "42" }) }));
  assert.equal(badBlock.ok, false);
});

test("an address that is not 40 lowercase hex characters is refused", () => {
  const short = parseMarketListing(listingFixture({ offers: [offerFixture({ seller: "0xab" })] }));
  assert.equal(short.ok, false);

  const lowercaseWithLetters = `0x${"ab".padEnd(40, "0")}`;
  assert.equal(
    parseMarketListing(listingFixture({ offers: [offerFixture({ seller: lowercaseWithLetters })] })).ok,
    true,
    "sanity: this address is valid lowercase to begin with",
  );
  const uppercase = parseMarketListing(
    listingFixture({
      offers: [offerFixture({ seller: `0x${lowercaseWithLetters.slice(2).toUpperCase()}` })],
    }),
  );
  assert.equal(uppercase.ok, false);

  const badOperative = parseMarketListing(listingFixture({ item: itemFixture({ operative: "not-an-address" }) }));
  assert.equal(badOperative.ok, false);
});

// R16: every offer carries `payment_processor` -- `null` for a native-token
// offer, a canonical address otherwise, and nothing else.
test("a native offer's payment_processor is null and is accepted", () => {
  const result = parseMarketListing(
    listingFixture({ offers: [offerFixture({ pay_token: addr(0), payment_processor: null })] }),
  );
  assert.equal(result.ok, true, result.reason);
  assert.equal(result.listing.offers[0].paymentProcessor, null);
});

test("an offer without the payment_processor key is refused", () => {
  const offer = offerFixture();
  delete offer.payment_processor;
  const result = parseMarketListing(listingFixture({ offers: [offer] }));
  assert.equal(result.ok, false);
  assert.equal(result.reason, "invalid market listing field: payment_processor");
});

test("a payment_processor that is neither null nor an address is refused", () => {
  for (const paymentProcessor of ["", "not-an-address", "0xab", `0x${"AB".padEnd(40, "0")}`, 0, false, {}]) {
    const result = parseMarketListing(
      listingFixture({ offers: [offerFixture({ payment_processor: paymentProcessor })] }),
    );
    assert.equal(result.ok, false, JSON.stringify(paymentProcessor));
    assert.equal(result.reason, "invalid market listing offer payment processor");
  }
});

test("a kid that is not 0x plus 32 lowercase hex characters is refused", () => {
  const tooShort = parseMarketListing(listingFixture({ item: itemFixture({ kid: "0x0102" }) }));
  assert.equal(tooShort.ok, false);

  const uppercase = parseMarketListing(
    listingFixture({ item: itemFixture({ kid: "0x0102030405060708090A0B0C0D0E0F10" }) }),
  );
  assert.equal(uppercase.ok, false);
});

test("a readability outside verified|unverified|foreign|unknown is refused", () => {
  for (const readability of ["verified", "unverified", "foreign", "unknown"]) {
    assert.equal(parseMarketListing(listingFixture({ asset: assetFixture({ readability }) })).ok, true);
  }
  const result = parseMarketListing(listingFixture({ asset: assetFixture({ readability: "trusted" }) }));
  assert.equal(result.ok, false);
});

test("an access_state outside available|purchased|creator is refused", () => {
  for (const accessState of ["available", "purchased", "creator"]) {
    assert.equal(parseMarketListing(listingFixture({ access_state: accessState })).ok, true);
  }
  const result = parseMarketListing(listingFixture({ access_state: "owned" }));
  assert.equal(result.ok, false);
});

test("a source.start outside token_uri|kid|item is refused", () => {
  for (const start of ["token_uri", "kid", "item"]) {
    assert.equal(parseMarketListing(listingFixture({ source: sourceFixture({ start }) })).ok, true);
  }
  const result = parseMarketListing(listingFixture({ source: sourceFixture({ start: "link" }) }));
  assert.equal(result.ok, false);
});

test("more than the offers cap is refused", () => {
  const offers = Array.from({ length: 33 }, (_, index) => offerFixture({
    seller: `0x${String(index).padStart(40, "0")}`,
  }));
  const result = parseMarketListing(listingFixture({ offers }));
  assert.equal(result.ok, false);
});

test("asset.cover_cid may be null", () => {
  const result = parseMarketListing(listingFixture({ asset: assetFixture({ cover_cid: null }) }));
  assert.equal(result.ok, true);
  assert.equal(result.listing.asset.coverCid, null);
});

test("asset.uri that is not the elastos:// + CID shape is refused", () => {
  const result = parseMarketListing(listingFixture({ asset: assetFixture({ uri: "elastos://not-a-cid" }) }));
  assert.equal(result.ok, false);
});

test("a directory-text field of exactly 200 CJK characters is accepted", () => {
  const title = "文".repeat(200); // 200 Unicode scalar values, well under the byte cap too
  const result = parseMarketListing(listingFixture({ asset: assetFixture({ title }) }));
  assert.equal(result.ok, true);
  assert.equal([...result.listing.asset.title].length, 200);
});

test("a directory-text field of 257 characters is refused, by character count not byte count", () => {
  const title = "文".repeat(257);
  const result = parseMarketListing(listingFixture({ asset: assetFixture({ title }) }));
  assert.equal(result.ok, false);
});

// --- parseBuyOfferAnswer: every typed code, plus complete and wait ---------

test("complete requires the exact buy-offer-complete shape, not any success-looking value", () => {
  for (const bad of [{}, null, undefined, "boom", 42, [], { status: "ok" }, { code: "" }]) {
    const result = parseBuyOfferAnswer(bad);
    assert.equal(result.kind, "failed", `expected failed for ${JSON.stringify(bad)}`);
  }
});

test("the real buy_offer terminal envelope is complete", () => {
  const result = parseBuyOfferAnswer(buyOfferCompleteFixture());
  assert.equal(result.kind, "complete");
  assert.equal(result.item.chainNamespace, "eip155:8453");
  assert.equal(result.item.kid, "0x0102030405060708090a0b0c0d0e0f10");
  assert.equal(result.assetUri, "elastos://QmcHh9eQuLiisfxZo4m4TxaVTSYzs4AF4LaVcmkd2o2Y76");
  assert.equal(result.adoption, "adopted");
});

test("every adoption value -- adopted, foreign, pending -- is accepted", () => {
  for (const adoption of ["adopted", "foreign", "pending"]) {
    const result = parseBuyOfferAnswer(buyOfferCompleteFixture({ adoption }));
    assert.equal(result.kind, "complete");
    assert.equal(result.adoption, adoption);
  }
});

test("an adoption value outside adopted|foreign|pending is a failure", () => {
  const result = parseBuyOfferAnswer(buyOfferCompleteFixture({ adoption: "owned" }));
  assert.equal(result.kind, "failed");
});

test("a buy-offer-complete envelope with an unknown extra key is a failure", () => {
  const result = parseBuyOfferAnswer({ ...buyOfferCompleteFixture(), extra: "x" });
  assert.equal(result.kind, "failed");
});

test("a buy-offer-complete envelope whose item is missing a required field is a failure", () => {
  const { operative: _operative, ...itemWithoutOperative } = itemFixture();
  const result = parseBuyOfferAnswer(buyOfferCompleteFixture({ item: itemWithoutOperative }));
  assert.equal(result.kind, "failed");
});

test("a buy-offer-complete item's kid may be null or absent (R46): a paid purchase is still complete", () => {
  const nullKid = parseBuyOfferAnswer(buyOfferCompleteFixture({ item: itemFixture({ kid: null }), adoption: "pending" }));
  assert.equal(nullKid.kind, "complete");
  assert.equal(nullKid.item.kid, null);
  assert.equal(nullKid.adoption, "pending");
  const { kid: _kid, ...itemWithoutKid } = itemFixture();
  assert.equal(parseBuyOfferAnswer(buyOfferCompleteFixture({ item: itemWithoutKid })).kind, "complete");
  // A kid that is present is still checked like a listing's.
  assert.equal(parseBuyOfferAnswer(buyOfferCompleteFixture({ item: itemFixture({ kid: "0x12" }) })).kind, "failed");
});

// `buy_offer` refused because Runtime could not reach the market or the
// chain, exactly as the object provider's error envelope carries it:
// `provider_error_from` puts the stable sentence
// (`RUNTIME_CUSTODY_PURCHASE_UNAVAILABLE_MESSAGE`) in `message` under the
// generic `library_error` code, with the cause chain in `detail`.
function buyOfferUnavailableFixture(overrides = {}) {
  return {
    status: "error",
    code: "library_error",
    message: "Runtime custody purchase is unavailable",
    detail: "src/api/gateway_marketplace_buy.rs:488: HTTP 429 Too Many Requests",
    ...overrides,
  };
}

test("a buy_offer refused as unavailable is its own kind, not a failed purchase (R48)", () => {
  assert.deepEqual(parseBuyOfferAnswer(buyOfferUnavailableFixture()), { kind: "unavailable" });
  const { detail: _detail, ...withoutDetail } = buyOfferUnavailableFixture();
  assert.deepEqual(parseBuyOfferAnswer(withoutDetail), { kind: "unavailable" });
  assert.deepEqual(parseBuyOfferAnswer({ code: "unavailable" }), { kind: "unavailable" });
});

test("only the exact unavailable envelope reads as unavailable; anything near it stays failed", () => {
  for (const near of [
    buyOfferUnavailableFixture({ message: "Runtime custody purchase is unavailable." }),
    buyOfferUnavailableFixture({ message: "Runtime custody purchase is denied" }),
    buyOfferUnavailableFixture({ code: "" }),
    { message: "Runtime custody purchase is unavailable" },
  ]) {
    assert.equal(parseBuyOfferAnswer(near).kind, "failed", JSON.stringify(near));
  }
  // A purchase that reports its own stage keeps it: progress is read first.
  const declined = parseBuyOfferAnswer(buyOfferUnavailableFixture({
    buy_progress: progressFixture({ stage: "declined", resumable: false }),
  }));
  assert.equal(declined.kind, "failed");
  assert.equal(declined.stage, "declined");
});

test("the old mint-based terminal shape is a failure, not complete -- buy_offer never answers with it", () => {
  const result = parseBuyOfferAnswer(oldMintBasedTerminalFixture());
  assert.equal(result.kind, "failed");
});

test("a resumable buy_progress is a wait, carrying stage/awaits_person/connector_id", () => {
  const result = parseBuyOfferAnswer({ buy_progress: progressFixture({ stage: "purchase_approval", awaits_person: true }) });
  assert.equal(result.kind, "wait");
  assert.equal(result.stage, "purchase_approval");
  assert.equal(result.awaitsPerson, true);
  assert.equal(result.connectorId, "wallet-metamask");
});

test("a non-resumable buy_progress is a failure, not a silent success", () => {
  const result = parseBuyOfferAnswer({ buy_progress: progressFixture({ stage: "declined", resumable: false }) });
  assert.equal(result.kind, "failed");
  assert.equal(result.stage, "declined");
});

test("a malformed buy_progress is a failure, never complete", () => {
  const result = parseBuyOfferAnswer({ buy_progress: { schema: BUY_OFFER_PROGRESS_SCHEMA_V1, stage: "not-a-stage" } });
  assert.equal(result.kind, "failed");
});

test("terms_changed carries the fresh offer, validated like one", () => {
  const result = parseBuyOfferAnswer({ code: "terms_changed", current: offerFixture({ price: "0x99" }) });
  assert.equal(result.kind, "terms_changed");
  assert.equal(result.current.price, "0x99");
});

test("terms_changed with current: null means the offer no longer exists", () => {
  const result = parseBuyOfferAnswer({ code: "terms_changed", current: null });
  assert.deepEqual(result, { kind: "terms_changed", current: null });
});

test("terms_changed with a malformed current offer is a failure, not a trusted null", () => {
  const result = parseBuyOfferAnswer({ code: "terms_changed", current: offerFixture({ price: "99" }) });
  assert.equal(result.kind, "failed");
});

test("attempt_in_progress carries the recorded offer, validated like one", () => {
  const recorded = offerFixture({ seller: addr(9), price: "0x99", payment_processor: null });
  const result = parseBuyOfferAnswer({ status: "error", code: "attempt_in_progress", current: recorded });
  assert.equal(result.kind, "attempt_in_progress");
  assert.deepEqual(result.current, {
    seller: addr(9),
    quantity: "0x3",
    price: "0x99",
    payToken: addr(0),
    paymentProcessor: null,
  });
});

test("attempt_in_progress with current: null names no seller, and is still that kind", () => {
  assert.deepEqual(
    parseBuyOfferAnswer({ code: "attempt_in_progress", current: null }),
    { kind: "attempt_in_progress", current: null },
  );
});

test("attempt_in_progress with a malformed or missing current is a failure", () => {
  assert.equal(parseBuyOfferAnswer({ code: "attempt_in_progress", current: offerFixture({ seller: "0xABC" }) }).kind, "failed");
  assert.equal(parseBuyOfferAnswer({ code: "attempt_in_progress" }).kind, "failed");
  assert.equal(parseBuyOfferAnswer({ code: "attempt_in_progress", current: { ...offerFixture(), extra: 1 } }).kind, "failed");
});

test("a buy-offer-complete item is checked by the listing item's own validators", () => {
  const badLedger = parseBuyOfferAnswer(buyOfferCompleteFixture({ item: itemFixture({ ledger: "0xABC" }) }));
  assert.equal(badLedger.kind, "failed");
  const extra = parseBuyOfferAnswer(buyOfferCompleteFixture({ item: { ...itemFixture(), seller: addr(1) } }));
  assert.equal(extra.kind, "failed");
  const ok = parseBuyOfferAnswer(buyOfferCompleteFixture());
  assert.equal(ok.kind, "complete");
  assert.equal(ok.item.kid, itemFixture().kid);
});

test("own_offer, already_owned and asset_mismatch map to their own bare kind", () => {
  for (const code of ["own_offer", "already_owned", "asset_mismatch"]) {
    assert.deepEqual(parseBuyOfferAnswer({ code }), { kind: code });
  }
});

test("an answer with a code this parser does not know is a failure", () => {
  const result = parseBuyOfferAnswer({ code: "something_new" });
  assert.equal(result.kind, "failed");
});

test("parseBuyOfferAnswer never throws on a value that is not an object", () => {
  for (const bad of [null, undefined, "x", 1, []]) {
    assert.doesNotThrow(() => parseBuyOfferAnswer(bad));
  }
});

// --- marketItemKey -----------------------------------------------------

test("marketItemKey is the same key for the same item, camelCase or wire-cased", () => {
  const parsed = parseMarketListing(listingFixture()).listing.item;
  const camel = marketItemKey(parsed);
  const wire = marketItemKey(itemFixture());
  assert.equal(camel, wire);
  assert.equal(camel, `eip155:8453|${addr(3)}|0x1f4`);
});

test("marketItemKey differs when ledger or token_id differ", () => {
  const base = marketItemKey(itemFixture());
  const otherLedger = marketItemKey(itemFixture({ ledger: addr(9) }));
  const otherToken = marketItemKey(itemFixture({ token_id: "0x9" }));
  assert.notEqual(base, otherLedger);
  assert.notEqual(base, otherToken);
});

test("marketItemKey is stable regardless of extra fields like kid or operative", () => {
  const { kid, ...withoutKid } = itemFixture();
  assert.equal(marketItemKey(itemFixture()), marketItemKey(withoutKid));
});

// --- marketItemClaim ---------------------------------------------------

test("marketItemClaim is the item a buy_offer or download_owned_copy names, without operative", () => {
  const parsed = parseMarketListing(listingFixture()).listing.item;
  const { operative, ...expected } = itemFixture();
  assert.deepEqual(marketItemClaim(parsed), expected);
});

test("marketItemClaim leaves kid out when the listing's kid is null (R46)", () => {
  const parsed = parseMarketListing(listingFixture({ item: itemFixture({ kid: null }) })).listing.item;
  const claim = marketItemClaim(parsed);
  assert.equal("kid" in claim, false);
  assert.deepEqual(Object.keys(claim), ["chain_namespace", "network", "ledger", "token_id"]);
});

test("marketItemClaim leaves kid out when the listing has not bound one", () => {
  const { kid, ...withoutKid } = itemFixture();
  const parsed = parseMarketListing(listingFixture({ item: withoutKid })).listing.item;
  const claim = marketItemClaim(parsed);
  assert.equal("kid" in claim, false);
  assert.deepEqual(Object.keys(claim), ["chain_namespace", "network", "ledger", "token_id"]);
});

// --- offersWithRecordedAttempt ------------------------------------------

function parsedOffer(overrides = {}) {
  return parseMarketListing(listingFixture({ offers: [offerFixture(overrides)] })).listing.offers[0];
}

test("a recorded attempt whose seller is still listed leaves the offers as they are", () => {
  const offers = [parsedOffer({ seller: addr(7) }), parsedOffer({ seller: addr(8) })];
  const recorded = parsedOffer({ seller: addr(8), price: "0x99" });
  assert.deepEqual(offersWithRecordedAttempt(offers, recorded), offers);
});

test("a recorded attempt whose seller is no longer listed gets its own row, on the recorded terms", () => {
  const offers = [parsedOffer({ seller: addr(7) })];
  const recorded = parsedOffer({ seller: addr(9), price: "0x99" });
  const rows = offersWithRecordedAttempt(offers, recorded);
  assert.equal(rows.length, 2);
  assert.deepEqual(rows[0], offers[0]);
  assert.deepEqual(rows[1], recorded);
  assert.equal(offers.length, 1, "the listing's own offers are not changed");
});

test("a recorded attempt replaces its seller's row once that offer is gone", () => {
  const gone = { ...parsedOffer({ seller: addr(9) }), _gone: true };
  const recorded = parsedOffer({ seller: addr(9), price: "0x99" });
  const rows = offersWithRecordedAttempt([parsedOffer({ seller: addr(7) }), gone], recorded);
  assert.equal(rows.length, 2);
  assert.deepEqual(rows[1], recorded);
});

test("no recorded offer (current: null) adds no row", () => {
  const offers = [parsedOffer({ seller: addr(7) })];
  assert.deepEqual(offersWithRecordedAttempt(offers, null), offers);
  assert.deepEqual(offersWithRecordedAttempt([], undefined), []);
});
