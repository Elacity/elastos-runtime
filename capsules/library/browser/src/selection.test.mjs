import assert from "node:assert/strict";
import test from "node:test";

import { createLibrarySelection } from "./selection.js";

test("Library selection exposes selected state and follows the visible order for ranges", () => {
  const objects = ["c", "a", "b"].map((name) => ({ uri: `localhost://Users/test/Documents/${name}`, name }));
  const items = objects.map(({ uri }) => ({
    dataset: { uri }, attributes: new Map(),
    getAttribute(name) { return this.attributes.get(name) || null; },
    setAttribute(name, value) { this.attributes.set(name, value); },
  }));
  const state = { objectsByUri: new Map(objects.map((object) => [object.uri, object])),
    selectedUris: new Set(), selectionAnchorUri: "" };
  let footerRenders = 0;
  const selection = createLibrarySelection({
    content: { querySelectorAll: () => items }, state,
    visibleObjects: () => objects,
    renderFooter: () => { footerRenders += 1; },
  });
  function selectedNames() { return selection.selectedObjects().map((object) => object.name); }
  function assertAccessibleSelection() {
    for (const item of items) {
      const expected = String(state.selectedUris.has(item.dataset.uri));
      assert.equal(item.dataset.selected, expected);
      assert.equal(item.attributes.get("aria-selected"), expected);
    }
  }
  selection.selectOnly(objects[0].uri);
  selection.selectRangeTo(objects[2].uri);
  assert.deepEqual(selectedNames(), ["c", "a", "b"]);
  assertAccessibleSelection();
  selection.selectRangeTo(objects[1].uri);
  assert.deepEqual(selectedNames(), ["c", "a"]);
  assertAccessibleSelection();
  selection.toggleSelected(objects[0].uri);
  assert.deepEqual(selectedNames(), ["a"]);
  assertAccessibleSelection();
  selection.clearSelection();
  assert.deepEqual(selectedNames(), []);
  assertAccessibleSelection();
  selection.prepareDragSelection(objects[2].uri, items[2]);
  assert.deepEqual(selectedNames(), ["b"]);
  assertAccessibleSelection();
  selection.selectAllVisible();
  assert.deepEqual(selectedNames(), ["c", "a", "b"]);
  assertAccessibleSelection();
  assert.equal(footerRenders, 7);
});

test("Library selection only changes DOM attributes when their selected state changes", () => {
  let mutations = 0;
  const objects = ["a", "b"].map((name) => ({ uri: `localhost://Users/test/Documents/${name}` }));
  const items = objects.map(({ uri }) => ({
    dataset: new Proxy({ uri, selected: "false" }, {
      set(target, name, value) { mutations += 1; target[name] = value; return true; },
    }),
    attributes: new Map([["aria-selected", "false"]]),
    getAttribute(name) { return this.attributes.get(name) || null; },
    setAttribute(name, value) { mutations += 1; this.attributes.set(name, value); },
  }));
  const state = { objectsByUri: new Map(objects.map((object) => [object.uri, object])),
    selectedUris: new Set(), selectionAnchorUri: "" };
  const selection = createLibrarySelection({
    content: { querySelectorAll: () => items }, state,
    visibleObjects: () => objects, renderFooter() {},
  });
  selection.clearSelection();
  assert.equal(mutations, 0);
  selection.selectOnly(objects[0].uri);
  assert.equal(mutations, 2);
  selection.selectOnly(objects[0].uri);
  assert.equal(mutations, 2, "An unchanged selection must keep DOM attributes untouched");
  selection.selectOnly(objects[1].uri);
  assert.equal(mutations, 6, "Moving selection must update both attributes on only the old and new items");
  assert.equal(items[0].getAttribute("aria-selected"), "false");
  assert.equal(items[1].getAttribute("aria-selected"), "true");
  items[1].attributes.set("aria-selected", "false");
  selection.syncSelectionDom();
  assert.equal(mutations, 7, "Accessible state must be repaired even when the visual state is already correct");
  assert.equal(items[1].getAttribute("aria-selected"), "true");
});
