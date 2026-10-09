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

// Home owns frame presentation. Child documents cannot observe an ancestor's
// visibility:hidden state through document.hidden or ordinary intersection.
export function capsuleFrameVisible(frame, doc = document, view = window) {
  if (doc.hidden || !frame.isConnected) return false;
  for (let node = frame; node; node = node.parentElement) {
    if (node.hidden || node.dataset?.spaceVisible === "false") return false;
    const style = view.getComputedStyle(node);
    if (style.display === "none" || style.visibility === "hidden"
      || style.visibility === "collapse" || Number(style.opacity) === 0) return false;
  }
  const rect = frame.getBoundingClientRect();
  return rect.width > 0 && rect.height > 0 && rect.bottom > 0 && rect.right > 0
    && rect.top < view.innerHeight && rect.left < view.innerWidth;
}

function sameLayout(left, right) {
  return left?.formFactor === right?.formFactor && left?.pointer === right?.pointer
    && left?.visible === right?.visible;
}

// Returns an unbind. Attribute observations are limited to shell presentation;
// an unrelated capsule DOM change does not cause another layout message.
export function bindCapsuleLayout(doc = document, view = window) {
  const sent = new WeakMap();
  const postFrame = (frame, force = false) => {
    const layout = { ...shellLayout(view), visible: capsuleFrameVisible(frame, doc, view) };
    if (force || !sameLayout(sent.get(frame), layout)) {
      sent.set(frame, layout);
      postShellLayout(frame.contentWindow, layout);
    }
  };
  const postToAllFrames = () => {
    for (const frame of doc.querySelectorAll("iframe")) postFrame(frame);
  };
  const handleFrameLoad = (event) => {
    if (event.target instanceof view.HTMLIFrameElement) postFrame(event.target, true);
  };
  const handlePresentationChange = (records) => {
    for (const frame of doc.querySelectorAll("iframe")) {
      if (records.some(record => record.target === frame || record.target.contains(frame))) postFrame(frame);
    }
  };
  const handleLayoutRequest = (event) => {
    if (event.data?.type !== SHELL_LAYOUT_MESSAGE || event.data.request !== true) return;
    const frame = [...doc.querySelectorAll("iframe")].find(frame => frame.contentWindow === event.source);
    if (frame) postFrame(frame, true);
  };
  const handleTransitionEnd = (event) => handlePresentationChange([{ target: event.target }]);
  const observer = new view.MutationObserver(handlePresentationChange);
  observer.observe(doc.documentElement, {
    subtree: true, childList: true, attributes: true,
    attributeFilter: ["class", "style", "hidden", "data-space-visible", "data-stage-active", "data-fullscreen-stage", "data-active-stage", "data-stage-kind", "data-space-sliding", "data-space-closing"],
  });
  doc.addEventListener("load", handleFrameLoad, true);
  doc.addEventListener("visibilitychange", postToAllFrames);
  doc.addEventListener("transitionend", handleTransitionEnd, true);
  view.addEventListener("resize", postToAllFrames);
  view.addEventListener("message", handleLayoutRequest);
  postToAllFrames();
  return () => {
    observer.disconnect();
    doc.removeEventListener("load", handleFrameLoad, true);
    doc.removeEventListener("visibilitychange", postToAllFrames);
    doc.removeEventListener("transitionend", handleTransitionEnd, true);
    view.removeEventListener("resize", postToAllFrames);
    view.removeEventListener("message", handleLayoutRequest);
  };
}
