import assert from "node:assert/strict";
import test from "node:test";

import {
  bindSheetHandle,
  sheetDragDismisses,
  sheetDragOffset,
  SHEET_DISMISS_DRAG_PX,
  SHEET_DRAG_DOWN,
  SHEET_DRAG_UP,
} from "../capsules/home-gui/browser/shell-sheet-handle.js";

function fakeNode() {
  const listeners = new Map();
  const style = {
    removeProperty(name) {
      delete this[name];
    },
  };
  return {
    dataset: {},
    style,
    addEventListener: (type, listener) => listeners.set(type, [...(listeners.get(type) || []), listener]),
    dispatch(type, init = {}) {
      const event = { preventDefault() { this.defaultPrevented = true; }, pointerId: 1, ...init };
      for (const listener of listeners.get(type) || []) {
        listener(event);
      }
      return event;
    },
    listenerCount: (type) => (listeners.get(type) || []).length,
  };
}

function boundHandle(options = {}) {
  const handle = fakeNode();
  const sheet = fakeNode();
  let closes = 0;
  bindSheetHandle(handle, { sheet, close: () => { closes += 1; }, ...options });
  return { handle, sheet, closes: () => closes };
}

function drag(handle, fromY, toY) {
  handle.dispatch("pointerdown", { clientY: fromY });
  handle.dispatch("pointermove", { clientY: toY });
  handle.dispatch("pointerup", { clientY: toY });
  handle.dispatch("click");
}

test("a bar sheet follows an upward drag only", () => {
  assert.equal(sheetDragOffset(SHEET_DRAG_UP, -30), -30);
  assert.equal(sheetDragOffset(SHEET_DRAG_UP, 30), 0);
  assert.equal(sheetDragOffset(SHEET_DRAG_DOWN, 30), 30);
  assert.equal(sheetDragOffset(SHEET_DRAG_DOWN, -30), 0);
});

test("only a drag of the dismiss distance toward the origin dismisses", () => {
  assert.equal(sheetDragDismisses(SHEET_DRAG_UP, -SHEET_DISMISS_DRAG_PX), true);
  assert.equal(sheetDragDismisses(SHEET_DRAG_UP, -(SHEET_DISMISS_DRAG_PX - 1)), false);
  assert.equal(sheetDragDismisses(SHEET_DRAG_UP, SHEET_DISMISS_DRAG_PX * 3), false);
  assert.equal(sheetDragDismisses(SHEET_DRAG_DOWN, SHEET_DISMISS_DRAG_PX), true);
  assert.equal(sheetDragDismisses(SHEET_DRAG_DOWN, -SHEET_DISMISS_DRAG_PX * 3), false);
});

test("a tap on a Close handle closes once; a drag-only grabber ignores taps", () => {
  const button = boundHandle({ direction: SHEET_DRAG_UP, tapCloses: true });
  drag(button.handle, 400, 402);
  assert.equal(button.closes(), 1);

  const grabber = boundHandle({ direction: SHEET_DRAG_DOWN });
  drag(grabber.handle, 400, 402);
  assert.equal(grabber.closes(), 0);
  assert.equal(grabber.handle.listenerCount("click"), 0);
});

test("a long drag closes once, and the click it ends with is not a second close", () => {
  const { handle, sheet, closes } = boundHandle({ direction: SHEET_DRAG_UP, tapCloses: true });
  handle.dispatch("pointerdown", { clientY: 400 });
  handle.dispatch("pointermove", { clientY: 380 });
  assert.equal(sheet.style.translate, "0 -20px");
  handle.dispatch("pointerup", { clientY: 400 - SHEET_DISMISS_DRAG_PX - 10 });
  handle.dispatch("click");
  assert.equal(closes(), 1);
  assert.equal(sheet.style.translate, undefined);
});

test("a short drag snaps back without closing", () => {
  const { handle, sheet, closes } = boundHandle({ direction: SHEET_DRAG_DOWN, tapCloses: true });
  drag(handle, 400, 420);
  assert.equal(closes(), 0);
  assert.equal(sheet.style.translate, undefined);
});

test("a cancelled drag snaps back", () => {
  const { handle, sheet, closes } = boundHandle({ direction: SHEET_DRAG_UP });
  handle.dispatch("pointerdown", { clientY: 400 });
  handle.dispatch("pointermove", { clientY: 300 });
  handle.dispatch("pointercancel");
  handle.dispatch("pointerup", { clientY: 300 });
  assert.equal(closes(), 0);
  assert.equal(sheet.style.translate, undefined);
});

test("pressing the handle keeps focus where it was", () => {
  const { handle } = boundHandle({ direction: SHEET_DRAG_UP, tapCloses: true });
  assert.equal(handle.dispatch("pointerdown", { clientY: 0 }).defaultPrevented, true);
});

test("binding twice does not double the listeners", () => {
  const handle = fakeNode();
  const sheet = fakeNode();
  const options = { sheet, direction: SHEET_DRAG_UP, close: () => {}, tapCloses: true };
  bindSheetHandle(handle, options);
  bindSheetHandle(handle, options);
  assert.equal(handle.listenerCount("pointerdown"), 1);
  assert.equal(handle.listenerCount("click"), 1);
});
