import assert from "node:assert/strict";
import test from "node:test";

import {
  PHONE_DOCK_SLOTS,
  fitPhoneHomePages,
  movePhoneDockTarget,
  movePhoneHomeTarget,
  normalizePhoneHomePages,
  phoneDockTargetIds,
  phoneHomePages,
  phoneHomeTargets,
  removePhoneHomeTarget,
} from "../capsules/home-gui/browser/shell-phone-home.js";

const app = (target, extra = {}) => ({ target, title: target, ...extra });
const TARGETS = [
  app("browser"),
  app("library"),
  app("notes", { target_kind: "object" }),
  app("marketplace"),
  app("wallet"),
  app("documents"),
  app("system"),
  app("people"),
];

const PINS = ["browser", "library", "marketplace", "wallet", "documents", "system"];

test("until the phone is arranged its Dock holds the first four Shelf pins, in Shelf order", () => {
  assert.equal(PHONE_DOCK_SLOTS, 4);
  assert.deepEqual(
    phoneDockTargetIds(TARGETS, ["wallet", "browser", "library", "marketplace", "documents", "system"], undefined),
    ["wallet", "browser", "library", "marketplace"],
  );
  assert.deepEqual(phoneDockTargetIds(TARGETS, ["browser"], null), ["browser"]);
  assert.deepEqual(phoneDockTargetIds(TARGETS, [], null), []);
});

test("an arranged phone Dock is the person's own, whatever the Shelf holds", () => {
  assert.deepEqual(phoneDockTargetIds(TARGETS, PINS, ["people", "wallet"]), ["people", "wallet"]);
  assert.deepEqual(phoneDockTargetIds(TARGETS, PINS, []), []);
});

test("a stored phone Dock drops what this Home lacks, repeats and the Assistant, and holds four", () => {
  assert.deepEqual(
    phoneDockTargetIds([...TARGETS, app("assistant")], PINS, ["gone", "people", "people", "assistant", "wallet", "system", "library", "browser"]),
    ["people", "wallet", "system", "library"],
  );
});

test("an app is in the Dock or on the Home grid, never both, and never missing", () => {
  const dock = phoneDockTargetIds(TARGETS, PINS, undefined);
  const home = phoneHomeTargets(TARGETS, dock).map((entry) => entry.target);
  assert.equal(home.some((target) => dock.includes(target)), false);
  assert.deepEqual([...dock, ...home].sort(), TARGETS.map((entry) => entry.target).sort());
});

test("Shelf pins past the Dock's slots stay on the grid", () => {
  const home = phoneHomeTargets(TARGETS, phoneDockTargetIds(TARGETS, PINS, undefined));
  assert.deepEqual(home.map((entry) => entry.target), ["documents", "system", "people", "notes"]);
});

test("the grid keeps the launcher's order: apps first, then Library items", () => {
  const home = phoneHomeTargets(TARGETS, []);
  assert.deepEqual(
    home.map((entry) => entry.target),
    ["browser", "library", "marketplace", "wallet", "documents", "system", "people", "notes"],
  );
});

test("the Assistant keeps its own Dock tile and never takes a grid slot", () => {
  const home = phoneHomeTargets([app("assistant"), app("people")], []);
  assert.deepEqual(home.map((entry) => entry.target), ["people"]);
});

test("the grid carries the targets themselves, so titles and glyphs come along", () => {
  const [first] = phoneHomeTargets([app("people", { title: "People" })], []);
  assert.equal(first.title, "People");
});

const ids = (pages) => pages.map((page) => page.map((entry) => entry.target));

test("with nothing arranged the Home is one page in the launcher's order", () => {
  assert.deepEqual(ids(phoneHomePages(TARGETS, ["browser"], undefined)), [
    ["library", "marketplace", "wallet", "documents", "system", "people", "notes"],
  ]);
  assert.deepEqual(ids(phoneHomePages([], [], null)), [[]]);
});

test("stored pages keep the person's arrangement", () => {
  const stored = [["people", "documents"], ["wallet", "library"]];
  assert.deepEqual(ids(phoneHomePages(TARGETS, ["browser", "marketplace", "system", "notes"], stored)), [
    ["people", "documents"],
    ["wallet", "library"],
  ]);
});

test("stored pages drop what the grid no longer holds and gain new installs on the last page", () => {
  const stored = [["gone", "people", "browser"], ["people", "gone-too"], ["wallet"]];
  assert.deepEqual(ids(phoneHomePages(TARGETS, ["browser"], stored)), [
    ["people"],
    ["wallet", "library", "marketplace", "documents", "system", "notes"],
  ]);
});

test("stored pages are shape-checked before use", () => {
  assert.equal(normalizePhoneHomePages("people"), null);
  assert.equal(normalizePhoneHomePages(undefined), null);
  assert.deepEqual(normalizePhoneHomePages([["people", 3, ""], "wallet", ["wallet"]]), [["people"], ["wallet"]]);
});

test("a page longer than the screen holds runs on to the next page", () => {
  assert.deepEqual(fitPhoneHomePages([["a", "b", "c", "d", "e"], ["f"]], 2), [["a", "b"], ["c", "d"], ["e"], ["f"]]);
  assert.deepEqual(fitPhoneHomePages([[]], 4), [[]]);
  assert.deepEqual(fitPhoneHomePages([["a"]], 0), [["a"]]);
});

test("a drop reorders within a page", () => {
  assert.deepEqual(movePhoneHomeTarget([["a", "b", "c"]], "c", 0, 0, 4), [["c", "a", "b"]]);
  assert.deepEqual(movePhoneHomeTarget([["a", "b", "c"]], "a", 0, 2, 4), [["b", "c", "a"]]);
});

test("a drop onto a full page hands its last icon to the next page", () => {
  assert.deepEqual(
    movePhoneHomeTarget([["a", "b"], ["c", "d"], ["e"]], "e", 0, 0, 2),
    [["e", "a"], ["b", "c"], ["d"]],
  );
});

test("a drop one page past the end opens a new page, and emptied pages close", () => {
  assert.deepEqual(movePhoneHomeTarget([["a", "b"]], "b", 1, 0, 4), [["a"], ["b"]]);
  assert.deepEqual(movePhoneHomeTarget([["a"], ["b"]], "a", 1, 1, 4), [["b", "a"]]);
  assert.deepEqual(movePhoneHomeTarget([["a"]], "z", 9, 0, 4), [["a"], ["z"]]);
});

test("an app dragged into the Dock leaves the grid", () => {
  assert.deepEqual(removePhoneHomeTarget([["a", "b"], ["c"]], "c"), [["a", "b"]]);
});

test("a Dock drop reorders within the Dock and takes a newcomer only while there is room", () => {
  assert.deepEqual(movePhoneDockTarget(["a", "b", "c", "d"], "d", 0), ["d", "a", "b", "c"]);
  assert.deepEqual(movePhoneDockTarget(["a", "b"], "z", 1), ["a", "z", "b"]);
  assert.deepEqual(movePhoneDockTarget(["a", "b"], "z", 9), ["a", "b", "z"]);
  assert.equal(movePhoneDockTarget(["a", "b", "c", "d"], "z", 0), null);
});
