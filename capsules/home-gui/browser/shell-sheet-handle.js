/* Phone sheet grab handles. A sheet that drops from the bar (Control Centre,
   Notification Centre, Spotlight) closes when its handle is dragged up; the
   launcher, which grows from the Dock, closes when dragged down. The sheet
   follows the finger toward dismissal only and snaps back if the drag stops
   short. On the bar sheets the handle is a button, so a tap, Enter or Space
   closes too; the launcher's grabber is drag-only because the Dock's Apps
   button under it already toggles it. Hidden off phone. docs/HOME_MOBILE.md. */

// A drag this far toward the sheet's origin dismisses it; shorter snaps back.
export const SHEET_DISMISS_DRAG_PX = 48;
// Movement under this is still a tap.
export const SHEET_HANDLE_TAP_SLOP_PX = 10;

export const SHEET_DRAG_UP = "up";
export const SHEET_DRAG_DOWN = "down";

// How far the sheet follows a vertical drag: all the way toward dismissal,
// not at all the other way.
export function sheetDragOffset(direction, deltaY) {
  if (direction === SHEET_DRAG_UP) {
    return Math.min(0, deltaY);
  }
  return Math.max(0, deltaY);
}

export function sheetDragDismisses(direction, deltaY) {
  return Math.abs(sheetDragOffset(direction, deltaY)) >= SHEET_DISMISS_DRAG_PX;
}

export function bindSheetHandle(handle, { sheet, direction, close, tapCloses = false }) {
  if (!handle || !sheet || typeof close !== "function" || handle.dataset.sheetHandleBound === "true") {
    return;
  }
  handle.dataset.sheetHandleBound = "true";
  let startY = null;
  let dragged = false;

  const settle = () => {
    startY = null;
    sheet.style.removeProperty("translate");
  };

  handle.addEventListener("pointerdown", (event) => {
    // Keeps focus (and a soft keyboard) where it was; the click still fires.
    event.preventDefault();
    startY = event.clientY;
    dragged = false;
    handle.setPointerCapture?.(event.pointerId);
  });
  handle.addEventListener("pointermove", (event) => {
    if (startY === null) {
      return;
    }
    const deltaY = event.clientY - startY;
    if (Math.abs(deltaY) > SHEET_HANDLE_TAP_SLOP_PX) {
      dragged = true;
    }
    sheet.style.translate = `0 ${sheetDragOffset(direction, deltaY)}px`;
  });
  handle.addEventListener("pointerup", (event) => {
    if (startY === null) {
      return;
    }
    const dismiss = sheetDragDismisses(direction, event.clientY - startY);
    settle();
    if (dismiss) {
      close();
    }
  });
  handle.addEventListener("pointercancel", settle);
  if (tapCloses) {
    handle.addEventListener("click", () => {
      if (dragged) {
        dragged = false;
        return;
      }
      close();
    });
  }
}
