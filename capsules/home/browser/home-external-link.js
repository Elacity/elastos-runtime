// Home opens an app's web link or shared file after the person sees the exact address
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

const SHARED_RESULT_SCHEMA = "elastos.home.shared-content.result/v1";
const CHECK_STATES = ["local", "available", "sender_offline", "not_contact", "not_here"];
const GET_STATES = ["local", "sender_offline", "not_shared", "too_large", "mismatch", "timeout"];

function validatedSharedResult(result, uri) {
  if (!result || typeof result !== "object" || Array.isArray(result)
    || Object.keys(result).some(key => !["schema", "check_id", "cid", "uri", "sender_display_name", "state", "size_bytes"].includes(key))
    || result.schema !== SHARED_RESULT_SCHEMA || result.uri !== uri
    || typeof result.cid !== "string" || !result.cid || uri !== `elastos://${result.cid}`
    || !CHECK_STATES.includes(result.state)
    || (["available", "sender_offline"].includes(result.state) && (typeof result.check_id !== "string" || !result.check_id))
    || (result.sender_display_name !== undefined && typeof result.sender_display_name !== "string")
    || (result.size_bytes !== undefined && (!Number.isSafeInteger(result.size_bytes) || result.size_bytes < 0))) return null;
  return result;
}

// The launch context is verified by Home before this boundary. Runtime owns
// CID and message validation; Home preserves the exact request and binding.
export function requestHomeSharedContent({ context, data, prompt, fetchJson }) {
  const source = data.source;
  const keys = ["type", "uri", "homeToken", ...(source === undefined ? [] : ["source"])];
  if (context.kind !== "app-frame" || context.targetId !== "chat-room"
    || !hasExactKeys(data, keys) || data.type !== "home:open-uri"
    || data.homeToken !== context.homeToken || typeof data.uri !== "string"
    || !data.uri.startsWith("elastos://") || data.uri.length > MAX_EXTERNAL_LINK_LENGTH
    || (source !== undefined && (!source
      || typeof source !== "object" || Array.isArray(source)
      || !hasExactKeys(source, ["conversation_id", "message_id"])
      || ![source.conversation_id, source.message_id].every(value => typeof value === "string" && value.length > 0)))) return false;
  const check = signal => fetchJson("/api/apps/home/shared-content/check", {
    method: "POST", signal,
    body: JSON.stringify({ schema: "elastos.home.shared-content.check/v1", app_token: context.homeToken, uri: data.uri,
      ...(source === undefined ? {} : { source }) }),
  });
  const get = (check_id, signal) => fetchJson("/api/apps/home/shared-content/get", {
    method: "POST", signal, body: JSON.stringify({ check_id }),
  });
  const requestId = prompt.begin({ homeToken: context.homeToken, appId: context.targetId,
    kind: "shared", uri: data.uri, check, get });
  if (!requestId) return false;
  check().then(result => prompt.show(requestId, result)).catch(() => prompt.checkFailed(requestId));
  return true;
}

// Read only the verified local gateway after the person chooses Open/Get.
// Raw CID responses use application/octet-stream, so inspect a bounded sample.
export async function openHomeSharedFile({ cid, uri }, { openTarget, fetchLocal = globalThis.fetch, signal } = {}) {
  const response = await fetchLocal(`/content/${encodeURIComponent(cid)}`, { cache: "no-store", signal });
  if (!response.ok) throw new Error("Could not read the local file");
  const reader = response.body.getReader();
  const sample = new Uint8Array(4096);
  let length = 0, complete = false;
  try {
    while (length < sample.length) {
      const { value, done } = await reader.read();
      if (done) { complete = true; break; }
      const part = value.subarray(0, sample.length - length);
      sample.set(part, length);
      length += part.length;
    }
  } finally {
    await reader.cancel();
  }
  const bytes = sample.subarray(0, length);
  let text = false;
  try {
    const decoded = new TextDecoder("utf-8", { fatal: true }).decode(bytes, { stream: !complete });
    text = !/[\x00-\x08\x0b\x0e-\x1f\x7f]/.test(decoded) && !decoded.startsWith("%PDF-");
  } catch (_) { /* Binary content opens in Library. */ }
  if (signal?.aborted) return;
  return openTarget(text ? "documents" : "library", {
    query: { cid, ...(text ? { uri, view: "read" } : {}) },
  });
}

export function createHomeExternalLinkPrompt({
  root,
  appName,
  host,
  address,
  openButton,
  cancelButton,
  eyebrow, title, asker, size, status, linkLabel,
  openSharedFile = async () => {},
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
  let pointerPress = null;
  let keyboardPress = null;
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
    pointerPress = null;
    keyboardPress = null;
    request?.controller?.abort();
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

  // One short sheet: a title naming the sender, at most one quiet line, and
  // the buttons. The address stays out of view; Home already checked it.
  function renderShared(request) {
    const value = request.result;
    const sender = value?.sender_display_name || "";
    const name = sender || "The sender";
    const state = request.state;
    if (eyebrow) eyebrow.hidden = true;
    if (appName.parentElement) appName.parentElement.hidden = true;
    appName.textContent = "";
    if (asker) asker.textContent = "";
    host.hidden = true;
    host.textContent = "";
    address.hidden = true;
    address.textContent = "";
    if (linkLabel) linkLabel.hidden = true;
    title.textContent = state === "local" ? "Open this file?"
      : state === "not_contact" ? "Only contacts can send you files"
      : sender ? `Get this file from ${sender}?` : "Get this file?";
    const ready = ["local", "available", "getting"].includes(state);
    size.textContent = ready && value?.size_bytes !== undefined ? formatSize(value.size_bytes) : "";
    size.hidden = !size.textContent;
    const messages = {
      sender_offline: `${name} is offline. Try again later.`,
      not_contact: `Add ${sender || "this person"} as a contact to get this file.`,
      not_here: "This file isn't on your Home.",
      not_shared: `${name} no longer shares this file.`,
      too_large: "This file is too large to get.",
      mismatch: "This file didn't match its link, so Home discarded it.",
      timeout: "This took too long. Try again.",
      check_failed: "Home could not check this file. Try again.",
      get_failed: "Home could not get this file. Try again.",
      open_failed: "Home could not open this file. Try again.",
    };
    status.textContent = messages[state] || "";
    status.hidden = !status.textContent;
    openButton.textContent = state === "local" ? "Open" : state === "available" ? "Get"
      : state === "getting" ? "Getting…" : state === "checking" ? "Checking…" : "Try again";
    const actionable = ["local", "available", "sender_offline", "timeout", "check_failed", "get_failed", "open_failed"].includes(state);
    openButton.hidden = !actionable && !request.busy;
    openButton.disabled = request.busy || !actionable;
    cancelButton.textContent = actionable || request.busy ? "Cancel" : "OK";
  }

  async function activateShared(request) {
    const state = request.state;
    request.busy = true;
    request.controller = new AbortController();
    const current = () => active === request && isLive(request.homeToken) && !request.controller.signal.aborted;
    try {
      if (state === "local") {
        await openSharedFile(request.result, request.controller.signal);
        if (current()) close();
        return;
      }
      request.state = state === "available" ? "getting" : "checking";
      renderShared(request);
      if (state === "available") {
        const result = await request.get(request.result.check_id, request.controller.signal);
        if (!current()) return;
        if (!result || !hasExactKeys(result, ["state"]) || !GET_STATES.includes(result.state)) throw new Error("Invalid file result");
        request.state = result.state;
        if (result.state === "local") {
          await openSharedFile(request.result, request.controller.signal);
          if (current()) close();
          return;
        }
      } else {
        const result = validatedSharedResult(await request.check(request.controller.signal), request.uri);
        if (!current()) return;
        if (!result) throw new Error("Invalid file check");
        request.result = result;
        request.state = result.state === "sender_offline" ? "available" : result.state;
      }
    } catch (_error) {
      if (!current()) return;
      request.state = state === "local" || request.state === "local" ? "open_failed"
        : state === "available" ? "get_failed" : "check_failed";
    }
    if (current()) {
      request.busy = false;
      request.shownAt = now();
      renderShared(request);
    }
  }

  function eligiblePress(event) {
    return event?.isTrusted === true
      && active?.phase === "shown"
      && now() - active.shownAt >= EXTERNAL_LINK_OPEN_GUARD_MS;
  }

  // A press anywhere on the sheet records when the gesture began: iOS may
  // move a tap that lands just outside Open link onto it.
  root.addEventListener("pointerdown", (event) => {
    keyboardPress = null;
    pointerPress = {
      requestId: active?.requestId,
      pointerId: event.pointerId,
      pointerType: event.pointerType,
      eligible: eligiblePress(event) && event.button === 0 && event.isPrimary !== false,
    };
  }, true);
  root.addEventListener("pointercancel", () => { pointerPress = null; }, true);
  openButton.addEventListener("keydown", (event) => {
    if (event.key !== "Enter" && event.key !== " ") return;
    if (event.repeat) {
      // A repeat cannot begin a new gesture; retain an initial Space press.
      event.preventDefault();
      return;
    }
    pointerPress = null;
    keyboardPress = {
      requestId: active?.requestId,
      key: event.key,
      eligible: eligiblePress(event),
    };
    if (!keyboardPress.eligible) event.preventDefault();
  });
  openButton.addEventListener("keyup", (event) => {
    if (event.key !== "Enter" && event.key !== " ") return;
    const press = keyboardPress;
    if (event.key === " " && (
      !press?.eligible || press.key !== event.key || press.requestId !== active?.requestId
    )) event.preventDefault();
  });
  windowRef.addEventListener("keyup", (event) => {
    if (event.key !== "Enter" && event.key !== " ") return;
    const press = keyboardPress;
    // Retain the decision through Space's native keyup click; discard it
    // afterward so a later assistive-technology click can stand on its own.
    windowRef.setTimeout?.(() => { if (keyboardPress === press) keyboardPress = null; }, 0);
  }, true);

  openButton.addEventListener("click", (event) => {
    // Each click consumes the press that began it. A touch tap's click can
    // arrive several tasks after pointerup (iOS), so nothing expires it early.
    const press = pointerPress;
    pointerPress = null;
    // A synthetic click never opens; only the person's own press does, and
    // only once the sheet has been on screen for the guard time.
    if (
      event?.isTrusted !== true ||
      active?.phase !== "shown" ||
      now() - active.shownAt < EXTERNAL_LINK_OPEN_GUARD_MS ||
      ((event.detail > 0 || event.pointerType) && (
        !press?.eligible || press.requestId !== active.requestId ||
        // Only touch ids carry from a touch press to its click: WebKit sends
        // a tap's click as a mouse click, and Chromium gives a pen tap's click
        // a new id.
        (event.pointerType === "touch" && press.pointerType === "touch" && event.pointerId !== press.pointerId)
      )) ||
      (keyboardPress && (
        !keyboardPress.eligible || keyboardPress.requestId !== active.requestId
      ))
    ) {
      return;
    }
    if (active.kind === "shared") {
      if (!active.busy && isLive(active.homeToken)) void activateShared(active);
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
      const next = !openButton.disabled && windowRef.document?.activeElement === cancelButton ? openButton : cancelButton;
      next.focus({ preventScroll: true });
    }
  });
  hide();

  return {
    // Claims the one request slot for this sender. Returns "" while another
    // request is open or being checked, or during the sender's cool-down.
    begin({ homeToken, appId, kind = "web", uri, check, get }) {
      if (active || !homeToken || !appId || (cooldownUntil.get(appId) || 0) > now()) {
        return "";
      }
      cooldownUntil.delete(appId);
      sequence += 1;
      const requestId = `external-link-${sequence}`;
      active = { requestId, homeToken, appId, kind, uri, check, get, phase: "checking", href: "", busy: false };
      return requestId;
    },
    // Shows the Runtime-checked request. Anything stale or malformed, or a
    // request that would stack on another Home sheet, is dropped unshown.
    show(requestId, result) {
      if (active?.requestId !== requestId || active.phase !== "checking") {
        return false;
      }
      const checked = isLive(active.homeToken) && !otherSheetShown()
        ? (active.kind === "shared" ? validatedSharedResult(result, active.uri) : validatedResult(result, active.appId, windowRef.location?.origin))
        : null;
      if (!checked) {
        active = null;
        return false;
      }
      pointerPress = null;
      keyboardPress = null;
      active.phase = "shown";
      active.shownAt = now();
      if (active.kind === "shared") {
        active.result = checked;
        active.state = checked.state === "sender_offline" ? "available" : checked.state;
        renderShared(active);
      } else {
        active.href = checked.href;
        if (eyebrow) { eyebrow.hidden = false; eyebrow.textContent = "Web link"; }
        if (title) title.textContent = "Open this link?";
        if (appName.parentElement) appName.parentElement.hidden = false;
        if (asker) asker.textContent = " wants to open this address in your browser.";
        appName.textContent = checked.appTitle;
        host.hidden = false;
        host.textContent = checked.host;
        address.hidden = false;
        address.textContent = checked.href;
        if (linkLabel) linkLabel.hidden = true;
        if (size) size.hidden = true;
        if (status) status.hidden = true;
        openButton.textContent = "Open link";
        cancelButton.textContent = "Cancel";
        openButton.hidden = false;
        openButton.disabled = false;
      }
      root.hidden = false;
      root.setAttribute("aria-hidden", "false");
      cancelButton.focus({ preventScroll: true });
      return true;
    },
    checkFailed(requestId) {
      if (active?.requestId !== requestId || active.kind !== "shared" || !isLive(active.homeToken) || otherSheetShown()) {
        if (active?.requestId === requestId) close();
        return;
      }
      active.phase = "shown";
      active.state = "check_failed";
      active.shownAt = now();
      renderShared(active);
      root.hidden = false;
      root.setAttribute("aria-hidden", "false");
      cancelButton.focus({ preventScroll: true });
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

function formatSize(bytes) {
  if (bytes < 1024) return `${bytes} bytes`;
  const power = bytes < 1024 ** 2 ? 1 : bytes < 1024 ** 3 ? 2 : 3;
  const scaled = bytes * 10 / 1024 ** power;
  const floor = Math.floor(scaled);
  // Rust's post formatter rounds exact halves to the even decimal.
  const rounded = scaled - floor === 0.5 ? floor + floor % 2 : Math.round(scaled);
  return `${(rounded / 10).toFixed(1)} ${['', 'KB', 'MB', 'GB'][power]}`;
}
