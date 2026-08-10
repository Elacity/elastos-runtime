/* Shared enter/leave dismiss for shell chrome popovers.
   Presentation only — no authority. Prefer this over instant `hidden = true`
   so menus and Spotlight flow away instead of hard-cutting. */

const DEFAULT_LEAVE_MS = 120;
const leaveTokens = new WeakMap();

export function prefersReducedMotion() {
  return Boolean(window.matchMedia?.("(prefers-reduced-motion: reduce)")?.matches);
}

export function cancelDismissMotion(el) {
  if (!el) {
    return;
  }
  const token = leaveTokens.get(el);
  if (!token) {
    return;
  }
  if (token.timer) {
    window.clearTimeout(token.timer);
  }
  if (token.onEnd) {
    el.removeEventListener("animationend", token.onEnd);
  }
  if (token.className) {
    el.classList.remove(token.className);
  }
  leaveTokens.delete(el);
}

/** Call before revealing a surface so a mid-leave reopen cancels cleanly. */
export function prepareSurfaceOpen(el) {
  cancelDismissMotion(el);
}

/**
 * Soft-dismiss: play leave animation, then optionally hide.
 * @param {HTMLElement|null} el
 * @param {{
 *   className?: string,
 *   ms?: number,
 *   hide?: boolean,
 *   animate?: boolean,
 *   onDone?: () => void,
 * }} [options]
 */
export function dismissWithMotion(el, options = {}) {
  if (!el || el.hidden) {
    options.onDone?.();
    return;
  }
  cancelDismissMotion(el);
  const className = typeof options.className === "string" && options.className
    ? options.className
    : "shell-surface-leaving";
  const ms = Number.isFinite(options.ms) ? Math.max(0, options.ms) : DEFAULT_LEAVE_MS;
  const shouldHide = options.hide !== false;
  const finish = () => {
    const current = leaveTokens.get(el);
    if (current && current.finish !== finish) {
      return;
    }
    cancelDismissMotion(el);
    if (shouldHide) {
      el.hidden = true;
    }
    options.onDone?.();
  };
  if (options.animate === false || prefersReducedMotion() || ms === 0) {
    finish();
    return;
  }
  const onEnd = (event) => {
    if (event.target !== el) {
      return;
    }
    finish();
  };
  const token = { className, onEnd, timer: 0, finish };
  leaveTokens.set(el, token);
  el.classList.add(className);
  el.addEventListener("animationend", onEnd);
  token.timer = window.setTimeout(finish, ms + 40);
}
