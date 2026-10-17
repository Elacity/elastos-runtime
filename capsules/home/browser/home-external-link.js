// Home opens an app's web link only after the person sees the exact address
// and chooses Open link (#430). App documents have no popup permission.
//
// One request at a time. The Runtime names the requesting app from its signed
// launch and returns the one validated address; this sheet shows and opens that
// same string. Only a trusted click on Home's own Open link button opens it,
// synchronously in that click, with no opener and no referrer. The app gets no
// reply, so it cannot learn what the person chose.

export const HOME_OPEN_EXTERNAL_TYPE = "home:open-external";
export const EXTERNAL_LINK_REQUEST_SCHEMA = "elastos.home.external-link.request/v1";
const EXTERNAL_LINK_RESULT_SCHEMA = "elastos.home.external-link.result/v1";
export const MAX_EXTERNAL_LINK_LENGTH = 2048;
const MAX_APP_TITLE_LENGTH = 128;
// After Cancel, the same app's link requests are ignored for this long, so an
// app cannot loop the sheet.
export const EXTERNAL_LINK_CANCEL_COOLDOWN_MS = 3_000;
// Open link ignores presses for this long after the sheet appears, so an app
// cannot time the sheet under a click or key press the person already started.
// A key press activates the button through the same click.
export const EXTERNAL_LINK_OPEN_GUARD_MS = 500;

function hasExactKeys(value, keys) {
  const actual = Object.keys(value).sort();
  const expected = [...keys].sort();
  return actual.length === expected.length
    && actual.every((key, index) => key === expected[index]);
}

// The Runtime result, accepted only when it names the asking app and its
// address is already the canonical http(s) form this browser would open.
function validatedResult(result, appId, homeOrigin) {
  if (
    !result || typeof result !== "object" || Array.isArray(result)
    || !hasExactKeys(result, ["schema", "app_id", "app_title", "url"])
    || result.schema !== EXTERNAL_LINK_RESULT_SCHEMA
    || result.app_id !== appId
    || typeof result.app_title !== "string"
    || !result.app_title.trim()
    || result.app_title.length > MAX_APP_TITLE_LENGTH
    || typeof result.url !== "string"
    || result.url.length > MAX_EXTERNAL_LINK_LENGTH
  ) {
    return null;
  }
  let parsed;
  try {
    parsed = new URL(result.url);
  } catch (_error) {
    return null;
  }
  if (
    (parsed.protocol !== "https:" && parsed.protocol !== "http:")
    || parsed.username
    || parsed.password
    || !parsed.hostname
    || parsed.href !== result.url
    || parsed.origin === homeOrigin
  ) {
    return null;
  }
  return { appTitle: result.app_title.trim(), href: parsed.href, host: parsed.host };
}

export function createHomeExternalLinkPrompt({
  root,
  appName,
  host,
  address,
  openButton,
  cancelButton,
  windowRef = globalThis.window,
  isLive = () => true,
  // True while another Home sheet (the Clipboard prompt) is on screen; the two
  // never stack, so this request is dropped.
  otherSheetShown = () => false,
  // Monotonic, so a wall-clock change cannot shorten the guard or cool-down.
  now = () => windowRef.performance.now(),
  cooldownMs = EXTERNAL_LINK_CANCEL_COOLDOWN_MS,
} = {}) {
  if (!root || !appName || !host || !address || !openButton || !cancelButton || !windowRef) {
    throw new TypeError("Home link sheet nodes are required");
  }
  let active = null;
  let sequence = 0;
  const cooldownUntil = new Map();

  function hide() {
    root.hidden = true;
    root.setAttribute("aria-hidden", "true");
    openButton.disabled = true;
    appName.textContent = "";
    host.textContent = "";
    address.textContent = "";
  }

  function close() {
    const request = active;
    active = null;
    if (request?.phase === "shown") {
      hide();
    }
    return request;
  }

  function cancel() {
    if (active?.phase !== "shown") {
      return;
    }
    const request = close();
    cooldownUntil.set(request.appId, now() + cooldownMs);
  }

  openButton.addEventListener("click", (event) => {
    // A synthetic click never opens; only the person's own press does, and
    // only once the sheet has been on screen for the guard time.
    if (
      event?.isTrusted !== true ||
      active?.phase !== "shown" ||
      now() - active.shownAt < EXTERNAL_LINK_OPEN_GUARD_MS
    ) {
      return;
    }
    const request = close();
    if (!isLive(request.homeToken)) {
      return;
    }
    // Synchronous in the click: Safari keeps the user activation.
    windowRef.open(request.href, "_blank", "noopener,noreferrer");
  });
  cancelButton.addEventListener("click", cancel);
  root.addEventListener("click", (event) => {
    if (event?.target === root) {
      cancel();
    }
  });
  windowRef.addEventListener("keydown", (event) => {
    if (active?.phase !== "shown") {
      return;
    }
    if (event?.key === "Escape") {
      event.preventDefault?.();
      cancel();
    } else if (event?.key === "Tab") {
      // Focus stays on the sheet's two buttons while it is open.
      event.preventDefault?.();
      const next = windowRef.document?.activeElement === cancelButton ? openButton : cancelButton;
      next.focus({ preventScroll: true });
    }
  });
  hide();

  return {
    // Claims the one request slot for this sender. Returns "" while another
    // request is open or being checked, or during the sender's cool-down.
    begin({ homeToken, appId }) {
      if (active || !homeToken || !appId || (cooldownUntil.get(appId) || 0) > now()) {
        return "";
      }
      cooldownUntil.delete(appId);
      sequence += 1;
      const requestId = `external-link-${sequence}`;
      active = { requestId, homeToken, appId, phase: "checking", href: "" };
      return requestId;
    },
    // Shows the Runtime-checked request. Anything stale or malformed, or a
    // request that would stack on another Home sheet, is dropped unshown.
    show(requestId, result) {
      if (active?.requestId !== requestId || active.phase !== "checking") {
        return false;
      }
      const checked = isLive(active.homeToken) && !otherSheetShown()
        ? validatedResult(result, active.appId, windowRef.location?.origin)
        : null;
      if (!checked) {
        active = null;
        return false;
      }
      active.phase = "shown";
      active.shownAt = now();
      active.href = checked.href;
      appName.textContent = checked.appTitle;
      host.textContent = checked.host;
      address.textContent = checked.href;
      root.hidden = false;
      root.setAttribute("aria-hidden", "false");
      openButton.disabled = false;
      cancelButton.focus({ preventScroll: true });
      return true;
    },
    abandon(requestId) {
      if (active?.requestId === requestId) {
        close();
      }
    },
    // The sender's launch ended (closed, reloaded, reconnected): its request
    // closes without opening anything.
    retire(homeToken) {
      if (active?.homeToken === homeToken) {
        close();
      }
    },
    clear() {
      close();
    },
  };
}
