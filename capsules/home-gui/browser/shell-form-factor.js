/* Form factor: the one place the shell decides whether it is on a phone, a
   tablet or a desktop, and whether the pointer is coarse. Presentation only —
   no authority. CSS reads the same facts from body[data-form-factor] and
   body[data-pointer]; the soft-keyboard height the host page relays lands on
   --keyboard-inset so the stage can shrink above it. See docs/HOME_MOBILE.md. */

// Size classes (CSS px). A width at or below PHONE_MAX_WIDTH is a phone; up to
// TABLET_MAX_WIDTH is a tablet; anything wider is the desktop.
export const PHONE_MAX_WIDTH = 640;
export const TABLET_MAX_WIDTH = 1100;

export const FORM_FACTORS = Object.freeze({
  phone: "phone",
  tablet: "tablet",
  desktop: "desktop",
});

// A phone held sideways is still a phone: 844 × 390 with a coarse pointer
// keeps the phone layout instead of turning into a tablet. A short desktop
// window with a mouse does not, so height only counts on coarse pointers.
export function classifyFormFactor(width, height = Number.POSITIVE_INFINITY, coarse = false) {
  if (!Number.isFinite(width)) {
    return FORM_FACTORS.desktop;
  }
  if (width <= PHONE_MAX_WIDTH) {
    return FORM_FACTORS.phone;
  }
  if (coarse && Number.isFinite(height) && height <= PHONE_MAX_WIDTH) {
    return FORM_FACTORS.phone;
  }
  if (width <= TABLET_MAX_WIDTH) {
    return FORM_FACTORS.tablet;
  }
  return FORM_FACTORS.desktop;
}

export function formFactor(view = window) {
  return classifyFormFactor(view.innerWidth, view.innerHeight, isCoarsePointer(view));
}

export function isPhone(view = window) {
  return formFactor(view) === FORM_FACTORS.phone;
}

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

// Takes the host's measurement; anything but a finite number clears it.
export function relayKeyboardInset(inset, doc = document, view = window) {
  relayedKeyboardInset = Number.isFinite(inset) ? Math.max(0, Math.round(inset)) : 0;
  return syncFormFactor(doc, view);
}

export function syncFormFactor(doc = document, view = window) {
  const factor = formFactor(view);
  const coarse = isCoarsePointer(view);
  doc.body.dataset.formFactor = factor;
  doc.body.dataset.pointer = coarse ? "coarse" : "fine";
  // Desktop pinch-zoom also shrinks the visual viewport; only coarse-pointer
  // hosts have a soft keyboard, so the inset is theirs alone.
  const inset = coarse ? relayedKeyboardInset : 0;
  doc.documentElement.style.setProperty("--keyboard-inset", `${inset}px`);
  return { factor, coarse, inset };
}

// Keeps the body dataset and --keyboard-inset current. Returns an unbind.
export function bindFormFactor(doc = document, view = window) {
  const sync = () => syncFormFactor(doc, view);
  sync();
  view.addEventListener("resize", sync);
  return () => {
    view.removeEventListener("resize", sync);
  };
}
