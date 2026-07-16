/* GENERATED from capsules/_shared/elastos-theme.js — do not edit. Run `just vendor-ui`. */
/* ElastOS UI — shared theme runtime (single source of truth).
 *
 * Vendored into each participating capsule's browser/ dir by `just vendor-ui`,
 * next to elastos-ui.css. Everything is served same-origin from the gateway,
 * so localStorage is the one canonical store (mirrors PC2's approach):
 *
 *   localStorage["elastos.ui.theme"] = "dark" | "light" | "auto"
 *
 * Dark is the default when unset. "auto" follows prefers-color-scheme.
 * The resolved theme is applied as data-el-theme="light" on <html> (absence
 * means dark), which activates the light token block in elastos-ui.css.
 * Cross-document sync is the browser's own `storage` event — no shell
 * messaging required, and sandboxed app frames (allow-same-origin) see it.
 */
(function () {
  const KEY = "elastos.ui.theme";
  const media = window.matchMedia
    ? window.matchMedia("(prefers-color-scheme: light)")
    : null;

  function preference() {
    try {
      const value = localStorage.getItem(KEY);
      if (value === "light" || value === "dark" || value === "auto") {
        return value;
      }
    } catch (_error) {
      // Storage may be unavailable; fall through to the default.
    }
    return "dark";
  }

  function resolve(pref) {
    if (pref === "auto") {
      return media && media.matches ? "light" : "dark";
    }
    return pref;
  }

  function apply() {
    if (resolve(preference()) === "light") {
      document.documentElement.setAttribute("data-el-theme", "light");
    } else {
      document.documentElement.removeAttribute("data-el-theme");
    }
  }

  apply();

  window.addEventListener("storage", (event) => {
    if (event.key === KEY || event.key === null) {
      apply();
    }
  });

  if (media && typeof media.addEventListener === "function") {
    media.addEventListener("change", () => {
      if (preference() === "auto") {
        apply();
      }
    });
  }

  window.elastosTheme = {
    preference,
    resolved: () => resolve(preference()),
    set(value) {
      const pref = value === "light" || value === "auto" ? value : "dark";
      try {
        localStorage.setItem(KEY, pref);
      } catch (_error) {
        // Applying still works for this document.
      }
      apply();
    },
  };
})();
