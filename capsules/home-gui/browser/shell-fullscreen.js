/* Top-bar full screen toggle. The browser Fullscreen API owns the state:
 * the button only asks for it and follows `fullscreenchange`, so Esc and the
 * browser's own exits keep the label and icon right. */

function fullscreenElement(doc) {
  return doc.fullscreenElement || doc.webkitFullscreenElement || null;
}

function fullscreenApi(doc) {
  const root = doc.documentElement;
  const request = root.requestFullscreen || root.webkitRequestFullscreen;
  const exit = doc.exitFullscreen || doc.webkitExitFullscreen;
  return { root, request, exit };
}

/** Binds `button` to document full screen; returns null when the browser
 * has no Fullscreen API (e.g. iPhone Safari) and the button stays hidden. */
export function bindFullscreenToggle(button, doc = document) {
  if (!button || typeof fullscreenApi(doc).request !== "function") {
    return null;
  }

  function sync() {
    const active = Boolean(fullscreenElement(doc));
    const text = active ? "Exit full screen" : "Enter full screen";
    button.setAttribute("aria-pressed", active ? "true" : "false");
    button.setAttribute("aria-label", text);
    button.title = text;
  }

  async function toggle() {
    const { root, request, exit } = fullscreenApi(doc);
    try {
      if (fullscreenElement(doc)) {
        if (typeof exit === "function") {
          await exit.call(doc);
        }
      } else if (typeof request === "function") {
        await request.call(root);
      }
    } catch {
      // Fail closed and resync from the real document state so the control
      // stays retryable.
    } finally {
      sync();
    }
  }

  button.hidden = false;
  doc.addEventListener("fullscreenchange", sync);
  doc.addEventListener("webkitfullscreenchange", sync);
  button.addEventListener("click", () => {
    void toggle();
  });
  sync();
  return { sync, toggle };
}
