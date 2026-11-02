/* Form factor: the one place the shell decides whether it is on a phone, a
   tablet or a desktop, and whether the pointer is coarse. Presentation only —
   no authority. CSS reads the same facts from body[data-form-factor] and
   body[data-pointer]; the soft-keyboard height the host page relays lands on
   --keyboard-inset so the stage can shrink above it. See docs/HOME_MOBILE.md. */

// Size classes (CSS px). A width at or below PHONE_MAX_WIDTH is a phone; up to
// TABLET_MAX_WIDTH is a tablet; anything wider is the desktop.
export const PHONE_MAX_WIDTH = 640;
export const TABLET_MAX_WIDTH = 1100;
// A landscape phone's viewport is at most ~430 px tall (the largest phones,
// browser bars included); a 7-inch tablet or a short touch-laptop window is
// 550 px or more. Coarse viewports this short are phones held sideways.
export const PHONE_LANDSCAPE_MAX_HEIGHT = 500;

export const FORM_FACTORS = Object.freeze({
  phone: "phone",
  tablet: "tablet",
  desktop: "desktop",
});

// A phone held sideways is still a phone: 844 × 390 with a coarse pointer
// keeps the phone layout instead of turning into a tablet. A short desktop
// window with a mouse does not, so height only counts on coarse pointers, only
// as short as a landscape phone, and only up to tablet width: a 1024 × 600
// tablet or a short window on a wide touch laptop stays tablet or desktop.
export function classifyFormFactor(width, height = Number.POSITIVE_INFINITY, coarse = false) {
  if (!Number.isFinite(width)) {
    return FORM_FACTORS.desktop;
  }
  if (width <= PHONE_MAX_WIDTH) {
    return FORM_FACTORS.phone;
  }
  if (coarse && width <= TABLET_MAX_WIDTH && Number.isFinite(height) && height <= PHONE_LANDSCAPE_MAX_HEIGHT) {
    return FORM_FACTORS.phone;
  }
  if (width <= TABLET_MAX_WIDTH) {
    return FORM_FACTORS.tablet;
  }
  return FORM_FACTORS.desktop;
}

// Live size class of a viewport. The shell itself reads formFactor(), the
// committed one.
export function measuredFormFactor(view = window) {
  return classifyFormFactor(view.innerWidth, view.innerHeight, isCoarsePointer(view));
}

// The form factor is decided at load and changes only through one clean
// stage reset when the viewport crosses the phone boundary (bindFormFactor),
// so no surface ever morphs between phone and desktop while it holds state.
let committed = null;

export function formFactor(view = window) {
  return committed && committed.view === view ? committed.factor : measuredFormFactor(view);
}

export function isPhone(view = window) {
  return formFactor(view) === FORM_FACTORS.phone;
}

// Dispatched on the view whenever the committed form factor changes.
export const FORM_FACTOR_EVENT = "elastos:form-factor";

// Pointer class comes from media features, never from the user agent: a
// tablet with a trackpad is fine-pointer, a touch laptop is coarse.
export function isCoarsePointer(view = window) {
  const matches = (query) => Boolean(view.matchMedia?.(query)?.matches);
  return matches("(pointer: coarse)") || matches("(hover: none)");
}

// Height of the soft keyboard in CSS px. This frame's visual viewport is its
// layout viewport, so it never sees the keyboard; the host page measures it
// and relays it (home:keyboard-inset, home/browser/home-keyboard-inset.js).
let relayedKeyboardInset = 0;

// Takes the host's measurement; anything but a finite number clears it. A
// viewport across the phone boundary is the stage reset's to commit, so the
// inset then follows the committed form factor.
export function relayKeyboardInset(inset, doc = document, view = window) {
  relayedKeyboardInset = Number.isFinite(inset) ? Math.max(0, Math.round(inset)) : 0;
  if (!crossesPhoneBoundary(view)) {
    return syncFormFactor(doc, view);
  }
  const factor = formFactor(view);
  const applied = factor === FORM_FACTORS.phone ? relayedKeyboardInset : 0;
  doc.documentElement.style.setProperty("--keyboard-inset", `${applied}px`);
  return { factor, coarse: isCoarsePointer(view), inset: applied };
}

// Notification Centre and Control Centre are groups on the desktop (base
// semantics) and non-modal sheets on a phone; the clock opens the
// Notification Centre.
const PHONE_DIALOGS = Object.freeze(["#notification-center", "#control-centre"]);

function applyPhoneSemantics(doc, phone) {
  for (const selector of PHONE_DIALOGS) {
    const node = doc.querySelector?.(selector);
    node?.setAttribute("role", phone ? "dialog" : "group");
    if (phone) {
      node?.setAttribute("aria-modal", "false");
    } else {
      node?.removeAttribute("aria-modal");
    }
  }
  doc.querySelector?.("#clock")?.setAttribute("aria-haspopup", phone ? "dialog" : "true");
}

// Commits the viewport's form factor and applies it.
export function syncFormFactor(doc = document, view = window) {
  const previous = committed && committed.view === view ? committed.factor : null;
  const factor = measuredFormFactor(view);
  committed = { view, factor };
  const coarse = isCoarsePointer(view);
  doc.body.dataset.formFactor = factor;
  doc.body.dataset.pointer = coarse ? "coarse" : "fine";
  // Desktop pinch-zoom also shrinks the visual viewport, and tablet and
  // desktop keep the desktop stage unchanged, so only the phone takes the
  // inset.
  const inset = factor === FORM_FACTORS.phone ? relayedKeyboardInset : 0;
  doc.documentElement.style.setProperty("--keyboard-inset", `${inset}px`);
  if (previous !== factor) {
    applyPhoneSemantics(doc, factor === FORM_FACTORS.phone);
    if (typeof view.CustomEvent === "function") {
      view.dispatchEvent(new view.CustomEvent(FORM_FACTOR_EVENT, { detail: { factor, previous } }));
    }
  }
  return { factor, coarse, inset };
}

// Whether the viewport now lies on the other side of the phone boundary from
// the committed form factor.
export function crossesPhoneBoundary(view = window) {
  return (measuredFormFactor(view) === FORM_FACTORS.phone) !== isPhone(view);
}

// Commits the form factor at load and keeps the body dataset and
// --keyboard-inset current. A resize that crosses the phone boundary runs
// `reset(commit)` instead: one clean stage reset that calls commit() itself
// once the old stage is gone. Resizes during a reset wait for it and are
// judged again afterwards. Returns an unbind.
export function bindFormFactor(doc = document, view = window, { reset = null } = {}) {
  let running = false;
  const sync = () => {
    if (running) {
      return;
    }
    if (!reset || !crossesPhoneBoundary(view)) {
      syncFormFactor(doc, view);
      return;
    }
    running = true;
    Promise.resolve()
      .then(() => reset(() => syncFormFactor(doc, view)))
      .catch(() => {})
      .then(() => {
        running = false;
        sync();
      });
  };
  syncFormFactor(doc, view);
  view.addEventListener("resize", sync);
  return () => {
    view.removeEventListener("resize", sync);
  };
}
