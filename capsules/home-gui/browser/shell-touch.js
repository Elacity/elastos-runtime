/* Touch long-press. iOS Safari never turns a held finger into a `contextmenu`
   event (Android Chrome does), so every shell menu bound to `contextmenu` —
   Dock, launcher, Bin, the empty desktop — was mouse-only on an iPhone. A
   touch or pen held still for LONG_PRESS_MS dispatches one `contextmenu` at
   the pressed element, so the existing bindings open their menus unchanged.
   When a menu took it (the event was cancelled) the release does not click;
   a slow tap on anything without a menu still clicks. Android's own
   `contextmenu` and ours never both land. Desktop icons opt out: their own
   long-press also arms the touch drag. docs/HOME_MOBILE.md. */

export const LONG_PRESS_MS = 500;
// A finger that drifts this far is scrolling or dragging, not holding.
export const LONG_PRESS_DRIFT_PX = 10;
// The click a released long-press would produce arrives well inside this.
export const LONG_PRESS_CLICK_SWALLOW_MS = 600;

function isTouchLike(event) {
  return event.pointerType === "touch" || event.pointerType === "pen";
}

function defaultContextMenuEvent({ clientX, clientY }) {
  return new MouseEvent("contextmenu", {
    bubbles: true,
    cancelable: true,
    composed: true,
    button: 2,
    clientX,
    clientY,
  });
}

// Binds on `root` in the capture phase, so element handlers that stop
// propagation cannot hide a press from it. Returns `swallowNextClick` for
// callers that dismiss a menu on pointerdown and must not let that tap land
// on whatever sat under the menu.
export function bindTouchLongPress(root, {
  skip = "",
  setTimer = (callback, ms) => window.setTimeout(callback, ms),
  clearTimer = (timer) => window.clearTimeout(timer),
  now = () => (window.performance ? window.performance.now() : Date.now()),
  makeContextMenuEvent = defaultContextMenuEvent,
} = {}) {
  let press = null;
  let swallowClickUntil = 0;

  const cancel = () => {
    if (press && press.timer !== null) {
      clearTimer(press.timer);
    }
    press = null;
  };

  const swallowNextClick = () => {
    swallowClickUntil = now() + LONG_PRESS_CLICK_SWALLOW_MS;
  };

  const fire = () => {
    if (!press) {
      return;
    }
    press.timer = null;
    const event = makeContextMenuEvent({ clientX: press.x, clientY: press.y });
    // dispatchEvent returns false when a listener called preventDefault,
    // which is how every shell menu binding says it opened.
    press.handled = !press.target.dispatchEvent(event);
    press.fired = true;
  };

  root.addEventListener("pointerdown", (event) => {
    cancel();
    if (!isTouchLike(event) || typeof event.target?.dispatchEvent !== "function") {
      return;
    }
    if (skip && event.target.closest?.(skip)) {
      return;
    }
    press = {
      pointerId: event.pointerId,
      target: event.target,
      x: event.clientX,
      y: event.clientY,
      fired: false,
      handled: false,
      timer: null,
    };
    press.timer = setTimer(fire, LONG_PRESS_MS);
  }, true);

  root.addEventListener("pointermove", (event) => {
    if (!press || event.pointerId !== press.pointerId || press.fired) {
      return;
    }
    if (Math.hypot(event.clientX - press.x, event.clientY - press.y) >= LONG_PRESS_DRIFT_PX) {
      cancel();
    }
  }, true);

  root.addEventListener("pointerup", (event) => {
    if (!press || event.pointerId !== press.pointerId) {
      return;
    }
    if (press.handled) {
      swallowNextClick();
    }
    cancel();
  }, true);

  root.addEventListener("pointercancel", (event) => {
    if (press && event.pointerId === press.pointerId) {
      cancel();
    }
  }, true);

  root.addEventListener("contextmenu", (event) => {
    if (!event.isTrusted || !press) {
      return;
    }
    if (press.fired) {
      // Ours already opened the menu; the browser's own would open it twice.
      event.preventDefault();
      event.stopImmediatePropagation();
      return;
    }
    // The browser's own long-press (Android) came first: it opens the menu
    // and ours stands down, but the release still must not click.
    clearTimer(press.timer);
    press.timer = null;
    press.fired = true;
    press.handled = true;
  }, true);

  root.addEventListener("click", (event) => {
    if (swallowClickUntil === 0) {
      return;
    }
    const swallow = now() < swallowClickUntil;
    swallowClickUntil = 0;
    if (swallow) {
      event.preventDefault();
      event.stopImmediatePropagation();
    }
  }, true);

  return { swallowNextClick };
}
