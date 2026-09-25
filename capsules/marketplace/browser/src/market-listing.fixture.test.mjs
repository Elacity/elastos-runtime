import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import { parseMarketListing } from "./market-listing.js";

// Written by the Runtime producer itself
// (`listing_object_writes_the_capsule_parser_fixture` in
// elastos/crates/elastos-server/src/api/gateway_tests/listing_object.rs),
// never by hand, so this parser is held to what Runtime really sends.
const fixture = JSON.parse(
  readFileSync(new URL("./fixtures/market-listing.json", import.meta.url), "utf8"),
);

test("the parser accepts the ListingObject the Runtime produced", () => {
  const parsed = parseMarketListing(fixture);
  assert.equal(parsed.ok, true, parsed.reason);
  assert.equal(parsed.listing.item.kid, fixture.item.kid);
  assert.equal(parsed.listing.asset.readability, "verified");
  assert.equal(parsed.listing.offers.length, fixture.offers.length);
});

test("the producer's fixture carries one native and one ERC-20 offer (R16)", () => {
  const native = fixture.offers.filter((offer) => offer.payment_processor === null);
  const erc20 = fixture.offers.filter((offer) => typeof offer.payment_processor === "string");
  assert.equal(native.length, 1);
  assert.equal(native[0].pay_token, "0x0000000000000000000000000000000000000000");
  assert.equal(erc20.length, 1);
  assert.notEqual(erc20[0].pay_token, "0x0000000000000000000000000000000000000000");
});

// Written by the same producer
// (`listing_object_writes_the_capsule_parser_variants_fixture` in
// listing_object.rs): the ListingObjects the main fixture does not show.
const variants = JSON.parse(
  readFileSync(new URL("./fixtures/market-listing-variants.json", import.meta.url), "utf8"),
);

test("the producer wrote exactly the five listing variants", () => {
  assert.deepEqual(Object.keys(variants).sort(), [
    "access_unknown",
    "foreign",
    "offers_truncated",
    "unknown",
    "unverified",
  ]);
});

test("the parser accepts a listing whose metadata Runtime could not read (R46)", () => {
  const parsed = parseMarketListing(variants.unknown);
  assert.equal(parsed.ok, true, parsed.reason);
  assert.equal(parsed.listing.asset.readability, "unknown");
  assert.equal(parsed.listing.item.kid, null);
});

test("the parser accepts a listing whose offers Runtime truncated at 32 sellers", () => {
  const parsed = parseMarketListing(variants.offers_truncated);
  assert.equal(parsed.ok, true, parsed.reason);
  assert.equal(parsed.listing.offersTruncated, true);
  assert.equal(parsed.listing.offers.length, 32);
  assert.equal(parsed.listing.accessUnknown, undefined);
});

test("the parser accepts a listing whose access state the chain did not answer", () => {
  const parsed = parseMarketListing(variants.access_unknown);
  assert.equal(parsed.ok, true, parsed.reason);
  assert.equal(parsed.listing.accessUnknown, true);
  assert.equal(parsed.listing.accessState, "available");
  assert.equal(parsed.listing.offersTruncated, undefined);
});

test("the parser accepts an unverified and a foreign asset as they are", () => {
  for (const readability of ["unverified", "foreign"]) {
    const parsed = parseMarketListing(variants[readability]);
    assert.equal(parsed.ok, true, `${readability}: ${parsed.reason}`);
    assert.equal(parsed.listing.asset.readability, readability);
    assert.equal(parsed.listing.offersTruncated, undefined);
    assert.equal(parsed.listing.accessUnknown, undefined);
  }
});
