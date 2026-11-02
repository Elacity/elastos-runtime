/* One popover at a time (macOS). Surfaces self-register a close function;
   each show* calls closeOtherShellPopovers(ownId) before revealing itself.
   Registry only — this module holds no DOM and no state of its own.
   Escape stack (Shell Continuity II): higher priority dismisses first.
   See DESIGN_SYSTEM / plan Appendix — modal > overlay > rails > MC > stage. */

const registry = new Map();
/** @type {Map<string, { priority: number, isActive: () => boolean, dismiss: () => void }>} */
const escapeHandlers = new Map();

export function registerShellPopover(id, close) {
  if (typeof id !== "string" || !id || typeof close !== "function") {
    return;
  }
  registry.set(id, close);
}

/**
 * Ordered Escape dismiss. Priority examples:
 * 100 modal/auth · 90 QL/shortcuts/context · 80 CC/NC/menus ·
 * 70 rails · 60 Mission Control / Show Windows · 50 fullscreen stage
 */
export function registerEscapeHandler(id, { priority = 50, isActive, dismiss } = {}) {
  if (typeof id !== "string" || !id || typeof isActive !== "function" || typeof dismiss !== "function") {
    return;
  }
  escapeHandlers.set(id, { priority, isActive, dismiss });
}

export function unregisterEscapeHandler(id) {
  escapeHandlers.delete(id);
}

/** @returns {boolean} true if something was dismissed */
export function handleShellEscape() {
  const active = [...escapeHandlers.entries()]
    .map(([id, handler]) => ({ id, ...handler }))
    .filter((handler) => {
      try {
        return handler.isActive() === true;
      } catch (_error) {
        return false;
      }
    })
    .sort((a, b) => b.priority - a.priority);
  const top = active[0];
  if (!top) {
    return false;
  }
  try {
    top.dismiss();
  } catch (_error) {
    return false;
  }
  return true;
}

/* Back layers (the phone's system Back). Every surface a person can dismiss
   registers once, next to its own close function:
   - selector: the element whose stacking order ranks the layer; it also ties
     the layer to HOME_GUI_HOST_SELECTORS so a host surface cannot be missed;
   - state (optional): the owner's own report, one of LAYER_STATE, read from
     the element that actually animates. The default is for a surface whose
     registered element is the one that animates: closed while not shown,
     closing while its dismissWithMotion leave plays, open otherwise;
   - close: the same dismissal as the surface's own Close (animation included).
   The phone stage closes exactly one layer per Back. The layer it took stops
   counting until it reports closed, or reports open again after it reported
   closing (a reopen mid-dismiss is a fresh open layer). No Escape is
   synthesised: a raw Escape would also reach the ordered stack above (and,
   say, leave a fullscreen Space). */
export const LAYER_STATE = Object.freeze({ open: "open", closing: "closing", closed: "closed" });
const stageLayers = new Map();

export function registerStageLayer(id, { selector, state = null, close } = {}) {
  if (typeof id !== "string" || !id || typeof selector !== "string" || !selector || typeof close !== "function") {
    return;
  }
  stageLayers.set(id, { id, selector, state: typeof state === "function" ? state : null, close });
}

export function registeredStageLayers() {
  return [...stageLayers.values()];
}

/* Gestures in progress (title-bar swipe, long-press, drag handles, window
   drags). Each registers a cancel next to its own state; a stage reset calls
   every cancel before it blocks input, so a press that began in the old form
   factor never completes, or moves anything, in the new one. A cancel with
   nothing in progress is a no-op. */
const stageGestures = new Map();

export function registerStageGesture(id, cancel) {
  if (typeof id !== "string" || !id || typeof cancel !== "function") {
    return;
  }
  stageGestures.set(id, cancel);
}

export function registeredStageGestures() {
  return [...stageGestures.keys()];
}

export function cancelStageGestures() {
  for (const cancel of stageGestures.values()) {
    try {
      cancel();
    } catch (_error) {
      // One gesture failing to cancel must not keep the others running.
    }
  }
}

export function closeOtherShellPopovers(exceptId) {
  for (const [id, close] of registry) {
    if (id === exceptId) {
      continue;
    }
    try {
      close();
    } catch (_error) {
      // Surface mid-teardown.
    }
  }
}

/* Toggle hidden + inert + aria-hidden symmetrically. Overlays that ship with
   HTML inert must clear it on open or keyboard/AT cannot reach them. */
export function setOverlayOpen(el, open, { invoker, focusEl } = {}) {
  if (!el) {
    return;
  }
  el.hidden = !open;
  el.inert = !open;
  el.setAttribute("aria-hidden", open ? "false" : "true");
  if (open) {
    if (invoker && invoker !== document.body) {
      el._overlayInvoker = invoker;
    } else if (!el._overlayInvoker) {
      const active = document.activeElement;
      el._overlayInvoker =
        active && active !== document.body ? active : null;
    }
    const target = focusEl || el;
    queueMicrotask(() => {
      try {
        target?.focus?.({ preventScroll: true });
      } catch (_error) {
        // Focus can fail on detached nodes during teardown.
      }
    });
    return;
  }
  const restore = el._overlayInvoker;
  el._overlayInvoker = null;
  if (restore && typeof restore.focus === "function") {
    try {
      restore.focus({ preventScroll: true });
    } catch (_error) {
      // Invoker may have been removed.
    }
  }
}
