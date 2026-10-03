/* Capsule layout: tells every capsule frame which size class the shell is in,
   so a capsule can drop its desktop window-chrome safe areas and switch to its
   phone layout. A capsule cannot tell a phone stage from a narrow desktop
   window by its own width: only the shell knows whether traffic lights sit
   over the frame. Presentation only — no authority, nothing a capsule could
   misuse. The shared theme runtime (capsules/_shared/elastos-theme.js) applies
   it as html[data-el-form-factor] and html[data-el-pointer]. */

import { formFactor, isCoarsePointer } from "./shell-form-factor.js?v=home-20260813a";

export const SHELL_LAYOUT_MESSAGE = "elastos:shell-layout";

export function shellLayout(view = window) {
  return {
    formFactor: formFactor(view),
    pointer: isCoarsePointer(view) ? "coarse" : "fine",
  };
}

export function postShellLayout(frameWindow, layout) {
  try {
    frameWindow?.postMessage({ type: SHELL_LAYOUT_MESSAGE, layout }, "*");
  } catch (_error) {
    // Frame mid-teardown; its next load gets the layout again.
  }
}

function sameLayout(left, right) {
  return left?.formFactor === right?.formFactor && left?.pointer === right?.pointer;
}

// Sends the layout to each iframe as it loads (load events do not bubble, so
// the listener is capture-phase) and to all of them when the size class
// changes. Returns an unbind.
export function bindCapsuleLayout(doc = document, view = window) {
  let current = shellLayout(view);
  const handleFrameLoad = (event) => {
    if (event.target instanceof view.HTMLIFrameElement) {
      postShellLayout(event.target.contentWindow, current);
    }
  };
  const postToAllFrames = () => {
    for (const frame of doc.querySelectorAll("iframe")) {
      postShellLayout(frame.contentWindow, current);
    }
  };
  const handleResize = () => {
    const next = shellLayout(view);
    if (sameLayout(next, current)) {
      return;
    }
    current = next;
    postToAllFrames();
  };
  doc.addEventListener("load", handleFrameLoad, true);
  view.addEventListener("resize", handleResize);
  // Frames that finished loading before the shell bound this still need it.
  postToAllFrames();
  return () => {
    doc.removeEventListener("load", handleFrameLoad, true);
    view.removeEventListener("resize", handleResize);
  };
}
