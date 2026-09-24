/* Soft-keyboard relay. Only the top-level document's visual viewport shrinks
   for the soft keyboard; a framed document's visual viewport is its layout
   viewport, so the framed shell cannot measure it. The host page is the one
   top-level document: it measures the covered height and tells the active
   shell (`home:keyboard-inset`), which ends its stage above it. Presentation
   only — no authority. See docs/HOME_MOBILE.md. */

export const KEYBOARD_INSET_MESSAGE = "home:keyboard-inset";

// Height of the soft keyboard (or any other visual-viewport occlusion) in CSS
// px. The layout viewport keeps its height while the visual viewport shrinks
// and may scroll; the difference is what covers the bottom of the page.
export function keyboardInset(view = window) {
  const visual = view.visualViewport;
  if (!visual || !Number.isFinite(visual.height) || !Number.isFinite(view.innerHeight)) {
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
