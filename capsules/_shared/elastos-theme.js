/* ElastOS UI — shared theme runtime (single source of truth).
 *
 * Vendored into each participating capsule's browser/ dir by `just vendor-ui`,
 * next to elastos-ui.css. Everything is served same-origin from the gateway,
 * so localStorage is the one canonical store (mirrors PC2's approach):
 *
 *   localStorage["elastos.ui.theme"]  = "dark" | "light" | "auto"
 *   localStorage["elastos.ui.accent"] = "blue" | "purple" | "pink" | "red" |
 *                                       "orange" | "yellow" | "green" | "graphite"
 *
 * Dark is the default theme when unset; blue is the default accent. "auto"
 * follows prefers-color-scheme. The resolved theme is applied as
 * data-el-theme="light" on <html> (absence means dark); the accent as
 * data-el-accent (absence means blue) — both activate token blocks in
 * elastos-ui.css. Cross-document sync is the browser's own `storage` event —
 * no shell messaging required, and sandboxed app frames (allow-same-origin)
 * see it.
 */
(function () {
  const KEY = "elastos.ui.theme";
  const ACCENT_KEY = "elastos.ui.accent";
  const ACCENTS = [
    "blue",
    "purple",
    "pink",
    "red",
    "orange",
    "yellow",
    "green",
    "graphite",
  ];
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

  function accentPreference() {
    try {
      const value = localStorage.getItem(ACCENT_KEY);
      if (ACCENTS.includes(value)) {
        return value;
      }
    } catch (_error) {
      // Storage may be unavailable; fall through to the default.
    }
    return "blue";
  }

  function apply() {
    if (resolve(preference()) === "light") {
      document.documentElement.setAttribute("data-el-theme", "light");
    } else {
      document.documentElement.removeAttribute("data-el-theme");
    }
    const accent = accentPreference();
    if (accent === "blue") {
      document.documentElement.removeAttribute("data-el-accent");
    } else {
      document.documentElement.setAttribute("data-el-accent", accent);
    }
  }

  apply();

  window.addEventListener("storage", (event) => {
    if (event.key === KEY || event.key === ACCENT_KEY || event.key === null) {
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
    accents: ACCENTS.slice(),
    accent: accentPreference,
    setAccent(value) {
      const accent = ACCENTS.includes(value) ? value : "blue";
      try {
        localStorage.setItem(ACCENT_KEY, accent);
      } catch (_error) {
        // Applying still works for this document.
      }
      apply();
    },
  };
})();
