import assert from "node:assert/strict";
import test from "node:test";

import {
  PHONE_DOCK_SLOTS,
  phoneDockTargetIds,
  phoneHomeTargets,
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

test("the phone Dock holds the first four Shelf pins, in Shelf order", () => {
  assert.equal(PHONE_DOCK_SLOTS, 4);
  assert.deepEqual(
    phoneDockTargetIds(["wallet", "browser", "library", "marketplace", "documents", "system"]),
    ["wallet", "browser", "library", "marketplace"],
  );
  assert.deepEqual(phoneDockTargetIds(["browser"]), ["browser"]);
  assert.deepEqual(phoneDockTargetIds([]), []);
});

test("an app is in the Dock or on the Home grid, never both, and never missing", () => {
  const pins = ["browser", "library", "marketplace", "wallet", "documents", "system"];
  const home = phoneHomeTargets(TARGETS, pins).map((entry) => entry.target);
  const dock = phoneDockTargetIds(pins);
  assert.equal(home.some((target) => dock.includes(target)), false);
  assert.deepEqual([...dock, ...home].sort(), TARGETS.map((entry) => entry.target).sort());
});

test("Shelf pins past the Dock's slots stay on the grid", () => {
  const home = phoneHomeTargets(TARGETS, ["browser", "library", "marketplace", "wallet", "documents", "system"]);
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
