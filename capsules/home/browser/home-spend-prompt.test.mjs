import assert from "node:assert/strict";
import test from "node:test";

import {
  SPEND_DECLINED_MESSAGE,
  moneyVerbIntentRows,
} from "./home-spend-prompt.js";

const BUY_INTENT = {
  content_id: "0f0e0d0c0b0a09080706050403020100",
  quantity: "1",
  seller: "0xabcdefabcdefabcdefabcdefabcdefabcdefabcd",
  expected_price: "1000000000000000000",
  expected_pay_token: "0x0000000000000000000000000000000000000000",
};

test("known fields render in a fixed order under friendly labels", () => {
  const rows = moneyVerbIntentRows(BUY_INTENT);
  assert.deepEqual(
    rows.map(([label]) => label),
    ["Asset", "Seller", "Quantity", "Price", "Pay with"],
  );
  assert.deepEqual(rows[0], ["Asset", BUY_INTENT.content_id]);
  assert.deepEqual(rows[3], ["Price", BUY_INTENT.expected_price]);
});

// The security property. The step-up ceremony signs the WHOLE request, so a field the display
// does not recognize is still signed — hiding it would let a compromised frame smuggle a term
// past the user while showing them a purchase that looks ordinary.
test("unrecognized fields are shown, never silently dropped", () => {
  const rows = moneyVerbIntentRows({
    ...BUY_INTENT,
    smuggled_recipient: "0xattacker",
    another: 7,
  });
  const labels = rows.map(([label]) => label);
  assert.ok(labels.includes("smuggled_recipient"));
  assert.ok(labels.includes("another"));
  assert.deepEqual(
    rows.find(([label]) => label === "smuggled_recipient"),
    ["smuggled_recipient", "0xattacker"],
  );
  // Known fields still lead; the unknown ones follow.
  assert.equal(labels[0], "Asset");
  assert.ok(labels.indexOf("smuggled_recipient") > labels.indexOf("Pay with"));
});

test("markup in a value survives as literal text for textContent to escape", () => {
  const [[, value]] = moneyVerbIntentRows({
    content_id: '<img src=x onerror="steal()">',
  });
  // The value is passed through verbatim — escaping is structural (textContent), not by
  // sanitising here, so a sanitiser bug can never be the thing standing between a frame and DOM.
  assert.equal(value, '<img src=x onerror="steal()">');
});

test("values are collapsed, bounded, and never blank", () => {
  const [[, long]] = moneyVerbIntentRows({ content_id: "a".repeat(500) });
  assert.ok(long.length <= 161, `value not bounded: ${long.length}`);
  assert.ok(long.endsWith("…"));

  assert.deepEqual(moneyVerbIntentRows({ content_id: "  a\n\tb  " }), [["Asset", "a b"]]);
  assert.deepEqual(moneyVerbIntentRows({ content_id: "" }), [["Asset", "—"]]);
  assert.deepEqual(moneyVerbIntentRows({ content_id: null }), [["Asset", "—"]]);
  assert.deepEqual(moneyVerbIntentRows({ seller: { a: 1 } }), [["Seller", '{"a":1}']]);
});

test("row count is bounded so a frame cannot push the buttons off screen", () => {
  const flooded = {};
  for (let index = 0; index < 200; index += 1) {
    flooded[`field_${index}`] = index;
  }
  assert.equal(moneyVerbIntentRows(flooded).length, 24);
});

test("a non-object request yields no rows rather than throwing", () => {
  for (const bad of [null, undefined, "string", 7, ["a"]]) {
    assert.deepEqual(moneyVerbIntentRows(bad), []);
  }
});

test("the decline message is distinct from a verification failure", () => {
  assert.equal(typeof SPEND_DECLINED_MESSAGE, "string");
  assert.ok(SPEND_DECLINED_MESSAGE.length > 0);
  assert.ok(!/passkey|step-up/i.test(SPEND_DECLINED_MESSAGE));
});
