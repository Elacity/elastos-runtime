import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import { parseBuyOfferAnswer } from "./market-listing.js";

// Written by the Runtime producer itself
// (`buy_offer_writes_the_capsule_parser_fixture` in
// elastos/crates/elastos-server/src/api/gateway_tests/buy_offer.rs),
// never by hand: one real answer of each kind `buy_offer` gives, exactly as
// this page receives it, so the parser and the producer cannot drift.
const fixture = JSON.parse(
  readFileSync(new URL("./fixtures/buy-offer-answers.json", import.meta.url), "utf8"),
);

const EXPECTED_KINDS = {
  wait: "wait",
  declined: "failed",
  terms_changed_native: "terms_changed",
  terms_changed_erc20: "terms_changed",
  terms_changed_gone: "terms_changed",
  attempt_in_progress: "attempt_in_progress",
  attempt_in_progress_other_path: "attempt_in_progress",
  own_offer: "own_offer",
  already_owned: "already_owned",
  asset_mismatch: "asset_mismatch",
  complete: "complete",
  complete_foreign: "complete",
};

test("the producer wrote one answer of every kind, and nothing else", () => {
  assert.deepEqual(Object.keys(fixture).sort(), Object.keys(EXPECTED_KINDS).sort());
});

for (const [name, kind] of Object.entries(EXPECTED_KINDS)) {
  test(`the parser reads the Runtime's ${name} answer as ${kind}`, () => {
    const outcome = parseBuyOfferAnswer(fixture[name]);
    assert.equal(outcome.kind, kind, JSON.stringify(outcome));
  });
}

test("terms_changed carries the fresh native offer, or null when it is gone", () => {
  const native = parseBuyOfferAnswer(fixture.terms_changed_native);
  assert.equal(native.current.paymentProcessor, null);
  assert.equal(native.current.payToken, "0x0000000000000000000000000000000000000000");
  assert.equal(native.current.price, fixture.terms_changed_native.current.price);
  assert.equal(parseBuyOfferAnswer(fixture.terms_changed_gone).current, null);
});

test("a wait resumes, and the completed purchase names the item and asset", () => {
  const wait = parseBuyOfferAnswer(fixture.wait);
  assert.equal(wait.stage, "purchase_approval");
  const complete = parseBuyOfferAnswer(fixture.complete);
  assert.equal(complete.assetUri, fixture.complete.asset_uri);
  assert.equal(complete.item.kid, fixture.complete.item.kid);
  assert.equal(complete.adoption, "pending");
});

test("terms_changed carries an ERC-20 offer's processor", () => {
  const erc20 = parseBuyOfferAnswer(fixture.terms_changed_erc20);
  assert.equal(erc20.current.paymentProcessor, fixture.terms_changed_erc20.current.payment_processor);
  assert.notEqual(erc20.current.payToken, "0x0000000000000000000000000000000000000000");
});

test("attempt_in_progress names the recorded offer, or null for the other path", () => {
  const inProgress = parseBuyOfferAnswer(fixture.attempt_in_progress);
  assert.equal(inProgress.current.seller, fixture.attempt_in_progress.current.seller);
  assert.equal(inProgress.current.price, fixture.attempt_in_progress.current.price);
  assert.equal(parseBuyOfferAnswer(fixture.attempt_in_progress_other_path).current, null);
});

test("a declined approval does not resume, and a foreign purchase completes as foreign", () => {
  const declined = parseBuyOfferAnswer(fixture.declined);
  assert.equal(declined.stage, "declined");
  assert.equal(parseBuyOfferAnswer(fixture.complete_foreign).adoption, "foreign");
});
