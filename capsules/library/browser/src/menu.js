export function createLibraryMenu({ contextMenu, perf, showError }) {
  function menuAction(label, action, options = {}) {
    const children = Array.isArray(options.children)
      ? normalizedMenuActions(options.children)
      : [];
    return {
      label,
      action,
      children,
      disabled: !!options.disabled || (!children.length && typeof action !== "function"),
      checked: !!options.checked,
    };
  }

  function normalizedMenuActions(actions) {
    const normalized = [];
    for (const entry of actions) {
      if (entry === "-") {
        if (normalized.length && normalized[normalized.length - 1] !== "-") normalized.push(entry);
        continue;
      }
      const item = Array.isArray(entry)
        ? { label: entry[0], action: entry[1], disabled: !!entry[2] }
        : { ...entry };
      if (Array.isArray(item.children)) {
        item.children = normalizedMenuActions(item.children);
      }
      normalized.push(item);
    }
    while (normalized[0] === "-") normalized.shift();
    while (normalized[normalized.length - 1] === "-") normalized.pop();
    return normalized;
  }

  function renderMenu(actions, x, y) {
    const startedAt = performance.now();
    contextMenu.innerHTML = "";
    const menuActions = normalizedMenuActions(actions);
    if (!menuActions.length) {
      hideMenu();
      return;
    }
    renderMenuEntries(contextMenu, menuActions);
    contextMenu.style.visibility = "hidden";
    contextMenu.classList.remove("hidden");
    const rect = contextMenu.getBoundingClientRect();
    const margin = 8;
    const left = Math.max(margin, Math.min(x, window.innerWidth - rect.width - margin));
    const top = Math.max(margin, Math.min(y, window.innerHeight - rect.height - margin));
    contextMenu.style.left = left + "px";
    contextMenu.style.top = top + "px";
    contextMenu.dataset.submenuSide = left + rect.width + 226 > window.innerWidth ? "left" : "right";
    contextMenu.style.visibility = "";
    perf.menuRenderCount += 1;
    perf.lastMenuRender = {
      durationMs: performance.now() - startedAt,
      actionCount: countMenuActions(menuActions),
    };
  }

  function renderMenuEntries(container, actions) {
    for (const entry of actions) {
      if (entry === "-") {
        const divider = document.createElement("div");
        divider.className = "menu-divider";
        divider.setAttribute("role", "separator");
        container.appendChild(divider);
        continue;
      }
      // A custom row builds its own DOM (e.g. the Finder-style colour-tag dots). It owns its own
      // click handling; `hideMenu` is provided so it can dismiss the menu after acting.
      if (typeof entry.custom === "function") {
        const node = entry.custom({ hideMenu });
        if (node) container.appendChild(node);
        continue;
      }
      const { label, action, disabled, checked } = entry;
      const children = Array.isArray(entry.children) ? entry.children : [];
      const wrapper = document.createElement("div");
      wrapper.className = children.length ? "menu-entry menu-entry-has-children" : "menu-entry";
      const button = document.createElement("button");
      button.className = "menu-item";
      button.type = "button";
      button.title = String(label || "");
      const labelNode = document.createElement("span");
      labelNode.className = "menu-item-label";
      labelNode.textContent = label;
      button.appendChild(labelNode);
      if (checked) {
        const markNode = document.createElement("span");
        markNode.className = "menu-item-mark";
        markNode.textContent = "\u2713";
        button.appendChild(markNode);
      }
      if (children.length) {
        const arrowNode = document.createElement("span");
        arrowNode.className = "menu-item-mark menu-item-arrow";
        arrowNode.textContent = "\u203a";
        button.appendChild(arrowNode);
        const openSubmenu = () => {
          closeSiblingSubmenus(wrapper);
          wrapper.dataset.open = "true";
        };
        button.addEventListener("pointerenter", openSubmenu);
        button.addEventListener("mouseenter", openSubmenu);
        button.addEventListener("focus", openSubmenu);
      }
      const runnable = typeof action === "function";
      button.disabled = !!disabled || (!children.length && !runnable);
      button.addEventListener("click", (event) => {
        event.stopPropagation();
        if (children.length) {
          closeSiblingSubmenus(wrapper);
          wrapper.dataset.open = "true";
          return;
        }
        if (button.disabled || !runnable) return;
        hideMenu();
        Promise.resolve(action()).catch(showError);
      });
      wrapper.appendChild(button);
      if (children.length) {
        const submenu = document.createElement("div");
        submenu.className = "menu-submenu";
        renderMenuEntries(submenu, children);
        wrapper.appendChild(submenu);
        wrapper.addEventListener("mouseenter", () => {
          closeSiblingSubmenus(wrapper);
          wrapper.dataset.open = "true";
        });
        wrapper.addEventListener("focusin", () => {
          closeSiblingSubmenus(wrapper);
          wrapper.dataset.open = "true";
        });
      }
      container.appendChild(wrapper);
    }
  }

  function closeSiblingSubmenus(wrapper) {
    for (const entry of wrapper.parentElement?.querySelectorAll(":scope > .menu-entry[data-open='true']") || []) {
      if (entry !== wrapper) entry.dataset.open = "false";
    }
  }

  function countMenuActions(actions) {
    return actions.reduce((count, entry) => {
      if (entry === "-") return count;
      return count + 1 + countMenuActions(Array.isArray(entry.children) ? entry.children : []);
    }, 0);
  }

  function hideMenu() {
    for (const entry of contextMenu.querySelectorAll(".menu-entry[data-open='true']")) {
      entry.dataset.open = "false";
    }
    contextMenu.classList.add("hidden");
  }

  return {
    hideMenu,
    menuAction,
    renderMenu,
  };
}
