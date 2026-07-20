/* One popover at a time (macOS). Surfaces self-register a close function;
   each show* calls closeOtherShellPopovers(ownId) before revealing itself.
   Registry only — this module holds no DOM and no state of its own. */

const registry = new Map();

export function registerShellPopover(id, close) {
  if (typeof id !== "string" || !id || typeof close !== "function") {
    return;
  }
  registry.set(id, close);
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
