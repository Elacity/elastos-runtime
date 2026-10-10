/* ElastOS UI — phone push drawer (the Assistant's saved-chats pattern).
 *
 * On the phone stage (html[data-el-form-factor="phone"], see elastos-theme.js)
 * a capsule's sidebar leaves the layout and becomes a drawer: the sidebar
 * slides in from the leading edge and the main view slides aside with a
 * rounded edge; tapping that view, Escape, or picking an item closes it. On
 * tablet and desktop this module does nothing and the sidebar stays a column.
 *
 * Markup contract (no capsule JS needed):
 *   <aside id="x" data-el-drawer>                     the sidebar
 *   <main data-el-drawer-room="x">                    the view it pushes aside
 *   <button data-el-drawer-toggle="x">                opens / closes it
 *   [data-el-drawer-close] inside the drawer          closes after a tap
 * Enter in a search field inside the drawer also closes it so the results are
 * visible. State lands as data-el-drawer-state="open|closed" on the drawer and
 * its room; layout and motion live in elastos-ui.css.
 * Vendored by `just vendor-ui` to the capsules that carry a sidebar.
 */
(function () {
  const OPEN = "open";
  const CLOSED = "closed";

  function onPhone() {
    return document.documentElement.getAttribute("data-el-form-factor") === "phone";
  }

  function bindDrawer(drawer) {
    const id = drawer.id;
    const room = document.querySelector(`[data-el-drawer-room="${id}"]`);
    const toggles = Array.from(document.querySelectorAll(`[data-el-drawer-toggle="${id}"]`));
    if (!id || !room || toggles.length === 0) {
      return;
    }
    let open = false;

    function render() {
      const phone = onPhone();
      const state = phone && open ? OPEN : CLOSED;
      drawer.setAttribute("data-el-drawer-state", state);
      room.setAttribute("data-el-drawer-state", state);
      // An off-screen drawer must not take focus or speak; off the phone the
      // sidebar is a normal column again.
      drawer.inert = phone && !open;
      for (const toggle of toggles) {
        toggle.setAttribute("aria-controls", id);
        toggle.setAttribute("aria-expanded", String(phone && open));
      }
    }

    function setOpen(next, { restoreFocus = false } = {}) {
      open = Boolean(next) && onPhone();
      render();
      if (open) {
        drawer.focus({ preventScroll: true });
      } else if (restoreFocus) {
        toggles[0].focus({ preventScroll: true });
      }
    }

    if (!drawer.hasAttribute("tabindex")) {
      drawer.setAttribute("tabindex", "-1");
    }
    for (const toggle of toggles) {
      toggle.addEventListener("click", () => setOpen(!open));
    }
    // A tap on the pushed-aside view closes the drawer and goes no further,
    // so it never also activates whatever sat under the finger.
    room.addEventListener(
      "click",
      (event) => {
        if (!open || toggles.some((toggle) => toggle.contains(event.target))) {
          return;
        }
        event.preventDefault();
        event.stopPropagation();
        setOpen(false);
      },
      true,
    );
    drawer.addEventListener("click", (event) => {
      if (open && event.target.closest?.("[data-el-drawer-close]")) {
        setOpen(false);
      }
    });
    drawer.addEventListener("keydown", (event) => {
      if (!open) {
        return;
      }
      if (event.key === "Escape") {
        event.preventDefault();
        setOpen(false, { restoreFocus: true });
        return;
      }
      if (event.key === "Enter" && event.target.matches?.('input[type="search"]')) {
        setOpen(false);
      }
    });
    new MutationObserver(() => {
      if (!onPhone()) {
        open = false;
      }
      render();
    }).observe(document.documentElement, { attributes: true, attributeFilter: ["data-el-form-factor"] });
    render();
  }

  function bindAll() {
    document.querySelectorAll("[data-el-drawer]").forEach(bindDrawer);
  }

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", bindAll, { once: true });
  } else {
    bindAll();
  }
})();
