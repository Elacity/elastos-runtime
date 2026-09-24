import assert from "node:assert/strict";
import test from "node:test";

import {
  MAX_RUNTIME_CUSTODY_LISTINGS,
  buyOutcomeFromAnswer,
  listingUriFromInput,
  parseRuntimeCustodyListing,
  parseRuntimeCustodyListings,
  uint256Decimal,
  viewerForListing,
} from "./listing.js";

// These fixtures are the exact documents Runtime publishes, field for field:
// `runtime_custody_listing_summary` and `runtime_custody_listing_availability`
// in elastos/crates/elastos-server/src/protected_content_runtime.rs. A field
// added there and left out here is the drift this file exists to catch, so the
// producer's key set is written out once and every case builds from it.
function availabilityFixture(overrides = {}) {
  return {
    schema: "elastos.library.runtime-custody-availability-summary/v1",
    status: "last_verified_receipt",
    checked_at: 1789656121,
    required_replicas: 3,
    observed_replicas: 3,
    receipt_digest: "357bc17ce614dda83ef2542c01f574c4db5571fe648e2e60eb9977c8b5f0c8ef",
    recheck_before_buy: true,
    recheck_before_open: true,
    ...overrides,
  };
}

function objectFixture(overrides = {}) {
  return {
    schema: "elastos.library.runtime-custody-listing/v1",
    mint_id: "9b481badc7bbf0d721f8d3263d67c4bebfdbc1318e75d334d144898fe6826497",
    display_name: "1409899-uhd_3840_2160_25fps.png",
    content_kind: "object",
    listing_uri: "elastos://QmcHh9eQuLiisfxZo4m4TxaVTSYzs4AF4LaVcmkd2o2Y76",
    // The published metadata directory, which carries the title the creator
    // typed and the cover they chose. Taken from a real record on this Home.
    metadata_cid: "QmX1KSGkt3fLX5GsuN7wVWrCj55PSnfbk4JJ2BTY2QRJ7F",
    mime_type: "image/png",
    // An object carries no codecs. Runtime sends the empty string for it.
    codecs: "",
    quantity: "0x3e8",
    price: "0xf4240",
    pay_token: "0x0000000000000000000000000000000000000000",
    seller_address: "0xab5028bdbb0826ad6f1885478e421db677b0001a",
    token_id: "0x66a0fd4edeb6869c2cc3168145f8010117f11f244df754600f5fea05958134ac",
    published_at: 1789656297,
    purchase_in_flight: false,
    availability: availabilityFixture(),
    access_state: "available",
    ...overrides,
  };
}

function mediaFixture(overrides = {}) {
  return objectFixture({
    mint_id: "0fd1d568e35f3af94e06857ef20bf9dce3a36c3cf46e8a80a25267844a40ada6",
    display_name: "_file_example_MP4_480_1_5MG.mp4",
    content_kind: "media",
    mime_type: "video/mp4",
    codecs: "avc1.640028",
    ...overrides,
  });
}

function listingsFixture(listings) {
  return {
    schema: "elastos.library.runtime-custody-listings/v1",
    truncated: false,
    listings,
  };
}

test("a media row Runtime publishes today is accepted whole", () => {
  const listing = parseRuntimeCustodyListing(mediaFixture());
  assert.equal(listing.mintId, "0fd1d568e35f3af94e06857ef20bf9dce3a36c3cf46e8a80a25267844a40ada6");
  assert.equal(listing.mimeType, "video/mp4");
  assert.equal(listing.codecs, "avc1.640028");
  assert.equal(listing.accessState, "available");
  assert.equal(listing.availability.observedReplicas, 3);
  assert.equal(listing.availability.requiredReplicas, 3);
});

test("an object row Runtime publishes today is accepted, empty codecs and all", () => {
  const listing = parseRuntimeCustodyListing(objectFixture());
  assert.equal(listing.mimeType, "image/png");
  assert.equal(listing.codecs, "");
  assert.equal(listing.displayName, "1409899-uhd_3840_2160_25fps.png");
  assert.equal(listing.listingUri, "elastos://QmcHh9eQuLiisfxZo4m4TxaVTSYzs4AF4LaVcmkd2o2Y76");
});

test("a purchase already under way survives the page that started it", () => {
  const idle = parseRuntimeCustodyListing(objectFixture());
  assert.equal(idle.purchaseInFlight, false);
  const running = parseRuntimeCustodyListing(objectFixture({ purchase_in_flight: true }));
  assert.equal(running.purchaseInFlight, true);
  assert.equal(running.accessState, "available", "it is not theirs until it settles");
});

test("the availability receipt digest is accepted and stays out of the rendered row", () => {
  const listing = parseRuntimeCustodyListing(objectFixture());
  assert.equal(
    Object.prototype.hasOwnProperty.call(listing.availability, "receiptDigest"),
    false,
    "the row renders replica counts, not the receipt hash",
  );
  assert.throws(
    () => parseRuntimeCustodyListing(objectFixture({
      availability: availabilityFixture({ receipt_digest: "not-a-digest" }),
    })),
    /invalid protected item availability/,
    "an accepted field is still a validated field",
  );
});

test("a field Runtime never sent fails closed", () => {
  assert.throws(
    () => parseRuntimeCustodyListing(objectFixture({ resale_price: "0x1" })),
    /invalid protected item shape/,
  );
  assert.throws(
    () => parseRuntimeCustodyListing(objectFixture({
      availability: availabilityFixture({ settled: true }),
    })),
    /invalid protected item shape/,
  );
});

test("a field Runtime always sends may not go missing", () => {
  for (const key of [
    "schema",
    "mint_id",
    "display_name",
    "mime_type",
    "codecs",
    "content_kind",
    "listing_uri",
    "metadata_cid",
    "quantity",
    "price",
    "pay_token",
    "seller_address",
    "token_id",
    "published_at",
    "purchase_in_flight",
    "availability",
    "access_state",
  ]) {
    const row = objectFixture();
    delete row[key];
    assert.throws(() => parseRuntimeCustodyListing(row), /invalid protected item shape/, key);
  }
});

test("values that could mislead a buyer are refused", () => {
  const cases = [
    ["quantity that is not a uint256", { quantity: "0x" }],
    ["price with a leading zero", { price: "0x0186a0" }],
    ["seller that is not an address", { seller_address: "0xab50" }],
    ["pay token that is not an address", { pay_token: "not-a-token" }],
    ["mint id that is not 32 bytes", { mint_id: "9b481bad" }],
    ["access state nobody defined", { access_state: "reserved" }],
    ["a kind Runtime does not publish", { content_kind: "document" }],
    ["an empty kind", { content_kind: "" }],
    ["a link that is not a listing link", { listing_uri: "https://example.com/listing" }],
    ["a purchase state that is not a fact", { purchase_in_flight: "yes" }],
    ["display name with a control character", { display_name: "one\u0007two" }],
    ["empty display name", { display_name: "" }],
  ];
  for (const [label, overrides] of cases) {
    assert.throws(() => parseRuntimeCustodyListing(objectFixture(overrides)), /invalid protected item/, label);
  }
});

test("an availability claim weaker than the policy is refused", () => {
  assert.throws(
    () => parseRuntimeCustodyListing(objectFixture({
      availability: availabilityFixture({ observed_replicas: 2 }),
    })),
    /invalid protected item availability state/,
  );
  assert.throws(
    () => parseRuntimeCustodyListing(objectFixture({
      availability: availabilityFixture({ recheck_before_buy: false }),
    })),
    /invalid protected item availability state/,
  );
});

test("the response envelope carries both kinds at once", () => {
  const parsed = parseRuntimeCustodyListings(listingsFixture([objectFixture(), mediaFixture()]));
  assert.equal(parsed.truncated, false);
  assert.deepEqual(parsed.listings.map((listing) => listing.mimeType), ["image/png", "video/mp4"]);
});

test("the envelope refuses more rows than Runtime will ever send", () => {
  const listings = Array.from({ length: MAX_RUNTIME_CUSTODY_LISTINGS + 1 }, () => objectFixture());
  assert.throws(() => parseRuntimeCustodyListings(listingsFixture(listings)), /invalid protected item list/);
});

test("one bad row does not hide the rest of the shelf", () => {
  const parsed = parseRuntimeCustodyListings(
    listingsFixture([objectFixture(), objectFixture({ price: "0x" }), mediaFixture()]),
  );
  assert.equal(parsed.listings.length, 2, "the two sound rows still render");
  assert.equal(parsed.rejected, 1, "and the app can say that one row was refused");
});

test("an item opens in the viewer its own kind names", () => {
  for (const mimeType of ["video/mp4", "video/quicktime", "audio/mpeg", "audio/mp4"]) {
    const listing = parseRuntimeCustodyListing(mediaFixture({ mime_type: mimeType }));
    assert.equal(viewerForListing(listing), "elacity-player", mimeType);
  }
  for (const mimeType of ["image/png", "image/jpeg", "application/pdf", "text/plain", "model/gltf-binary"]) {
    const listing = parseRuntimeCustodyListing(objectFixture({ mime_type: mimeType }));
    assert.equal(viewerForListing(listing), "elacity-reader", mimeType);
  }
});

test("the kind decides the viewer, and the MIME never overrides it", () => {
  // Runtime's open path admits one viewer per kind. A row whose MIME and kind
  // disagree still opens where the kind says, because that is the decision
  // Runtime will enforce; the alternative is a launch it refuses.
  const videoLabelledObject = parseRuntimeCustodyListing(
    objectFixture({ content_kind: "object", mime_type: "video/mp4", codecs: "avc1.640028" }),
  );
  assert.equal(viewerForListing(videoLabelledObject), "elacity-reader");
});

test("an item whose kind cannot be read opens in the viewer that can say so", () => {
  // Reader renders an honest unsupported state. Player would sit on a blank
  // frame, so anything this app cannot classify goes to Reader.
  assert.equal(viewerForListing({ contentKind: "" }), "elacity-reader");
  assert.equal(viewerForListing({}), "elacity-reader");
  assert.equal(viewerForListing(null), "elacity-reader");
});

test("a listing link is accepted in the shapes Runtime publishes", () => {
  // Runtime writes `elastos://<cid>` and nothing else: the creator listing
  // origin is refused unless the URI equals that exactly. Both CID spellings
  // in use here are plain alphanumeric.
  assert.equal(
    listingUriFromInput("elastos://QmcHh9eQuLiisfxZo4m4TxaVTSYzs4AF4LaVcmkd2o2Y76"),
    "elastos://QmcHh9eQuLiisfxZo4m4TxaVTSYzs4AF4LaVcmkd2o2Y76",
  );
  assert.equal(
    listingUriFromInput("  elastos://bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi  "),
    "elastos://bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi",
    "a pasted link keeps its surrounding whitespace out of the request",
  );
});

test("a listing link that is not one is refused before Runtime is asked", () => {
  for (const value of [
    "",
    "   ",
    "QmcHh9eQuLiisfxZo4m4TxaVTSYzs4AF4LaVcmkd2o2Y76",
    "https://example.com/QmcHh9eQuLiisfxZo4m4TxaVTSYzs4AF4LaVcmkd2o2Y76",
    "elastos://",
    "elastos://short",
    "elastos://QmcHh9eQuLiisfxZo4m4TxaVTSYzs4AF4LaVcmkd2o2Y76/listing.json",
    "elastos://QmcHh9eQuLiisfxZo4m4TxaVTSYzs4AF4LaVcmkd2o2Y76?v=1",
    "elastos://QmcHh9eQuLiisfxZo4m4TxaVTSYzs4AF4LaVcmkd2o2Y76#fragment",
    `elastos://${"Q".repeat(200)}`,
    null,
  ]) {
    assert.equal(listingUriFromInput(value), "", JSON.stringify(value));
  }
});

test("a purchase waiting on the person is told apart from one waiting on the network", () => {
  const approval = buyOutcomeFromAnswer({
    buy_progress: {
      schema: "elastos.protected-content.buy-progress/v1",
      stage: "purchase_approval",
      resumable: true,
      awaits_person: true,
      external_signer: true,
      connector_id: "wallet-metamask",
    },
  });
  assert.equal(approval.kind, "waiting");
  assert.equal(approval.awaitsPerson, true);
  assert.equal(approval.connectorId, "wallet-metamask");
  assert.equal(approval.stage, "purchase_approval");

  const settling = buyOutcomeFromAnswer({
    buy_progress: {
      schema: "elastos.protected-content.buy-progress/v1",
      stage: "chain_settlement",
      resumable: true,
      awaits_person: false,
    },
  });
  assert.equal(settling.kind, "waiting");
  assert.equal(settling.awaitsPerson, false);
  assert.equal(settling.connectorId, "");
});

test("an allowance approval is named as the payment it unlocks, not as the purchase", () => {
  const allowance = buyOutcomeFromAnswer({
    buy_progress: {
      schema: "elastos.protected-content.buy-progress/v1",
      stage: "allowance_approval",
      resumable: true,
      awaits_person: true,
      external_signer: true,
      connector_id: null,
    },
  });
  assert.equal(allowance.kind, "waiting");
  assert.equal(allowance.stage, "allowance_approval");
  assert.equal(allowance.connectorId, "");
});

test("a declined purchase stops, and asking again is the person's choice", () => {
  const declined = buyOutcomeFromAnswer({
    buy_progress: {
      schema: "elastos.protected-content.buy-progress/v1",
      stage: "declined",
      resumable: false,
      awaits_person: false,
    },
  });
  assert.equal(declined.kind, "declined");
  assert.equal(declined.awaitsPerson, false);
});

test("an answer that carries no progress is a failure, not a wait", () => {
  for (const answer of [
    {},
    null,
    { message: "Runtime custody purchase is denied before buy" },
    { buy_progress: { schema: "something.else/v1", stage: "purchase_approval", resumable: true } },
    { buy_progress: { schema: "elastos.protected-content.buy-progress/v1", stage: "nonsense", resumable: true } },
    { buy_progress: { schema: "elastos.protected-content.buy-progress/v1", stage: "purchase_approval" } },
  ]) {
    assert.equal(buyOutcomeFromAnswer(answer).kind, "failed", JSON.stringify(answer));
  }
});

test("uint256 values are shown in full, never through a float", () => {
  assert.equal(uint256Decimal("0x186a0"), "100000");
  assert.equal(uint256Decimal("0x0"), "0");
  assert.equal(
    uint256Decimal("0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"),
    "115792089237316195423570985008687907853269984665640564039457584007913129639935",
  );
  assert.throws(() => uint256Decimal("0x0186a0"), /invalid protected item field/);
  assert.throws(() => uint256Decimal("100000"), /invalid protected item field/);
});
