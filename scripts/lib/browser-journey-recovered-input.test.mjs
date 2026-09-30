import assert from "node:assert/strict";
import test from "node:test";
import { recoveredInputClickRect, recoveredInputLiftPx } from "./browser-journey-recovered-input.mjs";

const field = { x: 96, y: 192, width: 240, height: 40 };

test("a visible rest field needs no lift", () => {
  const result = recoveredInputClickRect(field, field);
  assert.equal(result.lift, 0);
  assert.equal(result.rect.y, 192);
});

test("a field below the viewport lifts once to the rest rectangle", () => {
  const last = { ...field, y: -767.6875 };
  const result = recoveredInputClickRect(last, field);
  assert.equal(result.lift, 1280);
  assert.equal(result.rect.y, 192);
  assert.equal(recoveredInputLiftPx(last), 1280);
});

test("a guessed lift without a rest rectangle stays at or above y 0", () => {
  const last = { ...field, y: -173.6875 };
  const result = recoveredInputClickRect(last, null);
  assert.equal(result.lift, 640);
  assert.equal(result.rect.y, 466.3125);
});
