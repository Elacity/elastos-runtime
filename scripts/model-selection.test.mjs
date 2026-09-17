import test from "node:test";
import assert from "node:assert/strict";
import { catalogModels, selectedModelOffer, readyModelChoices, validModelCid } from "../capsules/_shared/model-selection.js";

const cid = `bafybei${"a".repeat(52)}`;
const row = { source: "signed-model-catalog", role: "content", installed: false, launchable: false,
  cid, title: "Selected model", signature_state: "catalog-signature-verified",
  model_runtime: { admitted: true, dispatch_ready: true, offer_id: "operator-chosen-id" } };
const catalog = rows => ({ schema: "elastos.capsules.catalog/v1", model_catalog_state: "verified", capsules: rows });
const offer = { id: "operator-chosen-id" };

test("CID intent requires exact current unique ready mapping; service intent has no prefix inference", () => {
  const models = catalogModels(catalog([row]));
  assert.equal(selectedModelOffer([offer], offer.id, cid, models), offer);
  assert.deepEqual(readyModelChoices(models, [offer]), models);
  for (const unavailable of [[], [{ ...models[0], ready: false }], [{ ...models[0], offerId: "other" }], [models[0], models[0]]]) {
    assert.equal(selectedModelOffer([offer], offer.id, cid, unavailable), null);
  }
  assert.equal(selectedModelOffer([offer, offer], offer.id, cid, models), null);
  const hosted = { id: `model:${"a".repeat(64)}` };
  assert.equal(selectedModelOffer([hosted], hosted.id, null, []), hosted);
  assert.equal(selectedModelOffer([hosted], hosted.id, cid, []), null);
});

test("catalog accepts distinct signed entries and exact CID selection", () => {
  const otherCid = `bafybei${"c".repeat(51)}e`;
  const other = { ...row, cid: otherCid, title: "Other model",
    model_runtime: { admitted: true, dispatch_ready: true, offer_id: "other-offer-id" } };
  const models = catalogModels(catalog([row, other]));
  assert.equal(models.length, 2);
  assert.equal(selectedModelOffer([offer], offer.id, cid, models), offer);
  const otherOffer = { id: "other-offer-id" };
  assert.equal(selectedModelOffer([otherOffer], otherOffer.id, otherCid, models), otherOffer);
  assert.equal(selectedModelOffer([offer], offer.id, otherCid, models), null);
});

test("catalog rejects duplicate, malformed and noncanonical content identity", () => {
  assert.throws(() => catalogModels(catalog([row, row])));
  for (const value of [cid.toUpperCase(), `${cid} `, `bafybei${"a".repeat(51)}b`, "https://model.invalid"]) {
    assert.equal(validModelCid(value), false);
    assert.throws(() => catalogModels(catalog([{ ...row, cid: value }])));
  }
  assert.throws(() => catalogModels(catalog([{ ...row, model_runtime: { admitted: false, dispatch_ready: true, offer_id: offer.id } }])));
  assert.deepEqual(catalogModels({ ...catalog([row]), model_catalog_state: "unavailable" }), []);
});
