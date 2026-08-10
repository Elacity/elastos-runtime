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
 * 100 modal/passkey · 90 QL/shortcuts/context · 80 CC/NC/menus ·
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

export function closeOtherShellPopovers(exceptId) {
  for (const [id, close] of registry) {
    if (id === exceptId) {
      continue;
    }
    /*
      Apps is a Shelf face (mode), not a transient popover. Closing it belongs
      to desktop click-off / Apps toggle / Agent face — not context menus etc.
    */
    if (id === "launcher") {
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
