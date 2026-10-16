/* Soft-keyboard relay. Only the top-level document's visual viewport shrinks
   for the soft keyboard; a framed document's visual viewport is its layout
   viewport, so the framed shell cannot measure it. The host page is the one
   top-level document: it measures the covered height and tells the active
   shell (`home:keyboard-inset`), which ends its stage above it. Presentation
   only — no authority. See docs/HOME_MOBILE.md. */

export const KEYBOARD_INSET_MESSAGE = "home:keyboard-inset";
export const FORM_FACTOR_MESSAGE = "home:form-factor";

// The base viewport tag stays as it is off a phone. On a phone stage (the
// shell's committed form factor, `home:form-factor`) the page runs edge to
// edge and the soft keyboard resizes only the visual viewport, which the
// relay below measures.
export const PHONE_VIEWPORT_EXTRA = ", viewport-fit=cover, interactive-widget=resizes-visual";

export function phoneViewportContent(content, phone) {
  const base = String(content || "").split(PHONE_VIEWPORT_EXTRA).join("");
  return phone ? `${base}${PHONE_VIEWPORT_EXTRA}` : base;
}

export function applyPhoneViewport(phone, doc = document) {
  const meta = doc.querySelector('meta[name="viewport"]');
  const content = meta?.getAttribute("content");
  const next = phoneViewportContent(content, phone === true);
  if (meta && next !== content) {
    meta.setAttribute("content", next);
  }
}

// Height of the soft keyboard (or any other visual-viewport occlusion) in CSS
// px. The layout viewport keeps its height while the visual viewport shrinks
// and may scroll; the difference is what covers the bottom of the page. A
// pinch-zoomed viewport also shrinks without anything covering the page, so
// while zoomed there is no inset to report.
export function keyboardInset(view = window) {
  const visual = view.visualViewport;
  if (!visual || !Number.isFinite(visual.height) || !Number.isFinite(view.innerHeight)) {
    return 0;
  }
  if (Number.isFinite(visual.scale) && visual.scale > 1.01) {
    return 0;
  }
  const offsetTop = Number.isFinite(visual.offsetTop) ? visual.offsetTop : 0;
  return Math.max(0, Math.round(view.innerHeight - visual.height - offsetTop));
}

// Posts the inset whenever it changes, and again when the shell frame loads
// (a reloaded shell starts from 0). `post` returns false while no shell can
// take it, so the next change or load retries. Returns an unbind.
export function bindKeyboardInsetRelay({ post, frame = null, view = window }) {
  let delivered = null;
  const relay = () => {
    const inset = keyboardInset(view);
    if (inset !== delivered && post({ type: KEYBOARD_INSET_MESSAGE, inset })) {
      delivered = inset;
    }
  };
  const handleFrameLoad = () => {
    delivered = null;
    relay();
  };
  view.addEventListener("resize", relay);
  view.visualViewport?.addEventListener("resize", relay);
  view.visualViewport?.addEventListener("scroll", relay);
  frame?.addEventListener("load", handleFrameLoad);
  relay();
  return () => {
    view.removeEventListener("resize", relay);
    view.visualViewport?.removeEventListener("resize", relay);
    view.visualViewport?.removeEventListener("scroll", relay);
    frame?.removeEventListener("load", handleFrameLoad);
  };
}
